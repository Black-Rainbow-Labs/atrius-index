# Product overview

Atrius is a code index for AI agents and for people. It answers the questions an agent asks ripgrep,
and answers them from an index that it keeps current as files change. This page says what Atrius does,
who it is for, and how it compares with the tools it replaces.

## What it answers

```sh
atrius find resolveSkipToken           # the lines ripgrep would print, grouped by file
atrius def RetryPolicy                 # where a name is defined
atrius refs add_numbers                # every use of a name
atrius fragment src/layout.rs:386      # the function around a line, not the whole file
atrius outline src/layout.rs           # a file's definitions with their line ranges
atrius files "*.test.ts"               # files by name or glob
atrius find where do we retry a failed write   # a question in plain English
```

| Question | What Atrius does |
|---|---|
| A literal or a regex | Returns exactly the lines ripgrep returns, from a trigram index, cut to a token budget |
| Where is this defined | Reads definitions with keyword rules for 24 languages and ranks the project's source before tests and examples |
| Show me just this function | Returns the enclosing function with line numbers, so an agent does not read a whole file |
| A question in plain English | Ranks chunks of code by words, and by meaning when an embedding model is installed, and says when nothing matches |

## Who it is for

- **An AI agent** that searches a codebase many times in one session. The MCP server `atrius-mcp`
  offers one tool, `atrius_search`, which an agent uses in place of Grep, Glob and reading whole files.
- **A person at a terminal** who wants ripgrep's answers on a large repository without waiting for a
  full scan each time.
- **A program** that embeds search. Unluminous, the editor, links the `atrius-index` crate and shares
  each checkout's index with the `atrius` command line.

## How it compares

These numbers come from the study described in [Lab findings](lab/findings.md). The queries were the
grep patterns agents actually ran, mined from 1,076 Claude Code sessions, timed on one Windows machine.

| Tool | Speed against ripgrep, three local repositories / Linux | Same lines as ripgrep |
|---|---|---|
| Atrius, in the agent's MCP process | 32x to 40x / 140x | all 468 queries |
| Atrius, through a separate host process | 9.5x to 10x / 15x | all 468 |
| ugrep with its own index | 1.7x to 1.9x / 1.4x | 92% to 97% |
| ugrep | 2.0x to 2.3x / 1.1x | 87% to 97% |
| git grep | 1.2x to 1.3x / 1.0x | 99% to 100% |
| Zoekt, in WSL, against ripgrep in WSL | 5x to 6x / 51x | 93% to 99% |

ripgrep's time includes starting a process, which is 17 ms of a 40 to 50 ms search on Windows. Atrius
answers from a process that is already running, which is why the ratio is largest when the agent's MCP
server hosts the index itself. On the 19 open source repositories in [Testing on real code](testing.md),
the median query took 0.5 to 17 ms in the index and 16 to 232 ms through ripgrep.

## What it costs

| Cost | Size |
|---|---|
| Memory | The text of the checkout, compressed, plus its trigram lists: 19 MB for vite's 2,228 files, 94 MB for Go's 13,227, 849 MB for Linux |
| Disk | One Inillucent file per checkout in the user's cache folder, larger than the code because of the passage table: 80 MB for vite, 1.1 GB for Go, 13 GB for Linux |
| First build | Exact search ready in 0.6 s for vite, 3.5 s for Go, 28 s for Linux; plain English search 2 s, 40 s and 9 minutes |
| Processor at rest | Nothing between file changes. The watcher wakes on a change, and a query reindexes only changed files in its scope |

## What it does not do

- It does not resolve types. `refs` finds a name by its spelling, as ripgrep does, and lists the lines
  that define it first.
- It does not download anything. Plain English search uses an embedding model only when one is
  already installed with `inillucent setup-embeddings`.
- It does not change your files. The index lives in the user's cache folder. The only file it writes
  inside a checkout is an empty fence file in `.git`, which git ignores, created and removed within a
  query to learn that the watcher has delivered every earlier event. [Architecture](architecture.md)
  explains the fence.

## Where to go next

- [Getting started](getting-started.md) to install it and run a first search.
- [MCP](mcp.md) to connect an agent.
- [Architecture](architecture.md) for how it works.
