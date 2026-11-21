//! The benchmark's cases: the frozen held out queries of the search evaluation and the broad queries of
//! the corpus test, each as the arguments an agent sends to `atrius_search` and the Grep tool's ripgrep
//! arguments for the same question.
//!
//! The split is fixed: a case is in the test split when the 32 bit FNV-1a hash of its query text is 0
//! modulo 3, and in the train split otherwise.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// The Grep tool's arguments for a question.
#[derive(Clone, Debug)]
pub struct RgQuery {
    pub pattern: String,
    pub ignore_case: bool,
    pub path: String,
    pub globs: Vec<String>,
    pub types: Vec<String>,
}

/// One benchmark case.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct Case {
    pub id: String,
    pub repo: String,
    pub family: String,
    pub verb: String,
    pub args: Value,
    pub rg: Option<RgQuery>,
    pub split: String,
}

/// The 32 bit FNV-1a hash of a text.
///
/// @param text - the text
pub fn fnv1a(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// The split a query text falls in.
///
/// @param text - the query text
pub fn split_of(text: &str) -> &'static str {
    if fnv1a(text) % 3 == 0 {
        "test"
    } else {
        "train"
    }
}

/// Reads the cases from a JSONL file.
///
/// @param path - the file
pub fn read(path: &str) -> Vec<Case> {
    std::fs::read_to_string(path).expect("the cases file").lines().filter(|l| !l.trim().is_empty()).map(|l| from_json(&serde_json::from_str(l).expect("a case"))).collect()
}

/// A case from its JSON form.
///
/// @param v - the JSON
fn from_json(v: &Value) -> Case {
    let strings = |x: &Value| x.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect()).unwrap_or_default();
    let rg = v.get("rg").filter(|r| !r.is_null()).map(|r| RgQuery {
        pattern: r["pattern"].as_str().unwrap_or_default().to_owned(),
        ignore_case: r["ignoreCase"].as_bool().unwrap_or(false),
        path: r["path"].as_str().unwrap_or(".").to_owned(),
        globs: strings(&r["globs"]),
        types: strings(&r["types"]),
    });
    let text = |k: &str| v[k].as_str().unwrap_or_default().to_owned();
    Case { id: text("id"), repo: text("repo"), family: text("family"), verb: text("verb"), args: v["args"].clone(), rg, split: text("split") }
}

/// A find case from a pattern and its Grep arguments.
///
/// @param id - the case id
/// @param repo - the repository
/// @param family - F2, F3 or broad
/// @param rg - the Grep arguments; the pattern is already a regex
/// @param budget - the token budget, or None for the default an agent gets
fn find_case(id: String, repo: &str, family: &str, rg: RgQuery, budget: Option<u64>) -> Value {
    let mut args = json!({ "query": rg.pattern, "mode": "regex" });
    if !rg.path.is_empty() && rg.path != "." {
        args["path"] = json!(rg.path);
    }
    if !rg.globs.is_empty() {
        args["glob"] = json!(rg.globs.join(" "));
    }
    if let Some(t) = rg.types.first() {
        args["type"] = json!(t);
    }
    if rg.ignore_case {
        args["ignore-case"] = json!(true);
    }
    if let Some(b) = budget {
        args["budget"] = json!(b);
    }
    json!({
        "id": id, "repo": repo, "family": family, "verb": "find", "args": args, "split": split_of(&rg.pattern),
        "rg": { "pattern": rg.pattern, "ignoreCase": rg.ignore_case, "path": if rg.path.is_empty() { ".".to_owned() } else { rg.path }, "globs": rg.globs, "types": rg.types },
    })
}

/// The held out rows of one frozen query file, for the repositories given.
///
/// @param folder - the search evaluation's `queries` folder
/// @param file - the file name
/// @param repos - the repository names
fn held_out(folder: &Path, file: &str, repos: &BTreeSet<String>) -> Vec<Value> {
    std::fs::read_to_string(folder.join(file))
        .expect("a frozen query file")
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|q| q["split"] == "heldout" && q["repo"].as_str().is_some_and(|r| repos.contains(r)))
        .collect()
}

/// The F2 and F3 held out queries as find cases, as the earlier lab timed them: F2 as mined, F3's names
/// as whole words.
///
/// @param folder - the `queries` folder
/// @param repos - the repository names
fn grep_cases(folder: &Path, repos: &BTreeSet<String>) -> Vec<Value> {
    let strings = |x: &Value| -> Vec<String> { x.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect()).unwrap_or_default() };
    let mut out = Vec::new();
    for q in held_out(folder, "F2-exact.jsonl", repos) {
        let raw = q["pattern"].as_str().unwrap_or_default();
        let mut pattern = if q["fixed"] == true { regex::escape(raw) } else { raw.to_owned() };
        if q["word"] == true {
            pattern = format!(r"\b(?:{pattern})\b");
        }
        let rg = RgQuery { pattern, ignore_case: q["ignoreCase"] == true, path: q["path"].as_str().unwrap_or("").to_owned(), globs: strings(&q["globs"]), types: strings(&q["types"]) };
        out.push(find_case(q["id"].as_str().unwrap_or_default().to_owned(), q["repo"].as_str().unwrap_or_default(), "F2", rg, None));
    }
    for q in held_out(folder, "F3-references.jsonl", repos) {
        let pattern = format!(r"\b(?:{})\b", regex::escape(q["name"].as_str().unwrap_or_default()));
        let rg = RgQuery { pattern, ignore_case: false, path: String::new(), globs: vec![], types: vec![] };
        out.push(find_case(q["id"].as_str().unwrap_or_default().to_owned(), q["repo"].as_str().unwrap_or_default(), "F3", rg, None));
    }
    out
}

/// The F1 held out names through `def` and the F4 held out globs through `files`. These have no ripgrep
/// arm; they are timed through the MCP server as a guard on the other verbs.
///
/// @param folder - the `queries` folder
/// @param repos - the repository names
fn verb_cases(folder: &Path, repos: &BTreeSet<String>) -> Vec<Value> {
    let mut out = Vec::new();
    for q in held_out(folder, "F1-identifier.jsonl", repos) {
        let name = q["name"].as_str().unwrap_or_default();
        out.push(json!({ "id": q["id"], "repo": q["repo"], "family": "F1", "verb": "def", "args": { "name": name }, "split": split_of(name), "rg": null }));
    }
    for q in held_out(folder, "F4-files.jsonl", repos) {
        let pattern = q["pattern"].as_str().unwrap_or_default();
        let mut args = json!({ "query": pattern });
        if let Some(p) = q["path"].as_str().filter(|p| !p.is_empty()) {
            args["path"] = json!(p);
        }
        out.push(json!({ "id": q["id"], "repo": q["repo"], "family": "F4", "verb": "files", "args": args, "split": split_of(pattern), "rg": null }));
    }
    out
}

/// Writes the frozen case list: the agent grep cases, the other verbs, and 40 broad queries for
/// ai-service and Linux made by the corpus test's generator, with full output.
///
/// @param folder - the search evaluation's `queries` folder
/// @param repos - the repositories, name and folder
/// @param out - the JSONL file to write
pub fn freeze(folder: &str, repos: &[(String, PathBuf)], out: &str) {
    let names: BTreeSet<String> = repos.iter().map(|(n, _)| n.clone()).collect();
    let folder = Path::new(folder);
    let mut rows = grep_cases(folder, &names);
    rows.extend(verb_cases(folder, &names));
    for (name, root, language) in repos.iter().filter_map(|(n, r)| match n.as_str() {
        "linux" => Some((n, r, "C")),
        "ai-service" => Some((n, r, "TypeScript")),
        _ => None,
    }) {
        for (i, q) in broad::queries(name, root, language, 40).into_iter().enumerate() {
            rows.push(find_case(format!("broad:{name}:{i}"), name, "broad", q, Some(0)));
        }
    }
    let text: String = rows.iter().map(|r| format!("{r}\n")).collect();
    std::fs::write(out, text).expect("the cases are written");
    let mut counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    for r in &rows {
        *counts.entry((r["repo"].as_str().unwrap_or("").to_owned(), r["family"].as_str().unwrap_or("").to_owned())).or_default() += 1;
    }
    eprintln!("{counts:?}");
}

/// A copy of the query generator in `tests/corpora.rs`, which a test file cannot export. It must stay the
/// same as that one, so the Linux broad queries are the corpus test's 40.
mod broad {
    use super::RgQuery;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::Path;

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

    type Word = (String, usize, String, String);

    /// ripgrep's `--type` for a language, as the corpus test maps it.
    fn rg_type(language: &str) -> &'static str {
        match language {
            "TypeScript" => "ts",
            "C" => "c",
            _ => "txt",
        }
    }

    /// Identifiers in 400 sampled files, as the corpus test reads them.
    fn identifiers(root: &Path, rng: &mut Rng) -> (Vec<Word>, Vec<String>) {
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
                        phrases.push(trimmed[at..at + words[0].len() + 1 + words[1].len()].to_owned());
                    }
                }
            }
            for word in in_file {
                seen.entry(word).or_insert((0, folder.clone(), extension.clone())).0 += 1;
            }
        }
        (seen.into_iter().map(|(w, (n, f, e))| (w, n, f, e)).collect(), phrases)
    }

    /// The corpus test's queries for one corpus.
    pub fn queries(name: &str, root: &Path, language: &str, count: usize) -> Vec<RgQuery> {
        let mut rng = Rng::new(name);
        let (words, phrases) = identifiers(root, &mut rng);
        let rare: Vec<&Word> = words.iter().filter(|w| w.1 <= 3).collect();
        let common: Vec<&Word> = words.iter().filter(|w| w.1 > 3 && w.1 < 60).collect();
        let any: Vec<&Word> = words.iter().collect();
        let pool = |rng: &mut Rng| -> Word {
            let list = if rng.below(3) == 0 && !common.is_empty() { &common } else if !rare.is_empty() { &rare } else { &any };
            (*rng.pick(list)).clone()
        };
        let plain = |pattern: String| RgQuery { pattern, ignore_case: false, path: ".".into(), globs: vec![], types: vec![] };
        let anchored = [r"^\s*(class|struct|def|func|fn|function|interface|enum)\s+[A-Z]\w*", r"^#include\s+<\w+", r"^\s*(public|private|protected)\s+\w+", r"^\s*(import|use|require|from)\s"];
        let mut out = Vec::new();
        let mut i = 0;
        while out.len() < count {
            let word = pool(&mut rng);
            let e = regex::escape(&word.0);
            out.push(match i % 14 {
                0..=2 => plain(e),
                3 => RgQuery { ignore_case: true, ..plain(regex::escape(&word.0.to_lowercase())) },
                4 => plain(format!(r"\b{e}\b")),
                5 => plain(format!(r"{e}\s*\(")),
                6 => {
                    let other = pool(&mut rng).0;
                    plain(format!("{e}|{}", regex::escape(&other)))
                }
                7 => plain(anchored[rng.below(anchored.len())].to_owned()),
                8 => match phrases.get(rng.below(phrases.len().max(1))) {
                    Some(phrase) => plain(regex::escape(phrase)),
                    None => plain(e),
                },
                9 if !word.3.is_empty() => RgQuery { globs: vec![format!("*.{}", word.3)], ..plain(e) },
                10 if !word.2.is_empty() => RgQuery { path: word.2.clone(), ..plain(e) },
                11 => RgQuery { types: vec![rg_type(language).to_owned()], ..plain(e) },
                12 => RgQuery { globs: vec!["!*test*".into()], ..plain(e) },
                _ => plain(format!(r"{}\w*", regex::escape(&word.0[..word.0.len().min(6)]))),
            });
            i += 1;
        }
        out
    }
}
