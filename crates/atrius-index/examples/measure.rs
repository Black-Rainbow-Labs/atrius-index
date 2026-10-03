//! Measures building the full index of folders: the time until the exact index answers, until
//! definitions answer, and until the passage table is built, the memory the exact index holds, and the
//! size of the index file. Embedding is off, and the index files go to a scratch cache.
//!
//!   cargo run --release -p atrius-index --example measure -- <name>=<folder> [<name>=<folder> ...]
//!
//! Prints one JSON object a line.

use std::time::{Duration, Instant};

use atrius_index::index::Index;

fn main() {
    let cache = std::env::temp_dir().join(format!("atrius-measure-{}", std::process::id()));
    std::env::set_var("ATRIUS_INDEX_CACHE", &cache);
    std::env::set_var("ATRIUS_EMBED", "off");
    for argument in std::env::args().skip(1) {
        let Some((name, folder)) = argument.split_once('=') else { continue };
        let root = atrius_index::host::canonical(std::path::Path::new(folder));
        let started = Instant::now();
        let index = Index::open_quietly(&root);
        index.wait_ready(Duration::from_secs(3600));
        let ready = started.elapsed();
        index.wait_for_symbols(Duration::from_secs(3600));
        let symbols = started.elapsed();
        let mut passages = None;
        while started.elapsed() < Duration::from_secs(3600) {
            if index.status().passages_ready {
                passages = Some(started.elapsed());
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let (files, bytes) = index.with_exact(|e| (e.live(), e.heap_bytes())).unwrap_or((0, 0));
        // Wait for the store thread to finish writing before measuring the file.
        std::thread::sleep(Duration::from_secs(2));
        let status = index.status();
        let file_bytes = std::fs::metadata(&status.file).map(|m| m.len()).unwrap_or(0);
        println!(
            "{}",
            serde_json::json!({
                "corpus": name,
                "files": files,
                "readyMs": ready.as_millis() as u64,
                "symbolsMs": symbols.as_millis() as u64,
                "passagesMs": passages.map(|p| p.as_millis() as u64),
                "passages": status.passages,
                "exactHeapMb": bytes / (1 << 20),
                "indexFileMb": file_bytes / (1 << 20),
            })
        );
        drop(index);
    }
    let _ = std::fs::remove_dir_all(&cache);
}
