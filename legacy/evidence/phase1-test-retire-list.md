# T029(a) — Phase 1 test retire list

日付: 2026-08-13
baseline: `evidence/phase1-test-baseline-pre.txt` (`9deb8e2`, SHA-256 `525348b0b36e23bb5c1ccbfa51b000eb4a2f9b6f2c8a4f5a25a2e88ed0949f79`)
post-A7: `f1e84cf` / `/tmp/free-mem-phase1-post-t028.json` (SHA-256 `b475b510a9ba231e2cc58f62ff26a41c5cfcb5b85758c81ffe5593c24d89926c`)
post-T030: `/tmp/free-mem-phase1-post-t030-final.json` (SHA-256 `3b84e9d1f62a43ab6b00cef098ce475a24c7f9b6450489fe0bfacf0b4611e6b3`)
post-T031: `/tmp/free-mem-phase1-post-t031.json` (SHA-256 `4c6778965dcd6f176e164eb5e1ee84c23e087a33a811bad05949f3d5f3108ca1`)
post-T043: `/tmp/free-mem-phase1-post-t043-serial-final.json` (SHA-256 `d2e58af19a2b84bf7fc005ec931fcb9dcacb27445b05a7e293d29d405512bdfa`)
post-T044: `/tmp/free-mem-phase1-post-t044-serial-final3.json` (SHA-256 `f570ca29bb73d8987e3ad8e55af963bd1979bfd5d358f89de85f7ef313c91d17`)
post-T046: `/tmp/free-mem-phase1-post-t046-serial-final3.json` (SHA-256 `297ebd3719add24b02bdd6882d172180926960e4f3d7cfcb45d881f3849e2388`)
post-T047: `/tmp/free-mem-phase1-post-t047-serial.json` (SHA-256 `dce42b151bd40e1e49ed7fc67948e95f3904a75a5f68fac30f9fe17433158b2d`)
post-T048: `/tmp/free-mem-phase1-post-t048-serial.json` (SHA-256 `e7367709111ae546c770b5dac1c4654c9a878341d4a699a7ae13450c3c089f4c`)
比較単位: `<relative test file> > <ancestor titles> > <title>` の multiset（status は集合キーから除外）

## 集計

| 区分 | 件数 |
|---|---:|
| 事前 baseline | 4,037 |
| 維持 | 1,986 |
| retire | 2,051 |
| 登録済み追加（A7 17 + T030 1 + T031 2） | 20 |
| post-T031 | 2,006 |
| T043 直前 | 1,942 |
| T043 retire | 61 |
| T043 登録済み追加 | 13 |
| post-T043 | 1,894 |
| T044 retire | 61 |
| T044 登録済み追加 | 3 |
| post-T044 | 1,836 |
| T045 retire | 8 |
| T045/T046 登録済み追加 | 5 |
| post-T046 | 1,833 |
| T047 stub retire | 1 |
| T047 登録済み追加 | 3 |
| post-T047 | 1,835 |
| T048 direct-opener retire | 10 |
| T048 daemon-handle replacement | 2 |
| T048 登録済み追加 | 1 |
| post-T048 | 1,828 |
| #89 lock 競合修正 登録済み追加 | 3 |
| post-#89 | 1,831 |

実測の注記（#89、2026-08-22）: 完全な Vitest JSON report は total 1,867 / passed 1,864 / todo 3。#89 の `P1-T039-05/06/07` に加え、baseline 後の #28 (`124817a`) で追加済みだった 10 件を下の post-only exact additions に登録し、`harness/phase1-test-set-compare.mjs` の期待値と一致させた。

機械式は `4,037 - 2,051 + 20 = 2,006`。baseline 内には同一完全修飾名が 3 回現れる parameterized test が 1 組あるため、Set ではなく multiset で数える。retire 一覧も multiplicity を保持する。

post-A7 の実行結果は total 2,062 / passed 2,051 / failed 8。failed 8 名はすべて baseline に存在する linked-worktree の cwd / consumer 環境依存群で、新規 failure は 0。該当名:

```text
packages/core/src/export-import.test.ts > export/import > exports parsed JSON fields and prompt import key links
packages/core/src/export-import.test.ts > export/import > includes inactive memories when requested
packages/core/src/export-import.test.ts > export/import > imports idempotently and supports dry run
packages/core/src/ingest-pipeline.test.ts > ingest() integration > creates memories from observer response
packages/core/src/ingest-pipeline.test.ts > ingest() integration > falls back to cwd basename when payload project is missing
packages/core/src/observer-client.test.ts > ObserverClient.observe() > passes configured reasoning overrides to the codex consumer request
packages/core/src/project.test.ts > project helpers > returns cwd basename when no git repo exists
packages/mcp-server/src/project-scope.test.ts > project-scope helpers > falls back to default project when env or request project is blank on read filters
```

post-T030 は total 2,048 / passed 2,038 / failed 7 / todo 3。上記 8 名のうち T030 で retire した Codex consumer test 以外の 7 名だけが残り、新規 failure は 0。

post-T031 は total 2,006 / passed 1,996 / failed 7 / todo 3。post-T030 との差は sidecar 関連 retire 44 名 + 登録済み追加 2 名と完全一致し、failed 7 名も同一で新規 failure は 0。

T043 は直前 1,942 件から旧 viewer 直結 test 61 件を retire し、manifest 登録済み 13 件を追加した。機械式は `1,942 - 61 + 13 = 1,894`。post-T043 の serial full suite は 396 suites / total 1,894 / passed 1,891 / todo 3 / failed 0。通常の並列 full run では、互いに異なる既存 test が 1 件ずつ resource/concurrency failure になったため、各対象を単独再実行して成功を確認したうえで serial full suite を正本にした。

T044 は post-T043 1,894 件から旧 CLI local DB / prompt ledger / 後続 Phase 実装 test 61 件を retire し、manifest 登録済み 3 件だけを追加した。機械式は `1,894 - 61 + 3 = 1,836`。post-T044 の serial full suite は 390 suites / total 1,836 / passed 1,833 / todo 3 / failed 0。post-only 完全修飾名は `P1-T044-01..03` の3件と完全一致する。

T045/T046 は post-T044 1,836 件から別 process maintenance worker 固有 test 8 件を retire し、manifest 登録済み 5 件を追加した。機械式は `1,836 - 8 + 5 = 1,833`。post-T046 の serial full suite は 390 suites / total 1,833 / passed 1,830 / todo 3 / failed 0。追加 test は `P1-T045-01..03` と `P1-T046-01..02` に一致する。

T047 は `not_implemented` を固定していた T036 class B stub 1 件を retire し、manifest 登録済み 3 件を追加した。機械式は `1,833 - 1 + 3 = 1,835`。post-T047 の serial full suite は 392 suites / total 1,835 / passed 1,832 / todo 3 / failed 0。追加 test は `P1-T047-01..03` に一致する。

T048 は daemon 外で DB を自己 open していた init/viewer/store constructor test 10 件を retire し、vacuum / vector migration の handle 注入版 2 件と manifest 登録済み境界 test 1 件を追加した。機械式は `1,835 - 10 + 2 + 1 = 1,828`。post-T048 の serial full suite は 393 suites / total 1,828 / passed 1,825 / todo 3 / failed 0。追加境界 test は `P1-T048-01-zero-external-db-handles` と一致する。

## retire 理由・failure signature・再現

| ID | file scope | 件数 | retire 理由 | 期待 failure signature |
|---|---|---:|---|---|
| R-E2E | `e2e/**` | 95 | coordinator/sync dogfood・Compose 一式を A7 で物理削除 | 対象 file 指定時 `No test files found`。dogfood/Compose resource path も不在 |
| R-WORKER | `packages/cloudflare-coordinator-worker/**` | 11 | Worker + D1 migrations 全削除 | 対象 file 指定時 `No test files found`、workspace package 不在 |
| R-CLI | `packages/cli/**` | 50 | sync/coordinator/http/config workspace/status 配線と孤立 test を削除 | 削除 file は `No test files found`、存続 file の retire title は `No test found`、CLI help に削除 command が出ない |
| R-CORE | `packages/core/**` | 1,137 | sync/coordinator/sharing/recipient-policy/scope-cache と caller 0 の派生 API を削除 | 削除 file は `No test files found`、存続 fileの retire title は `No test found`、削除 export は import 不可 |
| R-MCP | `packages/mcp-server/**` | 97 | HTTP transport/OAuth/OIDC/provider/audit と A7 scope 入力を削除 | 削除 file は `No test files found`、`codemem mcp --help` に HTTP mode なし |
| R-UI | `packages/ui/**` | 385 | sync/coordinator/sharing/devices/projects UI・API client・孤立 primitive を削除 | 削除 file は `No test files found`、削除 route は feed fallback、bundle に削除 tab label なし |
| R-VIEWER | `packages/viewer-server/**` | 217 | sync/coordinator/sharing route・maintenance と対応 test を削除 | 削除 file/title は Vitest no-match、`/api/sync/status` と `/api/coordinator/admin/status` は HTTP 404 |
| R-AUTH | `packages/core/src/observer-auth.test.ts`, `observer-client.test.ts` | 15 | T030 で OAuth consumer、OpenCode auth cache、command 認証を物理削除 | retire title は `No test found`、登録 token test が explicit→env→file のみを固定 |
| R-SIDECAR | core/UI/viewer の observer runtime test | 44 | T031 で Claude/Codex subprocess、自動選択、command 設定、UI surface を物理削除 | retire title/file は `No test found`、登録 token tests が旧 runtime の API-only 化と設定非表示を固定 |
| R-T043-VIEWER | viewer direct DB/mutation/transport test + core viewer HTTP hook test | 61 | T043 で viewer を認証済み read-only daemon RPC relay に限定し、direct DB・mutation・hook ingest・旧 pack transport を物理削除 | retire title/file は `No test found`。登録済み T043 13 件が bearer/nonce/session/origin/CSP/read-only/RPC/503 を固定 |
| R-T044-CLI | CLI local DB/ledger/Phase 6-7 implementation test | 61 | T044 で production CLI を daemon RPC 化し、独立 prompt ledger と後続 Phase の local 実装を削除 | retire title/file は `No test found`。登録済み T044 3 件が endpoint map / typed stub / no DB fallback を固定 |
| R-T045-JOBS | maintenance worker runtime / serve worker PID test | 8 | T045 で別 process worker を daemon 内 durable jobs へ吸収し、worker command・PID 管理を削除 | 削除 file/title は `No test found`。登録済み T045/T046 5 件が job result/no retry/worker 廃止/maintenance mode/spool を固定 |
| R-T047-STUB | T036 class B not-implemented stub | 1 | T047 で export/import operation 本体を実装し、stub expectation を登録済み T047 3 件へ置換 | retire title は `No test found`。登録済み test が conflict/result retrieval/import backup precondition を固定 |
| R-T048-OPENERS | daemon 外 direct init/store opener test | 10 | T048 で path 自己 open API と public DB opener を削除し、daemon handle 注入と exact test helper へ限定 | retire title は `No test found`。vacuum/vector handle 注入 test と登録済み T048 scan が動作・境界を固定 |

再現コマンド（cwd = `vendor/codemem`）:

```bash
corepack pnpm install --frozen-lockfile
corepack pnpm run build
corepack pnpm exec vitest run --reporter=json --outputFile=/tmp/free-mem-phase1-post-t028.json
corepack pnpm exec vitest run packages/core/src/sync-replication.test.ts
corepack pnpm exec vitest run packages/cloudflare-coordinator-worker/src/index.test.ts
node packages/cli/dist/index.js --help
node packages/cli/dist/index.js config --help
node packages/cli/dist/index.js status --help
```

最初の 3 コマンドで post report を再生成する。続く削除 file 指定 2 本は非ゼロ + `No test files found` が期待値。help 3 本は sync/coordinator/http/workspace surface が表示されないことを確認する。viewer は一時 DB で起動し、`/api/health = 200`、削除 2 endpoint = `404` を確認済み。

## A7 中に追加済みの test（17 名）

これらは baseline 後・T028 完了前に追加された回帰 test で、T058 の「登録済み追加」に含める。

```text
packages/cli/src/commands/mcp.test.ts > mcp command > does not expose HTTP mode after carve-out
packages/cli/src/commands/status.test.ts > status command > projects an unconfigured observer and subsystem failures
packages/cli/src/commands/status.test.ts > status command > reads observer presence from environment evidence
packages/core/src/db.test.ts > ensureAdditiveSchemaCompatibility schema-compat gate > repairs partially-created share operation tables before marking compatibility
packages/core/src/store.test.ts > MemoryStore > remember > stamps local-default scope on new memory by default
packages/core/src/store.test.ts > MemoryStore > remember > stamps mapped scope on new memory
packages/core/src/vector-migration.test.ts > vector migration > removes stale old-model rows when there are zero embeddable memories
packages/viewer-server/src/index.test.ts > viewer-server > /api/config > ignores unchanged protected keys and removes retired sync settings
packages/ui/src/lib/state.test.ts > Viewer tab routing > exposes the canonical tab set
packages/ui/src/lib/state.test.ts > Viewer tab routing > resolves removed route sync to feed
packages/ui/src/lib/state.test.ts > Viewer tab routing > resolves removed route coordinator-admin to feed
packages/ui/src/lib/state.test.ts > Viewer tab routing > resolves removed route projects to feed
packages/ui/src/lib/state.test.ts > Viewer tab routing > resolves removed route sharing to feed
packages/ui/src/lib/state.test.ts > Viewer tab routing > resolves removed route devices to feed
packages/ui/src/lib/state.test.ts > Viewer tab routing > resolves removed route advanced to feed
packages/ui/src/lib/state.test.ts > Viewer tab routing > resolves a removed saved tab to feed
packages/ui/src/lib/state.test.ts > Viewer tab routing > keeps canonical tabs accessible and uses feed as the fallback
```

## Phase 1 追加予定 test の事前登録 manifest

後続 test は title に下記 token を 1 個だけ含める。file 配置を先に固定して実装構造を歪めない代わりに、token が test identity の安定キーになる。下記以外を追加する必要が出た場合は、**test を作る前**にこの manifest へ token と失敗条件を追記する。T058 は post-only 完全修飾名について「上記 17 名との完全一致、または登録 token ちょうど 1 個」を機械検査し、token 重複・未登録追加・登録済み未実装を失敗にする。

| token | owner | test が固定する失敗条件 |
|---|---|---|
| `P1-T030-01-auth-cascade` | T030 | observer 認証が explicit → env → file 以外の資格情報経路へ到達する |
| `P1-T031-01-sidecar-retired` | T031 | 廃止済み Claude/Codex sidecar runtime が subprocess 経路へ到達する |
| `P1-T031-02-settings-api-only` | T031 | viewer 設定が廃止済み runtime/command を再送または表示する |
| `P1-T033-01-single-writer` | T033 | write-capable DB handle が writer actor 外で開く |
| `P1-T033-02-migration-gate` | T033 | backup verify 前に migration が始まる |
| `P1-T033-03-no-raw-db-export` | T033 | public export から raw Database を取得できる |
| `P1-T033-04-storage-journal-recovery` | T033 | prepared/switched journal から一意に回復できない |
| `P1-T034-01-single-instance-lock` | T034 | 同じ data_dir で daemon が二重起動する |
| `P1-T034-02-force-kill-identity` | T034 | PID/start-time/fingerprint/nonce 不一致を kill する |
| `P1-T034-03-shutdown-fallback` | T034 | clean shutdown timeout 後の fallback が bounded でない |
| `P1-T034-04-data-dir-preflight` | T034 | network fs / WSL-Windows 共有 path を受理する |
| `P1-T035-01-handshake-version` | T035 | protocol version 不一致を受理する |
| `P1-T035-02-schema-allowlist` | T035 | 未知 field / method を受理する |
| `P1-T035-03-size-and-deadline` | T035 | size bound または hard deadline を越えて処理を続ける |
| `P1-T035-04-health-doctor` | T035 | health/doctor が instance/protocol/診断を typed に返さない |
| `P1-T036-01-receipt-atomicity` | T036 | class A 副作用と receipt が別 transaction になる |
| `P1-T036-02-idempotency-conflict` | T036 | 同一 key・異 payload を quarantine せず適用する |
| `P1-T036-03-endpoint-allowlist` | T036 | T029 未登録 endpoint が到達可能になる |
| `P1-T037-01-same-user-peer` | T037 | same-user peer を拒否する、または別 UID を許可する |
| `P1-T037-02-socket-permissions` | T037 | control dir/socket/token が owner-only でない |
| `P1-T038-01-adapter-order` | T038 | allowlist→size→strip→normalize→redact→sensitivity の順序が崩れる |
| `P1-T038-02-private-tag-grammar` | T038 | nested/unclosed/multiple private tag が fail-closed にならない |
| `P1-T038-03-japanese-redaction` | T038 | 日本語 fixture を secret と誤検出する、または secret を漏らす |
| `P1-T038-04-daemon-second-layer` | T038 | spool/RPC 経由の未処理 secret を daemon intake が拒否しない |
| `P1-T038-05-config-fail-closed` | T038 | privacy の未知 key/型エラーを許可側へ倒す |
| `P1-T038-06-no-plaintext-log` | T038 | secret body が log/diagnostic に残る |
| `P1-T039-01-quota-warning-reserve` | T039 | 80% 警告または予約枠が機能しない |
| `P1-T039-02-concurrent-writers` | T039 | 並行 writer で破損・重複・無期限待機が起きる |
| `P1-T039-03-disk-full-temp` | T039 | disk full/tmp 残骸/両枠満杯で bounded fail-open しない |
| `P1-T039-04-old-format-drain` | T039 | 旧 spool 残量を drain できない |
| `P1-T039-05-lock-publish-missing-retries` | T039 | lock 公開直後の stat が消えた path を競合として再試行しない |
| `P1-T039-06-stale-lock-restat-missing-retries` | T039 | stale lock 除去中の再 stat が消えた path を競合として扱わない |
| `P1-T039-07-lock-publish-io-error-surfaces` | T039 | 本物の device error を競合として握りつぶし lock_timeout に化ける |
| `P1-T040-01-commit-before-delete` | T040 | receipt commit 前に spool file を消す |
| `P1-T040-02-import-exactly-once` | T040 | 再起動/再読込で event を二重適用する |
| `P1-T040-03-import-conflict` | T040 | 同一 key・異 payload を quarantine しない |
| `P1-T041-01-hook-timeout-rescue` | T041 | RPC cutoff 後の spool 予約内に event を保全できない |
| `P1-T041-02-inject-fail-open` | T041 | daemon 不在で hook inject が agent を block する |
| `P1-T041-03-file-context-ledger` | T041 | file-context retrieval が daemon ledger receipt を持たない |
| `P1-T042-01-mcp-minimal-tools` | T042 | MCP tool list に未許可 tool が残る |
| `P1-T042-02-mcp-user-mutation-denied` | T042 | forget/confirm/pin/unpin/retract/mark_wrong/bulk が呼べる |
| `P1-T042-03-mcp-remember-spool` | T042 | remember の daemon 不在時に redacted spool へ保全できない |
| `P1-T042-04-mcp-no-db-fallback` | T042 | MCP process が DB handle を開く |
| `P1-T043-01-browser-auth-401` | T043 | 未認証/誤 token request が 401 にならない |
| `P1-T043-02-origin-403` | T043 | allowlist 外 Origin が 403 にならない |
| `P1-T043-03-nonce-single-use-race` | T043 | nonce 再利用/期限切れ/同時交換の複数成功を許す |
| `P1-T043-04-session-expiry-restart` | T043 | TTL/daemon 再起動/logout 後も session が有効 |
| `P1-T043-05-session-eviction` | T043 | 9 個目で最旧 session を evict しない |
| `P1-T043-06-browser-url-privacy` | T043 | history.replaceState / Referrer-Policy で token URL を除去しない |
| `P1-T043-07-viewer-read-only` | T043 | viewer mutation route または direct DB handle が残る |
| `P1-T043-08-viewer-daemon-unavailable` | T043 | daemon 不在 data API が typed 503 にならない |
| `P1-T043-09-bearer-file` | T043 | 256-bit Bearer が `control/token` に0600で永続化されない |
| `P1-T043-10-daemon-auth-rpc` | T043 | nonce/session の生成・検証・logout がdaemon外へ逸脱する |
| `P1-T043-11-loopback-cookie-csp` | T043 | loopback cookie交換、httpOnly/SameSite、self-only script CSP が崩れる |
| `P1-T043-12-daemon-view-collections` | T043 | viewer collection read がdaemon-owned storeを通らない |
| `P1-T043-13-daemon-restart-session` | T043 | 実daemon再起動後も旧HTTP sessionが有効 |
| `P1-T044-01-cli-rpc-map` | T044 | T029 endpoint/schema と CLI command の対応がずれる |
| `P1-T044-02-cli-typed-stubs` | T044 | Phase 6/7 command が local 実装へ到達する |
| `P1-T044-03-cli-no-db-fallback` | T044 | CLI process が daemon 不在時に DB を開く |
| `P1-T045-01-job-id-result` | T045 | class C trigger が jobId を返さず結果照会できない |
| `P1-T045-02-job-no-auto-retry` | T045 | lost response/失敗 job を client が自動再実行する |
| `P1-T045-03-worker-absorbed` | T045 | maintenance worker 別 process が残る |
| `P1-T046-01-maintenance-mode` | T046 | 破壊 job 中に通常 write が DB へ直行する |
| `P1-T046-02-maintenance-spool` | T046 | maintenance 中の spoolable mutation を保全できない |
| `P1-T047-01-operation-id-conflict` | T047 | class B 同一 operationId・異 hash を副作用前に拒否しない |
| `P1-T047-02-operation-result-retrieval` | T047 | 同一 operationId 再送/GET で最終結果を取得できない |
| `P1-T047-03-import-backup-precondition` | T047 | backup verify 失敗後に destructive import を開始する |
| `P1-T048-01-zero-external-db-handles` | T048 | daemon 外 production process が DB handle を開く |
| `P1-T049-01-install-manifest-roundtrip` | T049 | install ownership manifest が install/uninstall roundtrip しない |
| `P1-T050-01-backup-before-migration` | T050 | migration/破壊操作が verified backup より先に始まる |
| `P1-T050-02-backup-failure-blocks` | T050 | backup/verify failure 後に破壊操作を続ける |
| `P1-T050-03-online-backup-consistency` | T050 | 並行 write 中の artifact と manifest rows が一致しない |
| `P1-T051-01-legacy-owner-set` | T051 | prepared 前/lock 解放前の owner set が daemon 以外を含む |
| `P1-T051-02-cutover-fail-closed` | T051 | 旧 process/handle 残存時に cutover を開始する |
| `P1-T051-03-tombstone-before-unlock` | T051 | tombstone fsync 前に旧 handle/lock を解放する |
| `P1-T051-04-old-binary-split-brain` | T051 | commit 後の旧 binary が DB 書込み/新規生成に成功する |
| `P1-T052-01-backup-manifest-hash` | T052 | 完成 artifact の manifest/hash/rows が一致しない |
| `P1-T052-02-backup-retention-permissions` | T052 | retention 7d/4w または owner-only permission が崩れる |
| `P1-T052-03-restore-journal-order` | T052 | restore の durable state 順序・rollback が判断 #16 と異なる |
| `P1-T052-04-backup-privacy-copy` | T052 | private/local-only を含み得る表示、off-device 非提供表示がない |
| `P1-T055-01-spool-fault-boundaries` | T055 | tmp write / file・directory fsync / durable delete の障害後に spool が消失または二重適用される |
| `P1-T056-01-redaction-worker-deadline` | T056 | catastrophic user regex が hook/daemon を停止させる、または timeout 時に本文を保存する |
| `P1-T056-02-gitleaks-pin-source` | T056 | runtime scanner の Gitleaks version / config hash / mandatory subset が固定値から逸脱する |
| `P1-T056-03-gitleaks-subset-conversion` | T056 | subset converter が未対応構文を受理する、または mandatory rule を redaction できない |
| `P1-T056-04-gitleaks-ruleset-hash` | T056 | ruleset hash が pin・実ロード順・由来・entropy・capture group を反映しない |
| `P1-T057-01-backup-restore-fault-matrix` | T057 | fresh-dir restore、derived index rebuild、journal durability、legacy split-brain のいずれかが一意に回復しない |

## T058 final post-only exact additions（106）

final inventory にだけ存在し、A7 exact 名または事前登録 token では識別されない test。multiset multiplicity を保持する。

- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context > applies the size gate but bypasses it for config files
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context > delivers the hook event even when no file path can be searched
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context > fails open when daemon search fails
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context > honors the global kill switch before event delivery
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context > keeps legacy DB flags while describing PreToolUse output
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context > marks delivery failed when formatting cannot hand off selected rows
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context > records a no-results attempt
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context > rejects paths outside cwd before daemon search
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context > scores, deduplicates, and formats daemon search results
- packages/cli/src/commands/claude-hook-ingest.test.ts > claude-hook-ingest > keeps legacy flags while describing daemon delivery
- packages/cli/src/commands/claude-hook-ingest.test.ts > claude-hook-ingest > reports dropped delivery without a DB fallback
- packages/cli/src/commands/claude-hook-ingest.test.ts > claude-hook-ingest > reports rpc delivery without a DB fallback
- packages/cli/src/commands/claude-hook-ingest.test.ts > claude-hook-ingest > reports skipped delivery without a DB fallback
- packages/cli/src/commands/claude-hook-ingest.test.ts > claude-hook-ingest > reports spool delivery without a DB fallback
- packages/cli/src/commands/claude-hook-inject.contract.test.ts > claude-hook-inject output contract > 'disabled injection continues without …'
- packages/cli/src/commands/claude-hook-inject.contract.test.ts > claude-hook-inject output contract > 'empty prompt continues without output'
- packages/cli/src/commands/claude-hook-inject.contract.test.ts > claude-hook-inject output contract > 'long pack is truncated deterministica…'
- packages/cli/src/commands/claude-hook-inject.contract.test.ts > claude-hook-inject output contract > 'non-prompt events cannot inject promp…'
- packages/cli/src/commands/claude-hook-inject.contract.test.ts > claude-hook-inject output contract > 'prompt emits exact UserPromptSubmit o…'
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject > enriches the query from session state and passes working-set paths
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject > fails open when the daemon read fails
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject > honors the global kill switch before RPC or event delivery
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject > keeps legacy DB flags without opening SQLite
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject > logs RPC pack metrics without pack content
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject > requests the daemon pack and returns exact additionalContext
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject > truncates the returned pack and keeps the hook schema stable
- packages/cli/src/commands/claude-hook-session-state.test.ts > claude-hook-session-state > loadSessionState > drops legacy state written before pre-persistence redaction
- packages/cli/src/commands/codex-hook-ingest.test.ts > codex-hook-ingest > gives repeated timestamp-less payloads distinct delivery identities
- packages/cli/src/commands/codex-hook-ingest.test.ts > codex-hook-ingest > keeps legacy flags while describing daemon delivery
- packages/cli/src/commands/codex-hook-ingest.test.ts > codex-hook-ingest > reports daemon delivery without a DB fallback
- packages/cli/src/commands/codex-hook-inject.test.ts > codex-hook-inject > continues without RPC for an ineligible payload
- packages/cli/src/commands/codex-hook-inject.test.ts > codex-hook-inject > continues without RPC for an ineligible payload
- packages/cli/src/commands/codex-hook-inject.test.ts > codex-hook-inject > fails open when daemon retrieval fails
- packages/cli/src/commands/codex-hook-inject.test.ts > codex-hook-inject > keeps legacy DB flags without opening SQLite
- packages/cli/src/commands/codex-hook-inject.test.ts > codex-hook-inject > logs RPC metrics without pack content
- packages/cli/src/commands/codex-hook-inject.test.ts > codex-hook-inject > preserves the safety frame when the body is truncated
- packages/cli/src/commands/codex-hook-inject.test.ts > codex-hook-inject > requests a daemon pack and frames it as reference data
- packages/cli/src/commands/hook-thin-client.test.ts > hook thin clients > P1-T041-01b redacts hook content before the RPC write
- packages/cli/src/commands/hook-thin-client.test.ts > hook thin clients > P1-T041-01c applies repository policy before RPC and spool
- packages/cli/src/commands/hook-thin-client.test.ts > hook thin clients > P1-T041-01d shares the sanitized prompt with pack RPC and session state
- packages/cli/src/commands/hook-thin-client.test.ts > hook thin clients > P1-T041-01e enforces ignore and local-only path policy
- packages/cli/src/commands/hook-thin-client.test.ts > hook thin clients > P1-T041-04 preserves SessionStart timestamps through RPC and spool import
- packages/cli/src/commands/hook-thin-client.test.ts > hook thin clients > does not start a spool write after the fsync reserve is exhausted
- packages/cli/src/commands/hook-thin-client.test.ts > hook thin clients > keeps the claude default RPC cutoff inside its hard cap and spools
- packages/cli/src/commands/hook-thin-client.test.ts > hook thin clients > keeps the codex default RPC cutoff inside its hard cap and spools
- packages/cli/src/commands/hook-thin-client.test.ts > hook thin clients > redacts project and working-set filters before pack RPC
- packages/cli/src/commands/hook-thin-client.test.ts > hook thin clients > uses the declared Agent-specific outer watchdogs
- packages/cli/src/commands/mcp.test.ts > mcp command > routes the database option to the daemon-backed stdio server
- packages/cli/src/commands/serve.test.ts > serve command option resolution > only resolves process inspection tools from fixed absolute paths
- packages/cli/src/commands/setup-codex.test.ts > Codex hook runtime install > copies the standalone runtime and quotes its setup command
- packages/cli/src/commands/setup-codex.test.ts > buildCodememCodexHookGroups — command base > uses a direct `codemem` call with the outer watchdog
- packages/cli/src/commands/setup-codex.test.ts > buildCodememCodexHookGroups — command base > uses the same watchdog for the npx fallback
- packages/cli/src/commands/setup-codex.test.ts > installCodex — fresh CODEX_HOME > records the installed MCP and hook files in the cutover manifest
- packages/cli/src/commands/setup-codex.test.ts > installCodex — fresh CODEX_HOME > writes the MCP block and all hook events with correct schema
- packages/cli/src/commands/setup-codex.test.ts > installCodex — idempotency > migrates the legacy prompt ingest plus inject pair without --force
- packages/cli/src/commands/setup-codex.test.ts > installCodex — idempotency > replaces an installed standalone-runtime hook instead of duplicating it
- packages/cli/src/hook-runtime.test.ts > bundled hook runtime > fails open without persisting invalid or oversized input
- packages/cli/src/hook-runtime.test.ts > bundled hook runtime > rejects commands outside the hook-only allowlist
- packages/core/src/claude-hooks.test.ts > mapClaudeHookPayload > SessionEnd → session_end > keeps timestamp-less retries stable
- packages/core/src/claude-hooks.test.ts > mapClaudeHookPayload > Stop → assistant > bounds transcript fallback to the most recent 256 KiB
- packages/core/src/codex-hooks.test.ts > mapCodexHookPayload > keeps distinct timestamped Stop events distinct without turn IDs
- packages/core/src/codex-hooks.test.ts > mapCodexHookPayload > keeps timestamp-less SessionEnd retries stable after ingest normalization
- packages/core/src/daemon-jobs.test.ts > daemon jobs > bounds configured regexes used by daemon maintenance jobs
- packages/core/src/daemon-rpc.test.ts > Phase 1 daemon RPC > P1-T041-04-file-search stays repository-relative
- packages/core/src/daemon-rpc.test.ts > Phase 1 daemon RPC > P1-T041-05 records and completes the file-context retrieval ledger in the daemon
- packages/core/src/daemon-rpc.test.ts > Phase 1 daemon RPC > applies search filters to get_many reads
- packages/core/src/daemon-rpc.test.ts > Phase 1 daemon RPC > rejects malformed memory adapter redaction metadata
- packages/core/src/maintenance.test.ts > maintenance > vacuums a schema-ready database
- packages/core/src/memory-quality.test.ts > hasSameLineCoOccurrence > matches the pair of regexes it replaced
- packages/core/src/memory-quality.test.ts > hasSameLineCoOccurrence > does not scale with the length of the line
- packages/core/src/mutation-dispatcher.test.ts > Phase 1 mutation dispatcher > P1-T036-01-receipt-schema
- packages/core/src/mutation-dispatcher.test.ts > Phase 1 mutation dispatcher > P1-T036-02-events-idempotent
- packages/core/src/mutation-dispatcher.test.ts > Phase 1 mutation dispatcher > P1-T036-02b-event-id-required
- packages/core/src/mutation-dispatcher.test.ts > Phase 1 mutation dispatcher > P1-T036-04-memories-record
- packages/core/src/mutation-dispatcher.test.ts > Phase 1 mutation dispatcher > P1-T036-05b-view-hides-inaccessible-scopes
- packages/core/src/mutation-dispatcher.test.ts > Phase 1 mutation dispatcher > P1-T036-07-delete-revision-is-part-of-idempotency
- packages/core/src/mutation-dispatcher.test.ts > Phase 1 mutation dispatcher > P1-T038-07-daemon-persists-secret-events-without-secret-body
- packages/core/src/online-backup.test.ts > Phase 1 online backup > P1-T050-01-db-backup-api
- packages/core/src/online-backup.test.ts > Phase 1 online backup > P1-T050-04-rpc-create-verify
- packages/core/src/online-backup.test.ts > Phase 1 online backup > P1-T050-05-payload-hash-and-replay
- packages/core/src/online-backup.test.ts > Phase 1 online backup > P1-T050-06-fresh-bootstrap-skips-online-backup
- packages/core/src/project.test.ts > project helpers > ignores a .git directory that is not a Git repository
- packages/core/src/redaction-pipeline.test.ts > Phase 1 redaction > applies project tool-field policy without dropping the event schema
- packages/core/src/redaction-pipeline.test.ts > Phase 1 redaction > uses the caller allowlist when project config omits a tool allowlist
- packages/core/src/secret-scanner.test.ts > SecretScanner > non-plain objects and cycles in redactValue > redacts a shared object on every output path
- packages/core/src/secret-scanner.test.ts > loadScannerOptionsFromConfig > keeps valid rules but marks malformed entries degraded
- packages/core/src/secret-scanner.test.ts > loadScannerOptionsFromConfig > returns empty options for missing config and degrades malformed scanner blocks
- packages/core/src/spool-importer.test.ts > phase 1 spool importer > P1-T040-04-spooled-redaction-metadata-matches-direct-replay
- packages/core/src/spool.test.ts > phase 1 spool contract > does not rerun failed user rules on an already degraded event
- packages/core/src/spool.test.ts > phase 1 spool contract > keeps a degraded spool rescan over healthy adapter metadata
- packages/core/src/store.test.ts > MemoryStore > remember > persists metadata only when workspace scanner config is invalid
- packages/core/src/text-trim.test.ts > trimEndWhere > matches the regexes they replace
- packages/core/src/text-trim.test.ts > trimEndWhere > leaves the middle alone
- packages/core/src/text-trim.test.ts > trimEndWhere > treats surrogate pairs as one code point, like the /u regexes
- packages/core/src/text-trim.test.ts > trimEndWhere > does not scale with the length of the trimmed run
- packages/core/src/text-trim.test.ts > ReDoS を外した正規表現の等価性 > ファイル名検出は元の正規表現と同じ判定になる
- packages/core/src/text-trim.test.ts > ReDoS を外した正規表現の等価性 > フェンス剥がしは元の正規表現と同じ結果になる
- packages/core/src/vectors.test.ts > memory_vectors bootstrap on fresh databases > creates memory_vectors when explicitly migrating a fresh database
- packages/mcp-server/src/rpc-client.test.ts > MCP daemon RPC client > persists degraded remember diagnostics across daemon restart
- packages/mcp-server/src/rpc-client.test.ts > MCP daemon RPC client > redacts project policy matches before the daemon can persist them
- packages/mcp-server/src/rpc-client.test.ts > MCP daemon RPC client > returns a typed error instead of opening a local database when the daemon is down
- packages/mcp-server/src/rpc-client.test.ts > MCP daemon RPC client > routes backup create, list, and verify through the daemon
- packages/mcp-server/src/server.test.ts > Phase 1 MCP stdio RPC surface > exports a side-effect-free factory from the package root
- packages/mcp-server/src/server.test.ts > Phase 1 MCP stdio RPC surface > maps every read tool to its fixed daemon endpoint and mode
- packages/ui/src/tabs/feed/data/body-renderers.test.ts > isLabeledFact > matches the regex it replaced
- packages/ui/src/tabs/feed/data/body-renderers.test.ts > isLabeledFact > does not scale with the length of the line

## T043 retired fully qualified names（61）

- packages/viewer-server/src/index.test.ts > viewer-server > memory feed routes > applies sharing-domain visibility to memory list endpoints
- packages/viewer-server/src/index.test.ts > viewer-server > memory feed routes > keeps mixed-domain unauthorized scope rows out of viewer direct surfaces
- packages/viewer-server/src/index.test.ts > viewer-server > memory feed routes > applies mine/theirs scope filters to observations
- packages/viewer-server/src/index.test.ts > viewer-server > memory feed routes > moves an owned memory to a new project via /api/memories/project
- packages/viewer-server/src/index.test.ts > viewer-server > memory feed routes > rejects /api/memories/project with empty project
- packages/viewer-server/src/index.test.ts > viewer-server > memory feed routes > does not mutate memories outside visible sharing domains
- packages/viewer-server/src/index.test.ts > viewer-server > memory feed routes > forgets an owned memory via the viewer API
- packages/viewer-server/src/index.test.ts > viewer-server > memory feed routes > treats repeated forget requests as a no-op success
- packages/viewer-server/src/index.test.ts > viewer-server > memory feed routes > rejects forgetting a memory not owned by this device
- packages/viewer-server/src/index.test.ts > viewer-server > memory feed routes > treats metadata-only local provenance as owned for forget requests
- packages/viewer-server/src/index.test.ts > viewer-server > memory feed routes > validates forget requests
- packages/viewer-server/src/index.test.ts > viewer-server > memory feed routes > preserves query parameters on the /api/memories alias
- packages/viewer-server/src/index.test.ts > viewer-server > memory feed routes > routes observer summaries into summaries and excludes them from observations
- packages/viewer-server/src/index.test.ts > viewer-server > memory feed routes > keeps session observation counts aligned with active feed items
- packages/viewer-server/src/index.test.ts > viewer-server > memory feed routes > excludes hidden sharing domains from session memory counts
- packages/viewer-server/src/index.test.ts > viewer-server > memory feed routes > gates prompt and artifact aggregate counts by visible memory sessions
- packages/viewer-server/src/index.test.ts > viewer-server > memory feed routes > tolerates malformed metadata when classifying summaries
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/pack > uses async pack builder path
- packages/viewer-server/src/index.test.ts > viewer-server > POST /api/pack > rejects requests from a different viewer identity target
- packages/viewer-server/src/index.test.ts > viewer-server > POST /api/pack > rejects a viewer whose cached effective identity is stale
- packages/viewer-server/src/index.test.ts > viewer-server > POST /api/pack > validates structured request fields
- packages/viewer-server/src/index.test.ts > viewer-server > POST /api/pack > returns the same machine-readable pack as GET for equivalent inputs
- packages/viewer-server/src/index.test.ts > viewer-server > POST /api/pack > passes project, working-set, and render options to the shared builder
- packages/viewer-server/src/index.test.ts > viewer-server > POST /api/pack > records attempts and reports changed-artifact conflicts without blocking pack delivery
- packages/viewer-server/src/index.test.ts > viewer-server > POST /api/pack > returns a stable structured error when pack construction fails
- packages/viewer-server/src/index.test.ts > viewer-server > POST /api/pack > delivers a built pack when ledger instrumentation fails
- packages/viewer-server/src/index.test.ts > viewer-server > POST /api/prompt-pack-ledger > preserves record, delivery, and cache-reuse idempotency
- packages/viewer-server/src/index.test.ts > viewer-server > POST /api/prompt-pack-ledger > returns structured validation and core failure outcomes
- packages/viewer-server/src/index.test.ts > viewer-server > POST /api/pack/trace > uses async pack trace builder path
- packages/viewer-server/src/index.test.ts > viewer-server > POST /api/pack/trace > rejects invalid trace payloads
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/observer-status > returns live observer status and suppresses stale failures after success
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > GET resolves the same workspace-scoped file as the core resolver POST uses
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > returns provider options from real opencode config prefixes
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > redacts sensitive config values from config responses
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > writes config and returns effects
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > returns tiered observer routing fields from config
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > writes tiered observer routing config
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > accepts built-in observer providers on a clean config
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > clears hot-reload env override when interval key is removed
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > returns warnings for env-overridden keys
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > validates payload types
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > rejects invalid tiered observer routing values
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > rejects protected config mutations from the viewer API
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > ignores unchanged protected keys and removes retired sync settings
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > rejects non-object config wrapper payloads
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > parses integer fields strictly
- packages/viewer-server/src/index.test.ts > viewer-server > CORS middleware > allows POST without Origin header (CLI/programmatic callers)
- packages/viewer-server/src/index.test.ts > viewer-server > CORS middleware > rejects POST without Origin but with cross-site Sec-Fetch-Site
- packages/viewer-server/src/index.test.ts > viewer-server > CORS middleware > rejects POST with non-loopback Origin
- packages/viewer-server/src/index.test.ts > viewer-server > CORS middleware > allows POST with loopback Origin
- packages/viewer-server/src/index.test.ts > viewer-server > CORS middleware > allows GET without Origin header
- packages/viewer-server/src/index.test.ts > viewer-server > CORS middleware > returns 400 for invalid JSON on visibility updates
- packages/viewer-server/src/helpers.test.ts > queryInt > parses full integer strings
- packages/viewer-server/src/helpers.test.ts > queryInt > rejects partial or non-integer strings
- packages/viewer-server/src/routes/health.test.ts > GET /api/health > returns the stable healthy viewer contract through createApp
- packages/viewer-server/src/routes/health.test.ts > GET /api/health > uses only a cheap schema-page database probe without stats aggregation or egress
- packages/viewer-server/src/routes/health.test.ts > GET /api/health > reports store construction failure as degraded HTTP 200 without error details
- packages/viewer-server/src/routes/health.test.ts > GET /api/health > reports database probe failure as degraded HTTP 200 without error details
- packages/core/src/claude-hooks.test.ts > POST /api/claude-hooks via viewer-server > returns {inserted:0, skipped:1} for unsupported hook event
- packages/core/src/claude-hooks.test.ts > POST /api/claude-hooks via viewer-server > returns 400 for invalid JSON
- packages/core/src/claude-hooks.test.ts > POST /api/claude-hooks via viewer-server > allows POST without Origin header (CLI callers)

## T044 retired fully qualified names（R-T044-CLI、61）

```text
packages/cli/src/commands/distill.test.ts > distill command > does not write when the model declines to draft a rule
packages/cli/src/commands/distill.test.ts > distill command > drafts a rule for the top candidate and prints a diff without writing
packages/cli/src/commands/distill.test.ts > distill command > emits JSON candidates and passes parsed options to core
packages/cli/src/commands/distill.test.ts > distill command > emits JSON usage errors without throwing
packages/cli/src/commands/distill.test.ts > distill command > falls back to unjudged output when no observer is configured
packages/cli/src/commands/distill.test.ts > distill command > judges before drafting so the draft targets the top surviving candidate
packages/cli/src/commands/distill.test.ts > distill command > judges candidates by default and drops routine-activity clusters from the report
packages/cli/src/commands/distill.test.ts > distill command > keeps unjudged candidates when the model output is unparseable
packages/cli/src/commands/distill.test.ts > distill command > overfetches when judging and backfills routine drops up to the limit
packages/cli/src/commands/distill.test.ts > distill command > refuses to draft a project-scoped candidate from another repo
packages/cli/src/commands/distill.test.ts > distill command > renders evidence only when explain is enabled
packages/cli/src/commands/distill.test.ts > distill command > reports a structured error when drafting has no observer
packages/cli/src/commands/distill.test.ts > distill command > skips judging entirely with --no-judge
packages/cli/src/commands/embed.test.ts > embed command > prefers explicit --project over CODEMEM_PROJECT
packages/cli/src/commands/embed.test.ts > embed command > supports all-projects override
packages/cli/src/commands/memory-inject.test.ts > memory inject command > closes the store when inject pack generation fails
packages/cli/src/commands/memory-inject.test.ts > memory inject command > prints an empty string when inject returns no pack text
packages/cli/src/commands/memory.test.ts > benchmark observer overrides > preserves configured Responses transport unless the CLI flag enables it
packages/cli/src/commands/memory.test.ts > benchmark reasoning summaries > preserves explicit null reasoning from tier-routed benchmark runs
packages/cli/src/commands/memory.test.ts > memory command error boundaries > accepts a valid skip when the final failure is only summary count
packages/cli/src/commands/memory.test.ts > memory command error boundaries > does not accept a repaired skip with non-summary final failures
packages/cli/src/commands/memory.test.ts > memory command error boundaries > does not throw and emits a JSON error for an invalid extraction-replay batch id
packages/cli/src/commands/memory.test.ts > memory command error boundaries > does not throw and emits a JSON error for an unknown extraction-benchmark id
packages/cli/src/commands/memory.test.ts > memory command error boundaries > preserves a repaired benchmark pass when the initial disposition failed
packages/cli/src/commands/memory.test.ts > memory command error boundaries > preserves observer_no_output before summary disposition scoring
packages/cli/src/commands/memory.test.ts > memory command error boundaries > rejects unsafe extraction-benchmark repetition counts before running a model
packages/cli/src/commands/memory.test.ts > memory command scope safety > does not forget memories outside visible sharing domains
packages/cli/src/commands/memory.test.ts > memory command scope safety > stores vectors for manually remembered memories
packages/cli/src/commands/pack-ledger.test.ts > prompt-pack ledger transport > delegates UUID and timestamp validation without echoing rejected values
packages/cli/src/commands/pack-ledger.test.ts > prompt-pack ledger transport > handles failure recording, successful delivery retry, and cache reuse
packages/cli/src/commands/pack-ledger.test.ts > prompt-pack ledger transport > records an instrumented combined pack through the CLI boundary
packages/cli/src/commands/pack-ledger.test.ts > prompt-pack ledger transport > surfaces a changed-artifact idempotency conflict after caller cache loss
packages/cli/src/commands/pack.test.ts > pack command > accepts only bounded allowlisted internal ledger metadata
packages/cli/src/commands/pack.test.ts > pack command > dispatches the nested pack trace commander path
packages/cli/src/commands/pack.test.ts > pack command > emits a stable ledger conflict outcome with the built pack JSON
packages/cli/src/commands/pack.test.ts > pack command > emits legacy pack JSON when internal ledger stdin is malformed
packages/cli/src/commands/pack.test.ts > pack command > emits structured json errors for trace failures
packages/cli/src/commands/pack.test.ts > pack command > emits structured usage errors for invalid main pack numeric input
packages/cli/src/commands/pack.test.ts > pack command > emits structured usage errors for invalid numeric json input
packages/cli/src/commands/pack.test.ts > pack command > keeps storage-unavailable ledger failures fail-open
packages/cli/src/commands/pack.test.ts > pack command > omits project filters for all-projects pack requests
packages/cli/src/commands/pack.test.ts > pack command > passes explicit compression mode through main pack command
packages/cli/src/commands/pack.test.ts > pack command > rejects invalid compression mode
packages/cli/src/commands/pack.test.ts > pack command > supports the commander command path with json output
packages/cli/src/commands/pack.test.ts > pack command > waits for internal ledger persistence before emitting instrumented pack output
packages/cli/src/commands/stats.test.ts > stats command > auto-initializes a fresh database before reporting stats
packages/cli/src/commands/stats.test.ts > stats command > emits bounded machine-readable and concise attribution diagnostics without sensitive content
packages/cli/src/commands/stats.test.ts > stats command > reports memory counts through the local scope visibility gate
packages/cli/src/commands/status.test.ts > status command > does not treat observer tuning alone as configured
packages/cli/src/commands/status.test.ts > status command > emits the exact required healthy JSON shape with one stdout object
packages/cli/src/commands/status.test.ts > status command > keeps an unready viewer running and reports its readiness warning
packages/cli/src/commands/status.test.ts > status command > projects an unconfigured observer and subsystem failures
packages/cli/src/commands/status.test.ts > status command > reads observer presence from environment evidence
packages/cli/src/commands/status.test.ts > status command > registers shared options and no positional arguments
packages/cli/src/commands/status.test.ts > status command > rejects unknown options as usage errors
packages/cli/src/commands/status.test.ts > status command > renders compact human output and detailed command suggestions
packages/cli/src/commands/status.test.ts > status command > reports a missing database without creating it
packages/cli/src/commands/status.test.ts > status command > reports retryable observer failures as backoff warnings
packages/cli/src/commands/status.test.ts > status command > sets ok false for error attention while still exiting zero
packages/cli/src/commands/status.test.ts > status command > suppresses newer-schema compatibility warnings in JSON mode
packages/cli/src/commands/status.test.ts > status command > uses configured loopback viewer defaults when no PID record exists
```

## T045 retired fully qualified names（R-T045-JOBS、8）

```text
packages/cli/src/maintenance-worker-runtime.test.ts > maintenance worker runtime > stops a failed active backfill even when its pending predicate remains true
packages/cli/src/maintenance-worker-runtime.test.ts > maintenance worker runtime > constructs vector migration with the smaller worker-specific batch size
packages/cli/src/commands/serve.test.ts > serve command option resolution > builds maintenance worker args from the current runner
packages/cli/src/commands/serve.test.ts > serve command option resolution > matches likely codemem maintenance worker command lines
packages/cli/src/commands/serve.test.ts > serve command option resolution > requires exact maintenance worker db-path command ownership
packages/cli/src/commands/serve.test.ts > serve command option resolution > cleans stale maintenance worker pidfiles
packages/cli/src/commands/serve.test.ts > serve command option resolution > does not stop a maintenance worker pidfile for another database
packages/cli/src/commands/serve.test.ts > serve command option resolution > refuses running legacy maintenance worker pidfiles without database ownership
```

## T047 retired fully qualified names（R-T047-STUB、1）

```text
packages/core/src/mutation-dispatcher.test.ts > Phase 1 mutation dispatcher > P1-T036-06-class-b-stub
```

## T048 retired fully qualified names（R-T048-OPENERS、10）

```text
packages/cli/src/commands/serve.test.ts > serve command option resolution > prepares a fresh viewer database before startup
packages/core/src/maintenance-jobs.test.ts > maintenance jobs > initDatabase ensures maintenance_jobs exists on existing schema-ready dbs
packages/core/src/maintenance.test.ts > maintenance > initializes and vacuums a schema-ready database
packages/core/src/maintenance.test.ts > maintenance > initializes a fresh database schema
packages/core/src/maintenance.test.ts > maintenance > does not initialize unrelated non-empty SQLite databases
packages/core/src/maintenance.test.ts > maintenance > runs raw-event relink remediation during initDatabase
packages/core/src/store.test.ts > MemoryStore constructor auto-bootstrap > bootstraps schema when constructed against a path with no existing file
packages/core/src/store.test.ts > MemoryStore constructor auto-bootstrap > bootstraps schema when constructed against an empty existing file
packages/core/src/store.test.ts > MemoryStore constructor auto-bootstrap > does not re-bootstrap an already-initialized database
packages/core/src/vectors.test.ts > memory_vectors bootstrap on fresh databases > creates memory_vectors via auto-bootstrap when constructing MemoryStore against a fresh path
```

## Retired fully qualified names（machine-generated）

以下は理由 ID ごとに分類し、各 ID 内では baseline 順。各行の `R-*` は上記理由/signature を参照する。

### R-AUTH (15)

- packages/core/src/observer-auth.test.ts > ObserverAuthAdapter > resolve > caches command/file results
- packages/core/src/observer-auth.test.ts > ObserverAuthAdapter > resolve > falls back to oauth token
- packages/core/src/observer-auth.test.ts > resolveOAuthProvider > detects anthropic from claude model
- packages/core/src/observer-auth.test.ts > resolveOAuthProvider > detects openai from non-claude model
- packages/core/src/observer-auth.test.ts > resolveOAuthProvider > respects explicit configured provider
- packages/core/src/observer-client.test.ts > ObserverClient > constructor > does not use auth cache API keys for arbitrary custom providers
- packages/core/src/observer-client.test.ts > ObserverClient > constructor > prefers explicit observer api key over cached opencode auth
- packages/core/src/observer-client.test.ts > ObserverClient > getStatus > does not report responses_api runtime when OpenAI OAuth codex transport is active
- packages/core/src/observer-client.test.ts > ObserverClient > getStatus > reports sdk_client auth type for opencode cached key auth
- packages/core/src/observer-client.test.ts > ObserverClient.observe() > passes configured reasoning overrides to the codex consumer request
- packages/core/src/observer-client.test.ts > ObserverClient.observe() > sends the shipped 'rich' tier over OAuth codex_consumer
- packages/core/src/observer-client.test.ts > ObserverClient.observe() > sends the shipped 'simple' tier over OAuth codex_consumer
- packages/core/src/observer-client.test.ts > shouldAutoSelectCodexSidecar > respects a configured auth command
- packages/core/src/observer-client.test.ts > shouldAutoSelectCodexSidecar > respects a configured command auth source
- packages/core/src/observer-client.test.ts > shouldAutoSelectCodexSidecar > yields to a usable OpenCode OAuth cache

### R-SIDECAR (44)

- packages/core/src/extraction-tier-routing.test.ts > extraction tier routing > does not record fallback on claude_sidecar when provider was not explicitly requested
- packages/core/src/extraction-tier-routing.test.ts > extraction tier routing > maps claude_sidecar rich tier routing onto Claude defaults
- packages/core/src/extraction-tier-routing.test.ts > extraction tier routing > maps claude_sidecar simple tier routing onto Claude defaults
- packages/core/src/extraction-tier-routing.test.ts > extraction tier routing > preserves the Codex sidecar default for rich tier routing
- packages/core/src/extraction-tier-routing.test.ts > extraction tier routing > preserves the Codex sidecar default for simple tier routing
- packages/core/src/extraction-tier-routing.test.ts > extraction tier routing > records a visible fallback when an incompatible provider is requested on claude_sidecar
- packages/core/src/observer-client.test.ts > ObserverClient > constructor > defaults tier routing on for claude_sidecar
- packages/core/src/observer-client.test.ts > ObserverClient.observe() > leaves resolved model null when retry payload omits model
- packages/core/src/observer-client.test.ts > ObserverClient.observe() > passes the selected sidecar model via --model
- packages/core/src/observer-client.test.ts > ObserverClient.observe() > raises ObserverAuthError when the sidecar reports an auth failure
- packages/core/src/observer-client.test.ts > ObserverClient.observe() > reports the actual sidecar model after retrying without --model
- packages/core/src/observer-client.test.ts > ObserverClient.observe() > returns null and records ENOENT when the claude binary is missing
- packages/core/src/observer-client.test.ts > ObserverClient.observe() > sidecar error classifiers > does not misclassify unrelated errors as auth errors
- packages/core/src/observer-client.test.ts > ObserverClient.observe() > sidecar error classifiers > does not misclassify unrelated errors as model errors
- packages/core/src/observer-client.test.ts > ObserverClient.observe() > sidecar error classifiers > matches auth errors from known Claude CLI phrasings
- packages/core/src/observer-client.test.ts > ObserverClient.observe() > sidecar error classifiers > matches model errors from known Claude CLI phrasings
- packages/core/src/observer-client.test.ts > ObserverClient.observe() — codex_sidecar > codex sidecar error classifiers > does not false-positive on operational log noise (paths, offsets)
- packages/core/src/observer-client.test.ts > ObserverClient.observe() — codex_sidecar > codex sidecar error classifiers > does not misclassify unrelated errors as auth errors
- packages/core/src/observer-client.test.ts > ObserverClient.observe() — codex_sidecar > codex sidecar error classifiers > does not misclassify unrelated errors as model errors
- packages/core/src/observer-client.test.ts > ObserverClient.observe() — codex_sidecar > codex sidecar error classifiers > matches auth errors from known Codex CLI phrasings
- packages/core/src/observer-client.test.ts > ObserverClient.observe() — codex_sidecar > codex sidecar error classifiers > matches model errors from known Codex CLI phrasings
- packages/core/src/observer-client.test.ts > ObserverClient.observe() — codex_sidecar > omits -m when useModel is false
- packages/core/src/observer-client.test.ts > ObserverClient.observe() — codex_sidecar > passes the selected model via -m
- packages/core/src/observer-client.test.ts > ObserverClient.observe() — codex_sidecar > raises ObserverAuthError when the sidecar reports an auth failure
- packages/core/src/observer-client.test.ts > ObserverClient.observe() — codex_sidecar > retries without -m and reports fallback on model-unavailable
- packages/core/src/observer-client.test.ts > ObserverClient.observe() — codex_sidecar > skips API key init when constructed with no key
- packages/core/src/observer-client.test.ts > ObserverClient.observe() — codex_sidecar real spawn > returns null output and a redacted error on non-zero exit
- packages/core/src/observer-client.test.ts > ObserverClient.observe() — codex_sidecar real spawn > scrubs CLAUDE_CODE_* env, forwards the prompt on stdin, captures -o, and cleans up
- packages/core/src/observer-client.test.ts > shouldAutoSelectCodexSidecar > does not override an explicit runtime
- packages/core/src/observer-client.test.ts > shouldAutoSelectCodexSidecar > requires the codex CLI to be available
- packages/core/src/observer-client.test.ts > shouldAutoSelectCodexSidecar > requires ~/.codex/auth.json to exist
- packages/core/src/observer-client.test.ts > shouldAutoSelectCodexSidecar > respects a configured file auth source
- packages/core/src/observer-client.test.ts > shouldAutoSelectCodexSidecar > respects an explicit auth file path
- packages/core/src/observer-client.test.ts > shouldAutoSelectCodexSidecar > selects codex_sidecar when all preconditions hold
- packages/core/src/observer-client.test.ts > shouldAutoSelectCodexSidecar > yields to any available API key
- packages/ui/src/tabs/settings/components/ObserverPanel.test.tsx > ObserverPanel > offers a local Codex runtime and shows its protected command
- packages/ui/src/tabs/settings/data/value-helpers.test.ts > Codex sidecar settings helpers > formats Codex-sidecar authentication status
- packages/ui/src/tabs/settings/data/value-helpers.test.ts > Codex sidecar settings helpers > infers the current Codex-sidecar default model
- packages/ui/src/tabs/settings/data/value-helpers.test.ts > Codex sidecar settings helpers > loads and saves shared observer reasoning defaults
- packages/ui/src/tabs/settings/data/value-helpers.test.ts > Codex sidecar settings helpers > loads the protected Codex command into form state
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > accepts the Codex sidecar runtime and exposes its protected command
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > does not report normalized command arrays as changed on unrelated saves
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > normalizes a string-form Codex command from the config file
- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > reports CODEMEM_CODEX_COMMAND as normalized env-managed config

### R-E2E (95)

- e2e/bin/dogfood.test.ts > dogfood runner > adds future memories only through the owner fixture action
- e2e/bin/dogfood.test.ts > dogfood runner > brings an offline target online once and refuses an already-online target
- e2e/bin/dogfood.test.ts > dogfood runner > captures logs without forwarding raw Compose output
- e2e/bin/dogfood.test.ts > dogfood runner > captures snapshots without printing payloads or private paths
- e2e/bin/dogfood.test.ts > dogfood runner > checks fixed resources before a fresh successful setup
- e2e/bin/dogfood.test.ts > dogfood runner > cleans up twice idempotently even when state is absent
- e2e/bin/dogfood.test.ts > dogfood runner > does not persist success state or print a checklist when enrollment fails
- e2e/bin/dogfood.test.ts > dogfood runner > does not persist success state or print a checklist when inspection fails
- e2e/bin/dogfood.test.ts > dogfood runner > does not persist success state or print a checklist when owner-fixture fails
- e2e/bin/dogfood.test.ts > dogfood runner > does not persist success state or print a checklist when readiness fails
- e2e/bin/dogfood.test.ts > dogfood runner > does not persist success state or print a checklist when recipient-configuration fails
- e2e/bin/dogfood.test.ts > dogfood runner > does not persist success state or print a checklist when second-device-fixture fails
- e2e/bin/dogfood.test.ts > dogfood runner > does not persist success state or print a checklist when teammate-fixture fails
- e2e/bin/dogfood.test.ts > dogfood runner > does not persist success state or print a checklist when up fails
- e2e/bin/dogfood.test.ts > dogfood runner > does not report log capture success when Compose logs fail
- e2e/bin/dogfood.test.ts > dogfood runner > maps lifecycle targets to fixed peer services
- e2e/bin/dogfood.test.ts > dogfood runner > preserves state and artifacts when cleanup teardown fails
- e2e/bin/dogfood.test.ts > dogfood runner > preserves state and does not start setup when reset teardown fails
- e2e/bin/dogfood.test.ts > dogfood runner > preserves state when offline Compose mutation fails
- e2e/bin/dogfood.test.ts > dogfood runner > preserves state when online Compose mutation fails
- e2e/bin/dogfood.test.ts > dogfood runner > preserves state when restart Compose mutation fails
- e2e/bin/dogfood.test.ts > dogfood runner > refuses 'add-future' mutation or diagnostics without state
- e2e/bin/dogfood.test.ts > dogfood runner > refuses 'logs' mutation or diagnostics without state
- e2e/bin/dogfood.test.ts > dogfood runner > refuses 'offline' mutation or diagnostics without state
- e2e/bin/dogfood.test.ts > dogfood runner > refuses 'snapshot' mutation or diagnostics without state
- e2e/bin/dogfood.test.ts > dogfood runner > refuses 'status' mutation or diagnostics without state
- e2e/bin/dogfood.test.ts > dogfood runner > refuses setup when fixed Compose resources exist without metadata
- e2e/bin/dogfood.test.ts > dogfood runner > refuses setup when fixed state exists without mutating the environment
- e2e/bin/dogfood.test.ts > dogfood runner > rejects invalid lifecycle transitions before Compose mutation
- e2e/bin/dogfood.test.ts > dogfood runner > rejects otherwise-valid peer proofs that reuse the same Identity
- e2e/bin/dogfood.test.ts > dogfood runner > rejects setup when peer fixture summaries do not prove distinct local Identities
- e2e/bin/dogfood.test.ts > dogfood runner > reports fixed viewer URLs and redacts hostile Compose diagnostics
- e2e/bin/dogfood.test.ts > dogfood runner > resets only the fixed sandbox before deterministic setup
- e2e/bin/dogfood.test.ts > dogfood runner > snapshots an intentionally offline peer without executing inside it
- e2e/bin/dogfood.test.ts > fixed runtime paths > builds an owner-only coordinator setup plan
- e2e/bin/dogfood.test.ts > fixed runtime paths > builds human-named recipient profile setup plans
- e2e/bin/dogfood.test.ts > fixed runtime paths > redacts every hostile repetition instead of only the first occurrence
- e2e/bin/dogfood.test.ts > fixed runtime paths > redacts repository paths and the fixed coordinator credential from diagnostics
- e2e/bin/dogfood.test.ts > fixed runtime paths > resolves both Compose files and dogfood artifacts from the repository root
- e2e/bin/dogfood.test.ts > manual checklist > prints all fixed viewer URLs and keeps invitations manual
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > parses only the approved shape 0
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > parses only the approved shape 1
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > parses only the approved shape 10
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > parses only the approved shape 11
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > parses only the approved shape 2
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > parses only the approved shape 3
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > parses only the approved shape 4
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > parses only the approved shape 5
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > parses only the approved shape 6
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > parses only the approved shape 7
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > parses only the approved shape 8
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > parses only the approved shape 9
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > rejects unsupported arguments 0
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > rejects unsupported arguments 1
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > rejects unsupported arguments 10
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > rejects unsupported arguments 2
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > rejects unsupported arguments 3
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > rejects unsupported arguments 4
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > rejects unsupported arguments 5
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > rejects unsupported arguments 6
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > rejects unsupported arguments 7
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > rejects unsupported arguments 8
- e2e/bin/dogfood.test.ts > parseDogfoodCommand > rejects unsupported arguments 9
- e2e/lib/compose.test.ts > ComposeManager > checks all fixed-project containers and labeled volumes with exact Docker arguments
- e2e/lib/compose.test.ts > ComposeManager > detects stopped fixed-project containers even when no labeled volume remains
- e2e/lib/compose.test.ts > ComposeManager > keeps multiple Compose files and fixed profiles in declared order
- e2e/lib/compose.test.ts > ComposeManager > preserves the legacy composeFile up arguments and defaults
- e2e/lib/compose.test.ts > ComposeManager > preserves the opt-in image build behavior
- e2e/lib/compose.test.ts > ComposeManager > reports no fixed-project resources only when containers and volumes are both absent
- e2e/lib/compose.test.ts > ComposeManager > runs service lifecycle operations with fixed command bounds
- e2e/lib/compose.test.ts > docker-compose.dogfood.yml > advertises recipient peers by their Compose-reachable service URLs
- e2e/lib/compose.test.ts > docker-compose.dogfood.yml > builds dogfood peers on the supported Node runtime
- e2e/lib/compose.test.ts > docker-compose.dogfood.yml > publishes exactly one loopback viewer port per peer
- e2e/scripts/dogfood-sharing-fixture.test.ts > parseFixtureAction > accepts only one explicit supported action
- e2e/scripts/dogfood-sharing-fixture.test.ts > parseFixtureAction > keeps fixture source free of invitation inspection and network calls
- e2e/scripts/dogfood-sharing-fixture.test.ts > parseFixtureAction > rejects malformed or unsupported arguments: ["--action","accept-invite"]
- e2e/scripts/dogfood-sharing-fixture.test.ts > parseFixtureAction > rejects malformed or unsupported arguments: ["--action","commit-invite"]
- e2e/scripts/dogfood-sharing-fixture.test.ts > parseFixtureAction > rejects malformed or unsupported arguments: ["--action","create-invite"]
- e2e/scripts/dogfood-sharing-fixture.test.ts > parseFixtureAction > rejects malformed or unsupported arguments: ["--action","inspect-invite"]
- e2e/scripts/dogfood-sharing-fixture.test.ts > parseFixtureAction > rejects malformed or unsupported arguments: ["--action","setup-owner","extra"]
- e2e/scripts/dogfood-sharing-fixture.test.ts > parseFixtureAction > rejects malformed or unsupported arguments: ["--action","unsupported-policy-mutation"]
- e2e/scripts/dogfood-sharing-fixture.test.ts > parseFixtureAction > rejects malformed or unsupported arguments: ["setup-owner"]
- e2e/scripts/dogfood-sharing-fixture.test.ts > parseFixtureAction > rejects malformed or unsupported arguments: []
- e2e/scripts/dogfood-sharing-fixture.test.ts > runFixtureAction > adds one idempotent future memory to each exact Project independently
- e2e/scripts/dogfood-sharing-fixture.test.ts > runFixtureAction > counts replicated memories whose sessions arrive without a git remote
- e2e/scripts/dogfood-sharing-fixture.test.ts > runFixtureAction > creates distinct stable human-named Identities for all three fresh peers
- e2e/scripts/dogfood-sharing-fixture.test.ts > runFixtureAction > ensures device identity before creating one human-named active local Identity
- e2e/scripts/dogfood-sharing-fixture.test.ts > runFixtureAction > initializes the second-device profile idempotently without owner Projects or policy state
- e2e/scripts/dogfood-sharing-fixture.test.ts > runFixtureAction > initializes the teammate profile idempotently without owner Projects or policy state
- e2e/scripts/dogfood-sharing-fixture.test.ts > runFixtureAction > keeps selected and unrelated Projects in exact separate sessions
- e2e/scripts/dogfood-sharing-fixture.test.ts > runFixtureAction > leaves invitation and recipient-policy tables empty after supported actions
- e2e/scripts/dogfood-sharing-fixture.test.ts > runFixtureAction > rejects add-future-selected before the owner baseline exists
- e2e/scripts/dogfood-sharing-fixture.test.ts > runFixtureAction > rejects add-future-unrelated before the owner baseline exists
- e2e/scripts/dogfood-sharing-fixture.test.ts > runFixtureAction > rejects changing an initialized profile to a different dogfood role
- e2e/scripts/dogfood-sharing-fixture.test.ts > runFixtureAction > sets up the owner idempotently with separated Projects and an empty Team

### R-WORKER (11)

- packages/cloudflare-coordinator-worker/src/index.test.ts > createCloudflareCoordinatorWorker > migration 0007 preserves legacy invites while adding project-intent columns
- packages/cloudflare-coordinator-worker/src/index.test.ts > createCloudflareCoordinatorWorker > migration 0008 preserves legacy invites while adding atomic binding columns
- packages/cloudflare-coordinator-worker/src/index.test.ts > createCloudflareCoordinatorWorker > migration 0009 classifies existing invites and adds recipient invitation metadata
- packages/cloudflare-coordinator-worker/src/index.test.ts > createCloudflareCoordinatorWorker > migration 0010 preserves audit history and adds immutable effect receipts
- packages/cloudflare-coordinator-worker/src/index.test.ts > createCloudflareCoordinatorWorker > migration 0011 adds nullable reviewed intent without changing existing invitations
- packages/cloudflare-coordinator-worker/src/index.test.ts > createCloudflareCoordinatorWorker > migration 0012 adds nullable enrollment identity binding and preserves existing rows
- packages/cloudflare-coordinator-worker/src/index.test.ts > createCloudflareCoordinatorWorker > migration 0013 adds nullable assigned Team identity and preserves existing invitations
- packages/cloudflare-coordinator-worker/src/index.test.ts > createCloudflareCoordinatorWorker > rejects oversized presence bodies before auth processing
- packages/cloudflare-coordinator-worker/src/index.test.ts > createCloudflareCoordinatorWorker > returns missing_d1_binding when the worker env is incomplete
- packages/cloudflare-coordinator-worker/src/index.test.ts > createCloudflareCoordinatorWorker > serves coordinator admin data through the worker entrypoint
- packages/cloudflare-coordinator-worker/src/index.test.ts > createCloudflareCoordinatorWorker > supports invite, join approval, signed presence, and signed peer lookup through the worker entrypoint

### R-CLI (50)

- packages/cli/src/command-tree.test.ts > root command tree > hides sync coordinator from help without unregistering it
- packages/cli/src/commands/config.test.ts > buildWorkspaceConfigPatch > builds a patch from supported sync options
- packages/cli/src/commands/config.test.ts > buildWorkspaceConfigPatch > ignores unset values and preserves disable-sync
- packages/cli/src/commands/config.test.ts > buildWorkspaceConfigPatch > rejects invalid numeric flags
- packages/cli/src/commands/config.test.ts > mergeWorkspaceConfig > merges the patch over existing config
- packages/cli/src/commands/config.test.ts > runWorkspaceConfigCommand > rejects conflicting sync enable flags
- packages/cli/src/commands/config.test.ts > runWorkspaceConfigCommand > rejects no-op workspace config writes
- packages/cli/src/commands/config.test.ts > runWorkspaceConfigCommand > seeds a first workspace config from the currently effective config
- packages/cli/src/commands/config.test.ts > runWorkspaceConfigCommand > supports the commander command path with json output
- packages/cli/src/commands/config.test.ts > runWorkspaceConfigCommand > writes the workspace config file and reports updated keys
- packages/cli/src/commands/db.test.ts > db command > prune-replication-ops --dry-run deletes nothing across all scopes
- packages/cli/src/commands/db.test.ts > db command > prune-replication-ops deletes old ops across ALL scopes, not just the default
- packages/cli/src/commands/mcp.test.ts > mcp command > classifies MCP HTTP validation failures as usage errors
- packages/cli/src/commands/mcp.test.ts > mcp command > exposes HTTP mode with host, port, and database options
- packages/cli/src/commands/mcp.test.ts > mcp command > forwards HTTP mode options to the MCP HTTP starter
- packages/cli/src/commands/mcp.test.ts > mcp command > forwards parent-level database options to HTTP mode
- packages/cli/src/commands/mcp.test.ts > mcp command > lets the MCP HTTP starter resolve env/default host and port
- packages/cli/src/commands/serve.test.ts > serve command option resolution > bounds enrollment failure details
- packages/cli/src/commands/serve.test.ts > serve command option resolution > reports reconciliation issues without claiming a group failure
- packages/cli/src/commands/serve.test.ts > serve command option resolution > runs enrollment and policy reconciliation before surfacing share maintenance failures
- packages/cli/src/commands/serve.test.ts > serve command option resolution > runs policy reconciliation before surfacing incomplete enrollment reconciliation
- packages/cli/src/commands/serve.test.ts > serve command option resolution > surfaces recipient-policy failures after running every maintenance stage
- packages/cli/src/commands/status.test.ts > status command > keeps warnings successful and exits zero for degraded reports
- packages/cli/src/commands/status.test.ts > status command > projects disabled sync, unconfigured observer, and subsystem failures
- packages/cli/src/commands/status.test.ts > status command > reads sync and observer presence from environment evidence
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > allows positional group ids for create-invite and list-join-requests
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > brackets IPv6 interface advertise addresses
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > brackets configured IPv6 advertise hosts
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > builds sync restart as a serve restart invocation
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > builds sync start as a background serve invocation using the current runner
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > builds sync start without host/port when not explicitly provided
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > collects advertise addresses from non-loopback interfaces when host is unspecified
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > defaults coordinator serve to the coordinator store database
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > documents the coordinator command surface in help output
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > drops empty project filter entries
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > formats sync once error output like the Python command
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > formats sync once success output like the Python command
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > includes the error suffix when present
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > initializes sync identity from CODEMEM_KEYS_DIR during sync enable
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > manages Sharing domains through coordinator CLI JSON commands
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > matches the compact Python-era output shape
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > parses comma-separated project filter lists
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > passes config path through sync lifecycle args
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > registers coordinator parity subcommands
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > registers peer repair subcommands
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > removes a peer by exact name
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > reports Sharing domain CLI errors as JSON
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > reports missing restored identity keys in sync doctor output
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > reports per-Space sync progress in sync status output
- packages/cli/src/commands/sync.test.ts > formatSyncAttempt > reports semantic-index diagnostics in sync status json output

### R-CORE (1137)

- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > adopts the add-device target identity on a fresh profile
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > binds add-device bootstrap trust to the coordinator response, not mutable invite metadata
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > creates and lists groups
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > creates, updates, lists, grants, and revokes local Sharing domain memberships
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > does not adopt the add-device target when the config write fails and converges on retry
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > does not warn for public-looking invite coordinator URLs
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > enrolls and lists devices for an existing group
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > fails closed for remote unreviewed or kind-inconsistent recipient evidence
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > falls back to the local actor identity and accepts additive accepted-intent fields
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > imports invites using CODEMEM_DB and CODEMEM_KEYS_DIR when flags are omitted
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > lists only consumed Team invites without tokens
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > maps remote missing Sharing domain membership revokes to false
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > normalizes omitted nullable fields from legacy remote device lists
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > persists and deduplicates coordinator config after 'Team' onboarding
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > persists and deduplicates coordinator config after 'add-device' onboarding
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > propagates add-device self-acceptance rejection without local persistence
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > re-enables a disabled device
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > recovers idempotently when a consumed project invite initially cannot enable sync
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > refuses to replace claimed-local or incompatible inviter trust
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects a configured add-device identity conflict before fetch or onboarding writes
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects a conflicting 'add_device' response Identity after valid inspection without local persistence
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects a conflicting 'team_member' response Identity after valid inspection without local persistence
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects a mismatched 'kind' returned by recipient invite inspection without local mutation
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects a mismatched 'reviewed digest' returned by recipient invite inspection without local mutation
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects a mismatched 'target ID' returned by recipient invite inspection without local mutation
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects an accepted Project intent with a digest mismatch before projection persistence
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects an accepted Project intent with a group mismatch before projection persistence
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects an accepted Project intent with a operation mismatch before projection persistence
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects an accepted Project intent with a tampered_project mismatch before projection persistence
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects enrollment into a missing group
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects local Sharing domain actions for missing groups or scopes
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects malformed non-null nullable remote device fields: {"display_name":0}
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects malformed non-null nullable remote device fields: {"display_name":false}
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects malformed non-null nullable remote device fields: {"identity_id":""}
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects malformed non-null nullable remote device fields: {"identity_id":0}
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects malformed remote device lists: {"items":"not-a-list"}
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects malformed remote device lists: {"items":[null]}
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects malformed remote device lists: {"items":null}
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects malformed remote device lists: {}
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects overlong remote device device_id values
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects overlong remote device identity_id values
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects stale 'add_device' onboarding before consuming the invite or mutating local state
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > rejects stale 'team_member' onboarding before consuming the invite or mutating local state
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > renames, disables, and removes devices
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > requires a reviewed onboarding digest before consuming a 'add_device' invite
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > requires a reviewed onboarding digest before consuming a 'team_member' invite
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > restores bootstrap config when add-device local commit fails and converges on retry
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > retries Team onboarding without optional display names for older coordinators
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > returns false when enabling a missing device
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > returns the renamed disabled device instead of null
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > sends canonical reviewed intent when creating remote recipient invites
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > sends remote Sharing domain admin requests with the admin secret
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > serializes only exact, well-formed remote device presence capability fields
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > signs the exact identity-owned add-device invite body without admin or target fields
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > stores canonical reviewed intent for local recipient invites without embedding it in links
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > surfaces signed add-device coordinator authorization failures without admin fallback
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > validates consumed add-device evidence against its reviewed target Identity
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > validates remote consumed Team invite identity bindings
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > warns when local invite coordinator URL looks private-only
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > warns when local invite coordinator URL uses link-local IPv6 space
- packages/core/src/coordinator-actions.test.ts > coordinator local admin actions > warns when local invite coordinator URL uses private IPv6 space
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > allows an identically bound project invite retry after expiry
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > creates Sharing domain metadata without accepting memory payloads
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > creates a reciprocal approval for the authenticated device
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > does not fail a persisted revoke when response enrichment cannot reload it
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > does not let invalid admin requests consume the authenticated admin bucket
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > does not rely on process env when runtime admin secret is unset
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > fails closed before consuming a project invite with malformed stored intent
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > fails closed when project-first acceptance omits identity confirmation
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > grants devices explicitly to a Sharing domain
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > inspects and consumes explicit Team invitations without enrollment or scope grants
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > lets an authenticated seed device inspect its bootstrap grant
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > lets enrolled devices read non-admin Sharing domain membership snapshots
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > lists Sharing domains for an admin-authenticated group
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > lists bootstrap grants for admins
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > lists explicit Sharing domain memberships separately from group enrollment
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > lists reciprocal approvals for the authenticated device
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > preserves add-device idempotent existing status
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > rate limits archived-group authentication failures
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > rate limits repeated coordinator reads before route handling continues
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > reconstructs a safe project invite link only while the coordinator token is unconsumed
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > rejects Sharing domain grants for devices outside the scope group
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > rejects add-device invites accepted by the inviter device
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > rejects an invalid invite expires_at with 400 instead of a 500
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > rejects grants for archived groups before mutating membership
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > rejects invalid project-invite identity fields before consuming the invite
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > rejects malformed Team invite device_display_name values
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > rejects malformed Team invite recipient_display_name values
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > rejects missing or invalid admin auth on Sharing domain membership routes
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > rejects missing or invalid admin auth on Sharing domain routes
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > rejects non-numeric Sharing domain epochs before coercion
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > rejects project identity fields on a legacy invite before enrollment
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > rejects project invites accepted by the inviter device
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > rejects revokes for archived groups before mutating membership
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > rejects signed presence and peer reads for archived groups
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > reports a newly consumed project invite as pending setup
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > reports persisted revoke epoch when request omits membership_epoch
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > returns not found when updating a scope outside the requested group
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > returns safe errors for unavailable or mismatched stored recipient reviews
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > revokes bootstrap grants for admins
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > revokes explicit Sharing domain memberships
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > signed add-device invitation creation > derives invitation authority from the signed enrollment and signs exact body bytes
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > signed add-device invitation creation > preserves bad-signature and nonce-replay authorization errors
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > signed add-device invitation creation > rejects cross-Identity, malformed, and digest-mismatched reviewed intents
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > signed add-device invitation creation > rejects disabled enrollments centrally before signature or nonce handling
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > signed add-device invitation creation > rejects every field outside the fixed add-device request contract
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > signed add-device invitation creation > rejects signed issuance from an enrollment without an Identity binding
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > signed add-device invitation creation > rejects signed issuance from an unknown device
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > updates Sharing domain metadata only within the requested group
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > uses injected admin secret and store factory for admin routes
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > validates Sharing domain create inputs
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > validates Sharing domain grant inputs
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > validates and retains an additive project-intent reference while legacy invites remain valid
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > validates recipient reviewed intent at creation and keeps invitation payloads digest-only
- packages/core/src/coordinator-api.test.ts > createCoordinatorApp dependency injection > writes audit rows through admin Sharing domain grant and revoke APIs
- packages/core/src/coordinator-enrollment-reconciler.test.ts > reconcileCoordinatorEnrollmentSnapshot > accepts local and sibling device enrollments after proving the local Identity binding
- packages/core/src/coordinator-enrollment-reconciler.test.ts > reconcileCoordinatorEnrollmentSnapshot > fails closed on conflicting or inactive owner policy state
- packages/core/src/coordinator-enrollment-reconciler.test.ts > reconcileCoordinatorEnrollmentSnapshot > hashes invalid remote reference IDs before returning or persisting diagnostics
- packages/core/src/coordinator-enrollment-reconciler.test.ts > reconcileCoordinatorEnrollmentSnapshot > materializes accepted Team membership and new devices idempotently
- packages/core/src/coordinator-enrollment-reconciler.test.ts > reconcileCoordinatorEnrollmentSnapshot > persists, deduplicates, resolves, and reopens issue lifecycle
- packages/core/src/coordinator-enrollment-reconciler.test.ts > reconcileCoordinatorEnrollmentSnapshot > preserves an owner-revoked Team membership when the consumed invite is replayed
- packages/core/src/coordinator-enrollment-reconciler.test.ts > reconcileCoordinatorEnrollmentSnapshot > preserves an owner-revoked device when its enrollment is replayed
- packages/core/src/coordinator-enrollment-reconciler.test.ts > reconcileCoordinatorEnrollmentSnapshot > refreshes coordinator-managed device names without overwriting local names
- packages/core/src/coordinator-enrollment-reconciler.test.ts > reconcileCoordinatorEnrollmentSnapshot > rejects a consumed Team invite bound to a local actor
- packages/core/src/coordinator-enrollment-reconciler.test.ts > reconcileCoordinatorEnrollmentSnapshot > rejects a coordinator device enrollment bound to a local actor
- packages/core/src/coordinator-enrollment-reconciler.test.ts > reconcileCoordinatorEnrollmentSnapshot > rolls policy and lifecycle changes back together
- packages/core/src/coordinator-enrollment-reconciler.test.ts > reconcileCoordinatorEnrollmentSnapshot > treats changed codes distinctly and isolates coordinator/group boundaries
- packages/core/src/coordinator-enrollment-reconciler.test.ts > reconcileCoordinatorEnrollmentSnapshot > uses legacy Team member and enrolled device fallbacks when names are absent
- packages/core/src/coordinator-enrollment-reconciler.test.ts > reconcileCoordinatorEnrollmentSnapshot > uses neutral fallbacks when optional presentation names are malformed
- packages/core/src/coordinator-enrollment-reconciliation-issues.test.ts > getCoordinatorEnrollmentReconciliationIssueSummary > rejects invalid diagnostic limit -1
- packages/core/src/coordinator-enrollment-reconciliation-issues.test.ts > getCoordinatorEnrollmentReconciliationIssueSummary > rejects invalid diagnostic limit 0
- packages/core/src/coordinator-enrollment-reconciliation-issues.test.ts > getCoordinatorEnrollmentReconciliationIssueSummary > rejects invalid diagnostic limit 1.5
- packages/core/src/coordinator-enrollment-reconciliation-issues.test.ts > getCoordinatorEnrollmentReconciliationIssueSummary > rejects invalid diagnostic limit 101
- packages/core/src/coordinator-enrollment-reconciliation-issues.test.ts > getCoordinatorEnrollmentReconciliationIssueSummary > returns an empty bounded summary
- packages/core/src/coordinator-enrollment-reconciliation-issues.test.ts > getCoordinatorEnrollmentReconciliationIssueSummary > returns safe columns in open-first recency order and honors the bound
- packages/core/src/coordinator-group-preferences.test.ts > coordinator group preferences > delete removes the row and returns whether it existed
- packages/core/src/coordinator-group-preferences.test.ts > coordinator group preferences > derives a deterministic default Space scope id
- packages/core/src/coordinator-group-preferences.test.ts > coordinator group preferences > empty arrays normalize to null (no include filter)
- packages/core/src/coordinator-group-preferences.test.ts > coordinator group preferences > list returns only rows for the matching coordinator
- packages/core/src/coordinator-group-preferences.test.ts > coordinator group preferences > rejects empty coordinator_id or group_id
- packages/core/src/coordinator-group-preferences.test.ts > coordinator group preferences > returns null for an unknown group
- packages/core/src/coordinator-group-preferences.test.ts > coordinator group preferences > upsert creates a row with defaults filled in
- packages/core/src/coordinator-group-preferences.test.ts > coordinator group preferences > upsert preserves explicit default Space settings
- packages/core/src/coordinator-group-preferences.test.ts > coordinator group preferences > upsert updates projects + auto_seed_scope in place
- packages/core/src/coordinator-invites.test.ts > coordinator-invites > decodeInvitePayload > throws on non-object payload
- packages/core/src/coordinator-invites.test.ts > coordinator-invites > encode/decode round-trip > round-trips a full payload
- packages/core/src/coordinator-invites.test.ts > coordinator-invites > encode/decode round-trip > round-trips a payload with null team_name
- packages/core/src/coordinator-invites.test.ts > coordinator-invites > extractInvitePayload > extracts from a codemem:// link
- packages/core/src/coordinator-invites.test.ts > coordinator-invites > extractInvitePayload > full round-trip: encode → link → extract → decode
- packages/core/src/coordinator-invites.test.ts > coordinator-invites > extractInvitePayload > returns raw value if not a codemem:// link
- packages/core/src/coordinator-invites.test.ts > coordinator-invites > extractInvitePayload > throws if codemem:// link has no invite param
- packages/core/src/coordinator-invites.test.ts > coordinator-invites > extractInvitePayload > trims whitespace
- packages/core/src/coordinator-invites.test.ts > coordinator-invites > inviteLink > produces a codemem:// link
- packages/core/src/coordinator-runtime.test.ts > advertisedSyncAddresses > deduplicates bare host and explicit sync port after port inference
- packages/core/src/coordinator-runtime.test.ts > advertisedSyncAddresses > infers the configured sync port for bare advertised hostnames
- packages/core/src/coordinator-runtime.test.ts > advertisedSyncAddresses > preserves explicit ports in advertised URLs
- packages/core/src/coordinator-runtime.test.ts > coordinatorStatusSnapshot > reuses a short-lived coordinator snapshot instead of polling remote status on every viewer refresh
- packages/core/src/coordinator-runtime.test.ts > coordinatorStatusSnapshot > reuses private peer bindings when cached authorization becomes valid
- packages/core/src/coordinator-runtime.test.ts > fetchCoordinatorStalePeers > returns a stale pinned peer key when the same device has a fresh replacement fingerprint
- packages/core/src/coordinator-runtime.test.ts > lookupCoordinatorPeers > merges multi-group device groups as plain strings
- packages/core/src/coordinator-runtime.test.ts > lookupCoordinatorPeers > tracks freshness per group when merged device sightings disagree
- packages/core/src/coordinator-runtime.test.ts > readCoordinatorSyncConfig raw-events retention > clamps max-age below 1 up to 1
- packages/core/src/coordinator-runtime.test.ts > readCoordinatorSyncConfig raw-events retention > defaults to disabled with a 90-day max age when nothing is supplied
- packages/core/src/coordinator-runtime.test.ts > readCoordinatorSyncConfig raw-events retention > honors the env vars over config
- packages/core/src/coordinator-runtime.test.ts > readCoordinatorSyncConfig raw-events retention > marks retention configured only when the enabled key is explicitly present
- packages/core/src/coordinator-runtime.test.ts > readCoordinatorSyncConfig raw-events retention > reads enabled + max-age from the config object
- packages/core/src/coordinator-runtime.test.ts > readCoordinatorSyncConfig.syncOpsLimit > clamps values above the server cap of 1000
- packages/core/src/coordinator-runtime.test.ts > readCoordinatorSyncConfig.syncOpsLimit > clamps values below 1 up to 1
- packages/core/src/coordinator-runtime.test.ts > readCoordinatorSyncConfig.syncOpsLimit > defaults to 500 when neither config nor env supplies a value
- packages/core/src/coordinator-runtime.test.ts > readCoordinatorSyncConfig.syncOpsLimit > falls back to the default when the config value is not an integer
- packages/core/src/coordinator-runtime.test.ts > readCoordinatorSyncConfig.syncOpsLimit > honors the CODEMEM_SYNC_OPS_LIMIT env var over config
- packages/core/src/coordinator-runtime.test.ts > readCoordinatorSyncConfig.syncOpsLimit > reads the value from the sync_ops_limit config key
- packages/core/src/coordinator-runtime.test.ts > refreshStoredCoordinatorPeerAddresses > does not create peers for coordinator-only discovered devices
- packages/core/src/coordinator-runtime.test.ts > refreshStoredCoordinatorPeerAddresses > does not refresh addresses from stale coordinator input
- packages/core/src/coordinator-runtime.test.ts > refreshStoredCoordinatorPeerAddresses > does not refresh when the discovered fingerprint differs from the pinned peer
- packages/core/src/coordinator-runtime.test.ts > refreshStoredCoordinatorPeerAddresses > merges fresh multi-group coordinator addresses into an existing pinned peer
- packages/core/src/coordinator-runtime.test.ts > trustCoordinatorPeersWithSharedManagedScopes > does not revoke invite-derived or manually approved coordinator trust
- packages/core/src/coordinator-runtime.test.ts > trustCoordinatorPeersWithSharedManagedScopes > pins only the key discovered through the shared scope authority when a conflicting key appears first
- packages/core/src/coordinator-runtime.test.ts > trustCoordinatorPeersWithSharedManagedScopes > refreshes reciprocal trust after both devices gain the managed scope
- packages/core/src/coordinator-runtime.test.ts > trustCoordinatorPeersWithSharedManagedScopes > rejects discovered peers with missing or mismatched coordinator authority metadata
- packages/core/src/coordinator-runtime.test.ts > trustCoordinatorPeersWithSharedManagedScopes > trusts a discovered peer only when local policy grants both devices the managed scope
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > bootstrap grants > creates and retrieves a bootstrap grant
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > bootstrap grants > lists bootstrap grants for a group
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > bootstrap grants > revokes a bootstrap grant
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > devices > disables and re-enables a device
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > devices > keeps direct admin enrollment unbound to an identity
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > devices > lists enrolled devices
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > devices > removes a device and its presence
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > devices > renames a device
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > devices > returns false when removing a non-existent device
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > devices > returns null for missing enrollment
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > devices > upserts on re-enroll
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > groups > INSERT OR IGNORE on duplicate group_id
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > groups > archives and unarchives a group
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > groups > creates and retrieves a group
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > groups > lists groups
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > groups > renames a group
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > groups > returns null for missing group
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > atomically binds a project invite and returns the same acceptance only to the same identity
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > backfills only an exact legacy consumed project-invite enrollment binding
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > creates an invite and retrieves by token
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > distinguishes expired and invalid project invite acceptance
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > does not let a consumed invite retry re-enable an admin-disabled device
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > fails closed for archived groups and mismatched public-key fingerprints
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > fails closed for malformed coordinator-assigned Team identities
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > fails closed when recipient invitations expire or their coordinator group is archived
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > lists invites for a group
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > persists, enrolls, and single-use binds explicit Team and add-device invitations without scope membership
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > recovers an add-device bootstrap grant after the seed device is re-enabled
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > recovers pending inviter bootstrap once and reuses it across retries
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > reissues an expired project invite without changing its operation identity
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > rejects a project invite that conflicts with a disabled enrollment identity
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > rejects a revoked 'add_device' invite after an accepted replay without changing its binding
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > rejects a revoked 'add_device' invite before first inspection or acceptance
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > rejects a revoked 'team_member' invite after an accepted replay without changing its binding
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > rejects a revoked 'team_member' invite before first inspection or acceptance
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > rejects first recipient invite use after expiry
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > rejects reuse of an operation id with different reviewed intent
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > rejects the loser of concurrent conflicting-intent operation creation
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > replays only the exact consumed recipient binding after expiry
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > reports one accepted result for concurrent identical consumes
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > requires reviewed intent for new recipient invites and fails inspection for migrated null snapshots
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > retains project-intent references and retries the same operation idempotently
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > returns null for unknown token
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > returns one invite for concurrent same-intent operation creation
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > invites > treats project invite expiry as output when retrying the same operation
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > join requests > approves a join request and enrolls the device
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > join requests > can mint a bootstrap grant when approving a join request
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > join requests > creates a join request in pending status
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > join requests > denies a join request without enrolling
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > join requests > lists pending join requests
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > join requests > returns _no_transition for already-reviewed request
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > join requests > returns null for missing request_id
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > nonces > allows nonce cleanup and reuse after cutoff
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > nonces > records a nonce once and rejects replay
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > nonces > scopes nonce replay checks by device
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > presence > lists group peers excluding requesting device
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > presence > marks stale presence with empty addresses
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > presence > shows enrolled peers with no presence record
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > presence > upserts presence and returns normalized data
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > reciprocal approvals > completes the reverse pending approval when the second device also approves
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > reciprocal approvals > creates and lists a pending outgoing reciprocal approval
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > reciprocal approvals > surfaces incoming pending reciprocal approvals for the requested device
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > scope memberships > creates and lists scopes with explicit authority fields
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > scope memberships > fails closed when an effect id is reused for a different request
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > scope memberships > grants and revokes explicit device membership per scope
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > scope memberships > keeps grants and revocations isolated per scope
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > scope memberships > keeps group enrollment separate from scope grants
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > scope memberships > keeps group presence independent from scope revocation
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > scope memberships > keeps membership epochs monotonic across revoke and re-grant
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > scope memberships > rejects duplicate scope ids instead of silently changing authority
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > scope memberships > rejects first-time grant epochs below the scope epoch
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > scope memberships > rejects scope grants with mismatched authority fields
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > scope memberships > replays identical effects without changing membership or audit history
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > contract > scope memberships > requires scope members to be enrolled in the scope group
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > schema > adds nullable enrollment identity binding while preserving pre-column rows
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > schema > backfills project invite columns before creating their index
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > schema > creates all expected tables
- packages/core/src/coordinator-store.test.ts > CoordinatorStore > schema > upgrades an existing project-intent invite table additively
- packages/core/src/d1-coordinator-runtime.test.ts > createD1CoordinatorApp > rejects join requests with mismatched fingerprint and public key
- packages/core/src/d1-coordinator-runtime.test.ts > createD1CoordinatorApp > serves coordinator admin data from the D1-backed adapter
- packages/core/src/d1-coordinator-runtime.test.ts > createD1CoordinatorApp > uses injected runtime admin secret instead of process env
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > bootstrap grants > creates and retrieves a bootstrap grant
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > bootstrap grants > lists bootstrap grants for a group
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > bootstrap grants > revokes a bootstrap grant
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > devices > disables and re-enables a device
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > devices > keeps direct admin enrollment unbound to an identity
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > devices > lists enrolled devices
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > devices > removes a device and its presence
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > devices > renames a device
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > devices > returns false when removing a non-existent device
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > devices > returns null for missing enrollment
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > devices > upserts on re-enroll
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > groups > INSERT OR IGNORE on duplicate group_id
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > groups > archives and unarchives a group
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > groups > creates and retrieves a group
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > groups > lists groups
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > groups > renames a group
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > groups > returns null for missing group
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > atomically binds a project invite and returns the same acceptance only to the same identity
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > backfills only an exact legacy consumed project-invite enrollment binding
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > creates an invite and retrieves by token
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > distinguishes expired and invalid project invite acceptance
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > does not let a consumed invite retry re-enable an admin-disabled device
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > fails closed for archived groups and mismatched public-key fingerprints
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > fails closed for malformed coordinator-assigned Team identities
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > fails closed when recipient invitations expire or their coordinator group is archived
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > lists invites for a group
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > persists, enrolls, and single-use binds explicit Team and add-device invitations without scope membership
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > recovers an add-device bootstrap grant after the seed device is re-enabled
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > recovers pending inviter bootstrap once and reuses it across retries
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > reissues an expired project invite without changing its operation identity
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > rejects a project invite that conflicts with a disabled enrollment identity
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > rejects a revoked 'add_device' invite after an accepted replay without changing its binding
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > rejects a revoked 'add_device' invite before first inspection or acceptance
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > rejects a revoked 'team_member' invite after an accepted replay without changing its binding
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > rejects a revoked 'team_member' invite before first inspection or acceptance
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > rejects first recipient invite use after expiry
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > rejects reuse of an operation id with different reviewed intent
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > rejects the loser of concurrent conflicting-intent operation creation
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > replays only the exact consumed recipient binding after expiry
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > reports one accepted result for concurrent identical consumes
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > requires reviewed intent for new recipient invites and fails inspection for migrated null snapshots
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > retains project-intent references and retries the same operation idempotently
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > returns null for unknown token
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > returns one invite for concurrent same-intent operation creation
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > invites > treats project invite expiry as output when retrying the same operation
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > join requests > approves a join request and enrolls the device
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > join requests > can mint a bootstrap grant when approving a join request
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > join requests > creates a join request in pending status
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > join requests > denies a join request without enrolling
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > join requests > lists pending join requests
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > join requests > returns _no_transition for already-reviewed request
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > join requests > returns null for missing request_id
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > nonces > allows nonce cleanup and reuse after cutoff
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > nonces > records a nonce once and rejects replay
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > nonces > scopes nonce replay checks by device
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > presence > lists group peers excluding requesting device
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > presence > marks stale presence with empty addresses
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > presence > shows enrolled peers with no presence record
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > presence > upserts presence and returns normalized data
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > reciprocal approvals > completes the reverse pending approval when the second device also approves
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > reciprocal approvals > creates and lists a pending outgoing reciprocal approval
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > reciprocal approvals > surfaces incoming pending reciprocal approvals for the requested device
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > scope memberships > creates and lists scopes with explicit authority fields
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > scope memberships > fails closed when an effect id is reused for a different request
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > scope memberships > grants and revokes explicit device membership per scope
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > scope memberships > keeps grants and revocations isolated per scope
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > scope memberships > keeps group enrollment separate from scope grants
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > scope memberships > keeps group presence independent from scope revocation
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > scope memberships > keeps membership epochs monotonic across revoke and re-grant
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > scope memberships > rejects duplicate scope ids instead of silently changing authority
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > scope memberships > rejects first-time grant epochs below the scope epoch
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > scope memberships > rejects scope grants with mismatched authority fields
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > scope memberships > replays identical effects without changing membership or audit history
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > contract > scope memberships > requires scope members to be enrolled in the scope group
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > converges concurrent identical D1 effects to one receipt and one audit event
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > converges to completed when a reverse pending row appears during insert
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > dedupes mirrored pending rows before creating the pending-pair unique index
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > excludes expired invites and includes unexpired ones by token
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > leaves no partial deletion when the device removal batch fails mid-way
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > normalizes and validates bootstrap grant input consistently
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > rejects an unparseable invite expiry
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > removes presence, reciprocal approvals, and enrollment atomically
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > returns false when removing a non-existent device
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > returns false without audit when D1 revoke loses the guarded update race
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > rolls back D1 membership changes when the audit batch fails
- packages/core/src/d1-coordinator-store.test.ts > D1CoordinatorStore > stores a non-canonical invite expiry in canonical UTC form
- packages/core/src/db.test.ts > ensureAdditiveSchemaCompatibility schema-compat gate > repairs partially-created share-operation tables before marking compatibility
- packages/core/src/legacy-recipient-policy-projection.test.ts > legacy recipient-policy projection > does not use a waiting-for-acceptance invite as exact-project evidence
- packages/core/src/legacy-recipient-policy-projection.test.ts > legacy recipient-policy projection > does not use exact-project invite evidence after its reviewed digest becomes stale
- packages/core/src/legacy-recipient-policy-projection.test.ts > legacy recipient-policy projection > excludes an active membership whose epoch is stale for its active scope
- packages/core/src/legacy-recipient-policy-projection.test.ts > legacy recipient-policy projection > exposes a coordinator group only as a non-authoritative Team candidate
- packages/core/src/legacy-recipient-policy-projection.test.ts > legacy recipient-policy projection > includes the local runtime for an active custom local-authority scope
- packages/core/src/legacy-recipient-policy-projection.test.ts > legacy recipient-policy projection > isolates canonical Projects that share the same display name
- packages/core/src/legacy-recipient-policy-projection.test.ts > legacy recipient-policy projection > keeps an unassigned member device visible as current effective access
- packages/core/src/legacy-recipient-policy-projection.test.ts > legacy recipient-policy projection > lets the selected exact workspace mapping override an older matching wildcard
- packages/core/src/legacy-recipient-policy-projection.test.ts > legacy recipient-policy projection > offers local and personal evidence only as an actionable suggestion
- packages/core/src/legacy-recipient-policy-projection.test.ts > legacy recipient-policy projection > prefers a more specific pattern over a newer broad pattern at the same priority
- packages/core/src/legacy-recipient-policy-projection.test.ts > legacy recipient-policy projection > projects one exact canonical Project from one active managed scope
- packages/core/src/legacy-recipient-policy-projection.test.ts > legacy recipient-policy projection > treats a catch-all legacy mapping as ambiguous instead of inferring recipients
- packages/core/src/legacy-recipient-policy-projection.test.ts > legacy recipient-policy projection > under-shares an ambiguous scope containing multiple canonical Projects
- packages/core/src/legacy-recipient-policy-projection.test.ts > legacy recipient-policy projection > uses only read operations under query_only without changing total_changes
- packages/core/src/observer-config.test.ts > getCodememEnvOverrides > includes sync retention env overrides when set
- packages/core/src/operational-status.test.ts > collectOperationalStatus > reports daemon errors when the additive phase column is absent
- packages/core/src/project-invite-identity.test.ts > project invite identity > rejects empty, overlong, and control-character identity labels
- packages/core/src/project-invite-identity.test.ts > project invite identity > retains only safe project summaries
- packages/core/src/project-invite-identity.test.ts > project invite identity > uses the approved friendly device-name precedence
- packages/core/src/project-scope-settings.test.ts > project scope settings > allows assignment of reserved-prefix identities when a real local row owns them
- packages/core/src/project-scope-settings.test.ts > project scope settings > assigns a canonical project identity without granting membership
- packages/core/src/project-scope-settings.test.ts > project scope settings > checks basename collisions beyond the default candidate list when confirming assignments
- packages/core/src/project-scope-settings.test.ts > project scope settings > counts only active memories in project inventory
- packages/core/src/project-scope-settings.test.ts > project scope settings > does not double-list a replicated session whose memories gained a project later
- packages/core/src/project-scope-settings.test.ts > project scope settings > does not guess when multiple scopes match a project signal equally
- packages/core/src/project-scope-settings.test.ts > project scope settings > does not let peer-received duplicates inherit local mapping semantics
- packages/core/src/project-scope-settings.test.ts > project scope settings > does not mark same-basename worktrees as persistent needs-attention inventory
- packages/core/src/project-scope-settings.test.ts > project scope settings > does not reassign peer-owned-only project rows
- packages/core/src/project-scope-settings.test.ts > project scope settings > does not require basename collision confirmation for local-only assignments
- packages/core/src/project-scope-settings.test.ts > project scope settings > does not suggest org domains from generic category tokens only
- packages/core/src/project-scope-settings.test.ts > project scope settings > falls back from git remote to cwd when suggesting mappings
- packages/core/src/project-scope-settings.test.ts > project scope settings > groups peer-received memories from multiple origin devices by their managed scope
- packages/core/src/project-scope-settings.test.ts > project scope settings > includes explicitly mapped projects with no recent sessions
- packages/core/src/project-scope-settings.test.ts > project scope settings > includes workspace-id-only and unmapped sessions with memories as local-only
- packages/core/src/project-scope-settings.test.ts > project scope settings > keeps cwds that only coincidentally match the bootstrap LIKE-wildcard shape
- packages/core/src/project-scope-settings.test.ts > project scope settings > keeps mapping-only project identities editable when received projects share a name
- packages/core/src/project-scope-settings.test.ts > project scope settings > keeps peer-received identities separate from local project-like identities
- packages/core/src/project-scope-settings.test.ts > project scope settings > keeps peer-received rows separate when local workspace ids use the reserved prefix
- packages/core/src/project-scope-settings.test.ts > project scope settings > keeps project-less replicated sessions visible via their workspace identity
- packages/core/src/project-scope-settings.test.ts > project scope settings > keeps project-less siblings visible when a replicated session is partially upgraded
- packages/core/src/project-scope-settings.test.ts > project scope settings > lists local sharing-domain defaults and unknown projects as local-only
- packages/core/src/project-scope-settings.test.ts > project scope settings > lists project inventory when every memory arrived from sync bootstrap
- packages/core/src/project-scope-settings.test.ts > project scope settings > lists searchable project inventory after identity dedupe
- packages/core/src/project-scope-settings.test.ts > project scope settings > marks projects from incremental replication sessions as peer received
- packages/core/src/project-scope-settings.test.ts > project scope settings > normalizes incoming workspace identities before matching existing mappings
- packages/core/src/project-scope-settings.test.ts > project scope settings > propagates rows that matched a project Space mapping before it was edited
- packages/core/src/project-scope-settings.test.ts > project scope settings > propagates source-owned project Space assignments into syncable memory ops
- packages/core/src/project-scope-settings.test.ts > project scope settings > reassigns local sessions without memory rows
- packages/core/src/project-scope-settings.test.ts > project scope settings > reassigns sessions for a stable workspace identity to the corrected project
- packages/core/src/project-scope-settings.test.ts > project scope settings > rejects basename-only pattern mappings
- packages/core/src/project-scope-settings.test.ts > project scope settings > rejects direct assignment of peer-received project identities without local rows
- packages/core/src/project-scope-settings.test.ts > project scope settings > rejects inert unmapped and legacy-review assignments
- packages/core/src/project-scope-settings.test.ts > project scope settings > rejects mappings to inactive or unknown Sharing domains
- packages/core/src/project-scope-settings.test.ts > project scope settings > requires a current device id before reassigning a project
- packages/core/src/project-scope-settings.test.ts > project scope settings > suggests mappings from canonical signals without saving them
- packages/core/src/project-scope-settings.test.ts > project scope settings > surfaces bootstrap memory projects without exposing synthetic session cwd
- packages/core/src/project-scope-settings.test.ts > project scope settings > warns before saving broad home-directory patterns to org domains
- packages/core/src/project-scope-settings.test.ts > project scope settings > warns that scope reassignment may leave old copies behind
- packages/core/src/project-scope-settings.test.ts > project scope settings > warns when same-basename projects need review before assignment
- packages/core/src/recipient-policy-contract.test.ts > recipient policy V1 contract > distinguishes direct Identity and Team recipients
- packages/core/src/recipient-policy-contract.test.ts > recipient policy V1 contract > keeps Personal and Work as distinct Identities
- packages/core/src/recipient-policy-contract.test.ts > recipient policy V1 contract > keeps Team membership Identity-based
- packages/core/src/recipient-policy-contract.test.ts > recipient policy V1 contract > keeps authorization shortcuts out of recipient intent
- packages/core/src/recipient-policy-contract.test.ts > recipient policy V1 contract > keeps rejection and access-preserving migration as explicit decisions
- packages/core/src/recipient-policy-contract.test.ts > recipient policy V1 contract > makes keeping current setup an actionable outcome
- packages/core/src/recipient-policy-contract.test.ts > recipient policy V1 contract > maps each device edge to exactly one Identity
- packages/core/src/recipient-policy-contract.test.ts > recipient policy V1 contract > separates intent, effective devices, and enforcement in projections
- packages/core/src/recipient-policy-contract.test.ts > recipient policy V1 contract > uses a fixed contract version
- packages/core/src/recipient-policy-contract.test.ts > recipient policy V1 contract > uses canonical Project identity instead of display name
- packages/core/src/recipient-policy-edges.test.ts > recipient-policy edge changes > adds, removes, preserves row identity metadata, and keeps unchanged previews idempotent
- packages/core/src/recipient-policy-edges.test.ts > recipient-policy edge changes > fails closed when another connection holds the write lock
- packages/core/src/recipient-policy-edges.test.ts > recipient-policy edge changes > keeps preview read-only and rejects display names, unmapped identities, and inactive recipients
- packages/core/src/recipient-policy-edges.test.ts > recipient-policy edge changes > makes a second overlapping preview stale after the first commit changes desired edges
- packages/core/src/recipient-policy-edges.test.ts > recipient-policy edge changes > normalizes project-first and recipient-first ordering identically and writes identical rows
- packages/core/src/recipient-policy-edges.test.ts > recipient-policy edge changes > rejects a remove preview when the reviewed edge is removed before commit
- packages/core/src/recipient-policy-edges.test.ts > recipient-policy edge changes > rejects an already-absent preview when the edge becomes active before commit
- packages/core/src/recipient-policy-edges.test.ts > recipient-policy edge changes > rejects an already-present preview when the reviewed edge changes before commit
- packages/core/src/recipient-policy-edges.test.ts > recipient-policy edge changes > rejects stale digests after membership, device, memory, or other edge changes
- packages/core/src/recipient-policy-edges.test.ts > recipient-policy edge changes > removes an exact active edge for a Project absent from current facts
- packages/core/src/recipient-policy-edges.test.ts > recipient-policy edge changes > removes an exact active edge for a deactivated identity without weakening add validation
- packages/core/src/recipient-policy-edges.test.ts > recipient-policy edge changes > removes an exact active edge for a merged identity without weakening add validation
- packages/core/src/recipient-policy-edges.test.ts > recipient-policy edge changes > removes an exact active edge for an archived Team without allowing stale additions
- packages/core/src/recipient-policy-edges.test.ts > recipient-policy edge changes > rolls back the whole transaction and never mutates protected tables
- packages/core/src/recipient-policy-edges.test.ts > recipient-policy edge changes > shows Team current members, future inheritance, and resulting effective devices
- packages/core/src/recipient-policy-edges.test.ts > recipient-policy edge changes > strictly parses only the canonical direction-free request
- packages/core/src/recipient-policy-migration.test.ts > recipient policy intent migration > applies fingerprint-bound attach-device intent without automatic operation evidence
- packages/core/src/recipient-policy-migration.test.ts > recipient policy intent migration > applies recipient choices against the exact device-scoped review preview
- packages/core/src/recipient-policy-migration.test.ts > recipient policy intent migration > blocks a digest mismatch without a partial graph write
- packages/core/src/recipient-policy-migration.test.ts > recipient policy intent migration > fails closed when one device is already assigned to another Identity
- packages/core/src/recipient-policy-migration.test.ts > recipient policy intent migration > ignores non-local and unaccepted operations when applying valid exact-project evidence
- packages/core/src/recipient-policy-migration.test.ts > recipient policy intent migration > keeps Personal and Work actor IDs and same-name canonical Projects isolated
- packages/core/src/recipient-policy-migration.test.ts > recipient policy intent migration > keeps preserved diagnostic-only Projects skipped without migration evidence
- packages/core/src/recipient-policy-migration.test.ts > recipient policy intent migration > keeps reviewed preserve-current Projects on legacy enforcement
- packages/core/src/recipient-policy-migration.test.ts > recipient policy intent migration > lets preserve-current dominate automatic evidence and sibling review choices
- packages/core/src/recipient-policy-migration.test.ts > recipient policy intent migration > mints a policy Team distinct from a coordinator group and projects memberships
- packages/core/src/recipient-policy-migration.test.ts > recipient policy intent migration > performs no writes in dry-run mode
- packages/core/src/recipient-policy-migration.test.ts > recipient policy intent migration > requires a current review resolution and applies a local Identity recommendation
- packages/core/src/recipient-policy-migration.test.ts > recipient policy intent migration > reuses one created Identity for the same unassigned device across Projects
- packages/core/src/recipient-policy-migration.test.ts > recipient policy intent migration > revalidates exact operation digests, writes direct intent, and replays idempotently
- packages/core/src/recipient-policy-migration.test.ts > recipient policy intent migration > rolls back a created actor and device when its Project recipient conflicts
- packages/core/src/recipient-policy-migration.test.ts > recipient policy intent migration > skips stale resolved review rows
- packages/core/src/recipient-policy-migration.test.ts > recipient policy intent migration > treats durable keep-current review outcomes as migration no-ops
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > adopts and replays a truly empty actorless bootstrap
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > atomically commits the complete direct-share owner and recipient graph
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > atomically materializes and replays the minimum fresh Team graph
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > builds fresh Team and add-device previews only from reviewed intent and local binding
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > commits Team membership plus device atomically and exactly retries idempotently
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > commits add-device as the only intent write
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > commits exact direct recipients plus device without Team membership
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > does not stale a reviewed decision when only an excluded Project count changes
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > excludes a projected Project with non-deterministic boundary
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > excludes a projected Project with revoked accepting-device membership
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > excludes a projected Project with revoked projection
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > excludes a projected Project with stale accepting-device membership
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > excludes a projected Project with wrong Identity
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > excludes a projected Project with wrong coordinator boundary
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > includes a zero-memory recipient projection without owner-policy rows
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > keeps a zero-reference bootstrap Identity pristine with a human-friendly display name
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > key-binds a compatible exact-Project inviter device on recipient onboarding
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > matches normalized coordinator URLs for recipient projections
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > preserves compatible migration rows and writes only new recipient intent
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > previews Team Projects, memory counts, exclusions, and future inheritance without writes
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > previews direct Projects exactly and add-device direct plus Team inheritance
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rejects a changed device key across distinct invitations
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rejects a non-active inviter device binding
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rejects a revoked inviter Project edge without partial recipient intent
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rejects add-device adoption when a sync peer has a claimed-local assignment
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rejects add-device adoption when a sync peer has an actor assignment
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rejects add-device adoption when fallback-actor memories exist without an actor row
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rejects an exact-Project device transition without matching local key different-public-key
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rejects an exact-Project device transition without matching local key null
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rejects changed key, device, or Identity on invitation retry
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rejects established-profile adoption without writes
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rejects one device mapped to another Identity
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rejects pre-projection evidence with ambiguous local Identity state
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rejects pre-projection evidence with ambiguous local device state
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rejects pre-projection evidence with non-local Identity
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rejects pre-projection evidence with revoked membership
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rejects pre-projection evidence with wrong coordinator boundary
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rejects snapshot digest and device-key conflicts atomically
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > reuses an identical device binding across direct and Team invitations
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rolls back add-device adoption when identity-device insertion fails
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rolls back every intent row when a later write fails
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rolls back recipient intent when a reviewed inviter device belongs to another Identity
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > rolls back snapshot materialization when a later row fails
- packages/core/src/recipient-policy-onboarding.test.ts > recipient-policy onboarding > uses exact active managed-scope evidence for pre-projection recipients
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > allows legacy retries while active authority still matches policy
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > blocks legacy grants when active authority excludes a policy-desired device
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > bounds failed capability steps across repeated undetermined preflights
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > bounds failed capability steps across repeated unsupported preflights
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > cancels a stale generation after revokes and before any grant
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > clears a freshly active re-enabled device overlay before capability preflight
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > clears an abandoned deny overlay after a re-desired device is freshly verified active
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > continues policy revokes and stages enrollment denials after a refresh failure
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > does not grant a policy device that is not enrolled in the boundary group
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > does not grant when the enrollment Identity changes during preflight
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > does not grant when the enrollment becomes disabled during preflight
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > does not revoke a current policy device merely omitted from boundary enrollments
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > does not rewrite capability effect failures as undetermined
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > does not rewrite lease loss during capability preflight as undetermined
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > does not treat an enabled enrollment without an Identity as a revocation signal
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > keeps a deny overlay outside the active managed boundary
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > keeps a deny overlay until a fresh snapshot actually proves revocation
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > keeps a deny overlay when active membership lacks matching enabled enrollment proof
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > keeps a deny overlay when matching enrollment remains disabled
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > keeps a deny overlay without fresh active membership proof
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > leaves the same device active in another managed Project group
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > preserves active authority while capability evidence is undetermined
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > preserves active authority while fresh parity remains incomplete
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > preserves active authority while the coordinator snapshot is not fresh
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > re-grants stale active membership and withholds authority until its epoch is current
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > recovers an expired lease but leaves an unexpired foreign lease untouched
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > refreshes disabled-member revocations before unsupported capability returns
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > refreshes each distinct enrollment revocation reason for the same snapshot
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > rejects ambiguous exact-Project mappings before reading a coordinator snapshot
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > replays a pending refresh through the current boundary after a scope remap
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > retries a failed coordinator mutation with the same deterministic effect identity
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > retries a failed revocation refresh without repeating the completed revoke
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > retries an incomplete ordinary refresh through a remapped boundary
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > retries an incomplete ordinary refresh without creating another step
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > revokes a current device whose boundary enrollment changed Identity
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > revokes a new grant immediately when its enrollment Identity changes
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > revokes a new grant immediately when its enrollment becomes disabled
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > revokes an explicitly disabled current member without mutating global owner policy
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > revokes before grants, verifies parity, and activates only on a later no-op pass
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > revokes owner-policy removals when the boundary enrollment read fails
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > revokes removals before rejecting an unsupported grant candidate
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > rolls active authority back without granting or clearing a pending deny
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > stages every disabled-member deny overlay before the first revoke
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > stages new revocations before retrying an older failed refresh
- packages/core/src/recipient-policy-reconciler.test.ts > recipient-policy reconciler executor > waits without mutations when a capability is undetermined
- packages/core/src/recipient-policy-reconciliation.test.ts > recipient-policy reconciliation persistence > allows same-generation deny reasons to become stricter without accepting stale writes
- packages/core/src/recipient-policy-reconciliation.test.ts > recipient-policy reconciliation persistence > bounds ordinary refresh lookup without selecting revocation refreshes
- packages/core/src/recipient-policy-reconciliation.test.ts > recipient-policy reconciliation persistence > bounds pending revocation refresh lookup while preserving deterministic order
- packages/core/src/recipient-policy-reconciliation.test.ts > recipient-policy reconciliation persistence > creates deterministic generation-scoped steps and rejects conflicting reuse
- packages/core/src/recipient-policy-reconciliation.test.ts > recipient-policy reconciliation persistence > drains completed bookkeeping overflow with tied timestamps
- packages/core/src/recipient-policy-reconciliation.test.ts > recipient-policy reconciliation persistence > keeps deny overlays keyed by exact Project, scope, and device until verified
- packages/core/src/recipient-policy-reconciliation.test.ts > recipient-policy reconciliation persistence > persists attempt, error, and lease timestamps without changing authority
- packages/core/src/recipient-policy-reconciliation.test.ts > recipient-policy reconciliation persistence > persists observations idempotently without promoting legacy authority
- packages/core/src/recipient-policy-reconciliation.test.ts > recipient-policy reconciliation persistence > prunes only incomplete capability steps from superseded passes
- packages/core/src/recipient-policy-reconciliation.test.ts > recipient-policy reconciliation persistence > prunes only old completed capability and refresh bookkeeping
- packages/core/src/recipient-policy-reconciliation.test.ts > recipient-policy reconciliation persistence > stores stable parity evidence without changing authority
- packages/core/src/recipient-policy-reconciliation.test.ts > strict recipient-policy effective-device derivation > blocks all grant candidates when a team has an orphan active member
- packages/core/src/recipient-policy-reconciliation.test.ts > strict recipient-policy effective-device derivation > blocks the whole Project for a deactivated direct identity
- packages/core/src/recipient-policy-reconciliation.test.ts > strict recipient-policy effective-device derivation > blocks the whole Project for a merged direct identity
- packages/core/src/recipient-policy-reconciliation.test.ts > strict recipient-policy effective-device derivation > blocks the whole Project for a missing direct identity
- packages/core/src/recipient-policy-reconciliation.test.ts > strict recipient-policy effective-device derivation > blocks the whole Project for a pending direct identity
- packages/core/src/recipient-policy-reconciliation.test.ts > strict recipient-policy effective-device derivation > deduplicates an exact device reached directly and through a team
- packages/core/src/recipient-policy-reconciliation.test.ts > strict recipient-policy effective-device derivation > derives direct and team devices without enrollment, trust, or filter inputs
- packages/core/src/recipient-policy-reconciliation.test.ts > strict recipient-policy effective-device derivation > uses exact canonical Project identity and ignores sibling recipients
- packages/core/src/recipient-policy-review.test.ts > recipient policy review fingerprint > ignores labels and transport-only fields while changing for semantic state
- packages/core/src/recipient-policy-review.test.ts > recipient policy review fingerprint > is deterministic and order-insensitive
- packages/core/src/recipient-policy-review.test.ts > recipient policy review fingerprint > isolates same-name Projects by canonical identity
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > derives safe exact options and performs no writes under query_only
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > emits only repairable cards for mixed diagnostics and keeps blocked IDs stable
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > fails closed when the deciding local Identity is unavailable
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > ignores unrelated transport metadata changes in the current source fingerprint
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > is idempotent for matching input and fails closed for conflicting re-resolution
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > keeps a future_project_boundary multi-project scope repairable
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > keeps a managed_project multi-project scope repairable
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > keeps a wildcard scope mapping as continuity without repair cards
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > keeps an ambiguous umbrella scope as continuity without repair cards
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > keeps durable no-op history through memory churn and reopens on semantic change
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > keeps unassigned-device resolutions durable and scoped to one device
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > maps diagnostics to Blocked items without resolve options
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > normalizes and stores attach_device_to_identity decision input
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > normalizes and stores choose_recipients decision input
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > normalizes and stores create_identity decision input
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > normalizes and stores remove_stale_device decision input
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > records only the immutable resolution with server-derived attribution
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > rejects stale fingerprints without writing
- packages/core/src/recipient-policy-review.test.ts > recipient policy review persistence > validates decision input and resolves bulk items independently in request order
- packages/core/src/recipient-reviewed-intent.test.ts > recipient reviewed intent > accepts more than 100 presentation-only excluded Projects within the byte limit
- packages/core/src/recipient-reviewed-intent.test.ts > recipient reviewed intent > canonicalizes project and source ordering before digesting
- packages/core/src/recipient-reviewed-intent.test.ts > recipient reviewed intent > fails closed for missing or malformed stored snapshots
- packages/core/src/recipient-reviewed-intent.test.ts > recipient reviewed intent > normalizes valid Team and add-device reviewed intents
- packages/core/src/recipient-reviewed-intent.test.ts > recipient reviewed intent > rejects digest mismatches
- packages/core/src/recipient-reviewed-intent.test.ts > recipient reviewed intent > rejects invitation target mismatches
- packages/core/src/recipient-reviewed-intent.test.ts > recipient reviewed intent > rejects malformed or duplicate input: duplicate included Project
- packages/core/src/recipient-reviewed-intent.test.ts > recipient reviewed intent > rejects malformed or duplicate input: duplicate source
- packages/core/src/recipient-reviewed-intent.test.ts > recipient reviewed intent > rejects malformed or duplicate input: included and excluded Project overlap
- packages/core/src/recipient-reviewed-intent.test.ts > recipient reviewed intent > rejects malformed or duplicate input: missing Team
- packages/core/src/recipient-reviewed-intent.test.ts > recipient reviewed intent > rejects malformed or duplicate input: negative memory count
- packages/core/src/recipient-reviewed-intent.test.ts > recipient reviewed intent > rejects malformed or duplicate input: overlong target Identity display name
- packages/core/src/recipient-reviewed-intent.test.ts > recipient reviewed intent > rejects malformed or duplicate input: unknown version
- packages/core/src/scope-membership-cache.test.ts > scope membership cache > allows regrant only when the membership epoch advances
- packages/core/src/scope-membership-cache.test.ts > scope membership cache > caches coordinator scopes and active memberships for deterministic device lookups
- packages/core/src/scope-membership-cache.test.ts > scope membership cache > detects stale membership epochs without listing the scope as authorized
- packages/core/src/scope-membership-cache.test.ts > scope membership cache > distinguishes fresh no-authorization from stale unknown authorization
- packages/core/src/scope-membership-cache.test.ts > scope membership cache > does not authorize a membership from a different requested authority
- packages/core/src/scope-membership-cache.test.ts > scope membership cache > does not authorize active memberships when scope metadata is missing
- packages/core/src/scope-membership-cache.test.ts > scope membership cache > does not let stale active grants resurrect revoked memberships
- packages/core/src/scope-membership-cache.test.ts > scope membership cache > filters device lookups by requested coordinator and group authority
- packages/core/src/scope-membership-cache.test.ts > scope membership cache > hydrates the cache through enrolled-device coordinator endpoints without an admin secret
- packages/core/src/scope-membership-cache.test.ts > scope membership cache > includes revocation limitation payloads for revoked memberships
- packages/core/src/scope-membership-cache.test.ts > scope membership cache > keeps an exact policy deny ahead of stale active membership until foundation clearing
- packages/core/src/scope-membership-cache.test.ts > scope membership cache > keeps cached authorization visible as stale when coordinator refresh fails
- packages/core/src/scope-membership-cache.test.ts > scope membership cache > normalizes missing coordinator authority from refreshed remote payloads
- packages/core/src/scope-membership-cache.test.ts > scope membership cache > reconciles omitted scopes on a successful authoritative refresh
- packages/core/src/scope-membership-cache.test.ts > scope membership cache > reconciles removed memberships on a successful authoritative refresh
- packages/core/src/scope-membership-cache.test.ts > scope membership cache > restores unchanged peer memberships when an enrolled device regains scope visibility
- packages/core/src/scope-membership-semantics.test.ts > scope membership semantics > classifies stale, current, and unknown membership epochs
- packages/core/src/scope-membership-semantics.test.ts > scope membership semantics > explains revocation limits without promising remote deletion
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > distinguishes capability evidence waits from an offline recipient
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > does not age never-attempted setup from invite creation
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > does not describe a successfully reconnected recipient as offline while setup resumes
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > does not infer recent reachability from display-only last-seen metadata
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > keeps a running capability retry classified as a compatibility wait
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > keeps a terminal reviewed-intent failure actionable without exposing its code
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > keeps an offline recipient passive regardless of age and attempts
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > keeps offline copy when the device was last reached at the wait boundary
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > keeps offline copy when the device was last reached before the wait
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > keeps offline copy when the device was last reached outside the reachability window
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > keeps offline copy when the device was last reached with an invalid timestamp
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > maps safe failure codes without leaking technical traces
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > moves failed, exhausted, and stale non-device work to needs attention
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > moves failed, exhausted, and stale non-device work to needs attention
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > moves failed, exhausted, and stale non-device work to needs attention
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > offers Copy invite only when the safe link is available
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > projects accepted as provisioning
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > projects active as active
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > projects cancelled with its required recovery action
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > projects initial_sync as initial_sync
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > projects provisioning as provisioning
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > projects revoked with its required recovery action
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > projects revoking as revoking
- packages/core/src/share-operation-lifecycle.test.ts > projectShareLifecycle > warns that revocation cannot recall copied memories
- packages/core/src/share-operation.test.ts > share-operation persistence > accepts additive wire fields while preserving known accepted Project intent
- packages/core/src/share-operation.test.ts > share-operation persistence > accepts compatible migrated inviter policy without rewriting its metadata
- packages/core/src/share-operation.test.ts > share-operation persistence > accepts historical canonical Project identity /Users/example/workspace/api
- packages/core/src/share-operation.test.ts > share-operation persistence > accepts historical canonical Project identity git:https://example.invalid/acme/api.git
- packages/core/src/share-operation.test.ts > share-operation persistence > accepts historical canonical Project identity https://git.example.invalid/acme/api.git
- packages/core/src/share-operation.test.ts > share-operation persistence > accepts historical canonical Project identity workspace:acme/api
- packages/core/src/share-operation.test.ts > share-operation persistence > does not regress a provisioning lifecycle on authoritative acceptance replay
- packages/core/src/share-operation.test.ts > share-operation persistence > persists reviewed intent, pending Person, counts, expiry, and steps idempotently
- packages/core/src/share-operation.test.ts > share-operation persistence > persists separate teammate invites for the same managed projects
- packages/core/src/share-operation.test.ts > share-operation persistence > reconciles authoritative acceptance into the pending Person and device without name matching
- packages/core/src/share-operation.test.ts > share-operation persistence > rejects a recipient device already assigned to another Person
- packages/core/src/share-operation.test.ts > share-operation persistence > rejects a recipient device already locally claimed
- packages/core/src/share-operation.test.ts > share-operation persistence > rejects acceptance when the persisted inviter does not match the local runtime
- packages/core/src/share-operation.test.ts > share-operation persistence > rejects malformed or mismatched authoritative project intent
- packages/core/src/share-operation.test.ts > share-operation persistence > rejects pending acceptance that collides with an existing active Person
- packages/core/src/share-operation.test.ts > share-operation persistence > rejects retries when any persisted reviewed intent changes
- packages/core/src/share-operation.test.ts > share-operation persistence > rejects unmapped canonical Project identities
- packages/core/src/share-operation.test.ts > share-operation persistence > rolls back acceptance when a reviewed inviter device is bound to another Identity
- packages/core/src/share-operation.test.ts > share-operation persistence > updates invite credentials when a waiting operation is reissued
- packages/core/src/share-operation.test.ts > share-operation persistence > uses stable friendly inviter device names without persisting raw long IDs
- packages/core/src/share-operation.test.ts > share-operation planner > canonicalizes project/device permutations and binds only authorization-relevant identity
- packages/core/src/share-operation.test.ts > share-operation planner > digests canonical identities and active-memory counts but excludes labels
- packages/core/src/share-operation.test.ts > share-operation planner > links an existing Person without replacing its identity
- packages/core/src/share-operation.test.ts > share-operation planner > plans deterministic exact-project intent and idempotency metadata
- packages/core/src/share-operation.test.ts > share-operation planner > rejects empty and duplicate project selections
- packages/core/src/share-operation.test.ts > share-operation planner > reuses an explicitly matched pending Person across new share operations
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > blocks stale legacy grant retries before any mutation when active policy changed
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > cancels superseded accepted duplicates for the same person and project set once active
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > derives bounded membership without inheriting unreviewed source members
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > does not let a superseded invocation finishing late cancel newer duplicates or reactivate
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > fails capability preflight before any boundary, migration, or mapping mutation
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > fails closed when a reviewed inviter device no longer passes current project filters
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > includes locally authored legacy origins without adopting replicated local sentinels
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > keeps a superseded operation cancelled when its in-flight step fails afterwards
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > keeps undetermined capability probes retryable while the device is offline
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > migrates only selected memories, persists exact future mapping, refreshes, and observes scoped sync
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > moves a non-device provisioning step to needs-attention after three failed attempts
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > moves capability preflight to needs-attention after three failed attempts
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > preflights legacy project-filtered peers for local-default reassignment
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > preserves a pre-existing step effect_id while executing it
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > preserves completed effects and resumes from the failed grant
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > rejects deterministic boundary reuse when authority or group conflicts
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > reopens capability preflight when the required device set changes
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > resumes idempotently across migration, mapping, refresh, and initial-sync failures
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > revalidates a persisted membership plan before retrying grants
- packages/core/src/share-provisioning.test.ts > exact project share provisioning > waits for an offline device and resumes initial sync without repeating completed work
- packages/core/src/store.test.ts > MemoryStore > rebootstrap mutation guard > allows forget() for private memories when phase is needs_attention
- packages/core/src/store.test.ts > MemoryStore > rebootstrap mutation guard > allows remember() for private memories when phase is needs_attention
- packages/core/src/store.test.ts > MemoryStore > rebootstrap mutation guard > allows remember() for shared memories when phase is null (normal)
- packages/core/src/store.test.ts > MemoryStore > rebootstrap mutation guard > allows shared memory writes when only sync identity is impaired
- packages/core/src/store.test.ts > MemoryStore > rebootstrap mutation guard > allows updateMemoryVisibility() to private when phase is needs_attention
- packages/core/src/store.test.ts > MemoryStore > rebootstrap mutation guard > blocks forget() for shared-visibility memories when phase is needs_attention
- packages/core/src/store.test.ts > MemoryStore > rebootstrap mutation guard > blocks remember() for shared-visibility memories when phase is needs_attention
- packages/core/src/store.test.ts > MemoryStore > rebootstrap mutation guard > blocks remember() with default visibility (shared) when phase is needs_attention
- packages/core/src/store.test.ts > MemoryStore > rebootstrap mutation guard > blocks updateMemoryVisibility() to shared when phase is needs_attention
- packages/core/src/store.test.ts > MemoryStore > rebootstrap mutation guard > unblocks mutations after phase is cleared
- packages/core/src/store.test.ts > MemoryStore > remember > never persists original secrets to the replication_ops payload
- packages/core/src/store.test.ts > MemoryStore > remember > reassigns memory scope with an old-scope tombstone and new-scope upsert
- packages/core/src/store.test.ts > MemoryStore > remember > returns the existing id before shared-write blocking when a duplicate already exists
- packages/core/src/store.test.ts > MemoryStore > remember > stamps local-default scope on new memory and replication op by default
- packages/core/src/store.test.ts > MemoryStore > remember > stamps mapped scope on new memory and replication op
- packages/core/src/summary-dedup-backfill.test.ts > summary-dedup backfill > emits replication delete ops for superseded rows
- packages/core/src/sync-auth.test.ts > sync-auth > buildCanonicalRequest > includes all components in correct order
- packages/core/src/sync-auth.test.ts > sync-auth > buildCanonicalRequest > produces deterministic output
- packages/core/src/sync-auth.test.ts > sync-auth > buildCanonicalRequest > uppercases the method
- packages/core/src/sync-auth.test.ts > sync-auth > cleanupNonces > removes old entries and keeps recent ones
- packages/core/src/sync-auth.test.ts > sync-auth > recordNonce > rejects same nonce even from different devices
- packages/core/src/sync-auth.test.ts > sync-auth > recordNonce > returns false on duplicate nonce for same device
- packages/core/src/sync-auth.test.ts > sync-auth > recordNonce > succeeds on first insert
- packages/core/src/sync-auth.test.ts > sync-auth > signRequest + verifySignature > buildAuthHeaders signs with an explicit device ID without consulting the default DB
- packages/core/src/sync-auth.test.ts > sync-auth > signRequest + verifySignature > rejects expired timestamp
- packages/core/src/sync-auth.test.ts > sync-auth > signRequest + verifySignature > rejects tampered body
- packages/core/src/sync-auth.test.ts > sync-auth > signRequest + verifySignature > rejects wrong signature version
- packages/core/src/sync-auth.test.ts > sync-auth > signRequest + verifySignature > round-trips: sign then verify
- packages/core/src/sync-auth.test.ts > sync-auth > signRequest + verifySignature > signs with the enrolled keychain identity when device.key belongs to another identity
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > 'apply' rejects 'local-default scope' rows on default bootstrap before mutating local state
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > 'apply' rejects 'managed scope' rows on default bootstrap before mutating local state
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > 'apply' rejects 'non-string scope' rows on default bootstrap before mutating local state
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > 'apply' rejects 'null scope' rows on default bootstrap before mutating local state
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > 'merge' rejects 'local-default scope' rows on default bootstrap before mutating local state
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > 'merge' rejects 'managed scope' rows on default bootstrap before mutating local state
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > 'merge' rejects 'non-string scope' rows on default bootstrap before mutating local state
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > 'merge' rejects 'null scope' rows on default bootstrap before mutating local state
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > applies empty snapshot (wipes shared, inserts nothing)
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > bumps generation and snapshot_id to match peer
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > empty unscoped bootstrap preserves locally-originated local-only rows
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > handles tombstoned snapshot items correctly
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > merge recovery breaks rev/updated_at ties on clock_device_id
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > merge recovery preserves stale rows when a newer snapshot item is malformed
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > preserves private memories during bootstrap
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > preserves snapshot payload scope_id on inserted memories
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > queues a persisted vector backfill job for bootstrap catch-up
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > redacts secrets in inbound bootstrap snapshot items
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > relies on ON DELETE SET NULL during normal foreign-key-enforced bootstrap
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > replaces only the requested scope during scoped bootstrap
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > replaces shared memories with snapshot items
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > scoped bootstrap replaces only the requested managed scope
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > updates replication cursor to baseline_cursor
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > uses the requested managed scope when payload scope_id is non-string
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > validates exposure identity when bootstrap reuses a row ID for 'a different import identity'
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > validates exposure identity when bootstrap reuses a row ID for 'a different origin identity'
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > validates exposure identity when bootstrap reuses a row ID for 'an exposure without stable identity'
- packages/core/src/sync-bootstrap.test.ts > applyBootstrapSnapshot > validates exposure identity when bootstrap reuses a row ID for 'the same stable identity'
- packages/core/src/sync-bootstrap.test.ts > fetchAllSnapshotPages > fetches and applies every page for an empty authorized scope
- packages/core/src/sync-bootstrap.test.ts > fetchAllSnapshotPages > forwards bootstrap grant id as an auth header
- packages/core/src/sync-capability.test.ts > additive sync features > keeps the existing capability rank while negotiating reassign_scope independently
- packages/core/src/sync-daemon.test.ts > createSerializedDaemonTickRunner > does not overlap maintenance callbacks across serialized ticks
- packages/core/src/sync-daemon.test.ts > getSyncDaemonPhase / setSyncDaemonPhase > clears phase when set to null
- packages/core/src/sync-daemon.test.ts > getSyncDaemonPhase / setSyncDaemonPhase > persists and retrieves identity_error phase
- packages/core/src/sync-daemon.test.ts > getSyncDaemonPhase / setSyncDaemonPhase > persists and retrieves needs_attention phase
- packages/core/src/sync-daemon.test.ts > getSyncDaemonPhase / setSyncDaemonPhase > returns null when daemon state row exists but phase is null
- packages/core/src/sync-daemon.test.ts > getSyncDaemonPhase / setSyncDaemonPhase > returns null when no phase is set
- packages/core/src/sync-daemon.test.ts > getSyncDaemonPhase / setSyncDaemonPhase > setSyncDaemonOk clears an active phase
- packages/core/src/sync-daemon.test.ts > refreshCoordinatorPresenceForDaemon > applies additive schema compatibility before daemon tick state writes
- packages/core/src/sync-daemon.test.ts > refreshCoordinatorPresenceForDaemon > does not refresh authorized peer trust when membership refresh fails
- packages/core/src/sync-daemon.test.ts > refreshCoordinatorPresenceForDaemon > does nothing when coordinator sync is not configured
- packages/core/src/sync-daemon.test.ts > refreshCoordinatorPresenceForDaemon > keeps direct peer sync running when coordinator heartbeat fails
- packages/core/src/sync-daemon.test.ts > refreshCoordinatorPresenceForDaemon > posts coordinator presence and refreshes scope membership cache when enabled
- packages/core/src/sync-daemon.test.ts > refreshCoordinatorPresenceForDaemon > records maintenance failure and continues normal peer sync
- packages/core/src/sync-daemon.test.ts > refreshCoordinatorPresenceForDaemon > runs tick maintenance after coordinator refresh and before peer sync
- packages/core/src/sync-daemon.test.ts > refreshCoordinatorPresenceForDaemon > threads CODEMEM_KEYS_DIR through one-off ticks when keysDir is omitted
- packages/core/src/sync-daemon.test.ts > resolveSyncDaemonKeysDir > falls back to CODEMEM_KEYS_DIR when the caller omits keysDir
- packages/core/src/sync-daemon.test.ts > resolveSyncDaemonKeysDir > prefers the explicit daemon keysDir over CODEMEM_KEYS_DIR
- packages/core/src/sync-daemon.test.ts > runSyncDaemon identity recovery > records identity_error and keeps retrying when restored private keys are missing
- packages/core/src/sync-daemon.test.ts > runSyncDaemon identity recovery > records identity_error and recovers when device.key cannot be read
- packages/core/src/sync-daemon.test.ts > setSyncDaemonError > records error and traceback
- packages/core/src/sync-daemon.test.ts > setSyncDaemonError > upserts on subsequent errors
- packages/core/src/sync-daemon.test.ts > setSyncDaemonOk > inserts daemon ok state
- packages/core/src/sync-daemon.test.ts > setSyncDaemonOk > updates daemon ok state on subsequent calls
- packages/core/src/sync-daemon.test.ts > syncDaemonTick > forwards the workspace scanner to runSyncPass so inbound peer apply uses workspace rules
- packages/core/src/sync-daemon.test.ts > syncDaemonTick > returns empty array when no peers exist
- packages/core/src/sync-daemon.test.ts > syncDaemonTick > runs sync for each peer
- packages/core/src/sync-daemon.test.ts > syncDaemonTick > skips offline peers in backoff
- packages/core/src/sync-daemon.test.ts > syncDaemonTick > skips only the matching stale pinned coordinator identity
- packages/core/src/sync-daemon.test.ts > syncDaemonTick > skips peers with expired coordinator presence
- packages/core/src/sync-daemon.test.ts > syncDaemonTick > syncs all peers when stalePeers set is empty
- packages/core/src/sync-daemon.test.ts > syncDaemonTick > threads syncOpsLimit from coordinator config into runSyncPass
- packages/core/src/sync-discovery.test.ts > addressDedupeKey > keeps scheme and path in the dedupe key
- packages/core/src/sync-discovery.test.ts > addressDedupeKey > keeps scheme for host:port
- packages/core/src/sync-discovery.test.ts > addressDedupeKey > returns as-is for non-URL input
- packages/core/src/sync-discovery.test.ts > addressDedupeKey > returns empty for empty input
- packages/core/src/sync-discovery.test.ts > formatHostPort > brackets IPv6 literals before appending the port
- packages/core/src/sync-discovery.test.ts > mDNS runtime hooks > returns no entries and no-op close when mDNS is disabled
- packages/core/src/sync-discovery.test.ts > mdnsAddressesForPeer > brackets IPv6 resolved addresses
- packages/core/src/sync-discovery.test.ts > mdnsAddressesForPeer > extracts addresses matching peer device ID
- packages/core/src/sync-discovery.test.ts > mdnsAddressesForPeer > handles Uint8Array device_id
- packages/core/src/sync-discovery.test.ts > mdnsAddressesForPeer > preserves binary TXT device IDs for matching
- packages/core/src/sync-discovery.test.ts > mdnsAddressesForPeer > returns empty for no matching peer
- packages/core/src/sync-discovery.test.ts > mdnsAddressesForPeer > skips entries without device_id property
- packages/core/src/sync-discovery.test.ts > mdnsEnabled > config sync_mdns=false keeps it disabled when env is unset
- packages/core/src/sync-discovery.test.ts > mdnsEnabled > config sync_mdns=true enables when env is unset
- packages/core/src/sync-discovery.test.ts > mdnsEnabled > env '0' explicitly disables even if config says enabled
- packages/core/src/sync-discovery.test.ts > mdnsEnabled > env '1' wins over missing config
- packages/core/src/sync-discovery.test.ts > mdnsEnabled > env 'true' wins over missing config
- packages/core/src/sync-discovery.test.ts > mdnsEnabled > returns false with no env and no config
- packages/core/src/sync-discovery.test.ts > mergeAddresses > deduplicates addresses
- packages/core/src/sync-discovery.test.ts > mergeAddresses > filters out empty after normalization
- packages/core/src/sync-discovery.test.ts > mergeAddresses > preserves order (existing first)
- packages/core/src/sync-discovery.test.ts > normalizeAddress > can infer the configured sync port for http advertised hosts
- packages/core/src/sync-discovery.test.ts > normalizeAddress > does not infer the sync port over an explicit default http port
- packages/core/src/sync-discovery.test.ts > normalizeAddress > handles http:// prefix without port
- packages/core/src/sync-discovery.test.ts > normalizeAddress > normalizes host:port to http://host:port
- packages/core/src/sync-discovery.test.ts > normalizeAddress > normalizes host:port with uppercase
- packages/core/src/sync-discovery.test.ts > normalizeAddress > preserves scheme and strips default port
- packages/core/src/sync-discovery.test.ts > normalizeAddress > returns empty for empty input
- packages/core/src/sync-discovery.test.ts > normalizeAddress > returns empty for invalid port
- packages/core/src/sync-discovery.test.ts > normalizeAddress > strips trailing slashes and default port
- packages/core/src/sync-discovery.test.ts > peer address storage > deduplicates on merge
- packages/core/src/sync-discovery.test.ts > peer address storage > fills missing trust material without replacing existing trust by default
- packages/core/src/sync-discovery.test.ts > peer address storage > merges new addresses on subsequent updates
- packages/core/src/sync-discovery.test.ts > peer address storage > replaces stale trust material when accepting a fresh pairing payload
- packages/core/src/sync-discovery.test.ts > peer address storage > returns empty for unknown peer
- packages/core/src/sync-discovery.test.ts > peer address storage > round-trips addresses through update/load
- packages/core/src/sync-discovery.test.ts > recordPeerSuccess > handles null address gracefully
- packages/core/src/sync-discovery.test.ts > recordPeerSuccess > promotes successful address to front
- packages/core/src/sync-discovery.test.ts > recordSyncAttempt > records a failed attempt with error
- packages/core/src/sync-discovery.test.ts > recordSyncAttempt > records a successful attempt and clears last_error
- packages/core/src/sync-discovery.test.ts > selectDialAddresses > puts mDNS first when available
- packages/core/src/sync-discovery.test.ts > selectDialAddresses > returns stored when no mDNS
- packages/core/src/sync-http-client.test.ts > buildBaseUrl > adds http:// when no scheme is present
- packages/core/src/sync-http-client.test.ts > buildBaseUrl > preserves http:// scheme
- packages/core/src/sync-http-client.test.ts > buildBaseUrl > preserves https:// scheme
- packages/core/src/sync-http-client.test.ts > buildBaseUrl > returns empty string for empty/blank input
- packages/core/src/sync-http-client.test.ts > buildBaseUrl > trims whitespace and trailing slashes
- packages/core/src/sync-http-client.test.ts > requestJson > handles non-JSON response gracefully
- packages/core/src/sync-http-client.test.ts > requestJson > handles unexpected JSON type (array)
- packages/core/src/sync-http-client.test.ts > requestJson > passes custom headers
- packages/core/src/sync-http-client.test.ts > requestJson > returns null body for empty response
- packages/core/src/sync-http-client.test.ts > requestJson > returns parsed JSON on success
- packages/core/src/sync-http-client.test.ts > requestJson > sets Accept header and omits Content-Type for bodyless requests
- packages/core/src/sync-identity.test.ts > sync-identity > ensureDeviceIdentity > accepts a matching stored public key with an SSH comment
- packages/core/src/sync-identity.test.ts > sync-identity > ensureDeviceIdentity > creates new device in fresh DB
- packages/core/src/sync-identity.test.ts > sync-identity > ensureDeviceIdentity > fails closed when a restored database has no private key
- packages/core/src/sync-identity.test.ts > sync-identity > ensureDeviceIdentity > fails closed when restored key material belongs to another identity
- packages/core/src/sync-identity.test.ts > sync-identity > ensureDeviceIdentity > fails closed when restored private key material is corrupt
- packages/core/src/sync-identity.test.ts > sync-identity > ensureDeviceIdentity > falls back to a matching keychain key when device.key cannot be read
- packages/core/src/sync-identity.test.ts > sync-identity > ensureDeviceIdentity > falls back to a valid keychain key when device.key is corrupt
- packages/core/src/sync-identity.test.ts > sync-identity > ensureDeviceIdentity > falls back to a valid keychain key when device.key is missing
- packages/core/src/sync-identity.test.ts > sync-identity > ensureDeviceIdentity > falls back to a valid keychain key when device.key is non-Ed25519
- packages/core/src/sync-identity.test.ts > sync-identity > ensureDeviceIdentity > prefers a matching restored key file over another identity keychain material and repopulates the keychain
- packages/core/src/sync-identity.test.ts > sync-identity > ensureDeviceIdentity > prefers a matching restored key file over corrupt keychain material and repopulates the keychain
- packages/core/src/sync-identity.test.ts > sync-identity > ensureDeviceIdentity > recreates only the public key when the restored private key matches the database
- packages/core/src/sync-identity.test.ts > sync-identity > ensureDeviceIdentity > rejects non-Ed25519 private key material
- packages/core/src/sync-identity.test.ts > sync-identity > ensureDeviceIdentity > restores the same identity and signs coordinator and direct-peer requests
- packages/core/src/sync-identity.test.ts > sync-identity > ensureDeviceIdentity > returns existing device on second call
- packages/core/src/sync-identity.test.ts > sync-identity > ensureDeviceIdentity > uses a matching keychain key when a valid restored file belongs to another identity
- packages/core/src/sync-identity.test.ts > sync-identity > ensureDeviceIdentity > uses provided deviceId
- packages/core/src/sync-identity.test.ts > sync-identity > fingerprintPublicKey > different keys produce different fingerprints
- packages/core/src/sync-identity.test.ts > sync-identity > fingerprintPublicKey > produces consistent SHA-256 hex
- packages/core/src/sync-identity.test.ts > sync-identity > generateKeypair > creates key files on disk
- packages/core/src/sync-identity.test.ts > sync-identity > generateKeypair > is idempotent when keys exist
- packages/core/src/sync-identity.test.ts > sync-identity > loadPrivateKey > reads generated private key
- packages/core/src/sync-identity.test.ts > sync-identity > loadPrivateKey > returns null when file does not exist
- packages/core/src/sync-identity.test.ts > sync-identity > loadPublicKey > reads generated public key
- packages/core/src/sync-identity.test.ts > sync-identity > loadPublicKey > returns null when file does not exist
- packages/core/src/sync-identity.test.ts > sync-identity > resolveKeyPaths > returns correct paths for custom dir
- packages/core/src/sync-identity.test.ts > sync-identity > resolveKeyPaths > uses default dir when none provided
- packages/core/src/sync-identity.test.ts > sync-identity > validateExistingKeypair > returns false for invalid public key content
- packages/core/src/sync-identity.test.ts > sync-identity > validateExistingKeypair > returns false when files do not exist
- packages/core/src/sync-identity.test.ts > sync-identity > validateExistingKeypair > returns true for valid generated keypair
- packages/core/src/sync-mixed-scope.test.ts > mixed personal/work/OSS sync — boundary enforcement > a broad peer project filter narrows within scope but cannot widen across scopes
- packages/core/src/sync-mixed-scope.test.ts > mixed personal/work/OSS sync — boundary enforcement > a pending policy deny blocks exact outbound ops and snapshots without touching sibling scopes
- packages/core/src/sync-mixed-scope.test.ts > mixed personal/work/OSS sync — boundary enforcement > a pending sender deny rejects inbound ops only for the exact scope and device
- packages/core/src/sync-mixed-scope.test.ts > mixed personal/work/OSS sync — boundary enforcement > advances replication cursors independently per peer and per scope
- packages/core/src/sync-mixed-scope.test.ts > mixed personal/work/OSS sync — boundary enforcement > never routes any scoped op to an unauthorized rogue peer
- packages/core/src/sync-mixed-scope.test.ts > mixed personal/work/OSS sync — boundary enforcement > never routes default-scope local-only ops to any peer
- packages/core/src/sync-mixed-scope.test.ts > mixed personal/work/OSS sync — boundary enforcement > requires an explicit personal-scope grant for same-actor private sync
- packages/core/src/sync-mixed-scope.test.ts > mixed personal/work/OSS sync — boundary enforcement > revoking the work peer halts further work-scope ops without touching other scopes
- packages/core/src/sync-mixed-scope.test.ts > mixed personal/work/OSS sync — boundary enforcement > routes outbound ops to the peer authorized for their scope only
- packages/core/src/sync-mixed-scope.test.ts > mixed personal/work/OSS sync — boundary enforcement > scope-limited snapshot bootstrap returns only memories for the requested scope
- packages/core/src/sync-pass.test.ts > consecutiveConnectivityFailures > counts consecutive connectivity failures
- packages/core/src/sync-pass.test.ts > consecutiveConnectivityFailures > returns 0 when no attempts exist
- packages/core/src/sync-pass.test.ts > consecutiveConnectivityFailures > stops counting at a non-connectivity error
- packages/core/src/sync-pass.test.ts > consecutiveConnectivityFailures > stops counting at a success
- packages/core/src/sync-pass.test.ts > cursorAdvances > returns false when candidate equals current
- packages/core/src/sync-pass.test.ts > cursorAdvances > returns false when candidate has no pipe separator
- packages/core/src/sync-pass.test.ts > cursorAdvances > returns false when candidate is null
- packages/core/src/sync-pass.test.ts > cursorAdvances > returns false when candidate is older than current
- packages/core/src/sync-pass.test.ts > cursorAdvances > returns true when candidate is newer than current
- packages/core/src/sync-pass.test.ts > cursorAdvances > returns true when current is null and candidate is valid
- packages/core/src/sync-pass.test.ts > isConnectivityError > detects connection refused
- packages/core/src/sync-pass.test.ts > isConnectivityError > detects timeout
- packages/core/src/sync-pass.test.ts > isConnectivityError > returns false for auth errors
- packages/core/src/sync-pass.test.ts > isConnectivityError > returns false for null
- packages/core/src/sync-pass.test.ts > peerBackoffSeconds > caps at max backoff (with jitter)
- packages/core/src/sync-pass.test.ts > peerBackoffSeconds > doubles base for 3 failures (with jitter)
- packages/core/src/sync-pass.test.ts > peerBackoffSeconds > returns 0 for 0 or 1 failures
- packages/core/src/sync-pass.test.ts > peerBackoffSeconds > returns base backoff with jitter for 2 failures
- packages/core/src/sync-pass.test.ts > shouldSkipOfflinePeer > returns false when backoff period has elapsed
- packages/core/src/sync-pass.test.ts > shouldSkipOfflinePeer > returns false when fewer than 2 failures
- packages/core/src/sync-pass.test.ts > shouldSkipOfflinePeer > returns true when recent consecutive failures within backoff
- packages/core/src/sync-pass.test.ts > sync capability negotiation > does not upgrade a local scoped peer from an enforcing advertisement
- packages/core/src/sync-pass.test.ts > sync capability negotiation > downgrades aware-to-enforcing sessions to aware
- packages/core/src/sync-pass.test.ts > sync capability negotiation > downgrades scoped-to-aware sessions to aware
- packages/core/src/sync-pass.test.ts > sync capability negotiation > downgrades unsupported-to-aware sessions to unsupported
- packages/core/src/sync-pass.test.ts > sync capability negotiation > normalizes missing or unknown peer capability to unsupported
- packages/core/src/sync-pass.test.ts > syncOnce > bootstraps every authorized scope advertised by a scoped peer
- packages/core/src/sync-pass.test.ts > syncOnce > canary: bulk of source rows reach a fresh peer across multiple Spaces (ruu6.7)
- packages/core/src/sync-pass.test.ts > syncOnce > classifies inbound membership rejections as scope failures
- packages/core/src/sync-pass.test.ts > syncOnce > classifies peer trust failures structurally
- packages/core/src/sync-pass.test.ts > syncOnce > classifies scoped transport failures structurally
- packages/core/src/sync-pass.test.ts > syncOnce > clears a stale per-scope cursor after scoped reset_required
- packages/core/src/sync-pass.test.ts > syncOnce > does not advance the inbound cursor when an op fails to apply
- packages/core/src/sync-pass.test.ts > syncOnce > does not fetch scoped snapshots for local-only advertised scopes
- packages/core/src/sync-pass.test.ts > syncOnce > does not re-bootstrap null-baseline scoped bootstraps that already have a marker
- packages/core/src/sync-pass.test.ts > syncOnce > falls back to immediate vector maintenance when durable queueing throws
- packages/core/src/sync-pass.test.ts > syncOnce > keeps default-scope and per-scope cursors independent after scoped incremental sync
- packages/core/src/sync-pass.test.ts > syncOnce > keeps missing_scope reset_required failures categorized as scope access
- packages/core/src/sync-pass.test.ts > syncOnce > keeps scope_inactive reset_required failures categorized as scope access
- packages/core/src/sync-pass.test.ts > syncOnce > keeps stale_epoch reset_required failures categorized as scope access
- packages/core/src/sync-pass.test.ts > syncOnce > keeps unsupported-scope reset_required failures categorized as protocol drift
- packages/core/src/sync-pass.test.ts > syncOnce > merge-recovers scoped snapshots when local rows exist but the peer scope has no cursor
- packages/core/src/sync-pass.test.ts > syncOnce > promotes the working address after falling back from an unreachable candidate
- packages/core/src/sync-pass.test.ts > syncOnce > queues durable vector catch-up after applying incremental inbound ops
- packages/core/src/sync-pass.test.ts > syncOnce > re-bootstraps marked empty scopes when the peer advertises a baseline
- packages/core/src/sync-pass.test.ts > syncOnce > records local capability diagnostics when device identity fails before status
- packages/core/src/sync-pass.test.ts > syncOnce > rejects pulled ops that spoof the local device without advancing cursor
- packages/core/src/sync-pass.test.ts > syncOnce > rejects spoofed local-device ops from unsupported peers without advancing cursor
- packages/core/src/sync-pass.test.ts > syncOnce > replicates a multi-Space corpus from a scoped peer to a fresh receiver (ruu6.7)
- packages/core/src/sync-pass.test.ts > syncOnce > reports per-scope failure without rolling back default-scope sync
- packages/core/src/sync-pass.test.ts > syncOnce > retains a pending bootstrap grant when status returns the wrong peer identity
- packages/core/src/sync-pass.test.ts > syncOnce > returns error when peer has empty pinned_fingerprint
- packages/core/src/sync-pass.test.ts > syncOnce > returns error when peer is not pinned
- packages/core/src/sync-pass.test.ts > syncOnce > returns error with no dialable addresses
- packages/core/src/sync-pass.test.ts > syncOnce > treats missing peer capability as unsupported while preserving explicitly scoped transport
- packages/core/src/sync-pass.test.ts > syncOnce > uses and clears a pending bootstrap grant after the peer accepts status authentication
- packages/core/src/sync-pass.test.ts > syncOnce auto-bootstrap > bails when shared memories appear during bootstrap fetch
- packages/core/src/sync-pass.test.ts > syncOnce auto-bootstrap > cleans remote-origin local-only rows before initial bootstrap exchange
- packages/core/src/sync-pass.test.ts > syncOnce auto-bootstrap > falls through to incremental when cursor already exists
- packages/core/src/sync-pass.test.ts > syncOnce auto-bootstrap > falls through to incremental when local shared data exists
- packages/core/src/sync-pass.test.ts > syncOnce auto-bootstrap > rejects scoped top-level bootstrap when local membership is missing
- packages/core/src/sync-pass.test.ts > syncOnce auto-bootstrap > triggers bootstrap for empty local state with no cursor
- packages/core/src/sync-pass.test.ts > syncPassPreflight > does not run retention pruning in sync preflight
- packages/core/src/sync-reassign-scope.test.ts > reassign_scope replication > applies either authorized side without needing or exposing the other side payload
- packages/core/src/sync-reassign-scope.test.ts > reassign_scope replication > converges under either side ordering and rejects malformed payloads without mutation
- packages/core/src/sync-reassign-scope.test.ts > reassign_scope replication > keeps visibility filtering on new-side reassignment payloads
- packages/core/src/sync-reassign-scope.test.ts > reassign_scope replication > normalizes a legacy blank scope while applying a local-default reassignment
- packages/core/src/sync-reassign-scope.test.ts > reassign_scope replication > reassigns replicated legacy memories that use their numeric id as the entity id
- packages/core/src/sync-reassign-scope.test.ts > reassign_scope replication > records one logical revision as deterministic old and new scoped sides
- packages/core/src/sync-reassign-scope.test.ts > reassign_scope replication > sends local-default old-side cleanup only to peers allowed for the project
- packages/core/src/sync-reassign-scope.test.ts > reassign_scope replication > withholds reassignment from peers that did not negotiate the additive feature
- packages/core/src/sync-replication.test.ts > applyReplicationOps > accepts an absent-row old-side reassignment as a vacuous control operation
- packages/core/src/sync-replication.test.ts > applyReplicationOps > allows non-personal null-payload deletes when the op row carries an authorized scope
- packages/core/src/sync-replication.test.ts > applyReplicationOps > allows sender-origin old-side reassignment when strict validation is disabled
- packages/core/src/sync-replication.test.ts > applyReplicationOps > allows sender-owned local-default cleanup for a prior recipient outside the destination
- packages/core/src/sync-replication.test.ts > applyReplicationOps > applies a custom scanner's extra rules to inbound peer payloads
- packages/core/src/sync-replication.test.ts > applyReplicationOps > applies scoped inbound ops only when sender and receiver are members
- packages/core/src/sync-replication.test.ts > applyReplicationOps > bounds stale peer diagnostic scans
- packages/core/src/sync-replication.test.ts > applyReplicationOps > counts conflict when existing memory has newer clock
- packages/core/src/sync-replication.test.ts > applyReplicationOps > derives tags for inserted memories arriving with empty tags_text
- packages/core/src/sync-replication.test.ts > applyReplicationOps > diagnoses stale peer-received rows without deleting them
- packages/core/src/sync-replication.test.ts > applyReplicationOps > does not count a delete for an unknown memory as applied
- packages/core/src/sync-replication.test.ts > applyReplicationOps > does not let access cleanup delete local or differently scoped rows
- packages/core/src/sync-replication.test.ts > applyReplicationOps > does not overwrite existing tags with empty incoming tags on update
- packages/core/src/sync-replication.test.ts > applyReplicationOps > does not queue vector re-embed for metadata-only updates
- packages/core/src/sync-replication.test.ts > applyReplicationOps > does not resurrect a tombstone when an upsert omits deleted_at but sets active=1
- packages/core/src/sync-replication.test.ts > applyReplicationOps > inserts a new memory item on upsert
- packages/core/src/sync-replication.test.ts > applyReplicationOps > leaves authorized and ambiguous stale-scope candidates intact with diagnostics
- packages/core/src/sync-replication.test.ts > applyReplicationOps > limits local-only cleanup to rows proven to originate from the syncing peer
- packages/core/src/sync-replication.test.ts > applyReplicationOps > physically deletes matching peer-received rows when access cleanup arrives
- packages/core/src/sync-replication.test.ts > applyReplicationOps > populates ref tables when inserting a new memory with files and concepts
- packages/core/src/sync-replication.test.ts > applyReplicationOps > preserves authoritative inbound scope_id on existing upserts and deletes
- packages/core/src/sync-replication.test.ts > applyReplicationOps > preserves authoritative inbound scope_id on inserted memories and recorded ops
- packages/core/src/sync-replication.test.ts > applyReplicationOps > preserves incoming tags when source provides non-empty tags_text
- packages/core/src/sync-replication.test.ts > applyReplicationOps > preserves project on newly replicated memories
- packages/core/src/sync-replication.test.ts > applyReplicationOps > queues vector re-embed when a metadata-only update reactivates a deleted memory
- packages/core/src/sync-replication.test.ts > applyReplicationOps > reconciles provably stale peer-received rows without deleting receiver-owned rows
- packages/core/src/sync-replication.test.ts > applyReplicationOps > reconciles stale-epoch peer rows as stale retention
- packages/core/src/sync-replication.test.ts > applyReplicationOps > records applied ops in replication_ops table
- packages/core/src/sync-replication.test.ts > applyReplicationOps > records malformed-payload upserts so they are not reprocessed every pass
- packages/core/src/sync-replication.test.ts > applyReplicationOps > redacts peer-controlled actor_display_name and origin_source
- packages/core/src/sync-replication.test.ts > applyReplicationOps > redacts secrets in inbound peer payloads on insert
- packages/core/src/sync-replication.test.ts > applyReplicationOps > redacts secrets in inbound peer payloads on update of existing row
- packages/core/src/sync-replication.test.ts > applyReplicationOps > rejects 'ambiguous'-origin old-side reassignment when strict validation is disabled
- packages/core/src/sync-replication.test.ts > applyReplicationOps > rejects 'foreign'-origin old-side reassignment when strict validation is disabled
- packages/core/src/sync-replication.test.ts > applyReplicationOps > rejects 'local'-origin old-side reassignment when strict validation is disabled
- packages/core/src/sync-replication.test.ts > applyReplicationOps > rejects a replayed duplicate op after revocation instead of idempotently accepting it
- packages/core/src/sync-replication.test.ts > applyReplicationOps > rejects inbound ops before mutation when 'local-default inbound scope'
- packages/core/src/sync-replication.test.ts > applyReplicationOps > rejects inbound ops before mutation when 'missing op-row scope_id'
- packages/core/src/sync-replication.test.ts > applyReplicationOps > rejects inbound ops before mutation when 'no cached membership manifest'
- packages/core/src/sync-replication.test.ts > applyReplicationOps > rejects inbound ops before mutation when 'payload scope contradicts op-row scope'
- packages/core/src/sync-replication.test.ts > applyReplicationOps > rejects inbound ops before mutation when 'receiver is not a scope member'
- packages/core/src/sync-replication.test.ts > applyReplicationOps > rejects inbound ops before mutation when 'revoked or stale sender membership'
- packages/core/src/sync-replication.test.ts > applyReplicationOps > rejects inbound ops before mutation when 'sender is not a scope member'
- packages/core/src/sync-replication.test.ts > applyReplicationOps > rejects inbound ops before mutation when 'sender spoofs the receiver device id'
- packages/core/src/sync-replication.test.ts > applyReplicationOps > rejects local-default reassignment for a receiver-owned memory
- packages/core/src/sync-replication.test.ts > applyReplicationOps > rejects local-default reassignment for an unknown-origin memory
- packages/core/src/sync-replication.test.ts > applyReplicationOps > rejects local-default reassignment when the sender lacks destination access
- packages/core/src/sync-replication.test.ts > applyReplicationOps > rejects regular inbound ops with 'local-default scope' when legacy scope validation is disabled
- packages/core/src/sync-replication.test.ts > applyReplicationOps > rejects regular inbound ops with 'null scope' when legacy scope validation is disabled
- packages/core/src/sync-replication.test.ts > applyReplicationOps > removes remote-origin local-only rows conservatively and idempotently
- packages/core/src/sync-replication.test.ts > applyReplicationOps > repopulates ref tables when updating existing memory with new files/concepts
- packages/core/src/sync-replication.test.ts > applyReplicationOps > retains pending membership peer rows as ambiguous
- packages/core/src/sync-replication.test.ts > applyReplicationOps > skips duplicate op_ids (idempotent)
- packages/core/src/sync-replication.test.ts > applyReplicationOps > skips ops from the local device
- packages/core/src/sync-replication.test.ts > applyReplicationOps > soft-deletes on delete op_type
- packages/core/src/sync-replication.test.ts > applyReplicationOps > updates existing memory when op clock is newer
- packages/core/src/sync-replication.test.ts > applyReplicationOps > uses authoritative inbound op scope_id when deleting existing memories
- packages/core/src/sync-replication.test.ts > applyReplicationOps > uses authoritative inbound op scope_id when updating existing memories
- packages/core/src/sync-replication.test.ts > applyReplicationOps > uses inbound delete clock device metadata for later Lamport tie-breaks
- packages/core/src/sync-replication.test.ts > bootstrap snapshot round-trip parity > clears scoped null-baseline bootstrap markers when a scoped reset is required
- packages/core/src/sync-replication.test.ts > bootstrap snapshot round-trip parity > clears stale scoped cursors before writing a null-baseline bootstrap marker
- packages/core/src/sync-replication.test.ts > bootstrap snapshot round-trip parity > records a scoped cursor marker for empty scoped bootstrap snapshots
- packages/core/src/sync-replication.test.ts > bootstrap snapshot round-trip parity > round-trips all fields through snapshot page → applyBootstrapSnapshot
- packages/core/src/sync-replication.test.ts > chunkOpsBySize > returns a single batch when all ops fit
- packages/core/src/sync-replication.test.ts > chunkOpsBySize > returns empty array for empty input
- packages/core/src/sync-replication.test.ts > chunkOpsBySize > splits into multiple batches when ops exceed limit
- packages/core/src/sync-replication.test.ts > chunkOpsBySize > throws when a single op exceeds the limit
- packages/core/src/sync-replication.test.ts > clockTuple > builds a 3-element tuple
- packages/core/src/sync-replication.test.ts > extractReplicationOps > extracts ops from a valid payload
- packages/core/src/sync-replication.test.ts > extractReplicationOps > returns empty array for non-object payload
- packages/core/src/sync-replication.test.ts > extractReplicationOps > returns empty array when ops is missing
- packages/core/src/sync-replication.test.ts > extractReplicationOps > returns empty array when ops is not an array
- packages/core/src/sync-replication.test.ts > filterReplicationOpsForSyncWithStatus > applies project filters to null-payload deletes when local context exists
- packages/core/src/sync-replication.test.ts > filterReplicationOpsForSyncWithStatus > applies scope membership before broad project filters
- packages/core/src/sync-replication.test.ts > filterReplicationOpsForSyncWithStatus > blocks null and local-default regular ops before legacy project filtering
- packages/core/src/sync-replication.test.ts > filterReplicationOpsForSyncWithStatus > does not let personal grants make private payloads leave through org scopes
- packages/core/src/sync-replication.test.ts > filterReplicationOpsForSyncWithStatus > does not treat ordinary shared rows with actor_id as personal-scope rows
- packages/core/src/sync-replication.test.ts > filterReplicationOpsForSyncWithStatus > fails closed for personal workspace markers without a derivable personal scope
- packages/core/src/sync-replication.test.ts > filterReplicationOpsForSyncWithStatus > filters by peer include scope and advances cursor past skipped ops
- packages/core/src/sync-replication.test.ts > filterReplicationOpsForSyncWithStatus > keeps delete tombstones with null payload_json
- packages/core/src/sync-replication.test.ts > filterReplicationOpsForSyncWithStatus > keeps targeted cleanup and valid old-side reassignment as control exceptions
- packages/core/src/sync-replication.test.ts > filterReplicationOpsForSyncWithStatus > never sends local-authority scopes outbound
- packages/core/src/sync-replication.test.ts > filterReplicationOpsForSyncWithStatus > requires a matching personal scope grant instead of a claimed local actor flag
- packages/core/src/sync-replication.test.ts > filterReplicationOpsForSyncWithStatus > requires a personal scope grant when visibility is missing but scope is personal
- packages/core/src/sync-replication.test.ts > filterReplicationOpsForSyncWithStatus > requires the local device to remain a scope member before outbound sync
- packages/core/src/sync-replication.test.ts > filterReplicationOpsForSyncWithStatus > respects CODEMEM_SYNC_PROJECTS_* env overrides
- packages/core/src/sync-replication.test.ts > hasUnsyncedSharedMemoryChanges > ignores private-only rows when checking dirty shared-memory state
- packages/core/src/sync-replication.test.ts > hasUnsyncedSharedMemoryChanges > reports missing shared-memory replication ops as dirty
- packages/core/src/sync-replication.test.ts > hasUnsyncedSharedMemoryChanges > treats rows with matching replication ops as clean
- packages/core/src/sync-replication.test.ts > inbound scope rejection diagnostics > clamps the list limit to a sensible maximum
- packages/core/src/sync-replication.test.ts > inbound scope rejection diagnostics > filters rejections by peer and time window
- packages/core/src/sync-replication.test.ts > inbound scope rejection diagnostics > groups rejection counts by peer and reason
- packages/core/src/sync-replication.test.ts > inbound scope rejection diagnostics > lists rejection records newest-first without exposing payloads
- packages/core/src/sync-replication.test.ts > inbound scope rejection diagnostics > returns an empty summary when the rejection log table does not exist
- packages/core/src/sync-replication.test.ts > isNewerClock > higher rev wins
- packages/core/src/sync-replication.test.ts > isNewerClock > identical clocks are not newer
- packages/core/src/sync-replication.test.ts > isNewerClock > same rev and updated_at — tiebreaks on device_id
- packages/core/src/sync-replication.test.ts > isNewerClock > same rev — tiebreaks on updated_at
- packages/core/src/sync-replication.test.ts > legacy key migration + replication backfill > backfills missing delete/upsert ops once and remains idempotent
- packages/core/src/sync-replication.test.ts > legacy key migration + replication backfill > does not mint legacy:local import keys before device identity exists
- packages/core/src/sync-replication.test.ts > legacy key migration + replication backfill > rewrites old-format legacy import keys to device-scoped keys
- packages/core/src/sync-replication.test.ts > legacy key migration + replication backfill > stamps missing memory scope before backfilling replication ops
- packages/core/src/sync-replication.test.ts > loadMemorySnapshotPageForPeer > does not report hasMore when only skipped rows remain
- packages/core/src/sync-replication.test.ts > loadMemorySnapshotPageForPeer > does not return has_more without a next page token when only skipped rows remain
- packages/core/src/sync-replication.test.ts > loadMemorySnapshotPageForPeer > fails closed for scoped snapshot rows with personal workspace but no actor
- packages/core/src/sync-replication.test.ts > loadMemorySnapshotPageForPeer > filters out private memories from snapshot pages
- packages/core/src/sync-replication.test.ts > loadMemorySnapshotPageForPeer > filters snapshot pages by the memory session project
- packages/core/src/sync-replication.test.ts > loadMemorySnapshotPageForPeer > omitted scope excludes null and local-default snapshot rows
- packages/core/src/sync-replication.test.ts > loadMemorySnapshotPageForPeer > rejects boundary mismatches for snapshot pages
- packages/core/src/sync-replication.test.ts > loadMemorySnapshotPageForPeer > requires a matching personal scope grant for claimed local actor snapshot rows
- packages/core/src/sync-replication.test.ts > loadMemorySnapshotPageForPeer > requires a personal scope grant for snapshot rows with personal scope but missing visibility
- packages/core/src/sync-replication.test.ts > loadMemorySnapshotPageForPeer > returns deterministic memory snapshot pages with tombstones included
- packages/core/src/sync-replication.test.ts > loadMemorySnapshotPageForPeer > uses the memory session project when applying project filters to snapshot pages
- packages/core/src/sync-replication.test.ts > loadReplicationOpsForPeer > omitted scope advances past local-only ops without serving them
- packages/core/src/sync-replication.test.ts > loadReplicationOpsForPeer > requires an explicit reset boundary on incremental requests
- packages/core/src/sync-replication.test.ts > loadReplicationOpsForPeer > returns incremental ops when the request matches the current boundary
- packages/core/src/sync-replication.test.ts > loadReplicationOpsForPeer > returns reset_required when only part of the boundary tuple is provided
- packages/core/src/sync-replication.test.ts > loadReplicationOpsForPeer > returns reset_required when snapshot metadata is omitted for the current generation
- packages/core/src/sync-replication.test.ts > loadReplicationOpsForPeer > returns reset_required when the cursor is older than the retained floor
- packages/core/src/sync-replication.test.ts > loadReplicationOpsForPeer > returns reset_required when the requested generation mismatches
- packages/core/src/sync-replication.test.ts > loadReplicationOpsForPeer > scoped peer op windows only emit ops authored by the serving device
- packages/core/src/sync-replication.test.ts > loadReplicationOpsForPeer > uses scoped reset boundaries and op windows independently
- packages/core/src/sync-replication.test.ts > loadReplicationOpsSince > filters by deviceId
- packages/core/src/sync-replication.test.ts > loadReplicationOpsSince > respects limit
- packages/core/src/sync-replication.test.ts > loadReplicationOpsSince > returns [[], null] when no ops match
- packages/core/src/sync-replication.test.ts > loadReplicationOpsSince > returns all ops when cursor is null
- packages/core/src/sync-replication.test.ts > loadReplicationOpsSince > returns ops after cursor
- packages/core/src/sync-replication.test.ts > pruneReplicationOps > accepts a cursor equal to the retained floor as still replayable
- packages/core/src/sync-replication.test.ts > pruneReplicationOps > bulk deletes an oldest-first age cutoff chunk and updates retained floor to its boundary cursor
- packages/core/src/sync-replication.test.ts > pruneReplicationOps > bulk deletes oldest-prefix chunks during size trimming and remeasures between chunks
- packages/core/src/sync-replication.test.ts > pruneReplicationOps > bulkPruneReplicationOpsByAgeCutoff deletes the full oldest prefix before the cutoff
- packages/core/src/sync-replication.test.ts > pruneReplicationOps > bulkPruneReplicationOpsByAgeCutoff respects the requested maxDeleteOps chunk size
- packages/core/src/sync-replication.test.ts > pruneReplicationOps > does not advance retained floor when nothing is pruned
- packages/core/src/sync-replication.test.ts > pruneReplicationOps > does not overshoot the remaining delete budget during bulk age pruning
- packages/core/src/sync-replication.test.ts > pruneReplicationOps > does not overshoot the remaining delete budget during size trimming
- packages/core/src/sync-replication.test.ts > pruneReplicationOps > does not size-prune a scope because another scope is large
- packages/core/src/sync-replication.test.ts > pruneReplicationOps > keeps applying prune passes until retention is no longer budget-limited
- packages/core/src/sync-replication.test.ts > pruneReplicationOps > plans age-pass candidate ops, bytes, cutoff cursor, and estimated batches
- packages/core/src/sync-replication.test.ts > pruneReplicationOps > prunes oldest ops beyond size budget and updates retained floor
- packages/core/src/sync-replication.test.ts > pruneReplicationOps > prunes one scope without deleting ops or advancing floors in another scope
- packages/core/src/sync-replication.test.ts > pruneReplicationOps > respects maxDeleteOps budget during pruning
- packages/core/src/sync-replication.test.ts > pruneReplicationOps > stops multi-pass catch-up after the configured pass cap
- packages/core/src/sync-replication.test.ts > recordReplicationOp > falls back to memoryId as entity_id when import_key is null
- packages/core/src/sync-replication.test.ts > recordReplicationOp > includes full memory payload in upsert ops and round-trips all columns
- packages/core/src/sync-replication.test.ts > recordReplicationOp > inserts an op and returns a UUID op_id
- packages/core/src/sync-replication.test.ts > recordReplicationOp > records access cleanup ops on the default sync path
- packages/core/src/sync-replication.test.ts > recordReplicationOp > stamps a missing memory scope before recording the replication op
- packages/core/src/sync-replication.test.ts > recordReplicationOp > stores null payload for delete ops
- packages/core/src/sync-replication.test.ts > replication cursors > keeps cursor values independent per scope
- packages/core/src/sync-replication.test.ts > replication cursors > overwrites cursor on subsequent set
- packages/core/src/sync-replication.test.ts > replication cursors > returns [null, null] for unknown peer
- packages/core/src/sync-replication.test.ts > replication cursors > round-trips cursor values after set
- packages/core/src/sync-replication.test.ts > replication cursors > seeds legacy default cursor before a partial v2 update
- packages/core/src/sync-replication.test.ts > replication cursors > updates only the specified cursor field via COALESCE
- packages/core/src/sync-replication.test.ts > replication payload round-trip parity > round-trips all payload fields through record → parse → apply
- packages/core/src/sync-replication.test.ts > sync reset state > creates and persists a default reset boundary
- packages/core/src/sync-replication.test.ts > sync reset state > keeps reset boundaries independent per scope
- packages/core/src/sync-replication.test.ts > sync reset state > updates retained floor and baseline metadata
- packages/core/src/sync-retention-runner.test.ts > listRetentionScopeIds > enumerates scopes from replication_ops when the additive v2 state tables are absent
- packages/core/src/sync-retention-runner.test.ts > listRetentionScopeIds > tolerates a legacy replication_ops table that predates the scope_id column
- packages/core/src/sync-retention-runner.test.ts > runSyncRetentionPass > persists aggregated multi-pass pruning results
- packages/core/src/sync-retention-runner.test.ts > runSyncRetentionPass > persists scoped retention state without pruning other scopes
- packages/core/src/sync-retention-runner.test.ts > runSyncRetentionPass > prunes every known scope when no scope is specified
- packages/core/src/sync-scope-protocol.test.ts > listAuthorizedScopesForPeer > does not advertise a scope denied for the local device
- packages/core/src/sync-scope-protocol.test.ts > listAuthorizedScopesForPeer > does not advertise an exact denied scope and leaves sibling scopes available
- packages/core/src/sync-scope-protocol.test.ts > listAuthorizedScopesForPeer > does not treat peer-wide scoped attempts as per-Space evidence
- packages/core/src/sync-scope-protocol.test.ts > listAuthorizedScopesForPeer > excludes scopes where the peer is not a member
- packages/core/src/sync-scope-protocol.test.ts > listAuthorizedScopesForPeer > excludes scopes where the peer membership is revoked
- packages/core/src/sync-scope-protocol.test.ts > listAuthorizedScopesForPeer > excludes the legacy local-default scope
- packages/core/src/sync-scope-protocol.test.ts > listAuthorizedScopesForPeer > keeps empty Spaces pending after failed or non-scoped sync attempts
- packages/core/src/sync-scope-protocol.test.ts > listAuthorizedScopesForPeer > keeps newly granted Spaces pending until scoped sync records a cursor marker
- packages/core/src/sync-scope-protocol.test.ts > listAuthorizedScopesForPeer > keeps unadvertised Spaces pending even when newer irrelevant attempts exist
- packages/core/src/sync-scope-protocol.test.ts > listAuthorizedScopesForPeer > marks empty authorized Spaces current after scoped bootstrap records a cursor marker
- packages/core/src/sync-scope-protocol.test.ts > listAuthorizedScopesForPeer > returns an empty list when local and peer device ids are equal
- packages/core/src/sync-scope-protocol.test.ts > listAuthorizedScopesForPeer > returns an empty list when the local device has no memberships
- packages/core/src/sync-scope-protocol.test.ts > listAuthorizedScopesForPeer > returns scopes both devices are active members of, sorted by scope_id
- packages/core/src/sync-scope-protocol.test.ts > parseSyncScopeRequest scoped path > accepts a scoped request when peer is an authorized active member
- packages/core/src/sync-scope-protocol.test.ts > parseSyncScopeRequest scoped path > falls back to unsupported_scope when caller advertises a lower capability
- packages/core/src/sync-scope-protocol.test.ts > parseSyncScopeRequest scoped path > rejects a scoped request while an exact peer deny overlay is pending
- packages/core/src/sync-scope-protocol.test.ts > parseSyncScopeRequest scoped path > rejects with missing_scope when local device is not a member
- packages/core/src/sync-scope-protocol.test.ts > parseSyncScopeRequest scoped path > rejects with missing_scope when peer is not a member
- packages/core/src/sync-scope-protocol.test.ts > parseSyncScopeRequest scoped path > rejects with missing_scope when the scope does not exist
- packages/core/src/sync-scope-protocol.test.ts > parseSyncScopeRequest scoped path > rejects with scope_inactive when membership was revoked
- packages/core/src/sync-scope-protocol.test.ts > parseSyncScopeRequest scoped path > rejects with stale_epoch when peer membership epoch is behind authority
- packages/core/src/sync-scope-protocol.test.ts > sync scope protocol compatibility > adds legacy scope shape to reset boundaries
- packages/core/src/sync-scope-protocol.test.ts > sync scope protocol compatibility > builds reset_required payloads for scope protocol errors
- packages/core/src/sync-scope-protocol.test.ts > sync scope protocol compatibility > echoes the requested scope_id on reset_required when provided
- packages/core/src/sync-scope-protocol.test.ts > sync scope protocol compatibility > returns missing_scope when scope_id is present but empty
- packages/core/src/sync-scope-protocol.test.ts > sync scope protocol compatibility > returns unsupported_scope for explicit scoped requests without scoped capability
- packages/core/src/sync-scope-protocol.test.ts > sync scope protocol compatibility > treats omitted scope_id as legacy compatibility mode
- packages/core/src/vector-migration.test.ts > vector migration > completes an in-flight running job without re-embedding when corpus is already covered
- packages/core/src/vector-migration.test.ts > vector migration > marks a queued bootstrap backfill as failed when the embedding client is unavailable
- packages/core/src/vector-migration.test.ts > vector migration > preserves newly queued incremental ids while replaying queued work
- packages/core/src/vector-migration.test.ts > vector migration > prunes stale current-model vectors while replaying queued incremental upserts
- packages/core/src/vector-migration.test.ts > vector migration > removes stale old-model rows when queued work has zero embeddable memories
- packages/core/src/vector-migration.test.ts > vector migration > resumes bootstrap-queued vector catch-up after restart
- packages/core/src/vector-migration.test.ts > vector migration > resumes incremental sync queued vector catch-up after restart
- packages/core/src/vectors.test.ts > vectors > deletes vector rows for replicated tombstones
- packages/core/src/vectors.test.ts > vectors > refreshes same-model vectors for replicated content updates
- packages/core/src/vectors.test.ts > vectors > runs best-effort sync fallback vector maintenance without throwing on embedding failures

### R-MCP (97)

- packages/mcp-server/src/audit.test.ts > OAuth audit events > builds events with ISO timestamps and metadata
- packages/mcp-server/src/audit.test.ts > OAuth audit events > default emitter writes one JSON line per event to the supplied stream
- packages/mcp-server/src/audit.test.ts > OAuth audit events > env resolver returns a default-shaped emitter when unset or unknown
- packages/mcp-server/src/audit.test.ts > OAuth audit events > env resolver returns a silent emitter for falsy values
- packages/mcp-server/src/audit.test.ts > OAuth audit events > refuses to attach secret-bearing fields in any casing or separator style
- packages/mcp-server/src/audit.test.ts > OAuth audit events > silent emitter writes nothing
- packages/mcp-server/src/audit.test.ts > OAuth audit events > wraps emitters so emitter failures never throw into the caller
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > accepts loopback Host headers without an explicit port when bound to HTTP default (PR 1120 P2 regression)
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > accepts proxied OAuth token requests without rate-limit trust-proxy warnings
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > accepts valid MCP JSON bodies larger than Express's default limit
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > allows only loopback Host and Origin headers for the selected port
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > allows trusted hosted connector browser preflight requests on public OAuth and MCP routes
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > allows valid public bearer requests with the configured external Host
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > canonicalizes stateless request object ordering within the retry window
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > closes idempotently
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > deduplicates authenticated retries by principal, content, and JSON-RPC ID
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > defaults and validates port values
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > defaults to loopback host and validates host values
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > derives default OAuth metadata from the bound HTTP host
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > emits redacted audit events for the full OAuth + bearer flow
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > exposes only POST /mcp
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > fails closed at authorize when OIDC is not configured
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > handles repeated MCP initialize requests over POST
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > keeps Dynamic Client Registration clients across HTTP server restarts
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > keeps stateless retries together across an aligned boundary until first-observation expiry
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > logs early public Host and Origin guard denials
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > parses the explicit unsafe public bind switch
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > partitions retry identity by authenticated client without leaking request context
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > partitions retry identity for distinct tokens issued to the same authenticated client
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > records matching anonymous HTTP calls as separate retrieval attempts
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > registers OAuth clients through Dynamic Client Registration
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > rejects bare public Host headers when the configured public URL uses a non-default port
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > rejects browser requests from non-loopback origins
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > rejects expired and revoked bearer tokens
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > rejects hostile browser origins on OAuth endpoints when a public URL is configured
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > rejects local Dynamic Client Registration from non-loopback origins
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > rejects public OAuth requests with non-public Host headers
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > rejects requests with non-loopback Host headers
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > rejects unsupported OAuth redirect URIs
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > requires valid bearer tokens for public MCP requests
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > returns JSON-RPC errors for malformed and oversized MCP JSON bodies
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > separates authenticated retrieval principals by OAuth client
- packages/mcp-server/src/http.test.ts > MCP HTTP transport > serves OAuth authorization server and protected resource metadata
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > accepts ChatGPT hosted connector redirects
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > accepts native loopback callback redirects
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > allows refresh-token scope downscoping without expansion
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > binds exchanged access tokens to the authorized resource
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > builds authorization server metadata from the MCP public URL
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > builds protected resource metadata for /mcp
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > cleans every refresh-token hash for expired multi-rotated grants
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > cleans every refresh-token hash for multi-rotated grants
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > cleans refresh-token hash indexes when grants are revoked
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > consumes the authorization code on permanent grant failures
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > does not evict unexpired authorization codes when the in-memory cap is reached
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > issues authorization codes and exchanges them with PKCE S256
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > normalizes and validates MCP public URLs
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > preserves the authorization code when token issuance is temporarily unavailable
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > registers public clients and stores them by client id
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > rejects expired and reused authorization codes
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > rejects invalid authorize and token PKCE requests
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > rejects unsupported redirect URIs and confidential-client registration
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > revokes tokens issued during an authorization-code consume race
- packages/mcp-server/src/oauth.test.ts > MCP OAuth metadata and dynamic client registration > stores only hashed access-token material and rejects expired or revoked tokens
- packages/mcp-server/src/oidc.test.ts > OIDC-backed MCP OAuth authorization > consumes pending state when upstream OIDC returns an error callback
- packages/mcp-server/src/oidc.test.ts > OIDC-backed MCP OAuth authorization > denies identities outside the configured allowlist
- packages/mcp-server/src/oidc.test.ts > OIDC-backed MCP OAuth authorization > preserves path-based issuer URLs during discovery
- packages/mcp-server/src/oidc.test.ts > OIDC-backed MCP OAuth authorization > redirects to upstream OIDC and accepts an allowed identity
- packages/mcp-server/src/oidc.test.ts > OIDC-backed MCP OAuth authorization > rejects ID tokens issued too far in the future
- packages/mcp-server/src/oidc.test.ts > OIDC-backed MCP OAuth authorization > rejects ID tokens missing a subject
- packages/mcp-server/src/oidc.test.ts > OIDC-backed MCP OAuth authorization > rejects ID tokens missing issued-at time
- packages/mcp-server/src/oidc.test.ts > OIDC-backed MCP OAuth authorization > rejects ID tokens with a mismatched authorized party
- packages/mcp-server/src/oidc.test.ts > OIDC-backed MCP OAuth authorization > rejects discovered endpoints containing credentials
- packages/mcp-server/src/oidc.test.ts > OIDC-backed MCP OAuth authorization > rejects discovered loopback HTTP endpoints for HTTPS issuers
- packages/mcp-server/src/oidc.test.ts > OIDC-backed MCP OAuth authorization > rejects multi-audience ID tokens without a matching authorized party
- packages/mcp-server/src/oidc.test.ts > OIDC-backed MCP OAuth authorization > rejects non-2xx token responses even when they include an ID token
- packages/mcp-server/src/oidc.test.ts > OIDC-backed MCP OAuth authorization > rejects non-HTTPS discovered endpoints outside loopback
- packages/mcp-server/src/oidc.test.ts > OIDC-backed MCP OAuth authorization > resolves explicit OIDC configuration and requires an allowlist
- packages/mcp-server/src/project-scope.test.ts > project-scope helpers > passes through advanced search knobs
- packages/mcp-server/src/project-scope.test.ts > project-scope helpers > passes through scope filters while preserving default project narrowing
- packages/mcp-server/src/provider.test.ts > MemoryOAuthServerProvider > accepts one retry with the previous refresh token and then treats older tokens as replay
- packages/mcp-server/src/provider.test.ts > MemoryOAuthServerProvider > binds authorization-code exchanges and verified tokens to the requested resource
- packages/mcp-server/src/provider.test.ts > MemoryOAuthServerProvider > exchanges an authorization code for bearer tokens and consumes the code
- packages/mcp-server/src/provider.test.ts > MemoryOAuthServerProvider > exchanges refresh tokens with dual-token rotation and preserves resource binding
- packages/mcp-server/src/provider.test.ts > MemoryOAuthServerProvider > exposes the PKCE challenge for an active authorization code
- packages/mcp-server/src/provider.test.ts > MemoryOAuthServerProvider > leaves an authorization code reusable when token issuance is temporarily unavailable
- packages/mcp-server/src/provider.test.ts > MemoryOAuthServerProvider > normalizes Claude _access aliases before issuing refresh grants
- packages/mcp-server/src/provider.test.ts > MemoryOAuthServerProvider > persists registered clients and refresh grants across MCP process restarts
- packages/mcp-server/src/provider.test.ts > MemoryOAuthServerProvider > redirects authorize requests through upstream OIDC and preserves MCP OAuth params
- packages/mcp-server/src/provider.test.ts > MemoryOAuthServerProvider > rejects access tokens whose bound audience does not match this resource server
- packages/mcp-server/src/provider.test.ts > MemoryOAuthServerProvider > rejects authorization-code client and redirect mismatches
- packages/mcp-server/src/provider.test.ts > MemoryOAuthServerProvider > rejects refresh-token client, scope, and resource mismatches as invalid grants
- packages/mcp-server/src/provider.test.ts > MemoryOAuthServerProvider > rejects token redemption that omits redirect_uri
- packages/mcp-server/src/provider.test.ts > MemoryOAuthServerProvider > revokes a token issued during a lost authorization-code consume race
- packages/mcp-server/src/provider.test.ts > MemoryOAuthServerProvider > revokes refresh-token grants and cascades issued access tokens
- packages/mcp-server/src/provider.test.ts > MemoryOAuthServerProvider > stores refresh-token HMACs instead of plaintext token values
- packages/mcp-server/src/provider.test.ts > MemoryOAuthServerProvider > treats Claude _access scopes as no-op aliases during refresh
- packages/mcp-server/src/provider.test.ts > MemoryOAuthServerProvider > verifies and revokes access tokens with SDK AuthInfo semantics

### R-UI (385)

- packages/ui/src/app-devices.test.tsx > Devices app integration > does not steal focus from outside Devices during polling
- packages/ui/src/app-devices.test.tsx > Devices app integration > moves focus to the Devices tab when a focused device action is removed
- packages/ui/src/app-devices.test.tsx > Devices app integration > preserves stale cards, announces post-load failures, and marks refresh aggregation failed
- packages/ui/src/app-devices.test.tsx > Devices app integration > refreshes read-only inputs, routes actions canonically, and preserves polling focus
- packages/ui/src/app-sharing.test.tsx > Sharing app data refresh > replaces stale actions after a refresh failure and restores them after recovery
- packages/ui/src/components/primitives/primitives.test.tsx > RadixRadioGroup > lets users select an option by clicking the visible label row
- packages/ui/src/lib/api/sync.test.ts > recipient invitation API > ignores blank invite-import detail and falls back to the error code
- packages/ui/src/lib/api/sync.test.ts > recipient invitation API > omits unavailable optional identity names from Project invitation imports
- packages/ui/src/lib/api/sync.test.ts > recipient invitation API > prefers actionable invite-import detail over an opaque error code
- packages/ui/src/lib/api/sync.test.ts > recipient invitation API > preserves safe recipient invitation error codes for contextual UI guidance
- packages/ui/src/lib/api/sync.test.ts > recipient invitation API > sends exact Team preview/create and add-device inspect payloads
- packages/ui/src/lib/api/sync.test.ts > recipient invitation API > sends only the target Identity and reviewed digest for add-device creation
- packages/ui/src/lib/api/sync.test.ts > recipient policy edge API > loads intent and sends exact preview and commit payloads
- packages/ui/src/lib/api/sync.test.ts > recipient policy edge API > loads the typed safe reconciliation status
- packages/ui/src/lib/api/sync.test.ts > recipient policy edge API > returns structured conflict results from edge commit 409 responses
- packages/ui/src/lib/api/sync.test.ts > recipient policy edge API > throws a typed stale error for a stale edge commit
- packages/ui/src/lib/api/sync.test.ts > recipient policy review API > keeps legacy review items visible when continuity is absent
- packages/ui/src/lib/api/sync.test.ts > recipient policy review API > loads the camelCase review DTO and submits an input-free decision unchanged
- packages/ui/src/lib/api/sync.test.ts > recipient policy review API > returns per-item results from a 207 bulk response
- packages/ui/src/lib/api/sync.test.ts > recipient policy review API > throws a typed stale error for a stale 409 result
- packages/ui/src/lib/api/sync.test.ts > share operation API > loads the typed lifecycle list and advances through the single recovery endpoint
- packages/ui/src/lib/api/sync.test.ts > triggerSync > can scope a manual sync by peer device id when addresses are hidden
- packages/ui/src/lib/state.test.ts > Advanced access and Team admin gating > keeps Advanced reachable when the Team admin secret is missing
- packages/ui/src/lib/state.test.ts > Advanced access and Team admin gating > keeps Advanced visible when the Team admin secret is configured
- packages/ui/src/lib/state.test.ts > Advanced access and Team admin gating > keeps Advanced visible while admin status is still unknown
- packages/ui/src/lib/state.test.ts > Advanced access and Team admin gating > keeps canonical tabs accessible and uses feed as the fallback
- packages/ui/src/lib/state.test.ts > Viewer tab routing > maps compatibility route #advanced/sync/diagnostics into Advanced sync content
- packages/ui/src/lib/state.test.ts > Viewer tab routing > maps compatibility route #advanced/teams into Advanced teams content
- packages/ui/src/lib/state.test.ts > Viewer tab routing > maps compatibility route #coordinator-admin into Advanced teams content
- packages/ui/src/lib/state.test.ts > Viewer tab routing > maps compatibility route #sync into Advanced sync content
- packages/ui/src/lib/state.test.ts > Viewer tab routing > maps compatibility route #sync/diagnostics into Advanced sync content
- packages/ui/src/lib/state.test.ts > Viewer tab routing > migrates saved coordinator-admin state into Advanced teams
- packages/ui/src/lib/state.test.ts > Viewer tab routing > migrates saved sync state into Advanced sync
- packages/ui/src/lib/state.test.ts > Viewer tab routing > orders canonical tabs around the promoted project workflow
- packages/ui/src/lib/state.test.ts > Viewer tab routing > preserves a saved legacy Team destination while canonicalizing its hash
- packages/ui/src/lib/state.test.ts > Viewer tab routing > recognizes #advanced as a canonical route
- packages/ui/src/lib/state.test.ts > Viewer tab routing > recognizes #devices as a canonical route
- packages/ui/src/lib/state.test.ts > Viewer tab routing > recognizes #projects as a canonical route
- packages/ui/src/lib/state.test.ts > Viewer tab routing > recognizes #sharing as a canonical route
- packages/ui/src/lib/state.test.ts > Viewer tab routing > writes canonical Advanced section hashes
- packages/ui/src/project-first-layout.test.ts > project-first navigation layout > adds recipient-focused Sharing and a Devices mount before Advanced
- packages/ui/src/project-first-layout.test.ts > project-first navigation layout > includes the Projects share-flow mount used by row-level Share actions
- packages/ui/src/project-first-layout.test.ts > project-first navigation layout > keeps legacy and backend terminology out of primary navigation controls
- packages/ui/src/project-first-layout.test.ts > project-first navigation layout > keeps legacy device controls available but outside the primary project-sharing flow
- packages/ui/src/project-first-layout.test.ts > project-first navigation layout > keeps normal Projects controls recipient-focused and moves invitations to Advanced
- packages/ui/src/project-first-layout.test.ts > project-first navigation layout > marks only the initial Feed control with aria-current
- packages/ui/src/project-first-layout.test.ts > project-first navigation layout > orders the visible navigation as Feed, Projects, Sharing, Devices, Health, Advanced
- packages/ui/src/project-first-layout.test.ts > project-first navigation layout > preserves Health sync refresh and the legacy upgrade review destinations
- packages/ui/src/project-first-layout.test.ts > project-first navigation layout > reuses Sync and Team administration DOM inside the Advanced panel
- packages/ui/src/project-first-layout.test.ts > project-first navigation layout > wires Devices to its read-only data sources without introducing mutation endpoints
- packages/ui/src/tabs/coordinator-admin/components/devices-panel.test.tsx > DevicesPanel > keeps the device title on the saved display name while editing a rename draft
- packages/ui/src/tabs/coordinator-admin/components/invites-panel.test.tsx > Teams invite panel > labels coordinator invites as legacy and reflects project sharing read-only
- packages/ui/src/tabs/coordinator-admin/components/join-requests-panel.test.tsx > JoinRequestsPanel > shows friendly names with device identity as secondary diagnostics
- packages/ui/src/tabs/coordinator-admin/data/actions.test.ts > coordinator admin actions > keeps default Space grant warnings visible after approving a join request
- packages/ui/src/tabs/coordinator-admin/data/actions.test.ts > coordinator admin actions > keeps setup warnings visible when default Space creation needs repair
- packages/ui/src/tabs/coordinator-admin/data/actions.test.ts > coordinator admin actions > opens the guided setup callout after creating a Team with a default Space
- packages/ui/src/tabs/coordinator-admin/data/actions.test.ts > coordinator admin actions > points successful invite sharing copy at Teams
- packages/ui/src/tabs/coordinator-admin/data/actions.test.ts > coordinator admin actions > selects the created Team after refreshing stale group data
- packages/ui/src/tabs/coordinator-admin/data/device-card.test.ts > coordinator admin device card copy > demotes raw device and Team ids to advanced copy
- packages/ui/src/tabs/coordinator-admin/data/device-card.test.ts > coordinator admin device card copy > uses the active Team as a fallback for older device payloads
- packages/ui/src/tabs/coordinator-admin/data/scope-management.test.ts > coordinator admin scope management view helpers > falls back to cached enrolled devices for the selected group
- packages/ui/src/tabs/coordinator-admin/data/scope-management.test.ts > coordinator admin scope management view helpers > formats empty or underscored scope statuses for operator copy
- packages/ui/src/tabs/coordinator-admin/data/scope-management.test.ts > coordinator admin scope management view helpers > formats missing Space membership epochs as unknown in advanced copy
- packages/ui/src/tabs/coordinator-admin/data/scope-management.test.ts > coordinator admin scope management view helpers > gates sharing-domain management when admin setup is incomplete
- packages/ui/src/tabs/coordinator-admin/data/scope-management.test.ts > coordinator admin scope management view helpers > keeps raw Space ids in advanced Space card copy
- packages/ui/src/tabs/coordinator-admin/data/scope-management.test.ts > coordinator admin scope management view helpers > labels device Space access without making membership epochs primary copy
- packages/ui/src/tabs/coordinator-admin/data/scope-management.test.ts > coordinator admin scope management view helpers > shows enrolled devices that are not members as explicit non-member rows
- packages/ui/src/tabs/coordinator-admin/data/scope-management.test.ts > coordinator admin scope management view helpers > uses Space card copy in revoke prompts instead of raw ids
- packages/ui/src/tabs/coordinator-admin/data/target-group.test.ts > coordinator admin target group helpers > preserves dirty device rename drafts across refreshes
- packages/ui/src/tabs/coordinator-admin/data/target-group.test.ts > coordinator admin target group helpers > updates clean device rename drafts from refreshed server state
- packages/ui/src/tabs/coordinator-admin/data/team-card.test.ts > coordinator admin Team card helpers > counts active Spaces when the Spaces drawer has loaded
- packages/ui/src/tabs/coordinator-admin/data/team-card.test.ts > coordinator admin Team card helpers > ignores setup guide state for a different Team
- packages/ui/src/tabs/coordinator-admin/data/team-card.test.ts > coordinator admin Team card helpers > shows conservative migrated Team defaults when preferences are loaded
- packages/ui/src/tabs/coordinator-admin/data/team-card.test.ts > coordinator admin Team card helpers > summarizes a newly created default Space without exposing raw IDs
- packages/ui/src/tabs/coordinator-admin/data/team-card.test.ts > coordinator admin Team card helpers > trusts loaded preferences over stale setup guide state
- packages/ui/src/tabs/devices.test.tsx > read-only Devices > escapes friendly names and never renders internal identifiers or unsafe warning text
- packages/ui/src/tabs/devices.test.tsx > read-only Devices > excludes devices owned by pending or merged Identities
- packages/ui/src/tabs/devices.test.tsx > read-only Devices > gives repeated actions unique device-specific accessible names
- packages/ui/src/tabs/devices.test.tsx > read-only Devices > keeps stale cards visible while announcing a post-load refresh failure
- packages/ui/src/tabs/devices.test.tsx > read-only Devices > purely projects active devices with direct and Team-inherited Projects
- packages/ui/src/tabs/devices.test.tsx > read-only Devices > renders friendly semantic cards, safe copy, one contextual action, and revoked summary
- packages/ui/src/tabs/devices.test.tsx > read-only Devices > renders loading, error, and active-device empty states with live-region semantics
- packages/ui/src/tabs/devices.test.tsx > read-only Devices > uses explicit missing availability and hides actions when no navigation callback exists
- packages/ui/src/tabs/project-sharing.test.tsx > project sharing flow > announces successful invite copying without nesting live regions
- packages/ui/src/tabs/project-sharing.test.tsx > project sharing flow > blocks terminal-action restart when any original project is unavailable
- packages/ui/src/tabs/project-sharing.test.tsx > project sharing flow > clears selection, preview, created state, and errors when reopened
- packages/ui/src/tabs/project-sharing.test.tsx > project sharing flow > closes and clears an open selector when the complete inventory becomes unavailable
- packages/ui/src/tabs/project-sharing.test.tsx > project sharing flow > creates only the reviewed project set
- packages/ui/src/tabs/project-sharing.test.tsx > project sharing flow > disables unmapped, ambiguous, and peer-received projects
- packages/ui/src/tabs/project-sharing.test.tsx > project sharing flow > explains when the complete project selector cannot be loaded
- packages/ui/src/tabs/project-sharing.test.tsx > project sharing flow > opens a queued recovery request after direct Sync-tab startup
- packages/ui/src/tabs/project-sharing.test.tsx > project sharing flow > preselects only the canonical project requested by a row action
- packages/ui/src/tabs/project-sharing.test.tsx > project sharing flow > rejects and clears queued requests when the complete inventory cannot be loaded
- packages/ui/src/tabs/project-sharing.test.tsx > project sharing flow > renders safe copy for project_selection_ambiguous
- packages/ui/src/tabs/project-sharing.test.tsx > project sharing flow > renders safe copy for reviewed_project_set_changed
- packages/ui/src/tabs/project-sharing.test.tsx > project sharing flow > requires a fresh review after the selected project set changes
- packages/ui/src/tabs/project-sharing.test.tsx > project sharing flow > requires a project after the teammate is entered
- packages/ui/src/tabs/project-sharing.test.tsx > project sharing flow > reviews exact projects, counts, and future sharing without internal terminology
- packages/ui/src/tabs/project-sharing.test.tsx > project sharing flow > shows a safe error instead of raw server terminology
- packages/ui/src/tabs/project-sharing.test.tsx > project sharing flow > uses unique control IDs for canonical identities that sanitize alike
- packages/ui/src/tabs/project-sharing.test.tsx > project sharing flow > validates required fields and resets the teammate when reopened
- packages/ui/src/tabs/projects.test.ts > Projects tab > aggregates clustered recipients and shares all exact canonical identities
- packages/ui/src/tabs/projects.test.ts > Projects tab > blocks cluster bulk assignment when an identity needs guardrail review
- packages/ui/src/tabs/projects.test.ts > Projects tab > bulk-selects exact canonical Projects and opens sorted recipient sharing
- packages/ui/src/tabs/projects.test.ts > Projects tab > clears stale guardrail confirmation when the draft domain changes
- packages/ui/src/tabs/projects.test.ts > Projects tab > clusters related project identities and bulk assigns the group
- packages/ui/src/tabs/projects.test.ts > Projects tab > confirms cleanup before forgetting local project memories
- packages/ui/src/tabs/projects.test.ts > Projects tab > describes revoked and cancelled project shares as history
- packages/ui/src/tabs/projects.test.ts > Projects tab > disables project reassignment for saved mappings with no sessions
- packages/ui/src/tabs/projects.test.ts > Projects tab > disables stale sharing choices when a later primary inventory load fails
- packages/ui/src/tabs/projects.test.ts > Projects tab > disambiguates duplicate Space names in assignment options
- packages/ui/src/tabs/projects.test.ts > Projects tab > does not block cluster bulk assignment for informational guardrail warnings
- packages/ui/src/tabs/projects.test.ts > Projects tab > does not let an older coordinator refresh redraw stale project rows
- packages/ui/src/tabs/projects.test.ts > Projects tab > does not let an older project load overwrite a newer selector snapshot
- packages/ui/src/tabs/projects.test.ts > Projects tab > does not partially update cluster identities when bulk assignment fails
- packages/ui/src/tabs/projects.test.ts > Projects tab > does not reload inventory while a Space select is active
- packages/ui/src/tabs/projects.test.ts > Projects tab > does not render assignment controls for unmapped projects
- packages/ui/src/tabs/projects.test.ts > Projects tab > does not show bulk Space controls for unmapped-only clusters
- packages/ui/src/tabs/projects.test.ts > Projects tab > excludes legacy review from assignment options
- packages/ui/src/tabs/projects.test.ts > Projects tab > excludes peer-received identities from cluster bulk assignment
- packages/ui/src/tabs/projects.test.ts > Projects tab > explains backend guardrail confirmation as a required acknowledgement
- packages/ui/src/tabs/projects.test.ts > Projects tab > groups assignable Spaces by Team
- packages/ui/src/tabs/projects.test.ts > Projects tab > ignores stale suggested Spaces that are not assignable
- packages/ui/src/tabs/projects.test.ts > Projects tab > keeps draft domain selection after refresh
- packages/ui/src/tabs/projects.test.ts > Projects tab > keeps expanded project details open after refresh
- packages/ui/src/tabs/projects.test.ts > Projects tab > keeps inventory usable and disables recipient actions when intent loading fails
- packages/ui/src/tabs/projects.test.ts > Projects tab > keeps selection across renders and prunes Projects absent from complete inventory
- packages/ui/src/tabs/projects.test.ts > Projects tab > lets project rows reassign their stored project
- packages/ui/src/tabs/projects.test.ts > Projects tab > loads the sharing selector independently from the filtered inventory page
- packages/ui/src/tabs/projects.test.ts > Projects tab > mounts complete inventory and clears selection after a successful management commit
- packages/ui/src/tabs/projects.test.ts > Projects tab > opens row sharing with exactly the selected canonical project
- packages/ui/src/tabs/projects.test.ts > Projects tab > preserves a cluster Space draft across inventory re-renders until save succeeds
- packages/ui/src/tabs/projects.test.ts > Projects tab > preserves the continuity surface across an unchanged refresh
- packages/ui/src/tabs/projects.test.ts > Projects tab > refreshes active Team names and ignores archived Teams for Space labels
- packages/ui/src/tabs/projects.test.ts > Projects tab > removes the project inventory skeleton when loading fails
- packages/ui/src/tabs/projects.test.ts > Projects tab > renders active Team and Identity recipients with only recipient management primary
- packages/ui/src/tabs/projects.test.ts > Projects tab > renders blocked repair ownership without decision controls
- packages/ui/src/tabs/projects.test.ts > Projects tab > renders inventory before coordinator Team name refresh finishes
- packages/ui/src/tabs/projects.test.ts > Projects tab > renders mixed continuity and repair state without contradictory copy
- packages/ui/src/tabs/projects.test.ts > Projects tab > renders peer-received project identities read-only
- packages/ui/src/tabs/projects.test.ts > Projects tab > replays skipped refresh when a focused cluster Space select blurs
- packages/ui/src/tabs/projects.test.ts > Projects tab > requires explicit cluster domain choice for mixed suggestions
- packages/ui/src/tabs/projects.test.ts > Projects tab > rerenders the continuity surface when the deferred finding count changes
- packages/ui/src/tabs/projects.test.ts > Projects tab > shows empty inventory without bogus pagination range
- packages/ui/src/tabs/projects.test.ts > Projects tab > shows exact project sharing People without leaking the summary to a sibling identity
- packages/ui/src/tabs/projects.test.ts > Projects tab > surfaces suggestions and attention warnings on the collapsed card
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > blocks Project acceptance when the inspected Project list is 'empty'
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > blocks Project acceptance when the inspected Project list is 'missing'
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > blocks Project import for control characters
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > blocks Project import for empty names
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > blocks Project import for format characters
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > blocks Project import for names over 120 Unicode code points
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > discards a stale Project inspection after the invitation text changes
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > inspects and accepts add-device access with direct, inherited, and excluded Projects
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > keeps direct Project review and legacy import routed to their established journeys
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > keeps the reviewed Project payload available for retry after an acceptance error
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > keeps unknown recipient-import errors generic
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > moves focus to the heading and restores it after Radix keyboard close
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > omits the delivery expectation when an add-device invitation shares no Projects
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > persists a Team completion, prevents repeat acceptance, and resets after close and reopen
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > previews and creates a Team-member invitation with the exact reviewed request
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > retries failed Project inspection with the preserved invite text
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > reviews and accepts direct exact-Project access in the same dialog without repasting
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > seeds machine-generated identity and device names as empty fields with placeholder hints
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > shows add-device restart guidance when the active Identity could not refresh
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > shows contextual Identity guidance when add-device acceptance conflicts
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > shows contextual guidance for invite_identity_conflict
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > shows contextual guidance for recipient_invite_intent_mismatch
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > shows contextual guidance for recipient_invite_review_unavailable
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > shows loading, error, and empty states without exposing internal language
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > shows restart-required Project setup as pending and restores focus after keyboard close
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > surfaces restart guidance when Team acceptance enables sync
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > uses safe fallback result copy after reviewing unavailable optional Project identity names
- packages/ui/src/tabs/recipient-policy-invitations.test.tsx > recipient-policy invitations > uses the shared labelled close-button structure
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > allows exactly 500 changes and blocks larger reviews without an API call
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > blocks duplicate preview submissions before the first request settles
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > clears review-ready status when returning to selection
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > deduplicates effective devices by device ID while preserving first-item order
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > describes zero current-device impact
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > discards stale preview, preserves selection, and announces another review
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > labels an already-active Project recipient edge as unchanged
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > labels one recipient with mixed changes across selected Projects
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > labels recipient removals and additions without claiming unchanged recipients receive access
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > marks long Project, Team, Identity, member, and device names as wrappable
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > project-add emits canonical add edges for every seeded Project and selected recipient
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > project-manage emits only recipient add and remove diffs
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > project-manage exposes stale recipient edges only so they can be removed
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > provides labels, live regions, busy state, semantic actions, and neutral opening focus
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > recipient-add offers only additional Projects and cannot emit removals
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > recipient-manage emits the same canonical edge shape with only Project diffs
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > recipient-manage exposes stale active Projects only so they can be removed
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > renders a realistic non-idempotent conflict with text rather than color-only meaning
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > renders the three review sections in order and commits normalized changes plus digest
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > restores focus to a stable tab when polling replaced the original trigger
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > restores focus to the connected opening trigger
- packages/ui/src/tabs/recipient-policy-management.test.tsx > recipient policy management dialog > shows loading and empty states without enabling an unsafe review
- packages/ui/src/tabs/recipient-policy-sharing.test.tsx > recipient-focused Sharing > lists received Projects with counts, activity, and read-only guidance
- packages/ui/src/tabs/recipient-policy-sharing.test.tsx > recipient-focused Sharing > opens exact recipient management requests from both action labels
- packages/ui/src/tabs/recipient-policy-sharing.test.tsx > recipient-focused Sharing > renders all four accessible views and recipient-aware invitation controls
- packages/ui/src/tabs/recipient-policy-sharing.test.tsx > recipient-focused Sharing > renders loading, error, and empty states with live-region semantics
- packages/ui/src/tabs/recipient-policy-sharing.test.tsx > recipient-focused Sharing > separates direct Identity Projects from Projects inherited through active Teams
- packages/ui/src/tabs/recipient-policy-sharing.test.tsx > recipient-focused Sharing > shows Team members, current devices, shared Projects, and future-member inheritance
- packages/ui/src/tabs/recipient-policy-sharing.test.tsx > recipient-focused Sharing > supports automatic keyboard tab activation, wraparound, Home, End, and focus
- packages/ui/src/tabs/recipient-policy-sharing.test.tsx > recipient-focused Sharing > uses visible labels, responsive and target hooks, and no prohibited internal copy
- packages/ui/src/tabs/settings/components/SyncPanel.test.tsx > SyncPanel > keeps Settings focused on device sync configuration
- packages/ui/src/tabs/sync/components/project-invite-acceptance-copy.test.ts > project invite acceptance copy > asks for Person and friendly device identity without internal access language
- packages/ui/src/tabs/sync/components/project-share-operations.test.tsx > ProjectShareOperations > describes cancelled operations as historical rather than current sharing
- packages/ui/src/tabs/sync/components/project-share-operations.test.tsx > ProjectShareOperations > describes revoked operations as historical rather than current sharing
- packages/ui/src/tabs/sync/components/project-share-operations.test.tsx > ProjectShareOperations > groups strictly by actor identity and nests friendly devices and projects
- packages/ui/src/tabs/sync/components/project-share-operations.test.tsx > ProjectShareOperations > keeps recovery error codes in diagnostics instead of primary feedback
- packages/ui/src/tabs/sync/components/project-share-operations.test.tsx > ProjectShareOperations > loads an invite link only when the user asks to copy it
- packages/ui/src/tabs/sync/components/project-share-operations.test.tsx > ProjectShareOperations > renders active with one lifecycle and at most one action
- packages/ui/src/tabs/sync/components/project-share-operations.test.tsx > ProjectShareOperations > renders cancelled with one lifecycle and at most one action
- packages/ui/src/tabs/sync/components/project-share-operations.test.tsx > ProjectShareOperations > renders initial_sync with one lifecycle and at most one action
- packages/ui/src/tabs/sync/components/project-share-operations.test.tsx > ProjectShareOperations > renders needs_attention with one lifecycle and at most one action
- packages/ui/src/tabs/sync/components/project-share-operations.test.tsx > ProjectShareOperations > renders provisioning with one lifecycle and at most one action
- packages/ui/src/tabs/sync/components/project-share-operations.test.tsx > ProjectShareOperations > renders revoked with one lifecycle and at most one action
- packages/ui/src/tabs/sync/components/project-share-operations.test.tsx > ProjectShareOperations > renders revoking with one lifecycle and at most one action
- packages/ui/src/tabs/sync/components/project-share-operations.test.tsx > ProjectShareOperations > renders waiting_for_acceptance with one lifecycle and at most one action
- packages/ui/src/tabs/sync/components/project-share-operations.test.tsx > ProjectShareOperations > renders waiting_for_device with one lifecycle and at most one action
- packages/ui/src/tabs/sync/components/project-share-operations.test.tsx > ProjectShareOperations > reopens exact project sharing for cancelled operations
- packages/ui/src/tabs/sync/components/project-share-operations.test.tsx > ProjectShareOperations > reopens exact project sharing for revoked operations
- packages/ui/src/tabs/sync/components/project-share-operations.test.tsx > ProjectShareOperations > runs retry through the operation advance callback with busy semantics
- packages/ui/src/tabs/sync/components/sync-invite-join-panels.test.tsx > SyncInviteJoinPanels > keeps anchor-peer setup out of the primary invite/join flow
- packages/ui/src/tabs/sync/components/sync-peers.test.ts > canManageSpacesInTeams > allows the Teams management action only for ready coordinator admin devices
- packages/ui/src/tabs/sync/components/sync-peers.test.ts > canManageSpacesInTeams > blocks the Teams management action when admin capability is absent
- packages/ui/src/tabs/sync/components/sync-sharing-review.test.tsx > SyncSharingReview > does not show bulk preview controls without a reassignment target
- packages/ui/src/tabs/sync/components/sync-sharing-review.test.tsx > SyncSharingReview > explains inbound-only legacy groups instead of offering reassignment
- packages/ui/src/tabs/sync/components/sync-sharing-review.test.tsx > SyncSharingReview > filters cleanup groups and previews selected suggested groups in bulk
- packages/ui/src/tabs/sync/components/sync-sharing-review.test.tsx > SyncSharingReview > filters legacy groups by visible suggested Space labels
- packages/ui/src/tabs/sync/components/sync-sharing-review.test.tsx > SyncSharingReview > keeps failed legacy reassignment visible instead of clearing like success
- packages/ui/src/tabs/sync/components/sync-sharing-review.test.tsx > SyncSharingReview > lets users choose a non-suggested legacy destination before preview
- packages/ui/src/tabs/sync/components/sync-sharing-review.test.tsx > SyncSharingReview > renders grouped legacy shared review without automatic promotion
- packages/ui/src/tabs/sync/components/sync-sharing-review.test.tsx > SyncSharingReview > requires an explicit destination for legacy groups without a suggestion
- packages/ui/src/tabs/sync/components/sync-sharing-review.test.tsx > SyncSharingReview > requires explicit confirmation before applying a suggested legacy domain
- packages/ui/src/tabs/sync/components/team-sync-panel.test.tsx > TeamSyncPanel primary status > renders one concrete next action for configured unreachable
- packages/ui/src/tabs/sync/components/team-sync-panel.test.tsx > TeamSyncPanel primary status > renders one concrete next action for enrolled and reachable only
- packages/ui/src/tabs/sync/components/team-sync-panel.test.tsx > TeamSyncPanel primary status > renders one concrete next action for not enrolled
- packages/ui/src/tabs/sync/components/team-sync-panel.test.tsx > TeamSyncPanel primary status > renders one concrete next action for owner needs_attention
- packages/ui/src/tabs/sync/components/team-sync-panel.test.tsx > TeamSyncPanel primary status > renders one concrete next action for pending_setup
- packages/ui/src/tabs/sync/components/team-sync-panel.test.tsx > TeamSyncPanel primary status > renders one concrete next action for posted presence with sync disabled
- packages/ui/src/tabs/sync/components/team-sync-panel.test.tsx > TeamSyncPanel primary status > renders one concrete next action for trust pending
- packages/ui/src/tabs/sync/components/team-sync-panel.test.tsx > TeamSyncPanel primary status > renders one concrete next action for unconfigured setup
- packages/ui/src/tabs/sync/components/team-sync-panel.test.tsx > TeamSyncPanel primary status > reserves no-urgent copy for healthy enabled data-plane sync
- packages/ui/src/tabs/sync/diagnostics.test.ts > cleanupDiagnosticLabel > labels cleanup-specific sync states without raw ids
- packages/ui/src/tabs/sync/diagnostics.test.ts > syncAttemptsHistoryNote > explains that offline-peer attempts may be historical
- packages/ui/src/tabs/sync/diagnostics.test.ts > syncAttemptsHistoryNote > stays empty for other daemon states
- packages/ui/src/tabs/sync/diagnostics.test.ts > syncAttemptsHistoryNote > stays empty when there are no visible failures to explain
- packages/ui/src/tabs/sync/diagnostics/helpers.test.ts > pairingView > builds a base64 copy command when the payload is present
- packages/ui/src/tabs/sync/diagnostics/helpers.test.ts > pairingView > falls back to a not-available message when the payload is not an object
- packages/ui/src/tabs/sync/diagnostics/render/sync-attempts.test.ts > renderSyncAttempts > links redacted failed attempts to the Advanced diagnostics redaction setting
- packages/ui/src/tabs/sync/diagnostics/render/sync-attempts.test.ts > shouldShowSyncAttemptRedactionHint > does not show the reveal hint for unredacted generic-looking errors
- packages/ui/src/tabs/sync/diagnostics/render/sync-attempts.test.ts > shouldShowSyncAttemptRedactionHint > shows the reveal hint only for explicitly redacted attempt errors
- packages/ui/src/tabs/sync/helpers.test.ts > actorDisplayLabel > labels the local actor as You
- packages/ui/src/tabs/sync/helpers.test.ts > assignmentNote > describes local assignment as identity across devices
- packages/ui/src/tabs/sync/helpers.test.ts > buildActorSelectOptions > keeps You available while hiding unresolved duplicate people elsewhere
- packages/ui/src/tabs/sync/helpers.test.ts > buildActorSelectOptions > keeps an explicit unassigned choice in the option list
- packages/ui/src/tabs/sync/helpers.test.ts > buildActorSelectOptions > preserves a selected hidden actor so the control never renders blank
- packages/ui/src/tabs/sync/helpers.test.ts > shouldClearStalePeersFeedback > clears when the related peer reappears in the loaded list
- packages/ui/src/tabs/sync/helpers.test.ts > shouldClearStalePeersFeedback > does not clear when feedback has no relatedPeerDeviceId
- packages/ui/src/tabs/sync/helpers.test.ts > shouldClearStalePeersFeedback > does not clear when feedback is null
- packages/ui/src/tabs/sync/helpers.test.ts > shouldClearStalePeersFeedback > does not clear when no peers match
- packages/ui/src/tabs/sync/helpers.test.ts > shouldClearStalePeersFeedback > trims whitespace before comparing peer ids
- packages/ui/src/tabs/sync/index.test.ts > loadSyncData > does not extend the health-tab cache ttl on cache hits
- packages/ui/src/tabs/sync/index.test.ts > loadSyncData > does not request secondary sync data when status fails
- packages/ui/src/tabs/sync/index.test.ts > loadSyncData > ignores stale out-of-order sync payloads from older refreshes
- packages/ui/src/tabs/sync/index.test.ts > loadSyncData > keeps peer diagnostics when project sharing lifecycle loading fails
- packages/ui/src/tabs/sync/sync-dialogs/components/duplicate-person-content.test.tsx > DuplicatePersonDialogContent > submits the merge decision when Enter is pressed on the focused primary selector
- packages/ui/src/tabs/sync/team-sync/events/init-team-sync-events.test.ts > project invite review events > explains when the shared project flow is unavailable
- packages/ui/src/tabs/sync/team-sync/events/init-team-sync-events.test.ts > project invite review events > forwards confirmed project identity only after review
- packages/ui/src/tabs/sync/team-sync/events/init-team-sync-events.test.ts > project invite review events > ignores stale inspection results after the invite input changes
- packages/ui/src/tabs/sync/team-sync/events/init-team-sync-events.test.ts > project invite review events > moves focus into the labelled review region before acceptance
- packages/ui/src/tabs/sync/team-sync/events/init-team-sync-events.test.ts > project invite review events > opens the shared project flow from Sync
- packages/ui/src/tabs/sync/team-sync/events/init-team-sync-events.test.ts > project invite review events > reports project invitation pending inviter detail as a warning
- packages/ui/src/tabs/sync/team-sync/events/init-team-sync-events.test.ts > project invite review events > reports project invitation pending-setup status detail as a warning
- packages/ui/src/tabs/sync/team-sync/events/init-team-sync-events.test.ts > project invite review events > reports project invitation restart-required detail as a warning
- packages/ui/src/tabs/sync/team-sync/events/init-team-sync-events.test.ts > project invite review events > requires a second truthful action before importing a legacy invite
- packages/ui/src/tabs/sync/team-sync/render/render-team-sync.test.ts > needsCoordinatorGroupReview > keeps a paired device out of review when it belongs to multiple groups
- packages/ui/src/tabs/sync/team-sync/render/render-team-sync.test.ts > needsCoordinatorGroupReview > keeps a paired multi-group device in review when local approval is pending
- packages/ui/src/tabs/sync/team-sync/render/render-team-sync.test.ts > needsCoordinatorGroupReview > keeps an unpaired multi-group device in review
- packages/ui/src/tabs/sync/team-sync/render/render-team-sync.test.ts > renderTeamSyncPrimaryStatus > renders configured unreachable badge and metadata
- packages/ui/src/tabs/sync/team-sync/render/render-team-sync.test.ts > renderTeamSyncPrimaryStatus > renders disabled badge and metadata
- packages/ui/src/tabs/sync/team-sync/render/render-team-sync.test.ts > renderTeamSyncPrimaryStatus > renders healthy badge and metadata
- packages/ui/src/tabs/sync/team-sync/render/render-team-sync.test.ts > renderTeamSyncPrimaryStatus > renders needs attention badge and metadata
- packages/ui/src/tabs/sync/team-sync/render/render-team-sync.test.ts > renderTeamSyncPrimaryStatus > renders not enrolled badge and metadata
- packages/ui/src/tabs/sync/team-sync/render/render-team-sync.test.ts > renderTeamSyncPrimaryStatus > renders pending setup badge and metadata
- packages/ui/src/tabs/sync/team-sync/render/render-team-sync.test.ts > renderTeamSyncPrimaryStatus > renders reachable badge and metadata
- packages/ui/src/tabs/sync/team-sync/render/render-team-sync.test.ts > renderTeamSyncPrimaryStatus > renders trust blocked badge and metadata
- packages/ui/src/tabs/sync/team-sync/render/render-team-sync.test.ts > renderTeamSyncPrimaryStatus > renders unconfigured setup needed badge and metadata
- packages/ui/src/tabs/sync/view-model.test.ts > deriveCoordinatorApprovalSummary > flags coordinator devices that are still waiting on the other device
- packages/ui/src/tabs/sync/view-model.test.ts > deriveCoordinatorApprovalSummary > flags coordinator devices that need approval on this device
- packages/ui/src/tabs/sync/view-model.test.ts > deriveCoordinatorApprovalSummary > still flags devices that need local approval after a prior local peer record exists
- packages/ui/src/tabs/sync/view-model.test.ts > deriveCoordinatorSetupBlocker > asks users to configure a coordinator URL first
- packages/ui/src/tabs/sync/view-model.test.ts > deriveCoordinatorSetupBlocker > asks users to enable sync before pairing when coordinator setup exists
- packages/ui/src/tabs/sync/view-model.test.ts > deriveCoordinatorSetupBlocker > asks users to join or configure a team before pairing
- packages/ui/src/tabs/sync/view-model.test.ts > deriveCoordinatorSetupBlocker > returns no blocker when coordinator setup and sync are ready
- packages/ui/src/tabs/sync/view-model.test.ts > deriveDuplicatePeople > groups duplicate display names and preserves local involvement
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerAuthorizedDomainsView > does not promote raw Space ids when authorized Space labels are missing
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerAuthorizedDomainsView > formats authorized Spaces without exposing membership internals
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerAuthorizedDomainsView > labels peers with no cached Space access
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerGrantRoleMismatchView > counts explicitly work-like local scope names as valid grants
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerGrantRoleMismatchView > counts local work scopes as valid work/client-like grants
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerGrantRoleMismatchView > does not flag peers once a separate work/client-like grant is present
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerGrantRoleMismatchView > does not infer role mismatches without coordinator or group context
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerGrantRoleMismatchView > does not treat substring matches inside public-looking names as OSS grants
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerGrantRoleMismatchView > does not treat substring matches inside work domain names as personal grants
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerGrantRoleMismatchView > flags coordinator-discovered peers with personal or OSS grants but no work/client-like grant
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerProjectNarrowingView > explains project filters as narrowing instead of grants
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerProjectNarrowingView > treats all-project/no-exclusion defaults as no advanced filters
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerScopeRejectionsView > renders human-readable labels and orders reasons by count desc
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerScopeRejectionsView > returns an empty view when nothing has been rejected
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerScopeRejectionsView > uses singular badge label when there is exactly one rejection
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerScopeSyncView > keeps raw Space ids out of primary labels when labels are missing
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerScopeSyncView > maps per-Space cursor state to received and pending rows
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerTrustSummary > prioritizes current offline state over stale unauthorized history
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerTrustSummary > surfaces re-pairing guidance when the remote device rejects us with unauthorized
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerTrustSummary > surfaces two-way trust once sync or ping succeeds
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerTrustSummary > treats timeout-heavy device errors as offline guidance
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerUiStatus > flags unauthorized peers as needing re-pairing attention
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerUiStatus > maps stale peers to offline
- packages/ui/src/tabs/sync/view-model.test.ts > derivePeerUiStatus > treats timeout-heavy peers as offline instead of generic repair
- packages/ui/src/tabs/sync/view-model.test.ts > deriveSyncViewModel > creates attention items for duplicates and device issues that need review
- packages/ui/src/tabs/sync/view-model.test.ts > deriveSyncViewModel > does not count a stale coordinator record as offline when the paired peer is actively connected
- packages/ui/src/tabs/sync/view-model.test.ts > deriveSyncViewModel > hides duplicate-person attention when the user already marked them as different people
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > directs fresh ping without a successful sync to run sync
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > directs online presence without a successful sync to run sync
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > directs users to check a degraded peer
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > directs users to check a errored peer
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > directs users to check an offline peer before suggesting another pairing
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > directs users to check paired devices when daemon_state is degraded
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > directs users to check paired devices when daemon_state is offline-peers
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > directs users to check paired devices when daemon_state is stale
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > directs users to sync-service guidance when daemon_state is disabled
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > directs users to sync-service guidance when daemon_state is error
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > directs users to sync-service guidance when daemon_state is needs_attention
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > directs users to sync-service guidance when daemon_state is rebootstrapping
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > directs users to sync-service guidance when daemon_state is starting
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > directs users to sync-service guidance when daemon_state is stopped
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > directs users to sync-service guidance when daemon_state is stopping
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > fails closed when Project sharing lifecycle status could not be loaded
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > fails closed when coordinator.sync_enabled is missing despite daemon health and mutual trust
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > fails closed when daemon_state is unavailable
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > fails closed when status.enabled is missing despite daemon health and mutual trust
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > fails closed when the daemon explicitly is not running
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > keeps a revoking Project operation above healthy sync
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > keeps a trust blocker above a separate otherwise-healthy peer and posted presence
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > keeps a trusted peer with pending Space delivery out of Healthy
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > keeps owner needs_attention above pending reconciliation and healthy sync
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > keeps pending setup above trust blockers, healthy peers, and posted presence
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > keeps pending_setup above trust, coordinator presence, and healthy peers
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > keeps sync disabled above posted presence and every lower-priority signal
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > labels enrolled and reachable coordinator presence without claiming healthy sync
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > models configured unreachable with one exact directive
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > models not enrolled with one exact directive
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > models unconfigured with one exact directive
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > prioritizes coordinator error over peer-connectivity guidance
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > prioritizes not enrolled over peer-connectivity guidance
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > prioritizes pairing blockers when another paired device is offline
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > prioritizes unknown coordinator presence over peer-connectivity guidance
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > reports healthy only for an enabled trusted data plane without higher blockers
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > routes reconciliation-only failures to Sharing management
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > routes unauthorized peer errors to re-pairing guidance
- packages/ui/src/tabs/sync/view-model.test.ts > deriveTeamSyncPrimaryStatus > tells the owner to send a pending invitation
- packages/ui/src/tabs/sync/view-model.test.ts > deriveVisiblePeopleActors > hides unresolved zero-device duplicates of the local person from the people list
- packages/ui/src/tabs/sync/view-model.test.ts > deriveVisiblePeopleActors > keeps duplicate rows visible when the non-local duplicate already owns devices
- packages/ui/src/tabs/sync/view-model.test.ts > deviceNeedsFriendlyName > does not require naming when a friendly label already exists
- packages/ui/src/tabs/sync/view-model.test.ts > deviceNeedsFriendlyName > requires naming when no local or coordinator name exists
- packages/ui/src/tabs/sync/view-model.test.ts > resolveFriendlyDeviceName > falls back to coordinator display name before raw device ids
- packages/ui/src/tabs/sync/view-model.test.ts > resolveFriendlyDeviceName > prefers the explicit local name first
- packages/ui/src/tabs/sync/view-model.test.ts > resolveFriendlyDeviceName > uses a short fallback when nothing friendly exists
- packages/ui/src/tabs/sync/view-model.test.ts > shouldShowCoordinatorReviewAction > allows fresh unpaired discovered devices without a visible fingerprint
- packages/ui/src/tabs/sync/view-model.test.ts > shouldShowCoordinatorReviewAction > keeps already-paired devices hidden when they are only waiting on the other side
- packages/ui/src/tabs/sync/view-model.test.ts > shouldShowCoordinatorReviewAction > keeps rejoined devices actionable when reciprocal local approval is still needed
- packages/ui/src/tabs/sync/view-model.test.ts > summarizeSyncRunResult > falls back to the mixed-failure summary when trust and scope failures coexist
- packages/ui/src/tabs/sync/view-model.test.ts > summarizeSyncRunResult > keeps non-membership scoped sync incomplete failures generic
- packages/ui/src/tabs/sync/view-model.test.ts > summarizeSyncRunResult > keeps structured connectivity and other failures generic
- packages/ui/src/tabs/sync/view-model.test.ts > summarizeSyncRunResult > prefers structured scope failure categories over generic text
- packages/ui/src/tabs/sync/view-model.test.ts > summarizeSyncRunResult > prefers structured trust failure categories over error text
- packages/ui/src/tabs/sync/view-model.test.ts > summarizeSyncRunResult > routes scope_rejected failures to the Teams Space-access message
- packages/ui/src/tabs/sync/view-model.test.ts > summarizeSyncRunResult > routes scoped sync incomplete failures to the Teams Space-access message
- packages/ui/src/tabs/sync/view-model.test.ts > summarizeSyncRunResult > splits outbound filter diagnostics by reason
- packages/ui/src/tabs/sync/view-model.test.ts > summarizeSyncRunResult > summarizes mixed failures without pretending they are all one-way trust
- packages/ui/src/tabs/sync/view-model.test.ts > summarizeSyncRunResult > surfaces outbound filter diagnostics without treating them as failed sync
- packages/ui/src/tabs/sync/view-model.test.ts > summarizeSyncRunResult > turns unauthorized sync failures into a re-pairing message
- packages/ui/src/tabs/sync/view-model.test.ts > summarizeSyncRunResult > uses structured per-scope categories when the top-level item lacks one

### R-VIEWER (217)

- packages/viewer-server/src/index.test.ts > viewer-server > /api/config > ignores unchanged protected keys during full config saves
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > accepts a device pairing payload and writes the peer row
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > accepts a discovered coordinator device into sync peers
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > accepts a raw JSON pairing payload without base64 wrapping
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > accepts authorized feature-advertised reassign_scope batches
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > accepts capability metadata on POST /v1/ops bodies
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > accepts discovered peers when only plural coordinator groups are configured
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > accepts non-empty legacy pushes from unsupported peers
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > advertises capability on incremental /v1/ops responses
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > advertises reassign_scope and refreshes authorization only on explicit feature-aware status requests
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > allows /v1/status with a valid bootstrap grant using seed-device lookup
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > allows manual enroll-peer without admin secret and gates discovered-mode
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > assigns a sync peer to the local actor through the identity route
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > blocks unmapped projects from non-local Sharing domain assignment
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > claims a legacy device identity through the viewer route
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > clamps absurd sinceMinutes values instead of throwing on Date overflow
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > creates a 'Team' invite through the owner viewer and accepts it in a separate fresh recipient
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > creates a 'add-device' invite through the owner viewer and accepts it in a separate fresh recipient
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > creates add-device invites without an admin secret while Team creation stays admin-only
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > creates coordinator invites through the coordinator admin route
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > creates coordinator invites through the viewer route
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > creates, renames, and merges actors through viewer routes
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > deletes sync peers through the viewer route
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > demotes stale is_local=1 rows so only the canonical local actor stays marked local
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > does not allow a bootstrap grant to access /v1/ops
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > does not auto-grant a stale non-default Space preference on join approval
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > does not auto-grant when the canonical default Space is missing or wrong kind
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > does not expose /v1/status on viewer app (moved to sync listener)
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > does not expose join requests without diagnostics
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > does not expose viewer routes on sync app
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > does not let latest needs_attention metadata override live starting state
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > does not let unauthorized sync requests consume a verified peer bucket
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > does not outbound-scope-filter scoped pushes from unsupported peers
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > does not persist a local peer when reciprocal approval publish fails
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > drives syncOnce through real syncProtocolRoutes for multiple Spaces
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > exposes /v1/ops on sync app (auth-gated)
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > exposes /v1/snapshot on sync app (auth-gated)
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > exposes /v1/status on sync app (auth-gated)
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > exposes sync auth failure reasons only when diagnostics are explicitly enabled
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > fails closed with safe errors for unavailable or tampered recipient reviewed intent
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > filters local-only outbound /v1/ops before project filters
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > filters null-payload pushed deletes by the existing memory project
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > filters pushed ops by peer project filters before applying
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > forgets locally owned project memories while leaving peer-owned copies
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > gates coordinator Sharing domain admin routes without an admin secret
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > hard-rejects inbound scope failures without claimed_local_actor bypass
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > identifies claimed local actor peers that need a personal scope grant
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > imports directly from a fallback store with one stable human-named local actor
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > includes detailed maintenance summaries only when diagnostics are enabled
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > includes retention telemetry in sync status output
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > includes semantic-index diagnostics in sync status output
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > inspects and accepts Team/add-device invites with intent-only local writes
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > inspects and accepts a project invite with confirmed local identity only
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > keeps configured identity canonical and hides a stale fallback during invite import
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > keeps project invite reads pure and reconciles authoritative acceptance explicitly and idempotently
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > keeps the cheap semantic diagnostics path when sync diagnostics are explicitly requested
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > lists bootstrap grants through viewer sync routes
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > lists coordinator devices through the admin route
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > lists coordinator groups through the admin route
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > lists coordinator join requests through the admin route
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > maps claimed_local_actor=true to the local actor id when no actor id is provided
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > maps project provision errors and preflights reassignment before mutations
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > persists restart-required project setup and replays acceptance without duplicate records
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > picks up a Space granted after an initial scoped sync
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > preflights bulk project mappings before writing any mappings
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > previews and applies legacy shared review reassignment explicitly
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > previews and creates Team and add-device invites through the recipient coordinator contract
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > previews and creates an exact project-first invite from server inventory
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > proxies coordinator Sharing domain membership routes without relaying memory payloads
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > proxies coordinator Sharing domain metadata routes without relaying memory payloads
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rate limits repeated sync listener requests
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > reads and writes local coordinator group preferences without admin secret
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > reassigns a project inventory row to the corrected project
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > redacts discovered coordinator device metadata without diagnostics
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > redacts enrollment reconciliation issue identifiers while retaining counts
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > redacts maintenance details when diagnostics are disabled
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > redacts semantic-index job details when diagnostics are disabled
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > redacts sync attempt errors unless diagnostics are requested
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > refreshes an existing multi-group peer address cache from sync status without re-pairing
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects a device pairing payload that conflicts with an existing trusted fingerprint
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects a mismatched inspect 'kind' without mutating the recipient DB or config
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects a mismatched inspect 'reviewed digest' without mutating the recipient DB or config
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects a mismatched inspect 'target ID' without mutating the recipient DB or config
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects a pairing payload whose fingerprint does not match its public key
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects accepting a discovered device when an existing peer is pinned to another fingerprint
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects accepting a discovered device when multiple coordinator groups match
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects bootstrap grants whose worker enrollment does not match the granted worker
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects bulk project mappings before writing any partial updates
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects legacy shared review reassignment when the local device lacks target membership
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects local-default as a legacy shared review reassignment target
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects malformed feature-advertised reassign_scope payloads
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects merging the local actor into another actor
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects oversized sync request bodies before auth processing
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects project memory cleanup when the previewed row set changes
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects reassign_scope batches without feature advertisement
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects scope-rejections lookup with missing peer id
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects stale discovered coordinator devices before publishing reciprocal approval
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > rejects stale legacy shared review confirmation when the group changes
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > renames an existing sync peer through the viewer route
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > reports coordinator admin readiness states
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > reports inbound-only legacy shared review groups without offering reassignment capacity
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > requires confirmation before reassigning an existing project mapping
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > requires confirmation before saving risky Sharing domain mappings
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > requires explicit reset boundary metadata on incremental sync requests
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > respects env-only sync configuration in status output
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > retries coordinator presence immediately after not_enrolled status
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns 400 (not 500) when POST /api/sync/peers/rename gets a malformed body
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns 400 for /v1/snapshot without required params
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns 400 with coordinator_groups_empty when URL is set but no groups
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns 400 with coordinator_url_missing when no coordinator URL is configured
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns 400 with sync_disabled when coordinator is fully set up but sync is off
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns 404 when deleting a missing sync peer
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns 404 when the discovered device is no longer present
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns 409 when manual enroll-peer collides with an existing peer
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns 502 when coordinator lookup fails during acceptance
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns a clearer reachability error when invite import cannot contact the coordinator
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns a clearer timeout error when invite import cannot reach the coordinator in time
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns a deterministic read-only legacy recipient-policy projection
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns a safe actionable error when project sync config cannot be written
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns all legacy shared review groups so every group has an action path
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns bounded safe enrollment issue diagnostics in open-first recency order
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns confirmed setup suggestion fields in Sharing domain settings
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns effective global scope when a peer inherits global filters
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns empty enrollment reconciliation issue counts without diagnostics
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns empty peers list
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns invite warnings for private-looking coordinator URLs
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns pairing payload addresses that the CLI accept flow can use
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns read-only coordinator discovered devices and counts
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns real legacy device and sharing review summaries
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns real sync config and coordinator status details
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns reset_required for empty POST scope_id before missing ops validation
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns reset_required for unsupported POST scope_id before missing ops validation
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns reset_required for unsupported POST scope_id before oversized ops validation
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns reset_required metadata for stale peer cursors
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns reset_required when GET /v1/ops receives an empty scope_id
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns reset_required when GET /v1/ops receives an unsupported scope_id
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns reset_required when GET /v1/snapshot receives an empty scope_id
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns reset_required when GET /v1/snapshot receives an unsupported scope_id
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns reset_required when POST /v1/ops receives an unsupported body scope_id
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns retryable 503 when recipient-policy commit cannot acquire the write lock
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns retryable busy when sync auth cannot record a nonce
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns safe recipient-policy reconciliation states with the delivered-copy warning
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns scope-rejection records for a peer without exposing payloads
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > returns searchable project inventory for the Projects screen
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > reuses a reconciled Team identity for a later exact-Project share
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > reviews coordinator join requests through the admin route
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > revokes bootstrap grants through viewer sync routes
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > runs coordinator device admin actions through the admin routes
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > runs coordinator group lifecycle actions through the admin routes
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > runs sync for all peers through the compatibility sync route
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > serves and resolves recipient-policy review without mutating protected state
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > serves paginated memory bootstrap pages with tombstones
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > serves safe recipient intent and strictly validates per-Project migration
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > skips coordinator join request lookup unless includeJoinRequests=1
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > strictly previews and commits safe recipient-policy edge changes
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > surfaces authorized Sharing domains separately from project narrowing
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > surfaces grouped legacy shared review summary in sync status
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > surfaces inbound scope-rejection summary per peer
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > surfaces reciprocal approval state on discovered coordinator devices
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > surfaces runtime sync startup state before daemon settles
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > sync app does not apply CORS origin guard on POST
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > treats naive sync timestamps as UTC
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > unwraps the shell pairing command and accepts the embedded payload
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > updates an existing peer when the discovered fingerprint matches
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > updates local Sharing domain project mappings without granting membership
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > updates peer project scope through the viewer route
- packages/viewer-server/src/index.test.ts > viewer-server > GET /api/sync/peers > uses cheap semantic diagnostics for non-diagnostic sync status requests
- packages/viewer-server/src/index.test.ts > viewer-server > sync UI api routes all exist in the viewer sync router
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > accepts the local owner device capability without a recipient enrollment binding
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > backs off recent waiting-for-device operations
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > backs off transient invite reconciliation errors without poisoning daemon health
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > binds reviewed bootstrap capability evidence to the boundary group, identity, and key
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > does not poll a recently checked waiting-for-acceptance operation
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > keeps capability undetermined when reviewed invite evidence validation fails
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > keeps transient invite reconciliation passive and recovers after the coordinator returns
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > leaves terminal needs-attention operations for explicit user retry
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > moves deterministic advancement failure coordinator_not_configured to explicit recovery
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > moves deterministic advancement failure team_selection_ambiguous to explicit recovery
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > moves deterministic advancement failure team_sharing_not_configured to explicit recovery
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > moves status-less deterministic conflict coordinator_not_configured to explicit recovery
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > moves status-less deterministic conflict device_binding_conflict to explicit recovery
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > moves status-less deterministic conflict intent_conflict to explicit recovery
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > moves status-less deterministic conflict inviter_identity_conflict to explicit recovery
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > moves status-less deterministic conflict operation_intent_mismatch to explicit recovery
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > moves status-less deterministic conflict pending_person_identity_conflict to explicit recovery
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > moves status-less deterministic conflict recipient_actor_conflict to explicit recovery
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > moves status-less deterministic conflict recipient_device_identity_conflict to explicit recovery
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > moves status-less deterministic conflict recipient_fingerprint_mismatch to explicit recovery
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > moves status-less deterministic conflict team_selection_ambiguous to explicit recovery
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > moves status-less deterministic conflict team_sharing_not_configured to explicit recovery
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > moves terminal invite reconciliation errors to explicit recovery
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > polls waiting-for-acceptance operations and backs off after a pending response
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > preserves cooldown when capability preflight waits on another device
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > preserves cooldown when the latest peer activity was not a fully successful sync
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > preserves cooldown while a capability preflight retry is running
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > prioritizes advanceable work over older invite polling
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > processes a bounded oldest-first locally owned set and isolates failures
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > reports a newly terminal operation without failing global daemon health
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > requires scoped enforcement and reassign_scope capability
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > retries a recent waiting-for-device operation after a fully successful sync
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > retries waiting-for-device operations through the existing advance seam
- packages/viewer-server/src/share-operation-maintenance.test.ts > advancePendingProjectShares > treats an offline recipient as passive waiting instead of a daemon failure
- packages/viewer-server/src/share-operation-maintenance.test.ts > recipient-policy maintenance > backs off persisted failures and resumes them after the retry window
- packages/viewer-server/src/share-operation-maintenance.test.ts > recipient-policy maintenance > blocks a stale legacy share from regranting after active policy removal
- packages/viewer-server/src/share-operation-maintenance.test.ts > recipient-policy maintenance > bounds work and isolates one Project failure from the next
- packages/viewer-server/src/share-operation-maintenance.test.ts > recipient-policy maintenance > reads enabled and disabled enrollments from the exact managed boundary group
- packages/viewer-server/src/share-operation-maintenance.test.ts > recipient-policy maintenance > reconciles a first all-revoked transition without an authority row
- packages/viewer-server/src/share-operation-maintenance.test.ts > recipient-policy maintenance > rejects boundary enrollment reads outside the configured coordinator authority
- packages/viewer-server/src/share-operation-maintenance.test.ts > recipient-policy maintenance > rejects malformed boundary enrollment rows before reconciliation
- packages/viewer-server/src/share-operation-maintenance.test.ts > recipient-policy maintenance > rejects non-binary boundary enrollment state
- packages/viewer-server/src/share-operation-maintenance.test.ts > recipient-policy maintenance > uses persisted steps for two-pass cutover without duplicate coordinator effects
- packages/viewer-server/src/share-operation-maintenance.test.ts > reconcileConfiguredCoordinatorEnrollment > hashes unsafe group ids and never echoes arbitrary rejection text
- packages/viewer-server/src/share-operation-maintenance.test.ts > reconcileConfiguredCoordinatorEnrollment > preserves issues on fetch failure and resolves them after a successful empty snapshot
- packages/viewer-server/src/share-operation-maintenance.test.ts > reconcileConfiguredCoordinatorEnrollment > reads and reconciles each configured group
- packages/viewer-server/src/share-operation-maintenance.test.ts > reconcileConfiguredCoordinatorEnrollment > reports a sanitized local snapshot reconciliation failure
- packages/viewer-server/src/share-operation-maintenance.test.ts > reconcileConfiguredCoordinatorEnrollment > reports sanitized failures for each coordinator fetch stage
- packages/viewer-server/src/share-operation-maintenance.test.ts > reconcileConfiguredCoordinatorEnrollment > skips recipient devices without coordinator admin configuration

## T058 additional baseline retired fully qualified names（188）

事前 baseline に存在し final inventory から除去済みだが、従来の retire 節に未記録だった test。baseline 順・multiset multiplicity を保持する。

- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > annotates the timeline header when the file was modified after the newest observation
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > bypasses the size gate for small config files (json/toml/yaml)
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > delivers file context without recording when retrieval evidence capture is disabled
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > does not annotate when the file mtime is within the fresh tolerance window
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > does not classify in-repo basenames starting with .. as outside cwd
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > emits a PreToolUse additionalContext when observations exist and file is older
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > keeps disabled file-context fail-open when the database cannot be resolved
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > keeps disabled file-context fail-open when the ledger write fails
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > keeps successful delivery fail-open when the ledger recorder throws
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > keeps successful hook output and handed-off delivery when store cleanup throws
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > logs file_context.skip when the file is below the size gate and not a small-config bypass
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > logs file_context.skip when the file resolves outside cwd
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > logs file_context.skip when the query returns no observations
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > records disabled file-context as one skipped attempt without changing hook output
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > records file-context query failures with a stable code
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > records selected observations as handed off and correlates a known Claude session
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > records selection before formatting and confirms delivery afterward
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > records the 'below-size-gate' lifecycle without absolute paths
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > records the 'no-observations' lifecycle without absolute paths
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > records the 'outside-cwd' lifecycle without absolute paths
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > registers expected options and help text
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > returns continue when CODEMEM_FILE_CONTEXT disables injection
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > returns continue when CODEMEM_PLUGIN_IGNORE is truthy
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > returns continue when file is below the size gate
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > returns continue when file is missing from disk
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > returns continue when payload has no file_path
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > score-then-dedupe surfaces the highest-scoring observation per session
- packages/cli/src/commands/claude-hook-file-context.test.ts > claude-hook-file-context command > uses the production store retrieval path when no query dependency is injected
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > hasSpooledEntries > ignores in-flight tmp files and quarantined .bad-* files
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > hasSpooledEntries > returns false when the spool dir does not exist
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > hasSpooledEntries > returns false when the spool dir is empty
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > hasSpooledEntries > returns true when an active hook entry exists
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > lockTtlSeconds + spoolDir reflect env overrides > returns custom TTL when env is set
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > lockTtlSeconds + spoolDir reflect env overrides > returns the env-overridden spool dir
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > recoverStaleTmpSpool > leaves fresh .hook-tmp-*.json files alone
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > recoverStaleTmpSpool > renames stale .hook-tmp-*.json files to hook-recovered-*.json
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > shouldForceBoundaryFlush > does not flush Stop unless both flush envs opt in
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > shouldForceBoundaryFlush > flushes SessionEnd by default
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > shouldForceBoundaryFlush > respects CODEMEM_CLAUDE_HOOK_FLUSH=0 to disable SessionEnd flushing
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > shouldForceBoundaryFlush > returns false for unrelated events
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > spool failure logging > appends a spooled payload note to the plugin log
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > spool failure logging > logs reading failure when payload file disappears between listdir and read
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > spool stat survives missing dir > drainSpool returns zero counts when the dir cannot be created
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > spoolPayload + drainSpool roundtrip > ignores tmp files (.hook-tmp-*.json) during drain
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > spoolPayload + drainSpool roundtrip > leaves the spool entry on disk when the handler returns false
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > spoolPayload + drainSpool roundtrip > processes queued payloads in lexicographic (oldest-first) filename order
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > spoolPayload + drainSpool roundtrip > quarantines malformed JSON files so they don't loop forever
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > spoolPayload + drainSpool roundtrip > quarantines parseable but non-object payloads
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > spoolPayload + drainSpool roundtrip > writes a payload that drainSpool can replay through a handler
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > statSync sanity (no test pollution) > baseDir is the only thing the test owns
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > withClaudeHookIngestLock > acquires the lock, runs fn, and removes the lock dir on success
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > withClaudeHookIngestLock > removes the lock dir even when fn throws
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > withClaudeHookIngestLock > throws LockBusyError when the lock cannot be acquired
- packages/cli/src/commands/claude-hook-ingest-spool.test.ts > claude-hook-ingest-spool > withClaudeHookIngestLock > treats a lock held by a non-existent PID as stale and recovers it
- packages/cli/src/commands/claude-hook-ingest.test.ts > claude-hook-ingest command > direct enqueue bootstraps fresh databases on demand
- packages/cli/src/commands/claude-hook-ingest.test.ts > claude-hook-ingest command > direct enqueue inserts once and then deduplicates event_id
- packages/cli/src/commands/claude-hook-ingest.test.ts > claude-hook-ingest command > direct enqueue skips unsupported hook payloads gracefully
- packages/cli/src/commands/claude-hook-ingest.test.ts > claude-hook-ingest command > durability layer > Stop flush truth table: only fires when BOTH flush envs are truthy
- packages/cli/src/commands/claude-hook-ingest.test.ts > claude-hook-ingest command > durability layer > drains spooled backlog on the HTTP-success path so a recovered viewer doesn't strand entries
- packages/cli/src/commands/claude-hook-ingest.test.ts > claude-hook-ingest command > durability layer > drains spooled payloads through the handler before processing the new payload
- packages/cli/src/commands/claude-hook-ingest.test.ts > claude-hook-ingest command > durability layer > force-flushes SessionEnd via direct ingest + boundary flush even when HTTP succeeded
- packages/cli/src/commands/claude-hook-ingest.test.ts > claude-hook-ingest command > durability layer > skips backlog drain on HTTP success when spool is empty (no extra HTTP calls)
- packages/cli/src/commands/claude-hook-ingest.test.ts > claude-hook-ingest command > durability layer > spools the payload when both HTTP and direct ingest fail
- packages/cli/src/commands/claude-hook-ingest.test.ts > claude-hook-ingest command > durability layer > treats HTTP `skipped > 0` (deterministic null envelope) as a successful no-op
- packages/cli/src/commands/claude-hook-ingest.test.ts > claude-hook-ingest command > falls back to direct ingest when HTTP path fails
- packages/cli/src/commands/claude-hook-ingest.test.ts > claude-hook-ingest command > registers expected options and help text
- packages/cli/src/commands/claude-hook-ingest.test.ts > claude-hook-ingest command > returns HTTP result when viewer ingest succeeds
- packages/cli/src/commands/claude-hook-inject.contract.test.ts > claude-hook-inject contract fixtures > 'Injection disabled via CODEMEM_INJECT…'
- packages/cli/src/commands/claude-hook-inject.contract.test.ts > claude-hook-inject contract fixtures > 'Local pack throws → HTTP fallback wins'
- packages/cli/src/commands/claude-hook-inject.contract.test.ts > claude-hook-inject contract fixtures > 'PostToolUse payload (no prompt) → con…'
- packages/cli/src/commands/claude-hook-inject.contract.test.ts > claude-hook-inject contract fixtures > 'SessionEnd payload → continue without…'
- packages/cli/src/commands/claude-hook-inject.contract.test.ts > claude-hook-inject contract fixtures > 'SessionStart payload (no prompt) → co…'
- packages/cli/src/commands/claude-hook-inject.contract.test.ts > claude-hook-inject contract fixtures > 'Truncation: pack longer than CODEMEM_…'
- packages/cli/src/commands/claude-hook-inject.contract.test.ts > claude-hook-inject contract fixtures > 'UserPromptSubmit invariance: payload …'
- packages/cli/src/commands/claude-hook-inject.contract.test.ts > claude-hook-inject contract fixtures > 'UserPromptSubmit with empty string pr…'
- packages/cli/src/commands/claude-hook-inject.contract.test.ts > claude-hook-inject contract fixtures > 'UserPromptSubmit with no hook_event_n…'
- packages/cli/src/commands/claude-hook-inject.contract.test.ts > claude-hook-inject contract fixtures > 'UserPromptSubmit with no prompt key →…'
- packages/cli/src/commands/claude-hook-inject.contract.test.ts > claude-hook-inject contract fixtures > 'UserPromptSubmit with non-empty promp…'
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject command > always emits UserPromptSubmit hookEventName even when payload carries a different hook_event_name
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject command > appends [pack truncated] marker when additionalContext exceeds CODEMEM_INJECT_MAX_CHARS
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject command > emits UserPromptSubmit hookEventName when payload omits hook_event_name
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject command > enriches the retrieval query with prior session state and propagates working_set_paths
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject command > falls back to HTTP pack generation when local generation fails
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject command > logs inject.pack.ok with empty=true when no pack is produced
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject command > logs inject.pack.ok with metrics on local pack success
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject command > logs origin=http when local pack fails and http fallback succeeds
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject command > normalizes multi-line prompts before composing the rich query
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject command > registers expected options and help text
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject command > returns continue with local additionalContext when local pack succeeds
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject command > returns continue without additionalContext when CODEMEM_INJECT_CONTEXT disables injection
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject command > returns continue without additionalContext when all generation paths fail
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject command > returns continue without additionalContext when no prompt is present
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject command > returns continue without injection when CODEMEM_PLUGIN_IGNORE is truthy
- packages/cli/src/commands/claude-hook-inject.test.ts > claude-hook-inject command > truncates additionalContext to CODEMEM_INJECT_MAX_CHARS
- packages/cli/src/commands/codex-hook-ingest.test.ts > codex-hook-ingest command > direct enqueue bootstraps fresh databases on demand
- packages/cli/src/commands/codex-hook-ingest.test.ts > codex-hook-ingest command > direct enqueue inserts once and deduplicates event_id
- packages/cli/src/commands/codex-hook-ingest.test.ts > codex-hook-ingest command > direct enqueue skips unsupported hook payloads gracefully
- packages/cli/src/commands/codex-hook-ingest.test.ts > codex-hook-ingest command > direct enqueue starts a new stream sequence at zero to match the store path
- packages/cli/src/commands/codex-hook-ingest.test.ts > codex-hook-ingest command > does not collide repeated timestamp-less payloads
- packages/cli/src/commands/codex-hook-ingest.test.ts > codex-hook-ingest command > drains queued spool entries after HTTP recovers
- packages/cli/src/commands/codex-hook-ingest.test.ts > codex-hook-ingest command > drains queued spool entries via direct fallback when the viewer stays down
- packages/cli/src/commands/codex-hook-ingest.test.ts > codex-hook-ingest command > falls back to direct ingest when HTTP path fails
- packages/cli/src/commands/codex-hook-ingest.test.ts > codex-hook-ingest command > registers expected options and help text
- packages/cli/src/commands/codex-hook-ingest.test.ts > codex-hook-ingest command > returns HTTP result when viewer ingest succeeds
- packages/cli/src/commands/codex-hook-ingest.test.ts > codex-hook-ingest command > spools payloads when HTTP and direct ingest fail
- packages/cli/src/commands/codex-hook-inject.test.ts > codex-hook-inject command > continues when all pack generation paths fail
- packages/cli/src/commands/codex-hook-inject.test.ts > codex-hook-inject command > falls back to HTTP when local pack fails
- packages/cli/src/commands/codex-hook-inject.test.ts > codex-hook-inject command > frames injected memories as reference data rather than instructions
- packages/cli/src/commands/codex-hook-inject.test.ts > codex-hook-inject command > logs Codex injection metrics
- packages/cli/src/commands/codex-hook-inject.test.ts > codex-hook-inject command > preserves the safety frame when CODEMEM_INJECT_MAX_CHARS is tiny
- packages/cli/src/commands/codex-hook-inject.test.ts > codex-hook-inject command > preserves the safety frame when truncating the memory body
- packages/cli/src/commands/codex-hook-inject.test.ts > codex-hook-inject command > registers expected options and help text
- packages/cli/src/commands/codex-hook-inject.test.ts > codex-hook-inject command > respects CODEMEM_INJECT_CONTEXT=0
- packages/cli/src/commands/codex-hook-inject.test.ts > codex-hook-inject command > returns Codex additionalContext when local pack succeeds
- packages/cli/src/commands/codex-hook-inject.test.ts > codex-hook-inject command > returns continue for non-UserPromptSubmit payloads
- packages/cli/src/commands/codex-hook-inject.test.ts > codex-hook-inject command > returns continue without additionalContext for empty prompts
- packages/cli/src/commands/setup-codex.test.ts > buildCodememCodexHookGroups — command base > uses `npx -y codemem` with generous timeouts as the fallback
- packages/cli/src/commands/setup-codex.test.ts > buildCodememCodexHookGroups — command base > uses a direct `codemem` call with short timeouts when on PATH
- packages/cli/src/commands/setup-codex.test.ts > installCodex — fresh CODEX_HOME > writes the MCP block and all four hook events with correct schema
- packages/core/src/secret-scanner.test.ts > loadScannerOptionsFromConfig > drops malformed rule entries silently
- packages/core/src/secret-scanner.test.ts > loadScannerOptionsFromConfig > returns empty options for missing or malformed config
- packages/mcp-server/src/distill.test.ts > memory_distill_candidates MCP tool > falls back to unjudged output when the observer is unavailable
- packages/mcp-server/src/distill.test.ts > memory_distill_candidates MCP tool > judges candidates and drops routine-activity clusters when judge is set
- packages/mcp-server/src/distill.test.ts > memory_distill_candidates MCP tool > mines candidates within the server default project
- packages/mcp-server/src/distill.test.ts > memory_distill_candidates MCP tool > rejects project filters when all-project mining is requested
- packages/mcp-server/src/distill.test.ts > memory_distill_candidates MCP tool > supports all-project mining for user-scoped candidates
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > hides unauthorized scoped IDs and intersects explicit scope filters
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > keeps blank project filters default-scoped for expansion-style direct reads
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > keeps mixed-domain unauthorized scope rows out of MCP direct reads
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > leaves the session project null when no explicit/env project is supplied
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > normalizes blank project inputs to null on memory_remember
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > refuses to forget unauthorized or explicitly filtered-out memories
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > covers direct, observations, index, explain, recent, pack, timeline, and expand surfaces
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > does not capture when MCP retrieval ledger capture is disabled
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > does not let an explicit failed status override delivered memories
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > does not let completed active siblings consume or erase a pending failure
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > includes canonical call content when a transport session reuses a request ID
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > keeps MCP delivery fail-open when ledger tables are unavailable
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > keeps concurrent identical successful invocations distinct
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > keeps duplicate processing of one runtime invocation idempotent
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > keeps filter resolution failures inside the MCP fail-open boundary
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > keeps partial memory_explain errors nonfatal when a memory is delivered
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > keeps successful MCP results fail-open when identity bookkeeping throws
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > memory_expand still applies the default project when project is omitted
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > memory_expand with explicit blank project returns cross-project anchors (B2 regression)
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > memory_forget removes an ID outside the server default project (B1 regression)
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > memory_get returns an ID outside the server default project (B1 regression)
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > memory_get still honors an explicit project filter on direct-ID lookups
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > memory_get_observations returns IDs outside the server default project (B1 regression)
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > preserves ordinary error content as a failed retrieval
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > reconciles a concurrent failure after its successful sibling completes
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > reconciles a failed MCP request retry into one persisted no-results completion
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > reconciles a failed MCP request retry into the persisted handed-off success
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > reconciles an exact failed retry when no results are encoded as not_found
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > reconciles two concurrent failures one-for-one with sequential successes
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > records a valid empty memory_explain result as no-results
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > records an explicit empty failed status independently from response rendering
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > records explicit MCP surfaces, effective filters, returned IDs, and delivery
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > records invalid-only memory_explain ids as failed
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > records missing memory_explain input as failed without changing its structured payload
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > records repeated completed calls with identical IDs and arguments as new attempts
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > records repeated completed no-results calls as new attempts
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > records retrieval failures without changing the MCP error response
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > registered MCP tool scope behavior (regression for #1119 reviewer P1s) > records successful empty search, recent, and timeline calls as undelivered no-results
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > rolls back remembered MCP memories that resolve to unauthorized scopes
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > stamps remembered MCP memories with the resolved project scope
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > surfaces unauthorized_scope as a stable error contract through memory_remember
- packages/mcp-server/src/memory-access.test.ts > MCP memory access scope guards > uses the env project for memory_remember when no explicit project is supplied
- packages/mcp-server/src/server.test.ts > createCodememMcpServer > exports a side-effect-free factory from the package root
- packages/mcp-server/src/server.test.ts > createCodememMcpServer > registers the full MCP memory tool surface
- packages/mcp-server/src/stdio-viewer.test.ts > MCP viewer ensure > does not spawn when the viewer is healthy
- packages/mcp-server/src/stdio-viewer.test.ts > MCP viewer ensure > respects the CODEMEM_VIEWER opt-out
- packages/mcp-server/src/stdio-viewer.test.ts > MCP viewer ensure > respects the CODEMEM_VIEWER_AUTO opt-out
- packages/mcp-server/src/stdio-viewer.test.ts > MCP viewer ensure > spawns detached with preserved arguments and polls five times
- packages/mcp-server/src/stdio-viewer.test.ts > MCP viewer ensure > stops polling as soon as the spawned viewer becomes healthy
- packages/mcp-server/src/stdio-viewer.test.ts > MCP viewer ensure > swallows spawn failures
- packages/mcp-server/src/stdio-viewer.test.ts > MCP viewer probe > accepts a live degraded viewer
- packages/mcp-server/src/stdio-viewer.test.ts > MCP viewer probe > accepts a live healthy viewer
- packages/mcp-server/src/stdio-viewer.test.ts > MCP viewer probe > falls back to stats exactly once when health is absent
- packages/mcp-server/src/stdio-viewer.test.ts > MCP viewer probe > rejects a stats fallback with invalid viewer_pid
- packages/mcp-server/src/stdio-viewer.test.ts > MCP viewer probe > rejects a stats fallback with malformed JSON
- packages/mcp-server/src/stdio-viewer.test.ts > MCP viewer probe > rejects a stats fallback with missing viewer_pid
- packages/mcp-server/src/stdio-viewer.test.ts > MCP viewer probe > rejects a stats fallback with server error
- packages/mcp-server/src/stdio-viewer.test.ts > MCP viewer probe > rejects malformed JSON without compatibility fallback
- packages/mcp-server/src/stdio-viewer.test.ts > MCP viewer probe > rejects network failure without compatibility fallback
- packages/mcp-server/src/stdio-viewer.test.ts > MCP viewer probe > rejects server error without compatibility fallback
- packages/mcp-server/src/stdio-viewer.test.ts > MCP viewer probe > rejects timeout without compatibility fallback
- packages/mcp-server/src/stdio-viewer.test.ts > MCP viewer probe > rejects wrong service without compatibility fallback
