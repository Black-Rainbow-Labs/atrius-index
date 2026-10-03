//! Runs every frozen F2 query of one corpus through the exact engine and compares each answer with
//! ripgrep's, by the same digest the evaluation harness takes: the sha256 of the sorted `path:line`
//! set. Prints every mismatch with what was extra and what was missing.
//!
//!   cargo run --release -p atrius-index --example parity -- <F2-exact.jsonl> <corpus name> <snapshot folder>

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Instant;

use sha2::{Digest, Sha256};
use atrius_index::exact::{Exact, ExactRequest};
use atrius_index::files::Scope;

/// The regex the rg arm handed ripgrep for a query: fixed strings escaped, whole words wrapped.
///
/// @param query - the frozen query
fn pattern_of(query: &serde_json::Value) -> String {
    let raw = query["pattern"].as_str().unwrap_or_default();
    let mut pattern =
        if query["fixed"].as_bool().unwrap_or(false) { regex::escape(raw) } else { raw.to_owned() };
    if query["word"].as_bool().unwrap_or(false) {
        pattern = format!(r"\b(?:{pattern})\b");
    }
    pattern
}

/// The harness's digest of an answer.
///
/// @param set - the sorted `path:line` strings
fn digest(set: &BTreeSet<String>) -> String {
    let joined = set.iter().cloned().collect::<Vec<_>>().join("\n");
    let hash = Sha256::digest(joined.as_bytes());
    hash.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (file, corpus, root) = (&args[1], &args[2], Path::new(&args[3]));
    let started = Instant::now();
    let index = Exact::build(root);
    println!("built {} files in {:.1} s", index.live(), started.elapsed().as_secs_f64());
    let text = std::fs::read_to_string(file).expect("the query file");
    let (mut same, mut differ) = (0, 0);
    for line in text.lines() {
        let query: serde_json::Value = serde_json::from_str(line).expect("a query");
        if query["repo"] != *corpus {
            continue;
        }
        let strings = |key: &str| -> Vec<String> {
            query[key]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
                .unwrap_or_default()
        };
        let scope = match Scope::new(
            root,
            query["path"].as_str().unwrap_or_default(),
            &strings("globs"),
            &strings("types"),
        ) {
            Ok(scope) => scope,
            Err(e) => {
                println!("{} scope refused: {e}", query["id"]);
                differ += 1;
                continue;
            }
        };
        let pattern = pattern_of(&query);
        let request = ExactRequest {
            pattern: &pattern,
            case_insensitive: query["ignoreCase"].as_bool().unwrap_or(false),
            scope: &scope,
        };
        let answer = match index.search(root, &request) {
            Ok(answer) => answer,
            Err(e) => {
                println!("{} refused: {e}", query["id"]);
                differ += 1;
                continue;
            }
        };
        let set: BTreeSet<String> =
            answer.hits.iter().map(|h| format!("{}:{}", h.path, h.line)).collect();
        if digest(&set) == query["gold"]["digest"].as_str().unwrap_or_default() {
            same += 1;
        } else {
            differ += 1;
            println!(
                "{} differs: index {} hits, rg {} hits; pattern {:?} path {:?} globs {:?} i={}",
                query["id"],
                set.len(),
                query["gold"]["hits"],
                pattern,
                query["path"],
                strings("globs"),
                request.case_insensitive
            );
        }
    }
    println!("{corpus}: {same} the same, {differ} different");
}
