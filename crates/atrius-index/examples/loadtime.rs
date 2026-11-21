//! Times each part of loading a saved index: the exact index and the symbol table (`task-2696`).
//!
//!   ATRIUS_INDEX_CACHE=<cache> cargo run --release -p atrius-index --example loadtime -- <root>

use std::path::Path;
use std::time::Instant;

use atrius_index::store::{self, Store};

fn main() {
    let root = std::env::args().nth(1).expect("a root");
    let root = atrius_index::host::canonical(Path::new(&root));
    let started = Instant::now();
    let store = Store::open_replacing_an_old_one(&store::index_folder(&root)).expect("the store opens");
    println!("open {:.1} ms", started.elapsed().as_secs_f64() * 1000.0);
    let t = Instant::now();
    let exact = store.load_exact().expect("exact loads");
    println!("load_exact {:.1} ms, {} files, {} trigrams", t.elapsed().as_secs_f64() * 1000.0, exact.live(), exact.postings.len());
    let t = Instant::now();
    let symbols = store.load_symbols().expect("symbols load");
    println!("load_symbols {:.1} ms, loaded {}", t.elapsed().as_secs_f64() * 1000.0, symbols.is_some());
}
