# Changelog

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
