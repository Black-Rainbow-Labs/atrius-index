# How to write Atrius documentation

This page is the style guide for every page in `docs/`. Read it before you write or change a page.

Most readers are programmers who have used grep or ripgrep but have never looked inside a search
index. Write for that reader.

## The rules

1. **Short declarative sentences.** One idea per sentence.
2. **Plain words.** Use the common word: "write", "show", "use".
3. **Name the thing every time.** Write "`ATRIUS_IDLE_MINUTES` controls how long a host lives", and in
   the next sentence write `ATRIUS_IDLE_MINUTES` again. Do not write "the setting" or "it" when the
   reader could lose track.
4. **Say what is true and stop.** Do not add a contrast clause to make a sentence sound stronger.
5. **Explain a term the first time a page uses it**, or link it to [the glossary](glossary.md).
6. **Show, then explain.** Put a command, a query or a diagram near the top of a section.
7. **Every number has a source.** A number is printed by a program in this repository, or measured
   and recorded in [Performance](performance.md), [Testing on real code](testing.md) or
   [Lab findings](lab/findings.md), with the date of the run.
8. **Present tense, about the current release.** History belongs in `CHANGELOG.md`.

## What not to write

`cargo test -p atrius-cli --test docs` fails when a page holds one of these.

<!-- doc-style: off -->

| Do not write | Write instead |
|---|---|
| An em dash or an en dash | Two sentences |
| Words joined by a hyphen, like "read-only" or "built-in" | "read only", "built in" |
| "X, not Y" when Y is only there for contrast | "X" |
| "load bearing", "first class", "battle tested" | Say what the thing does |
| "worth noting", "to be clear", "honestly", "put simply" | Delete the announcement |
| "the whole point", "which is the point" | State the reason once |
| "the shape of", "surface", "vocabulary" | The concrete noun |
| "blows up", "takes down", "kills" for a failure | "the query fails with `not-found`" |
| An idiom, a metaphor or a saying | The fact it stands for |

<!-- doc-style: on -->

A hyphen is fine inside a name somebody else chose, such as B-tree or `atrius-mcp`, and inside
backticks.

## How a page is laid out

- A title that says what the page is about.
- Two or three sentences on what the page covers.
- A table of terms when the page uses more than three specialist ones.
- Sections, each starting with an example or a diagram.
- Mermaid diagrams of under about twelve boxes, with plain words in every box.
- Tables for anything the reader will look up.
- Every command can be copied and run as written.
