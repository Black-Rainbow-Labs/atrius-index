# Performance

This page gives Atrius's measured query times, build times, memory and index sizes, and says what
each number includes. Every figure here comes from a run on 2026-10-03 on one Windows 11 machine with
an Intel Core Ultra 9 285, ripgrep 15.2.0, and Atrius 0.1.0 built in release mode.

| Term | Meaning on this page |
|---|---|
| Ready | the exact index answers; `find`, `def` and `outline` are answered from memory |
| Passages ready | the passage table for plain English search is built too |
| Index file | `index.rdb` in the cache folder, without vectors |

## Query time on 19 repositories

From the corpus test in [Testing on real code](testing.md). Each median is over 120 queries made from
the repository's own text (40 on Linux). The index time is the exact engine's search function inside
the process, which is what an MCP server that hosts the index pays. The ripgrep time is a ripgrep
process with the Grep tool's flags, which is what an agent's Grep tool pays, including about 15 ms to
start a process on Windows.

| Repository | Files | Index median | ripgrep median | Lines matched by 120 queries |
|---|---:|---:|---:|---:|
| cobra | 67 | 0.50 ms | 15.95 ms | 3,444 |
| lua | 111 | 0.50 ms | 16.78 ms | 2,691 |
| requests | 130 | 0.49 ms | 16.11 ms | 945 |
| ripgrep | 213 | 0.56 ms | 17.25 ms | 4,891 |
| express | 226 | 0.49 ms | 17.00 ms | 1,967 |
| sinatra | 293 | 0.47 ms | 16.87 ms | 4,049 |
| gson | 304 | 0.62 ms | 18.10 ms | 27,303 |
| okhttp | 509 | 0.78 ms | 19.69 ms | 26,170 |
| alamofire | 569 | 1.34 ms | 21.00 ms | 30,463 |
| nlohmann-json | 1,091 | 0.99 ms | 29.34 ms | 11,115 |
| newtonsoft-json | 1,159 | 1.86 ms | 28.30 ms | 25,520 |
| redis | 1,608 | 1.58 ms | 30.27 ms | 15,979 |
| vite | 2,228 | 1.58 ms | 35.13 ms | 15,506 |
| laravel | 2,544 | 2.14 ms | 35.69 ms | 189,851 |
| django | 6,811 | 4.54 ms | 76.52 ms | 99,202 |
| go | 13,227 | 9.21 ms | 122.28 ms | 187,803 |
| zig | 17,712 | 5.95 ms | 161.14 ms | 227,460 |
| scala3 | 21,976 | 17.46 ms | 232.26 ms | 176,630 |
| linux | 89,668 | 525.65 ms | 785.26 ms | 518,188 (40 queries) |

**Why Linux is close to ripgrep here.** The generated queries take identifiers from the code, and on
Linux a common one such as `socket` matches 20,936 lines in thousands of files. The index has to read
and verify every one of those files, the same work ripgrep does, and on that much text the index does
not win by much. An index wins by not opening files that cannot match. The agent grep patterns in the
study were narrower: on Linux they had a median of 6.1 ms in the index against 999.8 ms for ripgrep.
[Lab findings](lab/findings.md) has those runs.

These are the engine's times for every matching line. A query from `atrius` or `atrius-mcp` also cuts
the answer to a token budget, 1,500 tokens by default, and a one off `atrius find` adds a loopback
round trip to the host of about 3 ms.

## Building an index

From `crates/atrius-index/examples/measure.rs`, with embedding off and each index built from nothing:

| Repository | Source | Ready | Passages ready | Chunks | Memory, exact index | Index file |
|---|---:|---:|---:|---:|---:|---:|
| cobra | 67 files | 0.1 s | 0.2 s | 329 | 1 MB | 2 MB |
| requests | 130 files | 0.2 s | 0.3 s | 234 | 7 MB | 8 MB |
| ripgrep | 213 files | 0.2 s | 0.7 s | 922 | 4 MB | 14 MB |
| gson | 304 files | 0.2 s | 0.9 s | 1,058 | 3 MB | 24 MB |
| okhttp | 509 files | 0.3 s | 1.6 s | 1,984 | 6 MB | 51 MB |
| nlohmann-json | 1,091 files | 0.4 s | 2.1 s | 3,253 | 13 MB | 78 MB |
| redis | 1,608 files | 0.6 s | 3.9 s | 6,054 | 16 MB | 141 MB |
| vite | 2,228 files, 27 MB | 0.6 s | 2.2 s | 3,475 | 19 MB | 80 MB |
| laravel | 2,544 files | 0.5 s | 3.9 s | 6,823 | 16 MB | 149 MB |
| django | 6,811 files | 1.5 s | 10.8 s | 12,977 | 44 MB | 304 MB |
| go | 13,227 files, 170 MB | 3.5 s | 39.5 s | 44,420 | 94 MB | 1,097 MB |
| zig | 17,712 files, 237 MB | 4.2 s | 63.1 s | 72,901 | 102 MB | 1,826 MB |
| scala3 | 21,976 files | 2.4 s | 16.7 s | 28,089 | 52 MB | 540 MB |
| linux | 89,668 files, 1.6 GB | 28.1 s | 561.2 s | 503,898 | 849 MB | 13,385 MB |

"Memory, exact index" is what the exact engine holds: each file's text compressed with lz4, and the
trigram lists. The host process holds more than that while it builds the passage table.

While an index builds, `find` is answered by scanning the files directly, and the other queries wait
up to 30 seconds for it ([Configuration](configuration.md), `ATRIUS_READY_WAIT_SECONDS`). A host that
starts with an index file already on disk loads it rather than building it.

## Where the index file's size goes

The index file is larger than the code it indexes, and the passage table is the reason. Measured on the
Go repository's 1,097 MB file:

| Part | Size |
|---|---:|
| Each file's text, compressed (`blob`) | 46 MB |
| Trigram lists (`trigram`) | 24 MB |
| The passage table's own text: chunk bodies, headers and split words | 108 MB |
| The passage table's search structures inside Inillucent: the full text index, the facets and the space for vectors | about 920 MB |

Exact search, `def`, `refs`, `fragment` and `outline` need only the first two rows. Plain English search
needs the passage table. [Lab findings](lab/findings.md) recommends making the passage table smaller,
or optional, as the next change to storage.

## Reproducing these numbers

```sh
node tests/fetch-corpora.mjs
ATRIUS_REPORT=corpus-report.json pwsh tools/cargo.ps1 test --release -p atrius-index --test corpora -- --ignored --nocapture
pwsh tools/cargo.ps1 run --release -p atrius-index --example measure -- cobra=<folder> go=<folder>
```
