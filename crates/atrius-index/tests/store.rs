//! The store thread survives what used to end it, and a plain English question is answered either way.
//!
//! Unluminous `task-2200`: an agent in the chat pane saw "the index's store has stopped" on many of its
//! searches. The store thread returned the moment its file could not be opened, and from then on every
//! passage search was refused for the life of the process while exact searches went on working.

use std::path::Path;
use std::time::{Duration, Instant};

use atrius_index::index::Index;
use serde_json::{json, Map, Value};

/// Writes a file, creating its folder.
fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A small project: a few files whose words a question can find.
fn a_project(root: &Path) {
    write(root, "src/shared.rs", "/// Doubles a value for every handler.\npub fn shared_helper(x: u32) -> u32 {\n    x * 2\n}\n");
    write(root, "src/handler.rs", "/// Handles a request by calling the shared helper.\npub fn handle_request(input: u32) -> u32 {\n    shared_helper(input) + 1\n}\n");
    write(root, "src/main.rs", "fn main() { println!(\"hi\"); }\n");
}

/// One scratch cache for the whole file, set once: the tests run in parallel in one process, and the
/// variable is the process's, so each setting its own would point the other at the wrong folder.
fn scratch_cache() {
    static CACHE: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| tempfile::tempdir().unwrap());
    std::env::set_var("ATRIUS_INDEX_CACHE", cache.path());
    std::env::set_var("ATRIUS_EMBED", "off");
}

/// The arguments of one `find`.
fn find(query: &str, mode: &str) -> Map<String, Value> {
    json!({ "query": query, "mode": mode }).as_object().unwrap().clone()
}

#[test]
fn a_store_that_cannot_be_opened_is_tried_again_and_a_question_is_answered_meanwhile() {
    scratch_cache();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    a_project(&root);
    let root = atrius_index::host::canonical(&root);
    // The index file's path taken by a folder, so the store cannot be opened, on every platform.
    let file = atrius_index::store::index_folder(&root).join("index.rdb");
    std::fs::create_dir_all(&file).unwrap();

    let index = Index::open_quietly(&root);
    assert!(index.wait_ready(Duration::from_secs(60)), "the exact index is built in memory");
    let answer = atrius_index::verbs::answer(&index, "find", &find("how is a request handled", "semantic"))
        .expect("a question is answered while the store cannot be opened");
    assert_eq!(answer["index"], "words", "{answer}");
    let text = answer["text"].as_str().unwrap();
    assert!(text.contains("src/handler.rs"), "{text}");
    assert!(index.status().store_error.is_some(), "the reason is kept");

    // The way clears, and the next search opens the store and is answered from the passages.
    std::fs::remove_dir_all(&file).unwrap();
    let began = Instant::now();
    let mut last = Value::Null;
    while began.elapsed() < Duration::from_secs(60) {
        last = atrius_index::verbs::answer(&index, "find", &find("how is a request handled", "semantic"))
            .expect("answered");
        if last["index"] == "passage" {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(last["index"], "passage", "the store was opened again: {last}");
    assert!(index.status().store_error.is_none(), "{:?}", index.status().store_error);
}

#[test]
fn a_line_of_code_in_auto_mode_is_searched_as_text_when_it_is_not_a_regex() {
    scratch_cache();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    a_project(&root);
    let root = atrius_index::host::canonical(&root);
    let index = Index::open_quietly(&root);
    assert!(index.wait_ready(Duration::from_secs(60)));
    let answer =
        atrius_index::verbs::answer(&index, "find", &find("fn main() { println!(\"hi\"); }", "auto"))
            .expect("a line of code is not refused as a malformed regex");
    let text = answer["text"].as_str().unwrap_or_default();
    assert!(text.contains("src/main.rs"), "{answer}");
    // Asked for as a regex, it is still refused.
    assert!(atrius_index::verbs::answer(&index, "find", &find("fn main() {", "regex")).is_err());
}

/// How many rows a table of the store holds.
fn rows_in(store: &atrius_index::store::Store, table: &str) -> i64 {
    let rows = store.session().query_all(&format!("SELECT count(*) FROM {table}"), &[]).unwrap();
    match rows.rows[0][0] {
        inillucent_driver::Value::Integer(n) => n,
        _ => panic!("a count"),
    }
}

/// Unluminous `task-2247`: a blob is a whole file's content, and every version of every file that
/// changed was kept for ever. The Unluminous checkout's index had reached 11.7 GB of them.
#[test]
fn a_file_that_changes_keeps_one_blob_and_a_removed_file_keeps_none() {
    use atrius_index::exact::{Exact, FileRecord};
    use atrius_index::store::Store;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("checkout");
    a_project(&root);
    let store = Store::open(&dir.path().join("index")).unwrap();
    let mut exact = Exact::build(&root);
    store.save_exact(&exact, &root).unwrap();
    assert_eq!(rows_in(&store, "blob"), 3);
    for round in 0..6 {
        let (record, trigrams) = FileRecord::from_bytes(
            "src/main.rs".into(),
            round,
            format!("fn main() {{ round_{round}(); }}\n").as_bytes(),
        );
        let old = exact.by_path["src/main.rs"];
        exact.remove("src/main.rs");
        exact.insert(record.clone(), &trigrams);
        let id = exact.files.len() as u32 - 1;
        store.save_changes(&[(id, record, trigrams)], &[old], exact.files.len()).unwrap();
    }
    assert_eq!(rows_in(&store, "blob"), 3, "one blob for each file, whatever it used to hold");
    let gone = exact.by_path["src/handler.rs"];
    exact.remove("src/handler.rs");
    store.save_changes(&[], &[gone], exact.files.len()).unwrap();
    assert_eq!(rows_in(&store, "blob"), 2, "a removed file's content goes with it");
}
