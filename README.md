# Atrius

A code index for AI agents and for people. Atrius answers the questions an agent asks ripgrep, with
exactly ripgrep's lines, from an index it keeps current as files change. It also finds where a name is
defined, shows just the function around a line, lists a file's definitions, and answers questions in
plain English.

```sh
atrius find resolveSkipToken          # ripgrep's lines, grouped by file, cut to a token budget
atrius def RetryPolicy                # where a name is defined
atrius fragment src/layout.rs:386     # the function around a line
atrius find where do we retry a failed write
```

| | |
|---|---|
| Speed | 32x to 40x faster than the ripgrep inside Claude Code on three repositories, 140x on the Linux kernel, held in the agent's MCP process |
| Exactness | the same lines as ripgrep on all 2,200 queries over 19 open source repositories in 13 languages, and on all 468 held out agent grep patterns in the study |
| Freshness | 0 stale results after edits, renames, deletes, bursts of thousands of files and branch switches |
| Languages | definitions in 24 languages, with kinds, containers, parameters, types and imports, from manifests anyone can extend |
| Platforms | Windows, macOS and Linux, with an optional background service on each |

## Install

```powershell
irm https://blackrainbowlabs.com/downloads/atrius-index/install.ps1 | iex       # Windows
```

```sh
curl -fsSL https://blackrainbowlabs.com/downloads/atrius-index/install.sh | sh  # macOS and Linux
brew install black-rainbow-labs/atrius/atrius-index                             # Homebrew
cargo install atrius-cli                                                        # from source
```

## Connect an agent

```sh
claude mcp add atrius -- atrius-mcp
```

`atrius-mcp` offers one tool, `atrius_search`. [docs/mcp.md](docs/mcp.md) has the lines for Cursor,
VS Code and Codex.

## Documentation

[docs/README.md](docs/README.md) lists every page. Start with the
[product overview](docs/product-overview.md) and [getting started](docs/getting-started.md). The
study that built the index is [docs/lab/findings.md](docs/lab/findings.md), also published at
[blackrainbowlabs.com/lab](https://blackrainbowlabs.com/lab).

## Licence

MIT or Apache 2.0, at your choice. See `LICENSE-MIT` and `LICENSE-APACHE`.
