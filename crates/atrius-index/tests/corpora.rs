//! The index against ripgrep on real code: the corpora in `tests/corpora.json`, open source repositories
//! in 13 languages from 30 files to 89,000, each pinned to a release tag.
//!
//! For each corpus that has been fetched (`node tests/fetch-corpora.mjs`), queries are made from the
//! corpus's own text with a seed taken from its name, so a run is repeatable: literals, case insensitive
//! literals, whole words, calls, alternations, anchored regexes, phrases with a space, and the same
//! scoped by `--glob`, `--type` and `--path`. Every answer's set of `path:line` must equal ripgrep's,
//! run with the flags Claude Code's Grep tool uses.
//!
//! Then `def` must find every hand checked definition listed for the corpus in its first three answers,
//! and `outline` and `fragment` must agree with each other on those files.
//!
//! | Variable | Effect |
//! |---|---|
//! | `ATRIUS_RG` | the ripgrep to compare with; `rg` on the path otherwise |
//! | `ATRIUS_CORPORA` | where the corpora are; `D:/atrius-corpora` or `~/.cache/atrius-corpora` otherwise |
//! | `ATRIUS_CORPUS` | a comma list of corpus names to run, instead of every fetched one |
//! | `ATRIUS_REQUIRE_CORPORA=1` | a corpus that is not fetched fails the test instead of being skipped |
//! | `ATRIUS_QUERIES` | queries per corpus, 120 by default and a third of that on the extra large tier |
//! | `ATRIUS_REPORT` | where the JSON report goes; `target/corpus-report.json` otherwise |

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use atrius_index::exact::{Exact, ExactRequest};
use atrius_index::files::Scope;
use serde_json::{json, Map, Value};

/// One query, as the Grep tool's arguments put it.
#[derive(Clone, Debug)]
struct Query {
    kind: &'static str,
    pattern: String,
    insensitive: bool,
    path: String,
    globs: Vec<String>,
    types: Vec<String>,
}

/// A small repeatable random number source.
struct Rng(u64);

impl Rng {
    fn new(seed: &str) -> Rng {
        Rng(blake3::hash(seed.as_bytes()).as_bytes()[..8].iter().fold(0u64, |a, b| (a << 8) | *b as u64) | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// The repository's `tests` folder.
fn tests_folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("tests")
}

/// Where the corpora are.
fn corpora_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("ATRIUS_CORPORA") {
        return PathBuf::from(dir);
    }
    if cfg!(windows) && Path::new("D:/").exists() {
        return PathBuf::from("D:/atrius-corpora");
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
    home.join(".cache").join("atrius-corpora")
}

/// The ripgrep to compare with.
fn ripgrep() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ATRIUS_RG") {
        return Some(PathBuf::from(path));
    }
    Command::new("rg").arg("--version").output().ok().filter(|o| o.status.success()).map(|_| PathBuf::from("rg"))
}

/// The folder a fetched corpus is in, if it has been fetched.
///
/// @param name - the corpus
fn corpus_folder(name: &str) -> Option<PathBuf> {
    let lock: Value = std::fs::read_to_string(corpora_root().join("corpora.lock.json")).ok().and_then(|t| serde_json::from_str(&t).ok())?;
    let folder = PathBuf::from(lock.get(name)?.get("folder")?.as_str()?);
    folder.is_dir().then_some(folder)
}

/// ripgrep's `--type` name for a corpus language.
///
/// @param language - the language as `corpora.json` writes it
fn rg_type(language: &str) -> &'static str {
    match language {
        "Rust" => "rust",
        "Go" => "go",
        "Python" => "py",
        "JavaScript" => "js",
        "TypeScript" => "ts",
        "Java" => "java",
        "C" => "c",
        "C++" => "cpp",
        "C#" => "csharp",
        "Ruby" => "ruby",
        "PHP" => "php",
        "Kotlin" => "kotlin",
        "Swift" => "swift",
        "Scala" => "scala",
        "Zig" => "zig",
        _ => "txt",
    }
}

/// Identifiers in the corpus's own text, with how many sampled files hold each, and the folder each
/// was first seen in.
///
/// @param root - the corpus
/// @param rng - the random source
fn identifiers(root: &Path, rng: &mut Rng) -> (Vec<(String, usize, String, String)>, Vec<String>) {
    let files = atrius_index::files::walk(root);
    let mut rels: Vec<String> = files.iter().map(|f| f.rel.clone()).collect();
    rels.sort();
    let mut seen: BTreeMap<String, (usize, String, String)> = BTreeMap::new();
    let mut phrases: Vec<String> = Vec::new();
    for _ in 0..rels.len().min(400) {
        let rel = rng.pick(&rels).clone();
        let Ok(bytes) = std::fs::read(root.join(&rel)) else { continue };
        if bytes.len() > 512 * 1024 || bytes.contains(&0) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else { continue };
        let folder = rel.split('/').next().filter(|_| rel.contains('/')).unwrap_or_default().to_owned();
        let extension = Path::new(&rel).extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        let mut in_file: BTreeSet<String> = BTreeSet::new();
        for line in text.lines().take(4000) {
            let words: Vec<&str> = line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).filter(|w| w.len() >= 5 && w.len() <= 30).collect();
            for word in &words {
                if word.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_') && word.chars().any(|c| c.is_ascii_alphabetic()) {
                    in_file.insert((*word).to_owned());
                }
            }
            if phrases.len() < 200 && words.len() >= 2 && rng.below(40) == 0 {
                let trimmed = line.trim();
                if let Some(at) = trimmed.find(&format!("{} {}", words[0], words[1])) {
                    let phrase = &trimmed[at..at + words[0].len() + 1 + words[1].len()];
                    phrases.push(phrase.to_owned());
                }
            }
        }
        for word in in_file {
            let entry = seen.entry(word).or_insert((0, folder.clone(), extension.clone()));
            entry.0 += 1;
        }
    }
    (seen.into_iter().map(|(w, (n, f, e))| (w, n, f, e)).collect(), phrases)
}

/// Escapes a literal for a regex.
///
/// @param text - the literal
fn escape(text: &str) -> String {
    regex::escape(text)
}

/// The queries for one corpus.
///
/// @param name - the corpus's name, the seed
/// @param root - the corpus
/// @param language - its main language
/// @param count - how many
fn queries(name: &str, root: &Path, language: &str, count: usize) -> Vec<Query> {
    let mut rng = Rng::new(name);
    let (words, phrases) = identifiers(root, &mut rng);
    assert!(!words.is_empty(), "{name}: no identifiers were found in the sampled files");
    let rare: Vec<&(String, usize, String, String)> = words.iter().filter(|w| w.1 <= 3).collect();
    let common: Vec<&(String, usize, String, String)> = words.iter().filter(|w| w.1 > 3 && w.1 < 60).collect();
    let any: Vec<&(String, usize, String, String)> = words.iter().collect();
    let pool = |rng: &mut Rng| -> &(String, usize, String, String) {
        let list = if rng.below(3) == 0 && !common.is_empty() { &common } else if !rare.is_empty() { &rare } else { &any };
        *rng.pick(list)
    };
    let plain = |kind, pattern: String| Query { kind, pattern, insensitive: false, path: ".".into(), globs: vec![], types: vec![] };
    let mut out: Vec<Query> = Vec::new();
    let anchored = [r"^\s*(class|struct|def|func|fn|function|interface|enum)\s+[A-Z]\w*", r"^#include\s+<\w+", r"^\s*(public|private|protected)\s+\w+", r"^\s*(import|use|require|from)\s"];
    let mut i = 0;
    while out.len() < count {
        let word = pool(&mut rng).clone();
        let q = match i % 14 {
            0 | 1 | 2 => plain("literal", escape(&word.0)),
            3 => Query { insensitive: true, ..plain("ignore case", escape(&word.0.to_lowercase())) },
            4 => plain("whole word", format!(r"\b{}\b", escape(&word.0))),
            5 => plain("call", format!(r"{}\s*\(", escape(&word.0))),
            6 => {
                let other = pool(&mut rng).0.clone();
                plain("alternation", format!("{}|{}", escape(&word.0), escape(&other)))
            }
            7 => plain("anchored", anchored[rng.below(anchored.len())].to_owned()),
            8 => match phrases.get(rng.below(phrases.len().max(1))) {
                Some(phrase) => plain("phrase", escape(phrase)),
                None => plain("literal", escape(&word.0)),
            },
            9 if !word.3.is_empty() => Query { globs: vec![format!("*.{}", word.3)], ..plain("glob", escape(&word.0)) },
            10 if !word.2.is_empty() => Query { path: word.2.clone(), ..plain("path", escape(&word.0)) },
            11 => Query { types: vec![rg_type(language).to_owned()], ..plain("type", escape(&word.0)) },
            12 => Query { globs: vec!["!*test*".into()], ..plain("negated glob", escape(&word.0)) },
            _ => plain("prefix regex", format!(r"{}\w*", escape(&word.0[..word.0.len().min(6)]))),
        };
        out.push(q);
        i += 1;
    }
    out
}

/// What ripgrep answers, with the Grep tool's flags, as a set of `path:line`.
///
/// @param rg - the ripgrep executable
/// @param root - the corpus
/// @param q - the query
fn rg_set(rg: &Path, root: &Path, q: &Query) -> Result<BTreeSet<String>, String> {
    let mut cmd = Command::new(rg);
    cmd.current_dir(root).args(["--no-config", "--hidden"]);
    for folder in [".git", ".svn", ".hg", ".bzr", ".jj", ".sl"] {
        cmd.args(["--glob", &format!("!{folder}")]);
    }
    for glob in &q.globs {
        cmd.args(["--glob", glob]);
    }
    for t in &q.types {
        cmd.args(["--type", t]);
    }
    cmd.args(["--max-columns", "500"]);
    if q.insensitive {
        cmd.arg("-i");
    }
    cmd.args(["--json", "-n", "-e", &q.pattern, &q.path]);
    let out = cmd.output().map_err(|e| e.to_string())?;
    // Code 2 with only file errors on standard error is ripgrep searching everything it could read and
    // naming what it could not: the Linux tree's symlinks under `scripts/dtc/include-prefixes` cannot be
    // opened on Windows. Its matches are still complete for every other file, which is what the index
    // has too. A refused pattern is code 2 with no match at all.
    let errors = String::from_utf8_lossy(&out.stderr);
    let only_file_errors = !errors.trim().is_empty() && errors.lines().all(|l| l.contains("(os error"));
    if !out.status.success() && out.status.code() != Some(1) && !(out.status.code() == Some(2) && only_file_errors) {
        return Err(errors.trim().to_owned());
    }
    let mut set = BTreeSet::new();
    for line in out.stdout.split(|b| *b == b'\n') {
        if !line.starts_with(b"{\"type\":\"match\"") {
            continue;
        }
        let event: Value = serde_json::from_slice(line).unwrap_or_default();
        let data = &event["data"]["path"];
        let file = data["text"].as_str().map(str::to_owned).unwrap_or_else(|| {
            // A path that is not UTF-8 comes as base64 bytes; it is compared lossily, as the index does.
            data["bytes"].as_str().unwrap_or_default().to_owned()
        });
        let file = file.replace('\\', "/");
        set.insert(format!("{}:{}", file.trim_start_matches("./"), event["data"]["line_number"]));
    }
    Ok(set)
}

/// The index's answer, as a set of `path:line`.
///
/// @param index - the exact index
/// @param root - the corpus
/// @param q - the query
fn index_set(index: &Exact, root: &Path, q: &Query) -> Result<BTreeSet<String>, String> {
    let scope = Scope::new(root, &q.path, &q.globs, &q.types)?;
    let request = ExactRequest { pattern: &q.pattern, case_insensitive: q.insensitive, scope: &scope };
    Ok(index.search(root, &request)?.hits.iter().map(|h| format!("{}:{}", h.path, h.line)).collect())
}

/// The parity of one corpus: how many queries, how many equal, the differences, and the timings.
///
/// @param rg - ripgrep
/// @param name - the corpus
/// @param root - its folder
/// @param language - its main language
/// @param count - how many queries
fn parity(rg: &Path, name: &str, root: &Path, language: &str, count: usize) -> Value {
    let built = Instant::now();
    let index = Exact::build(root);
    let build_ms = built.elapsed().as_millis();
    let files = index.live();
    let qs = queries(name, root, language, count);
    let (mut equal, mut both_refused, mut index_ms, mut rg_ms, mut hits) = (0, 0, 0f64, 0f64, 0usize);
    let mut differences: Vec<Value> = Vec::new();
    let mut by_kind: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let (mut index_times, mut rg_times): (Vec<f64>, Vec<f64>) = (Vec::new(), Vec::new());
    for q in &qs {
        let t = Instant::now();
        let ours = index_set(&index, root, q);
        index_ms += t.elapsed().as_secs_f64() * 1000.0;
        index_times.push(t.elapsed().as_secs_f64() * 1000.0);
        let t = Instant::now();
        let theirs = rg_set(rg, root, q);
        rg_ms += t.elapsed().as_secs_f64() * 1000.0;
        rg_times.push(t.elapsed().as_secs_f64() * 1000.0);
        let entry = by_kind.entry(q.kind).or_insert((0, 0));
        entry.0 += 1;
        match (ours, theirs) {
            (Ok(a), Ok(b)) if a == b => {
                equal += 1;
                entry.1 += 1;
                hits += a.len();
            }
            (Err(_), Err(_)) => {
                both_refused += 1;
                equal += 1;
                entry.1 += 1;
            }
            (a, b) => {
                let describe = |s: &Result<BTreeSet<String>, String>| match s {
                    Ok(set) => json!({ "count": set.len() }),
                    Err(e) => json!({ "refused": e }),
                };
                let (only_index, only_rg) = match (&a, &b) {
                    (Ok(a), Ok(b)) => (a.difference(b).take(5).cloned().collect::<Vec<_>>(), b.difference(a).take(5).cloned().collect::<Vec<_>>()),
                    _ => (vec![], vec![]),
                };
                differences.push(json!({ "kind": q.kind, "pattern": q.pattern, "insensitive": q.insensitive, "path": q.path, "globs": q.globs, "types": q.types, "index": describe(&a), "rg": describe(&b), "onlyIndex": only_index, "onlyRg": only_rg }));
            }
        }
    }
    json!({
        "corpus": name,
        "files": files,
        "buildMs": build_ms,
        "queries": qs.len(),
        "equal": equal,
        "bothRefused": both_refused,
        "lines": hits,
        "indexMs": (index_ms * 10.0).round() / 10.0,
        "rgMs": (rg_ms * 10.0).round() / 10.0,
        "indexMedianMs": median(&mut index_times),
        "rgMedianMs": median(&mut rg_times),
        "byKind": by_kind.iter().map(|(k, (n, e))| (k.to_string(), json!({ "queries": n, "equal": e }))).collect::<Map<String, Value>>(),
        "differences": differences,
    })
}

/// The median of some timings, to two places.
///
/// @param times - the timings
fn median(times: &mut [f64]) -> f64 {
    if times.is_empty() {
        return 0.0;
    }
    times.sort_by(|a, b| a.total_cmp(b));
    (times[times.len() / 2] * 100.0).round() / 100.0
}

/// Whether `def` finds each listed definition in its first three answers, and whether `outline` and
/// `fragment` agree on the file it is in.
///
/// @param root - the corpus
/// @param definitions - the hand checked list
fn definitions(root: &Path, definitions: &[Value]) -> Value {
    if definitions.is_empty() {
        return json!({ "checked": 0, "found": 0, "missed": [] });
    }
    let index = atrius_index::index::Index::open_quietly(root);
    assert!(index.wait_ready(std::time::Duration::from_secs(1800)), "the index did not become ready");
    let mut found = 0;
    let mut missed: Vec<Value> = Vec::new();
    for d in definitions {
        let name = d["name"].as_str().unwrap_or_default();
        let file = d["file"].as_str().unwrap_or_default();
        let mut args = Map::new();
        args.insert("name".into(), Value::String(name.into()));
        args.insert("structured".into(), Value::Bool(true));
        let answer = atrius_index::verbs::answer(&index, "def", &args).map_err(|r| r.message);
        let top: Vec<(String, u64)> = answer.as_ref().ok().and_then(|v| v["definitions"].as_array().cloned()).unwrap_or_default().iter().take(3).map(|v| (v["path"].as_str().unwrap_or_default().to_owned(), v["line"].as_u64().unwrap_or(0))).collect();
        let Some((path, line)) = top.iter().find(|(p, _)| p.ends_with(file)).cloned() else {
            missed.push(json!({ "name": name, "file": file, "got": top.iter().map(|(p, l)| format!("{p}:{l}")).collect::<Vec<_>>() }));
            continue;
        };
        // The outline of that file lists the definition, and a fragment at its line names it.
        let mut args = Map::new();
        args.insert("path".into(), Value::String(path.clone()));
        args.insert("structured".into(), Value::Bool(true));
        let outline = atrius_index::verbs::answer(&index, "outline", &args).map(|v| v["text"].as_str().unwrap_or_default().to_owned()).unwrap_or_default();
        let mut args = Map::new();
        args.insert("target".into(), Value::String(format!("{path}:{line}")));
        let fragment = atrius_index::verbs::answer(&index, "fragment", &args).map(|v| v["text"].as_str().unwrap_or_default().to_owned()).unwrap_or_default();
        if outline.contains(name) && fragment.contains(name) {
            found += 1;
        } else {
            missed.push(json!({ "name": name, "file": file, "outlineHasIt": outline.contains(name), "fragmentHasIt": fragment.contains(name) }));
        }
    }
    json!({ "checked": definitions.len(), "found": found, "missed": missed })
}

/// Ignored by default: it reads gigabytes of source and is meant for a release build. Run it with
/// `cargo test --release -p atrius-index --test corpora -- --ignored --nocapture`.
#[test]
#[ignore = "reads the fetched corpora; run with --ignored in a release build"]
fn every_fetched_corpus_matches_ripgrep_and_finds_its_definitions() {
    let Some(rg) = ripgrep() else {
        eprintln!("no ripgrep to compare with: set ATRIUS_RG to one");
        return;
    };
    // The index files of the definition checks go to a scratch folder, never a person's own cache.
    let cache = tempfile::tempdir().expect("a scratch cache");
    std::env::set_var("ATRIUS_INDEX_CACHE", cache.path());
    std::env::set_var("ATRIUS_EMBED", "off");
    let manifest: Value = serde_json::from_str(&std::fs::read_to_string(tests_folder().join("corpora.json")).expect("corpora.json")).expect("corpora.json reads");
    let wanted: Option<Vec<String>> = std::env::var("ATRIUS_CORPUS").ok().map(|v| v.split(',').map(|s| s.trim().to_owned()).collect());
    let require = std::env::var("ATRIUS_REQUIRE_CORPORA").is_ok_and(|v| v == "1");
    let per_corpus: usize = std::env::var("ATRIUS_QUERIES").ok().and_then(|v| v.parse().ok()).unwrap_or(120);
    let mut report: Vec<Value> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for corpus in manifest["corpora"].as_array().expect("corpora") {
        let name = corpus["name"].as_str().unwrap_or_default();
        if wanted.as_ref().is_some_and(|w| !w.iter().any(|n| n == name)) {
            continue;
        }
        let Some(root) = corpus_folder(name) else {
            if require {
                failures.push(format!("{name}: not fetched"));
            } else {
                eprintln!("{name}: not fetched, skipped. `node tests/fetch-corpora.mjs` fetches it");
            }
            continue;
        };
        let language = corpus["language"].as_str().unwrap_or_default();
        let count = if corpus["size"] == "extra large" { per_corpus / 3 } else { per_corpus };
        let started = Instant::now();
        let mut result = parity(&rg, name, &root, language, count);
        let defs = definitions(&root, corpus["definitions"].as_array().map(Vec::as_slice).unwrap_or_default());
        result["definitions"] = defs.clone();
        result["language"] = json!(language);
        result["size"] = corpus["size"].clone();
        result["seconds"] = json!(started.elapsed().as_secs());
        eprintln!(
            "{name:<16} {language:<11} {:>6} files  parity {}/{}  definitions {}/{}  index {:.1} ms  rg {:.1} ms",
            result["files"], result["equal"], result["queries"], defs["found"], defs["checked"], result["indexMs"].as_f64().unwrap_or(0.0), result["rgMs"].as_f64().unwrap_or(0.0)
        );
        if result["equal"] != result["queries"] {
            failures.push(format!("{name}: {} of {} queries differ from ripgrep: {}", result["queries"].as_u64().unwrap_or(0) - result["equal"].as_u64().unwrap_or(0), result["queries"], serde_json::to_string(&result["differences"]).unwrap_or_default()));
        }
        if defs["found"] != defs["checked"] {
            failures.push(format!("{name}: definitions missed: {}", defs["missed"]));
        }
        report.push(result);
    }
    let report_path = std::env::var_os("ATRIUS_REPORT").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("target").join("corpus-report.json"));
    if let Some(parent) = report_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&report_path, serde_json::to_string_pretty(&json!({ "ripgrep": rg.display().to_string(), "corpora": report })).unwrap_or_default());
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
