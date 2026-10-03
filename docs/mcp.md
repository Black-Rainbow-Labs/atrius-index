# MCP

`atrius-mcp` is an MCP server. It lets an AI agent search a checkout with Atrius in place of Grep, Glob
and reading whole files. This page shows how to register it with each common client, what the one tool
it offers looks like, and what an agent sees.

## Register it

| Client | How |
|---|---|
| Claude Code | `claude mcp add atrius -- atrius-mcp`, or `claude mcp add --scope user atrius -- atrius-mcp` for every project |
| Codex | add `[mcp_servers.atrius]` with `command = "atrius-mcp"` to `~/.codex/config.toml` |
| Cursor | add `"atrius": { "command": "atrius-mcp" }` under `mcpServers` in `.cursor/mcp.json` |
| VS Code | add `"atrius": { "type": "stdio", "command": "atrius-mcp" }` under `servers` in `.vscode/mcp.json` |
| Anything else | run `atrius-mcp`, which speaks MCP on standard input and output |

The server searches the checkout it was started in: the folder given with `--root`, else
`ATRIUS_ROOT`, else `CLAUDE_PROJECT_DIR`, else the git checkout at or above its working folder. A call
can name another checkout with `root`.

## The tool

There is one tool, `atrius_search`. Its `command` names a query and its `arguments` hold that query's
named values, exactly as the [command reference](commands.md) lists them.

```json
{
  "name": "atrius_search",
  "arguments": {
    "command": "find",
    "arguments": { "query": "retry_write", "glob": "*.rs", "budget": 800 }
  }
}
```

| `command` | What it answers |
|---|---|
| `find` | ripgrep's lines for a literal or regex, a name's definitions and uses for one identifier, passages for a sentence |
| `def` | where a name is defined |
| `refs` | every use of a name |
| `fragment` | the function around `path:line` |
| `outline` | a file's definitions with their line ranges |
| `files` | files by name or glob |
| `status` | whether the index is ready, and its size |

A command may also be written as one line, the way it is typed at a terminal. These two calls are the same:

```json
{ "command": "find", "arguments": { "query": "retry_write", "glob": "*.py" } }
{ "command": "find retry_write --glob *.py" }
```

**Why one tool.** A client sends every tool's definition on every turn of a conversation. The study
behind Atrius measured about 5,200 tokens of instructions and tool definitions at the first turn of a
session, and found that most of a capable model's session tokens are that repeated cost. One tool with a short schema keeps it small. [Lab findings](lab/findings.md) has the
numbers.

## What an agent sees

The text of the answer, the same text `atrius` prints:

```text
defined at:
  src/lib.rs:10: fn retry_write(policy: &RetryPolicy) -> bool {
  tools/report.py:6: def retry_write(times):
used in 3 places:
src/lib.rs
  10: fn retry_write(policy: &RetryPolicy) -> bool {
tools/report.py
  6: def retry_write(times):
README.md
  5: Writes are retried by retry_write.
```

A refused call is a tool result with `isError: true` and a message that says what to change. With
`"structured": true` among the arguments, the reply also carries `structuredContent` with the fields
behind the text, for a program that wants paths and line numbers.

## Where the index lives

The server hosts the checkout's index in its own process, so a query is answered without a socket. If
another program already hosts that checkout, such as an Unluminous window, the background service, or
an earlier `atrius` command, the server asks that host instead. One checkout has one index whatever is
asking.

## Protocol details

| Item | Value |
|---|---|
| Transport | standard input and output, one JSON message a line |
| Versions | `2025-06-18`, `2025-03-26`, `2024-11-05`; the server answers with the version the client asked for when it knows it |
| Methods | `initialize`, `ping`, `tools/list`, `tools/call`, `resources/list` and `prompts/list` (both empty) |
| Errors | an unknown method is `-32601`; an unknown tool is `-32602`; a refused query is a result with `isError` |
