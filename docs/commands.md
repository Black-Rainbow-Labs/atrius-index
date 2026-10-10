# Command reference

Every command `atrius` has, with its arguments and flags. This page is written from the command
table in `crates/atrius-cli/src/catalogue.rs` by `atrius help --markdown`, and a test fails when the
two differ.

The query commands are also the commands of the MCP tool `atrius_search`. [MCP](mcp.md) explains how
an agent calls them.

| Command | What it does |
|---|---|
| [`find`](#find) | Search the code: the same lines ripgrep would print for a literal or a regex, grouped by file and cut to a token budget, from an index kept current as files change. |
| [`def`](#def) | Where a name is defined: each definition's path, line and signature line, the likeliest first. With --json a row also has its kind, container, parameters, return type, visibility and doc comment where the language gives them. |
| [`refs`](#refs) | Every use of a name, grouped by file, the checkout's own source before tests and docs, and in each file the lines that define it first. |
| [`fragment`](#fragment) | The function or section around a line, with line numbers and the signatures that enclose it, so it can be read without reading the whole file. |
| [`outline`](#outline) | A file's definitions, or a Markdown file's headings, one line each with their line ranges. With --json a definition also has its kind, container, parameters, return type, visibility and doc comment. |
| [`files`](#files) | Files whose path matches a name, a part of one, or a glob, best match first. |
| [`status`](#status) | Whether the index of the checkout is ready, how many files it holds, how much memory it uses, where its file is and which process hosts it. |
| [`serve`](#serve) | Run the index host for a checkout in this terminal. A query starts one on its own when none is running, so this is only needed to start it ahead of time or to watch it. |
| [`stop`](#stop) | Stop the index host of a checkout. The index file stays, so the next host loads it. |
| [`hosts`](#hosts) | Every index host on this machine: its checkout, process, port and whether it answers. |
| [`mcp`](#mcp) | Run the MCP server on standard input and output. The same program as `atrius-mcp`. |
| [`watch add`](#watch-add) | Add a folder to the list the background service keeps indexed. |
| [`watch remove`](#watch-remove) | Take a folder off the list the background service keeps indexed. |
| [`watch list`](#watch-list) | The folders the background service keeps indexed. |
| [`service install`](#service-install) | Start the background service at every login, and now. A login item for this user: the Run key on Windows, a LaunchAgent on macOS, a systemd user unit or an autostart entry on Linux. |
| [`service uninstall`](#service-uninstall) | Stop the background service and remove its login item. |
| [`service status`](#service-status) | Whether the login item is installed and the background service is running. |
| [`daemon`](#daemon) | What the background service runs: hosts every watched folder and never stops on its own. Reads the watch list again every 10 seconds. |
| [`doctor`](#doctor) | Check this machine: the cache folder, that file events arrive, the Linux inotify limit against the watched folders, and whether embeddings can run. |
| [`languages`](#languages) | The languages the index reads definitions in, and the file extensions each one claims. |
| [`help`](#help) | This list, or one command's usage. |

## Query commands

A query is answered by the index host of the checkout. The first query starts one, and it stays
running for two hours after the last query.

### find

Search the code: the same lines ripgrep would print for a literal or a regex, grouped by file and cut to a token budget, from an index kept current as files change.

```
atrius find <query...> [--mode <auto|literal|regex|symbol|semantic>] [--path <path>] [--glob <glob>] [--type <name>] [-i|--ignore-case] [--budget <tokens>] [--structured] [--root <folder>] [--json]
```

| Argument | What it is |
|---|---|
| `query` | What to look for: text, a regex, a name, or a question in plain English such as `where do we retry a failed write`. |

| Flag | What it does |
|---|---|
| `--mode <auto|literal|regex|symbol|semantic>` | How to read the query. `auto` by default: a regex when it has regex characters, a name's definitions and uses for one identifier, a question about meaning for a sentence, plain text otherwise. |
| `--path <path>` | A folder or file inside the checkout to search, instead of all of it. |
| `--glob <glob>` | Only files matching these globs, as ripgrep takes them: `*.rs`, `!*.test.ts`, `src/**`. |
| `--type <name>` | Only files of this ripgrep type, such as `rust`, `ts` or `py`. |
| `-i`, `--ignore-case` | Match regardless of case. |
| `--budget <tokens>` | How much text to answer with, in tokens. 1500 by default; 0 for every line. |
| `--structured` | Also answer with the fields behind the text, for a program to read. |
| `--root <folder>` | The checkout. The git checkout at or above the working folder by default. |
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius find resolveSkipToken
atrius find "fn \w+_token" --mode regex --glob *.rs
atrius find watchdog -i --path backend --json
atrius find where do we retry a failed write
```

### def

Where a name is defined: each definition's path, line and signature line, the likeliest first. With --json a row also has its kind, container, parameters, return type, visibility and doc comment where the language gives them.

```
atrius def <name> [--limit <number>] [--path <path>] [--structured] [--root <folder>] [--json]
```

| Argument | What it is |
|---|---|
| `name` | The name: a function, type, constant or module. `Type::name` and `Type.name` narrow it to a member. |

| Flag | What it does |
|---|---|
| `--limit <number>` | How many definitions to list. 10 by default. |
| `--path <path>` | Only definitions under this folder. |
| `--structured` | Also answer with the fields behind the text, for a program to read. |
| `--root <folder>` | The checkout. The git checkout at or above the working folder by default. |
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius def resolveSkipToken
atrius def Layout --path crates/core --json
```

### refs

Every use of a name, grouped by file, the checkout's own source before tests and docs, and in each file the lines that define it first.

```
atrius refs <name> [--path <path>] [--glob <glob>] [--budget <tokens>] [--structured] [--root <folder>] [--json]
```

| Argument | What it is |
|---|---|
| `name` | The name. |

| Flag | What it does |
|---|---|
| `--path <path>` | Only uses under this folder or in this file. |
| `--glob <glob>` | Only files matching these globs. |
| `--budget <tokens>` | How much text to answer with, in tokens. 1500 by default; 0 for every use. |
| `--structured` | Also answer with the fields behind the text, for a program to read. |
| `--root <folder>` | The checkout. The git checkout at or above the working folder by default. |
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius refs resolveSkipToken
atrius refs relayout --path crates --budget 0
```

### fragment

The function or section around a line, with line numbers and the signatures that enclose it, so it can be read without reading the whole file.

```
atrius fragment <target> [--line <number>] [--context <lines>] [--structured] [--root <folder>] [--json]
```

| Argument | What it is |
|---|---|
| `target` | A file and a line, such as `src/layout.rs:386`. |

| Flag | What it does |
|---|---|
| `--line <number>` | The line, when the target is a file alone. |
| `--context <lines>` | Extra lines before and after it. None by default. |
| `--structured` | Also answer with the fields behind the text, for a program to read. |
| `--root <folder>` | The checkout. The git checkout at or above the working folder by default. |
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius fragment src/layout.rs:386
atrius fragment backend/src/main.ts --line 40 --context 5
```

### outline

A file's definitions, or a Markdown file's headings, one line each with their line ranges. With --json a definition also has its kind, container, parameters, return type, visibility and doc comment.

```
atrius outline <path> [--structured] [--root <folder>] [--json]
```

| Argument | What it is |
|---|---|
| `path` | The file. |

| Flag | What it does |
|---|---|
| `--structured` | Also answer with the fields behind the text, for a program to read. |
| `--root <folder>` | The checkout. The git checkout at or above the working folder by default. |
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius outline src/layout.rs
atrius outline README.md --json
```

### files

Files whose path matches a name, a part of one, or a glob, best match first.

```
atrius files <query...> [--limit <number>] [--path <folder>] [--structured] [--root <folder>] [--json]
```

| Argument | What it is |
|---|---|
| `query` | A file name, part of one, or a glob such as `**/*.test.ts`. |

| Flag | What it does |
|---|---|
| `--limit <number>` | How many to list. 20 by default. |
| `--path <folder>` | Only files under this folder; a glob is matched relative to it. |
| `--structured` | Also answer with the fields behind the text, for a program to read. |
| `--root <folder>` | The checkout. The git checkout at or above the working folder by default. |
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius files layout.rs
atrius files "*.test.ts" --limit 50
atrius files "*.rs" --path crates/core
```

### status

Whether the index of the checkout is ready, how many files it holds, how much memory it uses, where its file is and which process hosts it.

```
atrius status [--root <folder>] [--json]
```

| Flag | What it does |
|---|---|
| `--root <folder>` | The checkout. The git checkout at or above the working folder by default. |
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius status
atrius status --json
```

## Commands that run locally

### serve

Run the index host for a checkout in this terminal. A query starts one on its own when none is running, so this is only needed to start it ahead of time or to watch it.

```
atrius serve [--idle <minutes>] [--root <folder>] [--json]
```

| Flag | What it does |
|---|---|
| `--idle <minutes>` | Stop after this many minutes with no request. 120 by default, or `ATRIUS_IDLE_MINUTES`; 0 runs until stopped. |
| `--root <folder>` | The checkout. The git checkout at or above the working folder by default. |
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius serve
atrius serve --root ~/code/project --idle 0
```

### stop

Stop the index host of a checkout. The index file stays, so the next host loads it.

```
atrius stop [--root <folder>] [--json]
```

| Flag | What it does |
|---|---|
| `--root <folder>` | The checkout. The git checkout at or above the working folder by default. |
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius stop
atrius stop --root ~/code/project
```

### hosts

Every index host on this machine: its checkout, process, port and whether it answers.

```
atrius hosts [--json]
```

| Flag | What it does |
|---|---|
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius hosts
atrius hosts --json
```

### mcp

Run the MCP server on standard input and output. The same program as `atrius-mcp`.

```
atrius mcp [--root <folder>] [--json]
```

| Flag | What it does |
|---|---|
| `--root <folder>` | The checkout. The git checkout at or above the working folder by default. |
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius mcp
claude mcp add atrius -- atrius mcp
```

### watch add

Add a folder to the list the background service keeps indexed.

```
atrius watch add <folder> [--json]
```

| Argument | What it is |
|---|---|
| `folder` | The checkout. |

| Flag | What it does |
|---|---|
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius watch add ~/code/project
atrius watch add .
```

### watch remove

Take a folder off the list the background service keeps indexed.

```
atrius watch remove <folder> [--json]
```

| Argument | What it is |
|---|---|
| `folder` | The checkout. |

| Flag | What it does |
|---|---|
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius watch remove ~/code/project
```

### watch list

The folders the background service keeps indexed.

```
atrius watch list [--json]
```

| Flag | What it does |
|---|---|
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius watch list
```

### service install

Start the background service at every login, and now. A login item for this user: the Run key on Windows, a LaunchAgent on macOS, a systemd user unit or an autostart entry on Linux.

```
atrius service install [--json]
```

| Flag | What it does |
|---|---|
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius service install
```

### service uninstall

Stop the background service and remove its login item.

```
atrius service uninstall [--json]
```

| Flag | What it does |
|---|---|
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius service uninstall
```

### service status

Whether the login item is installed and the background service is running.

```
atrius service status [--json]
```

| Flag | What it does |
|---|---|
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius service status --json
```

### daemon

What the background service runs: hosts every watched folder and never stops on its own. Reads the watch list again every 10 seconds.

```
atrius daemon [--json]
```

| Flag | What it does |
|---|---|
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius daemon
```

### doctor

Check this machine: the cache folder, that file events arrive, the Linux inotify limit against the watched folders, and whether embeddings can run.

```
atrius doctor [--json]
```

| Flag | What it does |
|---|---|
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius doctor
atrius doctor --json
```

### languages

The languages the index reads definitions in, and the file extensions each one claims.

```
atrius languages [--json]
```

| Flag | What it does |
|---|---|
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius languages
```

### help

This list, or one command's usage.

```
atrius help [command...] [--json]
```

| Argument | What it is |
|---|---|
| `command` | A command, such as `find` or `watch add`. Optional. |

| Flag | What it does |
|---|---|
| `--json` | Print the whole reply as JSON instead of its text. |

Examples:

```sh
atrius help
atrius help find
```

## Exit codes

| Code | Meaning |
|---|---|
| 0 | The command was carried out |
| 1 | The command was refused: a folder or file that is not there, a pattern that does not compile, a host that would not start |
| 2 | The command line was wrong: an unknown command, a missing argument, an unknown flag |
