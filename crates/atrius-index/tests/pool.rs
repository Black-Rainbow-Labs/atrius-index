//! The index file's pool is small once the index is loaded, and large while anything is built.
//!
//! Unluminous `task-2279` gave the pool 128 pages to save memory, and Inillucent 2.0 then failed to build the
//! passage table of a 2,000 file checkout with "out of memory": the passages were never written, so every
//! open built them again. This builds the passages of a project large enough to need the room, then opens
//! it again and expects them loaded rather than built. A process of its own, because the cache folder is
//! the process's.

use std::path::Path;
use std::time::{Duration, Instant};

use atrius_index::index::Index;

/// Writes a file, creating its folder.
fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A project of 1,500 Rust files, each with a few dozen documented functions.
fn a_large_project(root: &Path) {
    for file in 0..1500 {
        let mut text = String::new();
        for function in 0..30 {
            text.push_str(&format!(
                "/// Works out part {function} of the answer for module {file}, from the input it is given.\npub fn part_{file}_{function}(input: u64) -> u64 {{\n    let doubled = input * 2 + {function};\n    doubled.wrapping_mul({file})\n}}\n\n"
            ));
        }
        write(root, &format!("src/module_{}/file_{file}.rs", file % 40), &text);
    }
}

/// Waits until the passage table is ready, for up to three minutes.
fn passages_ready(index: &Index) -> bool {
    let began = Instant::now();
    while !index.status().passages_ready && began.elapsed() < Duration::from_secs(180) {
        std::thread::sleep(Duration::from_millis(100));
    }
    index.status().passages_ready
}

#[test]
fn a_large_project_builds_its_passages_and_loads_them_the_next_time() {
    let cache = tempfile::tempdir().unwrap();
    std::env::set_var("ATRIUS_INDEX_CACHE", cache.path());
    std::env::set_var("ATRIUS_EMBED", "off");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    a_large_project(&root);
    let root = atrius_index::host::canonical(&root);

    {
        let index = Index::open_quietly(&root);
        assert!(index.wait_ready(Duration::from_secs(120)), "the index is built");
        assert!(passages_ready(&index), "the passages are built");
        let status = index.status();
        assert_eq!(status.store_error, None, "nothing failed while building");
        assert!(status.passages > 1_000, "the passages are there: {}", status.passages);
    }

    let again = Index::open_quietly(&root);
    assert!(again.wait_ready(Duration::from_secs(120)), "the index is loaded");
    let status = again.status();
    assert_eq!(status.origin, "loaded", "the file was written whole the first time");
    assert!(passages_ready(&again), "the passages are there");
    assert_eq!(again.status().store_error, None, "and nothing failed loading them");
}
