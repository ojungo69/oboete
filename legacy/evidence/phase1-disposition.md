# T029(b) — Phase 1 disposition 表（実装開始条件・T053 照合の正本）

日付: 2026-08-14 / 作成: Claude Code（セキュリティ判定につき委譲なし）
入力: `codemem/write-handle-inventory.md`（T007 全数）+ `codemem/write-handle-classification.md`（T010 + 2026-08-13 補遺 F9–F11）+ A7 実施後の実地 rg 全数再列挙。
行番号は **A7 削除後の現行 tree** を基準に再照合済み（凍結 evidence の旧行番号と異なる場合は本表が優先）。

**列の意味**
- disposition: {A7削除済み / 削除(T03x) / spool・RPC置換(T041) / RPC化(T042–T044,T048) / daemon内jobs(T045) / typed無効化(T044) / daemon常駐(T033) / scan例外}
- class: 判断 #11 の mutation class。A = transactional DB（dispatcher + receipt 同一 transaction）/ B = filesystem 副作用（operation ID + durable journal）/ C = 非 spoolable maintenance（maintenance mode 直列・job ID 照会・自動再試行禁止）/ − = read または非 DB
- authority: agent-callable（agent が自動で呼んでよい）/ user-authority（HI-30/31: agent 自動実行禁止。MCP surface からは物理削除 or typed-disable、CLI = user 操作面として存置）/ −

### RPC 契約の固定（T035/T036 の入力）

以下は Unix socket 上の local API の**意味契約**である。wire envelope、version handshake、size bound は T035 が実装するが、caller と endpoint、request/response、daemon 不在時挙動の対応は本表から変更しない。全 request は schema allowlist（未知 field 拒否）、全 error は `{ error: { code, message, retryable } }` とする。

| endpoint | request schema → response schema | class / exactly-once | daemon 不在時 |
|---|---|---|---|
| `POST /v1/events` | `{ idempotencyKey, event, adapterRedaction? }` → `{ receiptId, status }` | A。receipt と event を同一 transaction。同一 key・異 payload hash = `idempotency_conflict` + quarantine | hook/adapter は T038(a) 後の同一 envelope を spool。CLI は spool 成功を表示。直接 DB fallback 禁止 |
| `POST /v1/events/batch` | `{ items: [{ idempotencyKey, event }] }` → `{ receipts[] }` | A。item 単位 receipt。同一 key・異 payload hash = conflict + quarantine | hook/adapter は T038(a) 後の同一 batch envelope を spool。直接 DB fallback 禁止 |
| `POST /v1/context/pack` | `{ requestId, context, limit?, tokenBudget?, filters?, trace? }` → `{ pack, trace?, retrievalReceiptId }` | read + A。pack 読出しと usage/retrieval ledger receipt を daemon 内で処理。同一 requestId・異 payload hash = conflict | hook inject は空 context で fail-open、CLI/MCP は typed `daemon_unavailable`、viewer は HTTP 503。DB fallback 禁止 |
| `POST /v1/search` | `{ requestId, mode, query?, repositoryPath?, ids?, memoryId?, depthBefore?, depthAfter?, includePackContext?, filters?, limit? }` → `{ items, retrievalReceiptId }` | read + A。`mode` = `search|search_index|find_by_file|recent|timeline|get_many|explain|expand`。ledger receipt は同一 transaction | CLI/MCP は typed `daemon_unavailable`、viewer は 503 |
| `POST /v1/retrieval/file-context` | `{ attemptId, startedAt, completedAt, retrievalStatus, candidateIds?, candidateCount?, selectedIds?, failureCode?, failureStage?, project?, repositoryPath?, sourceSessionId? }` → `{ recorded, inserted?|errorCode? }` | A。file-context retrieval attempt と exposure を daemon transaction 内で記録 | hook は retrieval 本体を継続し、ledger failure を本文取得失敗へ昇格しない |
| `POST /v1/retrieval/file-context/delivery` | `{ attemptId, status:"handed_off"|"failed" }` → `{ updated, errorCode? }` | A。上記 attempt の delivery 状態だけを更新 | hook は typed failure を記録し、Agent の file read を阻害しない |
| `GET /v1/memories/:id` | path `id` + `{ requestId, project?, kind? }` → `{ item|null, retrievalReceiptId }` | read + A（retrieval ledger） | CLI/MCP は typed `daemon_unavailable`、viewer は 503 |
| `POST /v1/memories/record` | `{ idempotencyKey, kind, title, body, confidence?, project?, adapterRedaction? }` → `{ receiptId, memoryId }` | A。memory + receipt 同一 transaction。同一 key・異 payload hash = conflict + quarantine | spoolable caller のみ spool。直接 DB fallback 禁止 |
| `DELETE /v1/memories/:id` | path `id` + `{ requestId, expectedRevision? }` → `{ receiptId, status }` | A。**user-authority**。遅延 replay を避けるため spool 不可 | typed `daemon_unavailable`、自動 retry・DB fallback とも禁止 |
| `GET /v1/checkpoints` | query `{ project?, state?, limit? }` → `{ checkpoints[] }` | read | typed `daemon_unavailable` / viewer 503 |
| `GET /v1/health` / `GET /v1/doctor` | body なし → `{ status, instanceId, protocolVersion, diagnostics? }` | read | CLI `status` だけ `{ status: "not_running" }` として exit 0。他 caller は typed `daemon_unavailable` |
| `GET /v1/view` | `{ collection, sessionId?, project?, kind?, scope?, limit?, offset? }`。`collection` は sessions/projects/memories/observations/summaries/session/memory/artifacts/raw-events/raw-events-status/stats/runtime/usage/observer-status/config の allowlist → `{ status, body }` | read。collection と viewer route は §6 で 1 対 1 | viewer は `{ error: { code: "daemon_unavailable", ... } }` + HTTP 503。local DB fallback 禁止 |
| `POST /v1/viewer/auth/nonce` / `POST /v1/viewer/auth/exchange` / `POST /v1/viewer/auth/verify` / `POST /v1/viewer/auth/logout` | nonce=`{}`→`{nonce,expiresAt}`、exchange=`{nonce}`→`{session}`、verify=`{bearer?,session?}`→`{authenticated}`、logout=`{session}`→`{loggedOut}` | Unix DAC 内のviewer認証。nonce 60s単回、session 12h/上限8/daemon再起動失効 | browser交換失敗またはdata API 503。Bearer/nonce/sessionのDB fallback・URL query・log出力禁止 |
| `POST /v1/operations/export` | `{ operationId, payloadHash, outputPath, filters }` → `{ operationId, state }` | B。client が UUID を生成。`prepared→writing→verified→committed` | typed `daemon_unavailable`。自動 retry・client 側 export fallback 禁止 |
| `POST /v1/operations/import` | `{ operationId, payloadHash, inputPath, remapProject?, dryRun? }` → `{ operationId, state }` | B。client UUID。destructive 時 `prepared→backup_verified→applying→committed` | typed `daemon_unavailable`。自動 retry・client 側 import fallback 禁止 |
| `GET /v1/backup/list` | `{}` → `{ backups[] }` | read。各 artifact / manifest / owner mode を再検証 | typed `daemon_unavailable`。client 側 filesystem fallback 禁止 |
| `POST /v1/backup/create` | `{ operationId, payloadHash, reason }` → `{ operationId, backupId, state:"completed", artifactSha256, manifestHash }` | B。client UUID。完成 artifact + manifest sidecar が durable result。同一 ID・同一 hash は検証後 replay、異 hash は副作用前 conflict | typed `daemon_unavailable`。自動 retry・client 側 backup fallback 禁止 |
| `POST /v1/backup/verify` | `{ backupId }` → `{ backupId, valid, manifestHash, diagnostics[] }` | read（artifact 非改変） | typed `daemon_unavailable`。自動 retry なし |
| `POST /v1/backup/restore` | `{ operationId, payloadHash, backupId }` → `{ operationId, backupId, pointer, artifactSha256, manifestHash, restartRequired:true }` | B。client UUID。fresh staging 検証後、storage journal `prepared→switched→committed` で current pointer を切替。旧 artifact は保持 | typed `daemon_unavailable`。自動 retry・直接 restore 禁止 |
| `GET /v1/operations/:id` | path `id` → `{ operationId, payloadHash, state, result?, error? }` | B の結果再取得。全 B endpoint で同一 operationId + 同一 hash は現在/最終結果を返し、異 hash は副作用前に `idempotency_conflict` | typed `daemon_unavailable` |
| `POST /v1/jobs` | `{ kind, args, dryRun? }` → `{ jobId, state }` | C。daemon maintenance mode 内で直列、`maxAttempts=1` | typed `daemon_unavailable`。応答不明を含め client 自動 retry 禁止 |
| `GET /v1/jobs` / `GET /v1/jobs/:id` | query `{ kind?, state?, submittedAfter? }` / path `id` → `{ jobs[] }` / `{ job }` | C の照会・結果再取得。再実行は user が新 job を明示 trigger | typed `daemon_unavailable` |
| `GET /v1/processing-jobs/:id` | path `id` → `{ job }` | read。doctor が exact `retry_exhausted` job の bounded state/fingerprint/attempt/claim snapshot だけを表示 | typed `daemon_unavailable`。DB fallback 禁止 |
| `POST /v1/processing-jobs/:id/doctor-retry` | path `id` + `{ producerReceiptId, expectedRole, expectedProviderFingerprint, expectedManifestFingerprint, expectedAttemptCount, expectedClaimGeneration }` → `{ disposition, grantState }` | A。**user-authority**。表示 snapshot を transaction 内で再検証し、exact job に one-shot grant を 1 件だけ作成 | typed `daemon_unavailable`。応答不明を含め自動 retry・DB fallback 禁止 |

Class B の `payloadHash` は endpoint ごとの allowlisted payload に対する SHA-256。export/import は operation journal と `GET /v1/operations/:id`、backup create は artifact + manifest sidecar、restore は deterministic staging 名 + storage journal により同じ結果へ収束する。Class C は job ID を受領した後だけ照会し、lost response 時は `GET /v1/jobs` で探索してから user が判断する。

## §1 A7 で削除済み（2026-08-13、`9deb8e2..f1e84cf`・290 deleted paths）

| 対象 | 備考 |
|---|---|
| core: sync-* / coordinator-* / d1-* / better-sqlite-coordinator-* / share-* / recipient-policy-* / legacy-recipient-policy-projection / scope-membership-* / project-share-intent / internal/cloudflare-coordinator | モジュール + テスト全削除。schema DDL（sync/share 系テーブル定義）は非改変で残置 — 削除は Phase 2 の schema 再設計時 |
| cli: commands/sync.ts / sync-helpers.ts / coordinator.ts、command-tree 登録、db prune-replication-ops、serve の sync daemon/listener 配線、maintenance-worker の SyncRetentionRunner | |
| mcp-server: http.ts / oauth.ts / oidc.ts / provider.ts + package.json "./http" export + express 系 deps + mcp http サブコマンド | Core 1.0 = stdio のみ（§18） |
| viewer-server: routes/sync.ts（7.6k 行）/ createSyncApp / share-operation-maintenance.test / getSyncRuntimeStatus・syncRequestRateLimit 配線 | |
| packages/ui: tabs/sync/ / coordinator-admin / devices / project-sharing / recipient-policy-* / projects（データ源 = 削除 API のため）/ app-sharing / lib/api/sync・coordinator-admin / SyncPanel / index.html 該当タブ | frontend 例外につき Claude Code 自ら |
| packages/cloudflare-coordinator-worker 全体（D1 migrations 13 本含む）/ e2e/ 全体 / deploy/ 全体 / docker-compose.dogfood.yml / docker-compose.e2e.yml | |
| store.ts / ingest-pipeline / summary-dedup-backfill / project-scope-settings の recordReplicationOp・recordAccessCleanupOp 呼び出し、store.ts の rebootstrap mutation guard | ローカル書込み本体は温存 |
| A7 由来の孤立 surface: core project-scope-settings / project-invite-identity・acceptance / address-utils / sync 専用 vector migration API、CLI config workspace・sync status、MCP audit、UI の未使用 sharing primitives と依存 | caller 0 を rg + GitNexus で確認後に削除。legacy scope authorization と schema DDL は fail-closed 境界として残置 |

## §2 削除予定（auth / sidecar / template — Claude Code 自ら）

| # | 対象（現行 file:line） | disposition | class | authority |
|---|---|---|---|---|
| F1 | core/observer-client.ts `_callAnthropicConsumer` + dispatch | 削除(T030) | − | − |
| F2 | core/observer-client.ts `_callCodexConsumer` + `buildCodexHeaders` | 削除(T030) | − | − |
| F4 | core/observer-auth.ts OpenCode cache 読出し（:49-158 相当） | 削除(T030) | − | − |
| F8 | core/observer-auth.ts credential command loader（:185-206 相当） | 削除(T030)。cascade = explicit→env→file | − | − |
| F3 | core/observer-client.ts Claude sidecar 3 関数 + dispatch | 削除(T031) | − | − |
| F9 | core/observer-client.ts Codex sidecar 3 関数 + dispatch + 分岐（補遺） | 削除(T031) | − | − |
| F7 | scripts/templates/workspace-codemem-bootstrap.sh + scripts/workspace-bootstrap-template.sh + scripts/workspace/（pin-peer / read-peer-identity / prove-*） | 削除(T032)（sync peer 系 repo tooling ごと） | − | − |

## §3 hook 経路 → spool / RPC 置換（T041。全経路 T038(a) 前処理必須）

| # | 対象（現行） | endpoint / schema / daemon 不在時 | disposition | class | authority |
|---|---|---|---|---|---|
| F5 | cli/claude-hook-ingest.ts:128 `connect(dbPath)`（directEnqueue） | `POST /v1/events`。共通 schema。RPC cutoff 後は同 envelope を spool | spool のみ化(T041) | A（idempotencyKey は RPC/spool 分岐前に確定） | agent-callable |
| F11 | cli/claude-hook-ingest.ts:237 `new MemoryStore`（flushBoundaryRawEvents） | endpoint なし。daemon の spool importer が `POST /v1/events/batch` と同じ dispatcher に投入 | 除去 + spool(T041) | A | agent-callable |
| F6 | cli/codex-hook-ingest.ts:128 `connect(dbPath)` | `POST /v1/events`。共通 schema。RPC cutoff 後は同 envelope を spool | spool のみ化(T041) | A | agent-callable |
| F5' | cli/claude-hook-inject.ts:134 `new MemoryStore`（buildLocalPack） | `POST /v1/context/pack` `{ requestId,context,limit,tokenBudget,filters,trace:false }`。不在 = 空 context + exit 0 | RPC read + daemon 内 usage ledger(T041) | read + A | agent-callable |
| F10 | cli/codex-hook-inject.ts:125 `new MemoryStore`（buildLocalPack） | `POST /v1/context/pack` `{ requestId,context,limit,tokenBudget,filters,trace:false }`。不在 = 空 context + exit 0 | RPC read + daemon 内 usage ledger(T041) | read + A | agent-callable |
| F6' | cli/claude-hook-file-context.ts `queryByFile` | `POST /v1/search` `{ requestId,mode:"find_by_file",repositoryPath,filters,limit }`。不在 = additionalContext なし + exit 0 | RPC read + daemon 内 retrieval ledger(T041) | read + A | agent-callable |

## §4 CLI → RPC 化 / typed 無効化（T044）

| 対象（現行） | endpoint / schema / daemon 不在時 | disposition | class | authority |
|---|---|---|---|---|
| enqueue-raw-event.ts | `POST /v1/events` 共通 schema。不在 = T038(a) 後 spool 成功なら exit 0 | RPC + spool fail-over | A | agent-callable |
| memory remember | `POST /v1/memories/record` 共通 schema。不在 = spool（idempotencyKey は分岐前確定） | RPC + spool fail-over | A | agent-callable |
| memory forget | `DELETE /v1/memories/:id` 共通 schema。不在 = typed error、spool/自動 retry なし | RPC 化 | A | **user-authority**（CLI のみ。MCP には出さない） |
| memory show | `GET /v1/memories/:id`。不在 = typed error | RPC read | read + A ledger | user-authority |
| memory inject | `POST /v1/context/pack` `{ requestId,context,limit,tokenBudget,filters,trace:false }`。不在 = typed error | RPC read | read + A ledger | user-authority |
| pack / pack trace | `POST /v1/context/pack` の `trace:false|true`。不在 = typed error | RPC read | read + A ledger | user-authority |
| prompt-pack-ledger | 独立 endpoint を廃止。pack/search/get の daemon 内 ledger receipt に統合 | typed 無効化後に command 削除 | − | − |
| recent | `POST /v1/search` `{ requestId,mode:"recent",filters,limit }`。不在 = typed error | RPC read | read + A ledger | user-authority |
| search | `POST /v1/search` `{ requestId,mode:"search",query,filters,limit }`。不在 = typed error | RPC read | read + A ledger | user-authority |
| stats | `GET /v1/view` `{collection:"stats"}`。不在 = typed error | RPC read | read | user-authority |
| status | `GET /v1/health` + `GET /v1/doctor`。不在 = `not_running` / DB `unknown` 表示 + exit 0 | RPC read（readonly 例外なし） | read | user-authority |
| memory role-report / role-compare / artifact-report / relink-report / relink-plan | `POST /v1/jobs` kind = `report.memory-role|report.role-compare|report.artifact|report.relink|plan.relink` → `GET /v1/jobs/:id`。不在/応答不明 = 自動 retry なし | daemon jobs(T045) | C | user-authority |
| memory extraction-report | `POST /v1/jobs` kind = `report.extraction` → job result。不在/応答不明 = 自動 retry なし | daemon job(T045) | C | user-authority |
| memory extraction-replay / extraction-benchmark | endpoint なし。不在時を含め常に `{ code:"feature_unavailable", phase:6 }` | typed 無効化（Phase 6 で再設計） | − | user-authority |
| db raw-events-status / raw-events-gate / size-report | `POST /v1/jobs` kind = `report.raw-events|gate.raw-events|report.db-size`, `dryRun:true` → job result。不在/応答不明 = 自動 retry なし | daemon jobs | C | user-authority |
| db init / vacuum / prune-raw-events / raw-events-retry | `POST /v1/jobs` kind = `db.init|db.vacuum|raw-events.prune|raw-events.retry` → job ID。不在/応答不明 = 自動 retry なし | daemon maintenance mode(T045/T046) | C | **user-authority** |
| db rename-project / normalize-projects | `POST /v1/jobs` kind = `projects.rename|projects.normalize`, `{ dryRun, ...args }`。不在/応答不明 = 自動 retry なし | daemon maintenance mode | C | **user-authority** |
| db backfill-tags / backfill-dedup-keys / backfill-narrative / ai-backfill-structured | `POST /v1/jobs` kind = `tags.backfill|dedup-keys.backfill|narrative.backfill|structured.backfill`。不在/応答不明 = 自動 retry なし | daemon jobs | C | **user-authority** |
| db prune-observations / prune-memories / dedup-memories / scan-secrets | `POST /v1/jobs` kind = `observations.prune|memories.prune|memories.dedup|secrets.scan`。不在/応答不明 = 自動 retry なし | daemon maintenance mode | C | **user-authority** |
| export-memories.ts + core/export-import.ts export | `POST /v1/operations/export` → `GET /v1/operations/:id`。共通 B schema/state。不在 = typed error | daemon RPC export | B | user-authority |
| import-memories.ts + core/export-import.ts import | `POST /v1/operations/import` → result endpoint。destructive = backup verified 後のみ。不在 = typed error | daemon RPC import(T047) | B | **user-authority** |
| distill.ts / embed.ts | endpoint なし。`{ code:"feature_unavailable", phase:6|7 }` | typed 無効化 stub（判断 #13） | − | user-authority |
| maintenance status | `GET /v1/jobs` allowlisted filter。不在 = typed error | RPC jobs 照会 | read | user-authority |
| maintenance worker | endpoint なし | command 削除。daemon 内へ吸収(T045) | − | − |
| config.ts（config file 書込みのみ・DB 非関与） | RPC なし。local config file の allowlisted read/write | DB-open 対象外。設定キー整理は T030/T038 | − | user-authority |

## §5 MCP stdio（T042）

| 対象 | endpoint / schema / daemon 不在時 | disposition | class | authority |
|---|---|---|---|---|
| mcp-server/stdio.ts `new MemoryStore` | raw handle を持たない RPC client。全 tool で不在 = MCP typed `daemon_unavailable`、DB fallback なし | RPC client 化(T042) | − | agent-callable |
| memory_get | `GET /v1/memories/:id` 共通 schema | allowlist に保持 | read + A ledger | agent-callable |
| memory_get_observations | `POST /v1/search` `{ requestId,mode:"get_many",ids,filters }` | allowlist に保持 | read + A ledger | agent-callable |
| memory_search / memory_recent / memory_timeline / memory_expand / memory_explain / memory_search_index | `POST /v1/search`。tool ごと `mode` を固定し、tool schema から allowlisted field のみ写像 | read allowlist | read + A ledger | agent-callable |
| memory_pack | `POST /v1/context/pack` 共通 schema | read allowlist | read + A ledger | agent-callable |
| memory_remember | `POST /v1/memories/record`。idempotencyKey は tool requestId から RPC/spool 分岐前に導出。不在 = T038(a) 後 spool | write allowlist | A | agent-callable |
| memory_status（T042 で追加） | `GET /v1/health`。不在 = typed `daemon_unavailable` | status allowlist | read | agent-callable |
| memory_schema | DB/RPC なし。固定 protocol schema のみ返す | local static tool として保持 | − | agent-callable |
| memory_forget / confirm / pin / unpin / retract / mark_wrong / destructive bulk 相当 | endpoint なし。登録自体を物理削除し、未知 tool として拒否。`DELETE /v1/memories/:id` は CLI user-authority 専用 | **tool-list 検査 + 拒否試験**（HI-30/31・§18.5。正式 surface = Phase 5） | − | user-authority（MCP 不可） |
| memory_distill_candidates / memory_learn | endpoint なし。tool 登録削除（互換応答を残す場合も typed `feature_unavailable`, phase 6 のみ） | typed 無効化(T042、正式実装 Phase 6) | − | − |
| mcp-retrieval-ledger.ts の store 書込み | 独立 event RPC は作らず、上記 read endpoint の `requestId` と daemon 内 `retrievalReceiptId` に統合 | local 書込み除去(T042) | A | agent-callable |

## §6 viewer（T043 = read-only 化）

| viewer route（現行） | daemon endpoint / schema | daemon 不在時 | disposition | class | authority |
|---|---|---|---|---|---|
| index.ts sharedStore | RPC client のみ。browser 認証後の allowlisted route だけ呼出し | 全 data API = typed JSON 503。local DB fallback なし | sharedStore 削除(T043) | − | − |
| `GET /api/health` | `GET /v1/health` | typed JSON 503 | RPC relay | read | user-authority |
| `GET /api/sessions` | `GET /v1/view` `{collection:"sessions", project?, limit?, offset?}` | typed JSON 503 | RPC relay | read | user-authority |
| `GET /api/projects` | `GET /v1/view` `{collection:"projects"}` | typed JSON 503 | RPC relay | read | user-authority |
| `GET /api/memories` | `GET /v1/view` `{collection:"memories", project?, kind?, limit?, offset?}` | typed JSON 503 | RPC relay | read | user-authority |
| `GET /api/observations` | `GET /v1/view` `{collection:"observations", project?, scope?, limit?, offset?}` | typed JSON 503 | RPC relay | read | user-authority |
| `GET /api/summaries` | `GET /v1/view` `{collection:"summaries", project?, scope?, limit?, offset?}` | typed JSON 503 | RPC relay | read | user-authority |
| `GET /api/session` | `GET /v1/view` `{collection:"session", project?}` | typed JSON 503 | RPC relay | read | user-authority |
| `GET /api/memory` | `GET /v1/view` `{collection:"memory", project?, kind?, limit?}` | typed JSON 503 | RPC relay | read | user-authority |
| `GET /api/artifacts` | `GET /v1/view` `{collection:"artifacts", sessionId?, project?}` | typed JSON 503 | RPC relay | read | user-authority |
| `GET /api/pack` | `POST /v1/context/pack` `{ requestId,context,limit,tokenBudget,filters,trace:false }` | typed JSON 503 | RPC relay | read + A ledger | user-authority |
| `POST /api/pack/trace` | `POST /v1/context/pack` の `trace:true`。working-set は filters に allowlist copy。ledger は daemon が同一 request 内で記録 | typed JSON 503 | **RPC read relay に確定**。viewer 自身は書込まない | read + A ledger | user-authority |
| `GET /api/raw-events` / `/api/raw-events/status` | `GET /v1/view` `{collection:"raw-events"|"raw-events-status"}` | typed JSON 503 | RPC relay | read | user-authority |
| `GET /api/stats` / `/api/runtime` / `/api/usage` | `GET /v1/view` `{collection:"stats"|"runtime"|"usage", project?}` | typed JSON 503 | RPC relay | read | user-authority |
| `GET /api/observer-status` | `GET /v1/view` `{collection:"observer-status"}` | typed JSON 503 | RPC relay | read | user-authority |
| `GET /api/config` | `GET /v1/view` `{collection:"config"}`（secret は daemon 側で redaction） | typed JSON 503 | RPC relay | read | user-authority |
| `POST /api/memories/visibility` / `project` / `forget` | endpoint なし | HTTP 404（route 非登録） | **削除**（UI 操作は A7 で撤去済み） | − | − |
| `POST /api/raw-events` / `claude-hooks` / `codex-hooks` | endpoint なし。hook は T041 で daemon socket 直行 | HTTP 404 | **削除** | − | − |
| routes/pack.ts `GET /api/prompt-pack-profile` / transport `POST /api/pack` / `POST /api/prompt-pack-ledger` | endpoint なし。hook/MCP は T041/T042 で daemon RPC 直行、ledger は pack/search/get に統合 | HTTP 404 | **pack transport routes 全削除** | − | − |
| `POST /api/config` | endpoint なし | HTTP 404 | **削除**（viewer read-only） | − | − |

## §7 maintenance / backfill → daemon 内 jobs（T045）

| 対象（現行） | endpoint / job kind / daemon 不在時 | disposition | class | authority |
|---|---|---|---|---|
| core/maintenance/with-db.ts `connect` / `connectReadOnly` | endpoint なし。daemon 内 job handler は writer actor/audited wrapper から handle を受け取る | wrapper ごと daemon 内へ移設（readonly 例外なし） | C | − |
| init-vacuum | `POST /v1/jobs` kind `db.init|db.vacuum` → jobId。不在/応答不明 = 自動 retry なし | daemon 内 jobs | C | user-authority |
| ai-structured / backfill-narrative / backfill-tags / dedup-keys | kind `structured.backfill|narrative.backfill|tags.backfill|dedup-keys.backfill` | daemon 内 jobs | C | user-authority |
| dedup / low-signal / relink / scan-secrets / memory-role-report | kind `memories.dedup|memories.prune|memories.relink|secrets.scan|report.memory-role` | daemon 内 jobs | C | user-authority |
| dedup-key-backfill / ref-backfill / scope-backfill / session-context-backfill / summary-dedup-backfill / vector-migration | kind `dedup-keys.backfill|refs.backfill|scopes.backfill|session-context.backfill|summary-dedup.backfill|vectors.migrate` | daemon 内 jobs。runner は daemon の job registry のみ | C | user-authority |
| 全 class C result | `GET /v1/jobs/:id`。一覧探索は `GET /v1/jobs?kind&state&submittedAfter` | job ID 受領後は照会のみ。再実行は user が新 job を trigger | C | user-authority |
| cli/maintenance-worker-runtime.ts（store 構築 + runner 群） | endpoint なし | daemon 内へ吸収(T045)。別プロセス worker は廃止 | C | − |

## §8 core 基盤（daemon 常駐化 — T033）

| 対象 | disposition |
|---|---|
| core/store.ts constructor の `connect()` | daemon 所有のみ。writer actor + audited wrapper 一本化。raw Database 非 export（判断 #7/#15/#16） |
| core/db.ts `connect` / `connectReadOnly` 定義 + migration/vacuum helpers | audited wrapper 内部へ。bootstrap/migration は connect から分離（判断 #15） |
| core/schema-bootstrap.ts（bootstrap DDL） | migration runner（backup + verify 後のみ）へ分離 |
| core/extraction-replay.ts:431 / extraction-eval.ts:335 の `connect()` | CLI 到達点なし（distill 系 typed 無効化に伴い）。core 側 connect は除去し db 引数受け取りへ（実行主体 = 将来の daemon jobs のみ） |

## §9 scan 例外（T053 の exact-path 許可リスト）

| path | 理由 |
|---|---|
| `packages/*/src/**/*.test.ts` / `packages/core/src/test-utils.ts` / `packages/core/src/test-schema.generated.ts` / `packages/core/scripts/generate-test-schema.ts` | test 専用（inventory Appendix の test 群） |
| `scripts/eval/`（pack-eval / lib） | repo tooling。product 実行経路なし。Phase 8 benchmark 素材として保持。T053 では tooling-allowlist として明示（黙認しない） |

## §10 対象外（DB open なし・確認済み）

- Drizzle wrapper 群: 既 open handle のラップのみ（第 2 の open なし — inventory §A）
- plugins/claude / plugins/codex の hook シェル・opencode-plugin: CLI/npx 起動のみで直接 DB open なし
- UI（ブラウザ側）: HTTP client のみ

## §11 2026-08-14 live tree 再照合

`vendor/codemem` で test/spec を除外して再走査した。

| opener | production file set | 本表の disposition |
|---|---:|---|
| `new MemoryStore(` | 2 files: `daemon-canonical.ts`, `daemon-jobs.ts` | daemon canonical store / comparison job のみ = §8 / T048 |
| direct `connect(` call | 2 files: `daemon-canonical.ts`, `daemon-jobs.ts` | audited writer を daemon 所有 code だけが開く = §8 / T048 |
| `connectReadOnly(` call | 0 files | definition は `db.ts` 内部、production caller 0 |
| `WriterActor.open` / `ReadOnlyActor.open` | 4 files: `db.ts`, `legacy-cutover.ts`, `online-backup.ts`, `storage.ts` | audited wrapper / cutover / backup / storage verification = T048 |
| runtime `better-sqlite3` + raw constructor | 2 files: `daemon-lifecycle.ts`, `writer-actor.ts` | instance lock + audited actor 実装だけ = T034 / T048 |

T053 harness は TypeScript AST で alias/deep import、opener、DDL、旧 direct path、未認定 sidecar、public runtime export を走査し、上記 file set と exact match させる。MCP SDK の `server.connect(transport)` と type-only DB import は opener に数えない。inventory 本文 + F9–F11 補遺の未分類 production 経路は 0。

## 完了条件との対応

- 本表の「削除 / 置換 / 移設 / 無効化」全行の消化 + §9 以外での daemon 外 open ゼロ = T048/T053 の判定基準。
- 実装で新設される surface（control/lock.db・backup verify・staging・viewer 認証・spool）は各タスク完了時に本表へ追記し、T053 直前に再生成して完全一致を検証する。

## T034 新設 surface（2026-08-13）

| surface | path | disposition | class | authority |
|---|---|---|---|---|
| instance lock | `control/lock.db` | daemon常駐(T034)。better-sqlite3 `BEGIN IMMEDIATE` writer reservation・busy_timeout 0・journal DELETE・0600。SQLITE_BUSY = 二重起動拒否 | − | − |
| force-kill identity | `control/identity.json` | daemon常駐(T034)。PID + startTime + exe/cmdline fingerprint + nonce。不一致は kill 拒否 | − | − |
| RPC socket (bind only) | `control/daemon.sock` | daemon常駐(T034)。parent 0700 / socket 0600。handshake / allowlist / typed RPC は T035 | − | − |
| health probe | `readDaemonHealth(dataDir)` + socket liveness line | daemon常駐(T034)。typed instance/protocol/doctor は T035 | − | − |

## T035 新設 surface（2026-08-13）

| surface | path | disposition | class | authority |
|---|---|---|---|---|
| daemon RPC | `control/daemon.sock` JSONL | daemon常駐(T035)。handshake + allowlist + 32KiB + hard deadline。未知 method/field 拒否 | − | − |
| `GET /v1/health` / `GET /v1/doctor` | RPC method | read。`{ status, instanceId, protocolVersion, diagnostics? }` | − | − |
| `GET /v1/backup/list` / `POST /v1/backup/create` / `POST /v1/backup/verify` / `POST /v1/backup/restore` | RPC method | T050 online backup を T052 で manifest/list/retention/journal restore まで完成 | B / read | user-authority |
- viewer の pack transport mutation は削除、browser の pack/trace は `POST /v1/context/pack` read relay（ledger は daemon 内）に確定した。未決 disposition は 0 件。
- inventory 全数 + F9–F11 補遺に未分類の production DB-open 経路は rg 全数照合で存在しない（2026-08-13 時点）。

## T039 新設 surface（2026-08-14）

| surface | path | disposition | class | authority |
|---|---|---|---|---|
| atomic spool producer | `control/spool/tmp/*.json.tmp` → `control/spool/ready/*.json` | T038 adapter 前処理後の `POST /v1/events` / `POST /v1/memories/record` だけを保存。sensitivity / ruleset / degraded / private omitted / local-only metadataを保持し、idempotency key + payload hash filename、write+fsync+atomic rename。T040 importer が回収 | A | agent-callable |
| spool quota/counter | `control/spool/dropped-counter` | 通常128MiB・予約16MiB（64KiB/file、最低64 eventを超える）・tmp+ready 合算。80%をstderr + health/doctorへ表示。4KiB事前確保領域をlock下write-in-place | − | − |
| spool lock | `control/spool/lock` | `open(wx)` 排他fileをfd存命保持しinode照合。PID + OS start identity + fingerprint + nonce、stale owner回収、100ms hard deadline。hook側SQLite handleなし | − | − |
| spool quarantine | `control/spool/quarantine/*.json` | 32MiB別枠。満杯時readyを削除せず新規隔離を拒否しcritical counter。T040 importer が broken / tamper / conflict を隔離 | − | − |
| legacy spool drain | `{claude-hook-spool,codex-hook-spool}/*.json` | handler成功後だけ旧fileをfsync付き削除。失敗/tmp/bad entryは保持。legacy cutoverでの実呼出しはT051 | A | agent-callable |

## T040 新設 surface（2026-08-14）

| surface | path | disposition | class | authority |
|---|---|---|---|---|
| spool importer | `importReadySpoolEntries` + `control/spool/{tmp,ready,quarantine}` | daemon起動時と1秒ごとに実行。entryをcanonical JSON・payload hash・hashed filename・method schema・quota classまで再検証し、valid tmpをreadyへ回復。dispatcher成功後だけreadyをfsync付き削除し、失敗・quarantine満杯では保持 | A | daemon内部 |
| spool dispatcher bridge | `dispatchSpoolMutation` → `handleEvent` / `handleRemember` → `dispatchClassA` | direct RPC と同一handler/transactional receiptを使用。同一method+key・異payloadはDB conflict記録後にspool fileをquarantine。adapter metadataとdaemon第2層redactionを強い側へ統合 | A | daemon内部 |

## T043 新設 surface（2026-08-14）

| surface | path | disposition | class | authority |
|---|---|---|---|---|
| viewer bearer | `control/token` | daemon が256-bit tokenを0600で永続化。regular file・owner・mode・形式を再読込時も検査し、CLI/plugin probeはheaderにだけ載せる | − | local user |
| viewer browser auth | `POST /v1/viewer/auth/{nonce,exchange,verify,logout}` | nonce/session stateはdaemon memoryだけが所有。nonce 60秒・one-use、session 12時間・上限8・logout/restart失効 | − | local user |
| browser credential exchange | URL `#auth=<nonce>` → `/api/auth/exchange` → origin-scoped `sessionStorage` | fragmentをnetwork前に`history.replaceState`で除去。exact loopback Origin、相対`/api/`だけへ`Authorization: Session`、cookie非発行、`credentials: "omit"`、no-store、no-referrer | − | local user |
| viewer read relay | `GET /v1/view` + allowlisted context/health RPC | viewer processのDB handleとmutation/ingest/config-write routeを削除。daemon不在はtyped 503でDB fallbackなし | read | user-authority |
| viewer public health | `GET /api/health` → `GET /v1/health` | liveness metadataだけを非認証で返す。probeは未検証loopback listenerへBearerを送らない。data APIは引き続き認証必須 | read | − |
| viewer web policy | loopback HTTP response headers / static bundle | `script-src 'self'`、第三者script/fontなし、frame/object/base/form拒否。既存inline CSSのためstyleのみ`unsafe-inline` | − | − |

## T044 新設・確定 surface（2026-08-14）

| surface | path | disposition | class | authority |
|---|---|---|---|---|
| shared CLI RPC client | `@codemem/mcp` `createMcpRpcClient` | CLI/MCP が同一 endpoint allowlist、project policy pre-redaction、typed failure、shared spool を使用。client process は DB を開かない | read / A | caller に従う |
| CLI event/remember fail-over | `requestWithSpool` → `control/spool/ready` | RPC 前に redaction。event は adapter metadata を保持。forget と read は spool せず typed failure | A | event = agent-callable、remember/forget = user-authority |
| CLI read surface | context/search/memory/view/health/doctor RPC | pack/search/recent/show/stats/status は daemon read。独立 prompt-pack ledger command は削除 | read + A ledger | user-authority |
| later-phase typed stub | distill / embed / extraction replay / benchmark | local DB/model 実装を削除し Phase 6/7 `feature_unavailable` のみ | − | user-authority |
| CLI service lifecycle | `serve start|stop|restart` | resolved data directory の daemon socket と viewer を同時管理。explicit legacy DB path は data directory の導出だけに使用 | − | user-authority |

## T048 DB handle closure（2026-08-14）

| opener | production allowlist | disposition |
|---|---|---|
| `connect` / `connectReadOnly` | `core/db.ts` 定義、`daemon-canonical.ts`、`daemon-jobs.ts` | package runtime export を削除。daemon canonical store と daemon job の report comparison だけが audited writer を開く。`connectReadOnly` の production caller は 0 |
| `new MemoryStore` | `daemon-canonical.ts`、`daemon-jobs.ts` | constructor は既 open `WriterActor` 必須。path/default/bootstrap による自己 open を削除し、public runtime export は type-only 化 |
| `WriterActor.open` / `ReadOnlyActor.open` | `db.ts`、`legacy-cutover.ts`、`online-backup.ts`、`storage.ts` | package runtime export を type-only 化。daemon cutover/backup/verify/storage と audited wrapper 内だけに限定 |
| `new BetterSqlite3` | `daemon-lifecycle.ts`、`writer-actor.ts` | daemon instance lock と actor 実装内部だけに限定 |
| test-only opener | `packages/core/src/test-utils.ts` と test file | `openTestMemoryStore` は package public export せず、exact-path scan 例外からのみ利用 |

path を受け取って DB を自己 open していた export/import、maintenance/report/relink/reliability/status、migration wrapper と、独立 connection を所有していた backfill runner class は削除した。daemon job/operation handler は既存 handle を受け取る `*WithDb` / pass 関数だけを利用する。`P1-T048-01-zero-external-db-handles` は production source の opener allowlistと、public runtime bypass の不在を同時に固定する。T053 では同じ基準を harness の restricted-import / deep-import scan へ昇格する。

## T051 legacy cutover surface（2026-08-14）

| surface | path | disposition | class | authority |
|---|---|---|---|---|
| install target manifest | `control/install-manifest.json` | setup は source checkout の共通 built CLI/hook を事前検査する。manifest は built CLI と選択 integration の config/installed runtime を記録し、OpenCode は wrapper/plugin source も SHA-256 fingerprint に含める。各 lane は config と manifest を一体で commit/rollbackする。部分 setup は他 integration を保持し、選択 integration の消滅 target は除外。cutover 開始前と unlock 直前に再照合し、欠落/symlink/hash drift は中止 | − | local user |
| legacy owner handoff | 旧 DB inode + `/proc/*/fd` / trusted absolute `lsof` | transient owner grace後、identity再検証できる旧 codemem processだけへSIGTERM。prepared前とunlock直前に owner set = `{daemonPid}` を要求 | − | daemon内部 |
| legacy backup/publish | `control/backups/legacy-*.sqlite` → `db/versions/` → `db/current` | daemon EXCLUSIVE保持中のread-only handleからT050 online backupを作成・verify。tombstone・final owner/manifest検査後に判断 #16 journalでpublish | B | daemon内部 |
| rollback hardlink | `control/legacy-*.legacy-recovery.sqlite` | tombstone前に旧inodeをprivate hardlinkで保持。通常失敗は旧pathへrollbackし、tombstone後のprocess crashは次回起動でexact target + dev/inodeを検証して旧pathを復元。不一致・欠損はfail-closed | − | daemon内部 |
| legacy tombstone | 旧 DB path → `control/legacy-db-tombstone/` symlink | atomic rename + parent fsync後にfinal owner検査。旧binaryのcanonical writeと別DB新規生成を双方拒否 | − | daemon内部 |
| legacy spool handoff | `{claude-hook-spool,codex-hook-spool}` → T039 ready spool | canonical daemon 起動ごとに normalized event 化を再試行し、共通redaction済みdurable spoolへ移せた後だけ旧fileを削除。変換不能・quota失敗は保持 | A | agent-callable |

## T052 backup / restore surface（2026-08-14）

| surface | path | disposition | class | authority |
|---|---|---|---|---|
| canonical backup artifact | `control/backups/<id>.sqlite` | SQLite online backup 完了後に standalone 化・0600・fsync・integrity/hash検証。live DB inode / symlink / WAL sidecar は拒否 | B | local user / daemon内部 |
| canonical manifest | `control/backups/<id>.json` | 完成 artifact を read-only で開き、schema/SQLite/FTS/sqlite-vec/generation/canonical rows/watermark/privacy を記録。canonical hash + artifact hash、Phase 1 は hash-only | B | local user / daemon内部 |
| daily retention | `daily-YYYY-MM-DD` + 7 daily / 4 older weekly | daemon 1分 sweep。maintenance/restore 中は開始せず、automatic だけ prune、manual は保持 | B | daemon内部 |
| restore staging | `db/versions/restore-<operation>-<payload>.sqlite` | verified artifactをCOPYFILE_EXCL、FTS rebuild、vector degraded、manifest/rows/integrity再検証後だけ publish | B | local user |
| restore activation | `control/restore-journal.json` + `db/current` | 判断 #16 `prepared→switched→committed` + reopen/integrity。成功 response 後 daemon停止、旧 artifact保持 | B | local user |
| backup CLI | `codemem backup create|list|verify|restore` | shared RPC clientのみ。reasonはproject custom secret rule適用後にhash再計算。private/local-only と off-device/export Phase 1非提供を表示 | B / read | user-authority |
