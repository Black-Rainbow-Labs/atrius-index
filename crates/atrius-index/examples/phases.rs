//! Times each phase of one exact search on a real folder: building the matcher, planning the trigram
//! query, narrowing to the scope, intersecting posting lists, and verifying candidates. For judging a
//! speed change before a full evaluation run.
//!
//!   cargo run --release -p atrius-index --example phases -- <folder> <pattern> [reps]

use std::path::Path;
use std::time::Instant;

use atrius_index::exact::{matcher, Exact, ExactRequest};
use atrius_index::files::Scope;
use atrius_index::plan::plan;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let root = Path::new(&args[1]);
    let pattern = &args[2];
    let reps: usize = args.get(3).and_then(|r| r.parse().ok()).unwrap_or(50);
    let index = Exact::build(root);
    let scope = Scope::everything();
    let mut timings = vec![Vec::new(); 4];
    let request = ExactRequest { pattern, case_insensitive: false, scope: &scope };
    let (candidates, _, _) = index.candidates(&request).expect("candidates");
    let mut sizes: Vec<(u64, String)> = candidates
        .iter()
        .filter_map(|&unit| {
            index.files[(unit >> atrius_index::exact::BLOCK_BITS) as usize].as_ref()
        })
        .map(|r| (r.size, r.rel.clone()))
        .collect();
    sizes.sort_by_key(|s| std::cmp::Reverse(s.0));
    println!("largest candidates: {:?}", &sizes[..sizes.len().min(5)]);
    for &unit in &candidates {
        let r =
            index.files[(unit >> atrius_index::exact::BLOCK_BITS) as usize].as_ref().unwrap();
        println!(
            "  {} block {} of {} binary={} size={}",
            r.rel,
            unit & 255,
            r.blocks.len(),
            r.binary,
            r.size
        );
    }
    for _ in 0..reps {
        let t = Instant::now();
        let _c = index.candidates(&request).expect("candidates");
        timings[3].push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        let _m = matcher(pattern, false).expect("matcher");
        timings[0].push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        let _q = plan(pattern, false).expect("plan");
        timings[1].push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        let found = index
            .search(root, &ExactRequest { pattern, case_insensitive: false, scope: &scope })
            .expect("search");
        timings[2].push(t.elapsed().as_secs_f64() * 1000.0);
        if timings[2].len() == 1 {
            println!(
                "{} hits, {} candidates of {} in scope",
                found.hits.len(),
                found.candidates,
                found.in_scope
            );
        }
    }
    for (name, mut t) in ["matcher", "plan", "whole search", "candidates"].into_iter().zip(timings)
    {
        t.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!("{name}: median {:.3} ms", t[t.len() / 2]);
    }
}
