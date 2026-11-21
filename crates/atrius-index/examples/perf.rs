//! The whole call benchmark of the performance hill climb (`task-2696`, TDD `task-2695`).
//!
//! It times a query the way a caller pays for it, from request to complete answer, with the freshness
//! gate and the shaping included, through four arms timed in the same pass:
//!
//! | Arm | What is timed |
//! |---|---|
//! | `rg` | a ripgrep process with the flags Claude Code's Grep tool passes |
//! | `mcp` | a `tools/call` written to a running `atrius-mcp`, which hosts the index in its own process, until its reply line is read |
//! | `serve` | one request to a running `atrius serve` over its loopback socket |
//! | `oneshot` | an `atrius find` process that asks that running host |
//!
//! Subcommands:
//!
//!   perf freeze --queries <search-eval queries folder> --repo name=folder... --out cases.jsonl
//!   perf prepare --bin <folder with atrius and atrius-mcp> --cache <folder> --repo name=folder...
//!   perf pass --cases cases.jsonl --bin <folder> --cache <folder> --repo name=folder --build <label> --pass <n> --out raw.jsonl
//!   perf buildtime --bin <folder> --scratch <folder> --repo name=folder --build <label> --out raw.jsonl
//!   perf engine --cases cases.jsonl --repo name=folder --build <label> --out raw.jsonl
//!
//! `--cache` holds two cache folders, `a` for the MCP server and `b` for the separate host, because one
//! index folder has one host. Every process runs with `ATRIUS_EMBED=off`. `ATRIUS_HOLD_CONTENT` and any
//! other setting are passed through from the environment.

#[path = "perf/cases.rs"]
mod cases;
#[path = "perf/clients.rs"]
mod clients;

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{json, Value};

use cases::Case;
use clients::{Mcp, Serve};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = Options::read(&args[1..]);
    match args.first().map(String::as_str) {
        Some("freeze") => cases::freeze(&opts.one("queries"), &opts.repos(), &opts.one("out")),
        Some("prepare") => prepare(&opts),
        Some("pass") => pass(&opts),
        Some("buildtime") => build_time(&opts),
        Some("engine") => engine(&opts),
        _ => eprintln!("perf freeze|prepare|pass|buildtime|engine; see the top of examples/perf.rs"),
    }
}

/// The command line's `--name value` pairs; a name may repeat.
struct Options(Vec<(String, String)>);

impl Options {
    /// Reads `--name value` pairs.
    ///
    /// @param words - the words after the subcommand
    fn read(words: &[String]) -> Options {
        let mut out = Vec::new();
        let mut at = 0;
        while at + 1 < words.len() {
            out.push((words[at].trim_start_matches("--").to_owned(), words[at + 1].clone()));
            at += 2;
        }
        Options(out)
    }

    /// The one value of a name, or a stop when it is missing.
    ///
    /// @param name - the option's name
    fn one(&self, name: &str) -> String {
        self.get(name).unwrap_or_else(|| panic!("--{name} is needed"))
    }

    /// The value of a name, if given.
    ///
    /// @param name - the option's name
    fn get(&self, name: &str) -> Option<String> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
    }

    /// Every `--repo name=folder`, in order.
    fn repos(&self) -> Vec<(String, PathBuf)> {
        self.0
            .iter()
            .filter(|(n, _)| n == "repo")
            .filter_map(|(_, v)| v.split_once('=').map(|(a, b)| (a.to_owned(), PathBuf::from(b))))
            .collect()
    }
}

/// Appends rows to a JSONL file.
///
/// @param out - the file
/// @param rows - the rows
fn append(out: &str, rows: &[Value]) {
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(out).expect("the output file");
    for row in rows {
        writeln!(file, "{row}").expect("a row is written");
    }
}

/// Builds each repository's index from nothing in cache `a`, waits for the passage table, then copies
/// it to cache `b`, so every timed pass loads a saved index the way a person's second session does.
///
/// @param opts - `--bin`, `--cache` and `--repo`
fn prepare(opts: &Options) {
    let (bin, cache) = (PathBuf::from(opts.one("bin")), PathBuf::from(opts.one("cache")));
    for (name, root) in opts.repos() {
        let started = Instant::now();
        let mut mcp = Mcp::start(&bin, &root, &cache.join("a"));
        mcp.wait_until(|s| s["ready"] == true && s["passagesReady"] == true, 7200.0);
        eprintln!("{name}: built in {:.1} s", started.elapsed().as_secs_f64());
        mcp.stop();
        let (a, b) = (cache.join("a"), cache.join("b"));
        let _ = std::fs::create_dir_all(&b);
        let status = std::process::Command::new("cp").arg("-R").arg(format!("{}/.", a.display())).arg(&b).status();
        assert!(status.is_ok_and(|s| s.success()), "the cache is copied");
    }
}

/// One timed pass over one repository's cases, in two halves: ripgrep and the MCP server, then ripgrep,
/// the separate host and the one shot command. The MCP server and the separate host never run at once,
/// because each would see the other's fence files. In each half the arms rotate from case to case. The
/// first pass also checks every Atrius answer's lines against ripgrep's. `--passes K` runs K passes in the
/// same two processes, numbered from `--pass`, for a repository whose saved index takes minutes to load;
/// its start up is then measured once.
///
/// @param opts - `--cases`, `--bin`, `--cache`, `--repo`, `--build`, `--pass`, `--passes`, `--out`, `--rg`, `--arms`
fn pass(opts: &Options) {
    let process_started = Instant::now();
    let (bin, cache) = (PathBuf::from(opts.one("bin")), PathBuf::from(opts.one("cache")));
    let (name, root) = opts.repos().into_iter().next().expect("--repo");
    let (build, out) = (opts.one("build"), opts.one("out"));
    let number: usize = opts.one("pass").parse().expect("--pass is a number");
    let passes: usize = opts.get("passes").and_then(|p| p.parse().ok()).unwrap_or(1);
    let rg = opts.get("rg").unwrap_or_else(|| "rg".into());
    let arms: Vec<String> = opts.get("arms").unwrap_or_else(|| "rg,mcp,serve,oneshot".into()).split(',').map(str::to_owned).collect();
    let all = cases::read(&opts.one("cases"));
    let mine: Vec<&Case> = all.iter().filter(|c| c.repo == name).collect();
    let tag_as = |row: Value, n: usize| -> Value { tagged(row, &build, n, &name) };
    let tag = |row: Value| -> Value { tag_as(row, number) };
    let half = |wanted: &[&str]| -> Vec<String> { arms.iter().filter(|a| wanted.contains(&a.as_str())).cloned().collect() };
    let mut mcp = Mcp::start(&bin, &root, &cache.join("a"));
    let first = mcp.first_answer(mine.iter().find(|c| c.verb == "find").copied(), process_started);
    append(&out, &[tag(json!({ "kind": "startup", "arm": "mcp", "firstAnyMs": first.0, "firstIndexMs": first.1 }))]);
    if number == 0 {
        append(&out, &parity_rows(&mine, &mut mcp, &rg, &root).into_iter().map(tag).collect::<Vec<_>>());
    }
    for n in number..number + passes {
        let rows = timed_half(&mine, n, &half(&["rg", "mcp"]), &mut mcp, None, &bin, &cache, &root, &rg, "a");
        append(&out, &rows.into_iter().map(|r| tag_as(r, n)).collect::<Vec<_>>());
    }
    let memory = json!({ "kind": "memory", "arm": "mcp", "rssKb": clients::rss_kb(mcp.pid()), "status": mcp.status() });
    append(&out, &[tag(memory)]);
    mcp.stop();
    let second = half(&["rg", "serve", "oneshot"]);
    if second.iter().any(|a| a != "rg") {
        let mut serve = Serve::start(&bin, &root, &cache.join("b"));
        append(&out, &[tag(json!({ "kind": "startup", "arm": "serve", "firstIndexMs": serve.ready_ms }))]);
        for n in number..number + passes {
            let rows = timed_half(&mine, n, &second, &mut mcp_placeholder(), Some(&mut serve), &bin, &cache, &root, &rg, "b");
            append(&out, &rows.into_iter().map(|r| tag_as(r, n)).collect::<Vec<_>>());
        }
        append(&out, &[tag(json!({ "kind": "memory", "arm": "serve", "status": serve.call("status", &json!({})) }))]);
        serve.stop(&bin, &root, &cache.join("b"));
    }
}

/// Stands in for the MCP server in the half that does not use it.
fn mcp_placeholder() -> Option<Mcp> {
    None
}

/// Times every case through the given arms, rotating the order of cases by pass and of arms by case.
#[allow(clippy::too_many_arguments)]
fn timed_half(cases: &[&Case], number: usize, arms: &[String], mcp: &mut impl McpSlot, mut serve: Option<&mut Serve>, bin: &Path, cache: &Path, root: &Path, rg: &str, half: &str) -> Vec<Value> {
    let mut rows = Vec::new();
    for (i, case) in rotated(cases, number).into_iter().enumerate() {
        for arm in rotated_arms(arms, i + number) {
            if let Some((ms, work)) = time_arm(&arm, case, mcp.slot(), serve.as_deref_mut(), bin, cache, root, rg) {
                let gate = work["gateMicros"].as_f64().map(|us| us / 1000.0);
                rows.push(json!({ "kind": "call", "case": case.id, "arm": arm, "half": half, "ms": ms, "gateMs": gate }));
            }
        }
    }
    rows
}

/// Either the running MCP server or nothing.
trait McpSlot {
    /// The server, if this slot holds one.
    fn slot(&mut self) -> Option<&mut Mcp>;
}

impl McpSlot for Mcp {
    fn slot(&mut self) -> Option<&mut Mcp> {
        Some(self)
    }
}

impl McpSlot for Option<Mcp> {
    fn slot(&mut self) -> Option<&mut Mcp> {
        self.as_mut()
    }
}

/// A row with the build, pass and repository it belongs to.
///
/// @param row - the row
/// @param build - the build's label
/// @param number - the pass
/// @param repo - the repository
fn tagged(mut row: Value, build: &str, number: usize, repo: &str) -> Value {
    row["build"] = json!(build);
    row["pass"] = json!(number);
    row["repo"] = json!(repo);
    row
}

/// The cases in an order that starts at a different place in each pass.
///
/// @param cases - the cases
/// @param number - the pass
fn rotated<'a>(cases: &[&'a Case], number: usize) -> Vec<&'a Case> {
    if cases.is_empty() {
        return Vec::new();
    }
    let start = (number * 7919) % cases.len();
    cases[start..].iter().chain(cases[..start].iter()).copied().collect()
}

/// The arms in an order that rotates from case to case.
///
/// @param arms - the arms
/// @param i - the case's place in the pass
fn rotated_arms(arms: &[String], i: usize) -> Vec<String> {
    let start = i % arms.len();
    arms[start..].iter().chain(arms[..start].iter()).cloned().collect()
}

/// Times one call through one arm, or None when the arm does not take this kind of case. A find through
/// the MCP server or the separate host asks for the structured answer too, so the sample carries the
/// gate's own time from the same call; every build is timed the same way.
#[allow(clippy::too_many_arguments)]
fn time_arm(arm: &str, case: &Case, mcp: Option<&mut Mcp>, serve: Option<&mut Serve>, bin: &Path, cache: &Path, root: &Path, rg: &str) -> Option<(f64, Value)> {
    let find = case.verb == "find";
    let mut args = case.args.clone();
    if find {
        args["structured"] = json!(true);
    }
    let started = Instant::now();
    let work = match arm {
        "mcp" => mcp?.call(&case.verb, &args)["structuredContent"]["work"].clone(),
        "rg" if find && case.rg.is_some() => {
            clients::ripgrep(rg, root, case.rg.as_ref()?);
            Value::Null
        }
        "serve" if find => serve?.call(&case.verb, &args)["work"].clone(),
        "oneshot" if find => {
            clients::oneshot(bin, root, &cache.join("b"), &case.args);
            Value::Null
        }
        _ => return None,
    };
    Some((started.elapsed().as_secs_f64() * 1000.0, work))
}

/// Every find case's lines through the MCP server with no budget, against ripgrep's.
///
/// @param cases - the cases
/// @param mcp - the server
/// @param rg - ripgrep
/// @param root - the repository
fn parity_rows(cases: &[&Case], mcp: &mut Mcp, rg: &str, root: &Path) -> Vec<Value> {
    let mut rows = Vec::new();
    for case in cases.iter().filter(|c| c.verb == "find" && c.rg.is_some()) {
        let mut args = case.args.clone();
        args["budget"] = json!(0);
        args["structured"] = json!(true);
        let reply = mcp.call("find", &args);
        let ours: std::collections::BTreeSet<String> = reply["structuredContent"]["hits"]
            .as_array()
            .map(|hits| hits.iter().map(|h| format!("{}:{}", h[0].as_str().unwrap_or(""), h[1])).collect())
            .unwrap_or_default();
        let theirs = clients::ripgrep(rg, root, case.rg.as_ref().expect("rg"));
        rows.push(json!({ "kind": "parity", "case": case.id, "equal": ours == theirs, "atrius": ours.len(), "rg": theirs.len() }));
    }
    rows
}

/// The time from an empty cache until the index is ready, through the MCP server, three times.
///
/// @param opts - `--bin`, `--scratch`, `--repo`, `--build`, `--out`
fn build_time(opts: &Options) {
    let (bin, scratch) = (PathBuf::from(opts.one("bin")), PathBuf::from(opts.one("scratch")));
    let reps: usize = opts.get("reps").and_then(|r| r.parse().ok()).unwrap_or(3);
    for (name, root) in opts.repos() {
        for rep in 0..reps {
            let cache = scratch.join(format!("{name}-{rep}"));
            let _ = std::fs::remove_dir_all(&cache);
            let started = Instant::now();
            let mut mcp = Mcp::start(&bin, &root, &cache);
            mcp.wait_until(|s| s["ready"] == true, 3600.0);
            let ready = started.elapsed().as_secs_f64() * 1000.0;
            mcp.wait_until(|s| s["passagesReady"] == true, 7200.0);
            let passages = started.elapsed().as_secs_f64() * 1000.0;
            let status = mcp.status();
            mcp.stop();
            let row = json!({ "kind": "build", "rep": rep, "readyMs": ready, "passagesMs": passages, "indexBytes": clients::folder_bytes(&cache), "status": status });
            append(&opts.one("out"), &[tagged(row, &opts.one("build"), rep, &name)]);
            let _ = std::fs::remove_dir_all(&cache);
        }
    }
}

/// `Exact::search` alone, inside this process, on an index built here: the number the earlier lab
/// published, so the whole call can be compared with it.
///
/// @param opts - `--cases`, `--repo`, `--build`, `--out`, `--reps`
fn engine(opts: &Options) {
    use atrius_index::exact::{Exact, ExactRequest};
    use atrius_index::files::Scope;
    let reps: usize = opts.get("reps").and_then(|r| r.parse().ok()).unwrap_or(10);
    let all = cases::read(&opts.one("cases"));
    for (name, root) in opts.repos() {
        let index = Exact::build(&root);
        let mut times: HashMap<String, Vec<f64>> = HashMap::new();
        for rep in 0..reps {
            for case in all.iter().filter(|c| c.repo == name && c.verb == "find") {
                let Some(q) = case.rg.as_ref() else { continue };
                let Ok(scope) = Scope::new(&root, &q.path, &q.globs, &q.types) else { continue };
                let request = ExactRequest { pattern: &q.pattern, case_insensitive: q.ignore_case, scope: &scope };
                let started = Instant::now();
                let _ = index.search(&root, &request);
                times.entry(case.id.clone()).or_default().push(started.elapsed().as_secs_f64() * 1000.0);
            }
            eprintln!("{name}: engine rep {rep}");
        }
        let rows: Vec<Value> = times
            .into_iter()
            .map(|(id, ms)| tagged(json!({ "kind": "engine", "case": id, "ms": ms }), &opts.one("build"), 0, &name))
            .collect();
        append(&opts.one("out"), &rows);
    }
}
