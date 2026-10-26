//! Keeping the index current: the watcher, the dirty set, and the gate every query passes
//! (`tasks/task-2138-unluminous-code-index-tdd.md` §6.4).
//!
//! **Events are hints, not truth.** A file event marks the file and its folder dirty; an event on a
//! folder, or a watcher buffer overflow, marks a whole tree. Nothing is reindexed on the event itself.
//!
//! **The gate is what makes a query fresh.** Before a query reads the index, every dirty folder is
//! listed again with the same walk the index was built with, and every file in it whose size, time or
//! content changed is read again. A file the agent wrote a moment ago is therefore searched as it now is.
//!
//! **The fence closes the last gap.** The operating system queues a file's event when the write
//! finishes, but the watcher's thread reads it a little later. So the gate writes a fence file of its
//! own and waits for that file's event: events are delivered in order, so once the fence's arrives every
//! event queued before it has arrived too. The fence goes inside `.git` when that is a folder, where git
//! ignores unknown files and the index never looks.
//!
//! **A root with no `.git` keeps its fence in the index's own folder** (Unluminous `task-2370`). It used to
//! go into the root itself, so every query put a file into the person's project for up to 250 ms: a sync
//! client, a build watcher or another editor saw it come and go, a listing could count it and a copy
//! could fail on it. `<index folder>/fence` is added to the same watcher instead. Linux reads every watch
//! through one inotify descriptor and macOS through one FSEvents stream, so the order holds there as it
//! did. Windows gives each watched folder a `ReadDirectoryChangesW` handle of its own, and a root batch
//! the watcher's thread has not reached yet can be delivered after the fence. So with the fence outside
//! the root the gate notes how many root events had been delivered before it wrote the fence, and writes
//! another when any arrived between the write and the fence's own event. A round with none in between
//! shows every root event from before the write was delivered: a root read that completed before the
//! write is queued ahead of the fence's and runs first, and a root read that was not pending is reissued
//! by a completion that also runs first, so its change is either delivered in that round or counted.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::exact::{Exact, FileRecord};
use crate::files::{self, VCS_FOLDERS};

/// How long the gate waits for its fence's event before going on anyway and reconciling everything.
const FENCE_WAIT: Duration = Duration::from_millis(250);
/// The name a fence file starts with.
const FENCE_PREFIX: &str = "atrius-index-fence-";

/// What the watcher has seen since the gate last ran.
#[derive(Default)]
struct Dirty {
    /// Files named by an event, relative to the root. Read again and compared by content.
    files: HashSet<String>,
    /// Folders whose direct children may have changed.
    folders: HashSet<String>,
    /// Folders whose whole tree may have changed: a folder was created, renamed or removed.
    trees: HashSet<String>,
    /// The watcher lost events, so everything must be reconciled.
    overflow: bool,
}

/// The fence: the last sequence number whose event has arrived, and how many root events have.
#[derive(Default)]
struct FenceSeen {
    seen: Mutex<u64>,
    arrived: Condvar,
    /// Events delivered for paths that are not fences. Read only when the fence is outside the root.
    root_events: AtomicU64,
}

/// The watcher and the dirty set of one root.
pub struct Freshness {
    root: PathBuf,
    dirty: Arc<Mutex<Dirty>>,
    fence: Arc<FenceSeen>,
    fence_folder: PathBuf,
    /// Whether the fence folder is outside the root, watched through a handle of its own.
    fence_apart: bool,
    next_fence: AtomicU64,
    _watcher: Option<RecommendedWatcher>,
}

/// What the gate changed, for the store and for `search status`.
#[derive(Debug, Default)]
pub struct GateReport {
    /// Files read again and added or replaced: `(new id, record, trigrams)`.
    pub changed: Vec<(u32, FileRecord, Vec<Vec<u32>>)>,
    /// Ids tombstoned.
    pub removed: Vec<u32>,
    /// Paths that are no longer in the set at all.
    pub removed_paths: Vec<String>,
    /// Whether the whole tree was reconciled.
    pub reconciled_all: bool,
    /// Whether the fence's event arrived in time.
    pub fenced: bool,
    /// How long the gate took.
    pub took: Duration,
}

impl Freshness {
    /// Starts watching a root. With no watcher available (an unsupported file system), every gate
    /// reconciles the whole tree, which is slow and still correct.
    ///
    /// @param root - the root
    pub fn start(root: &Path) -> Freshness {
        let dirty = Arc::new(Mutex::new(Dirty { overflow: false, ..Dirty::default() }));
        let fence = Arc::new(FenceSeen::default());
        let git = root.join(".git");
        let apart = if git.is_dir() { None } else { own_fence_folder(root) };
        let (watcher, fence_apart) =
            match watch(root, apart.as_deref(), Arc::clone(&dirty), Arc::clone(&fence)) {
                Some((watcher, watched_apart)) => (Some(watcher), watched_apart),
                None => (None, false),
            };
        if watcher.is_none() {
            dirty.lock().expect("dirty").overflow = true;
        }
        let fence_folder = match apart {
            _ if git.is_dir() => git,
            Some(folder) if fence_apart => folder,
            // The index's folder could not be made or watched: the root is the one place left whose
            // events are certain to be in order with the root's own.
            _ => root.to_path_buf(),
        };
        Freshness {
            root: root.to_path_buf(),
            dirty,
            fence,
            fence_folder,
            fence_apart,
            next_fence: AtomicU64::new(1),
            _watcher: watcher,
        }
    }

    /// Where the fence files are written: `.git`, the index's own `fence` folder, or the root.
    pub fn fence_folder(&self) -> &Path {
        &self.fence_folder
    }

    /// Whether anything is waiting to be reconciled.
    pub fn pending(&self) -> usize {
        let d = self.dirty.lock().expect("dirty");
        d.files.len() + d.folders.len() + d.trees.len() + usize::from(d.overflow)
    }

    /// Marks the whole tree for reconciling, as after a restart.
    pub fn mark_everything(&self) {
        self.dirty.lock().expect("dirty").overflow = true;
    }

    /// Writes fences until every event queued before now has been seen, or the time runs out. A fence in
    /// the root's own watch needs one; a fence watched apart needs a round with no root event delivered
    /// between its write and its arrival.
    fn fence(&self) -> bool {
        let deadline = Instant::now() + FENCE_WAIT;
        loop {
            let before = self.fence.root_events.load(Ordering::SeqCst);
            if !self.one_fence(deadline) {
                return false;
            }
            if !self.fence_apart || self.fence.root_events.load(Ordering::SeqCst) == before {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
        }
    }

    /// Writes one fence file and waits for its event until the deadline.
    ///
    /// @param deadline - when to stop waiting
    fn one_fence(&self, deadline: Instant) -> bool {
        let seq = self.next_fence.fetch_add(1, Ordering::SeqCst);
        let path = self.fence_folder.join(format!("{FENCE_PREFIX}{seq}"));
        if std::fs::write(&path, b"").is_err() {
            return false;
        }
        let mut seen = self.fence.seen.lock().expect("fence");
        while *seen < seq {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            seen = self.fence.arrived.wait_timeout(seen, left).expect("fence").0;
        }
        let arrived = *seen >= seq;
        drop(seen);
        let _ = std::fs::remove_file(&path);
        arrived
    }

    /// The gate: brings the index up to date with everything that changed before this call.
    ///
    /// @param exact - the in-memory index
    pub fn gate(&self, exact: &mut Exact) -> GateReport {
        let started = Instant::now();
        let fenced = self.fence();
        let dirty = std::mem::take(&mut *self.dirty.lock().expect("dirty"));
        let mut report = GateReport { fenced, ..GateReport::default() };
        if dirty.overflow || !fenced {
            reconcile(&self.root, &self.root, None, exact, &HashSet::new(), &mut report);
            report.reconciled_all = true;
        } else {
            for tree in &dirty.trees {
                reconcile(
                    &self.root,
                    &self.root.join(tree),
                    None,
                    exact,
                    &dirty.files,
                    &mut report,
                );
            }
            for folder in
                dirty.folders.iter().filter(|f| !dirty.trees.iter().any(|t| is_under(f, t)))
            {
                reconcile(
                    &self.root,
                    &self.root.join(folder),
                    Some(1),
                    exact,
                    &dirty.files,
                    &mut report,
                );
            }
        }
        report.took = started.elapsed();
        report
    }
}

/// Whether a path is a tree or inside it.
///
/// @param path - a relative path
/// @param tree - a relative folder, empty for the root
fn is_under(path: &str, tree: &str) -> bool {
    tree.is_empty()
        || path == tree
        || (path.starts_with(tree) && path.as_bytes().get(tree.len()) == Some(&b'/'))
}

/// Lists a folder (to a depth) with the index's walk and makes the index agree with it: files gone are
/// tombstoned, files new or changed by size or time are read again, and files an event named are read
/// again and compared by content even when their size and time did not move.
///
/// @param root - the root
/// @param start - the folder to list
/// @param depth - Some(1) for the folder's direct children, None for its whole tree
/// @param exact - the index
/// @param named - files an event named
/// @param report - what changed is added here
fn reconcile(
    root: &Path,
    start: &Path,
    depth: Option<usize>,
    exact: &mut Exact,
    named: &HashSet<String>,
    report: &mut GateReport,
) {
    let prefix = files::relative(root, start).unwrap_or_default();
    // A path that is a file now (an editor saving by rename reports a removal and a creation) is listed
    // through its folder, so the ignore rules still decide whether it belongs to the set.
    let listed = if start.is_dir() {
        files::walk_under(root, start, depth)
    } else if start.is_file() {
        let folder = start.parent().unwrap_or(root);
        files::walk_under(root, folder, Some(1)).into_iter().filter(|f| f.rel == prefix).collect()
    } else {
        Vec::new()
    };
    let listed_paths: HashSet<&str> = listed.iter().map(|f| f.rel.as_str()).collect();
    let held: Vec<(String, u32)> = exact
        .by_path
        .iter()
        .filter(|(rel, _)| {
            is_under(rel, &prefix)
                && (depth.is_none() || !rel[prefix.len()..].trim_start_matches('/').contains('/'))
        })
        .map(|(rel, &id)| (rel.clone(), id))
        .collect();
    for (rel, id) in &held {
        if !listed_paths.contains(rel.as_str()) {
            exact.remove(rel);
            report.removed.push(*id);
            report.removed_paths.push(rel.clone());
        }
    }
    for found in &listed {
        let current =
            exact.by_path.get(&found.rel).and_then(|&id| exact.files[id as usize].as_ref());
        let stat_changed =
            current.is_none_or(|r| r.size != found.size || r.mtime_ns != found.mtime_ns);
        if !stat_changed && !named.contains(&found.rel) {
            continue;
        }
        let Ok(reading) = crate::exact::read_bounded(&root.join(&found.rel)) else { continue };
        let (record, trigrams) =
            FileRecord::from_reading(found.rel.clone(), found.mtime_ns, reading);
        if let Some(r) = current {
            if !stat_changed && r.hash == record.hash {
                continue;
            }
            report.removed.push(exact.by_path[&found.rel]);
        }
        exact.insert(record.clone(), &trigrams);
        report.changed.push((exact.files.len() as u32 - 1, record, trigrams));
    }
}

/// The index's own fence folder for a root, made and emptied of fences an earlier process left. None when
/// it cannot be made, or when it is inside the root, where it would be a folder in the person's project.
///
/// @param root - the root
fn own_fence_folder(root: &Path) -> Option<PathBuf> {
    let folder = crate::store::index_folder(root).join("fence");
    std::fs::create_dir_all(&folder).ok()?;
    let folder =
        std::fs::canonicalize(&folder).map(|p| crate::index::strip_verbatim(&p)).unwrap_or(folder);
    if folder.starts_with(root) {
        return None;
    }
    for entry in std::fs::read_dir(&folder).ok()?.flatten() {
        if entry.file_name().to_string_lossy().starts_with(FENCE_PREFIX) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Some(folder)
}

/// Starts the operating system watcher on a root, recursively, and on a fence folder outside it. The fence
/// folder is watched first: FSEvents restarts its stream for each folder added, and nothing of the root
/// can be lost while only the fence folder is watched. Answers the watcher and whether the fence folder
/// is watched; None when the root cannot be.
///
/// @param root - the root
/// @param apart - a fence folder outside the root, if there is one
/// @param dirty - where events are recorded
/// @param fence - where fence events are recorded
fn watch(
    root: &Path,
    apart: Option<&Path>,
    dirty: Arc<Mutex<Dirty>>,
    fence: Arc<FenceSeen>,
) -> Option<(RecommendedWatcher, bool)> {
    let base = root.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        let Ok(event) = result else {
            dirty.lock().expect("dirty").overflow = true;
            return;
        };
        if event.need_rescan() {
            dirty.lock().expect("dirty").overflow = true;
        }
        record_event(&base, &event, &dirty, &fence);
    })
    .ok()?;
    let watched_apart =
        apart.is_some_and(|folder| watcher.watch(folder, RecursiveMode::NonRecursive).is_ok());
    watcher.watch(root, RecursiveMode::Recursive).ok()?;
    Some((watcher, watched_apart))
}

/// Records one event's paths in the dirty set, and a fence file's creation as arrived.
///
/// @param root - the root
/// @param event - the event
/// @param dirty - the dirty set
/// @param fence - the fence
fn record_event(root: &Path, event: &notify::Event, dirty: &Mutex<Dirty>, fence: &FenceSeen) {
    let mut d = dirty.lock().expect("dirty");
    for path in &event.paths {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(seq) = name.strip_prefix(FENCE_PREFIX).and_then(|s| s.parse::<u64>().ok()) {
            if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any) {
                let mut seen = fence.seen.lock().expect("fence");
                *seen = (*seen).max(seq);
                fence.arrived.notify_all();
            }
            continue;
        }
        fence.root_events.fetch_add(1, Ordering::SeqCst);
        let Some(rel) = files::relative(root, path) else { continue };
        if rel.split('/').any(|part| VCS_FOLDERS.contains(&part)) {
            continue;
        }
        let parent = rel.rsplit_once('/').map(|(p, _)| p.to_owned()).unwrap_or_default();
        let is_folder = path.is_dir()
            || matches!(
                event.kind,
                EventKind::Create(notify::event::CreateKind::Folder)
                    | EventKind::Remove(notify::event::RemoveKind::Folder)
            );
        if is_folder
            || matches!(
                event.kind,
                EventKind::Modify(notify::event::ModifyKind::Name(_)) | EventKind::Remove(_)
            )
        {
            d.trees.insert(rel.clone());
        }
        d.files.insert(rel);
        d.folders.insert(parent);
    }
}
