//! Runs one `atrius` command line and answers its exit code.

use std::time::Duration;

use atrius_index::host::{self, Hosting};
use atrius_index::protocol::{code, Reply};
use serde_json::{json, Value};

use crate::catalogue::{self, Runs};
use crate::parse::{self, Parsed};

/// Exit code: carried out.
pub const OK: i32 = 0;
/// Exit code: refused.
pub const REFUSED: i32 = 1;
/// Exit code: the command line was wrong.
pub const USAGE: i32 = 2;

/// Runs the command line `atrius <words>`.
///
/// @param words - the words after the program's name
pub fn main(words: &[String]) -> i32 {
    match words.first().map(String::as_str) {
        None => {
            print!("{}", help_text(None));
            return OK;
        }
        Some("--version") | Some("-V") | Some("version") => {
            println!("atrius {}", atrius_index::VERSION);
            return OK;
        }
        Some("--help") | Some("-h") => {
            print!("{}", help_text(None));
            return OK;
        }
        Some("help") if words.get(1).map(String::as_str) == Some("--markdown") => {
            print!("{}", crate::docs::commands_page());
            return OK;
        }
        _ => {}
    }
    let parsed = match parse::parse(words) {
        Ok(p) => p,
        Err(problem) => {
            eprintln!("atrius: {problem}");
            return USAGE;
        }
    };
    if parsed.command.runs == Runs::Query {
        let root = host::project_root(parsed.root.as_deref());
        let reply = host::ask(&root, parsed.command.name, &parsed.arguments, Duration::from_secs(60), Hosting::Spawn);
        return print_reply(&reply, parsed.json);
    }
    local(&parsed)
}

/// Prints a reply as text or JSON and answers the exit code.
///
/// @param reply - the reply
/// @param as_json - whether `--json` was given
pub fn print_reply(reply: &Reply, as_json: bool) -> i32 {
    if as_json {
        println!("{}", serde_json::to_string_pretty(&reply.to_json()).unwrap_or_default());
    } else if reply.ok {
        print!("{}", reply.text());
    } else {
        eprintln!("atrius: {}", reply.text());
    }
    if reply.ok {
        OK
    } else if reply.code == code::USAGE || reply.code == code::UNKNOWN_COMMAND {
        USAGE
    } else {
        REFUSED
    }
}

/// Runs a command that does not go to an index host.
///
/// @param parsed - the command line
fn local(parsed: &Parsed) -> i32 {
    let name = parsed.command.name;
    let text_arg = |key: &str| parsed.arguments.get(key).and_then(Value::as_str).map(str::to_owned);
    let done = |result: Value| print_reply(&Reply::done(name, result), parsed.json);
    let refused = |message: String| print_reply(&Reply::failed(name, code::FAILED, message), parsed.json);
    match name {
        "help" => {
            let asked = text_arg("command");
            if let Some(asked) = &asked {
                if catalogue::command(asked).is_none() {
                    eprintln!("atrius: there is no command called `{asked}`");
                    return USAGE;
                }
            }
            print!("{}", help_text(asked.as_deref()));
            OK
        }
        "serve" => {
            let root = host::project_root(parsed.root.as_deref());
            let idle = match parsed.arguments.get("idle").and_then(Value::as_u64) {
                Some(0) => None,
                Some(minutes) => Some(Duration::from_secs(minutes * 60)),
                None => host::idle_limit(),
            };
            match host::serve(&root, idle) {
                Ok(()) => OK,
                Err(problem) => {
                    eprintln!("atrius serve: {problem}");
                    REFUSED
                }
            }
        }
        "stop" => {
            let root = host::project_root(parsed.root.as_deref());
            if host::stop(&root) {
                done(json!({ "stopped": true, "text": format!("asked the index host of {} to stop\n", root.display()) }))
            } else {
                done(json!({ "stopped": false, "text": format!("no index host is running for {}\n", root.display()) }))
            }
        }
        "hosts" => done(hosts()),
        "mcp" => crate::mcp::serve(parsed.root.as_deref()),
        "watch add" | "watch remove" => {
            let folder = host::canonical(std::path::Path::new(&text_arg("folder").unwrap_or_default()));
            let result = if name == "watch add" { crate::watchlist::add(&folder) } else { crate::watchlist::remove(&folder) };
            match result {
                Ok(changed) => {
                    let verb = if name == "watch add" { "added" } else { "removed" };
                    let text = if changed { format!("{verb} {}\n", folder.display()) } else { format!("{} was already {}\n", folder.display(), if name == "watch add" { "on the list" } else { "off the list" }) };
                    done(json!({ "changed": changed, "folder": folder.to_string_lossy(), "text": text }))
                }
                Err(problem) => refused(problem),
            }
        }
        "watch list" => {
            let folders = crate::watchlist::read();
            let mut text: String = folders.iter().map(|f| format!("{}\n", f.display())).collect();
            if folders.is_empty() {
                text = "no folders are watched. `atrius watch add <folder>` adds one\n".into();
            }
            done(json!({ "folders": folders.iter().map(|f| f.to_string_lossy().into_owned()).collect::<Vec<_>>(), "file": crate::watchlist::watch_file().to_string_lossy(), "text": text }))
        }
        "service install" => match std::env::current_exe() {
            Ok(exe) => match crate::service::install(&host::canonical(&exe)) {
                Ok(text) => done(json!({ "text": text })),
                Err(problem) => refused(problem),
            },
            Err(e) => refused(format!("cannot find this program's path: {e}")),
        },
        "service uninstall" => match crate::service::uninstall() {
            Ok(text) => done(json!({ "text": text })),
            Err(problem) => refused(problem),
        },
        "service status" => done(crate::service::status()),
        "daemon" => crate::daemon::run(),
        "doctor" => {
            let result = crate::doctor::run();
            let ok = result["ok"].as_bool().unwrap_or(false);
            let code = done(result);
            if ok { code } else { REFUSED }
        }
        "languages" => done(languages()),
        other => {
            eprintln!("atrius: `{other}` is not handled");
            USAGE
        }
    }
}

/// The running hosts, as `hosts` prints them.
fn hosts() -> Value {
    let found = host::running_hosts();
    let mut text = String::new();
    for h in &found {
        text.push_str(&format!(
            "{}  process {}  port {}  {}{}\n",
            h.file.root,
            h.file.pid,
            h.file.port,
            if h.answering { "answering" } else { "not answering" },
            if h.file.program.is_empty() { String::new() } else { format!("  ({})", h.file.program) }
        ));
    }
    if found.is_empty() {
        text = "no index host is running\n".into();
    }
    json!({
        "hosts": found.iter().map(|h| json!({ "root": h.file.root, "pid": h.file.pid, "port": h.file.port, "answering": h.answering, "program": h.file.program })).collect::<Vec<_>>(),
        "text": text,
    })
}

/// The languages and their extensions, as `languages` prints them.
fn languages() -> Value {
    let table = atrius_index::grammars::by_extension();
    let mut by_id: std::collections::BTreeMap<String, (String, Vec<String>)> = std::collections::BTreeMap::new();
    for (extension, language) in table.iter() {
        let entry = by_id.entry(language.id.clone()).or_insert_with(|| (language.grammar.language.clone(), Vec::new()));
        entry.1.push(format!(".{extension}"));
    }
    let mut text = String::new();
    let mut list = Vec::new();
    for (id, (name, mut extensions)) in by_id {
        extensions.sort();
        text.push_str(&format!("{name:<12} {}\n", extensions.join(" ")));
        list.push(json!({ "id": id, "name": name, "extensions": extensions }));
    }
    json!({ "languages": list, "text": text })
}

/// `atrius help`, or one command's usage.
///
/// @param command - the command, if one was named
pub fn help_text(command: Option<&str>) -> String {
    if let Some(command) = command.and_then(catalogue::command) {
        let mut out = format!("{}\n\n{}\n", catalogue::usage(command, "atrius"), command.summary);
        if !command.arguments.is_empty() {
            out.push('\n');
            for argument in command.arguments {
                out.push_str(&format!("  <{}>  {}\n", argument.name, argument.help));
            }
        }
        out.push('\n');
        for flag in catalogue::all_flags(command) {
            let short = flag.short.map(|c| format!("-{c}, ")).unwrap_or_default();
            out.push_str(&format!("  {short}--{}  {}\n", flag.name, flag.help));
        }
        out.push_str("\nExamples:\n");
        for example in command.examples {
            out.push_str(&format!("  {example}\n"));
        }
        return out;
    }
    let mut out = format!("atrius {}: a code index for agents and people\n\nUsage: atrius <command> [arguments] [flags]\n\nQueries, answered by the index host of the checkout:\n", atrius_index::VERSION);
    for c in catalogue::COMMANDS.iter().filter(|c| c.runs == Runs::Query) {
        out.push_str(&format!("  {:<18} {}\n", c.name, first_sentence(c.summary)));
    }
    out.push_str("\nEverything else:\n");
    for c in catalogue::COMMANDS.iter().filter(|c| c.runs == Runs::Local) {
        out.push_str(&format!("  {:<18} {}\n", c.name, first_sentence(c.summary)));
    }
    out.push_str("\n`atrius help <command>` shows one command's flags. Every query takes --root <folder> and --json.\n");
    out
}

/// The first sentence of a summary.
///
/// @param text - the summary
fn first_sentence(text: &str) -> &str {
    match text.find(". ") {
        Some(at) => &text[..=at],
        None => text,
    }
}
