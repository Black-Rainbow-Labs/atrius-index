# Testing on real code

Atrius claims to return exactly ripgrep's lines. This page describes the tests that check that claim
on 19 open source repositories written by other people, in 13 languages, from 67 files to 89,668, and
the other tests that run the programs the way a person and an agent run them.

## The corpora

`tests/corpora.json` pins each repository to a release tag. `node tests/fetch-corpora.mjs` clones each
one, shallowly, at that tag into `ATRIUS_CORPORA` and records the commit and file count in
`corpora.lock.json` there.

| Corpus | Language | Tag | Files | Size |
|---|---|---|---:|---|
| [spf13/cobra](https://github.com/spf13/cobra) | Go | v1.9.1 | 67 | small |
| [lua/lua](https://github.com/lua/lua) | C | v5.4.7 | 111 | small |
| [psf/requests](https://github.com/psf/requests) | Python | v2.32.3 | 130 | small |
| [BurntSushi/ripgrep](https://github.com/BurntSushi/ripgrep) | Rust | 14.1.1 | 213 | small |
| [expressjs/express](https://github.com/expressjs/express) | JavaScript | v5.1.0 | 226 | small |
| [sinatra/sinatra](https://github.com/sinatra/sinatra) | Ruby | v4.1.1 | 293 | small |
| [google/gson](https://github.com/google/gson) | Java | gson-parent-2.11.0 | 304 | small |
| [square/okhttp](https://github.com/square/okhttp) | Kotlin | parent-4.12.0 | 509 | medium |
| [Alamofire/Alamofire](https://github.com/Alamofire/Alamofire) | Swift | 5.10.2 | 569 | medium |
| [nlohmann/json](https://github.com/nlohmann/json) | C++ | v3.11.3 | 1,091 | medium |
| [JamesNK/Newtonsoft.Json](https://github.com/JamesNK/Newtonsoft.Json) | C# | 13.0.3 | 1,159 | medium |
| [redis/redis](https://github.com/redis/redis) | C | 7.4.2 | 1,608 | medium |
| [vitejs/vite](https://github.com/vitejs/vite) | TypeScript | v6.0.0 | 2,228 | medium |
| [laravel/framework](https://github.com/laravel/framework) | PHP | v11.0.0 | 2,544 | medium |
| [django/django](https://github.com/django/django) | Python | 5.1 | 6,811 | large |
| [golang/go](https://github.com/golang/go) | Go | go1.23.0 | 13,227 | large |
| [ziglang/zig](https://github.com/ziglang/zig) | Zig | 0.13.0 | 17,712 | large |
| [scala/scala3](https://github.com/scala/scala3) | Scala | 3.5.0 | 21,976 | large |
| [torvalds/linux](https://github.com/torvalds/linux) | C | v6.16 | 89,668 | extra large |

The file counts are the files in Atrius's file set, which is ripgrep's with hidden files included and
`.git` left out.

## The tests

| Test | File | What it checks |
|---|---|---|
| Corpus parity and definitions | `crates/atrius-index/tests/corpora.rs` | On every fetched corpus, 120 queries made from the corpus's own text (40 on Linux), each answered with exactly ripgrep's set of lines. Then every hand checked definition is found by `def` in its first three answers, and `outline` and `fragment` both name it |
| Fixture parity | `crates/atrius-index/tests/parity.rs` | CRLF line endings, a UTF-16 file with a byte order mark, binary files with matches before and after the NUL, a file over 1 MB, a nested `.gitignore`, hidden files, an ignored folder, and a file named as the search path |
| Freshness | `crates/atrius-index/tests/freshness.rs` | A new file, an edited line, a rename, a delete, a burst of 2,000 files, a branch switch and back, and five rounds of writing and searching, each followed at once by queries that must equal ripgrep's |
| The command line | `crates/atrius-cli/tests/end_to_end.rs` | Every query verb through the built `atrius`, with and without `--json`; the exit codes of six kinds of mistake; one host started and reused; `hosts` and `stop` |
| MCP | the same file | `atrius-mcp` answers `initialize` and `tools/list`, and every query through `atrius_search` gives the same text as the command line; a refusal is `isError`; `structured` adds the fields |
| The shared host | the same file | A host started by `atrius` answers a `search.find` request, which is how Unluminous asks, and refuses a wrong token |
| The service | the same file | `service install` registers the login item and starts the daemon, which hosts the watched folder; `uninstall` removes both. On Windows this writes and deletes a real Run value under a test name |
| The watch list, languages and doctor | the same file | Adding, listing and removing folders; 24 languages listed; `doctor` sees a file event |
| The command reference | the same file | `docs/commands.md` equals what the command table writes |
| Unit tests | in each module | 124 tests in the library and 7 in the command line crate |

### How the corpus queries are made

The seed is the corpus's name, so a run is repeatable. 400 files are sampled and their identifiers
counted. Each query takes a rare identifier two times in three and a common one otherwise, and cycles
through these forms:

| Form | Example |
|---|---|
| literal, three in every fourteen | `serverCron` |
| case insensitive | `-i servercron` |
| whole word | `\bserverCron\b` |
| call | `serverCron\s*\(` |
| alternation | `serverCron\|createClient` |
| anchored | `^\s*(class\|struct\|def\|func\|fn\|function\|interface\|enum)\s+[A-Z]\w*` |
| phrase with a space | `robj *lookupKeyRead` |
| `--glob *.<extension>` | the file's own extension |
| `--path <folder>` | the top folder the identifier was seen in |
| `--type <language>` | ripgrep's type name for the corpus language |
| `--glob '!*test*'` | leaves out test files |
| prefix regex | `server\w*` |

ripgrep is run with the flags Claude Code's Grep tool uses: `--no-config --hidden`, every version
control folder left out with `--glob`, `--max-columns 500`. On Windows, Linux's tree holds symbolic
links ripgrep cannot open; ripgrep then exits with code 2 and still prints every match it could read,
and the test reads those matches.

## Results

Run on 2026-10-03 on Windows 11, ripgrep 15.2.0:

| Corpus | Queries equal to ripgrep | Lines matched | Definitions found |
|---|---:|---:|---:|
| cobra | 120 / 120 | 3,444 | 3 / 3 |
| lua | 120 / 120 | 2,691 | 3 / 3 |
| requests | 120 / 120 | 945 | 4 / 4 |
| ripgrep | 120 / 120 | 4,891 | 4 / 4 |
| express | 120 / 120 | 1,967 | 3 / 3 |
| sinatra | 120 / 120 | 4,049 | 2 / 2 |
| gson | 120 / 120 | 27,303 | 3 / 3 |
| okhttp | 120 / 120 | 26,170 | 3 / 3 |
| alamofire | 120 / 120 | 30,463 | 3 / 3 |
| nlohmann-json | 120 / 120 | 11,115 | 3 / 3 |
| newtonsoft-json | 120 / 120 | 25,520 | 3 / 3 |
| redis | 120 / 120 | 15,979 | 3 / 3 |
| vite | 120 / 120 | 15,506 | 4 / 4 |
| laravel | 120 / 120 | 189,851 | 2 / 2 |
| django | 120 / 120 | 99,202 | 3 / 3 |
| go | 120 / 120 | 187,803 | 3 / 3 |
| zig | 120 / 120 | 227,460 | 2 / 2 |
| scala3 | 120 / 120 | 176,630 | 3 / 3 |
| linux | 40 / 40 | 518,188 | none listed |
| **all** | **2,200 / 2,200** | **1,569,177** | **54 / 54** |

Writing the suite found two faults, both fixed before these results:

- **Lua's C functions were not definitions.** Lua writes `void luaD_call (lua_State *L, ...)` with a
  space before the parameters, so the name was not read as a call and the function rule never ran.
  Under `language.definition_tail` a space is now allowed there.
- **vite's `createServer` ranked fourth.** It is defined once in `packages/vite/src/node/server/index.ts`
  and again in eleven `playground/*/server.js` files, which ranked ahead of it. Folders such as
  `playground`, `examples`, `docs` and `benchmarks` now rank after the source.

## Running the tests

```sh
node tests/fetch-corpora.mjs                  # about 3 GB; ATRIUS_LINUX names an existing Linux tree
pwsh tools/cargo.ps1 test --workspace         # unit, fixture, freshness, command line and MCP tests
ATRIUS_REQUIRE_CORPORA=1 pwsh tools/cargo.ps1 test --release -p atrius-index --test corpora -- --ignored --nocapture
```

On macOS and Linux, use `cargo` in place of `pwsh tools/cargo.ps1`. The corpus test takes about five
minutes in a release build, most of it in ripgrep on the large corpora. Without ripgrep on the path or
in `ATRIUS_RG`, the parity tests say so and pass, because there is nothing to compare with.
