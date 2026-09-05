# Phase 0A static inventory summary — ai-memory

## Snapshot, scope, and counting rules

- Audited revision: `a9e9a24d50f59e970fc01ae48efe647abf20702e` (`v1.26.0`, detached HEAD).
- This is the Phase 0A/T008 summary-level inventory requested for delta comparison, not an exhaustive call graph or runtime trace.
- DB-open count unit: executable direct `rusqlite::Connection::open*` call sites. Wrapper callers are reported separately. Production excludes inline `#[cfg(test)]` modules and files under `*/tests/`.
- Credential-loader count unit: source implementations that acquire secret material from process environment or persistent auth storage. Request-time token verification is a boundary, not a loader.
- Transfer paths are grouped by externally meaningful flow. Generic HTTP helpers, local installer-file edits, and same-store multi-user visibility are not double-counted as separate sync implementations.

## Comparison summary

| Surface | Production count | Interpretation |
|---|---:|---|
| Direct SQLite open sites | 10 | 2 primary-store opens (1 R/W + 1 R/O pool) and 8 R/O native-transcript source opens |
| Test/support direct SQLite open sites | 64 | Excluded from production: 49 inline-test and 15 integration-test sites |
| Primary write-capable DB connection origins | 1 | `Store::open` creates it; one writer actor owns it |
| Cloneable primary writer-handle types | 1 | `WriterHandle`, an `mpsc` command facade; clones do not clone the DB connection |
| Production `Store::open` entry points | 3 | Serve, reindex, restore; all reuse the same store/writer implementation |
| LLM provider / embedder choices | 8 / 4 | Five provider auth requirement variants |
| Main-process provider secret loader | 1 | 13 environment names/aliases populate 9 typed secret fields; static server bearer is one additional env read |
| Persistent auth-file modules / raw read sites | 1 / 2 | One runtime load read plus one save-time read/modify/write preservation read; three typed token entries share the module |
| Companion credential loaders | 1 | Importer reads a static server bearer from environment |
| Transfer/path families | 6 | Backup, restore, OMC importer, hook/handoff, managed workstream, admission webhooks |
| Same-store sharing/coordinator mutation dependency | Yes | Shared handoffs and managed-workstream state mutate through the core `WriterHandle`; no alternate DB writer surfaced |
| Remote/cloud wiki replication implementations found | 0 | Wiki Git operations are local checkpoints; no production push/fetch replication path surfaced |

## 1. Database open sites

The source scan found 74 executable direct opens in total: 10 production and 64 test-only. One additional textual match is documentation/commentary rather than an executable open.

### Primary store: 2 production direct opens

- **One normal read/write origin:** `crates/ai-memory-store/src/lib.rs:103-121`. The direct `Connection::open` is at line 108; the same connection is configured, migrated, and transferred to `WriterHandle::spawn` at line 120.
- **One read-only pool origin:** `crates/ai-memory-store/src/reader.rs:7007-7013`. It uses `SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_URI | SQLITE_OPEN_NO_MUTEX`; pooled readers route through this helper.
- Three production wrapper callers create a `Store` but do not introduce new direct-open implementations: `crates/ai-memory-cli/src/commands/serve.rs:346`, `reindex.rs:41`, and `restore.rs:73`.

### External/native transcript sources: 8 production direct opens

All eight open agent-harness databases read-only with `SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_NO_MUTEX`; they do not open the ai-memory primary store:

- `crates/ai-memory-workstream/src/transcript.rs:1856,1917`
- `crates/ai-memory-workstream/src/transcript.rs:2639`
- `crates/ai-memory-workstream/src/transcript.rs:2754,2782,2833,2864,2897`

These cover native transcript discovery/export paths such as OpenCode/Crush and Antigravity metadata.

### Test-only sites: 64, excluded

- 49 inline `#[cfg(test)]` sites: ai-memory-store 42, ai-memory-consolidate 2, ai-memory-workstream 5.
- 15 integration-test sites: ai-memory-store 11 and ai-memory-consolidate 4.
- Representative examples: `crates/ai-memory-store/src/migrations.rs:214`, `crates/ai-memory-store/src/ops.rs:3089`, `crates/ai-memory-consolidate/tests/lifecycle.rs:148`, `crates/ai-memory-workstream/src/transcript.rs:3393`.

## 2. Write-capable handles

- **Static primary-writer architecture count: one origin, one actor-owned connection.** `Store` exposes `writer: WriterHandle` and `reader: ReaderPool` separately (`crates/ai-memory-store/src/lib.rs:86-93`). `Store::open` creates/migrates the only primary R/W connection and passes ownership to the writer (`lib.rs:103-121`).
- **One cloneable command facade:** `WriterHandle` wraps a bounded `mpsc::Sender` (`crates/ai-memory-store/src/writer.rs:395-420`). `spawn` moves the connection into the dedicated `ai-memory-writer` thread (`writer.rs:406-413`), and `worker_loop(mut conn, ...)` is the command dispatcher (`writer.rs:1593+`). Handle clones therefore submit commands; they are not independent writable SQLite handles.
- **Backup output is a separate file sink, not another primary writer actor.** `ReaderPool::snapshot_to` calls SQLite's online backup API with a destination path (`crates/ai-memory-store/src/reader.rs:3430-3440`). It writes a snapshot file while reading from the pool and is counted under backup/export below.
- No production direct writable SQLite open was found outside `Store::open`; the native-transcript database opens are explicitly read-only.

## 3. Provider auth and credential loaders

### Provider surface and typed boundary

- Eight LLM choices are declared at `crates/ai-memory-llm/src/factory.rs:25-42`: Anthropic, OpenAI, Gemini, OpenAI-compatible, OpenAI OAuth, Copilot, Anthropic OAuth, and OpenCode.
- They map to five auth mechanisms at `factory.rs:60-82`: required API key, optional API key, OpenAI OAuth token, Copilot token, and Anthropic OAuth token. Provider construction consumes typed `ProviderAuth` rather than reading process state itself (`factory.rs:229-271`).
- Four embedder choices are declared at `factory.rs:104-116`: OpenAI, Voyage, Google, and OpenAI-compatible.

### Environment and config acquisition

- The main acquisition point is `RuntimeEnv::from_process` (`crates/ai-memory-cli/src/config.rs:285-322`). Thirteen secret environment names/aliases populate nine typed provider/embedder fields (`config.rs:271-282,303-321`): `ANTHROPIC_API_KEY`, `ANTHROPIC_OAUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `GOOGLE_API_KEY`, `LLM_API_KEY`, `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN`, `GITHUB_COPILOT_API_TOKEN`, `VOYAGE_API_KEY`, and `OPENCODE_API_KEY`.
- The static server bearer `AI_MEMORY_AUTH_TOKEN` is one separate read in the same aggregator (`config.rs:291`). `Config::load` merges process inputs with config once (`config.rs:771-835`).
- Embedder credential selection is centralized at `config.rs:1024-1054`; LLM material is converted to typed `ProviderAuth` at `config.rs:1108-1142`.

### Persistent auth material

- Persistent credential storage is centralized in one module, `crates/ai-memory-llm/src/auth_file.rs`. It contains two direct raw reads: the runtime `load_entry` read at lines 10-29 (`std::fs::read` at 17), and the existing-document read used by `save_entry`'s read/modify/write path at lines 32-40 (`std::fs::read` at 37). Only the former is a runtime credential loader; the latter preserves sibling entries while writing auth state.
- Three typed entries delegate to it: OpenAI OAuth (`openai_oauth.rs:98-100`), Copilot (`copilot.rs:138-149`), and OIDC (`oidc.rs:88-90`). Runtime consumers include OpenAI construction (`openai_oauth.rs:148-175`), Copilot construction/refresh (`copilot.rs:196-261`), and client bearer resolution (`crates/ai-memory-cli/src/auth_bearer.rs:14-44`). CLI login/status manages the same entries (`crates/ai-memory-cli/src/commands/auth.rs:69-295`).
- The optional companion importer has one separate static-bearer environment loader (`companions/ai-memory-importer/src/main.rs:532-572`).
- Incoming DB-user token lookup (`crates/ai-memory-mcp/src/auth.rs:372`) is request authorization/validation and is intentionally not counted as a credential loader.

## 4. Sync, sharing, import, and export paths

Six production path families surfaced. These are transfer/mutation paths, not six independent database writers.

1. **Backup export.** CLI request/file sink: `crates/ai-memory-cli/src/commands/backup.rs:19-38` and `crates/ai-memory-cli/src/http_client.rs:388-418`; admin route: `crates/ai-memory-mcp/src/admin.rs:536-541`; archive construction: `admin.rs:670-747`; SQLite snapshot source: `crates/ai-memory-store/src/reader.rs:3430-3440`.
2. **Backup and checkpoint restore/import.** Full-archive restore performs local CLI extraction, validation, and store reopen (`crates/ai-memory-cli/src/commands/restore.rs:33-97`, reopen at lines 71-74); no server `/admin/restore` route was found. Single-page recovery instead POSTs a Git checkpoint revision through `/admin/restore-page` (`crates/ai-memory-cli/src/commands/restore_page.rs:29-49`; route registration at `crates/ai-memory-mcp/src/admin.rs:580-583`).
3. **Optional OMC wiki importer.** Source scan/plan: `companions/ai-memory-importer/src/main.rs:144-257,312-381`; manifest output: lines 274-301; target preflight/list/existence checks and `POST /admin/write-page`: lines 574-652. It uses the public admin mutation path rather than opening the DB.
4. **Hook spool/transport, handoff, and explicit same-store sharing.** Events are atomically queued to a private local spool (`crates/ai-memory-cli/src/commands/hook_spool.rs:92-136`) and drained oldest-first in bounded batches with per-event fallback (`hook_spool.rs:397-416,451-510`). Client single/batch POST and handoff GET live at `crates/ai-memory-cli/src/commands/hook_capture.rs:339-362,403-463,482-510`; server ingress routes are registered at `crates/ai-memory-hooks/src/router.rs:568-575`. The MCP handoff path treats `shared: true` as project-wide ownership (`owner_user = None`) and persists it through `WriterHandle::insert_handoff` (`crates/ai-memory-mcp/src/server.rs:3078-3082,3128-3168`; ownership meaning at `crates/ai-memory-core/src/handoff.rs:148-150`). Thus sharing/coordinator behavior does depend on a core mutation, but does not introduce an alternate DB handle.
5. **Managed workstream transfer.** Client start/finish protocol and visible-event import: `crates/ai-memory-cli/src/commands/run.rs:120-165,1152-1221`; read-only native transcript exporter dispatch: `crates/ai-memory-workstream/src/transcript.rs:107-136`. This is client/server workstream transfer, not cloud replication.
6. **Admission webhooks.** Configuration: `crates/ai-memory-cli/src/config.rs:237-250`; outbound payload includes page path, frontmatter, and body (`crates/ai-memory-wiki/src/admission.rs:200-212`). Blocking webhooks synchronously POST and may accept, mutate, or reject a write (`admission.rs:311-418`, POST at 343-350). Observer/mirror webhooks make bounded asynchronous POSTs (`admission.rs:580-649`). This is the only surfaced external page-content egress/mirror family and can affect a core wiki mutation before commit.

The configurable HTTP client (`crates/ai-memory-cli/src/http_client.rs:55-100`) is shared transport for remote server access and is not a seventh family. `apply_shared` edits local agent installer configuration; multi-user “shared” records remain in the same Store. Local wiki Git code opens/commits a repository (`crates/ai-memory-wiki/src/git.rs:51,75-126`), but no production Git push/fetch or cloud wiki replication implementation surfaced.

## Reproduction notes

- Direct-open search basis: `rg -n 'Connection::open(_with_flags|_in_memory)?\(' crates companions evals --glob '*.rs'`, followed by manual production/test and flags classification.
- Counts describe this exact pinned source tree. Feature-gated/generated code or dynamically loaded code not present in the tree is outside this static inventory.
