# Atrius: notes for an AI agent

Read this first when you use or change this repository.

## Using Atrius to search this repository

Register the server once (`claude mcp add atrius -- atrius-mcp`) and use the `atrius_search` tool in
place of Grep, Glob and reading whole files. `docs/mcp.md` describes it.

## Changing it

| Where | What |
|---|---|
| `crates/atrius-index/src/` | The engine. `verbs.rs` answers every query; `host.rs` owns the process model |
| `crates/atrius-index/languages/` | Language manifests; `docs/languages.md` is the format |
| `crates/atrius-cli/src/catalogue.rs` | Every command. Change it, then run `atrius help --markdown > docs/commands.md` |
| `docs/` | Reference pages in the style `docs/writing-style.md` sets. Every new page goes in `docs/README.md` |
| `packaging/` | The release. Run `pwsh packaging/ship.ps1`, never the scripts under it by hand |

## Rules that hold everywhere

- **The exact engine returns ripgrep's lines.** A change that makes any corpus query differ from
  ripgrep is a fault, not a trade. Run the corpus test (`docs/testing.md`) before and after.
- **Nothing is downloaded at run time.** Embeddings are used only when a model is already installed.
- **The host protocol and the index folder layout are shared with Unluminous.** A change to either
  must keep `search.<verb>` requests working and must be released in both.
- **Build on Windows with `pwsh tools/cargo.ps1`**, which sets the MSVC environment a C dependency
  needs.
- **Tests never touch a person's own index or watch list.** Set `ATRIUS_INDEX_CACHE` and
  `ATRIUS_CONFIG` to scratch folders, as the existing tests do.
