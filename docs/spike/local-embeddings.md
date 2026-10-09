# Spike: local bge-m3 (milestone 4, Task 10)

Throwaway (spec 8.3). Milestone 4's Task 10 (docs/milestone-4-plan.md): can bge-m3 run inside oboete's own processes on the owner's machines, agreeing with the Workers AI vectors it would replace, within the memory and time a resident worker and an MCP server may take? This note fixes the lines and how each is measured before any run (Step 1); the results come in Step 3 (Step 2 runs them). PR-A2 measured a first version on this PC and the M1 iMac on 2026-09-24 (docs/pr-a.md, A2): its agreement checks passed on both, a query took 1.79 GB at peak and an 8,000-character text 2.79 GB, and fastembed grew the binary from 10.7 to 36.1 MB.

## What runs

- **The runtime**: fastembed 7.1.1 behind the cargo feature `local-embed`, with ONNX Runtime 1.28.0 (ort 2.0.0-rc.13), measured in two shapes. First it was linked into the binary (`ort-download-binaries-rustls-tls`, pyke's build); after that failed lines D and F, it was loaded at run time (`ort-load-dynamic`): the binary links no ONNX Runtime, and only a process that embeds loads Microsoft's released library for its target from the model's folder (`onnxruntime/`), once it has checked the file against the size and SHA-256 pinned in the binary (`src/embed_local.rs`, `RUNTIME`). Step 3 chose the second. Either way the model is loaded from files oboete fetched and checked itself (`model_fetch`), never through hf-hub: fastembed's user-defined model, given its graph as bytes and its weights' folder by name (`session.model_external_initializers_file_folder_path`).
- **The runtime's library** (the second shape), from the archives of Microsoft's release v1.28.0 on github.com/microsoft/onnxruntime, each archive pinned too (`.github/workflows/local-embed.yml`):

  | Target | File | Bytes | SHA-256 |
  |---|---|---|---|
  | Linux x64 | `libonnxruntime.so.1.28.0` | 24,268,848 | `1461ef7c…` |
  | Linux arm64 | `libonnxruntime.so.1.28.0` | 20,591,712 | `f1ec1a08…` |
  | macOS arm64 | `libonnxruntime.1.28.0.dylib` | 39,312,136 | `dc19bbcb…` |
  | Windows x64 | `onnxruntime.dll` | 15,809,848 | `18370c37…` |

  Microsoft publishes no 1.28.0 library for macOS x64.
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

## Results with the runtime linked in (Step 2, 2026-10-09)

GB is 10^9 bytes. WSL is this PC (AMD Ryzen 9 5950X, 16 cores and 32 threads, WSL given 22 GB); the iMac is the M1 with 8 GB. "BAAI's graph" is the export as published; "the fused graph" is the one oboete loads.

- **A** (the public fixture): over its 220 items (200 texts of up to 3,138 tokens, 20 queries), cos at least 0.99996 (`ja-20-24`) and every query's top 10 overlapping in at least 9 (`q-ja-12`), the same on WSL and on the iMac with the fused graph (and on WSL with BAAI's), and on the four targets that built in `local-embed.yml` (run 37930389936: macOS arm64, Linux x64 and arm64, Windows x64). The two graphs' vectors for one 6,706-token text agree to cos 1.0. The evaluation's run is part of the one evaluation after completion.
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
  | Windows MSVC | 27.3 to 34.8 (+7.5) | 28.4 to 35.8 (+7.4) |

  **Windows fails the line**, measured on this PC's Windows (the same Ryzen 9 5950X), not on a runner, with the two MSVC builds of `local-embed.yml`'s run: the feature build was the slower in every run but one. WSL passes on the line itself. Every process pays it, embedding or not: `oboete --version` starts in 1.40 ms without the feature and 2.35 ms with it (p50 of 400 alternated starts), as the feature build loads `libstdc++.so.6` and runs 18 more static initializers (ONNX Runtime's), and its dynamic loader takes 424,000 cycles instead of 230,000. A first WSL session, run while test suites shared the CPU, gave +3.0 to +3.4 ms in its first two runs and is not counted.
- **F** (`local-embed.yml`, run 37930389936): macOS arm64, Linux x64 and arm64 and Windows x64 built, started alone with only the system's directories on the PATH, and passed the fixture. **macOS x64 fails**: `ort-sys@2.0.0-rc.13: no prebuilt binaries available for target x86_64-apple-darwin` (pyke's prebuilt ONNX Runtime 1.28.0 covers macOS arm64, Linux x64 and arm64 and Windows x64, and Microsoft publishes no macOS x64 build of 1.28.0 either). On Windows the feature build imports 28 DLLs against the plain build's 18. Beyond the system's own, they are the Visual C++ runtime's C++ libraries (`MSVCP140.dll`, `MSVCP140_1.dll`, `VCRUNTIME140_1.dll`, from the same redistributable as the `VCRUNTIME140.dll` the plain build already imports, so a machine without it runs neither build) and DirectML (`DirectML.dll`, in System32 since Windows 10 1903) with Direct3D 12 and DXGI: every Windows build pyke offers includes DirectML. The runners have the redistributable installed.
- **G** (WSL): inside `unshare -rn` (a network namespace whose only device, loopback, is down), `--verify`, a load and an embedding succeeded (a vector of 1,024 values), and `strace -f -e trace=network` over the whole run recorded no network call.
- **H** (WSL, curl `-L -C -` standing in for `model_fetch`): huggingface.co answered each file's URL with a redirect to its storage. The 2.27 GB file, killed at 30% (686,206,976 bytes) and again at 70% (1,590,059,008 bytes), resumed both times through the redirect and ended at 2,266,820,608 bytes with the pinned SHA-256, as every other file did.
- **Reported**: loading takes 1.8-2.5 s on WSL and 1.1-2.2 s on the iMac, so a fresh process's load and one query take about 1.9-2.7 s and 1.1-2.3 s. Typical documents (1,000 drawn at random from what the evaluation home's embedding phase sends, the fused graph): 0.33 s each on WSL at 8 threads (p50 245 ms, p95 559 ms; 16 and 32 threads were no faster) and 0.62 s on the iMac over 300 of them (p50 456 ms, p95 1,338 ms): that home's 201,261 texts in about 18 hours on WSL and 35 on the iMac, one process each. The binary grows from 19.8 to 45.2 MB on Linux x64, from 20.2 to 44.2 MB on Windows x64, and from 16.2 to 36.2 MB on macOS arm64 before the graph's 0.47 MB.

Also found:

- ONNX Runtime refuses weights reached through a symbolic link out of the model's folder (`External data path escapes model directory`). PR-A2's copy on the iMac, hf-hub's cache of links into `blobs/`, loaded only once its files were hard-linked into a folder of their own: `model_fetch` writes plain files.
- The fused graph has no attention mask, so the padding a batch adds to its shorter texts would change their vectors: one text per run. Texts of exactly the same token count need no padding; a backfill could batch those (not built).
- Python's onnx package, used only by `fuse.py`, refuses weights whose file has more than one hard link.

## Results with the runtime loaded at run time (2026-10-09)

The same lines, machines and harnesses, with Microsoft's library in the model's folder. This run and its stop rule were fixed before it ran (#423: if it failed line D on WSL, on the iMac or on Windows, Task 10 would stop at this note). WSL and the iMac ran release builds of one commit with the feature and without it, each built on its machine; Windows ran the MSVC pair of `local-embed.yml`'s run 37935548058.

- **A** (the public fixture): the same numbers as with the linked runtime, on WSL, on the iMac and on the four targets with a library in run 37935548058 (Linux x64 and arm64, macOS arm64, Windows x64): cos at least 0.99996 (`ja-20-24`), and every query's top 10 overlapping in at least 9 (`q-ja-12`).
- **B** (312 dev questions, the embeddings' p95 / p50 in ms): WSL at 32 threads 113.1 / 48.2, at 8 threads 73.7 / 32.6, and pinned to 4 threads 120.8 / 40.0; the iMac at its 8 threads 175.6 / 51.9, and at 4 threads 199.0 / 60.4. Passes: the slowest p95 is 199.0 ms (181.7 with the linked runtime).
- **C** (the iMac; peak RSS, its peak memory footprint in brackets): a reader took 1.98 GB [1.83] for one query; the worker 2.30 GB [2.10] and 23.5 s at 6,706 tokens, and 2.47 GB [2.20] and 35.8 s at 8,192; the 8,192-token worker and two readers started together 2.29 + 1.67 + 1.68 = 5.64 GB [5.87] (swap in use 806 to 1,206 MiB). Passes, each within 0.12 GB of the linked runtime's numbers. On WSL at 8 threads a reader took 1.84 GB, and the worker 2.32 GB at 6,706 tokens and 2.44 GB at 8,192.
- **D** (hook spawn p95 in ms, as before: the median of five runs of each build alternated, without the feature to with it):

  | | 1 KB | 64 KB |
  |---|---|---|
  | WSL | 11.9 to 11.9 (0.0) | 13.7 to 13.8 (+0.1) |
  | iMac | 24.0 to 23.8 (-0.2) | 23.1 to 23.2 (+0.1) |
  | Windows MSVC | 23.4 to 23.3 (-0.1) | 24.8 to 25.1 (+0.3) |

  **Passes on all three.** A process that does not embed loads nothing more than the plain build: on WSL `oboete --version` starts in 1.58 ms against the plain build's 1.50 (p50 of 400 alternated starts). A first Windows session, run while WSL's and the iMac's measurements shared this PC, gave +1.8 and -0.8 ms with one of the plain build's runs at 60.6 ms; the table's is the second, on a quiet machine.
- **F** (run 37935548058): all five targets built and started alone, macOS x64 included, as no ONNX Runtime is linked. On Windows the feature build imports the same 18 DLLs as the plain build, and on macOS only the system's libraries (`otool -L`). Microsoft's `onnxruntime.dll` itself imports the Visual C++ runtime's C++ libraries (`MSVCP140.dll`, from the same redistributable as the `VCRUNTIME140.dll` that every oboete build already imports) and DXGI, loaded only by a process that embeds.
- **G** (WSL): inside `unshare -rn`, `--verify` (which now checks the library too), a load and an embedding succeeded, and `strace -f -e trace=network` over the whole run recorded no network call.
- **H** (WSL, curl `-L -C -`, for the library's host): github.com answered a release archive's URL with a redirect to its storage (release-assets.githubusercontent.com), where a ranged request returned 206 with the bytes asked for. The Windows archive (78,796,801 bytes), killed at 30% and again at 70%, resumed both times and ended with the release's SHA-256.
- **Reported**: loading takes 1.7-3.1 s on WSL and 1.2-2.2 s on the iMac. Embedding documents is slower than with the linked runtime: the fixture's 220 texts took 199 s against 170 at 8 threads on WSL (+17%), 187 against 151 at 32 (+24%), and the iMac's whole fixture test 390 s against 361 (+8%), so the evaluation home's texts would take about 21 hours on WSL and 38 on the iMac. Queries were not slower on WSL (B). The binary grows from 19.9 to 22.7 MB on Linux x64, from 20.2 to 23.1 MB on Windows x64 and from 16.4 to 19.1 MB on macOS arm64, instead of to 45.2, 44.2 and 36.2; the library adds 15.8 to 39.3 MB to the model's folder, only where `local` is used.

## Step 3: the runtime and D14

**The runtime is the second one**: ONNX Runtime loaded at run time from Microsoft's pinned library. It passes every line the linked runtime passed, and the two that one failed: D on Windows (+0.3 ms against +7.5) and F on macOS x64. A process that does not embed, a hook first of all, pays nothing for the feature. What it costs: embedding a document takes 8-24% longer, and the build fetches one more file per target, from a second host (github.com, whose H passes), pinned as the model's files are. The binary stays one static binary (spec 7.1): it links no ONNX Runtime and starts alone on all five targets; the library belongs to the optional local model, as its weights do.

- **The targets**: Linux x64 and arm64, macOS arm64 and Windows x64 may offer `local`. macOS x64 builds and starts, but Microsoft publishes no ONNX Runtime 1.28.0 for it, so it gets no `local`.
- **The machines, line by line**: this PC's WSL passes A, B, C (WSL's numbers), D, G and H; the iMac passes A, B, C and D (G is the binary's and H the host's, measured on WSL); Windows passes A (on its runner) and D. The slowest WSL machine's 4-thread stand-in passes B; its memory is not known (What needs the owner, item 5). Which machines use `local` stays the owner's choice (What needs the owner, item 2), with these numbers: about 2.5 GB for the worker and 2.0 GB per searching process, the evaluation home's whole store in about 21 hours on WSL and 38 on the iMac, no money.
- **The model**: the fused graph and BAAI's five files, one text per run for any length, at most 8 threads (B: WSL's p95 74 ms at 8 against 113 at 32 and 121 at 4; the iMac has 8).
- **`CLI_EMBEDS_QUERIES`**: false. A fresh process needs 1.2-3.1 s to load the model, past MCP's 1.5 s, so only a process that stays loaded (MCP's server) embeds queries.
- **What the build adds for the runtime**: `model_fetch` also fetches the target's library: Microsoft's archive at its pinned SHA-256, the one file taken out of it and checked against its own pin (`embed_local::RUNTIME`) before it is placed in `onnxruntime/`. Doctor names a missing or changed library as it names the model's files.
- **The spike's downloads** stay until the owner chooses (What needs the owner, items 2 and 6): this PC's (2.3 GB, and the library), the iMac's (PR-A2's cache with the spike's hard links to it, and the library), and the builds in the Windows probe folder.

The linked runtime's results stay above as measured. Its D failure on Windows came from the start of every process: the feature build loaded the C++ runtime and DirectML's three libraries and ran ONNX Runtime's static initializers, and on WSL it started 0.95-1.18 ms later than the plain build, though no hook embeds.
