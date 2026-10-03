# Configuration

Atrius has no configuration file of its own. It reads environment variables, and keeps two folders:
the cache folder, which holds the indexes, and the configuration folder, which holds the watch list
of the [background service](service.md). This page lists both folders and every variable.

## Where things are

| What | Windows | macOS | Linux |
|---|---|---|---|
| Indexes | `%LOCALAPPDATA%\atrius-index\index\<hash>\` | `~/Library/Caches/atrius-index/index/<hash>/` | `$XDG_CACHE_HOME/atrius-index/index/<hash>/`, else `~/.cache/...` |
| Embedding cache | `%LOCALAPPDATA%\atrius-index\embeddings\` | `~/Library/Caches/atrius-index/embeddings/` | `~/.cache/atrius-index/embeddings/` |
| Watch list, daemon id | `%APPDATA%\atrius-index\` | `~/Library/Application Support/atrius-index/` | `$XDG_CONFIG_HOME/atrius-index/`, else `~/.config/atrius-index/` |
| Programs, from `install.ps1` or `install.sh` | `%LOCALAPPDATA%\Programs\atrius-index\bin` | `~/.local/share/atrius-index/bin` | `~/.local/share/atrius-index/bin` |

`<hash>` is the first 24 hexadecimal characters of the blake3 hash of the checkout's canonical path in
lower case, so a checkout keeps its index when it is opened by another program or another spelling of
its path. Each index folder holds:

| File | What it is |
|---|---|
| `index.rdb` | The Inillucent file: file records, trigram lists, definitions and the passage table |
| `host.lock` | Held open by the host for its whole life. A second host cannot take it |
| `host.conf` | The host's process id, port, token, checkout and program. Removed when the host stops |

Deleting an index folder while no host runs is safe. The next query builds it again.

## Environment variables

### Where and what

| Variable | Default | Effect |
|---|---|---|
| `ATRIUS_INDEX_CACHE` | the cache folder above | Where the indexes are kept. The tests set it to a scratch folder |
| `ATRIUS_CONFIG` | the configuration folder above | Where the watch list and the daemon's id are kept |
| `ATRIUS_ROOT` | unset | The checkout `atrius-mcp` answers for when it is started with no `--root` |
| `CLAUDE_PROJECT_DIR` | set by Claude Code | Read by `atrius-mcp` after `ATRIUS_ROOT`, so a server registered at user scope searches the project Claude Code opened |
| `ATRIUS_LANGUAGES` | unset | A folder of extra language manifests, read at start. [Languages](languages.md) explains the format |

### The host

| Variable | Default | Effect |
|---|---|---|
| `ATRIUS_IDLE_MINUTES` | `120` | How long a host started by a one off command lives with no query. `0` keeps it until `atrius stop` |
| `ATRIUS_READY_WAIT_SECONDS` | `30` | How long `def`, `refs`, `outline`, `fragment` and `files` wait for an index that is still being built before refusing |

### Plain English search

| Variable | Default | Effect |
|---|---|---|
| `ATRIUS_EMBED` | on | `off` stops a host making vectors. Searches then rank chunks by words alone |
| `INILLUCENT_EMBED_DEVICE` | the processor | A card such as `cuda:0` to make vectors on. Without one, a checkout over 100,000 chunks is not embedded, because on a processor that takes days |
| `ATRIUS_EMBED_BATCH` | `4` | How many chunks are embedded in one transaction |
| `ATRIUS_SEARCH_ABSTAIN` | `0.31` | The word coverage below which a plain English search answers that nothing matches |
| `ATRIUS_SEARCH_CONFIDENCE` | `0.0` | A floor on Inillucent's `confidence()` for the same purpose. Off by default, because on code it did not separate questions with an answer from questions without one |
| `ATRIUS_PASSAGE_FUSION` | `weighted`, vector weight 0.5 | How word and vector rankings are combined: `weighted`, `rrf` or `adaptive` |

The values for the last three were chosen by measurement. [Lab findings](lab/findings.md) has the
experiments.

### The service, for tests

| Variable | Effect |
|---|---|
| `ATRIUS_SERVICE_NAME` | The Windows Run value's name, `AtriusIndex` by default |
| `ATRIUS_SERVICE_DRY` | `1` writes the login item files without asking launchctl or systemctl |
| `ATRIUS_SERVICE_SYSTEMD` | With `ATRIUS_SERVICE_DRY=1`, `1` writes a systemd unit instead of an autostart entry |

### The tests

| Variable | Effect |
|---|---|
| `ATRIUS_RG` | The ripgrep the parity tests compare with |
| `ATRIUS_CORPORA` | Where the test corpora are |
| `ATRIUS_CORPUS` | A comma list of corpora to test |
| `ATRIUS_REQUIRE_CORPORA` | `1` fails the corpus test for a corpus that has not been fetched |
| `ATRIUS_QUERIES` | Queries per corpus, 120 by default |
| `ATRIUS_REPORT` | Where the corpus test writes its JSON report |
