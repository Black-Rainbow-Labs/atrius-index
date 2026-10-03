# Lab findings: a code index against ripgrep

This page records the study that built the index Atrius is made of. It ran in September and October
2026 as tickets task-2138 (the design) and task-2139 (the build and the measurements), inside the
Unluminous editor, where the index shipped in Unluminous 0.58.0 before it became Atrius. It says what
was asked, how it was measured, what was found, which designs were weighed, every experiment tried,
and how the index compares with other tools.

The same study is published at [blackrainbowlabs.com/lab](https://blackrainbowlabs.com/lab). The
measurements on 19 open source repositories that Atrius added afterwards are in
[Testing on real code](../testing.md) and [Performance](../performance.md).

| Term | Meaning on this page |
|---|---|
| rg | the ripgrep inside Claude Code (`claude.exe`), run with the Grep tool's own flags |
| index arm, rg arm | an agent given the index's tool, or given Grep, Glob and Read |
| held out | the 30% of every query set never used to steer a change, used only for the final gate |
| family | one kind of query, F1 to F8 below |

## The headline

The index returns ripgrep's exact lines 32 to 140 times faster, stays correct through every kind of
change, and was the fastest Windows tool measured against ripgrep, git grep, ugrep and ugrep's own
index. It leaves an agent's accuracy and token cost where ripgrep leaves them.

| Measure | Result |
|---|---|
| Speed against ripgrep, held in the agent's MCP process | 32x to 40x on three local repositories, 140x on Linux, same lines on all 468 held out queries |
| Next fastest tool on Windows | ugrep: 1.4x to 2.3x |
| Zoekt, in WSL, against ripgrep in WSL | 5x to 51x, same lines on 93% to 99% of queries |
| Stale results after edits, renames, deletes, a 5,000 file burst and a branch switch | 0 |
| Agent success, Sonnet, 121 held out tasks, 3 draws | 0.689 against 0.678; the goal was 0.745 |
| Agent session tokens, index arm over rg arm | 0.95x; the goal was 0.50x |

## The question

Agents search code with ripgrep. The study asked whether a local index could be more effective, more
accurate and faster, and set three goals before the first measurement. The goals and the query sets
were frozen and hashed, and were not changed after any result.

| Goal | Definition | Bar |
|---|---|---|
| G1 speed | geometric mean of paired per query speed ratios on F1 to F4, the index over MCP against rg, both pinned to the performance cores | at least 6.0x, with the lower end of the 95% interval at least 6.0x, on the medium and the large repository |
| G2 tokens | geometric mean of paired per task ratios of whole agent session tokens, index arm over rg arm | at most 0.50x |
| G3 accuracy | agent success at least min(1.10 x rg, rg plus half its misses); no family worse than rg; every exact family equal to rg | 0.745 agent success on Sonnet |

## How it was measured

**Repositories**, at pinned commits: Unluminous (1,213 files), Inillucent (3,743), ai-service (6,633)
and the Linux kernel 6.16 (89,750).

**Query families**, frozen and hashed before the first run:

| Family | What it is | Queries | Held out | Graded by |
|---|---|---:|---:|---|
| F1 identifier | where is this name defined | 480 | 150 | definition at rank 1, against SCIP gold |
| F2 exact | grep patterns agents really ran, mined from 1,076 Claude Code transcripts | 1,200 | 348 | exact set equality with rg |
| F3 references | every use of a name | 416 | 120 | recall of uses, against SCIP gold |
| F4 files | a file by name or glob | 72 | 30 | file in the top 3 |
| F5 localisation | which files does this ticket change, from Tasks board tickets with their commits | 135 | 46 | file recall at 5 |
| F6 concept | a question about the code in plain English | 203 | 75 | nDCG at 10 |
| F8 unanswerable | a question the code cannot answer | 114 | 34 | correct abstention |

Family weights for the tool level metric came from 11,043 searches in the transcripts.

**Rules.** Every Windows timing was pinned to the performance cores, and a run was refused when
ripgrep's reference queries ran more than 3% slower than on a quiet machine. Speed is the geometric
mean of paired per query ratios with a 10,000 sample bootstrap interval. Agent runs used `claude -p`
with no settings or CLAUDE.md loaded and a fixed tool list: one arm with Grep, Glob and Read, the other
with the index's tool and Read. Answers were graded against gold, with a separate grader session for
the code questions.

## Results

### G1 speed: passes

Three held out gate runs, each from cold hosts so the index was loaded from disk, embedding off:

| Run | unluminous (small) | ai-service (medium) | inillucent (medium) | linux (large) |
|---|---|---|---|---|
| 1 | 27.76x (25.80 to 29.81) | 36.07x (32.36 to 40.43) | 29.07x (26.76 to 31.55) | 133.64x (96.96 to 181.52) |
| 2 | 26.72x (24.82 to 28.71) | 34.91x (31.29 to 39.09) | 28.62x (26.40 to 31.01) | 135.88x (98.09 to 185.24) |
| 3 | 26.72x (24.85 to 28.64) | 33.18x (29.87 to 36.99) | 29.00x (26.69 to 31.53) | 128.36x (93.18 to 174.37) |

Medians in the last run: rg 39.6, 80.7, 34.6 and 996.3 ms; the index 1.10, 1.21, 1.25 and 7.02 ms.

### The exact families, held out

| Repository | Family | n | rg | index |
|---|---|---:|---:|---:|
| ai-service | F1 definition at rank 1 | 48 | 0.208 | 0.500 |
| ai-service | F2 exact set of lines | 79 | 1.000 | 1.000 |
| ai-service | F3 recall of uses | 36 | 0.715 | 0.715 |
| ai-service | F4 file in the top 3 | 4 | 0.000 | 0.500 |
| inillucent | F1 | 50 | 0.360 | 0.780 |
| inillucent | F2 | 100 | 1.000 | 1.000 |
| inillucent | F3 | 44 | 1.000 | 1.000 |
| inillucent | F4 | 24 | 0.583 | 0.583 |
| linux | F2 | 75 | 1.000 | 1.000 |
| unluminous | F1 | 52 | 0.346 | 0.827 |
| unluminous | F2 | 94 | 1.000 | 1.000 |
| unluminous | F3 | 40 | 0.997 | 0.997 |
| unluminous | F4 | 2 | 0.500 | 0.500 |

The index returned exactly ripgrep's lines for every agent grep pattern on all four repositories, and
found a definition at the first place about twice as often as a whole word ripgrep search.

### F7 freshness

A scratch clone of each repository was changed and searched at once through both arms: a new file, an
edited line, a rename, a delete, a burst of 5,000 files, a branch switch, and five rounds of writing
and searching with no pause. Stale or missing results: **0** on all three local repositories, 11 steps
each.

### G2 tokens: not reachable by construction

| Model | Tasks x draws | rg median tokens | index median tokens | Paired ratio |
|---|---|---:|---:|---|
| Sonnet | 121 x 3 | 44,024 | 47,065 | 0.95 (0.88 to 1.03) |
| Opus | 121 x 1 | 66,556 | 58,501 | 0.98 (0.88 to 1.09) |
| Haiku (dev) | 40 x 2 | 577,684 | 529,496 | 0.77 (0.58 to 1.02) |

G2 measures the whole agent session. Sonnet and Opus finish these tasks in five to seven turns with
either tool, and each turn sends the same instructions and tool definitions again, about 5,200 tokens
at the first turn in both arms. Their Grep calls were already narrow: a Grep that lists files returned
60 to 850 characters. Search answers are a small share of a session, so no change to them can halve
its total. Haiku makes three times as many turns and saves about a quarter: three two draw runs gave
0.80, 0.75 and 0.77.

### G3 accuracy: not met

| Model | Arm | Success | F5 tickets | F6 questions |
|---|---|---:|---:|---:|
| Sonnet, 3 draws | rg | 0.678 | 41/138 | 205/225 |
| Sonnet, 3 draws | index | 0.689 | 42/138 | 208/225 |
| Opus, 1 draw | rg | 0.736 | 17/46 | 72/75 |
| Opus, 1 draw | index | 0.702 | 16/46 | 69/75 |

The index arm was within one or two sessions of rg in every family and repository, so no family was
worse, and it did not reach the bar. F6 is near the ceiling in both arms: Sonnet with ripgrep answers
91% of the questions. F5 counts a ticket solved only when half the files it changed are among the
first five the agent names, and 15 of the 46 held out tickets changed more than ten files, so 45 of
the 138 F5 sessions could not succeed in either arm.

**At tool level**, one plain English `find` was compared with every Grep call the Sonnet agent made
for the same task, replayed against the same folder:

| Repository | F5 index | F5 rg replay | F6 index | F6 rg replay | F8 index |
|---|---:|---:|---:|---:|---:|
| unluminous | 0.020 | 0.310 | 0.014 | 0.109 | 0.857 |
| ai-service | 0.101 | 0.029 | 0.084 | 0.000 | 0.500 |
| inillucent | 0.165 | 0.217 | 0.046 | 0.166 | 0.636 |

One plain English search written from a ticket or a question finds the right file less often than the
Grep calls an agent writes after reading the code, on two repositories of three. F8 has no rg arm:
ripgrep cannot say that the code holds no answer, and the index said so for 50% to 86% of those
questions.

## Compared with other tools

The 468 held out queries (agent grep patterns, plus names searched as whole words) were timed through
every tool on the same machine, and each tool's lines were compared with ripgrep's. Each query ran once
to warm up and three times timed, in an order that rotated from query to query.

| Repository | Tool | Median ms | Against rg | Same lines as rg | Build | Index size |
|---|---|---:|---|---:|---:|---:|
| unluminous | rg in Claude Code | 43.04 | 1x | 134/134 | | |
| unluminous | the index, in the agent's MCP process | 1.03 | 39.65x (37.1 to 42.1) | 134/134 | 1.0 s | 236 MB |
| unluminous | the index, through a separate host | 3.94 | 9.55x | 134/134 | | |
| unluminous | ugrep with its index | 17.60 | 1.70x | 117/134 | 0.4 s | 7 MB |
| unluminous | ugrep | 17.37 | 2.32x | 117/134 | | |
| unluminous | git grep | 32.20 | 1.33x | 133/134 | | |
| unluminous | Zoekt, WSL (against rg 4.22 ms in WSL) | 0.48 | 5.05x | 132/134 | 1.3 s | 43 MB |
| inillucent | rg in Claude Code | 42.93 | 1x | 144/144 | | |
| inillucent | the index, in the agent's MCP process | 1.12 | 38.63x (35.5 to 42.0) | 144/144 | 0.9 s | 276 MB |
| inillucent | the index, through a separate host | 3.78 | 10.14x | 144/144 | | |
| inillucent | ugrep with its index | 22.48 | 1.69x | 132/144 | 15.0 s | 12 MB |
| inillucent | ugrep | 19.32 | 2.19x | 132/144 | | |
| inillucent | git grep | 32.72 | 1.26x | 143/144 | | |
| inillucent | Zoekt, WSL (against rg 4.37 ms in WSL) | 0.51 | 5.70x | 139/144 | 1.9 s | 73 MB |
| ai-service | rg in Claude Code | 50.11 | 1x | 115/115 | | |
| ai-service | the index, in the agent's MCP process | 1.96 | 32.43x (29.9 to 35.1) | 115/115 | 4.7 s | 913 MB |
| ai-service | the index, through a separate host | 4.91 | 10.30x | 115/115 | | |
| ai-service | ugrep with its index | 22.22 | 1.86x | 111/115 | 48.5 s | 32 MB |
| ai-service | ugrep | 22.62 | 2.02x | 111/115 | | |
| ai-service | git grep | 38.28 | 1.22x | 114/115 | | |
| ai-service | Zoekt, WSL (against rg 5.83 ms in WSL) | 0.58 | 6.34x | 111/115 | 5.6 s | 248 MB |
| linux | rg in Claude Code | 999.84 | 1x | 75/75 | | |
| linux | the index, in the agent's MCP process | 6.11 | 139.78x (101.9 to 189.5) | 75/75 | 86.6 s | 1.44 GB |
| linux | the index, through a separate host | 75.87 | 14.92x | 75/75 | | |
| linux | ugrep with its index | 661.60 | 1.35x | 73/75 | 417.6 s | 404 MB |
| linux | ugrep | 918.04 | 1.08x | 73/75 | | |
| linux | git grep | 969.99 | 1.02x | 75/75 | | |
| linux | Zoekt, WSL (against rg 101.33 ms in WSL) | 1.06 | 50.73x | 70/75 | 90.2 s | 3.65 GB |

Zoekt does not build on Windows, so it was built from source and timed in WSL against ripgrep 14.1.1
there. Its absolute times are the fastest measured. ripgrep itself took 4 to 6 ms a query in WSL
against 43 to 50 ms inside Claude Code on Windows, so most of the index's lead on Windows is ripgrep's
Windows cost: a process start and file system calls on every search. Compare Zoekt's absolute times with
the index's, not its ratio.

The loopback socket costs about 3 ms a query, which is why the index through a separate host is about
four times slower than the index in the agent's own process. Atrius's MCP server hosts the checkout in
its own process unless another program already does.

## Published figures from other systems

None of these was reproduced here. "Vendor" marks a figure the system's maker published about itself.

| System | Claim | Figure | Source |
|---|---|---|---|
| Zoekt | size of the positional trigram index | about 1.2x the corpus | [Zoekt design](https://github.com/sourcegraph/zoekt/blob/main/doc/design.md) |
| Google Code Search | files a regex's trigram query narrows a search to, in Russ Cox's example | 36,972 to 25 | [Regular Expression Matching with a Trigram Index](https://swtch.com/~rsc/regexp/regexp4.html) |
| zilliz claude-context | tokens and tool calls against grep only Claude Code, 30 SWE-bench Verified instances (vendor) | 39.4% fewer tokens, 36.1% fewer tool calls | [zilliz blog](https://zilliz.com/blog/why-im-against-claude-codes-grep-only-retrieval-it-just-burns-too-many-tokens) |
| Cursor | internal accuracy metric with semantic search added to grep (vendor) | +12.5 points | [Cursor blog](https://cursor.com/blog/fast-regex-search) |
| Claude Code | local vector database against agentic search (vendor) | "agentic search generally works better" | [Boris Cherny](https://x.com/bcherny/status/2017824286489383315) |
| Anthropic Contextual Retrieval | fewer top 20 retrieval failures: embeddings, plus contextual BM25, plus a reranker (vendor) | 35%, 49%, 67% | [Anthropic](https://www.anthropic.com/news/contextual-retrieval) |
| cAST chunking | RepoEval Recall@5 from chunking at syntax boundaries | +1.8 to +4.3 points | [arXiv 2506.15655](https://arxiv.org/abs/2506.15655) |
| LocAgent | file level localisation Acc@5 on SWE-bench Lite with Claude 3.5 | 94.16% | [arXiv 2503.09089](https://arxiv.org/abs/2503.09089) |
| SweRank | retrieval alone, Acc@10 | 82.12% | [arXiv 2505.07849](https://arxiv.org/abs/2505.07849) |

## The approaches weighed

| Approach | For | Against | Decision |
|---|---|---|---|
| Keep ripgrep and only shape its output | no index to keep fresh | every call walks the tree and pays a process start, so it cannot be much faster; it cannot answer a question asked in English | rejected; output shaping was kept as part of the index |
| Vector search only, as Cursor and Copilot do | good at questions asked in English | cannot give exactly ripgrep's lines, and Claude Code's authors found agentic grep beat their vector retrieval | rejected; vectors are one of three engines, and optional |
| Trigram posting lists with the real regex run on the candidates (Google Code Search, Zoekt, IntelliJ) | any regex with exactly ripgrep's lines, from memory | an index to build and keep current; more memory than the text alone | chosen for the exact engine, verified with ripgrep's own grep-searcher |
| A trigram tokenizer inside Inillucent | its full text search could answer substrings itself | Inillucent 2.0.2 has none and no substring index | not in this work; recommended to Inillucent |
| Zoekt or another existing engine | mature, and the design this index follows | a separate server and another language runtime, and it does not build on Windows | rejected as the engine; measured as a comparison |
| tree-sitter for chunks and symbols | better spans for every language | a compiled grammar per language and platform | rejected; chunks come from keyword manifests and folding |
| A language server or SCIP at run time | references that are correct by type | a server per language, slow to start, and not every project builds | rejected at run time; SCIP made the gold answers |
| Every process opens the index file directly | no host process | Inillucent readers wait for a writer, and watchers in several processes repeat each other | rejected; one host per checkout, found through a lock file |
| The index stored inside the project | easy to find | shows in the working tree, is copied by worktrees, may be committed | rejected; stored in the user's cache folder |
| A code embedding model (CodeRankEmbed, jina code, nomic-embed-code) | trained on code | Inillucent's `embed()` serves nomic-embed-text-v1.5 only, and nomic-embed-code needs a GPU | out of reach; nomic-embed-text was measured against no vectors |
| A cross encoder reranker (gte-reranker-modernbert-base) | Contextual Retrieval reports it as the largest single gain | measured by Inillucent at 3 to 11 s a query on the processor | not measured; possible later only on a GPU, off by default |

## Every experiment

Rows marked kept are in Atrius today.

| Experiment | Setting | Measure | Result | Kept |
|---|---|---|---|---|
| Verifying candidates | whole files | a query with no hits, ai-service | 3.5 ms | |
| Verifying candidates | 64 KB blocks, searched in place | the same | 1.3 ms | kept |
| Regex to trigram planner | text joined across a wildcard (a fault) | dev F2 patterns with ripgrep's lines | 898 of 900 | |
| Regex to trigram planner | fixed | the same | 899 of 900; the last is a pattern ripgrep refuses too | kept |
| Uses of a name (F3) | definition lines left out | recall of uses against rg | below rg on all three repositories | |
| Uses of a name (F3) | definition lines kept, listed first | the same | equal to rg | kept |
| Compaction | saved while holding the index lock | longest query wait after a 5,000 file burst | 19.1 s | |
| Compaction | rebuilt off the lock from a copy | the same | 69 ms | kept |
| Writing vectors | two commits a chunk | chunks embedded a second per host, GPU use | about 4, GPU 5% | |
| Writing vectors | the model run with no transaction open, one transaction a batch | the same | 22 to 37, GPU 66% | kept |
| Abstention cutoff (dev, with vectors) | none | F5 / F6 / F8 | 0.105 / 0.150 / 0.000 | |
| Abstention cutoff | Inillucent `confidence()` below 0.349 | the same | 0.003 / 0.102 / 0.804: every ticket turned away | |
| Abstention cutoff | word coverage below 0.294 | the same | 0.095 / 0.100 / 0.571 | |
| Abstention cutoff | word coverage below 0.340 | the same | 0.086 / 0.091 / 0.625 | |
| Abstention cutoff | word coverage below 0.31, code ranked first | F6 / F8 | 0.116 / 0.554 | kept |
| Fusion of word and vector rankings (dev) | weighted, vector weight 0.5 | weighted metric, 0.10 F6 + 0.05 F8 | 0.0385 | kept |
| Fusion | weighted, vector weight 0.7 | the same | 0.0387, within one query | |
| Fusion | weighted, vector weight 0.3 | the same | 0.0355 | |
| Fusion | rrf | the same | 0.0362 | |
| Fusion | adaptive | the same | 0.0352 | |
| Ranking for questions in English (dev) | code and documents mixed | F6, unluminous / inillucent | 0.150 / 0.157 | |
| Ranking for questions in English | filtered on the kind facet inside the hybrid search | the same | 0.040 / 0.000 | |
| Ranking for questions in English | one search, documents moved after code | the same | 0.215 / 0.183 | kept |
| What an answer carries (Haiku, dev) | lines only | token ratio against rg, one draw | 0.79 (0.49 to 1.20) | |
| What an answer carries | small answers carry the code around the hit | the same | 0.55 (0.33 to 0.89) | kept |
| What an answer carries | plus a short `files` answer carries an outline | the same | 0.52 (0.31 to 0.83) | kept |
| Draws a task (Haiku, dev) | two draws, three builds | token ratio against rg | 0.80, 0.75 and 0.77: the one draw figures were lucky | kept |
| Model (dev, one draw) | Haiku | token ratio / median turns | 0.77 / 16 | |
| Model (dev, one draw) | Sonnet | the same | 0.95 / 4 to 7 | kept for the gate |

## What it means

As a search engine the index does what it was built to do: exactly ripgrep's lines, 27 to 140 times
faster, fresh after every kind of change, with a definition found at the first place about twice as
often as a whole word ripgrep search finds it there. As a tool for an agent it is as accurate as
ripgrep and costs about the same: a capable model already uses ripgrep well enough that the cost of a
session is mostly its instructions.

## Caveats

- The token and accuracy goals were frozen before the first run and were not changed after it.
- Embeddings were made on a GPU for the evaluation. The product embeds on the processor and stops above
  100,000 chunks.
- The F5 grading counts a ticket solved only when half its changed files are among the first five
  named, which no agent can do for a ticket that changed more than ten files.
- Zoekt's ratio is to ripgrep in WSL and the other tools' ratios are to ripgrep in Claude Code on
  Windows. The two ripgreps differ by about ten times.
- ugrep was asked with `-P` and ugrep with its index in ugrep's own syntax, because ugrep refuses `-P`
  together with `--index`. A first run that passed `-P` to the index mode returned nothing and was
  discarded.

## Recommendations

These are recommendations; none of them changed a default.

1. Use the index wherever an agent searches code on Windows. It gives ripgrep's answers 32x to 140x
   faster when held in the agent's MCP process, and nothing else measured on Windows was within a
   factor of ten of it.
2. Keep the index in the agent's own process when nothing else hosts the checkout. Look at a cheaper
   path than a loopback socket when another program hosts it, since the socket costs about 3 ms of a
   1 ms answer.
3. Do not expect the index to cut an agent's tokens or raise its accuracy on its own. Those depend on
   how many turns the agent takes, which the search tool barely changes for Sonnet and Opus.
4. The weakest part is a single search in plain English from a ticket's words. Improving it is the
   next piece of work, followed by a trigram tokenizer in Inillucent so the exact index can live in the
   database.
5. Look at the passage table's size. On ai-service it makes the index file four times the size of the
   code, and exact search does not need it.

## What Atrius changed after the study

Atrius is the same engine, moved into its own repository. These changes came after the study, and none
of them touches the exact engine's results:

- Twelve more languages: C, C++, C#, Go, Java, Kotlin, Lua, PHP, Ruby, Scala, Swift and Zig, with two
  new manifest keys for functions whose brace is on the next line or follows qualifiers.
- Folders such as `examples/`, `playground/` and `docs/` rank below the source for `def`.
- `find` for one name keeps its definitions inside `--path`, `--glob` and `--type`.
- Verbs other than `find` wait up to 30 seconds for an index that is still being built, instead of
  refusing.
- A test suite on 19 open source repositories in 13 languages: every one of 2,200 generated queries
  equals ripgrep 15.2, and 54 of 54 hand checked definitions are found. [Testing on real code](../testing.md).
