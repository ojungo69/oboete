# Phase 0A static inventory summary — remem

- Snapshot: `cde8bc05504c74794d044ef118f74d8f828adbf5`
- Generated: 2026-08-12 (Asia/Tokyo)
- Purpose: summary-level delta inventory, not an exhaustive call graph.

## Counting boundary

- DB counts scan production Rust under `src/`. Files/directories explicitly named as tests and exact `#[cfg(test)]` items are excluded; in-memory SQLite fixtures are excluded.
- A “site” is a lexical acquisition call site, not a runtime connection count. Alternative reopen/fallback branches count separately. Wrapper definitions and their internal delegation are not counted as consumers.
- Credential counts include the Rust runtime, plugin app server, and the macOS coding-benchmark runner. Test fixtures and strings used only for secret-redaction tests are excluded.
- Sync/import/export counts are logical boundary families: data entering remem persistence or remem data being copied/serialized outside the primary DB. Normal search/context output and installer/release downloads are outside this count.

## DB open sites and write-capable handles

### Counts

| Surface | Static count | Files | Notes |
|---|---:|---:|---|
| Central physical DB constructors | 3 | 1 | 2 read-write/create factories and 1 read-only factory in `src/db/core.rs` |
| Write-capable wrapper acquisitions | 101 | 62 | `open_db`: 94; `open_db_no_migrate`: 2; `open_db_for_hook`: 5 |
| Explicit read-only wrapper acquisitions | 20 | 9 | `open_db_read_only`: 15; `open_db_read_only_current`: 5 |
| Non-core raw persistent/external DB opens | 8 | 6 | 4 default read-write opens and 4 explicit read-only opens; fallback reopens count separately |
| Migration dry-run temporary DB open | 1 | 1 | Writable temporary clone only; excluded from persistent-handle count |

### Representative evidence

- The central constructors are `Connection::open` (read-write/create) at `src/db/core.rs:188-201`, `SQLITE_OPEN_READ_WRITE` at `src/db/core.rs:204-223`, and `SQLITE_OPEN_READ_ONLY` at `src/db/core.rs:226-238`.
- `open_db` is intrinsically write-capable: it can create the data directory, run migrations, ensure the vector table/index, and enforce policy state (`src/db/core.rs:112-133`). A caller that only queries still receives a handle whose open path can mutate.
- `open_db_no_migrate` still opens `READ_WRITE` and performs vector-index/policy maintenance (`src/db/core.rs:148-160`). `open_db_for_hook` delegates to it (`src/db/core.rs:163-168`), so hook handles are not read-only.
- Hook examples: SessionStart/session setup `src/observe/session_init.rs:38`, cursor `src/observe/cursor.rs:35`, event hook `src/observe/hook.rs:59`, and spill replay `src/observe/spill.rs:84`.
- Long-lived/runtime examples: extraction worker `src/extraction_worker.rs:27,49`, general worker `src/worker.rs:281,328`, MCP preflight `src/mcp/server/runtime.rs:62-64`, and a fresh MCP tool connection `src/mcp/server.rs:35-40`.
- API handlers obtain the same write-capable wrapper through `src/api/helpers.rs:81-88`; even read-resource handlers use `open_db` at `src/api/read_resources.rs:187,286`.
- CLI export/read examples also use the write-capable wrapper: Markdown export `src/cli/actions/markdown_archive.rs:163-174`, pack export `src/cli/actions/pack_export.rs:20-22`, and status/query paths represented by `src/cli/actions/query/status.rs:31`.
- Explicit read-only examples are raw-session export `src/cli/actions/query/raw.rs:82-103`, user profile export `src/cli/actions/user_profile.rs:13-46`, and pack-import dry-run `src/cli/actions/pack_import.rs:12-27`.
- The four non-core raw read-write opens are plaintext/encryption inspection (`src/install/runtime.rs:404-437`), SQLCipher conversion (`src/db/crypto.rs:229-279`), and two alternative backup-source opens (`src/cli/actions/admin.rs:40-62`). These are write-capable handles even where the intended operation is inspection/read.
- The four non-core raw read-only opens are two alternative legacy-import source opens (`src/cli/actions/import.rs:72-90`), encrypted-state inspection (`src/cli/actions/encrypt_state.rs:14-31`), and benchmark artifact verification (`src/eval/bench_artifact/verify/coding.rs:299-313`).

### Delta-relevant conclusion

There is no single-writer/coordinator boundary. CLI actions, hooks, API handlers, MCP tools, workers, and evaluation flows independently acquire SQLite connections. The dominant read/write distinction is chosen by caller wrapper, and many semantically read-only paths call `open_db`, whose open sequence itself may write.

## Provider auth and credential loaders

### Counts

- Remote provider auth loaders: **2**.
- Credential classes including local storage/API and benchmark auth: **5**.
- Concrete loader implementations: **6** because the same local REST token is read independently by Rust and the Node plugin app.
- Hard-coded private provider backends found: **0**. Two configurable HTTP provider base URLs default to public Anthropic/OpenAI endpoints and are documented.

| Credential class / loader | Source and behavior | Scope |
|---|---|---|
| Anthropic HTTP API key | Reads `ANTHROPIC_API_KEY`, then `ANTHROPIC_AUTH_TOKEN`; sends `x-api-key` to configured/default `/v1/messages` (`src/ai/http.rs:34-64`) | Normal runtime when `executor=http` |
| OpenAI-compatible embedding key | Reads `REMEM_EMBEDDINGS_API_KEY`, legacy `REMEM_EMBEDDING_API_KEY`, or the configured `api_key_env` (default `OPENAI_API_KEY`) at `src/retrieval/embedding.rs:511-529`; sends Bearer auth at `:552-572`. Config/env resolution is `src/retrieval/embedding/config.rs:39-78,81-107`. | Normal runtime when API embeddings are selected |
| SQLCipher store key | Reads `REMEM_CIPHER_KEY`, else `${REMEM_DATA_DIR}/.key`; missing key fails closed unless explicit plaintext override (`src/db/crypto.rs:24-67`) | Every primary DB open |
| Local REST bearer token — Rust | Reads `${REMEM_DATA_DIR}/.api-token` for every auth check (`src/api/auth.rs:12-17,48-74`); install/API startup creates and permission-hardens it (`:19-46,95-162`) | Native local API |
| Local REST bearer token — Node | Independently reads the same `.api-token` and attaches Bearer auth (`plugins/remem/apps/remem/server.js:31-36,233-265`) | Codex plugin app proxy |
| Codex `auth.json` copy | macOS-only coding benchmark copies host `${CODEX_HOME:-~/.codex}/auth.json` into an isolated Codex home (`src/eval/coding_bench/isolation.rs:32-78,102-129`) and removes the copied file after the sandboxed run (`:192-203`) | Evaluation command only; not normal capture/recall runtime |

Normal Claude/Codex AI executors spawn the installed CLIs and delegate authentication to those CLIs (`src/ai/cli.rs:7-35`, `src/ai/codex_cli.rs:10-42`); they do not parse Claude/Codex credential stores themselves. The macOS benchmark copy above is the sole explicit third-party credential-file loader found. Local model download is intentionally unauthenticated and pins the official Hugging Face endpoint/revision (`src/retrieval/embedding/local_semantic/download.rs:25-94,267-287`).

## Sync, sharing, and import/export paths

### Counts

- Logical boundary families: **14**.
- Inbound/mutating import or ingest families: **6**.
- Outbound/copy/serialization families: **8**.
- Remote memory sharing, cloud sync, peer sync, or coordinator service: **0**.

“Sharing” in this snapshot is local/user-mediated file exchange (Markdown, packs, backups, host-native files), not a network synchronization protocol.

| # | Path family | Direction / target | Direct business mutation of core DB | Representative evidence |
|---:|---|---|---|---|
| 1 | Legacy SQLite backup import | External SQLite → remem memories | Yes | `src/cli/actions/import.rs:47-90` |
| 2 | Markdown mirror import | Markdown files → curated memories/indexes | Yes | `src/cli/actions/markdown_archive.rs:191-201,263-337` |
| 3 | Project memory pack import | `pack.json` + `memories.jsonl` → local rows/review | Apply only; dry-run is read-only | `src/cli/actions/pack_import.rs:12-33,177-225` |
| 4 | Codex native memory import | `~/.codex/memories/rollout_summaries` → candidate review | Apply only; digest-bound dry-run first | `src/cli/actions/codex_memory_import.rs:1-8,22-75` |
| 5 | Transcript batch ingest | Claude/Codex JSONL roots → raw archive | Yes | `src/cli/actions/ingest_sessions.rs:13-44`; defaults documented by `src/cli/types.rs:585-597` |
| 6 | Claude native topic-file ingest | Host-written `.claude/.../memory/*.md` → candidate review | Yes | `src/observe/hook.rs:208`; `src/observe/native.rs:1-8,22-75` |
| 7 | Admin database backup | remem DB → SQLite backup file | No intended source mutation; raw source handle is write-capable | `src/cli/actions/admin.rs:14-24,40-87` |
| 8 | Markdown mirror export | Curated memories → one Markdown file per memory | No business mutation; uses `open_db`, so open-time maintenance can write | `src/cli/actions/markdown_archive.rs:163-188` |
| 9 | Project pack export | Active repo-owned memories → deterministic pack directory | No business mutation; uses `open_db`, so open-time maintenance can write | `src/cli/actions/pack_export.rs:20-75` |
| 10 | Claude native memory sync | DB session index → `remem_sessions.md` + `MEMORY.md` index | No business mutation; callers use write-capable DB handles | Explicit CLI: `src/cli/dispatch.rs:145-150`; automatic summary/rollup: `src/summarize/summary_job/process.rs:147`, `src/session_rollup/side_effects.rs:297`; writer: `src/context/claude_memory/runtime.rs:17-49` |
| 11 | Procedure draft export | Verified procedure → Claude Skill, Codex prompt, or runbook draft | Yes: records export registry row in addition to file write | `src/cli/actions/procedures/write.rs:19-65,198-207` |
| 12 | User profile export | Read-only profile projection → Markdown/stdout | No; explicit read-only DB | `src/cli/actions/user_profile.rs:13-74` |
| 13 | Exact raw-session export | Read-only raw messages → human output or JSON/stdout | No; explicit read-only-current DB | `src/cli/query_types.rs:187-210`; `src/cli/actions/query/raw.rs:82-112` |
| 14 | `save_memory` local Markdown copy | Manual durable save → DB plus local note | Yes; dual-write with backup/rollback handling | `src/memory/service/save.rs:61-102,436-483,536-550` |

The three principal portable formats are therefore backup SQLite, editable Markdown, and deterministic project packs. All imports that apply data write through ordinary process-local SQLite handles; none depends on a sharing/coordinator service.
