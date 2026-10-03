# Using the library

The `atrius-index` crate is the whole index: the engines, the host, the wire protocol and the query
verbs. `atrius` and `atrius-mcp` are thin programs over it, and Unluminous links it the same way. This
page shows how to use it from Rust.

```toml
[dependencies]
atrius-index = "0.1"
```

The crate depends on `inillucent-driver` with its `embed` feature. On Windows, building it needs the
MSVC environment, because a dependency compiles C code: build from a Developer PowerShell, or with
`tools/cargo.ps1` in this repository.

## Ask a question

```rust
use std::time::Duration;
use atrius_index::host::{self, Hosting};
use serde_json::{Map, Value};

let root = host::project_root(None);
let mut arguments = Map::new();
arguments.insert("query".into(), Value::String("retry_write".into()));
let reply = host::ask(&root, "find", &arguments, Duration::from_secs(30), Hosting::InProcess);
if reply.ok {
    print!("{}", reply.text());
}
```

`host::ask` answers through whichever host the checkout has. With `Hosting::InProcess` this process
becomes the host when none is running; with `Hosting::Spawn` it starts a detached `atrius serve`
instead; with `Hosting::Never` it asks a running host or scans the files directly.

| Choose | When the caller |
|---|---|
| `InProcess` | lives as long as someone is working: an editor, an MCP server, a daemon |
| `Spawn` | runs once and exits, such as a command line program |
| `Never` | must not start anything |

## Use the index directly

```rust
use atrius_index::index::Index;
use atrius_index::verbs;

let index = Index::open(&root);            // starts the watcher, then loads or builds
index.wait_for_symbols(std::time::Duration::from_secs(60));
let answer = verbs::answer(&index, "def", &arguments)?;
```

`Index::open` does not take the host lock. Use it for a tool that is the only reader, such as a test or
a benchmark; otherwise go through `host`, so one checkout has one watcher.

## Share hosts with your own program

When a one off command in your program should start a host that your program runs, name it:

```rust
atrius_index::host::set_server_command(my_cli_path, vec!["search".into(), "serve".into()]);
atrius_index::host::set_program_name(&format!("unluminous-cli {}", env!("CARGO_PKG_VERSION")));
```

Your server command must call `atrius_index::host::serve(root, idle)`. A host started by your program
answers `atrius` and `atrius-mcp`, and the other way round, because the lock, the host file and the
protocol are the same. A command may carry the prefix `search.`, so `search.find` and `find` reach the
same verb.

## Add a language

```rust
let manifest = std::fs::read_to_string("elixir.conf")?;
atrius_index::grammars::register("elixir", &manifest);
```

Register before opening an index; a file already chunked is not read again. [Languages](languages.md)
describes the format.

## The modules

| Module | What it holds |
|---|---|
| `host` | The lock, the host file, the loopback socket, starting a detached host, `ask`, `serve`, `stop`, `running_hosts` |
| `protocol` | The JSON request and reply, and `ask` for one request to one port |
| `verbs` | `answer`: the seven query verbs, each taking named arguments and returning `text` plus fields |
| `index` | `Index`: the engines, the watcher and the store thread for one checkout |
| `exact` | The trigram engine with ripgrep's results |
| `files` | The file set, which is the Grep tool's: ripgrep's ignore rules with hidden files included |
| `freshness` | The watcher and the gate |
| `symbols`, `outline` | Definitions, outlines and chunks |
| `passages`, `words` | The passage table and the words a chunk is searched by |
| `grammars`, `lang` | The language manifests and the tokeniser, definition and folding code |
| `store` | The Inillucent file and the index folder |
| `shape` | Cutting an answer to a token budget |
