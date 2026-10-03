# Glossary

Each term the Atrius pages use, in one or two sentences. A page links here the first time it uses one.

| Term | Meaning |
|---|---|
| Checkout | The folder being searched, normally a git working tree. Atrius keeps one index per checkout |
| Root | The checkout's top folder. Without `--root`, Atrius uses the nearest folder at or above the working folder that holds `.git` |
| Index | What Atrius keeps about a checkout so it can answer without reading every file: the text, its trigrams, the definitions and the chunks |
| Host | The one process that owns a checkout's index, runs its file watcher and answers queries. Every other caller asks the host |
| Host file | `host.conf` in the index folder. It names the host's process, port and token |
| Token | 32 hexadecimal characters in the host file. A request without it is refused |
| Trigram | Three consecutive bytes of text. `retry` holds the trigrams `ret`, `etr` and `try` |
| Posting list | For one trigram, the list of places in the checkout that hold it |
| Candidate | A file region whose trigrams could match a query. The real regex then runs on it |
| Verification | Running the real regex over the candidates, with the same code ripgrep uses, so the answer is exactly ripgrep's |
| Parity | Giving the same set of `(file, line)` pairs as ripgrep for the same query |
| Gate | The check before every query: any file in the query's scope that changed since the index last saw it is read again first |
| Watcher | The operating system's file change notifications: ReadDirectoryChangesW on Windows, FSEvents on macOS, inotify on Linux |
| Definition | A place a name is introduced: a function, a type, a constant, a module or a variable |
| Grammar | What Atrius knows about a language: its comments, strings, keywords and which keywords introduce a definition. Read from a manifest |
| Manifest | A `.conf` file of `name = value` lines that describes one language |
| Fragment | The function or section around a line, with the signatures that enclose it |
| Outline | A file's definitions, one line each, with the lines each one spans |
| Chunk | A piece of a file, usually one definition, that plain English search ranks |
| Passage table | The `inillucent_search` table that holds the chunks, ranked by words and optionally by vectors |
| Embedding, vector | A list of numbers that stands for a chunk's meaning, made by an embedding model. Optional |
| Token budget | How much text an answer may hold, counted in model tokens. 1,500 by default |
| MCP | Model Context Protocol: how an AI agent calls tools a separate program offers |
| Corpus | A real repository the tests search, pinned to a release tag |
| Inillucent | The embedded database the index file is kept in. It speaks SQLite's dialect and has full text and vector search built in |
