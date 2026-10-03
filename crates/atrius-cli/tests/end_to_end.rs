//! The built `atrius` and `atrius-mcp` programs, run the way a person and an agent run them.
//!
//! Every test uses its own scratch cache, configuration folder and checkout, so nothing touches the
//! machine's own index or watch list, and every host a test starts is stopped by the test.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// A scratch checkout, cache and configuration folder, and the environment that points at them.
struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
    config: PathBuf,
}

impl Scratch {
    /// A git checkout holding a few files in four languages.
    fn new() -> Scratch {
        let dir = tempfile::tempdir().expect("a scratch folder");
        let root = dir.path().join("project");
        let write = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("src/lib.rs", "/// Adds two numbers.\npub fn add_numbers(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub struct RetryPolicy {\n    pub attempts: u32,\n}\n\nfn retry_write(policy: &RetryPolicy) -> bool {\n    add_numbers(1, 2) > policy.attempts as i32\n}\n");
        write("server/main.go", "package main\n\ntype Server struct {\n\tport int\n}\n\nfunc (s *Server) ListenAndServe(addr string) error {\n\treturn nil\n}\n\nfunc main() {\n\ts := &Server{}\n\t_ = s.ListenAndServe(\":80\")\n}\n");
        write("tools/report.py", "class Report:\n    def render(self, rows):\n        return [r for r in rows if r]\n\n\ndef retry_write(times):\n    return times > 0\n");
        write("web/app.ts", "export function createServer(port: number): string {\n  return `listening on ${port}`;\n}\n\nexport class Router {\n  route(path: string) {\n    return path;\n  }\n}\n");
        write("README.md", "# Project\n\n## Retry\n\nWrites are retried by retry_write.\n");
        write(".gitignore", "build/\n");
        write("build/out.txt", "retry_write in a build output that is ignored\n");
        Command::new("git").args(["init", "--quiet"]).arg(&root).status().expect("git init");
        let cache = dir.path().join("cache");
        let config = dir.path().join("config");
        let root = atrius_index::host::canonical(&root);
        Scratch { _dir: dir, root, cache, config }
    }

    /// A command for one of the two programs with the scratch environment.
    fn command(&self, program: &str) -> Command {
        let exe = match program {
            "atrius" => env!("CARGO_BIN_EXE_atrius"),
            _ => env!("CARGO_BIN_EXE_atrius-mcp"),
        };
        let mut command = Command::new(exe);
        command
            .env("ATRIUS_INDEX_CACHE", &self.cache)
            .env("ATRIUS_CONFIG", &self.config)
            .env("ATRIUS_EMBED", "off")
            .env("ATRIUS_IDLE_MINUTES", "5")
            .env_remove("CLAUDE_PROJECT_DIR")
            .env_remove("ATRIUS_ROOT")
            .current_dir(&self.root);
        command
    }

    /// Runs `atrius` with arguments and answers its exit code, standard output and standard error.
    fn atrius(&self, args: &[&str]) -> (i32, String, String) {
        let out = self.command("atrius").args(args).stdin(Stdio::null()).output().expect("atrius runs");
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
    }

    /// Runs `atrius ... --json` and answers the reply.
    fn json(&self, args: &[&str]) -> Value {
        let mut all: Vec<&str> = args.to_vec();
        all.push("--json");
        let (_, out, err) = self.atrius(&all);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?} printed no JSON ({e}): {out} {err}"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = self.command("atrius").args(["stop"]).output();
    }
}

#[test]
fn every_query_answers_through_the_command_line() {
    let s = Scratch::new();
    let (code, out, _) = s.atrius(&["find", "retry_write"]);
    assert_eq!(code, 0);
    assert!(out.contains("src/lib.rs") && out.contains("tools/report.py"), "{out}");
    assert!(!out.contains("build/out.txt"), "an ignored file was searched: {out}");

    let (code, out, _) = s.atrius(&["find", r"fn \w+_numbers", "--mode", "regex", "--glob", "*.rs"]);
    assert_eq!(code, 0);
    assert!(out.contains("add_numbers"), "{out}");

    let reply = s.json(&["def", "ListenAndServe"]);
    assert_eq!(reply["ok"], true);
    assert!(reply["result"]["text"].as_str().unwrap().contains("server/main.go"), "{reply}");

    let reply = s.json(&["def", "Router"]);
    assert!(reply["result"]["text"].as_str().unwrap().contains("web/app.ts"), "{reply}");

    let (code, out, _) = s.atrius(&["refs", "add_numbers"]);
    assert_eq!(code, 0);
    assert!(out.matches("add_numbers").count() >= 2, "{out}");

    let (code, out, _) = s.atrius(&["outline", "tools/report.py"]);
    assert_eq!(code, 0);
    assert!(out.contains("Report") && out.contains("render"), "{out}");

    let (code, out, _) = s.atrius(&["fragment", "src/lib.rs:3"]);
    assert_eq!(code, 0);
    assert!(out.contains("pub fn add_numbers"), "{out}");

    let reply = s.json(&["files", "*.go"]);
    assert!(reply["result"]["text"].as_str().unwrap().contains("server/main.go"), "{reply}");

    let reply = s.json(&["status"]);
    assert_eq!(reply["ok"], true);
    assert!(reply["result"]["host"]["pid"].as_u64().is_some(), "{reply}");
}

#[test]
fn mistakes_are_refused_with_the_right_exit_code() {
    let s = Scratch::new();
    let (code, _, err) = s.atrius(&["find"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("needs <query>"));
    let (code, _, err) = s.atrius(&["frobnicate"]);
    assert_eq!(code, 2, "{err}");
    let (code, _, err) = s.atrius(&["def", "x", "--limit", "many"]);
    assert_eq!(code, 2, "{err}");
    let (code, _, err) = s.atrius(&["find", "x", "--root", "/no/such/folder/anywhere"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("not-found"), "{err}");
    let (code, _, err) = s.atrius(&["find", "(unclosed", "--mode", "regex"]);
    assert_ne!(code, 0, "{err}");
    let (code, _, err) = s.atrius(&["outline", "no/such/file.rs"]);
    assert_ne!(code, 0, "{err}");
}

#[test]
fn a_spawned_host_is_reused_listed_and_stopped() {
    let s = Scratch::new();
    let first = s.json(&["status"]);
    let pid = first["result"]["host"]["pid"].as_u64().expect("a host pid");
    let again = s.json(&["find", "createServer"]);
    assert_eq!(again["ok"], true);
    let second = s.json(&["status"]);
    assert_eq!(second["result"]["host"]["pid"].as_u64(), Some(pid), "a second host was started");
    let hosts = s.json(&["hosts"]);
    let listed = hosts["result"]["hosts"].as_array().unwrap();
    assert!(listed.iter().any(|h| h["pid"].as_u64() == Some(pid) && h["answering"] == true), "{hosts}");
    let stopped = s.json(&["stop"]);
    assert_eq!(stopped["result"]["stopped"], true);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && atrius_index_host_file(&s).is_some() {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(atrius_index_host_file(&s).is_none(), "the host file outlived `stop`");
}

/// The host file of the scratch checkout, if one is written.
fn atrius_index_host_file(s: &Scratch) -> Option<String> {
    std::env::set_var("ATRIUS_INDEX_CACHE", &s.cache);
    let folder = atrius_index::store::index_folder(&s.root);
    std::fs::read_to_string(folder.join("host.conf")).ok()
}

/// A running `atrius-mcp` and the reader of its output.
struct Mcp {
    child: Child,
    reader: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl Mcp {
    fn start(s: &Scratch) -> Mcp {
        let mut child = s.command("atrius-mcp").arg("--root").arg(&s.root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().expect("atrius-mcp starts");
        let reader = BufReader::new(child.stdout.take().unwrap());
        Mcp { child, reader, next_id: 1 }
    }

    fn send(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        self.reader.read_line(&mut line).expect("a reply");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON ({e}): {line}"))
    }

    fn notify(&mut self, method: &str) {
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", json!({ "jsonrpc": "2.0", "method": method })).unwrap();
        stdin.flush().unwrap();
    }

    fn call(&mut self, command: &str, arguments: Value) -> Value {
        self.send("tools/call", json!({ "name": "atrius_search", "arguments": { "command": command, "arguments": arguments } }))
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn the_mcp_server_answers_every_query_with_the_command_lines_text() {
    let s = Scratch::new();
    let mut mcp = Mcp::start(&s);
    let init = mcp.send("initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } }));
    assert_eq!(init["result"]["serverInfo"]["name"], "atrius-index");
    mcp.notify("notifications/initialized");
    let tools = mcp.send("tools/list", json!({}));
    let tool = &tools["result"]["tools"][0];
    assert_eq!(tool["name"], "atrius_search");
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 1);

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        // `def` first: it waits for the index, so `find` after it is answered from the index rather
        // than by the direct scan that answers while the index is still being built.
        ("def", json!({ "name": "RetryPolicy" }), vec!["def", "RetryPolicy"]),
        ("find", json!({ "query": "retry_write" }), vec!["find", "retry_write"]),
        ("refs", json!({ "name": "add_numbers" }), vec!["refs", "add_numbers"]),
        ("fragment", json!({ "target": "web/app.ts:2" }), vec!["fragment", "web/app.ts:2"]),
        ("outline", json!({ "path": "src/lib.rs" }), vec!["outline", "src/lib.rs"]),
        ("files", json!({ "query": "report" }), vec!["files", "report"]),
    ];
    for (command, arguments, cli) in cases {
        let answer = mcp.call(command, arguments);
        assert_eq!(answer["result"]["isError"], Value::Null, "{command}: {answer}");
        let text = answer["result"]["content"][0]["text"].as_str().unwrap_or_default().to_owned();
        let (code, out, err) = s.atrius(&cli);
        assert_eq!(code, 0, "{cli:?}: {err}");
        assert_eq!(text, out, "{command}: MCP and the command line gave different text");
    }

    // A command written as a line is read the way the command line reads it.
    let answer = mcp.send("tools/call", json!({ "name": "atrius_search", "arguments": { "command": "find retry_write --glob *.py" } }));
    let text = answer["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("tools/report.py") && !text.contains("src/lib.rs"), "{text}");

    // A refusal is a tool result with isError, and an unknown tool is a protocol error.
    let refused = mcp.call("outline", json!({ "path": "nope.rs" }));
    assert_eq!(refused["result"]["isError"], true, "{refused}");
    let unknown = mcp.call("frobnicate", json!({}));
    assert_eq!(unknown["result"]["isError"], true, "{unknown}");
    let wrong_tool = mcp.send("tools/call", json!({ "name": "grep", "arguments": {} }));
    assert_eq!(wrong_tool["error"]["code"], -32602);
    // Asking for the fields adds structuredContent.
    let structured = mcp.call("def", json!({ "name": "Router", "structured": true }));
    assert!(structured["result"]["structuredContent"]["definitions"].is_array(), "{structured}");
}

#[test]
fn a_host_answers_unluminous_style_commands() {
    let s = Scratch::new();
    let _ = s.json(&["status"]);
    std::env::set_var("ATRIUS_INDEX_CACHE", &s.cache);
    let file = atrius_index::host::read_host_file(&s.root).expect("a host file");
    let mut arguments = serde_json::Map::new();
    arguments.insert("query".into(), Value::String("createServer".into()));
    let request = atrius_index::protocol::Request::new(&file.token, "search.find", arguments.clone());
    let reply = atrius_index::protocol::ask(file.port, &request, Duration::from_secs(20)).expect("the host answers");
    assert!(reply.ok, "{reply:?}");
    assert!(reply.text().contains("web/app.ts"));
    let wrong = atrius_index::protocol::Request::new("not-the-token", "find", arguments);
    let refused = atrius_index::protocol::ask(file.port, &wrong, Duration::from_secs(20)).expect("the host answers");
    assert_eq!(refused.code, "refused");
}

#[test]
fn the_watch_list_languages_and_doctor_work() {
    let s = Scratch::new();
    let root = s.root.to_string_lossy().into_owned();
    let added = s.json(&["watch", "add", &root]);
    assert_eq!(added["result"]["changed"], true);
    let again = s.json(&["watch", "add", &root]);
    assert_eq!(again["result"]["changed"], false);
    let list = s.json(&["watch", "list"]);
    assert_eq!(list["result"]["folders"].as_array().unwrap().len(), 1);
    let removed = s.json(&["watch", "remove", &root]);
    assert_eq!(removed["result"]["changed"], true);
    let languages = s.json(&["languages"]);
    let names: Vec<&str> = languages["result"]["languages"].as_array().unwrap().iter().map(|l| l["id"].as_str().unwrap()).collect();
    for id in ["c", "cpp", "csharp", "go", "java", "kotlin", "php", "python", "ruby", "rust", "swift", "typescript"] {
        assert!(names.contains(&id), "{id} missing from {names:?}");
    }
    let doctor = s.json(&["doctor"]);
    let checks = doctor["result"]["checks"].as_array().unwrap();
    let events = checks.iter().find(|c| c["name"] == "file events").unwrap();
    assert_eq!(events["ok"], true, "{doctor}");
}

#[test]
fn the_service_installs_runs_the_daemon_and_uninstalls() {
    let s = Scratch::new();
    let root = s.root.to_string_lossy().into_owned();
    let _ = s.json(&["watch", "add", &root]);
    let name = format!("AtriusIndexTest{}", std::process::id());
    let run = |args: &[&str]| {
        let out = s.command("atrius").args(args).arg("--json").env("ATRIUS_SERVICE_NAME", &name).env("ATRIUS_SERVICE_DRY", if cfg!(windows) { "0" } else { "1" }).env("HOME", s.config.join("home")).env("XDG_CONFIG_HOME", s.config.join("xdg")).output().expect("atrius runs");
        serde_json::from_slice::<Value>(&out.stdout).unwrap_or_else(|e| panic!("{args:?}: {e}: {}", String::from_utf8_lossy(&out.stderr)))
    };
    let installed = run(&["service", "install"]);
    assert_eq!(installed["ok"], true, "{installed}");
    let status = run(&["service", "status"]);
    if cfg!(windows) {
        assert_eq!(status["result"]["installed"], true, "{status}");
        // The daemon was started and hosts the watched folder.
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut hosted = false;
        while Instant::now() < deadline && !hosted {
            let hosts = s.json(&["hosts"]);
            hosted = hosts["result"]["hosts"].as_array().unwrap().iter().any(|h| h["program"].as_str().unwrap_or_default().starts_with("atrius daemon"));
            std::thread::sleep(Duration::from_millis(250));
        }
        assert!(hosted, "the daemon did not host the watched folder");
    } else {
        assert!(Path::new(status["result"]["loginItem"].as_str().unwrap()).exists(), "{status}");
    }
    let removed = run(&["service", "uninstall"]);
    assert_eq!(removed["ok"], true, "{removed}");
    let status = run(&["service", "status"]);
    assert_eq!(status["result"]["installed"], false, "{status}");
    assert_eq!(status["result"]["running"], false, "{status}");
}

#[test]
fn the_command_reference_page_is_current() {
    let page = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("docs").join("commands.md");
    let committed = std::fs::read_to_string(&page).unwrap_or_default().replace("\r\n", "\n");
    assert!(committed == atrius_cli::docs::commands_page(), "docs/commands.md is out of date: run `atrius help --markdown > docs/commands.md`");
}
