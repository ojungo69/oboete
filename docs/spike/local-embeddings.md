# Spike: local bge-m3 (milestone 4, Task 10)

Throwaway (spec 8.3). Milestone 4's Task 10 (docs/milestone-4-plan.md): can bge-m3 run inside oboete's own processes on the owner's machines, agreeing with the Workers AI vectors it would replace, within the memory and time a resident worker and an MCP server may take? This note fixes the lines and how each is measured before any run (Step 1); the results come in Step 3 (Step 2 runs them). PR-A2 measured a first version on this PC and the M1 iMac on 2026-09-24 (docs/pr-a.md, A2): its agreement checks passed on both, a query took 1.79 GB at peak and an 8,000-character text 2.79 GB, and fastembed grew the binary from 10.7 to 36.1 MB.

## What runs

- **The runtime**: fastembed 7.1.1, with ONNX Runtime 1.28.0 (ort 2.0.0-rc.13) linked into the binary (`ort-download-binaries-rustls-tls`), behind the cargo feature `local-embed`. The model is loaded from files oboete fetched and checked itself (`model_fetch`), never through hf-hub: fastembed's user-defined model, given its graph as bytes and its weights' folder by name (`session.model_external_initializers_file_folder_path`).
- **The model**: BAAI/bge-m3 (MIT) at commit `5617a9f61b028005a4858fdac845db406aefb181` of huggingface.co/BAAI/bge-m3, run at 8,192 tokens, one text at a time. The graph is oboete's, built into the binary: `assets/bge-m3-mha.onnx` (474,829 bytes, SHA-256 `395d177d56b5eb75c0cdd0b87bf9b939652e0a424b3257ba31c8b7fe848e1654`) is BAAI's `onnx/model.onnx` with its 24 attention blocks fused into ONNX Runtime's `MultiHeadAttention` with no mask, which runs as FlashAttention on the CPU (`docs/spike/local-embeddings/fuse.py`, which makes it again byte for byte; NOTICE carries BGE-M3's licence). BAAI's own graph builds each layer's whole attention matrices and fails line C (Results). From BAAI the model needs five files, 2,283,920,811 bytes, each checked against its size and SHA-256 (`src/embed_local.rs`, `FILES`): `config.json` (687, `26159e7a…`), `special_tokens_map.json` (964, `8c785abe…`), `tokenizer_config.json` (444, `a62b2b67…`), `tokenizer.json` (17,098,108, `21106b6d…`) and `onnx/model.onnx_data` (2,266,820,608, `1eebfb28…`). A change to the graph, a file or the commit is a new embedder id. The int8 export is not a candidate: it failed PR-A2's agreement check (cos 0.846 at worst, docs/pr-a.md, A2 item 5).
- **Machines**: this PC's WSL (x86_64, 32 threads), the same pinned to 4 threads as the stand-in for the slowest WSL machine (What needs the owner, item 5), the M1 iMac (8 GB) over SSH, and Windows on this PC with the MSVC build. `.github/workflows/local-embed.yml` builds the branch it is run on (`gh workflow run local-embed.yml --ref <branch>`) on the five release targets (spec 7.1: Linux x64 and arm64, macOS arm64 and x64, Windows x64), starts each binary alone and runs the agreement test on the public fixture.

## Lines

As the plan sets them (Task 10, Step 1), fixed here before any run.

- **A. Agreement** (D8), per owner machine, over at least 1,000 stored evaluation vectors stratified by length, language and kind, with the 312 dev questions: cos at least 0.99; every question's top 10 over the sample overlaps in 9, local against Workers AI and local against local (`a2_compare.py`). Only aggregates leave the machine. This run reads the evaluation's vectors and questions, so it is part of the one evaluation after completion (owner decision 34, A113); the build does not wait for it. Before it, each target checks the same two numbers on the public fixture (`src/testdata/embed-agreement/`: 200 public texts and 20 queries with their Workers AI vectors).
- **B. Speed**: a warm query's embedding, p95 at most 500 ms on the slowest machine.
- **C. Memory on the iMac** (peak RSS): the worker 3.5 GB on a 12,000-character text; a reader 2.0 GB; the worker and two readers together 7.0 GB.
- **D. Hooks**: a hook's spawn p95 at 1 KB and 64 KB rises at most 2 ms over the same machine's build without the feature, runs alternated in one session (the iMac's two runs differed by 1.6 ms), on WSL, the iMac and Windows MSVC.
- **F. Builds** on the five targets; on Windows the binary starts with no extra DLL.
- **G. No connection** with the files present (`unshare -n`).
- **H. Downloads**: `Range` holds through the host's redirect; downloads killed at 30% and at 70% resume.
- **Reported, no line**: load time, speed, and a fresh process's load-plus-embed p95.

The plan's rule: failing A, F, G or the worker's C, that target gets no `local`. Failing B or a reader's C, that machine is not offered `local`. Failing the combined C, the iMac holds one reader's model at a time (MCP or the viewer, whichever starts first), or, if that also fails, is not offered `local`. Failing D, Task 10 stops at this note. Failing H, `model_fetch` is not offered on that host path, and `local` there needs the model placed by hand, which doctor names.

## How each is measured

- **B**: one process loads the model once, then embeds each of the 312 dev questions in turn; the p95 of the embeddings alone, on each machine and on WSL pinned to 4 threads (`taskset -c 0-3`, the runtime's threads set to 4). The questions stay on the machine; only the times are written here.
- **C**: the peak resident set (`VmHWM` on Linux, `/usr/bin/time -l` on macOS) of a worker that embeds one 12,000-character text, of a reader that embeds one query, and of the worker and two readers started together. For the three together, the sum of each process's own peak is an upper bound on what they hold at once (their peaks need not coincide, and pages of the same file count in each): it settles a pass; a sum over the bound needs their resident sets sampled together before it is called a fail.
- **D**: milestone 2's harness, `oboete --home <tmp> replay <events-1000.jsonl> --spawn-sample 300 --sizes 1,64` (docs/milestone-2.md), with release builds of one commit with the feature and without it, five runs of each alternated in one session, on each of the three machines. Windows runs the MSVC pair that `local-embed.yml` keeps as its run's artifact (this PC has no MSVC linker).
- **F**: `local-embed.yml` on the five targets. Each binary is copied alone into an empty directory and started with only the system's directories on the PATH.
- **G**: on WSL, the model's files in place, `unshare -n` (no network at all) around a load and an embedding: they must succeed, and the process must open no socket (`strace -f -e trace=network` shows none).
- **H**: `model_fetch` against huggingface.co, which answers the file's URL with a redirect to its storage: a `Range` request after the redirect returns 206 with the requested bytes; a download killed at 30% and one killed at 70% resume where they stopped and end with the pinned SHA-256.

## Results (Step 2, 2026-10-09)

GB is 10^9 bytes. WSL is this PC (AMD Ryzen 9 5950X, 16 cores and 32 threads, WSL given 22 GB); the iMac is the M1 with 8 GB. "BAAI's graph" is the export as published; "the fused graph" is the one oboete loads.

- **A** (the public fixture, WSL): over its 220 items (200 texts of up to 3,138 tokens, 20 queries), cos at least 0.99996 (`ja-20-24`) and every query's top 10 overlapping in at least 9 (`q-ja-12`), with BAAI's graph and with the fused graph alike; the two graphs' vectors for one 6,706-token text agree to cos 1.0. The five targets: pending (`local-embed.yml` on this branch). The evaluation's run is part of the one evaluation after completion.
- **B** (312 dev questions, the embeddings' p95 / p50 in ms):

  | | BAAI's graph | The fused graph |
  |---|---|---|
  | WSL, 32 threads | 165.4 / 113.4 | 143.1 / 109.3 |
  | WSL, 8 threads | | 108.4 / 54.7 |
  | WSL pinned to 4 threads (the slowest machine's stand-in) | 142.7 / 56.6 | 133.5 / 53.2 |
  | iMac, its 8 threads | 134.8 / 51.2 | 126.8 / 47.7 |
  | iMac, 4 threads | 181.7 / 57.0 | 168.3 / 53.2 |

  Passes: the slowest p95 is 181.7 ms.
- **C** (the iMac; peak RSS from `/usr/bin/time -l`, its peak memory footprint in brackets):
  - BAAI's graph **fails the worker's line**. A reader took 1.89 GB [1.83] for one query and 1.99 GB [1.84] over the 312 questions. The worker took 4.24 GB [3.93] and 14.0 s on 12,000 characters of English (4,089 tokens); on 12,000 characters of Japanese (6,706 tokens) it swapped (swap in use 218 to 2,116 MiB) and was stopped after 168 s at a footprint of 7.66 GB. WSL shows why: the peak grows with the square of the text's tokens n, about 1.84 GB + 136 bytes x n^2 (2.00 GB at 1,024 tokens, 2.42 at 2,048, 3.18 at 3,072, 4.12 at 4,096, 7.92 at 6,706, about 11 at 8,192), each layer's attention matrices of 16 heads x n^2 x 4 bytes, about two held at once.
  - The fused graph **passes**. A reader took 1.97 GB [1.83] for one query and 1.98 GB [1.83] over the 312 questions, within 20 MB of its line. The worker took 2.42 GB [2.10] and 21.2 s at 6,706 tokens, and 2.53 GB [2.20] and 32.1 s at 8,192, the most a text can have. The 8,192-token worker and two readers started together took 2.28 + 1.68 + 1.67 = 5.62 GB [5.87], each process's own peak (swap in use 938 to 1,118 MiB). On WSL a reader took 1.99 GB, the worker 2.33 GB at 6,706 tokens (6.7 s) and 2.45 GB at 8,192 (9.0 s).
  - A reader's resident set grew from one question to 312 while its footprint stayed at 1.83 GB, so a reader that lives long (MCP's server) may pass 2.0 GB resident with no more memory in use; the implementation watches it.
- **D** (hook spawn p95 in ms over 300 spawns, the median of five runs of each build alternated, without the feature to with it):

  | | 1 KB | 64 KB |
  |---|---|---|
  | WSL | 11.8 to 13.2 (+1.4) | 13.3 to 15.3 (+2.0) |
  | iMac | 24.9 to 24.1 (-0.8) | 23.1 to 24.2 (+1.1) |
  | Windows MSVC | pending | pending |

  WSL passes on the line itself. Every process pays it, embedding or not: `oboete --version` starts in 1.40 ms without the feature and 2.35 ms with it (p50 of 400 alternated starts), as the feature build loads `libstdc++.so.6` and runs 18 more static initializers (ONNX Runtime's), and its dynamic loader takes 424,000 cycles instead of 230,000. A first WSL session, run while test suites shared the CPU, gave +3.0 to +3.4 ms in its first two runs and is not counted.
- **F**: pending (`local-embed.yml` on this branch). The Windows binary's imported DLLs are also compared with the build's without the feature: GitHub's runners have the Visual C++ runtime installed, so a start there cannot show what a machine without it needs.
- **G** (WSL): inside `unshare -rn` (a network namespace whose only device, loopback, is down), `--verify`, a load and an embedding succeeded (a vector of 1,024 values), and `strace -f -e trace=network` over the whole run recorded no network call.
- **H** (WSL, curl `-L -C -` standing in for `model_fetch`): huggingface.co answered each file's URL with a redirect to its storage. The 2.27 GB file, killed at 30% (686,206,976 bytes) and again at 70% (1,590,059,008 bytes), resumed both times through the redirect and ended at 2,266,820,608 bytes with the pinned SHA-256, as every other file did.
- **Reported**: loading takes 1.8-2.5 s on WSL and 1.1-2.2 s on the iMac, so a fresh process's load and one query take about 1.9-2.7 s and 1.1-2.3 s. Typical documents (1,000 drawn at random from what the evaluation home's embedding phase sends, the fused graph): 0.33 s each on WSL at 8 threads (p50 245 ms, p95 559 ms; 16 and 32 threads were no faster) and 0.62 s on the iMac over 300 of them (p50 456 ms, p95 1,338 ms): that home's 201,261 texts in about 18 hours on WSL and 35 on the iMac, one process each. The binary grows from 19.8 to 45.2 MB on Linux x64, and from 16.2 to 36.2 MB on macOS arm64 before the graph's 0.47 MB.

Also found:

- ONNX Runtime refuses weights reached through a symbolic link out of the model's folder (`External data path escapes model directory`). PR-A2's copy on the iMac, hf-hub's cache of links into `blobs/`, loaded only once its files were hard-linked into a folder of their own: `model_fetch` writes plain files.
- The fused graph has no attention mask, so the padding a batch adds to its shorter texts would change their vectors: one text per run. Texts of exactly the same token count need no padding; a backfill could batch those (not built).
- Python's onnx package, used only by `fuse.py`, refuses weights whose file has more than one hard link.

## After the runs

Step 3, from the results above:

- **The runtime**: fastembed 7.1.1 with ONNX Runtime 1.28.0 linked in, the fused graph built into the binary, and BAAI's five files fetched and checked by oboete. One text at a time; at most 8 threads (B: WSL's p95 108 ms at 8 against 143 at 32 and 134 at 4; the iMac has 8).
- **The owner machines that get `local`**: on this PC's WSL and on the iMac, lines A (the fixture), B, C, G and H pass and D passes; F and Windows's D come from `local-embed.yml` on this branch, and the evaluation's A from the one evaluation after completion. The slowest WSL machine's 4-thread stand-in passes B; its memory is not known (What needs the owner, item 5). Which machines use `local` stays the owner's choice (What needs the owner, item 2), now with measured numbers: about 2.5 GB for the worker and 2.0 GB per searching process, the evaluation home's whole store in about 18 hours on WSL and 35 on the iMac, and no money.
- **How many long texts one batch may hold**: one text per run, for any length (the fused graph has no mask).
- **`CLI_EMBEDS_QUERIES`**: false. A fresh process needs 1.1-2.5 s to load the model, past MCP's 1.5 s on WSL, so only a process that stays loaded (MCP's server) embeds queries.
- **The spike's downloads** stay until the owner chooses (What needs the owner, items 2 and 6): this PC's (2.3 GB) and the iMac's (PR-A2's cache, with the spike's hard links to it, which take no more space).
