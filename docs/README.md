# Atrius documentation

This folder holds the reference pages for Atrius, the code index. Each page can be read on its own.

The tables below list the pages in reading order for a programmer who has used grep or ripgrep but has
never looked inside a search index. Start with what Atrius is, then the terms, then how to install and
run it, then how it works.

`cargo test -p atrius-cli --test docs` fails when a page in `docs/` is missing from this index, and when
a page uses a phrase that [Writing style](writing-style.md) lists.

## Start here

| | Page | What it covers |
|---|---|---|
| 1 | [Product overview](product-overview.md) | What Atrius is, who it is for, and how it compares with ripgrep, git grep, ugrep and Zoekt |
| 2 | [Glossary](glossary.md) | Each search and index term the pages use, explained in one sentence |
| 3 | [Getting started](getting-started.md) | Installing, a first search, connecting an agent, and the exit codes |

## Using it

| | Page | What it covers |
|---|---|---|
| 4 | [Command reference](commands.md) | Every command, argument and flag. A program writes this page from the command table |
| 5 | [MCP](mcp.md) | The `atrius_search` tool, how to register `atrius-mcp` with Claude Code, Cursor, VS Code and other agents, and what an agent sees |
| 6 | [The background service](service.md) | Keeping chosen checkouts indexed at all times on Windows, macOS and Linux, the watch list, and `atrius doctor` |
| 7 | [Configuration](configuration.md) | Where the index and the settings live, and every environment variable |
| 8 | [Languages](languages.md) | The 24 languages Atrius reads definitions in, the manifest format, and adding a language |
| 9 | [Using the library](library.md) | Embedding the `atrius-index` crate in a Rust program, as Unluminous does |

## How it works

| | Page | What it covers |
|---|---|---|
| 10 | [Architecture](architecture.md) | The host process, the three engines, how the index stays current, the file it is kept in, and the wire protocol |

## Measurements

| | Page | What it covers |
|---|---|---|
| 11 | [Testing on real code](testing.md) | The 19 open source repositories the tests search, what each test checks, and the results |
| 12 | [Performance](performance.md) | Query times against ripgrep on every corpus, build times and index sizes |
| 13 | [Lab findings](lab/findings.md) | The study that built the index: the goals, the approaches weighed, every experiment, the scorecard, and the comparison with other tools |

The same study is published at [blackrainbowlabs.com/lab](https://blackrainbowlabs.com/lab).

## Working on it

| | Page | What it covers |
|---|---|---|
| 14 | [Repository and building](repository.md) | The crates and what each one does, building, running the tests, and releasing |
| 15 | [Writing style](writing-style.md) | How to write and check a page in this folder |
| | [`AGENTS.md`](../AGENTS.md) | The starting page for an AI agent that uses or changes this repository |
