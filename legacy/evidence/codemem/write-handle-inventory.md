# Phase 0A static write-handle and integration-path inventory

Evidence snapshot: commit `26438e75ce1d0fec6be34981f15045a15c89658b`.

## Scope and enumeration method

- This is a static inventory of the pinned tracked tree. It records locations and call paths only. It does not assign fatal/non-fatal labels, recommend removals, or make the Phase 0A base decision.
- Database-open coverage was cross-checked over every tracked TypeScript, TSX, JavaScript, MJS, CJS, and shell file for direct `better-sqlite3` construction, the `connect`, `connectReadOnly`, and `connectCoordinator` wrappers, `MemoryStore`, both coordinator-store implementations, D1 bindings, and Drizzle wrappers. The full call-site appendix includes runtime, repository tooling/E2E, and tests.
- Write capability means that the handle itself, or a wrapper around the same underlying handle, exposes an `INSERT`, `UPDATE`, `DELETE`, DDL, transaction, pragma, vector, or store mutation path. This functional grouping is distinct from the deferred T010 fatal/non-fatal classification.
- Provider/auth coverage follows config loading through credential resolution and backend dispatch. Sync/sharing/import-export coverage follows CLI/API/UI entry points into the core transport and mutation modules.

## A. Database open definitions and handle roots

### Main SQLite database

1. `connect()` is the general writable opener. It resolves/migrates the path, creates the parent directory, constructs `better-sqlite3`, runs schema bootstrap, enables WAL, and returns the handle (`packages/core/src/db.ts:200-243`). Schema bootstrap and compatibility DDL are reachable during open via `ensureSchemaBootstrapped`, `assertSchemaReady`, and `ensureAdditiveSchemaCompatibility` (`packages/core/src/schema-bootstrap.ts:631-816`, `packages/core/src/db.ts:516-620`, `packages/core/src/db.ts:640-1495`).
2. `connectReadOnly()` is the separate existing-file opener with `{ readonly: true, fileMustExist: true }`; it runs the read-only readiness check and no bootstrap (`packages/core/src/db.ts:245-285`). Its two production call sites are `packages/cli/src/commands/status.ts:356` and `packages/core/src/maintenance/with-db.ts:36`.
3. `MemoryStore` exposes the raw connection as public `readonly db`, creates a lazy Drizzle wrapper over that same connection, calls `connect()` in its constructor, and runs vector/schema/planner initialization (`packages/core/src/store.ts:209-252`). Every direct or aliased `MemoryStore` construction therefore opens the general handle; all 120 sites are in Appendix A.
4. Direct hook fallbacks open the general handle without constructing `MemoryStore`: Claude `directEnqueue` (`packages/cli/src/commands/claude-hook-ingest.ts:128-199`) and Codex `directEnqueue` (`packages/cli/src/commands/codex-hook-ingest.ts:128-197`).
5. Other production `connect()` callers are the DB/admin command paths (`packages/cli/src/commands/db.ts:146`, `packages/cli/src/commands/db.ts:582`); export/import (`packages/core/src/export-import.ts:309-376`, `packages/core/src/export-import.ts:573-594`); coordinator invite persistence (`packages/core/src/coordinator-actions.ts:1764-1846`, `packages/core/src/coordinator-actions.ts:1973-2110`); extraction replay/eval (`packages/core/src/extraction-replay.ts:431`, `packages/core/src/extraction-eval.ts:335`); maintenance wrapper/vacuum (`packages/core/src/maintenance/with-db.ts:17`, `packages/core/src/maintenance/init-vacuum.ts:12-35`); dedup/ref/scope/session/summary/vector backfills (`packages/core/src/dedup-key-backfill.ts:213`, `packages/core/src/ref-backfill.ts:285`, `packages/core/src/scope-backfill.ts:724`, `packages/core/src/session-context-backfill.ts:415`, `packages/core/src/summary-dedup-backfill.ts:321`, `packages/core/src/vector-migration.ts:645`); and sync retention (`packages/core/src/sync-retention-runner.ts:259`).
6. The workspace bootstrap template contains an inline Node program that imports `better-sqlite3`, constructs a DB directly, and upserts a peer (`scripts/templates/workspace-codemem-bootstrap.sh:73`).

### Coordinator databases

1. `connectCoordinator()` constructs a separate `better-sqlite3` handle, enables WAL, and initializes coordinator schema (`packages/core/src/better-sqlite-coordinator-store.ts:864-873`). `BetterSqliteCoordinatorStore.db` owns that handle (`packages/core/src/better-sqlite-coordinator-store.ts:876-883`). Its constructor sites are listed in Appendix A.
2. `D1DatabaseLike` defines the Cloudflare D1 binding surface, including `prepare`, `batch`, and `exec` (`packages/core/src/d1-coordinator-store.ts:73-85`). `D1CoordinatorStore.db` retains the supplied binding (`packages/core/src/d1-coordinator-store.ts:678-683`).
3. The D1 runtime injects the binding into `D1CoordinatorStore` (`packages/core/src/d1-coordinator-runtime.ts:6-24`). The Worker obtains that binding from `env.COORDINATOR_DB` and supplies it to the runtime (`packages/cloudflare-coordinator-worker/src/index.ts:4-7`, `packages/cloudflare-coordinator-worker/src/index.ts:22-36`). Binding/admin-secret deployment configuration is `packages/cloudflare-coordinator-worker/wrangler.jsonc:6-11` and `packages/cloudflare-coordinator-worker/wrangler.toml.example:6-12`.
4. The local coordinator runtime constructs `BetterSqliteCoordinatorStore` through its store factory (`packages/core/src/better-sqlite-coordinator-runtime.ts:15-33`).
5. D1 schema/DDL is supplied outside the store constructor by `packages/cloudflare-coordinator-worker/schema.sql:1-211` and migrations `packages/cloudflare-coordinator-worker/migrations/0001_init.sql:1-59`, `packages/cloudflare-coordinator-worker/migrations/0002_add_reciprocal_approvals.sql:1-9`, `packages/cloudflare-coordinator-worker/migrations/0003_harden_reciprocal_approval_pending_pairs.sql:1-33`, `packages/cloudflare-coordinator-worker/migrations/0004_add_group_archival.sql:1`, `packages/cloudflare-coordinator-worker/migrations/0005_add_scope_memberships.sql:1-44`, `packages/cloudflare-coordinator-worker/migrations/0006_add_scope_membership_audit_log.sql:1-24`, `packages/cloudflare-coordinator-worker/migrations/0007_add_invite_project_intent_reference.sql:1-5`, `packages/cloudflare-coordinator-worker/migrations/0008_add_project_invite_acceptance.sql:1-30`, `packages/cloudflare-coordinator-worker/migrations/0009_add_recipient_invite_kinds.sql:1-11`, `packages/cloudflare-coordinator-worker/migrations/0010_add_scope_membership_effect_receipts.sql:1-23`, `packages/cloudflare-coordinator-worker/migrations/0011_add_recipient_reviewed_intent.sql:1`, `packages/cloudflare-coordinator-worker/migrations/0012_add_enrolled_device_identity.sql:1`, and `packages/cloudflare-coordinator-worker/migrations/0013_add_invite_assigned_identity.sql:1`. The binding is declared at `packages/cloudflare-coordinator-worker/wrangler.jsonc:6-11`; the documented remote DDL/SQL command paths are `docs/cloudflare-coordinator-deployment.md:90-100` and `docs/cloudflare-coordinator-deployment.md:134-141`.

### Drizzle wrappers

Drizzle never opens a second file in this tree; each `drizzle(db, { schema })` wraps an already-open `better-sqlite3` handle. The lazy `MemoryStore.d` wrapper is at `packages/core/src/store.ts:228-232`. The other 84 construction sites are listed in Appendix A, including CLI sync, core import/ingest/sync/maintenance, viewer memory/raw-event/sync routes, and one test.

## B. Write-capable handle owners and consumers

### `MemoryStore` and the main DB handle

- Handle ownership and initialization: `packages/core/src/store.ts:211-252`.
- Local identity/actor persistence: `packages/core/src/store.ts:315-430`.
- Vector enqueue/flush over the same DB: `packages/core/src/store.ts:533-546`; concrete vector inserts/deletes/updates are in `packages/core/src/vectors.ts:147-164`, `packages/core/src/vectors.ts:418-458`, and `packages/core/src/vectors.ts:515-578`.
- Pack construction writes usage events through the supplied store handle at `packages/core/src/pack.ts:1062-1125`; prompt-pack retrieval ledger writers receive the raw handle at `packages/core/src/prompt-pack-ledger.ts:89-127`, `packages/core/src/prompt-pack-ledger.ts:249-290` and delegate mutations to `packages/core/src/retrieval-ledger.ts:1290-1566`.
- Session creation/update: `packages/core/src/store.ts:603-729`.
- Memory insertion, reference population, vector enqueue, forget/tombstone, visibility, scope, and project reassignment: `packages/core/src/store.ts:737-1006`, `packages/core/src/store.ts:1060-1101`, `packages/core/src/store.ts:1341-1417`, `packages/core/src/store.ts:1432-1582`.
- Raw-event retention, stuck-batch recovery, batch creation/claim/status/failure, flush cursor, ingest statistics, single-event and batch insertion: `packages/core/src/store.ts:1859-1882`, `packages/core/src/store.ts:1947-1981`, `packages/core/src/store.ts:2077-2623`.
- Schema/DDL reachable from the handle: `packages/core/src/schema-bootstrap.ts:55-612`, `packages/core/src/schema-bootstrap.ts:631-816`; compatibility, migration, planner-stat, and vacuum operations are in `packages/core/src/db.ts:573-1495` and `packages/core/src/maintenance/init-vacuum.ts:12-35`.
- Replication operation recording is coupled into memory/session mutations through `packages/core/src/sync-replication.ts:841-1141`; scope/ref helpers write through the supplied handle at `packages/core/src/scope-stamping.ts:88-110` and `packages/core/src/ref-populate.ts:20-65`.

### Package/process owners of a `MemoryStore` handle

- CLI commands: hook file context/ingest/inject, Codex inject, DB administration, distill, embed, raw-event enqueue, maintenance, memory, pack, recent, search, stats, and sync. Exact constructor sites are under “MemoryStore constructor sites” in Appendix A. The direct CLI mutation surfaces include raw-event enqueue (`packages/cli/src/commands/enqueue-raw-event.ts:56-110`), memory delete/remember (`packages/cli/src/commands/memory.ts:115-170`, `packages/cli/src/commands/memory.ts:227-293`), DB maintenance (`packages/cli/src/commands/db.ts:81-1044`), and peer removal (`packages/cli/src/commands/sync.ts:1098-1149`).
- MCP stdio owns one store (`packages/mcp-server/src/stdio.ts:7-19`). MCP HTTP owns one store (`packages/mcp-server/src/http.ts:89`, `packages/mcp-server/src/http.ts:214-230`). The store is passed into server/tool context (`packages/mcp-server/src/server.ts:14-31`, `packages/mcp-server/src/tool-context.ts:10-18`). The write tool path calls transaction/session/remember/forget at `packages/mcp-server/src/memory-access.ts:8-88`, registered through `packages/mcp-server/src/tools/items.ts:22-93`; distill also creates derived memories (`packages/mcp-server/src/tools/distill.ts:121-165`). Retrieval attempts/exposures are persisted through the same handle (`packages/mcp-server/src/mcp-retrieval-ledger.ts:213-310`, `packages/core/src/retrieval-ledger.ts:471-481`, `packages/core/src/retrieval-ledger.ts:1312-1566`).
- Viewer server owns a module-level shared store and creates it on startup (`packages/viewer-server/src/index.ts:48-61`). Memory mutation routes update visibility, project, and forget (`packages/viewer-server/src/routes/memory.ts:535-660`). Raw-event routes batch/single insert and update stream metadata (`packages/viewer-server/src/routes/raw-events.ts:110-160`, `packages/viewer-server/src/routes/raw-events.ts:162-377`, `packages/viewer-server/src/routes/raw-events.ts:389-484`). Sync/sharing/admin mutations use the same handle throughout `packages/viewer-server/src/routes/sync.ts`; direct Drizzle/transaction mutation anchors include `packages/viewer-server/src/routes/sync.ts:2946-3024`, `packages/viewer-server/src/routes/sync.ts:3338-3376`, `packages/viewer-server/src/routes/sync.ts:3910-3912`, `packages/viewer-server/src/routes/sync.ts:4052-4054`, `packages/viewer-server/src/routes/sync.ts:5183-5306`, `packages/viewer-server/src/routes/sync.ts:5962-6085`, `packages/viewer-server/src/routes/sync.ts:6088-6328`, `packages/viewer-server/src/routes/sync.ts:7306-7450`, and `packages/viewer-server/src/routes/sync.ts:7635-7652`.
- Viewer pack routes receive the shared write-capable store: the GET/trace routes call pack builders (`packages/viewer-server/src/routes/memory.ts:428-490`), and the POST transport writes retrieval-ledger records through `store.db` (`packages/viewer-server/src/routes/pack.ts:368-405`, `packages/viewer-server/src/routes/pack.ts:408-540`). Viewer health and stats routes also receive that store handle (`packages/viewer-server/src/routes/health.ts:4-34`, `packages/viewer-server/src/routes/stats.ts:329-495`).
- Core evaluation/maintenance owners: memory-role report, extraction replay/eval, pack eval, and all backfill/maintenance owners listed in the constructor/open appendices. The pack-eval fixture receives a store and writes its corpus at `packages/core/src/pack-eval-fixtures.ts:23-169`. Other mutation-bearing modules are `packages/core/src/maintenance-jobs.ts:58-280`, `packages/core/src/maintenance/ai-structured.ts:321-340`, `packages/core/src/maintenance/backfill-narrative.ts:72-82`, `packages/core/src/maintenance/backfill-tags.ts:98-130`, `packages/core/src/maintenance/dedup-keys.ts:148-163`, `packages/core/src/maintenance/dedup.ts:100-105`, `packages/core/src/maintenance/low-signal.ts:100-105`, `packages/core/src/maintenance/relink.ts:226-286`, `packages/core/src/maintenance/scan-secrets.ts:239-397`, `packages/core/src/ref-backfill.ts:155-171`, `packages/core/src/scope-backfill.ts:179-187`, `packages/core/src/scope-backfill.ts:402-416`, `packages/core/src/session-context-backfill.ts:208-214`, `packages/core/src/summary-dedup-backfill.ts:127-145`, and `packages/core/src/vector-migration.ts:83-92`, `packages/core/src/vector-migration.ts:317-326`.
- E2E/fixture/eval owners are enumerated separately in Appendix A. They include direct store or `connect()` writes under `e2e/seeds/load.ts`, `e2e/scripts/`, `e2e/scenarios/sharing-domains.ts`, `scripts/eval/pack-eval.ts`, and `scripts/workspace/`. Shared test helpers accept a raw write-capable handle for schema bootstrap/session/scope/memory inserts at `packages/core/src/test-utils.ts:5-25`, `packages/core/src/test-utils.ts:51-153`; the generated base DDL is consumed by bootstrap at `packages/core/src/schema-bootstrap.ts:631-660`, sourced from `packages/core/src/test-schema.generated.ts:1-9`, and regenerated by `packages/core/scripts/generate-test-schema.ts:1-29`.

### Other functions receiving the main write-capable handle

- Ingest and raw-event processing: `packages/core/src/ingest-pipeline.ts:349-581`, `packages/core/src/ingest-pipeline.ts:677-963`; `packages/core/src/raw-event-flush.ts:210-373`; `packages/core/src/raw-event-sweeper.ts:322-476`.
- Attribution/outcome/retrieval ledgers: `packages/core/src/attribution-assessment.ts:1373-1505`; `packages/core/src/outcome-evidence.ts:1174-1470`; `packages/core/src/retrieval-ledger.ts:440-481`, `packages/core/src/retrieval-ledger.ts:1290-1566`; `packages/core/src/retrieval-surface-ledger.ts:159-301`.
- Project/scope/recipient-policy mutation modules: `packages/core/src/project-scope-settings.ts:779-807`, `packages/core/src/project-scope-settings.ts:1229-1321`; `packages/core/src/recipient-policy-edges.ts:696-822`; `packages/core/src/recipient-policy-migration.ts:575-613`; `packages/core/src/recipient-policy-onboarding.ts:980-1134`; `packages/core/src/recipient-policy-reconciler.ts:189-239`, `packages/core/src/recipient-policy-reconciler.ts:449-917`; `packages/core/src/recipient-policy-reconciliation.ts:568-1049`; `packages/core/src/recipient-policy-review.ts:650-680`; `packages/core/src/coordinator-enrollment-reconciler.ts:125-276`; `packages/core/src/coordinator-enrollment-reconciliation-issues.ts:38-66`; `packages/core/src/coordinator-group-preferences.ts:85-182`; `packages/core/src/legacy-recipient-policy-projection.ts:699-986`; `packages/core/src/scope-membership-cache.ts:154-446`.
- Sharing mutation modules: `packages/core/src/share-operation.ts:249-416`, `packages/core/src/share-operation.ts:478-650`; `packages/core/src/share-provisioning.ts:423-513`, `packages/core/src/share-provisioning.ts:579-772`, `packages/core/src/share-provisioning.ts:784-845`.
- Sync mutation modules: `packages/core/src/sync-auth.ts:262-286`; `packages/core/src/sync-bootstrap.ts:228-375`, `packages/core/src/sync-bootstrap.ts:388-560`; `packages/core/src/sync-daemon.ts:94-160`, `packages/core/src/sync-daemon.ts:186-321`; `packages/core/src/sync-discovery.ts:55-234`; `packages/core/src/sync-identity.ts:212-219`, `packages/core/src/sync-identity.ts:449-578`; `packages/core/src/sync-pass.ts:290-487`, `packages/core/src/sync-pass.ts:652-1781`; `packages/core/src/sync-replication.ts:373-783`, `packages/core/src/sync-replication.ts:841-1141`, `packages/core/src/sync-replication.ts:1476-1839`, `packages/core/src/sync-replication.ts:2482-3233`, `packages/core/src/sync-replication.ts:3411-3912`; `packages/core/src/sync-retention-runner.ts:34-58`, `packages/core/src/sync-retention-runner.ts:112-260`; `packages/core/src/sync-scope-protocol.ts:204-235`.
- Additional holders of a write-capable main-store/raw-database object are the distill query/report pipeline (`packages/core/src/distill.ts:415-525`, `packages/core/src/distill.ts:993-1052`), visible-scope resolution (`packages/core/src/scope-resolution.ts:287-311`), and pack evaluation probes (`scripts/eval/lib.ts:116-151`).

### Coordinator store handles

- `BetterSqliteCoordinatorStore` owns the local coordinator handle (`packages/core/src/better-sqlite-coordinator-store.ts:876-883`). Schema creation/migration is `packages/core/src/better-sqlite-coordinator-store.ts:608-861`; all store methods, including group/device/invite/join-request/scope/grant/presence mutations, span `packages/core/src/better-sqlite-coordinator-store.ts:885-2184`.
- `D1CoordinatorStore` owns the D1 binding (`packages/core/src/d1-coordinator-store.ts:678-683`). Its audited D1 write helpers are at `packages/core/src/d1-coordinator-store.ts:497-675`; all group/device/invite/join-request/scope/grant/presence methods span `packages/core/src/d1-coordinator-store.ts:689-2270`.
- `CoordinatorStore` operations are reached through the HTTP app (`packages/core/src/coordinator-api.ts:216-2330`). The local factory is `packages/core/src/better-sqlite-coordinator-runtime.ts:15-33`; the D1 factory is `packages/core/src/d1-coordinator-runtime.ts:15-24`; the Worker binding entry is `packages/cloudflare-coordinator-worker/src/index.ts:22-39`.
- Local CLI/viewer coordinator actions repeatedly construct the local coordinator store and can also open the main DB to persist imported trust/onboarding data (`packages/core/src/coordinator-actions.ts:257-872`, `packages/core/src/coordinator-actions.ts:983-1628`, `packages/core/src/coordinator-actions.ts:1764-2110`, `packages/core/src/coordinator-actions.ts:2395-2529`). Exact constructor calls are in Appendix A.
- Cloudflare test runtimes expose additional write-capable binding/adapter handles: migration application is `packages/cloudflare-coordinator-worker/test/apply-migrations.ts:1-4`; direct `env.COORDINATOR_DB` setup and mutation sites are `packages/cloudflare-coordinator-worker/test/worker.integration.test.ts:91-121`, `packages/cloudflare-coordinator-worker/test/worker.integration.test.ts:232-249`, `packages/cloudflare-coordinator-worker/test/worker.integration.test.ts:326-342`, `packages/cloudflare-coordinator-worker/test/worker.integration.test.ts:488-503`, `packages/cloudflare-coordinator-worker/test/worker.integration.test.ts:735-750`, `packages/cloudflare-coordinator-worker/test/worker.integration.test.ts:854-931`, `packages/cloudflare-coordinator-worker/test/worker.integration.test.ts:1008-1023`, `packages/cloudflare-coordinator-worker/test/worker.integration.test.ts:1090-1121`, and `packages/cloudflare-coordinator-worker/test/worker.integration.test.ts:1209-1260`. The Node D1 adapters and their raw SQLite DDL/write setup are `packages/cloudflare-coordinator-worker/src/index.test.ts:61-142`, `packages/cloudflare-coordinator-worker/src/index.test.ts:165-393`, `packages/core/src/d1-coordinator-runtime.test.ts:51-91`, and `packages/core/src/d1-coordinator-store.test.ts:47-297`; their raw SQLite opens and `D1CoordinatorStore` constructors are also enumerated in Appendix A.

## C. Provider, credential, authentication, and backend-loader paths

### Observer configuration and provider loaders

- OpenCode provider config loader: locate/read/parse `opencode.json{c}` at `packages/core/src/observer-config.ts:102-134`.
- Codemem config path precedence and config read/write: workspace/runtime/global resolution at `packages/core/src/observer-config.ts:145-180`, env-key map at `packages/core/src/observer-config.ts:183-230`, resolver at `packages/core/src/observer-config.ts:269-435`, JSON/JSONC read at `packages/core/src/observer-config.ts:438-475`, and normalized write at `packages/core/src/observer-config.ts:477-489`.
- Built-in/custom provider discovery and model resolution: `packages/core/src/observer-config.ts:514-588`, `packages/core/src/observer-config.ts:691-770`.
- Environment and `{file:...}` placeholder expansion for provider headers/API keys: `packages/core/src/observer-config.ts:594-679`; header/base URL/API-key extraction: `packages/core/src/observer-config.ts:685-725`.
- Observer config assembly from files and environment, including runtime/model/auth/sidecar fields and auto-sidecar selection: `packages/core/src/observer-client.ts:264-425`, `packages/core/src/observer-client.ts:427-700`.
- Tier-specific backend selection: `packages/core/src/extraction-tier-routing.ts:64-358`, applied at `packages/core/src/extraction-tier-routing.ts:369-394`.
- Embedding backend loader: model selection and lazy `@xenova/transformers` dynamic import/pipeline creation at `packages/core/src/embeddings.ts:124-190`; callers write vectors through `packages/core/src/vector-migration.ts:334-365` and `packages/core/src/vectors.ts:515-578`.

### Observer credential resolution

- Third-party OpenCode OAuth/API-key cache path and JSON loader: `packages/core/src/observer-auth.ts:49-67`; provider entry/access/key/account/expiry extractors and availability probe: `packages/core/src/observer-auth.ts:68-158`.
- Codex consumer header construction from cached OAuth access/account data: `packages/core/src/observer-auth.ts:163-183`.
- External credential command loader: `packages/core/src/observer-auth.ts:185-206`.
- Credential file loader with home/env expansion: `packages/core/src/observer-auth.ts:208-257`.
- Credential cascade and cache (`explicit -> env -> oauth -> file -> command`): `packages/core/src/observer-auth.ts:263-380`; custom header auth placeholder rendering: `packages/core/src/observer-auth.ts:382-413`.
- Observer client retains resolved auth/backend state at `packages/core/src/observer-client.ts:1244-1303`; constructor/provider/runtime/custom-base/auth adapter initialization is `packages/core/src/observer-client.ts:1304-1515`; provider initialization consumes OpenCode cache, custom provider data, configured/env keys, and OAuth at `packages/core/src/observer-client.ts:1805-1879`.
- The viewer credential-probe consumer is `GET /api/observer-status` at `packages/viewer-server/src/routes/observer-status.ts:47-96`, mounted at `packages/viewer-server/src/index.ts:107-122`.

### Observer backend dispatch

- Dispatch order is Claude sidecar, Codex sidecar, OpenAI OAuth consumer, Anthropic OAuth consumer, then direct Anthropic/OpenAI HTTP (`packages/core/src/observer-client.ts:1886-1922`).
- Direct Anthropic and OpenAI-compatible HTTP paths: `packages/core/src/observer-client.ts:1929-1986`.
- OpenAI Codex consumer and Anthropic OAuth streaming paths: `packages/core/src/observer-client.ts:1989-2067`.
- Claude CLI sidecar: `packages/core/src/observer-client.ts:2083-2245`.
- Codex CLI sidecar, temporary final-message file, child-process execution, and environment handoff: `packages/core/src/observer-client.ts:2256-2497`.
- JSON/SSE fetch backends: `packages/core/src/observer-client.ts:2503-2649`.
- Instantiation/consumer sites: ingest pipeline (`packages/core/src/ingest-pipeline.ts:542`), extraction replay (`packages/core/src/extraction-replay.ts:827-833`), CLI Claude hook/distill/memory benchmark/serve/maintenance (`packages/cli/src/commands/claude-hook-ingest.ts:227`, `packages/cli/src/commands/distill.ts:156`, `packages/cli/src/commands/distill.ts:255`, `packages/cli/src/commands/memory.ts:854-884`, `packages/cli/src/commands/memory.ts:1109-1141`, `packages/cli/src/commands/serve.ts:624`, `packages/core/src/maintenance/ai-structured.ts:102-103`), and MCP distill (`packages/mcp-server/src/tools/distill.ts:137`).

### MCP HTTP authentication and credential persistence

- The CLI stdio/HTTP loader dynamically imports the MCP backends and passes database/bind/public-URL options at `packages/cli/src/commands/mcp.ts:5-73`.
- HTTP auth composition resolves host/public URL, opens `MemoryStore`, selects JSON-file or in-memory OAuth stores, resolves OIDC, and constructs the provider (`packages/mcp-server/src/http.ts:205-269`). OAuth callback, metadata, and SDK auth router are mounted at `packages/mcp-server/src/http.ts:320-433`; public-path selection and bearer preflight/middleware enforcement are `packages/mcp-server/src/http.ts:712-861`.
- In-memory OAuth client/code/access/refresh stores: `packages/mcp-server/src/oauth.ts:172-456`.
- JSON-file OAuth state store load and atomic temp-file/rename persistence: `packages/mcp-server/src/oauth.ts:468-793`; factories/default path: `packages/mcp-server/src/oauth.ts:796-817`.
- OAuth metadata, client registration, authorization, token exchange, revocation, and public URL normalization: `packages/mcp-server/src/oauth.ts:820-1069`.
- MCP SDK provider bridge for authorization, access/refresh token lifecycle, and bearer verification: `packages/mcp-server/src/provider.ts:37-300`.
- OIDC env loader/pending-state store: `packages/mcp-server/src/oidc.ts:9-13`, `packages/mcp-server/src/oidc.ts:74-120`; upstream authorization/code exchange/discovery/JWKS/ID-token verification: `packages/mcp-server/src/oidc.ts:122-303`.
- OAuth audit emitter selection: `packages/mcp-server/src/audit.ts:68-138`.

### Sync/coordinator credentials and authorization

- Device private/public key storage modes, OS keychain commands, file paths, key generation/loading/validation, and DB identity binding: `packages/core/src/sync-identity.ts:29-196`, `packages/core/src/sync-identity.ts:212-438`, `packages/core/src/sync-identity.ts:449-578`.
- Request canonicalization, Ed25519 signing/verification, auth-header construction, and nonce writes: `packages/core/src/sync-auth.ts:46-180`, `packages/core/src/sync-auth.ts:187-286`.
- Coordinator admin-secret loader for local runtime: `packages/core/src/better-sqlite-coordinator-runtime.ts:15-28`; D1 option loader: `packages/core/src/d1-coordinator-runtime.ts:9-21`; Worker environment loader: `packages/cloudflare-coordinator-worker/src/index.ts:4-7`, `packages/cloudflare-coordinator-worker/src/index.ts:22-35`.
- Coordinator admin header and signed-request authorization paths: `packages/core/src/coordinator-api.ts:50-201`; the Cloudflare Ed25519 request verifier is `packages/cloudflare-coordinator-worker/src/request-verifier.ts:1-125`; group-member authorization is wired into routes from `packages/core/src/coordinator-api.ts:407-429` onward. Sync auth/capability constants and public-key fingerprinting are `packages/core/src/sync-auth-constants.ts:1`, `packages/core/src/sync-capability.ts:1-85`, and `packages/core/src/sync-fingerprint.ts:1-6`.
- Sync/coordinator config and admin-secret resolution from config/environment is `packages/core/src/coordinator-runtime.ts:181-254`; peer request signing and coordinator calls are `packages/core/src/coordinator-runtime.ts:255-324`, `packages/core/src/coordinator-runtime.ts:695-817`. Peer sync HTTP uses `packages/core/src/sync-http-client.ts:37-103` and signing headers from sync passes. Viewer peer/bootstrap authorization, nonce persistence, coordinator-secret use, and trust binding are `packages/viewer-server/src/routes/sync.ts:2600-2838`.
- Viewer config endpoint exposes config/provider status and persists accepted config fields at `packages/viewer-server/src/routes/config.ts:24-64`, `packages/viewer-server/src/routes/config.ts:88-233`; the UI loader/form payload paths are `packages/ui/src/lib/api/config.ts:1-34`, `packages/ui/src/tabs/settings/data/config-loader.ts:1-75`, and `packages/ui/src/tabs/settings/data/collect-payload.ts:54-166`. CLI workspace config writes use `packages/cli/src/commands/config.ts:105-230`.
- Provider/runtime/credential controls are rendered and wired by `packages/ui/src/tabs/settings/components/ObserverPanel.tsx:23-272`, `packages/ui/src/tabs/settings/data/model-accessors.ts:1-42`, and `packages/ui/src/tabs/settings/lifecycle.tsx:50-216`.
- OpenCode plugin backend runner selection accepts installed CLI, pinned `npx`, source-tree Node, or custom runner paths at `packages/opencode-plugin/.opencode/plugins/codemem.js:1518-1572`, consumes runner/viewer/backend env at `packages/opencode-plugin/.opencode/plugins/codemem.js:1574-1732`, and spawns commands at `packages/opencode-plugin/.opencode/plugins/codemem.js:2230-2308`. Compatibility/update command planning is `packages/opencode-plugin/.opencode/lib/compat.js:18-153` and invocation is `packages/opencode-plugin/.opencode/plugins/codemem.js:2530-2664`; the CLI-packaged compatibility copy is `packages/cli/.opencode/lib/compat.js:1-142`. The other plugin entries re-export the main implementation (`packages/opencode-plugin/.opencode/plugins/runtime.js:1`, `packages/cli/.opencode/plugins/codemem.js:1-5`, `.opencode/plugins/codemem.js:1`).
- Claude hook command loaders invoke an installed `codemem` binary or an `npx` fallback: version-pin resolution, temporary stderr capture, and ingest dispatch are `plugins/claude/scripts/ingest-hook.sh:44-66`, `plugins/claude/scripts/ingest-hook.sh:87-136`; inject and file-context dispatch are `plugins/claude/scripts/inject-context-hook.sh:24-34`, `plugins/claude/scripts/pre-read-hook.sh:24-34`. Codex hook loaders resolve the plugin version, spawn installed/`npx` ingest, and persist a fallback spool at `plugins/codex/scripts/ingest-hook.mjs:47-105`; the prompt hook launches detached ingest and installed/`npx` injection at `plugins/codex/scripts/user-prompt-hook.mjs:88-123`.
- Setup-generated backend-loader configuration covers legacy OpenCode/Claude MCP migration and `npx codemem mcp` entries at `packages/cli/src/commands/setup.ts:67-116`, `packages/cli/src/commands/setup.ts:186-230`, `packages/cli/src/commands/setup.ts:244-280`; Codex installed/`npx` hook command selection plus MCP/hooks config persistence are `packages/cli/src/commands/setup.ts:330-425`, `packages/cli/src/commands/setup.ts:428-578`.
- Shipped host manifests expose MCP command loaders at `plugins/claude/.claude-plugin/plugin.json:8-15` and `plugins/codex/.mcp.json:1-8`; the Codex plugin version manifest read by its hook loader is `plugins/codex/.codex-plugin/plugin.json:1-5`.

## D. Sync, sharing, and import/export paths

### File import/export

- CLI registration and canonical `memory export` / `memory import` wrappers: `packages/cli/src/command-tree.ts:94-158`, registered with compatibility aliases at `packages/cli/src/command-tree.ts:160-195`.
- Export command reads via core and writes JSON to stdout/file: `packages/cli/src/commands/export-memories.ts:20-75`. Core export opens the DB and reads sessions, memories, summaries, and prompts at `packages/core/src/export-import.ts:293-377`.
- Import command reads JSON/stdin and invokes core import/dry-run: `packages/cli/src/commands/import-memories.ts:17-115`. Payload parsing is `packages/core/src/export-import.ts:379-393`; row insert helpers are `packages/core/src/export-import.ts:395-571`; transaction/import mapping/dedup is `packages/core/src/export-import.ts:573-728`.

### Peer sync and replication

- CLI surfaces: `sync attempts`, hidden daemon controls, `once`, `pair`, `doctor`, `status`, `enable`, `disable`, `peers remove`, `bootstrap`, `connect`, and coordinator group registration span `packages/cli/src/commands/sync.ts:209-1430`; the separate coordinator CLI spans `packages/cli/src/commands/coordinator.ts:76-1292`.
- Shared CLI lifecycle/address/project parsers used by the sync surface are `packages/cli/src/commands/sync-helpers.ts:1-94`.
- Viewer peer protocol routes are status, incremental ops, snapshot, and inbound ops (`packages/viewer-server/src/routes/sync.ts:4222-4664`). Viewer management/sync APIs are registered at `packages/viewer-server/src/routes/sync.ts:4671-5318`, `packages/viewer-server/src/routes/sync.ts:6088-6606`, and `packages/viewer-server/src/routes/sync.ts:7593-7652`.
- HTTP client and signed request path: `packages/core/src/sync-http-client.ts:37-103`, `packages/core/src/sync-auth.ts:46-286`.
- One-pass exchange/push/pull/bootstrap orchestration: `packages/core/src/sync-pass.ts:290-487`, `packages/core/src/sync-pass.ts:652-1124`, `packages/core/src/sync-pass.ts:1142-1781`.
- Snapshot fetch/apply/merge: `packages/core/src/sync-bootstrap.ts:79-177`, `packages/core/src/sync-bootstrap.ts:179-375`, `packages/core/src/sync-bootstrap.ts:388-560`.
- Replication op/cursor/reset, outbound filtering, inbound validation/application, prune/backfill: `packages/core/src/sync-replication.ts:373-783`, `packages/core/src/sync-replication.ts:841-1839`, `packages/core/src/sync-replication.ts:1915-2397`, `packages/core/src/sync-replication.ts:2482-3233`, `packages/core/src/sync-replication.ts:3411-3912`.
- Daemon scheduling/state/presence: `packages/core/src/sync-daemon.ts:76-321`, `packages/core/src/sync-daemon.ts:328-379`; retention scheduling: `packages/core/src/sync-retention-runner.ts:195-260`.
- Peer address/state persistence and mDNS discovery/advertising: `packages/core/src/sync-discovery.ts:41-234`, `packages/core/src/sync-discovery.ts:235-470`.
- Coordinator presence/peer lookup/trust/revocation/join-request paths: `packages/core/src/coordinator-runtime.ts:181-374`, `packages/core/src/coordinator-runtime.ts:416-817`, `packages/core/src/coordinator-runtime.ts:836-989`.
- Sync protocol constants/capability/fingerprint modules are `packages/core/src/sync-auth-constants.ts:1`, `packages/core/src/sync-bootstrap-constants.ts:1-6`, `packages/core/src/sync-capability.ts:1-85`, and `packages/core/src/sync-fingerprint.ts:1-6`.

### Sharing and coordinator

- Share intent normalization/digests: `packages/core/src/project-share-intent.ts:20-60`; invite identity normalization: `packages/core/src/project-invite-identity.ts:8-62`; enablement error contract: `packages/core/src/project-invite-acceptance.ts:8-23`.
- Coordinator invite encode/decode/link handling is `packages/core/src/coordinator-invites.ts:1-90`; membership-effect request/receipt normalization is `packages/core/src/coordinator-membership-effects.ts:1-134`; the coordinator store interface is `packages/core/src/coordinator-store-contract.ts:1-520`.
- Share operation planning/persistence/acceptance: `packages/core/src/share-operation.ts:64-416`, `packages/core/src/share-operation.ts:428-650`; lifecycle projection: `packages/core/src/share-operation-lifecycle.ts:71-150`.
- Provisioning plan, membership writes, local reassignment/mapping, step transitions, activation/cancellation: `packages/core/src/share-provisioning.ts:97-446`, `packages/core/src/share-provisioning.ts:447-772`, `packages/core/src/share-provisioning.ts:784-845`.
- Coordinator local/remote actions, invite creation/import, trust/onboarding persistence, join requests, and grants: `packages/core/src/coordinator-actions.ts:102-872`, `packages/core/src/coordinator-actions.ts:983-1628`, `packages/core/src/coordinator-actions.ts:1630-2110`, `packages/core/src/coordinator-actions.ts:2395-2529`.
- Coordinator server routes for presence/peers/scopes/approvals/invites/admin/join/bootstrap are registered at `packages/core/src/coordinator-api.ts:453-2012`; store/runtime boundaries are in Section B.
- Viewer recipient-policy, project invite/share-operation, sharing-domain mapping, actor/peer, coordinator-admin, and bootstrap-grant routes are registered at `packages/viewer-server/src/routes/sync.ts:5319-6085`, `packages/viewer-server/src/routes/sync.ts:6088-6606`, `packages/viewer-server/src/routes/sync.ts:6616-7588`, and `packages/viewer-server/src/routes/sync.ts:7593-7652`.
- UI request clients for sync status/peers/attempts/pairing are `packages/ui/src/lib/api/sync.ts:97-239`; recipient policy/invite/review is `packages/ui/src/lib/api/sync.ts:804-937`; sharing/project mapping/share operations are `packages/ui/src/lib/api/sync.ts:939-1189`; enrollment/actors/sync trigger are `packages/ui/src/lib/api/sync.ts:1192-1330`. Coordinator-admin request methods are `packages/ui/src/lib/api/coordinator-admin.ts:9-258`.
- UI orchestration surfaces are shared app state (`packages/ui/src/app-sharing.ts:1-97`), sync entry/controller/team-sync barrels (`packages/ui/src/tabs/sync/index.ts:117-318`, `packages/ui/src/tabs/sync/sync-view-controller.ts:1-44`, `packages/ui/src/tabs/sync/team-sync.ts:1-9`), project/recipient flows (`packages/ui/src/tabs/project-sharing.tsx:16-367`, `packages/ui/src/tabs/recipient-policy-invitations.tsx:365-618`, `packages/ui/src/tabs/recipient-policy-management.tsx:45-878`, `packages/ui/src/tabs/recipient-policy-sharing.tsx:1-475`, `packages/ui/src/tabs/recipient-policy-review.ts:33-108`), and the coordinator-admin barrel/lifecycle/action paths (`packages/ui/src/tabs/coordinator-admin.tsx:1-9`, `packages/ui/src/tabs/coordinator-admin/lifecycle.ts:1-269`, `packages/ui/src/tabs/coordinator-admin/data/actions.ts:1-304`).
- MCP project-scope parsing and item mutations are `packages/mcp-server/src/project-scope.ts:8-42` and `packages/mcp-server/src/tools/items.ts:22-93`.
- Cloudflare coordinator Worker entry is `packages/cloudflare-coordinator-worker/src/index.ts:22-39`.

### Repository E2E and operational paths

- E2E scenario drivers cover bootstrap, direct sync, coordinator, project sharing, sharing domains, fleet setup/readiness/cleanup, and smoke: `e2e/scenarios/bootstrap.ts:1`, `e2e/scenarios/direct-sync.ts:1`, `e2e/scenarios/coordinator.ts:1`, `e2e/scenarios/project-sharing.ts:1`, `e2e/scenarios/sharing-domains.ts:1`, `e2e/scenarios/fleet-ready.ts:1`, `e2e/scenarios/fleet-cleanup.ts:1`, `e2e/scenarios/fleet-smoke.ts:1`, `e2e/scenarios/smoke.ts:1`.
- E2E store/fixture writers are `e2e/seeds/load.ts:26-73`, `e2e/scripts/accept-discovered.ts:40-128`, `e2e/scripts/add-shared-memory.ts:27-51`, `e2e/scripts/dogfood-sharing-fixture.ts:93-306`, `e2e/scripts/project-sharing-fixture.ts:68-654`, `e2e/scripts/sharing-domain-smoke.ts:112-735`, `e2e/scripts/pin-peer.ts:39-53`, and `e2e/scripts/remove-local-peer.ts:25-33`.
- Containerized E2E orchestration that invokes CLI coordinator/setup/seed write paths is `e2e/bin/dogfood.ts:482-558`, `e2e/lib/coordinator.ts:15-65`, and `e2e/lib/seed.ts:14-40`.
- Workspace peer provisioning/read paths are `scripts/templates/workspace-codemem-bootstrap.sh:73`, `scripts/workspace/pin-peer.ts:37-52`, `scripts/workspace/read-peer-identity.ts:24-38`, and the coordinator/database setup plus bootstrap checks in `scripts/workspace/prove-workspace-codemem-bootstrap-local.sh:72-105`, `scripts/workspace/prove-workspace-codemem-bootstrap-local.sh:142-167`.

## Appendix A — exhaustive DB open/handle construction call sites

The following mechanically generated lists include all tracked runtime, repository tooling/E2E, and test call sites. Function declarations and comment-only mentions are excluded. The direct shell constructor at `scripts/templates/workspace-codemem-bootstrap.sh:73` and D1 binding sites above are outside these TypeScript/JavaScript constructor scans.

### Direct better-sqlite3 constructor sites (277 referenced lines)

#### Runtime and package source

- `packages/core/src/better-sqlite-coordinator-store.ts:867`
- `packages/core/src/db.ts:210`, `packages/core/src/db.ts:262`

#### Test code

- `packages/core/src/attribution-assessment.test.ts:437`, `packages/core/src/attribution-assessment.test.ts:4671`, `packages/core/src/attribution-assessment.test.ts:5029`
- `packages/core/src/attribution-diagnostics.test.ts:119`
- `packages/core/src/claude-hooks.test.ts:732`
- `packages/core/src/coordinator-enrollment-reconciliation-issues.test.ts:10`
- `packages/core/src/coordinator-group-preferences.test.ts:13`
- `packages/core/src/coordinator-runtime.test.ts:144`, `packages/core/src/coordinator-runtime.test.ts:189`, `packages/core/src/coordinator-runtime.test.ts:240`, `packages/core/src/coordinator-runtime.test.ts:355`, `packages/core/src/coordinator-runtime.test.ts:356`, `packages/core/src/coordinator-runtime.test.ts:462`, `packages/core/src/coordinator-runtime.test.ts:514`, `packages/core/src/coordinator-runtime.test.ts:664`, `packages/core/src/coordinator-runtime.test.ts:665`, `packages/core/src/coordinator-runtime.test.ts:766`, `packages/core/src/coordinator-runtime.test.ts:810`, `packages/core/src/coordinator-runtime.test.ts:889`, `packages/core/src/coordinator-runtime.test.ts:890`, `packages/core/src/coordinator-runtime.test.ts:891`, `packages/core/src/coordinator-runtime.test.ts:958`, `packages/core/src/coordinator-runtime.test.ts:959`
- `packages/core/src/coordinator-store.test.ts:49`, `packages/core/src/coordinator-store.test.ts:95`, `packages/core/src/coordinator-store.test.ts:169`
- `packages/core/src/db.test.ts:204`, `packages/core/src/db.test.ts:218`, `packages/core/src/db.test.ts:314`, `packages/core/src/db.test.ts:384`, `packages/core/src/db.test.ts:396`, `packages/core/src/db.test.ts:411`, `packages/core/src/db.test.ts:413`, `packages/core/src/db.test.ts:447`, `packages/core/src/db.test.ts:487`, `packages/core/src/db.test.ts:1096`, `packages/core/src/db.test.ts:1167`, `packages/core/src/db.test.ts:1183`, `packages/core/src/db.test.ts:1207`, `packages/core/src/db.test.ts:1262`, `packages/core/src/db.test.ts:1385`, `packages/core/src/db.test.ts:1419`
- `packages/core/src/dedup-key-backfill.test.ts:31`, `packages/core/src/dedup-key-backfill.test.ts:54`, `packages/core/src/dedup-key-backfill.test.ts:98`
- `packages/core/src/export-import.test.ts:15`, `packages/core/src/export-import.test.ts:136`, `packages/core/src/export-import.test.ts:170`, `packages/core/src/export-import.test.ts:213`, `packages/core/src/export-import.test.ts:233`, `packages/core/src/export-import.test.ts:283`, `packages/core/src/export-import.test.ts:295`, `packages/core/src/export-import.test.ts:306`, `packages/core/src/export-import.test.ts:323`, `packages/core/src/export-import.test.ts:336`
- `packages/core/src/extraction-eval.test.ts:269`, `packages/core/src/extraction-eval.test.ts:299`
- `packages/core/src/extraction-replay.test.ts:150`, `packages/core/src/extraction-replay.test.ts:294`, `packages/core/src/extraction-replay.test.ts:353`, `packages/core/src/extraction-replay.test.ts:432`, `packages/core/src/extraction-replay.test.ts:489`, `packages/core/src/extraction-replay.test.ts:552`, `packages/core/src/extraction-replay.test.ts:651`
- `packages/core/src/legacy-recipient-policy-projection.test.ts:164`
- `packages/core/src/maintenance-jobs.test.ts:24`, `packages/core/src/maintenance-jobs.test.ts:49`, `packages/core/src/maintenance-jobs.test.ts:84`, `packages/core/src/maintenance-jobs.test.ts:117`, `packages/core/src/maintenance-jobs.test.ts:143`, `packages/core/src/maintenance-jobs.test.ts:173`, `packages/core/src/maintenance-jobs.test.ts:198`, `packages/core/src/maintenance-jobs.test.ts:225`, `packages/core/src/maintenance-jobs.test.ts:247`, `packages/core/src/maintenance-jobs.test.ts:263`, `packages/core/src/maintenance-jobs.test.ts:273`
- `packages/core/src/maintenance.test.ts:38`, `packages/core/src/maintenance.test.ts:70`, `packages/core/src/maintenance.test.ts:116`, `packages/core/src/maintenance.test.ts:150`, `packages/core/src/maintenance.test.ts:162`, `packages/core/src/maintenance.test.ts:168`, `packages/core/src/maintenance.test.ts:182`, `packages/core/src/maintenance.test.ts:213`, `packages/core/src/maintenance.test.ts:228`, `packages/core/src/maintenance.test.ts:245`, `packages/core/src/maintenance.test.ts:263`, `packages/core/src/maintenance.test.ts:291`, `packages/core/src/maintenance.test.ts:322`, `packages/core/src/maintenance.test.ts:351`, `packages/core/src/maintenance.test.ts:395`, `packages/core/src/maintenance.test.ts:420`, `packages/core/src/maintenance.test.ts:501`, `packages/core/src/maintenance.test.ts:545`, `packages/core/src/maintenance.test.ts:593`, `packages/core/src/maintenance.test.ts:629`, `packages/core/src/maintenance.test.ts:683`, `packages/core/src/maintenance.test.ts:729`, `packages/core/src/maintenance.test.ts:755`, `packages/core/src/maintenance.test.ts:763`, `packages/core/src/maintenance.test.ts:782`, `packages/core/src/maintenance.test.ts:835`, `packages/core/src/maintenance.test.ts:882`, `packages/core/src/maintenance.test.ts:913`, `packages/core/src/maintenance.test.ts:936`, `packages/core/src/maintenance.test.ts:983`, `packages/core/src/maintenance.test.ts:1035`, `packages/core/src/maintenance.test.ts:1100`, `packages/core/src/maintenance.test.ts:1200`, `packages/core/src/maintenance.test.ts:1234`, `packages/core/src/maintenance.test.ts:1266`, `packages/core/src/maintenance.test.ts:1284`, `packages/core/src/maintenance.test.ts:1309`, `packages/core/src/maintenance.test.ts:1338`, `packages/core/src/maintenance.test.ts:1378`, `packages/core/src/maintenance.test.ts:1419`, `packages/core/src/maintenance.test.ts:1450`, `packages/core/src/maintenance.test.ts:1475`, `packages/core/src/maintenance.test.ts:1501`, `packages/core/src/maintenance.test.ts:1583`, `packages/core/src/maintenance.test.ts:1611`, `packages/core/src/maintenance.test.ts:1630`, `packages/core/src/maintenance.test.ts:1662`, `packages/core/src/maintenance.test.ts:1710`, `packages/core/src/maintenance.test.ts:1733`, `packages/core/src/maintenance.test.ts:1756`, `packages/core/src/maintenance.test.ts:1781`, `packages/core/src/maintenance.test.ts:1804`, `packages/core/src/maintenance.test.ts:1880`, `packages/core/src/maintenance.test.ts:1921`, `packages/core/src/maintenance.test.ts:1963`, `packages/core/src/maintenance.test.ts:2005`, `packages/core/src/maintenance.test.ts:2046`, `packages/core/src/maintenance.test.ts:2072`, `packages/core/src/maintenance.test.ts:2092`, `packages/core/src/maintenance.test.ts:2123`, `packages/core/src/maintenance.test.ts:2161`, `packages/core/src/maintenance.test.ts:2244`, `packages/core/src/maintenance.test.ts:2293`, `packages/core/src/maintenance.test.ts:2313`, `packages/core/src/maintenance.test.ts:2336`, `packages/core/src/maintenance.test.ts:2355`, `packages/core/src/maintenance.test.ts:2388`, `packages/core/src/maintenance.test.ts:2422`, `packages/core/src/maintenance.test.ts:2454`, `packages/core/src/maintenance.test.ts:2506`, `packages/core/src/maintenance.test.ts:2530`, `packages/core/src/maintenance.test.ts:2564`
- `packages/core/src/operational-status.test.ts:9`, `packages/core/src/operational-status.test.ts:77`, `packages/core/src/operational-status.test.ts:104`
- `packages/core/src/outcome-evidence.test.ts:171`, `packages/core/src/outcome-evidence.test.ts:742`, `packages/core/src/outcome-evidence.test.ts:2522`, `packages/core/src/outcome-evidence.test.ts:2834`
- `packages/core/src/project-scope-settings.test.ts:108`
- `packages/core/src/recipient-policy-edges.test.ts:25`, `packages/core/src/recipient-policy-edges.test.ts:637`, `packages/core/src/recipient-policy-edges.test.ts:638`
- `packages/core/src/recipient-policy-migration.test.ts:187`
- `packages/core/src/recipient-policy-onboarding.test.ts:285`, `packages/core/src/recipient-policy-onboarding.test.ts:304`, `packages/core/src/recipient-policy-onboarding.test.ts:339`, `packages/core/src/recipient-policy-onboarding.test.ts:363`, `packages/core/src/recipient-policy-onboarding.test.ts:414`, `packages/core/src/recipient-policy-onboarding.test.ts:428`, `packages/core/src/recipient-policy-onboarding.test.ts:457`, `packages/core/src/recipient-policy-onboarding.test.ts:475`, `packages/core/src/recipient-policy-onboarding.test.ts:521`, `packages/core/src/recipient-policy-onboarding.test.ts:578`, `packages/core/src/recipient-policy-onboarding.test.ts:612`, `packages/core/src/recipient-policy-onboarding.test.ts:643`, `packages/core/src/recipient-policy-onboarding.test.ts:676`, `packages/core/src/recipient-policy-onboarding.test.ts:704`, `packages/core/src/recipient-policy-onboarding.test.ts:757`, `packages/core/src/recipient-policy-onboarding.test.ts:811`
- `packages/core/src/recipient-policy-reconciler.test.ts:132`
- `packages/core/src/recipient-policy-reconciliation.test.ts:148`
- `packages/core/src/recipient-policy-review.test.ts:259`
- `packages/core/src/ref-backfill.test.ts:44`, `packages/core/src/ref-backfill.test.ts:82`, `packages/core/src/ref-backfill.test.ts:112`, `packages/core/src/ref-backfill.test.ts:145`, `packages/core/src/ref-backfill.test.ts:174`, `packages/core/src/ref-backfill.test.ts:212`, `packages/core/src/ref-backfill.test.ts:237`
- `packages/core/src/retrieval-ledger.test.ts:248`, `packages/core/src/retrieval-ledger.test.ts:738`, `packages/core/src/retrieval-ledger.test.ts:794`, `packages/core/src/retrieval-ledger.test.ts:795`, `packages/core/src/retrieval-ledger.test.ts:2443`, `packages/core/src/retrieval-ledger.test.ts:3158`, `packages/core/src/retrieval-ledger.test.ts:3204`, `packages/core/src/retrieval-ledger.test.ts:3237`
- `packages/core/src/scope-backfill.test.ts:92`
- `packages/core/src/scope-membership-cache.test.ts:70`
- `packages/core/src/session-context-backfill.test.ts:99`
- `packages/core/src/share-operation.test.ts:241`
- `packages/core/src/share-provisioning.test.ts:27`
- `packages/core/src/summary-dedup-backfill.test.ts:40`, `packages/core/src/summary-dedup-backfill.test.ts:58`, `packages/core/src/summary-dedup-backfill.test.ts:91`, `packages/core/src/summary-dedup-backfill.test.ts:108`, `packages/core/src/summary-dedup-backfill.test.ts:129`, `packages/core/src/summary-dedup-backfill.test.ts:159`
- `packages/core/src/sync-bootstrap.test.ts:114`, `packages/core/src/sync-bootstrap.test.ts:687`, `packages/core/src/sync-bootstrap.test.ts:751`
- `packages/core/src/sync-daemon.test.ts:72`, `packages/core/src/sync-daemon.test.ts:328`, `packages/core/src/sync-daemon.test.ts:419`, `packages/core/src/sync-daemon.test.ts:442`, `packages/core/src/sync-daemon.test.ts:468`, `packages/core/src/sync-daemon.test.ts:508`, `packages/core/src/sync-daemon.test.ts:557`, `packages/core/src/sync-daemon.test.ts:597`, `packages/core/src/sync-daemon.test.ts:614`, `packages/core/src/sync-daemon.test.ts:665`, `packages/core/src/sync-daemon.test.ts:711`, `packages/core/src/sync-daemon.test.ts:753`
- `packages/core/src/sync-discovery.test.ts:169`, `packages/core/src/sync-discovery.test.ts:246`, `packages/core/src/sync-discovery.test.ts:296`
- `packages/core/src/sync-mixed-scope.test.ts:53`
- `packages/core/src/sync-pass.test.ts:173`, `packages/core/src/sync-pass.test.ts:2372`, `packages/core/src/sync-pass.test.ts:2452`, `packages/core/src/sync-pass.test.ts:2516`, `packages/core/src/sync-pass.test.ts:2623`
- `packages/core/src/sync-reassign-scope.test.ts:43`, `packages/core/src/sync-reassign-scope.test.ts:126`, `packages/core/src/sync-reassign-scope.test.ts:136`, `packages/core/src/sync-reassign-scope.test.ts:159`, `packages/core/src/sync-reassign-scope.test.ts:189`, `packages/core/src/sync-reassign-scope.test.ts:202`
- `packages/core/src/sync-replication.test.ts:101`, `packages/core/src/sync-replication.test.ts:241`, `packages/core/src/sync-replication.test.ts:421`, `packages/core/src/sync-replication.test.ts:548`, `packages/core/src/sync-replication.test.ts:622`, `packages/core/src/sync-replication.test.ts:685`, `packages/core/src/sync-replication.test.ts:896`, `packages/core/src/sync-replication.test.ts:1295`, `packages/core/src/sync-replication.test.ts:1349`, `packages/core/src/sync-replication.test.ts:1686`, `packages/core/src/sync-replication.test.ts:1896`, `packages/core/src/sync-replication.test.ts:2457`, `packages/core/src/sync-replication.test.ts:4159`, `packages/core/src/sync-replication.test.ts:4307`, `packages/core/src/sync-replication.test.ts:4498`
- `packages/core/src/sync-retention-runner.test.ts:199`
- `packages/core/src/sync-scope-protocol.test.ts:141`, `packages/core/src/sync-scope-protocol.test.ts:316`
- `packages/core/src/vector-migration.test.ts:62`, `packages/core/src/vector-migration.test.ts:297`, `packages/core/src/vector-migration.test.ts:350`, `packages/core/src/vector-migration.test.ts:376`, `packages/core/src/vector-migration.test.ts:399`
- `packages/core/src/vectors.test.ts:36`, `packages/core/src/vectors.test.ts:588`, `packages/core/src/vectors.test.ts:621`, `packages/core/src/vectors.test.ts:686`, `packages/core/src/vectors.test.ts:744`
- `packages/viewer-server/src/index.test.ts:43`, `packages/viewer-server/src/index.test.ts:69`, `packages/viewer-server/src/index.test.ts:9269`, `packages/viewer-server/src/index.test.ts:14068`, `packages/viewer-server/src/index.test.ts:14118`, `packages/viewer-server/src/index.test.ts:14169`, `packages/viewer-server/src/index.test.ts:14201`, `packages/viewer-server/src/index.test.ts:14236`
- `packages/viewer-server/src/share-operation-maintenance.test.ts:165`, `packages/viewer-server/src/share-operation-maintenance.test.ts:252`, `packages/viewer-server/src/share-operation-maintenance.test.ts:1138`

### Calls to the writable connect() wrapper (162 referenced lines)

#### Runtime and package source

- `packages/cli/src/commands/claude-hook-ingest.ts:128`
- `packages/cli/src/commands/codex-hook-ingest.ts:128`
- `packages/cli/src/commands/db.ts:146`, `packages/cli/src/commands/db.ts:582`
- `packages/core/src/coordinator-actions.ts:1774`, `packages/core/src/coordinator-actions.ts:1984`, `packages/core/src/coordinator-actions.ts:2021`, `packages/core/src/coordinator-actions.ts:2054`, `packages/core/src/coordinator-actions.ts:2101`
- `packages/core/src/dedup-key-backfill.ts:213`
- `packages/core/src/export-import.ts:310`, `packages/core/src/export-import.ts:579`
- `packages/core/src/extraction-eval.ts:335`
- `packages/core/src/extraction-replay.ts:431`
- `packages/core/src/maintenance/init-vacuum.ts:12`
- `packages/core/src/maintenance/with-db.ts:17`
- `packages/core/src/ref-backfill.ts:285`
- `packages/core/src/scope-backfill.ts:724`
- `packages/core/src/session-context-backfill.ts:415`
- `packages/core/src/store.ts:237`
- `packages/core/src/summary-dedup-backfill.ts:321`
- `packages/core/src/sync-retention-runner.ts:259`
- `packages/core/src/vector-migration.ts:645`

#### E2E and repository tooling

- `e2e/scenarios/sharing-domains.ts:42`
- `e2e/scripts/add-shared-memory.ts:27`
- `e2e/scripts/fixture-summary.ts:20`
- `e2e/scripts/peer-identity.ts:4`
- `e2e/scripts/pin-peer.ts:39`
- `e2e/scripts/remove-local-peer.ts:25`
- `scripts/workspace/pin-peer.ts:37`
- `scripts/workspace/read-peer-identity.ts:24`

#### Test code

- `packages/cli/.opencode/tests/plugin-transform-hook.test.js:367`, `packages/cli/.opencode/tests/plugin-transform-hook.test.js:654`, `packages/cli/.opencode/tests/plugin-transform-hook.test.js:834`, `packages/cli/.opencode/tests/plugin-transform-hook.test.js:1507`
- `packages/cli/src/commands/claude-hook-file-context.test.ts:572`, `packages/cli/src/commands/claude-hook-file-context.test.ts:632`, `packages/cli/src/commands/claude-hook-file-context.test.ts:657`, `packages/cli/src/commands/claude-hook-file-context.test.ts:717`, `packages/cli/src/commands/claude-hook-file-context.test.ts:787`, `packages/cli/src/commands/claude-hook-file-context.test.ts:805`
- `packages/cli/src/commands/claude-hook-ingest.test.ts:15`, `packages/cli/src/commands/claude-hook-ingest.test.ts:128`, `packages/cli/src/commands/claude-hook-ingest.test.ts:178`
- `packages/cli/src/commands/codex-hook-ingest.test.ts:25`, `packages/cli/src/commands/codex-hook-ingest.test.ts:142`, `packages/cli/src/commands/codex-hook-ingest.test.ts:191`, `packages/cli/src/commands/codex-hook-ingest.test.ts:215`, `packages/cli/src/commands/codex-hook-ingest.test.ts:241`, `packages/cli/src/commands/codex-hook-ingest.test.ts:281`
- `packages/cli/src/commands/pack-ledger.test.ts:24`
- `packages/cli/src/commands/stats.test.ts:49`
- `packages/cli/src/commands/sync.test.ts:414`, `packages/cli/src/commands/sync.test.ts:458`, `packages/cli/src/commands/sync.test.ts:497`, `packages/cli/src/commands/sync.test.ts:586`
- `packages/cli/src/maintenance-worker-runtime.test.ts:20`
- `packages/cloudflare-coordinator-worker/src/index.test.ts:484`
- `packages/core/src/coordinator-actions.test.ts:81`, `packages/core/src/coordinator-actions.test.ts:316`, `packages/core/src/coordinator-actions.test.ts:1466`, `packages/core/src/coordinator-actions.test.ts:1551`, `packages/core/src/coordinator-actions.test.ts:1620`, `packages/core/src/coordinator-actions.test.ts:1716`, `packages/core/src/coordinator-actions.test.ts:1849`, `packages/core/src/coordinator-actions.test.ts:1904`, `packages/core/src/coordinator-actions.test.ts:1997`, `packages/core/src/coordinator-actions.test.ts:2077`, `packages/core/src/coordinator-actions.test.ts:2099`, `packages/core/src/coordinator-actions.test.ts:2188`, `packages/core/src/coordinator-actions.test.ts:2259`, `packages/core/src/coordinator-actions.test.ts:2279`, `packages/core/src/coordinator-actions.test.ts:2297`, `packages/core/src/coordinator-actions.test.ts:2370`, `packages/core/src/coordinator-actions.test.ts:2399`, `packages/core/src/coordinator-actions.test.ts:2443`, `packages/core/src/coordinator-actions.test.ts:2565`, `packages/core/src/coordinator-actions.test.ts:2611`, `packages/core/src/coordinator-actions.test.ts:2709`, `packages/core/src/coordinator-actions.test.ts:2716`, `packages/core/src/coordinator-actions.test.ts:2887`
- `packages/core/src/coordinator-enrollment-reconciler.test.ts:18`
- `packages/core/src/d1-coordinator-runtime.test.ts:156`
- `packages/core/src/db.test.ts:57`, `packages/core/src/db.test.ts:63`, `packages/core/src/db.test.ts:69`, `packages/core/src/db.test.ts:75`, `packages/core/src/db.test.ts:82`, `packages/core/src/db.test.ts:92`, `packages/core/src/db.test.ts:117`, `packages/core/src/db.test.ts:126`, `packages/core/src/db.test.ts:161`, `packages/core/src/db.test.ts:170`, `packages/core/src/db.test.ts:175`, `packages/core/src/db.test.ts:187`, `packages/core/src/db.test.ts:188`, `packages/core/src/db.test.ts:208`, `packages/core/src/db.test.ts:223`, `packages/core/src/db.test.ts:351`, `packages/core/src/db.test.ts:375`, `packages/core/src/db.test.ts:463`, `packages/core/src/db.test.ts:1046`, `packages/core/src/db.test.ts:1178`, `packages/core/src/db.test.ts:1202`, `packages/core/src/db.test.ts:1258`
- `packages/core/src/distill.test.ts:36`
- `packages/core/src/ingest-pipeline.test.ts:1971`, `packages/core/src/ingest-pipeline.test.ts:2483`, `packages/core/src/ingest-pipeline.test.ts:3242`, `packages/core/src/ingest-pipeline.test.ts:3297`
- `packages/core/src/pack.eval.test.ts:19`
- `packages/core/src/pack.test.ts:44`, `packages/core/src/pack.test.ts:1504`, `packages/core/src/pack.test.ts:1598`, `packages/core/src/pack.test.ts:1791`, `packages/core/src/pack.test.ts:2077`, `packages/core/src/pack.test.ts:2361`
- `packages/core/src/prompt-pack-ledger.test.ts:34`
- `packages/core/src/raw-event-flush.test.ts:21`
- `packages/core/src/raw-event-sweeper.test.ts:29`
- `packages/core/src/ref-queries.test.ts:28`
- `packages/core/src/scope-regression.test.ts:34`
- `packages/core/src/search.test.ts:695`, `packages/core/src/search.test.ts:1021`, `packages/core/src/search.test.ts:1251`, `packages/core/src/search.test.ts:1481`, `packages/core/src/search.test.ts:1706`
- `packages/core/src/store.test.ts:44`
- `packages/core/src/sync-auth.test.ts:137`, `packages/core/src/sync-auth.test.ts:189`, `packages/core/src/sync-auth.test.ts:239`, `packages/core/src/sync-auth.test.ts:385`, `packages/core/src/sync-auth.test.ts:430`
- `packages/core/src/sync-daemon.test.ts:238`, `packages/core/src/sync-daemon.test.ts:250`, `packages/core/src/sync-daemon.test.ts:263`, `packages/core/src/sync-daemon.test.ts:281`, `packages/core/src/sync-daemon.test.ts:294`, `packages/core/src/sync-daemon.test.ts:308`
- `packages/core/src/sync-identity.test.ts:240`, `packages/core/src/sync-identity.test.ts:293`, `packages/core/src/sync-identity.test.ts:307`, `packages/core/src/sync-identity.test.ts:363`, `packages/core/src/sync-identity.test.ts:426`, `packages/core/src/sync-identity.test.ts:452`, `packages/core/src/sync-identity.test.ts:483`
- `packages/core/src/sync-retention-runner.test.ts:32`, `packages/core/src/sync-retention-runner.test.ts:165`
- `packages/mcp-server/src/distill.test.ts:56`
- `packages/mcp-server/src/memory-access.test.ts:62`
- `packages/viewer-server/src/index.test.ts:230`, `packages/viewer-server/src/index.test.ts:4811`, `packages/viewer-server/src/index.test.ts:4905`, `packages/viewer-server/src/index.test.ts:4974`, `packages/viewer-server/src/index.test.ts:5089`, `packages/viewer-server/src/index.test.ts:5181`, `packages/viewer-server/src/index.test.ts:5267`, `packages/viewer-server/src/index.test.ts:5340`, `packages/viewer-server/src/index.test.ts:5820`, `packages/viewer-server/src/index.test.ts:5982`, `packages/viewer-server/src/index.test.ts:6059`, `packages/viewer-server/src/index.test.ts:6413`

### Calls through an injected connectDb alias (3 referenced lines)

#### Runtime and package source

- `packages/core/src/sync-daemon.ts:274`, `packages/core/src/sync-daemon.ts:360`
- `packages/core/src/sync-identity.ts:215`

### Calls to connectCoordinator() (4 referenced lines)

#### Runtime and package source

- `packages/core/src/better-sqlite-coordinator-store.ts:882`

#### Test code

- `packages/cloudflare-coordinator-worker/src/index.test.ts:90`
- `packages/core/src/d1-coordinator-runtime.test.ts:79`
- `packages/core/src/d1-coordinator-store.test.ts:179`

### Read-only open calls (2 referenced lines)

#### Runtime and package source

- `packages/cli/src/commands/status.ts:356`
- `packages/core/src/maintenance/with-db.ts:36`

### MemoryStore constructor sites (120 referenced lines)

#### Runtime and package source

- `packages/cli/src/commands/claude-hook-file-context.ts:269`, `packages/cli/src/commands/claude-hook-file-context.ts:310`
- `packages/cli/src/commands/claude-hook-ingest.ts:237`
- `packages/cli/src/commands/claude-hook-inject.ts:134`
- `packages/cli/src/commands/codex-hook-inject.ts:125`
- `packages/cli/src/commands/db.ts:275`, `packages/cli/src/commands/db.ts:443`, `packages/cli/src/commands/db.ts:504`, `packages/cli/src/commands/db.ts:664`, `packages/cli/src/commands/db.ts:717`, `packages/cli/src/commands/db.ts:758`, `packages/cli/src/commands/db.ts:806`, `packages/cli/src/commands/db.ts:856`, `packages/cli/src/commands/db.ts:901`, `packages/cli/src/commands/db.ts:954`, `packages/cli/src/commands/db.ts:1003`
- `packages/cli/src/commands/distill.ts:357`
- `packages/cli/src/commands/embed.ts:60`
- `packages/cli/src/commands/enqueue-raw-event.ts:90`
- `packages/cli/src/commands/maintenance.ts:48`
- `packages/cli/src/commands/memory.ts:73`, `packages/cli/src/commands/memory.ts:115`, `packages/cli/src/commands/memory.ts:170`, `packages/cli/src/commands/memory.ts:284`
- `packages/cli/src/commands/pack.ts:296`
- `packages/cli/src/commands/recent.ts:34`
- `packages/cli/src/commands/search.ts:37`
- `packages/cli/src/commands/stats.ts:61`
- `packages/cli/src/commands/sync.ts:222`, `packages/cli/src/commands/sync.ts:293`, `packages/cli/src/commands/sync.ts:387`, `packages/cli/src/commands/sync.ts:661`, `packages/cli/src/commands/sync.ts:835`, `packages/cli/src/commands/sync.ts:983`, `packages/cli/src/commands/sync.ts:1060`, `packages/cli/src/commands/sync.ts:1105`, `packages/cli/src/commands/sync.ts:1193`
- `packages/cli/src/maintenance-worker-runtime.ts:166` (`MemoryStoreCtor` alias)
- `packages/core/src/maintenance/memory-role-report.ts:212`
- `packages/mcp-server/src/http.ts:226`
- `packages/mcp-server/src/stdio.ts:17`
- `packages/viewer-server/src/index.ts:57`

#### E2E and repository tooling

- `e2e/scripts/accept-discovered.ts:40`
- `e2e/scripts/coordinator-status.ts:26`
- `e2e/scripts/dogfood-sharing-fixture.ts:292`
- `e2e/scripts/project-sharing-fixture.ts:631`
- `e2e/scripts/sharing-domain-smoke.ts:120`
- `e2e/seeds/load.ts:64`
- `scripts/eval/pack-eval.ts:76`

#### Test code

- `e2e/scripts/dogfood-sharing-fixture.test.ts:30`
- `packages/cli/.opencode/tests/plugin-transform-hook.test.js:370`, `packages/cli/.opencode/tests/plugin-transform-hook.test.js:657`, `packages/cli/.opencode/tests/plugin-transform-hook.test.js:838`
- `packages/cli/src/commands/alias-deprecation.test.ts:18`
- `packages/cli/src/commands/claude-hook-file-context.test.ts:606`, `packages/cli/src/commands/claude-hook-file-context.test.ts:667`, `packages/cli/src/commands/claude-hook-file-context.test.ts:720`
- `packages/cli/src/commands/db.test.ts:105`, `packages/cli/src/commands/db.test.ts:124`, `packages/cli/src/commands/db.test.ts:146`, `packages/cli/src/commands/db.test.ts:161`, `packages/cli/src/commands/db.test.ts:179`, `packages/cli/src/commands/db.test.ts:270`, `packages/cli/src/commands/db.test.ts:297`, `packages/cli/src/commands/db.test.ts:333`
- `packages/cli/src/commands/memory.test.ts:425`, `packages/cli/src/commands/memory.test.ts:445`, `packages/cli/src/commands/memory.test.ts:465`
- `packages/cli/src/commands/pack-ledger.test.ts:27`
- `packages/cli/src/commands/serve.test.ts:385`
- `packages/cli/src/commands/stats.test.ts:62`, `packages/cli/src/commands/stats.test.ts:114`
- `packages/cli/src/commands/sync.test.ts:388`, `packages/cli/src/commands/sync.test.ts:417`, `packages/cli/src/commands/sync.test.ts:461`, `packages/cli/src/commands/sync.test.ts:500`
- `packages/core/src/distill.test.ts:39`
- `packages/core/src/ingest-pipeline.test.ts:1974`, `packages/core/src/ingest-pipeline.test.ts:2486`, `packages/core/src/ingest-pipeline.test.ts:3245`, `packages/core/src/ingest-pipeline.test.ts:3300`
- `packages/core/src/pack.eval.test.ts:22`
- `packages/core/src/pack.test.ts:47`, `packages/core/src/pack.test.ts:1507`, `packages/core/src/pack.test.ts:1601`, `packages/core/src/pack.test.ts:1794`, `packages/core/src/pack.test.ts:2080`, `packages/core/src/pack.test.ts:2364`
- `packages/core/src/prompt-pack-ledger.test.ts:37`, `packages/core/src/prompt-pack-ledger.test.ts:240`
- `packages/core/src/raw-event-flush.test.ts:24`
- `packages/core/src/raw-event-sweeper.test.ts:32`
- `packages/core/src/ref-queries.test.ts:31`
- `packages/core/src/retrieval-ledger.test.ts:2828`, `packages/core/src/retrieval-ledger.test.ts:2873`, `packages/core/src/retrieval-ledger.test.ts:3168`, `packages/core/src/retrieval-ledger.test.ts:3226`
- `packages/core/src/scope-regression.test.ts:37`
- `packages/core/src/search.test.ts:698`, `packages/core/src/search.test.ts:1024`, `packages/core/src/search.test.ts:1254`, `packages/core/src/search.test.ts:1484`, `packages/core/src/search.test.ts:1709`
- `packages/core/src/store.test.ts:48`, `packages/core/src/store.test.ts:266`, `packages/core/src/store.test.ts:281`, `packages/core/src/store.test.ts:389`, `packages/core/src/store.test.ts:403`, `packages/core/src/store.test.ts:471`, `packages/core/src/store.test.ts:986`, `packages/core/src/store.test.ts:1009`, `packages/core/src/store.test.ts:2177`, `packages/core/src/store.test.ts:2210`, `packages/core/src/store.test.ts:2224`, `packages/core/src/store.test.ts:2235`
- `packages/core/src/vectors.test.ts:708`
- `packages/mcp-server/src/distill.test.ts:59`
- `packages/mcp-server/src/memory-access.test.ts:68`
- `packages/viewer-server/src/index.test.ts:50`, `packages/viewer-server/src/index.test.ts:72`

### BetterSqliteCoordinatorStore constructor sites (36 referenced lines)

#### Runtime and package source

- `packages/core/src/better-sqlite-coordinator-runtime.ts:32`
- `packages/core/src/coordinator-actions.ts:281`, `packages/core/src/coordinator-actions.ts:321`, `packages/core/src/coordinator-actions.ts:359`, `packages/core/src/coordinator-actions.ts:397`, `packages/core/src/coordinator-actions.ts:429`, `packages/core/src/coordinator-actions.ts:462`, `packages/core/src/coordinator-actions.ts:518`, `packages/core/src/coordinator-actions.ts:585`, `packages/core/src/coordinator-actions.ts:633`, `packages/core/src/coordinator-actions.ts:686`, `packages/core/src/coordinator-actions.ts:756`, `packages/core/src/coordinator-actions.ts:789`, `packages/core/src/coordinator-actions.ts:872`, `packages/core/src/coordinator-actions.ts:1009`, `packages/core/src/coordinator-actions.ts:1159`, `packages/core/src/coordinator-actions.ts:1218`, `packages/core/src/coordinator-actions.ts:1264`, `packages/core/src/coordinator-actions.ts:1305`, `packages/core/src/coordinator-actions.ts:1346`, `packages/core/src/coordinator-actions.ts:1486`, `packages/core/src/coordinator-actions.ts:2416`, `packages/core/src/coordinator-actions.ts:2453`, `packages/core/src/coordinator-actions.ts:2486`, `packages/core/src/coordinator-actions.ts:2523`

#### Test code

- `packages/core/src/coordinator-actions.test.ts:497`, `packages/core/src/coordinator-actions.test.ts:564`, `packages/core/src/coordinator-actions.test.ts:1170`, `packages/core/src/coordinator-actions.test.ts:1398`
- `packages/core/src/coordinator-api.test.ts:2711`, `packages/core/src/coordinator-api.test.ts:2728`, `packages/core/src/coordinator-api.test.ts:2821`
- `packages/core/src/coordinator-store.test.ts:12`, `packages/core/src/coordinator-store.test.ts:71`, `packages/core/src/coordinator-store.test.ts:108`, `packages/core/src/coordinator-store.test.ts:183`

### D1CoordinatorStore constructor sites (17 referenced lines)

#### Runtime and package source

- `packages/core/src/d1-coordinator-runtime.ts:23`

#### Test code

- `packages/cloudflare-coordinator-worker/src/index.test.ts:415`, `packages/cloudflare-coordinator-worker/src/index.test.ts:478`
- `packages/core/src/d1-coordinator-runtime.test.ts:91`, `packages/core/src/d1-coordinator-runtime.test.ts:146`
- `packages/core/src/d1-coordinator-store.test.ts:199`, `packages/core/src/d1-coordinator-store.test.ts:242`, `packages/core/src/d1-coordinator-store.test.ts:369`, `packages/core/src/d1-coordinator-store.test.ts:370`, `packages/core/src/d1-coordinator-store.test.ts:420`, `packages/core/src/d1-coordinator-store.test.ts:421`, `packages/core/src/d1-coordinator-store.test.ts:471`, `packages/core/src/d1-coordinator-store.test.ts:472`, `packages/core/src/d1-coordinator-store.test.ts:507`, `packages/core/src/d1-coordinator-store.test.ts:549`, `packages/core/src/d1-coordinator-store.test.ts:561`, `packages/core/src/d1-coordinator-store.test.ts:562`

### Drizzle handle construction sites (85 referenced lines)

#### Runtime and package source

- `packages/cli/src/commands/sync.ts:224`, `packages/cli/src/commands/sync.ts:297`, `packages/cli/src/commands/sync.ts:543`, `packages/cli/src/commands/sync.ts:663`, `packages/cli/src/commands/sync.ts:837`, `packages/cli/src/commands/sync.ts:1062`, `packages/cli/src/commands/sync.ts:1107`, `packages/cli/src/commands/sync.ts:1197`
- `packages/core/src/coordinator-group-preferences.ts:89`, `packages/core/src/coordinator-group-preferences.ts:107`
- `packages/core/src/export-import.ts:593`
- `packages/core/src/ingest-pipeline.ts:368`, `packages/core/src/ingest-pipeline.ts:963`
- `packages/core/src/maintenance/reliability.ts:35`
- `packages/core/src/maintenance/relink.ts:226`
- `packages/core/src/maintenance/status.ts:12`, `packages/core/src/maintenance/status.ts:104`
- `packages/core/src/search.ts:47`
- `packages/core/src/store.ts:231`
- `packages/core/src/sync-auth.ts:268`, `packages/core/src/sync-auth.ts:284`
- `packages/core/src/sync-bootstrap.ts:364`, `packages/core/src/sync-bootstrap.ts:405`, `packages/core/src/sync-bootstrap.ts:511`
- `packages/core/src/sync-daemon.ts:95`, `packages/core/src/sync-daemon.ts:110`, `packages/core/src/sync-daemon.ts:138`, `packages/core/src/sync-daemon.ts:153`
- `packages/core/src/sync-discovery.ts:56`, `packages/core/src/sync-discovery.ts:94`, `packages/core/src/sync-discovery.ts:137`, `packages/core/src/sync-discovery.ts:182`, `packages/core/src/sync-discovery.ts:216`, `packages/core/src/sync-discovery.ts:227`
- `packages/core/src/sync-identity.ts:217`, `packages/core/src/sync-identity.ts:458`
- `packages/core/src/sync-pass.ts:327`, `packages/core/src/sync-pass.ts:357`, `packages/core/src/sync-pass.ts:1161`, `packages/core/src/sync-pass.ts:1744`, `packages/core/src/sync-pass.ts:1800`, `packages/core/src/sync-pass.ts:1837`
- `packages/core/src/sync-replication.ts:411`, `packages/core/src/sync-replication.ts:441`, `packages/core/src/sync-replication.ts:491`, `packages/core/src/sync-replication.ts:632`, `packages/core/src/sync-replication.ts:666`, `packages/core/src/sync-replication.ts:706`, `packages/core/src/sync-replication.ts:740`, `packages/core/src/sync-replication.ts:856`, `packages/core/src/sync-replication.ts:1001`, `packages/core/src/sync-replication.ts:1118`, `packages/core/src/sync-replication.ts:1174`, `packages/core/src/sync-replication.ts:1340`, `packages/core/src/sync-replication.ts:1491`, `packages/core/src/sync-replication.ts:1683`, `packages/core/src/sync-replication.ts:1765`, `packages/core/src/sync-replication.ts:1879`, `packages/core/src/sync-replication.ts:3133`, `packages/core/src/sync-replication.ts:3214`, `packages/core/src/sync-replication.ts:3418`
- `packages/core/src/sync-retention-runner.ts:201`
- `packages/core/src/sync-scope-protocol.ts:216`
- `packages/viewer-server/src/routes/memory.ts:55`, `packages/viewer-server/src/routes/memory.ts:540`, `packages/viewer-server/src/routes/memory.ts:635`
- `packages/viewer-server/src/routes/raw-events.ts:124`
- `packages/viewer-server/src/routes/sync.ts:2947`, `packages/viewer-server/src/routes/sync.ts:3273`, `packages/viewer-server/src/routes/sync.ts:4690`, `packages/viewer-server/src/routes/sync.ts:5027`, `packages/viewer-server/src/routes/sync.ts:5089`, `packages/viewer-server/src/routes/sync.ts:5106`, `packages/viewer-server/src/routes/sync.ts:5145`, `packages/viewer-server/src/routes/sync.ts:5186`, `packages/viewer-server/src/routes/sync.ts:5290`, `packages/viewer-server/src/routes/sync.ts:6103`, `packages/viewer-server/src/routes/sync.ts:6165`, `packages/viewer-server/src/routes/sync.ts:6193`, `packages/viewer-server/src/routes/sync.ts:6218`, `packages/viewer-server/src/routes/sync.ts:6270`, `packages/viewer-server/src/routes/sync.ts:6514`, `packages/viewer-server/src/routes/sync.ts:7376`, `packages/viewer-server/src/routes/sync.ts:7638`

#### Test code

- `packages/core/src/ingest-pipeline.test.ts:3314`
