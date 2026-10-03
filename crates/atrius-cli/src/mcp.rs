//! The MCP server: JSON-RPC 2.0 over standard input and output, one message a line.
//!
//! It offers one tool, `atrius_search`, whose `command` names a query verb and whose `arguments` are
//! the verb's named values. One tool rather than one per verb, because a client sends every tool's
//! definition on every turn, and task-2139 measured that cost as most of an agent session's tokens.
//! The server hosts the index of its checkout in its own process, so a query is answered with no
//! socket at all.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use atrius_index::host::{self, Hosting};
use atrius_index::protocol::Reply;
use serde_json::{json, Map, Value};

use crate::catalogue::{self, Takes};
use crate::parse;

/// The tool's name.
pub const TOOL: &str = "atrius_search";

/// The MCP versions this server answers, newest first.
const VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// What an agent is told the tool is for, before the list of commands.
const PURPOSE: &str = "Use this instead of Grep, Glob, grep in a shell, and reading whole files. `find` gives the lines ripgrep would, grouped by file within a token budget; one identifier gets its definitions first. `def` and `refs` find where a name is defined and used. `fragment path:line` shows just the function around a line, so read that rather than the file; `outline` lists a file's definitions with their line ranges, and a Read of more than that should take an offset and a limit from them. Small answers already include the code around the hit. `files` finds a file by name or glob. Kept current as files change.";

/// The tool's description: what it is for, then each command's usage line and summary.
pub fn description() -> String {
    let mut out = format!("{PURPOSE}\n\nCommands:\n\n");
    for command in catalogue::queries() {
        out.push_str(&format!("  {}\n      {}\n", catalogue::usage(command, "").trim_start(), command.summary));
    }
    out
}

/// The tool's input schema, from the catalogue.
pub fn input_schema() -> Value {
    let mut properties = Map::new();
    let mut names: Vec<&str> = Vec::new();
    for command in catalogue::queries() {
        names.push(command.name);
        for argument in command.arguments {
            properties.insert(argument.name.into(), json!({ "type": "string" }));
        }
        for flag in command.flags {
            let kind = match flag.takes {
                Takes::Switch => "boolean",
                Takes::Number => "integer",
                Takes::Text => "string",
            };
            properties.insert(flag.name.into(), json!({ "type": kind }));
        }
    }
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["command"],
        "properties": {
            "command": {
                "type": "string",
                "enum": names,
                "description": "Which command to run. The usage lines in the description say what each one takes.",
            },
            "arguments": {
                "type": "object",
                "additionalProperties": false,
                "description": "Named values from the selected command's usage line. A switch is true.",
                "properties": properties,
            },
            "root": {
                "type": "string",
                "description": "The checkout to search, when it is not the one the server was started in.",
            },
        },
    })
}

/// The tool, as `tools/list` returns it.
pub fn tool() -> Value {
    json!({
        "name": TOOL,
        "title": "Atrius: the code index. Find text, definitions, uses and files without ripgrep",
        "description": description(),
        "inputSchema": input_schema(),
    })
}

/// The checkout the server answers for: the `--root` it was given, `ATRIUS_ROOT`,
/// `CLAUDE_PROJECT_DIR`, or the git checkout at or above the working folder.
///
/// @param given - the `--root` the server was started with
pub fn server_root(given: Option<&str>) -> PathBuf {
    let from_env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    let chosen = given.map(str::to_owned).or_else(|| from_env("ATRIUS_ROOT")).or_else(|| from_env("CLAUDE_PROJECT_DIR"));
    host::project_root(chosen.as_deref())
}

/// Runs one tool call and returns the reply.
///
/// @param root - the server's checkout
/// @param params - the call's `arguments`: `command`, `arguments` and `root`
pub fn call(root: &Path, params: &Value) -> Reply {
    let raw = params.get("command").and_then(Value::as_str).unwrap_or_default().trim();
    let mut arguments = params.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default();
    // A command written as a line, such as `find retry --path backend`, is read the way the command
    // line reads it, and anything in `arguments` is added on top.
    let words = parse::split_words(raw);
    let name = words.first().cloned().unwrap_or_default();
    let name = name.strip_prefix("search.").unwrap_or(&name).to_owned();
    let Some(command) = catalogue::queries().find(|c| c.name == name) else {
        return Reply::failed(raw, atrius_index::protocol::code::UNKNOWN_COMMAND, format!(
            "There is no command `{name}`. The commands are {}.",
            catalogue::queries().map(|c| c.name).collect::<Vec<_>>().join(", ")
        ));
    };
    let mut root = root.to_path_buf();
    if words.len() > 1 {
        match parse::parse_command(command, &words[1..]) {
            Ok(parsed) => {
                for (key, value) in parsed.arguments {
                    arguments.entry(key).or_insert(value);
                }
                if let Some(r) = parsed.root {
                    root = host::project_root(Some(&r));
                }
            }
            Err(problem) => return Reply::failed(command.name, atrius_index::protocol::code::USAGE, problem),
        }
    }
    if let Some(given) = params.get("root").and_then(Value::as_str).filter(|r| !r.trim().is_empty()) {
        root = resolve_against(&root, given);
    }
    if let Some(Value::String(given)) = arguments.remove("root") {
        root = resolve_against(&root, &given);
    }
    arguments.remove("json");
    host::ask(&root, command.name, &arguments, Duration::from_secs(60), Hosting::InProcess)
}

/// A folder named in a call, read relative to the server's checkout when it is not absolute.
///
/// @param base - the server's checkout
/// @param given - the folder named
fn resolve_against(base: &Path, given: &str) -> PathBuf {
    let path = Path::new(given.trim());
    if path.is_absolute() {
        host::project_root(Some(given))
    } else {
        host::canonical(&base.join(path))
    }
}

/// The JSON-RPC answer to one message, or `None` for a notification.
///
/// @param root - the server's checkout
/// @param message - the message
pub fn handle(root: &Path, message: &Value) -> Option<Value> {
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str).unwrap_or_default();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(VERSIONS[0]);
            let version = VERSIONS.iter().find(|v| **v == asked).copied().unwrap_or(VERSIONS[0]);
            Ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "atrius-index", "title": "Atrius", "version": atrius_index::VERSION },
                "instructions": format!("Atrius searches the checkout at {}. Use the `{TOOL}` tool instead of Grep, Glob and reading whole files.", root.display()),
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": [tool()] })),
        "resources/list" => Ok(json!({ "resources": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            if name != TOOL {
                Err((-32602, format!("There is no tool called `{name}`. The tool is `{TOOL}`.")))
            } else {
                let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
                Ok(tool_result(&call(root, &arguments)))
            }
        }
        _ if id.is_none() => return None,
        other => Err((-32601, format!("There is no method `{other}`."))),
    };
    let id = id?;
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    })
}

/// A reply as an MCP tool result: the text an agent reads, the fields when they were asked for, and
/// `isError` when the command was refused.
///
/// @param reply - the reply
pub fn tool_result(reply: &Reply) -> Value {
    if !reply.ok {
        return json!({
            "content": [{ "type": "text", "text": format!("{}: {}", reply.code, reply.message) }],
            "isError": true,
        });
    }
    let mut out = json!({ "content": [{ "type": "text", "text": reply.text() }] });
    if reply.result.get("structured").and_then(Value::as_bool).unwrap_or(false) {
        out["structuredContent"] = reply.result.clone();
    }
    out
}

/// Serves MCP on standard input and output until input ends.
///
/// @param given_root - the `--root` the server was started with
pub fn serve(given_root: Option<&str>) -> i32 {
    let root = server_root(given_root);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle(&root, &message),
            Err(e) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("That was not JSON: {e}") } })),
        };
        if let Some(answer) = answer {
            let mut out = stdout.lock();
            let _ = writeln!(out, "{answer}");
            let _ = out.flush();
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_names_every_query_and_no_local_command() {
        let schema = input_schema();
        let names: Vec<&str> = schema["properties"]["command"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(names, ["find", "def", "refs", "fragment", "outline", "files", "status"]);
        assert!(description().contains("find <query...>"));
    }

    #[test]
    fn initialize_and_unknown_methods_answer() {
        let root = std::env::temp_dir();
        let init = handle(&root, &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}})).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2024-11-05");
        let unknown = handle(&root, &json!({"jsonrpc":"2.0","id":2,"method":"nope"})).unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
        assert!(handle(&root, &json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
    }
}
