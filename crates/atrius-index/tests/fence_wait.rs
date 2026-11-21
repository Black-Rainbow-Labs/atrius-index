//! The fence and the gate, with the kqueue off, under the cases `task-2696` added (TDD `task-2695`, testing item 2,
//! and the review in `task-2838`):
//!
//! - a file written and searched within a millisecond, 500 times, half of the writes made by another
//!   process, is always found, through whichever proof the platform uses;
//! - two indexes watching one checkout never take each other's fence for their own;
//! - a query that starts while another query's fence is pending sees a change made after that fence was
//!   written.
//!
//! Every case here sets `ATRIUS_KQUEUE=off`, so on macOS they test the fence and not the kqueue.

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use atrius_index::exact::ExactRequest;
use atrius_index::files::Scope;
use atrius_index::index::Index;

/// A git checkout with a few files, in a fresh folder.
fn checkout() -> (tempfile::TempDir, std::path::PathBuf) {
    let cache = tempfile::tempdir().unwrap();
    std::env::set_var("ATRIUS_INDEX_CACHE", cache.path());
    std::env::set_var("ATRIUS_EMBED", "off");
    std::mem::forget(cache);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("checkout");
    std::fs::create_dir_all(root.join("src")).unwrap();
    for i in 0..50 {
        std::fs::write(root.join(format!("src/f{i}.rs")), format!("fn f{i}() {{}}\n")).unwrap();
    }
    let git = |args: &[&str]| assert!(Command::new("git").current_dir(&root).args(args).output().unwrap().status.success());
    git(&["init", "--quiet", "-b", "main"]);
    let root = atrius_index::host::canonical(&root);
    (dir, root)
}

/// How many lines of the index's answer match a pattern.
fn hits(index: &Index, pattern: &str) -> usize {
    let scope = Scope::everything();
    let request = ExactRequest { pattern, case_insensitive: false, scope: &scope };
    index.exact(&request).expect("searched").0.hits.len()
}


#[test]
fn two_indexes_of_one_checkout_wait_for_their_own_fences() {
    std::env::set_var("ATRIUS_KQUEUE", "off");
    let (_dir, root) = checkout();
    let (a, b) = (Index::open_in_memory(&root), Index::open_in_memory(&root));
    assert!(a.wait_ready(Duration::from_secs(60)) && b.wait_ready(Duration::from_secs(60)));
    let busy = Arc::clone(&b);
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stopping = Arc::clone(&stop);
    let other = std::thread::spawn(move || {
        while !stopping.load(std::sync::atomic::Ordering::SeqCst) {
            hits(&busy, "anything");
        }
    });
    let mut missed = 0;
    for i in 0..100 {
        let token = format!("shared_token_{i}_end");
        std::fs::write(root.join("src/shared.rs"), format!("// {token}\n")).unwrap();
        missed += usize::from(hits(&a, &token) != 1);
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    other.join().unwrap();
    assert_eq!(missed, 0, "the index missed writes while another index of the checkout fenced");
    assert_eq!(a.status().unfenced_gates, 0, "every fence arrived");
}

#[test]
fn a_query_that_starts_while_a_fence_is_pending_sees_a_change_made_after_that_fence() {
    std::env::set_var("ATRIUS_KQUEUE", "off");
    let (_dir, root) = checkout();
    let index = Index::open_in_memory(&root);
    assert!(index.wait_ready(Duration::from_secs(60)));
    hits(&index, "warm");
    for i in 0..50 {
        let first = Arc::clone(&index);
        let pending = std::thread::spawn(move || hits(&first, "fn f1\\b"));
        std::thread::sleep(Duration::from_millis(1));
        let token = format!("late_token_{i}_end");
        std::fs::write(root.join("src/late.rs"), format!("// {token}\n")).unwrap();
        assert_eq!(hits(&index, &token), 1, "round {i}: the second query missed the change");
        pending.join().unwrap();
    }
}
