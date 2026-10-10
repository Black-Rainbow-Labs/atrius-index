//! What the structural reading of one file costs, step by step (Unluminous `task-2231` §5.2).
//!
//! An editor reads its open tab this way on each change of the text, so the steps are timed apart:
//! the tokeniser's definitions, the folding regions, and the structure taken out of them.
//!
//! ```sh
//! cargo run --release -p atrius-index --example structure_cost -- <file>
//! ```

use std::time::Instant;

use atrius_index::lang::folding::{self, Reading};
use atrius_index::lang::symbols::FileSymbols;

/// The mean of `runs` runs of `body`, in milliseconds, after one untimed run.
fn timed(runs: usize, mut body: impl FnMut()) -> f64 {
    body();
    let start = Instant::now();
    for _ in 0..runs {
        body();
    }
    start.elapsed().as_secs_f64() * 1000.0 / runs as f64
}

fn main() {
    let path = std::env::args().nth(1).expect("a file");
    let text = std::fs::read_to_string(&path).expect("read the file").replace("\r\n", "\n");
    let rel = std::path::Path::new(&path).file_name().unwrap().to_string_lossy().into_owned();
    let language = atrius_index::grammars::for_path(&rel).expect("a language claims it");
    let grammar = &language.grammar;
    let symbols_ms = timed(10, || {
        std::hint::black_box(FileSymbols::read(&text, grammar));
    });
    let regions_ms = timed(10, || {
        std::hint::black_box(folding::regions(&text, Reading::Code(grammar)));
    });
    let symbols = FileSymbols::read(&text, grammar);
    let regions = folding::regions_from(&text, Reading::Code(grammar), &symbols.folding_tokens());
    let shared_ms = timed(10, || {
        std::hint::black_box(folding::regions_from(&text, Reading::Code(grammar), &symbols.folding_tokens()));
    });
    let structure_ms = timed(10, || {
        std::hint::black_box(atrius_index::structure::read(&text, grammar, &symbols, &regions));
    });
    let whole_ms = timed(10, || {
        std::hint::black_box(atrius_index::outline::read_structure(&rel, &text));
    });
    println!("{path}: {} KB", text.len() / 1024);
    println!("  definitions (tokeniser)   {symbols_ms:8.3} ms");
    println!("  folding regions           {regions_ms:8.3} ms");
    println!("  regions from its tokens   {shared_ms:8.3} ms  (no second scan)");
    println!("  structure from them       {structure_ms:8.3} ms");
    println!("  read_structure, whole     {whole_ms:8.3} ms");
}
