//! Freshness on macOS without a fence: one kqueue with a vnode filter on every file of the set and every
//! folder the walk visits (`task-2696`, round 1, from the advice in `task-2838`).
//!
//! **Why this proves freshness and FSEvents cannot.** FSEvents goes from the kernel to `fseventsd` and
//! then to the watcher, about 11 ms on an M6, and nothing a client can call shows whether `fseventsd` has
//! seen a write yet: `FSEventStreamFlushSync` returned before the event of a write made just before it in
//! 500 of 500 tries. A kqueue vnode note is posted by the writer's own system call, before `write`,
//! `rename`, `unlink` or `truncate` returns to the writer. So when the gate drains the kqueue with a zero
//! timeout, every change made before the query is already there. A probe with the writer in another
//! process saw 500 of 500 for each of an in place write, an append, a truncate, a write through `mmap` and
//! `msync`, a create, a delete, a rename over a file, a folder rename, `clonefile` and `RENAME_SWAP`, and
//! the drain took 11 microseconds.
//!
//! **A note says only that something changed.** A file's note marks the file dirty; a folder's
//! `NOTE_WRITE` marks the folder dirty, so the gate lists its direct children as it does for an FSEvents
//! hint, and any child folder not registered yet is new and is reconciled as a tree. A file or folder
//! that is deleted or renamed away is forgotten and its path reconciled as a tree, which removes it.
//!
//! **Arming.** Every folder and file is opened with `O_EVTONLY` on a thread of its own after the watcher
//! starts. Until it finishes, the gate fences as before. The first gate after it fences once more: that
//! fence covers every change from before the last registration, and every change after a registration is
//! in the kqueue, so from then on the gate drains the kqueue alone. A file the gate reads is registered
//! after it is read and checked by size and time once more, so a write between the two is caught.
//!
//! **Where it is not used.** A root on a volume that is not local, a root whose files and folders need
//! more descriptors than half of what one process may open, and every root when `ATRIUS_KQUEUE=off`. The
//! FSEvents watcher runs for every root either way, so falling back to the fence at any time loses nothing.

use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Arming: descriptors are still being opened.
const ARMING: u8 = 0;
/// Every descriptor is open; one more fence makes the kqueue the proof.
const CONFIRMING: u8 = 1;
/// The kqueue alone proves freshness.
const ACTIVE: u8 = 2;
/// Not used for this root.
const OFF: u8 = 3;

/// The notes asked for on every file and folder.
const NOTES: u32 = libc::NOTE_WRITE
    | libc::NOTE_EXTEND
    | libc::NOTE_ATTRIB
    | libc::NOTE_DELETE
    | libc::NOTE_RENAME
    | libc::NOTE_REVOKE
    | libc::NOTE_LINK;

/// Descriptors open for events in this process, across every root, so one host of several roots keeps
/// to one budget.
static OPEN: AtomicUsize = AtomicUsize::new(0);

/// What one note said about one path.
#[derive(Debug)]
pub struct Change {
    /// The path relative to the root, empty for the root itself.
    pub rel: String,
    /// Whether the path is a folder.
    pub folder: bool,
    /// Whether it was deleted, renamed away or revoked, so its descriptor is closed.
    pub gone: bool,
}

/// The registered descriptors, both ways round.
#[derive(Default)]
struct Registry {
    by_fd: HashMap<i32, (String, bool)>,
    by_rel: HashMap<String, i32>,
}

/// The kqueue of one root.
pub struct Kqueue {
    fd: i32,
    root: PathBuf,
    budget: usize,
    state: AtomicU8,
    registry: Mutex<Registry>,
}

impl Drop for Kqueue {
    fn drop(&mut self) {
        self.close_all();
        // SAFETY: the kqueue descriptor belongs to this value and is closed once.
        unsafe { libc::close(self.fd) };
    }
}

impl Kqueue {
    /// A kqueue for a root, armed on a thread of its own, or None where it is not used.
    ///
    /// @param root - the root
    pub fn start(root: &Path) -> Option<Arc<Kqueue>> {
        let off = std::env::var("ATRIUS_KQUEUE").is_ok_and(|v| v.trim().eq_ignore_ascii_case("off"));
        if off || !is_local(root) {
            return None;
        }
        // SAFETY: kqueue takes no arguments and returns a descriptor or -1.
        let fd = unsafe { libc::kqueue() };
        if fd < 0 {
            return None;
        }
        let kq = Arc::new(Kqueue {
            fd,
            root: root.to_path_buf(),
            budget: descriptor_budget(),
            state: AtomicU8::new(ARMING),
            registry: Mutex::new(Registry::default()),
        });
        let arming = Arc::clone(&kq);
        let started = std::thread::Builder::new().name("atrius-index-kqueue".into()).spawn(move || arming.arm_all());
        started.is_ok().then_some(kq)
    }

    /// Whether the kqueue alone proves freshness now.
    pub fn is_active(&self) -> bool {
        self.state.load(Ordering::SeqCst) == ACTIVE
    }

    /// Whether every descriptor is open and one fence is all that is left before the kqueue is the proof.
    pub fn is_confirming(&self) -> bool {
        self.state.load(Ordering::SeqCst) == CONFIRMING
    }

    /// Whether the kqueue is still used for this root at all.
    pub fn is_on(&self) -> bool {
        self.state.load(Ordering::SeqCst) != OFF
    }

    /// Makes the kqueue the proof, after the fence that followed arming arrived.
    pub fn confirm(&self) {
        let _ = self.state.compare_exchange(CONFIRMING, ACTIVE, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// Opens a descriptor for every folder and file of the set, then waits for the confirming fence.
    fn arm_all(&self) {
        let folders = crate::files::walk_folders(&self.root, &self.root, None);
        let files = crate::files::walk(&self.root);
        if OPEN.load(Ordering::SeqCst) + folders.len() + files.len() > self.budget {
            self.turn_off();
            return;
        }
        for folder in &folders {
            self.register(folder, true);
        }
        for file in &files {
            self.register(&file.rel, false);
        }
        let _ = self.state.compare_exchange(ARMING, CONFIRMING, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// Stops using the kqueue for this root and closes every descriptor. The gate fences from then on.
    pub fn turn_off(&self) {
        self.state.store(OFF, Ordering::SeqCst);
        self.close_all();
    }

    /// Closes every registered descriptor.
    fn close_all(&self) {
        let mut registry = self.registry.lock().expect("registry");
        for fd in registry.by_fd.keys() {
            // SAFETY: each descriptor in the registry was opened here and is closed once, here.
            unsafe { libc::close(*fd) };
            OPEN.fetch_sub(1, Ordering::SeqCst);
        }
        registry.by_fd.clear();
        registry.by_rel.clear();
    }

    /// Whether a path has a descriptor.
    ///
    /// @param rel - the path relative to the root
    pub fn is_registered(&self, rel: &str) -> bool {
        self.registry.lock().expect("registry").by_rel.contains_key(rel)
    }

    /// Opens a path with `O_EVTONLY` and adds its vnode filter, once. Over the budget the kqueue is turned
    /// off. Answers whether the path is registered now.
    ///
    /// @param rel - the path relative to the root, empty for the root
    /// @param folder - whether it is a folder
    pub fn register(&self, rel: &str, folder: bool) -> bool {
        if !self.is_on() {
            return false;
        }
        let mut registry = self.registry.lock().expect("registry");
        if registry.by_rel.contains_key(rel) {
            return true;
        }
        if OPEN.load(Ordering::SeqCst) >= self.budget {
            drop(registry);
            self.turn_off();
            return false;
        }
        let Some(fd) = open_for_events(&self.root.join(rel)) else { return false };
        if !add_filter(self.fd, fd) {
            // SAFETY: the descriptor was opened just above and is not in the registry.
            unsafe { libc::close(fd) };
            return false;
        }
        OPEN.fetch_add(1, Ordering::SeqCst);
        registry.by_fd.insert(fd, (rel.to_owned(), folder));
        registry.by_rel.insert(rel.to_owned(), fd);
        true
    }

    /// Closes the descriptors of a path and of everything under it.
    ///
    /// @param rel - the path relative to the root
    pub fn forget_under(&self, rel: &str) {
        let mut registry = self.registry.lock().expect("registry");
        let prefix = format!("{rel}/");
        let gone: Vec<(String, i32)> = registry
            .by_rel
            .iter()
            .filter(|(path, _)| rel.is_empty() || path.as_str() == rel || path.starts_with(&prefix))
            .map(|(path, fd)| (path.clone(), *fd))
            .collect();
        for (path, fd) in gone {
            registry.by_rel.remove(&path);
            registry.by_fd.remove(&fd);
            // SAFETY: the descriptor was in the registry, so it is open, and it is closed once.
            unsafe { libc::close(fd) };
            OPEN.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Every note posted since the last drain, read with a zero timeout until none is left. A path that
    /// is gone has its descriptor closed here.
    pub fn drain(&self) -> Vec<Change> {
        let mut out = Vec::new();
        let zero = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        loop {
            // SAFETY: an array of plain C structs, filled by the kernel up to the count it returns.
            let mut events: [libc::kevent; 256] = unsafe { std::mem::zeroed() };
            // SAFETY: the kqueue is open, no change list is passed, and the event list holds 256 entries.
            let n = unsafe { libc::kevent(self.fd, std::ptr::null(), 0, events.as_mut_ptr(), 256, &zero) };
            if n <= 0 {
                return out;
            }
            for event in &events[..n as usize] {
                if let Some(change) = self.change_of(event) {
                    out.push(change);
                }
            }
        }
    }

    /// The change one note describes, closing the descriptor of a path that is gone.
    ///
    /// @param event - the note
    fn change_of(&self, event: &libc::kevent) -> Option<Change> {
        let fd = event.ident as i32;
        let gone = event.fflags & (libc::NOTE_DELETE | libc::NOTE_RENAME | libc::NOTE_REVOKE) != 0;
        let (rel, folder) = self.registry.lock().expect("registry").by_fd.get(&fd).cloned()?;
        if gone {
            self.forget_under(&rel);
        }
        Some(Change { rel, folder, gone })
    }

    /// The child folders of a folder that have no descriptor yet, in the set's walk: new folders.
    ///
    /// @param rel - the folder relative to the root
    pub fn new_child_folders(&self, rel: &str) -> Vec<String> {
        let start = self.root.join(rel);
        crate::files::walk_folders(&self.root, &start, Some(1))
            .into_iter()
            .filter(|child| child != rel && !self.is_registered(child))
            .collect()
    }
}

/// Opens a path for its events only, closed on exec.
///
/// @param path - the path
fn open_for_events(path: &Path) -> Option<i32> {
    let text = CString::new(path.as_os_str().to_string_lossy().as_bytes()).ok()?;
    // SAFETY: a valid C string; the descriptor is owned by the registry from here on.
    let fd = unsafe { libc::open(text.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) };
    (fd >= 0).then_some(fd)
}

/// Adds the vnode filter for a descriptor to the kqueue.
///
/// @param kq - the kqueue
/// @param fd - the descriptor
fn add_filter(kq: i32, fd: i32) -> bool {
    let event = libc::kevent {
        ident: fd as usize,
        filter: libc::EVFILT_VNODE,
        flags: libc::EV_ADD | libc::EV_CLEAR,
        fflags: NOTES,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    // SAFETY: one change, no event list, on an open kqueue.
    unsafe { libc::kevent(kq, &event, 1, std::ptr::null_mut(), 0, std::ptr::null()) == 0 }
}

/// Whether a root is on a local volume, where kqueue sees every change. A network volume's changes made
/// by another machine post no note.
///
/// @param root - the root
fn is_local(root: &Path) -> bool {
    let Ok(text) = CString::new(root.as_os_str().to_string_lossy().as_bytes()) else { return false };
    // SAFETY: statfs fills the struct for a valid C string path.
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    unsafe { libc::statfs(text.as_ptr(), &mut stat) == 0 && stat.f_flags & libc::MNT_LOCAL as u32 != 0 }
}

/// How many descriptors this process may hold for events, over every root: half of what it may open,
/// after raising the soft limit to the hard one, so the search's own reads have room.
fn descriptor_budget() -> usize {
    let per_process = sysctl_number("kern.maxfilesperproc").unwrap_or(10_240);
    // SAFETY: getrlimit and setrlimit read and write a plain struct.
    let mut limit: libc::rlimit = unsafe { std::mem::zeroed() };
    unsafe {
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) == 0 {
            let wanted = limit.rlim_max.min(per_process as u64);
            if limit.rlim_cur < wanted {
                limit.rlim_cur = wanted;
                libc::setrlimit(libc::RLIMIT_NOFILE, &limit);
                libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit);
            }
        }
    }
    (limit.rlim_cur.min(per_process as u64) / 2) as usize
}

/// A number from `sysctl`.
///
/// @param name - the name
fn sysctl_number(name: &str) -> Option<usize> {
    let text = CString::new(name).ok()?;
    let mut value: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: reads one integer into a buffer of its size.
    let ok = unsafe {
        libc::sysctlbyname(text.as_ptr(), (&mut value as *mut libc::c_int).cast(), &mut size, std::ptr::null_mut(), 0) == 0
    };
    (ok && value > 0).then_some(value as usize)
}
