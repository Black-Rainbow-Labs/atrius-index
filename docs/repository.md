# Repository and building

This page lists the crates and folders in this repository, how to build Atrius, how to run its tests,
and how a release is made.

## Layout

| Path | What it holds |
|---|---|
| `crates/atrius-index/` | The library: engines, languages, host, protocol and query verbs. Published to crates.io |
| `crates/atrius-index/languages/` | The 24 language manifests compiled into the library |
| `crates/atrius-cli/` | The `atrius` and `atrius-mcp` programs, the command table, the MCP server, the service and `doctor`. Published to crates.io |
| `tests/corpora.json`, `tests/fetch-corpora.mjs` | The real world repositories the corpus test searches, and the script that fetches them |
| `docs/` | These pages |
| `packaging/` | The release: `ship.ps1`, the install scripts, the Homebrew formula, the Linux package template and the macOS installer |
| `tools/cargo.ps1` | cargo with the MSVC compiler's environment, on Windows |

## Building

```sh
cargo build --release                 # macOS and Linux
pwsh tools/cargo.ps1 build --release  # Windows
```

The programs are `target/release/atrius` and `target/release/atrius-mcp`. The toolchain is pinned in
`rust-toolchain.toml`, and the oldest Rust that builds the crates is 1.85.

On Windows, `inillucent-driver`'s `embed` feature compiles C code with `cl.exe`, which needs the
environment `vcvars64.bat` sets. `tools/cargo.ps1` finds Visual Studio with vswhere and sets it when it
is missing, so the build runs from any PowerShell.

## Testing

```sh
pwsh tools/cargo.ps1 test --workspace
node tests/fetch-corpora.mjs
ATRIUS_REQUIRE_CORPORA=1 pwsh tools/cargo.ps1 test --release -p atrius-index --test corpora -- --ignored --nocapture
```

[Testing on real code](testing.md) describes every test and the latest results. The parity tests need
ripgrep: on the path as `rg`, or named by `ATRIUS_RG`.

After changing the command table, write the reference page again:

```sh
atrius help --markdown > docs/commands.md
```

## Releasing

```powershell
pwsh packaging/ship.ps1              # a patch release
pwsh packaging/ship.ps1 -Part minor
pwsh packaging/ship.ps1 -WhatIf      # print the plan and change nothing
```

`ship.ps1` works in five phases, the same ones Inillucent's release uses:

| Phase | What it does |
|---|---|
| preflight | reads every credential and tool, and prints which routes will run and why any will not |
| version | writes the new version into every file that carries it |
| build | Windows natively, Linux and macOS with cargo-zigbuild; a universal macOS binary signed with the Developer ID certificate; `.deb` and `.rpm`; a signed and notarized `.pkg`; `SHA256SUMS` signed with minisign |
| publish | tags and pushes, creates the GitHub release, copies the downloads to blackrainbowlabs.com and deploys it, publishes the crates, and pushes the Homebrew formula |
| report | asks each destination what it serves |

A route whose credential or tool is missing is skipped with the reason. `packaging/README.md` lists the
routes, the credentials each one needs, and where they are kept.
