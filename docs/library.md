# Using the library

The `atrius-index` crate is the whole index: the engines, the host, the wire protocol and the query
verbs. `atrius` and `atrius-mcp` are thin programs over it, and Unluminous links it the same way. This
page shows how to use it from Rust.

```toml
[dependencies]
atrius-index = "0.2"
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

## Read definitions as data

An editor or a test that wants the definitions and must not write a cache opens an index held in memory:

```rust
use std::time::Duration;
use atrius_index::index::Index;

let index = Index::open_in_memory(&root);   // origin "memory": nothing is read from or written to disk
Index::keep_fresh(&index, Duration::from_millis(250));
let names = index.try_symbols(|table| {
    table.members_of("Layout").map(|(path, d)| (path.to_owned(), d.name.clone())).collect::<Vec<_>>()
});
```

`try_symbols` never waits. It returns `None` while the table is being written or is not built yet, and
the caller tries again on its next keystroke. An index held in memory has no passage table, so a plain
English question is answered from its words.

| Call | What it does |
|---|---|
| `Index::open_in_memory(root)` | Opens an index with no index file and returns it in an `Arc` |
| `Index::try_symbols(work)` | Runs `work` over the `SymbolTable` without waiting, or returns `None` |
| `Index::symbols_ready()` | True once the symbol table is built or loaded |
| `Index::keep_fresh(&index, interval)` | Starts a thread that runs the gate when the watcher saw a change, at most once an interval. The thread ends when the index is dropped |

To read one file's definitions without an index, use `outline::read_structure`:

```rust
let (definitions, imports) = atrius_index::outline::read_structure("src/layout.rs", &text);
for d in &definitions {
    println!("{} {:?} in {:?}({})", d.name, d.symbol_kind, d.container, d.parameter_text());
}
```

`read_structure` returns the definitions and the imports and does not cut the file into chunks. The
`Outline` that `outline::read` returns has an `imports` field as well.

### A definition

`outline::Definition` has these fields beyond its name and lines.

| Field | Holds |
|---|---|
| `kind` | The family of the kind: `function`, `type`, `constant`, `variable`, `module`, `field` or `variant` |
| `symbol_kind` | A `structure::Kind`, one of 16, for example `Kind::Method` or `Kind::TypeAlias` |
| `container` | The type or module whose block holds it |
| `parameters` | A list of `structure::Parameter`, each with a `name` and an optional `type_text` |
| `returns` | The return type as written |
| `type_text` | A field's or variable's type |
| `visibility` | A `structure::Visibility`: `Public`, `Crate` or `Private` |
| `doc` | The first line of its documentation |
| `range` | The byte range of the name |
| `member` | True for a field, a variant or an interface method |

`Definition::parameter_text()` returns the parameters as a person writes them, such as `ui: &mut Ui,
at: Pos2`. An `Import` has `path`, `alias`, `glob` and `range`.

### The symbol table

`SymbolTable` answers these lookups. Each takes lower case names where the table is keyed by lower case.

| Call | Returns |
|---|---|
| `named(lower)` | The definitions of a name |
| `of_file(path)` | A file's definitions in line order |
| `members_of(container)` | Every definition whose block is the container's, with its file |
| `prefixed(prefix)` | The names that start with a prefix, with their definitions |
| `word_started(prefix)` | The names that have a later word starting with a prefix: `scroll` reaches `layout_scroll_margin` |
| `humped(prefix)` | The names whose initials start with a prefix: `lsm` reaches `layout_scroll_margin` |
| `imports_of(path)` | A file's imports in the order they are written |
| `all_imports()` | Every file's imports |

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
| `symbols`, `outline` | Definitions, outlines and chunks, and the in memory `SymbolTable` |
| `structure` | The structural reading: kinds, containers, parameters, types, visibility, documentation and imports |
| `passages`, `words` | The passage table and the words a chunk is searched by |
| `grammars`, `lang` | The language manifests and the tokeniser, definition and folding code |
| `store` | The Inillucent file and the index folder |
| `shape` | Cutting an answer to a token budget |
