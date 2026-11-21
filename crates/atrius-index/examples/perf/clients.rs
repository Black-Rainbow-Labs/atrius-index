//! The clients the benchmark times: the MCP server over its standard input and output, the separate
//! host over its socket, a one shot `atrius find`, and ripgrep.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use atrius_index::protocol::{self, Request};
use serde_json::{json, Map, Value};

use super::cases::RgQuery;

/// The environment every Atrius process of the benchmark runs with.
///
/// @param command - the command to set it on
/// @param cache - the index cache folder
fn atrius_env(command: &mut Command, cache: &Path) {
    command.env("ATRIUS_INDEX_CACHE", cache).env("ATRIUS_EMBED", "off").env("ATRIUS_IDLE_MINUTES", "0");
}

/// A running `atrius-mcp`, hosting its checkout's index in its own process.
pub struct Mcp {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
}

impl Mcp {
    /// Starts the server for a root and initializes the session.
    ///
    /// @param bin - the folder holding `atrius-mcp`
    /// @param root - the checkout
    /// @param cache - the index cache folder
    pub fn start(bin: &Path, root: &Path, cache: &Path) -> Mcp {
        let mut command = Command::new(bin.join("atrius-mcp"));
        command.arg("--root").arg(root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        atrius_env(&mut command, cache);
        let mut child = command.spawn().expect("atrius-mcp starts");
        let input = child.stdin.take().expect("stdin");
        let output = BufReader::new(child.stdout.take().expect("stdout"));
        let mut mcp = Mcp { child, input, output, next_id: 1 };
        mcp.request("initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "perf", "version": "1" } }));
        writeln!(mcp.input, "{}", json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).expect("a notification");
        mcp
    }

    /// The server's process id.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Sends one JSON-RPC request and reads its reply's result.
    ///
    /// @param method - the method
    /// @param params - its parameters
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        writeln!(self.input, "{}", json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).expect("a request");
        self.input.flush().expect("flushed");
        loop {
            let mut line = String::new();
            if self.output.read_line(&mut line).unwrap_or(0) == 0 {
                panic!("atrius-mcp closed its output");
            }
            let reply: Value = serde_json::from_str(&line).unwrap_or_default();
            if reply["id"] == id {
                return reply["result"].clone();
            }
        }
    }

    /// One `atrius_search` call, answered as the tool result an agent reads.
    ///
    /// @param verb - the command
    /// @param args - its arguments
    pub fn call(&mut self, verb: &str, args: &Value) -> Value {
        self.request("tools/call", json!({ "name": "atrius_search", "arguments": { "command": verb, "arguments": args } }))
    }

    /// The index's status fields.
    pub fn status(&mut self) -> Value {
        self.call("status", &json!({}))["structuredContent"].clone()
    }

    /// Asks for the status until it satisfies a test, or the time runs out.
    ///
    /// @param test - what the status must say
    /// @param seconds - how long to wait
    pub fn wait_until(&mut self, test: impl Fn(&Value) -> bool, seconds: f64) {
        let deadline = Instant::now() + Duration::from_secs_f64(seconds);
        while Instant::now() < deadline {
            if test(&self.status()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the index did not become ready in time");
    }

    /// The time from `since` to the first answer of a find, and to the first answered from the index
    /// rather than by scanning the files.
    ///
    /// @param case - a find case, if the repository has one
    /// @param since - when the measuring process started
    pub fn first_answer(&mut self, case: Option<&super::cases::Case>, since: Instant) -> (f64, f64) {
        let ms = || since.elapsed().as_secs_f64() * 1000.0;
        let mut args = case.map(|c| c.args.clone()).unwrap_or_else(|| json!({ "query": "fn", "mode": "regex" }));
        args["structured"] = json!(true);
        let first = self.call("find", &args);
        let any = ms();
        if first["structuredContent"]["index"] == "trigram" {
            return (any, any);
        }
        self.wait_until(|s| s["ready"] == true, 3600.0);
        while self.call("find", &args)["structuredContent"]["index"] != "trigram" {}
        (any, ms())
    }

    /// Ends the session and waits for the server to exit.
    pub fn stop(mut self) {
        drop(self.input);
        let _ = self.child.wait();
    }
}

/// A running `atrius serve`, reached over its loopback socket.
pub struct Serve {
    child: Child,
    port: u16,
    token: String,
    /// How long it took from starting the host to its first answer from the index.
    pub ready_ms: f64,
}

impl Serve {
    /// Starts a separate host for a root and waits until it answers from its index.
    ///
    /// @param bin - the folder holding `atrius`
    /// @param root - the checkout
    /// @param cache - its index cache folder
    pub fn start(bin: &Path, root: &Path, cache: &Path) -> Serve {
        std::env::set_var("ATRIUS_INDEX_CACHE", cache);
        let started = Instant::now();
        let mut command = Command::new(bin.join("atrius"));
        command.arg("serve").arg("--root").arg(root).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        atrius_env(&mut command, cache);
        let child = command.spawn().expect("atrius serve starts");
        let file = loop {
            match atrius_index::host::read_host_file(root).filter(|f| f.pid == child.id()) {
                Some(file) => break file,
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        };
        let mut serve = Serve { child, port: file.port, token: file.token, ready_ms: 0.0 };
        while serve.call("status", &json!({}))["ready"] != true {
            std::thread::sleep(Duration::from_millis(20));
        }
        serve.ready_ms = started.elapsed().as_secs_f64() * 1000.0;
        serve
    }

    /// One request, answered as the host's reply.
    ///
    /// @param verb - the command
    /// @param args - its arguments
    pub fn call(&mut self, verb: &str, args: &Value) -> Value {
        let arguments: Map<String, Value> = args.as_object().cloned().unwrap_or_default();
        let request = Request::new(&self.token, verb, arguments);
        protocol::ask(self.port, &request, Duration::from_secs(600)).map(|r| r.result).unwrap_or_default()
    }

    /// Asks the host to stop and waits for it to exit.
    pub fn stop(mut self, _bin: &Path, _root: &Path, _cache: &Path) {
        self.call("stop", &json!({}));
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
    }
}

/// One `atrius find` process asking the running host, as a person or a script types it.
///
/// @param bin - the folder holding `atrius`
/// @param root - the checkout
/// @param cache - the host's index cache folder
/// @param args - the find arguments
pub fn oneshot(bin: &Path, root: &Path, cache: &Path, args: &Value) {
    let mut command = Command::new(bin.join("atrius"));
    command.arg("find").arg("--root").arg(root).current_dir(root).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    atrius_env(&mut command, cache);
    for (key, value) in args.as_object().into_iter().flatten().filter(|(k, _)| k.as_str() != "query") {
        match value {
            Value::Bool(true) => {
                command.arg(format!("--{key}"));
            }
            Value::Bool(false) => {}
            Value::String(s) => {
                command.arg(format!("--{key}")).arg(s);
            }
            other => {
                command.arg(format!("--{key}")).arg(other.to_string());
            }
        }
    }
    command.arg("--").arg(args["query"].as_str().unwrap_or_default());
    let _ = command.status();
}

/// A ripgrep process with the Grep tool's flags, read as a set of `path:line`.
///
/// @param rg - the ripgrep executable
/// @param root - the checkout
/// @param q - the query
pub fn ripgrep(rg: &str, root: &Path, q: &RgQuery) -> BTreeSet<String> {
    let mut cmd = Command::new(rg);
    cmd.current_dir(root).args(["--no-config", "--hidden"]);
    for folder in [".git", ".svn", ".hg", ".bzr", ".jj", ".sl"] {
        cmd.args(["--glob", &format!("!{folder}")]);
    }
    cmd.args(["--max-columns", "500"]);
    if q.ignore_case {
        cmd.arg("-i");
    }
    cmd.args(["--json", "-n", "-e", &q.pattern]);
    for t in &q.types {
        cmd.args(["--type", t]);
    }
    for g in &q.globs {
        cmd.args(["--glob", g]);
    }
    cmd.arg(if q.path.is_empty() { "." } else { &q.path });
    let out = cmd.stderr(Stdio::null()).output().expect("ripgrep runs");
    let mut set = BTreeSet::new();
    for line in out.stdout.split(|b| *b == b'\n').filter(|l| l.starts_with(b"{\"type\":\"match\"")) {
        let event: Value = serde_json::from_slice(line).unwrap_or_default();
        let path = event["data"]["path"]["text"].as_str().unwrap_or_default().replace('\\', "/");
        set.insert(format!("{}:{}", path.trim_start_matches("./"), event["data"]["line_number"]));
    }
    set
}

/// A process's resident memory in kilobytes, from `ps`.
///
/// @param pid - the process
pub fn rss_kb(pid: u32) -> u64 {
    let out = Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output();
    out.ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok()).unwrap_or(0)
}

/// The bytes of every file under a folder.
///
/// @param folder - the folder
pub fn folder_bytes(folder: &Path) -> u64 {
    let mut total = 0;
    let mut stack: Vec<PathBuf> = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            match entry.metadata() {
                Ok(m) if m.is_dir() => stack.push(entry.path()),
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
    }
    total
}
