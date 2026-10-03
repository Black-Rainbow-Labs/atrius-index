# Changelog

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
