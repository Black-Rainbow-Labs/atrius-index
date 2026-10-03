# Languages

Atrius searches every text file exactly, in any language. A language also needs a manifest for `def`,
`outline`, `fragment` and plain English search to know where its definitions are. This page lists the
24 languages compiled in, explains the manifest format, and shows how to add a language.

```sh
atrius languages
```

## The bundled languages

| Language | Extensions | Definitions come from |
|---|---|---|
| C | `.c` `.h` | `struct`, `enum`, `union`, and a name followed by its parameters and a `{` on the same line or the next |
| C++ | `.cpp` `.cc` `.cxx` `.hpp` `.hh` `.hxx` and others | `class`, `struct`, `enum`, `union`, `namespace`, `concept`, and functions as in C, with `const`, `override` and other words allowed before the `{` |
| C# | `.cs` `.csx` | `class`, `struct`, `interface`, `enum`, `record`, `namespace`, and methods as in C |
| CSS | `.css` | none |
| Go | `.go` | `func`, `type`, `const`, `var`, and methods with a receiver |
| HTML | `.html` `.htm` | none |
| Java | `.java` | `class`, `interface`, `enum`, `record`, and methods as in C, with a `throws` clause allowed |
| JavaScript | `.js` `.jsx` `.mjs` `.cjs` | `function`, `class`, `const`, `let`, `var`, and class methods |
| JSON | `.json` | none |
| Kotlin | `.kt` `.kts` | `fun`, `class`, `interface`, `object`, `typealias`, `val`, `var` |
| Lua | `.lua` | `function`, `local` |
| Mermaid | `.mmd` `.mermaid` | none |
| PHP | `.php` `.phtml` | `function`, `class`, `interface`, `trait`, `enum`, `namespace`, `const` |
| Python | `.py` `.pyw` | `def`, `class` |
| Ruby | `.rb` `.rake` `.gemspec` `.ru` | `def`, `class`, `module` |
| Rust | `.rs` | `fn`, `struct`, `enum`, `trait`, `mod`, `const`, `static`, `type`, `let` |
| Scala | `.scala` `.sc` | `def`, `class`, `object`, `trait`, `enum`, `type`, `val`, `var`, `given` |
| Shell | `.sh` `.bash` `.zsh` | a name followed by `()` and `{`, as in `deploy() {` |
| SQL | `.sql` `.ddl` `.dml` `.psql` | none |
| Swift | `.swift` | `func`, `class`, `struct`, `enum`, `protocol`, `actor`, `typealias`, `let`, `var` |
| TOML | `.toml` | none |
| TypeScript | `.ts` `.tsx` `.mts` `.cts` | as JavaScript, plus `interface`, `type`, `enum`, `namespace` |
| YAML | `.yml` `.yaml` | none |
| Zig | `.zig` `.zon` | `fn`, `const`, `var` |

Markdown has no manifest. Its headings are its outline and its sections are its chunks.

Each manifest's `plugin.limitations` line says what it misses, such as a Kotlin extension function
written `fun Type.name`, or a Ruby method written `def self.name`. Definitions found by the
parameter and brace rule are marked likely, and rank after the ones a keyword introduced.

The [corpus tests](testing.md) check hand picked definitions in 18 real repositories across these
languages.

## How a definition is found

```mermaid
flowchart LR
    T["the file's text"] --> S["one pass of the tokeniser:<br/>comments, strings, keywords, words"]
    S --> K{"a definer keyword<br/>before a word?"}
    K -->|yes| D["a sure definition:<br/>fn add, class Router"]
    K -->|no| B{"a word, then ( ... ),<br/>then {"}
    B -->|"and brace_definitions"| L["a likely definition:<br/>a C function, a Java method"]
    D --> F["folding finds where it ends:<br/>its brackets, or its indentation"]
    L --> F
```

There is no parser. A definition is a keyword the manifest names as a definer, followed by a name. A
language whose functions have no keyword, such as C or Java, turns on `language.brace_definitions`: a
name directly followed by its parameter list and an opening brace is then a likely definition. Where a
definition ends comes from its brackets, or from its indentation in Python, Ruby and other languages
without braces.

## The manifest format

A manifest is a `.conf` file of `name = value` lines. A `#` followed by a space and more text starts a
comment; a `#` that ends a line is part of the value, which is how a language whose comments start
with `#` writes `language.line_comment = #`.

```text
plugin.id          = go
plugin.name        = Go
plugin.limitations = A method with a receiver is found when its parameter list opens and closes on one line.

language.extensions    = .go
language.line_comment  = //
language.block_comment = /*, */
language.strings       = ", ', `
language.keywords      = break, case, chan, const, func, go, if, import, package, return, type, var
language.builtins      = append, len, make, new, panic, string, int, error

language.definers          = func=function, type=type, const=constant, var=variable
language.brace_definitions = true
language.definition_tail   = true
```

| Key | Meaning |
|---|---|
| `plugin.id` | The language's id, such as `go` |
| `plugin.name` | Its name, such as `Go` |
| `plugin.limitations` | What the manifest knowingly misses, in a sentence or two |
| `language.extensions` | The file extensions it claims, comma separated |
| `language.line_comment` | What starts a comment that runs to the end of the line |
| `language.block_comment` | The opening and closing marks of a block comment |
| `language.strings` | The quote characters, comma separated |
| `language.escapes` | Whether a backslash escapes the next character in a string; `true` by default |
| `language.keywords` | The reserved words. A keyword is never a definition's name or a call |
| `language.builtins` | Names the language provides. A builtin is never a definition |
| `language.types` | Names that are types even in lower case |
| `language.operators` | Characters that are operators |
| `language.word_characters` | Characters other than letters, digits and `_` that belong in a word, such as `-` in CSS |
| `language.definers` | `keyword=kind` pairs. The kinds are `function`, `type`, `constant`, `variable` and `module` |
| `language.brace_definitions` | `true` when a name followed by `( ... )` and `{` on the same line is a likely function |
| `language.brace_next_line` | `true` when that `{` may be on the next line, as in C, C# and Java written in Allman style |
| `language.definition_tail` | `true` when words may stand between the `)` and the `{`, such as `const`, `throws IOException` or `-> Int`, and when a space may stand between the name and its `(` |
| `language.export_keyword` | The word that makes a definition visible to other files, such as `export` or `pub` |
| `language.markup` | `true` for a markup language such as HTML |

`language.brace_next_line` and `language.definition_tail` exist only in Atrius. The other keys are the
same as in Unluminous's language plugins, so a manifest written for one reads in the other.

## Adding a language

Put a manifest in a folder and name the folder in `ATRIUS_LANGUAGES` before the host starts:

```sh
mkdir -p ~/.config/atrius-index/languages
cp elixir.conf ~/.config/atrius-index/languages/
export ATRIUS_LANGUAGES=~/.config/atrius-index/languages
atrius stop && atrius languages
```

A manifest in that folder claims its extensions from a bundled one, so it also replaces a bundled
language. A program that embeds the library adds one with `atrius_index::grammars::register`, as
[Using the library](library.md) shows.

To add a language to the bundled list, put its manifest in `crates/atrius-index/languages/`, add it to
`BUNDLED` in `crates/atrius-index/src/grammars.rs`, and add a corpus in that language to
`tests/corpora.json` with three hand checked definitions.
