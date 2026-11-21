# Changelog

## Unreleased

A saved index loads in seconds instead of minutes, and on macOS a query no longer waits for the file
watcher (`task-2696`, from the performance TDD of `task-2695`). Neither change has been through the
benchmark's keep rule yet; the climb was stopped before its baseline window finished.

- **Loading.** `Store::load_exact` and `Store::load_symbols` read their tables in pages of 1,048,576 rows,
  8,192 for file contents, instead of 1,024, and `file` and `blob` are read on their own and matched by
  content hash instead of joined. A page cost time in proportion to where it started in the table, so the
  load grew with the square of the row count. Timed with `examples/loadtime.rs` on the same saved index:
  ai-service 2.98 s to 0.29 s, inillucent 0.64 s to 0.21 s, Linux from more than 105 minutes to 14.7 s.
- **A kqueue gate on macOS** (`src/kq.rs`). Every file and folder of the set is opened with `O_EVTONLY` on
  one kqueue, and once that is armed and one fence has confirmed it, the gate drains the kqueue with a
  zero timeout instead of writing a fence and waiting for FSEvents. A note is posted inside the writer's
  own system call, so a change made before the query is always in the drain. The FSEvents fence took a
  median of 10.75 ms on an M6; the drain takes 0.011 ms. A probe with the writer in another process saw
  500 of 500 for each of an in place write, an append, a truncate, a write through `mmap`, a create, a
  delete, a rename over a file, a folder rename, `clonefile` and `RENAME_SWAP`. A root on a volume that
  is not local, a root that needs more than half the descriptors the process may open (Linux's tree
  does), and every root under `ATRIUS_KQUEUE=off` keep the fence. `status` says which with `freshness`.
- **Two hosts of one checkout no longer take each other's fences.** Every watcher's fence files carry a
  value of its own, and a fence of another watcher is ignored. Before, a host whose fence numbers were
  lower than another host's passed its fence before its own event arrived.
- **The gate takes the write lock only when something changed**, and a query that waited behind another
  gate does not fence again when that gate began after the query arrived. `status` counts gates whose
  fence did not arrive as `unfencedGates`.
- **The benchmark** is `examples/perf.rs`: ripgrep, `atrius-mcp`, `atrius serve` and a one shot
  `atrius find`, timed from request to answer with the gate and shaping included. Phase counters are in
  `src/phases.rs` behind the `phases` feature, which no release turns on.
- New tests: `tests/fences.rs` (500 writes each searched straight after, half from another process;
  deletes, renames, a new folder and a renamed folder) and `tests/fence_wait.rs` (two indexes of one
  checkout, and a query that starts while another's fence is pending).

## 0.3.2

A folder that is not a git repository no longer gets a file written into it. The freshness gate used
to write `atrius-index-fence-<n>` into the root of such a folder on every query and remove it up to
250 ms later, so a sync client, a build watcher or another editor saw a file come and go, a listing
could count it, and a copy could fail on it (Unluminous `task-2370`). A checkout with `.git` is
unchanged: its fence is in `.git`.

- The fence of a folder with no `.git` is written in `<index folder>/fence` and that folder is added to
  the same watcher. `Freshness::fence_folder` says where the fences go.
- On Windows the fence folder has its own `ReadDirectoryChangesW` handle, so a batch of the root's events
  can arrive after the fence's. The gate counts the root's events and writes another fence when any
  arrived between writing a fence and seeing it. A fence with none in between means every event from
  before it has arrived. On Linux and macOS the order across watches already holds.
- When the index folder cannot be made or watched, the fence goes in the root as before.
- The freshness test runs on a folder with no git as well, with forty bursts of 50 files each searched
  at once, while a thread lists the folder and fails if a fence appears. The same test with the fence in
  the root fails; with one fence a query and no repeat it passed three runs out of three, while about 45
  queries a run would have repeated the fence.

## 0.3.1

No file is read into memory whole when it is larger than 32 MB (`exact::LARGEST_HELD`). A macOS hang
report from Unluminous 1.3.3 showed a window at a footprint of 180 GB on a 64 GB Mac, with all fourteen
indexing threads inside `FileRecord::read`: every file in the project was read whole, so one disk image
or log of tens of gigabytes was enough to take the whole machine's memory.

- A text file larger than the limit is a record that holds nothing and has no trigrams. Every search
  is a candidate for it and reads it from the disk in pieces, the way ripgrep reads a file it does not
  map, so the lines found are the same. `FileRecord::is_large` says which records these are.
- A binary file larger than the limit is read only up to its first NUL, which is all a directory search
  reads of it.
- A file that grows while it is read stops at the limit too, because the read is bounded by bytes read
  and not by the size the file had when it was listed.
- The freshness gate, the direct scan and the `fragment` and `outline` verbs read files the same bounded
  way. `exact::read_bounded` is the one function that does it.
- `Exact::build` reads and adds 1,024 files at a time, so the trigram lists waiting to be added are those
  of one slice of the project rather than of all of it.
- The schema version is 5, so an index written before this, which may hold such a file whole and would
  read it into memory as it loads, is replaced.
- The parity test against ripgrep gained a 40 MB log and a 40 MB disk image.

## 0.3.0

A loaded index holds about a third of the memory it did. Measured on the Unluminous checkout (2,000
files) in a probe that opens the index and waits: private memory went from 140 MB to 51 MB, the live heap
from 116 MB to 31 MB, and the number of live allocations from 393,000 to 250,000. Search results are
unchanged, and the parity test against ripgrep now runs every search a second time after the index has
let go of the file contents.

- **Breaking:** `SymbolTable::named` returns an iterator of `DefinedRef`, which borrows the path and the
  definition, where it returned a `Vec<Defined>`. `DefinedRef::to_owned` gives the old value. Each path
  is held once, as an `Arc<str>` shared by every place that names it, where every place held its own
  `String`.
- File contents are no longer held in memory once the index is built. A search verifies a candidate
  block by reading it from the disk, which is what ripgrep does for every block. On this checkout an
  exact name is still found in under a millisecond and the pattern `fn ` in 20 to 26 ms.
  `atrius_index::exact::set_holding_content(true)` or `ATRIUS_HOLD_CONTENT=on` keeps them, for a host
  that is asked many questions a second.
- The buffer pool of the index file is 128 pages of 32 KiB once the index is built, where it was 1,024.
  Building the passage table needs about 1,024 pages, and fails with `out of memory [syntax]` with 512 or
  fewer, so a host opens the file with 1,024 pages while it builds or writes a change and reopens it with
  the small pool afterwards. `ATRIUS_STORE_CACHE_FRAMES` still sets the number used at rest.
  `tests/pool.rs` builds an index that fails with 128 pages and checks that it builds and loads.
- The search index Inillucent builds for a plain English question is let go of 20 seconds after the last
  question (`ATRIUS_SEARCH_INDEX_IDLE_SECONDS`). It was 115 MB on this checkout and was held until the
  next change to a file. `search status` says whether it is held, as `searchIndexHeld`.
- The trigram postings and the symbol listings are kept in smaller forms: a posting list of up to 14
  bytes is stored inline, and the word and capital letter listings are sorted vectors with short keys
  stored inline, where they were maps of owned strings.
- The index file, the trigrams and the symbols are read in pages of 1,024 rows, so loading an index no
  longer holds every row of a table in memory at once.
- Embedding: switching it on while an index is still being set up could be lost, because the host read
  the switch before it had read its own status. The host now starts from the status.

## 0.2.2

- Embedding can be switched on and off while an index runs, with `atrius_index::index::set_embedding`.
  It starts from `ATRIUS_EMBED` as before. Off, each store thread drops its embedder before its next
  batch and a plain English search is ranked by its words, so the model is never asked again; on, a
  store thread opens its embedder within half a second and goes on from the chunks with no vector.
  Unluminous turns this into a setting, because the model was 1.1 GB of its window at rest.
- The index file's buffer pool holds 32 MiB, where Inillucent's default is 128 MiB. The file is read whole
  once when a host opens it, and the pool kept what that read filled for the life of the host. A window
  hosting the index of a 2,000 file checkout held about 100 MB less. `ATRIUS_STORE_CACHE_FRAMES` sets
  the number of 32 KiB pages.
- A binary file is held in memory and in the index file only up to and including its first NUL. A
  directory search never reads past it, which is ripgrep's rule, and a search that names the file reads
  it from the disk, as ripgrep does. A record an older index file holds whole is cut as it loads. Results
  are unchanged; the parity test against ripgrep covers matches before and after the NUL.
- `search status` says `embedding is switched off` in `notEmbedding` while the switch is off. It said
  `ATRIUS_EMBED is off` before.

## 0.2.1

- A change inside a folder the ignore rules leave out is no longer indexed. The watcher walked a changed
  folder starting at that folder, so the rule naming a folder above it, such as `/target`, was never
  asked. Every build added its output to the index: one Rust checkout held 22,386 files of `target`
  output, its index file was 11.7 GB, and a host that opened it read the file whole into memory. A
  changed folder is now walked from the root down to that folder, under the same rules as the first walk.
- A file's stored content is deleted when the file changes or is removed and no other file has the same
  content. Before, every version of every file was kept.
- The index file has schema version `4`. The tables are the same. An index file written by an earlier
  release is deleted and built again, so the space the faults above took is given back.

## 0.2.0

- A structural reading of every definition. Each definition now has a kind (one of 16: `function`,
  `method`, `type`, `struct`, `enum`, `variant`, `trait`, `interface`, `class`, `field`, `constant`,
  `variable`, `module`, `parameter`, `alias` and `macro`), the container it is inside, its parameters with
  their types, its return type, the type of a variable or field, its visibility (`public`, `crate` or
  `private`), and the first line of its documentation. An `impl X for Y` gives the container `Y`, and a Go
  receiver gives the type it names. Fields, enum variants and interface methods are listed as members.
- Each file's import statements are read into paths with an alias and a glob flag: Rust `use`, quoted
  imports in TypeScript and JavaScript, and Go imports.
- Nine new manifest keys describe how a language writes these parts: `language.containers`,
  `language.records`, `language.enums`, `language.annotation`, `language.returns`, `language.visibility`,
  `language.kinds`, `language.members` and `language.receivers`. A manifest that names a visibility
  other than `public`, `crate` or `private`, or a kind that is not one of the 16, is refused with a
  message. All 16 bundled languages that define symbols set them. A language that names none of them is
  read as before.
- `def`, `outline` and `fragment` rows in JSON carry `symbolKind`, `visibility`, `container`,
  `parameters`, `returns`, `typeText` and `doc` when they are known. `kind` is still the family it was.
  `fragment` adds a `definition` object when it returns one short definition.
- The definitions of every file are held in a symbol table with lookups by name, file, container, name
  prefix, later word and initials, and the imports of each file.
- The index file has schema version `3`, with a `symbol` table and an `import` table keyed by the file's
  path. An index written by an earlier release is built again once. The tables load from the file when
  they are current, so a host that starts with an index file does not read every file again for its
  definitions.
- New library calls: `Index::try_symbols`, which never waits, `Index::symbols_ready`,
  `Index::keep_fresh`, `Index::open_in_memory` and `outline::read_structure`. `outline::Definition` has
  the new fields `symbol_kind`, `container`, `parameters`, `returns`, `type_text`, `visibility`, `doc`,
  `range` and `member`, and the method `parameter_text()`. `outline::Outline` has an `imports` field.

## 0.1.2

- `outline` lists every heading of a Markdown file, at every level, nested by level, with the line each
  section ends on. It used to list a `#` title as one section for the whole document and none of the
  `##` headings under it. Each heading also starts its own search chunk now, and a chunk's header names
  the sections it is in.
- The store thread no longer ends when looking for the embedding model panics, or when building the
  passage table does. The reason is shown by `status`, and plain English questions go on being answered.
  If the thread does end, a question is told why instead of only "the index's store has stopped".
- `ATRIUS_EMBED=off` stops the model being looked for at all, rather than only stopping it being used.
- `status` says that searching is safe while the index is still being built: `find` reads the files
  directly with the same results, and the other queries wait for the index. Its JSON has `searchable`
  and `answeredBy`, and the text carries the store error and why nothing is being embedded.

## 0.1.0

The first release of Atrius, the code index that shipped inside Unluminous 0.58.0, as its own product.

- `atrius`, a command line with the seven queries (`find`, `def`, `refs`, `fragment`, `outline`,
  `files`, `status`), `serve`, `stop`, `hosts`, `watch`, `service`, `daemon`, `doctor` and
  `languages`.
- `atrius-mcp`, an MCP server with one tool, `atrius_search`, giving the same text as the command line.
- A background service for each platform: a Run value on Windows, a LaunchAgent on macOS, a systemd
  user unit or an autostart entry on Linux.
- Definitions in 24 languages. New: C, C++, C#, Go, Java, Kotlin, Lua, PHP, Ruby, Scala, Swift and Zig,
  with the manifest keys `language.brace_next_line` and `language.definition_tail`.
- `def` ranks folders such as `examples/`, `playground/` and `docs/` after the source.
- `find` for one name keeps its definitions inside `--path`, `--glob` and `--type`.
- Queries other than `find` wait for an index that is still being built instead of refusing.
- A test suite on 19 open source repositories pinned to release tags.
