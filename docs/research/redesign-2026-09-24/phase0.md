# Phase 0: can Hindsight be the engine? (spike, 2026-09-24)

Throwaway spike for the redesign (options-draft.md §9, pass lines fixed before measuring). Hindsight v0.10.1 (hindsight-api 0.10.1, pg0-embedded 0.15.2, PostgreSQL 18.1 + pgvector 0.8.5), documented pip path, LLM provider `none` unless noted. Workflow `wf_e1d80fc5-ed4`; raw results in `phase0-result.json` (scratchpad copy).

## A-install (pass: all three install, RSS ≤ 1 GB)

| Platform | Install | Starts | RSS | Japanese full-text in the embedded DB |
|---|---|---|---|---|
| WSL2 Ubuntu (RTX 5080) | 43.5 s, 7.2 GB on disk | yes | 2.4 GB idle (CUDA torch loads the embedder and reranker eagerly); 3.4 GB with bge-m3 + bge-reranker-v2-m3 | no: pg0 ships only `vector` and `pg_trgm` |
| Windows 11 native (Japanese locale) | 45.2 s, 2.0 GB | only with `PYTHONUTF8=1` set by hand: the startup banner raises `UnicodeEncodeError: 'cp932' codec can't encode character '▄'` and the port never binds; not in the docs. pg0 writes to `C:/Users/jura/.pg0` whatever HOME/USERPROFILE say | 1.2 GB | no |
| M1 iMac, macOS 26.6 (no Homebrew) | 601 s, 3.9 GB | **no**: pg0's PostgreSQL fails with `dyld: Library not loaded: /opt/homebrew/opt/openssl@3/lib/libssl.3.dylib` (absolute Homebrew path in `libpq.5.dylib`), twice, deterministic. pg0's README says macOS needs no dependencies | — | — |

**Fail.** macOS without Homebrew does not start; Windows needs an undocumented workaround on Japanese Windows; every platform is over 1 GB; Japanese BM25 needs an external PostgreSQL with pgroonga (Docker), so the embedded path has no Japanese full-text.

## A-delete (pass: 0 hits for the canary after deletion through the public API)

Local Ollama `qwen3.5:9b` as the LLM so facts, observations and a mental model exist. Canary `OBOETE-CANARY-7Q2X` in one document.

| Step | Tables still holding the canary |
|---|---|
| Before | chunks, documents, entities, memory_units (2 facts + 2 observations), mental_models, mental_model_history, llm_requests (11 rows, 122 occurrences) |
| `DELETE .../documents/{id}` | mental_models (full text, `is_stale=true`), mental_model_history, llm_requests |
| `PATCH .../memories/{id}` invalidate | nothing left to invalidate |
| `DELETE .../banks/{id}` | **llm_requests** (the LLM prompt/completion audit trail; no API deletes it, only a time-based background sweep) |
| On disk after CHECKPOINT | canary bytes in 7 heap/index files and the WAL (dead tuples until VACUUM) |

**Fail.** Mental models keep the deleted text until the whole bank is deleted, and the audit table keeps full copies even then. (Correction to the earlier verified-docs note: deletion does mark the mental model stale; it does not remove its text.)

## Decision by the pre-stated rule

A is chosen only if Phase 0 passes. It does not, so the engine is B (self-built). M1 (retrieval on the 112 questions) was not run: it was conditional on Phase 0, and ingesting the 178,502-document corpus measured 2.63 documents/s end to end (about 19 hours).

## What B takes from this

- Hindsight's strongest retrieval difference, a multilingual cross-encoder (`BAAI/bge-reranker-v2-m3`, fp16 on CUDA), is testable directly on top of oboete's hybrid top 50 (proposal E4) without Hindsight.
- Deletion must reach every copy of text, including LLM request logs, provider caches and derived summaries; SQLite needs `secure_delete`, a WAL checkpoint and FTS `optimize` for the bytes to go (M4 greps files, not only tables).
- Release checks on the owner's machines: macOS without Homebrew (no absolute dylib paths), Windows with a Japanese (cp932) console.

## Cleanup

Windows probe directory and the escaped `C:/Users/jura/.pg0` deleted (PATH, Run keys, services and scheduled tasks unchanged). iMac `~/oboete-probe/hindsight` deleted. WSL: API stopped, `hindsight-m1-db` container stopped; `/home/jura/hindsight-probe` (venv, models, M1 data) kept until the owner's decision, then removed.
