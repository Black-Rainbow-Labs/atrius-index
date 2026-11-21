//! The fence, the kqueue and the gate under the cases `task-2696` added (TDD `task-2695`, testing item 2,
//! and the review in `task-2838`):
//!
//! - a file written and searched within a millisecond, 500 times, half of the writes made by another
//!   process, is always found, through whichever proof the platform uses;
//! - two indexes watching one checkout never take each other's fence for their own;
//! - a query that starts while another query's fence is pending sees a change made after that fence was
//!   written.
//!
//! On macOS these run with the kqueue, and the first waits until the kqueue is the proof. The fence cases
//! are in `fence_wait.rs`, a binary of their own, because `ATRIUS_KQUEUE` is the process's.

use std::path::Path;
use std::process::Command;
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

/// Writes a file from another process and waits for it to exit.
fn write_elsewhere(path: &Path, text: &str) {
    let status = Command::new("/bin/sh").arg("-c").arg("printf '%s\\n' \"$1\" > \"$2\"").arg("sh").arg(text).arg(path).status().unwrap();
    assert!(status.success());
}

#[test]
fn a_write_is_found_by_the_query_straight_after_it() {
    let (_dir, root) = checkout();
    let index = Index::open_in_memory(&root);
    assert!(index.wait_ready(Duration::from_secs(60)));
    hits(&index, "warm");
    if cfg!(target_os = "macos") {
        let armed = std::time::Instant::now();
        while index.proof() != "kqueue" && armed.elapsed() < Duration::from_secs(30) {
            hits(&index, "warm");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(index.proof(), "kqueue", "the kqueue was armed and confirmed");
    }
    let mut missed = Vec::new();
    for i in 0..500 {
        let path = root.join(format!("src/w{}.rs", i % 7));
        let token = format!("written_token_{i}_end");
        if i % 2 == 0 {
            std::fs::write(&path, format!("const X: &str = \"{token}\";\n")).unwrap();
        } else {
            write_elsewhere(&path, &format!("const X: &str = \"{token}\";"));
        }
        if hits(&index, &token) != 1 {
            missed.push(i);
        }
    }
    assert!(missed.is_empty(), "missed {} of 500 writes: {:?}, freshness proof {}", missed.len(), &missed[..missed.len().min(10)], index.proof());
}

#[test]
fn a_deleted_and_a_renamed_file_are_gone_from_the_query_straight_after() {
    let (_dir, root) = checkout();
    let index = Index::open_in_memory(&root);
    assert!(index.wait_ready(Duration::from_secs(60)));
    for i in 0..100 {
        let path = root.join(format!("src/gone{i}.rs"));
        std::fs::write(&path, format!("fn gone_token_{i}() {{}}\n")).unwrap();
        assert_eq!(hits(&index, &format!("gone_token_{i}\\b")), 1, "the new file {i}");
        if i % 2 == 0 {
            std::fs::remove_file(&path).unwrap();
        } else {
            std::fs::rename(&path, root.join(format!("src/moved{i}.txt"))).unwrap();
        }
        let expected = usize::from(i % 2 == 1);
        assert_eq!(hits(&index, &format!("gone_token_{i}\\b")), expected, "after the change to file {i}");
    }
    std::fs::create_dir_all(root.join("newdir/deeper")).unwrap();
    std::fs::write(root.join("newdir/deeper/x.rs"), "fn in_a_new_folder() {}\n").unwrap();
    assert_eq!(hits(&index, "in_a_new_folder"), 1, "a file in a new folder");
    std::fs::rename(root.join("newdir"), root.join("renamed_dir")).unwrap();
    std::fs::write(root.join("renamed_dir/deeper/x.rs"), "fn after_the_folder_rename() {}\n").unwrap();
    assert_eq!(hits(&index, "after_the_folder_rename"), 1, "a write inside a renamed folder");
    assert_eq!(hits(&index, "in_a_new_folder"), 0, "the old content is gone");
}

