//! One checkout's index, as the host holds it: the in-memory engines, the watcher, and the store thread.
//!
//! Opening an index starts the watcher first, so nothing written while the index loads is missed, then
//! loads the stored index (or builds one) on the store's own thread. Until that finishes, a query is
//! answered by the direct scan and says so with `"index":"none"`. After it, the first query's gate
//! reconciles the whole tree by size and time, because the files may have changed while no host was
//! running.
//!
//! The store's `Database` cannot leave the thread that opened it, so that thread owns it for the life of
//! the index and is sent each batch of changes the gates made, written in one short transaction.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::direct;
use crate::exact::{Exact, ExactAnswer, ExactRequest, FileRecord};
use crate::freshness::{Freshness, GateReport};
use crate::passages::{self, PassageHit};
use crate::store::{self, Store};
use crate::symbols::{self, SymbolTable};
use inillucent_driver::Database;

/// A job for the store thread.
enum StoreJob {
    Changes {
        changed: Vec<(u32, FileRecord, Vec<Vec<u32>>)>,
        removed: Vec<u32>,
        removed_paths: Vec<String>,
        next_id: usize,
        /// Each changed file's definitions and imports, as the gate read them.
        symbols: Vec<(String, Vec<crate::outline::Definition>, Vec<crate::structure::Import>)>,
    },
    /// A whole index to write, made by compaction, so the store thread writes it without any lock.
    Full { index: Box<Exact> },
    Search {
        question: String,
        k: usize,
        kind: Option<String>,
        reply: Sender<Result<Vec<PassageHit>, String>>,
    },
}

/// What `search status` reports.
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// Whether the index is loaded and answering, rather than the direct scan.
    pub ready: bool,
    /// How the index came to be: `loaded`, `built`, or `building`.
    pub origin: String,
    /// How long loading or building took.
    pub load_ms: u128,
    /// How many times a gate ran.
    pub gates: u64,
    /// How many files gates have read again.
    pub reindexed: u64,
    /// The last store error, if any.
    pub store_error: Option<String>,
    /// How many chunks the passage table holds, once it is built.
    pub passages: usize,
    /// Whether the passage table is built, so passage search can answer.
    pub passages_ready: bool,
    /// How many chunks have a vector.
    pub vectors: usize,
    /// Whether `embed()` can run on this machine.
    pub can_embed: bool,
    /// Why the vectors are not being made although `embed()` can run, when that is so.
    pub not_embedding: Option<String>,
    /// Whether the passage table's search index is held in memory, which it is from a plain English
    /// question until the questions have stopped for a while (Unluminous `task-2279`).
    pub search_index_held: bool,
    /// The index file.
    pub file: PathBuf,
}

/// One checkout's index.
pub struct Index {
    root: PathBuf,
    exact: Arc<RwLock<Option<Exact>>>,
    symbols: Arc<RwLock<Option<SymbolTable>>>,
    freshness: Freshness,
    gate_lock: Mutex<()>,
    last_gate: Mutex<Duration>,
    compacting: Arc<std::sync::atomic::AtomicBool>,
    store: Sender<StoreJob>,
    status: Arc<Mutex<Status>>,
}

impl Index {
    /// Opens the index of a root: starts the watcher, then loads or builds on the store thread. Once it
    /// is ready, a background gate reconciles the whole tree, so the first query does not pay for it.
    ///
    /// @param root - the checkout's root
    pub fn open(root: &Path) -> Arc<Index> {
        let index = Arc::new(Index::open_quietly(root));
        let warm = Arc::clone(&index);
        std::thread::Builder::new()
            .name("atrius-index-warm".into())
            .spawn(move || {
                if warm.wait_ready(Duration::from_secs(3600)) {
                    warm.gate();
                }
            })
            .expect("the warm up thread starts");
        index
    }

    /// Opens the index without the background reconcile; the first gate does it instead.
    ///
    /// @param root - the checkout's root
    pub fn open_quietly(root: &Path) -> Index {
        Index::open_with(root, true)
    }

    /// Opens an index that is held in memory alone: nothing is read from or written to the index file,
    /// there is no passage table, and plain English questions are answered from their words.
    ///
    /// For a program that wants the definitions and must not touch the person's cache, such as an editor
    /// a test builds (Unluminous `task-2231` §5.2), and for one that is not the checkout's host.
    ///
    /// @param root - the checkout's root
    pub fn open_in_memory(root: &Path) -> Arc<Index> {
        Arc::new(Index::open_with(root, false))
    }

    /// Opens an index, with or without its file.
    ///
    /// @param root - the checkout's root
    /// @param on_disk - whether the index file is read and written
    fn open_with(root: &Path, on_disk: bool) -> Index {
        let root = std::fs::canonicalize(root)
            .map(|p| strip_verbatim(&p))
            .unwrap_or_else(|_| root.to_path_buf());
        let freshness = Freshness::start(&root);
        let exact = Arc::new(RwLock::new(None));
        let status = Arc::new(Mutex::new(Status {
            origin: "building".into(),
            file: store::index_folder(&root).join("index.rdb"),
            ..Status::default()
        }));
        let symbols = Arc::new(RwLock::new(None));
        let (tx, rx) = channel::<StoreJob>();
        let (thread_root, thread_exact, thread_status, thread_symbols) =
            (root.clone(), Arc::clone(&exact), Arc::clone(&status), Arc::clone(&symbols));
        std::thread::Builder::new()
            .name("atrius-index-store".into())
            .spawn(move || {
                // **A store thread that ends says why** (Unluminous `task-2207`). Everything that can panic
                // on it is caught where it runs, below; this is for whatever is not, so that a search sent
                // to a thread that has gone answers with the reason instead of `storeError: null`.
                let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    match on_disk {
                        true => store_thread(&thread_root, &thread_exact, &thread_symbols, &thread_status, rx),
                        false => memory_thread(&thread_root, &thread_exact, &thread_symbols, &thread_status, rx),
                    }
                }));
                if let Err(panic) = ran {
                    lock(&thread_status).store_error =
                        Some(format!("the store thread stopped: {}", panic_message(&panic)));
                }
            })
            .expect("the store thread starts");
        freshness.mark_everything();
        Index {
            root,
            exact,
            symbols,
            freshness,
            gate_lock: Mutex::new(()),
            last_gate: Mutex::new(Duration::ZERO),
            compacting: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            store: tx,
            status,
        }
    }

    /// The root this index covers.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A copy of the status.
    pub fn status(&self) -> Status {
        lock(&self.status).clone()
    }

    /// Blocks until the index is loaded or built, or the timeout passes. For tests and `search build`.
    ///
    /// @param timeout - how long to wait
    pub fn wait_ready(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.status().ready {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    /// Blocks until the symbol table is built as well as the exact index, or the timeout passes. A
    /// question about definitions asked the moment a host starts waits here rather than being refused.
    ///
    /// @param timeout - how long to wait
    pub fn wait_for_symbols(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let built = self.status().ready && self.symbols.read().map(|s| s.is_some()).unwrap_or(false);
            if built {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Runs the gate, so the index agrees with the files as they are now, and hands what changed to the
    /// store thread. Returns None while the index is not loaded yet.
    pub fn gate(&self) -> Option<GateReport> {
        let _one_at_a_time = self.gate_lock.lock().expect("gate");
        let mut guard = self.exact.write().expect("exact");
        let exact = guard.as_mut()?;
        let report = self.freshness.gate(exact);
        // Read before the table's lock is taken, so a reader that only tries the lock (the window's
        // completion, on every keystroke) is never kept waiting while files are read.
        let read: Vec<(String, symbols::Read)> = report
            .changed
            .iter()
            .map(|(_, record, _)| {
                let read = symbols::structure_of(record)
                    .unwrap_or(symbols::Read { definitions: Vec::new(), imports: Vec::new() });
                (record.rel.clone(), read)
            })
            .collect();
        let changed_symbols: Vec<_> = read
            .iter()
            .map(|(path, r)| (path.clone(), r.definitions.clone(), r.imports.clone()))
            .collect();
        if let Some(table) = self.symbols.write().expect("symbols").as_mut() {
            for path in &report.removed_paths {
                table.remove_file(path);
            }
            for (path, read) in read {
                table.set_file_with_imports(&path, read.definitions, read.imports);
            }
        }
        if !report.changed.is_empty() || !report.removed.is_empty() {
            let next_id = exact.files.len();
            let _ = self.store.send(StoreJob::Changes {
                changed: report.changed.clone(),
                removed: report.removed.clone(),
                removed_paths: report.removed_paths.clone(),
                next_id,
                symbols: changed_symbols,
            });
            // The store and the symbol table have what they need from the files just read.
            exact.let_go_of(report.changed.iter().map(|(id, _, _)| *id));
            if exact.tombstones() > exact.live() / 2 + 1000
                && !self.compacting.swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                self.compact_in_background();
            }
        }
        let mut status = lock(&self.status);
        status.gates += 1;
        status.reindexed += report.changed.len() as u64;
        Some(report)
    }

    /// An exact search: through the gate and the index when it is ready, by direct scan before then.
    /// The second value says which answered: `"trigram"` or `"none"`.
    ///
    /// @param request - the pattern, case and scope
    pub fn exact(&self, request: &ExactRequest) -> Result<(ExactAnswer, &'static str), String> {
        let gate_started = Instant::now();
        let gated = self.gate();
        *self.last_gate.lock().expect("last gate") = gate_started.elapsed();
        if gated.is_none() {
            return direct::scan(&self.root, request).map(|a| (a, "none"));
        }
        let guard = self.exact.read().expect("exact");
        match guard.as_ref() {
            Some(exact) => exact.search(&self.root, request).map(|a| (a, "trigram")),
            None => direct::scan(&self.root, request).map(|a| (a, "none")),
        }
    }

    /// How long the last exact search's gate took, which `search find` reports beside the search time.
    pub fn last_gate(&self) -> Duration {
        *self.last_gate.lock().expect("last gate")
    }

    /// Rebuilds the index without its tombstones on a thread of its own. The live files are copied under
    /// a short read lock, the new index is built with no lock held, and it replaces the old one only if no
    /// gate changed the old one meanwhile; otherwise the work is thrown away and a later gate tries again.
    /// The store is then sent the whole new index to write, so no save ever holds a lock either.
    fn compact_in_background(&self) {
        let (exact, store, compacting) =
            (Arc::clone(&self.exact), self.store.clone(), Arc::clone(&self.compacting));
        let _ =
            std::thread::Builder::new().name("atrius-index-compact".into()).spawn(move || {
                let taken = exact.read().expect("exact").as_ref().map(|e| {
                    (e.files.iter().flatten().cloned().collect::<Vec<_>>(), e.generation, e.root.clone())
                });
                if let Some((records, generation, root)) = taken {
                    let mut fresh = Exact::from_records(records, root.as_deref());
                    let copy = fresh.clone();
                    fresh.let_go_of_content();
                    let mut guard = exact.write().expect("exact");
                    if guard.as_ref().is_some_and(|e| e.generation == generation) {
                        fresh.generation = generation;
                        *guard = Some(fresh);
                        drop(guard);
                        let _ = store.send(StoreJob::Full { index: Box::new(copy) });
                    }
                }
                compacting.store(false, std::sync::atomic::Ordering::SeqCst);
            });
    }

    /// The chunks that best answer a question, best first, after the gate. Answered on the store thread,
    /// which owns the passage table.
    ///
    /// @param question - the question
    /// @param k - how many to retrieve
    /// @param kind - only chunks of this kind, if given
    pub fn passages(
        &self,
        question: &str,
        k: usize,
        kind: Option<&str>,
    ) -> Result<Vec<PassageHit>, String> {
        if self.gate().is_none() {
            return Err("the index is still being built".into());
        }
        // **Answered at once while the passage table is being built**, rather than queued behind the build
        // on the store thread and given up on after thirty seconds, which on a first build was every plain
        // English question an agent asked. The caller answers from the question's words meanwhile.
        // A store that could not be opened is the exception: the search goes to the store thread, which
        // tries to open it again for every job and answers with the reason when it still cannot.
        {
            let st = lock(&self.status);
            if !st.passages_ready && st.store_error.is_none() {
                return Err("the passage table is still being built".to_owned());
            }
        }
        let (tx, rx) = channel();
        self.store
            .send(StoreJob::Search {
                question: question.to_owned(),
                k,
                kind: kind.map(str::to_owned),
                reply: tx,
            })
            .map_err(|_| match lock(&self.status).store_error.clone() {
                Some(why) => format!("the index's store has stopped: {why}"),
                None => "the index's store has stopped".to_owned(),
            })?;
        rx.recv_timeout(Duration::from_secs(30))
            .map_err(|_| "the passage search did not answer".to_owned())?
    }

    /// Runs a closure over the symbol table and the exact index, after the gate. None while either is
    /// not built yet.
    ///
    /// @param work - what to do with them
    pub fn with_symbols<T>(&self, work: impl FnOnce(&SymbolTable, &Exact) -> T) -> Option<T> {
        self.gate()?;
        let exact = self.exact.read().expect("exact");
        let symbols = self.symbols.read().expect("symbols");
        Some(work(symbols.as_ref()?, exact.as_ref()?))
    }

    /// Runs a closure over the symbol table **without the gate and without waiting**: None while the
    /// table is not built yet, or while a gate is writing to it at this very moment.
    ///
    /// For a caller that must never be kept waiting, such as an editor reading names on every keystroke
    /// (Unluminous `task-2231` §5.2). Such a caller keeps the table fresh with [`Index::keep_fresh`]
    /// rather than gating on every read.
    ///
    /// @param work - what to do with the table
    pub fn try_symbols<T>(&self, work: impl FnOnce(&SymbolTable) -> T) -> Option<T> {
        let guard = self.symbols.try_read().ok()?;
        guard.as_ref().map(work)
    }

    /// Whether the symbol table has been built or loaded.
    pub fn symbols_ready(&self) -> bool {
        self.symbols.read().map(|s| s.is_some()).unwrap_or(false)
    }

    /// Starts a thread that runs the gate whenever the watcher has seen a change, at most once an
    /// interval, so a reader that uses [`Index::try_symbols`] sees a file's new definitions within an
    /// interval of it being written. The thread ends when the index is dropped.
    ///
    /// @param index - the index
    /// @param interval - how often to look
    pub fn keep_fresh(index: &Arc<Index>, interval: Duration) {
        let weak = Arc::downgrade(index);
        let _ = std::thread::Builder::new().name("atrius-index-fresh".into()).spawn(move || loop {
            std::thread::sleep(interval);
            let Some(index) = weak.upgrade() else { return };
            if index.status().ready && index.freshness.pending() > 0 {
                index.gate();
            }
        });
    }

    /// Runs a closure over the loaded exact index, after the gate. None while it is not loaded.
    ///
    /// @param work - what to do with the index
    pub fn with_exact<T>(&self, work: impl FnOnce(&Exact) -> T) -> Option<T> {
        self.gate()?;
        let guard = self.exact.read().expect("exact");
        guard.as_ref().map(work)
    }
}

/// The store thread: opens the file, loads or builds the index, then writes each batch of changes.
///
/// @param root - the root
/// @param exact - where the loaded index goes
/// @param symbol_table - where the symbol table built from it goes
/// @param status - the status to update
/// @param jobs - batches of changes from the gates
fn store_thread(
    root: &Path,
    exact: &RwLock<Option<Exact>>,
    symbol_table: &RwLock<Option<SymbolTable>>,
    status: &Mutex<Status>,
    jobs: std::sync::mpsc::Receiver<StoreJob>,
) {
    let started = Instant::now();
    let store = Store::open_replacing_an_old_one(&store::index_folder(root));
    let (mut index, origin) = match &store {
        Ok(s) if s.exact_is_current() => match s.load_exact() {
            Ok(loaded) => (loaded, "loaded"),
            Err(_) => (Exact::build(root), "built"),
        },
        _ => (Exact::build(root), "built"),
    };
    if origin == "built" {
        if let Ok(s) = &store {
            if let Err(e) = s.save_exact(&index, root) {
                lock(status).store_error = Some(e);
            }
        }
    }
    // **The symbol table is loaded from the file when it can be** (Unluminous `task-2231` §5.2), which is
    // what lets the first keystroke after a project opens answer from the whole project. When the file
    // holds none, or one another structural reading wrote, it is read from the files and written down
    // once the index is answering.
    let loaded = match (&store, origin) {
        (Ok(s), "loaded") => s.load_symbols().ok().flatten(),
        _ => None,
    };
    let save_symbols = loaded.is_none();
    let table = loaded.unwrap_or_else(|| SymbolTable::build(&index));
    index.root = Some(root.to_path_buf());
    *exact.write().expect("exact") = Some(index);
    *symbol_table.write().expect("symbols") = Some(table);
    {
        let mut st = lock(status);
        st.ready = true;
        st.origin = origin.into();
        st.load_ms = started.elapsed().as_millis();
        if let Err(e) = &store {
            st.store_error = Some(e.clone());
        }
    }
    // **A store that cannot be opened is tried again rather than given up on** (Unluminous `task-2200`).
    // This thread used to return here, and from then on every plain English search answered "the index's
    // store has stopped" for the life of the process while exact searches went on working. Opening fails
    // when the file is still held by a process that is on its way out, so it is tried again a few times
    // now and then once for every job that arrives, and a search that arrives meanwhile is told why.
    let mut first_job = None;
    let (store, rebuilt) = match store {
        Ok(store) => (store, origin == "built"),
        Err(error) => match open_the_store_again(root, status, error, &jobs) {
            Some((store, job)) => {
                first_job = job;
                // The exact index this process holds was never written to this file, so it is written now
                // and the passages are built from it.
                if let Some(index) = exact.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
                    if let Err(e) = store.save_exact(index, root) {
                        lock(status).store_error = Some(e);
                    }
                }
                (store, true)
            }
            None => return,
        },
    };
    // **Neither of these may end the thread** (Unluminous `task-2207`). An agent on macOS saw every plain
    // English search answer "the index's store has stopped" with `passagesReady: true`, `canEmbed: false`
    // and `storeError: null`: the passage table had been built, and the thread then ended before it had
    // written anything down. The one thing between the two is `Embedder::open`, whose first act is to run
    // `embed()` to find out whether a model is installed. A panic in either is written down now, and the
    // thread goes on answering searches from the words.
    if save_symbols || rebuilt {
        // Copied under the read lock and written with no lock held, so a gate is never kept waiting
        // behind a write of every definition in the project.
        let rows = symbol_table.read().ok().and_then(|t| t.as_ref().map(SymbolTable::rows));
        if let Some(Err(e)) = rows.map(|rows| store.save_symbols(&rows)) {
            lock(status).store_error = Some(e);
        }
    }
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        build_passages(&store, exact, status, rebuilt)
    }));
    if let Err(panic) = built {
        lock(status).store_error =
            Some(format!("building the passage table panicked: {}", panic_message(&panic)));
    }
    let opened = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(test)]
        tests::panic_if_asked(root);
        Embedder::open(&store, status)
    }));
    let mut embedder = match opened {
        Ok(embedder) => embedder,
        Err(panic) => {
            let mut st = lock(status);
            st.can_embed = false;
            st.not_embedding =
                Some(format!("checking for the embedding model panicked: {}", panic_message(&panic)));
            None
        }
    };
    // **The content is let go of once everything that reads it has** (Unluminous `task-2279`): the store
    // has the files, the symbol table and the passage table are built. See `exact::set_holding_content`.
    if let Some(index) = exact.write().expect("exact").as_mut() {
        index.let_go_of_content();
    }
    // **And the file's pool goes back to small**, unless the embedder has passages to write. A batch of
    // changes, a compaction and embedding open it large again for as long as they take.
    let mut store = store;
    if !embedder.as_ref().is_some_and(|e| e.pending) {
        let _ = store.reopen_with(store::cache_frames());
    }
    // Whether the embedder is missing because the switch is off, rather than because there is no model.
    // Read from what opening it said rather than from the switch now, because the switch may have been
    // turned on while the store was being set up, and then the embedder was never opened.
    let mut switched_off = lock(status).not_embedding.as_deref() == Some(SWITCHED_OFF);
    // When the passage table was last searched, while the index it built for that is still held.
    let mut searched: Option<Instant> = None;
    loop {
        // **The index a question built is let go of once questions stop** (Unluminous `task-2279`): a
        // burst of them is answered from it, and then its 115 MB is given back. See
        // `passages::let_go_of_the_search_index`.
        if searched.is_some_and(|at| at.elapsed() >= *SEARCH_INDEX_IDLE) {
            searched = None;
            match passages::let_go_of_the_search_index(&store.session()) {
                Ok(()) => lock(status).search_index_held = false,
                Err(e) => lock(status).store_error = Some(format!("letting go of the search index: {e}")),
            }
        }
        if let Some(job) = first_job.take() {
            if matches!(job, StoreJob::Search { .. }) {
                searched = Some(Instant::now());
                lock(status).search_index_held = true;
            }
            run_a_job(&mut store, root, status, embedder.as_mut(), job);
            continue;
        }
        // **The switch is read on every pass** (Unluminous `task-2279`), so turning embedding off stops it
        // before the next batch and turning it on starts it within one wait below.
        match (embedding_enabled(), switched_off) {
            (false, false) => {
                embedder = None;
                switched_off = true;
                lock(status).not_embedding = Some(SWITCHED_OFF.into());
            }
            (true, true) => {
                switched_off = false;
                lock(status).not_embedding = None;
                embedder = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    Embedder::open(&store, status)
                }))
                .unwrap_or_else(|panic| {
                    lock(status).not_embedding = Some(format!(
                        "checking for the embedding model panicked: {}",
                        panic_message(&panic)
                    ));
                    None
                });
            }
            _ => {}
        }
        // Embedding is done a small batch at a time, only when no other job is waiting, so a query
        // never waits behind more than one batch.
        let job = match embedder.as_ref().filter(|e| e.pending) {
            Some(_) => match jobs.try_recv() {
                Ok(job) => job,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    let _ = store.reopen_with(store::BUILD_FRAMES);
                    let panicked = embedder.as_mut().and_then(|e| {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            e.step(&store, status)
                        }))
                        .err()
                    });
                    if let Some(panic) = panicked {
                        // An embedder that panics once will panic again on the same chunk, so it is
                        // stopped; the words still answer every search.
                        lock(status).not_embedding =
                            Some(format!("the embedder panicked: {}", panic_message(&panic)));
                        embedder = None;
                    }
                    if !embedder.as_ref().is_some_and(|e| e.pending) {
                        let _ = store.reopen_with(store::cache_frames());
                    }
                    continue;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            },
            // Woken at least twice a second while embedding is switched off, so switching it on is noticed
            // without a job arriving.
            None if switched_off => match jobs.recv_timeout(Duration::from_millis(500)) {
                Ok(job) => job,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            },
            // Woken when the search index is due to be let go of, so it is not held until the next job.
            None if searched.is_some() => match jobs.recv_timeout(*SEARCH_INDEX_IDLE) {
                Ok(job) => job,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
            },
            None => match jobs.recv() {
                Ok(job) => job,
                Err(_) => return,
            },
        };
        if matches!(job, StoreJob::Search { .. }) {
            searched = Some(Instant::now());
            lock(status).search_index_held = true;
        }
        run_a_job(&mut store, root, status, embedder.as_mut(), job);
    }
}

/// The store thread of an index held in memory: builds the exact index and the symbol table, then
/// answers every passage search with the reason there is none and ignores the batches of changes, which
/// the gates have already applied in memory.
///
/// @param root - the root
/// @param exact - where the built index goes
/// @param symbol_table - where the symbol table goes
/// @param status - the status to update
/// @param jobs - what the gates and the searches send
fn memory_thread(
    root: &Path,
    exact: &RwLock<Option<Exact>>,
    symbol_table: &RwLock<Option<SymbolTable>>,
    status: &Mutex<Status>,
    jobs: std::sync::mpsc::Receiver<StoreJob>,
) {
    let started = Instant::now();
    let mut index = Exact::build(root);
    let table = SymbolTable::build(&index);
    index.let_go_of_content();
    *exact.write().expect("exact") = Some(index);
    *symbol_table.write().expect("symbols") = Some(table);
    {
        let mut st = lock(status);
        st.ready = true;
        st.origin = "memory".into();
        st.load_ms = started.elapsed().as_millis();
    }
    while let Ok(job) = jobs.recv() {
        if let StoreJob::Search { reply, .. } = job {
            let _ = reply.send(Err("this index is held in memory and has no passage table".into()));
        }
    }
}

/// The status, even when a thread panicked while it held it.
///
/// @param status - the status
fn lock(status: &Mutex<Status>) -> std::sync::MutexGuard<'_, Status> {
    status.lock().unwrap_or_else(|e| e.into_inner())
}

/// How many times the store is tried again straight away, and how long apart, before it is left to the
/// next job to try.
const STORE_RETRIES: u32 = 5;
const STORE_RETRY_GAP: Duration = Duration::from_millis(400);

/// Opens the store again after the first open failed: a few times straight away, then once for every job
/// that arrives. A search that arrives while it still cannot be opened is answered with the reason.
/// Answers the store and the job that found it open, or `None` when the index was dropped first.
///
/// @param root - the root
/// @param status - the status to update
/// @param error - why the first open failed
/// @param jobs - the jobs
fn open_the_store_again(
    root: &Path,
    status: &Mutex<Status>,
    mut error: String,
    jobs: &std::sync::mpsc::Receiver<StoreJob>,
) -> Option<(Store, Option<StoreJob>)> {
    let folder = store::index_folder(root);
    for _ in 0..STORE_RETRIES {
        std::thread::sleep(STORE_RETRY_GAP);
        match Store::open(&folder) {
            Ok(store) => {
                lock(status).store_error = None;
                return Some((store, None));
            }
            Err(e) => error = e,
        }
    }
    loop {
        lock(status).store_error = Some(error.clone());
        let job = jobs.recv().ok()?;
        match Store::open(&folder) {
            Ok(store) => {
                lock(status).store_error = None;
                return Some((store, Some(job)));
            }
            Err(e) => {
                error = e;
                if let StoreJob::Search { reply, .. } = job {
                    let _ = reply.send(Err(format!("the index file could not be opened: {error}")));
                }
            }
        }
    }
}

/// Runs one job on the store thread.
///
/// **A panic in one job is caught and written down rather than ending the thread** (Unluminous
/// `task-2200`). The thread owns the passage table, so ending it turned every later plain English search
/// into "the index's store has stopped". A search whose job panicked is answered by its reply being
/// dropped, which its caller reports, and the next job runs as usual.
///
/// @param store - the store
/// @param root - the root
/// @param status - the status to update
/// @param embedder - the embedder, if there is one
/// @param job - the job
fn run_a_job(
    store: &mut Store,
    root: &Path,
    status: &Mutex<Status>,
    embedder: Option<&mut Embedder>,
    job: StoreJob,
) {
    if let (Some(e), StoreJob::Changes { .. }) = (embedder, &job) {
        e.pending = true;
    }
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
        // A write runs with the pool a build needs, which the passage table's update asks for too.
        StoreJob::Changes { changed, removed, removed_paths, next_id, symbols } => {
            store.with_room(|store| {
                let records: Vec<&FileRecord> = changed.iter().map(|c| &c.1).collect();
                store
                    .save_changes(&changed, &removed, next_id)
                    .and_then(|()| store.save_symbol_changes(&symbols, &removed_paths))
                    .and_then(|()| passages::update(&store.session(), &removed_paths, &records))
            })
        }
        StoreJob::Full { index } => store.with_room(|store| store.save_exact(&index, root)),
        StoreJob::Search { question, k, kind, reply } => {
            let vectors = lock(status).vectors > 0 && embedding_enabled();
            let _ = reply.send(passages::search(
                &store.session(),
                &question,
                k,
                kind.as_deref(),
                vectors,
            ));
            Ok(())
        }
    }));
    let result = match ran {
        Ok(result) => result,
        Err(panic) => Err(format!("a store job panicked: {}", panic_message(&panic))),
    };
    if let Err(e) = result {
        lock(status).store_error = Some(e);
    }
}

/// The words a panic was raised with, when it was raised with words.
///
/// @param panic - what `catch_unwind` caught
fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "no message".to_owned())
}

/// Whether this process embeds passages and asks the model about questions. Read from `ATRIUS_EMBED`
/// the first time it is asked, and changed while the process runs by [`set_embedding`].
static EMBEDDING: std::sync::OnceLock<std::sync::atomic::AtomicBool> = std::sync::OnceLock::new();

/// The switch, made from `ATRIUS_EMBED` the first time.
fn embedding_switch() -> &'static std::sync::atomic::AtomicBool {
    EMBEDDING.get_or_init(|| {
        let off = std::env::var("ATRIUS_EMBED").is_ok_and(|v| v.trim().eq_ignore_ascii_case("off"));
        std::sync::atomic::AtomicBool::new(!off)
    })
}

/// Whether this process embeds: `ATRIUS_EMBED` unless [`set_embedding`] has said otherwise.
pub fn embedding_enabled() -> bool {
    embedding_switch().load(std::sync::atomic::Ordering::SeqCst)
}

/// Turns embedding on or off for every index this process holds, while they run.
///
/// Off, each store thread drops its embedder on its next pass, so no batch starts after this returns,
/// and a plain English search is answered from the passages' words, so nothing asks the model again.
/// The model's memory is returned by its residency, which a host sets with
/// `INILLUCENT_EMBED_RESIDENCY`. On, a store thread opens its embedder within half a second and carries
/// on from the chunks that still have no vector. An editor that has a setting for this calls it when the
/// setting changes (Unluminous `task-2279`, where the model was 1.1 GB of a window at rest and the only
/// way to stop it was the environment).
///
/// @param on - whether to embed
pub fn set_embedding(on: bool) {
    embedding_switch().store(on, std::sync::atomic::Ordering::SeqCst);
}

/// How long the passage table's search index is kept after the last question: long enough for an agent's
/// burst of questions to be answered from one build, short enough that an editor does not hold it all day.
/// `ATRIUS_SEARCH_INDEX_IDLE_SECONDS` changes it.
static SEARCH_INDEX_IDLE: std::sync::LazyLock<Duration> = std::sync::LazyLock::new(|| {
    Duration::from_secs(
        std::env::var("ATRIUS_SEARCH_INDEX_IDLE_SECONDS")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(20),
    )
});

/// What `search status` says while embedding is switched off.
const SWITCHED_OFF: &str = "embedding is switched off";

/// The background embedder: whether `embed()` works here, the shared cache of vectors by chunk hash,
/// and whether chunks are still waiting for a vector.
struct Embedder {
    cache: Option<Database>,
    pending: bool,
    batch: usize,
}

/// The most chunks a host embeds on the processor. On the processor a host makes about ten vectors a
/// second, so the Linux kernel's chunks would keep a core busy for days, and every search in that time
/// waits behind a batch. Past this a repository is searched by its words alone unless a card is named.
pub const PROCESSOR_CHUNK_LIMIT: usize = 100_000;

/// Why a host that could embed should not, if it should not: `ATRIUS_EMBED=off`, which an
/// evaluation run sets so that nothing embeds while it is being timed, or a repository too large to
/// embed on the processor when `INILLUCENT_EMBED_DEVICE` names no card.
///
/// @param chunks - how many chunks the passage table holds
fn why_not_embed(chunks: usize) -> Option<String> {
    if !embedding_enabled() {
        return Some(SWITCHED_OFF.into());
    }
    let on_a_card = std::env::var("INILLUCENT_EMBED_DEVICE")
        .is_ok_and(|d| d.trim().to_ascii_lowercase().starts_with("cuda"));
    (!on_a_card && chunks > PROCESSOR_CHUNK_LIMIT).then(|| format!("{chunks} chunks is more than the {PROCESSOR_CHUNK_LIMIT} embedded on the processor; set INILLUCENT_EMBED_DEVICE to a card to embed them"))
}

impl Embedder {
    /// Opens the embedder when `embed()` can run, with the shared cache beside the indexes.
    ///
    /// @param store - the store
    /// @param status - the status to update
    fn open(store: &Store, status: &Mutex<Status>) -> Option<Embedder> {
        let session = store.session();
        // **Turned off means the model is not even looked for.** `ATRIUS_EMBED=off` used to be read after
        // the probe below, which runs the model, so a machine where running it is the problem could not
        // be told to skip it. `task-2207`.
        if let Some(reason) = why_not_embed(0) {
            let mut st = lock(status);
            st.not_embedding = Some(reason);
            st.vectors = passages::vector_counts(&session).0;
            return None;
        }
        let able = passages::can_embed(&session);
        let (vectors, _) = passages::vector_counts(&session);
        {
            let mut st = lock(status);
            st.can_embed = able;
            st.vectors = vectors;
        }
        if !able {
            return None;
        }
        if let Some(reason) = why_not_embed(passages::vector_counts(&session).1) {
            lock(status).not_embedding = Some(reason);
            return None;
        }
        let folder = store::cache_folder().join("atrius-index").join("embeddings");
        let cache = std::fs::create_dir_all(&folder)
            .ok()
            .and_then(|_| {
                let options = inillucent_driver::OpenOptions {
                    create: true,
                    cache_frames: store::cache_frames(),
                    ..inillucent_driver::OpenOptions::default()
                };
                Database::open_with(folder.join("embeddings.rdb"), options).ok()
            });
        if let Some(c) = &cache {
            let _ = c.session().execute_batch(
                "CREATE TABLE IF NOT EXISTS emb (hash BLOB PRIMARY KEY, model TEXT, vector BLOB);",
            );
        }
        let batch =
            std::env::var("ATRIUS_EMBED_BATCH").ok().and_then(|b| b.parse().ok()).unwrap_or(4);
        Some(Embedder { cache, pending: true, batch })
    }

    /// Embeds one batch and records how many chunks now have a vector.
    ///
    /// @param store - the store
    /// @param status - the status to update
    fn step(&mut self, store: &Store, status: &Mutex<Status>) {
        let session = store.session();
        let cache = self.cache.as_ref().map(Database::session);
        match passages::embed_pending(&session, cache.as_ref(), self.batch) {
            Ok(0) => self.pending = false,
            Ok(_) => {}
            Err(e) => {
                self.pending = false;
                lock(status).store_error = Some(format!("embedding: {e}"));
            }
        }
        lock(status).vectors = passages::vector_counts(&session).0;
    }
}

/// Builds the passage table when it was never built, was built by another version, or the exact index
/// was just built from scratch; otherwise keeps the one in the file.
///
/// @param store - the store
/// @param exact - the exact index
/// @param status - the status to update
/// @param rebuilt - whether the exact index was just built rather than loaded
fn build_passages(
    store: &Store,
    exact: &RwLock<Option<Exact>>,
    status: &Mutex<Status>,
    rebuilt: bool,
) {
    let current = store.meta("passage_version") == Some(passages::passage_version());
    let session = store.session();
    let built = if current && !rebuilt {
        passages::create(&session).and_then(|()| {
            let rows =
                session.query("SELECT count(*) FROM chunk", &[], 1).map_err(|e| e.to_string())?;
            Ok(match rows.value(0, 0) {
                Some(inillucent_driver::Value::Integer(n)) => *n as usize,
                _ => 0,
            })
        })
    } else {
        // The passage sources are copied under a short read lock and written with none held, so a
        // query is never kept waiting by a rebuild (on ai-service one takes minutes).
        let records: Vec<FileRecord> = exact
            .read()
            .expect("exact")
            .as_ref()
            .map(|index| {
                index
                    .files
                    .iter()
                    .flatten()
                    .filter(|r| passages::is_passage_source(r))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        passages::rebuild(&session, &records)
    };
    let mut st = lock(status);
    match built {
        Ok(count) => {
            st.passages = count;
            st.passages_ready = true;
            let _ = store.set_meta("passage_version", &passages::passage_version());
        }
        Err(e) => st.store_error = Some(format!("the passage table: {e}")),
    }
}

/// A Windows path without its `\\?\` prefix, which `canonicalize` adds and other programs refuse.
///
/// @param path - a canonical path
pub fn strip_verbatim(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    /// The roots whose store thread is made to panic where it looks for the embedding model.
    fn panicking_roots() -> &'static Mutex<Vec<PathBuf>> {
        static ROOTS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();
        ROOTS.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// Called on the store thread just before the embedder is opened: panics for a root a test named,
    /// which is how a model that panics on this machine is stood in for on one that has none.
    ///
    /// @param root - the root the store thread serves
    pub(super) fn panic_if_asked(root: &Path) {
        if panicking_roots().lock().unwrap().iter().any(|r| r == root) {
            panic!("the embedding probe failed on purpose");
        }
    }

    /// `task-2207`: a store thread that met a panic where it looks for the embedding model ended, and every
    /// plain English search after it answered "the index's store has stopped" with nothing written down.
    /// The thread lives on now, says why it is not embedding, and answers from the passage table's words.
    #[test]
    fn a_panic_looking_for_the_model_leaves_the_store_answering_questions() {
        static CACHE: OnceLock<tempfile::TempDir> = OnceLock::new();
        let cache = CACHE.get_or_init(|| tempfile::tempdir().unwrap());
        std::env::set_var("ATRIUS_INDEX_CACHE", cache.path());
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/route.ts"),
            "// Decides whether an offer is sent to the v1 or the v2 service.\nexport function routeOffer(offer) {\n  return offer.version === 2 ? 'v2' : 'v1';\n}\n",
        )
        .unwrap();
        std::fs::write(root.join("src/health.ts"), "export function health() { return 'ok'; }\n")
            .unwrap();
        let root = crate::host::canonical(&root);
        panicking_roots().lock().unwrap().push(root.clone());

        let index = Index::open_quietly(&root);
        assert!(index.wait_ready(Duration::from_secs(60)));
        let began = Instant::now();
        while index.status().not_embedding.is_none() && began.elapsed() < Duration::from_secs(60) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let status = index.status();
        assert!(
            status.not_embedding.as_deref().is_some_and(|why| why.contains("panicked")),
            "{:?}",
            status.not_embedding
        );
        assert!(status.store_error.is_none(), "{:?}", status.store_error);
        assert!(!status.can_embed);
        let args = serde_json::json!({ "query": "where does an offer get routed to v1 or v2", "mode": "semantic" });
        let answer = crate::verbs::answer(&index, "find", args.as_object().unwrap())
            .expect("a question is answered");
        assert_eq!(answer["index"], "passage", "{answer}");
        assert!(answer["text"].as_str().unwrap().contains("src/route.ts"), "{answer}");
    }
}
