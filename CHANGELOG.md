# Changelog

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
