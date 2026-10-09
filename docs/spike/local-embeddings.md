# Spike: local bge-m3 (milestone 4, Task 10)

Throwaway (spec 8.3). Milestone 4's Task 10 (docs/milestone-4-plan.md): can bge-m3 run inside oboete's own processes on the owner's machines, agreeing with the Workers AI vectors it would replace, within the memory and time a resident worker and an MCP server may take? This note fixes the lines and how each is measured before any run (Step 1); the results come in Step 3 (Step 2 runs them). PR-A2 measured a first version on this PC and the M1 iMac on 2026-09-24 (docs/pr-a.md, A2): its agreement checks passed on both, a query took 1.79 GB at peak and an 8,000-character text 2.79 GB, and fastembed grew the binary from 10.7 to 36.1 MB.

## What runs

- **The runtime**: fastembed 7.1 (7.1.1 at writing), with ONNX Runtime linked into the binary (`ort-download-binaries`, rustls), behind the cargo feature `local-embed`. The model is loaded from files oboete fetched and checked itself (`model_fetch`), never through hf-hub: fastembed's user-defined model, given its external data by name.
- **The model**: BAAI/bge-m3's fp32 ONNX export, the files fastembed's `EmbeddingModel::BGEM3` names (`onnx/model.onnx`, `onnx/model.onnx_data`, `onnx/Constant_7_attr__value`, and the tokenizer's `tokenizer.json`, `config.json`, `special_tokens_map.json`, `tokenizer_config.json`), at one pinned commit of huggingface.co/BAAI/bge-m3, each with its size and SHA-256, run at 8,192 tokens. The first download in Step 2 records the commit and the hashes here, and they are fixed from then on: a change to either is a new embedder id. The int8 export is not a candidate: it failed PR-A2's agreement check (cos 0.846 at worst, docs/pr-a.md, A2 item 5).
- **Machines**: this PC's WSL (x86_64, 32 threads), the same pinned to 4 threads as the stand-in for the slowest WSL machine (What needs the owner, item 5), the M1 iMac (8 GB) over SSH, and Windows on this PC with the MSVC build. `.github/workflows/local-embed.yml` builds a ref on the five release targets (spec 7.1: Linux x64 and arm64, macOS arm64 and x64, Windows x64), starts each binary alone and runs the agreement test on the public fixture.

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
- **C**: the peak resident set (`VmHWM` on Linux, `/usr/bin/time -l` on macOS) of a worker that embeds one 12,000-character text, of a reader that embeds one query, and of the worker and two readers started together.
- **D**: milestone 2's harness, `oboete --home <tmp> replay <events-1000.jsonl> --spawn-sample 300 --sizes 1,64` (docs/milestone-2.md), with release builds of one commit with the feature and without it, five runs of each alternated in one session, on each of the three machines.
- **F**: `local-embed.yml` on the five targets. Each binary is copied alone into an empty directory and started with only the system's directories on the PATH.
- **G**: on WSL, the model's files in place, `unshare -n` (no network at all) around a load and an embedding: they must succeed, and the process must open no socket (`strace -f -e trace=network` shows none).
- **H**: `model_fetch` against huggingface.co, which answers the file's URL with a redirect to its storage: a `Range` request after the redirect returns 206 with the requested bytes; a download killed at 30% and one killed at 70% resume where they stopped and end with the pinned SHA-256.

## After the runs

Step 3 records the results here: the runtime, the owner machines that get `local`, the thread cap, how many long texts one batch may hold (PR-A2: 8 long texts in one batch reached 17.8 GB), and whether the CLI embeds its queries (`CLI_EMBEDS_QUERIES`, true only within MCP's 1.5 s). Then the spike's model downloads are deleted on each machine (their hashes stay here), except where the owner chose `local`, where they are the installed model (What needs the owner, item 6).
