//! Embedding is switched on and off while an index runs, and the store thread follows the switch.
//!
//! Unluminous `task-2279`: the embedding model was 1.1 GB of a window at rest, and the only way to stop it
//! was `ATRIUS_EMBED=off` in the environment before the window started. `set_embedding` is read on every
//! pass of the store loop. This file is a process of its own, because the switch is the process's.

use std::path::Path;
use std::time::{Duration, Instant};

use atrius_index::index::{embedding_enabled, set_embedding, Index};

/// Writes a file, creating its folder.
fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Waits until the store thread's reason for not embedding answers `wanted`, for up to five seconds.
fn wait_for(index: &Index, wanted: impl Fn(Option<&str>) -> bool) -> Option<String> {
    let began = Instant::now();
    loop {
        let reason = index.status().not_embedding;
        if wanted(reason.as_deref()) || began.elapsed() > Duration::from_secs(5) {
            return reason;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn the_store_thread_follows_the_switch_while_it_runs() {
    let cache = tempfile::tempdir().unwrap();
    std::env::set_var("ATRIUS_INDEX_CACHE", cache.path());
    std::env::set_var("ATRIUS_EMBED", "off");
    assert!(!embedding_enabled(), "ATRIUS_EMBED=off is where the switch starts");

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    write(
        &root,
        "src/handler.rs",
        "/// Handles a request.\npub fn handle_request(input: u32) -> u32 {\n    input + 1\n}\n",
    );
    let root = atrius_index::host::canonical(&root);
    let index = Index::open_quietly(&root);
    assert!(index.wait_ready(Duration::from_secs(60)), "the index is ready");

    let off = wait_for(&index, |r| r == Some("embedding is switched off"));
    assert_eq!(off.as_deref(), Some("embedding is switched off"), "off from the start");

    set_embedding(true);
    let on = wait_for(&index, |r| r != Some("embedding is switched off"));
    assert_ne!(
        on.as_deref(),
        Some("embedding is switched off"),
        "switched on within half a second's wait"
    );

    set_embedding(false);
    let off_again = wait_for(&index, |r| r == Some("embedding is switched off"));
    assert_eq!(off_again.as_deref(), Some("embedding is switched off"), "and off again");
}
