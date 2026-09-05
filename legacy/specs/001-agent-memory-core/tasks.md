# Tasks: Agent Memory Continuity Platform — Core 1.0

**Input**: [plan.md](plan.md) / [spec.md](spec.md) / 正本 = `agent-memory-final-spec-v6.md`（v6.1）§29–30

**方針**: Phase 0A（= PR 1）のみ完全詳細化。Phase 0B〜8 は見出し + Exit gate 参照のみとし、
各 Phase 着手時に本ファイルへ詳細タスクを追記する（plan.md「多段フェーズはターンをまたぐ」）。
テストタスクは生成しない（Exit gate 自体が検証タスクとして列挙されるため）。

**実行体制**: ブランチ `phase-0a-evidence` + worktree 隔離。調査・量産系は Codex へ委譲、
auth path の fatal/non-fatal 判定・action plan・base ADR 決定は Claude Code（セキュリティ関連 + 最終判断）。

---

## Phase 0A — Evidence Freeze / Base Bake-off（v6.1 §29 Phase 0A / §30 PR 1）

**Goal**: 候補 3 リポの証拠を凍結し、§4.3 base gate で base ADR を確定する。機能変更ゼロ。

**Independent Test**: spec.md SC-0A — base ADR 確定（inventory 完了 + fatal/non-fatal 分類 +
delta 比較記録）、clean install、unsafe path action plan。

### Setup

- [X] T001 ブランチ `phase-0a-evidence` を作成し worktree を `~/projects/free-mem-wt/phase-0a-evidence` に隔離（`git worktree add`）
- [X] T002 [P] 候補 3 リポを sibling dir へローカル clone し pin commit を checkout（`~/projects/free-mem-vendor/codemem` @ `26438e75` / `~/projects/free-mem-vendor/ai-memory` @ `a9e9a24d` / `~/projects/free-mem-vendor/remem` @ `cde8bc05`。公開 fork 禁止・clone のみ）
- [X] T003 worktree に `evidence/` ディレクトリと `evidence/README.md`（成果物一覧の索引）を作成

### Evidence Freeze（各候補独立 → 並列可・Codex 委譲）

- [X] T004 [P] codemem: exact toolchain（node/npm/OS）記録 + upstream check/test 実行、ログを `evidence/codemem/upstream-test.log` へ保存。license / SBOM / native asset 一覧を `evidence/codemem/sbom.md` へ
- [X] T005 [P] ai-memory: 同上を `evidence/ai-memory/` へ
- [X] T006 [P] remem: 同上を `evidence/remem/` へ
- [X] T007 codemem: 静的 inventory — 全 DB open 箇所 / write-capable handle / provider auth・backend / sync・sharing import を file:line 付きで `evidence/codemem/write-handle-inventory.md` へ（v6.1 §4.3 の必須成果物）
- [X] T008 [P] ai-memory・remem: 同観点の簡易 inventory（delta 比較に必要な粒度）を各 `evidence/<repo>/inventory-summary.md` へ
- [X] T009 codemem: observer runtime audit と現行 benchmark runner の有無・実行可否を確認し `evidence/codemem/runtime-audit.md` へ

### 判定（セキュリティ関連 = Claude Code 自ら）

- [X] T010 T007 の inventory を fatal / non-fatal に分類（v6.1 §4.3 基準: unsafe auth path・undocumented provider loader・direct DB fallback 等）。結果を `evidence/codemem/write-handle-classification.md` へ
- [X] T011 fork/vendor/greenfield delta を「残る write handle 数・壊す test 数・移植資産数・unsafe auth path 数」で比較し `evidence/delta-comparison.md` へ
- [X] T012 unsafe path action plan（実 remove は Phase 1）を `evidence/unsafe-path-action-plan.md` へ
- [X] T013 base ADR を `evidence/adr-001-base.md` に確定（§4.3 gate 判定。不合格時: MIT 資産選択移植の比較検討を同 ADR 内に記録し plan.md を改訂）

### 取り込み（ADR = codemem 継続の場合のみ）

- [X] T014 pinned snapshot を `vendor/codemem/` へ取り込み（ローカル clone から。`.git` を含めない snapshot コピー + 出所記録 `vendor/codemem/VENDOR.md`）
- [X] T015 clean install 検証（vendor snapshot からの素の install 手順が通ること）。手順と結果を `evidence/clean-install.md` へ
- [X] T016 Exit gate 照合（SC-0A 全項目）→ speckit-verify-tasks → 2 本立てレビュー（/code-review → ponytail-review）→ main へマージ

**Checkpoint**: SC-0A 達成 = Phase 0B へ。ここまで product コード差分ゼロ。

---

## Phase 0B — Adapter / Sidecar Contract Harness（v6.1 §29 Phase 0B / §30 PR 2。2026-08-12 着手時詳細化）

**Goal**: Claude Code / Codex の exact stable binary で hook lifecycle を fixture 化し、§7.2 capability schema による version-pinned golden matrix と sidecar certification 可否判定を得る。product DB 変更なし。Tier A 宣言なし（未観測 cell は `unknown` — HI-23。全 cell 充足は Exit 要件ではない）。

**実行体制**: ブランチ `phase-0b-harness` + worktree。schema/組立コード = Grok 委譲（範囲宣言可: `harness/`）。capture 実行・隔離 rig・sidecar hostile harness・certification 判定 = Claude Code 自ら（セキュリティ関連: credential/config 隔離・process 隔離）。ToS 確認 = researcher。

- [X] T017 ブランチ `phase-0b-harness` + worktree 作成
- [X] T018 [P] harness scaffold: §7.2 の capability schema（TS 型 + JSON Schema）、fixture 記録形式、matrix assembler CLI を `harness/` に実装（委譲可。product コード・vendor/ に触れない独立 package）
- [X] T019 隔離 capture rig: scratch `CLAUDE_CONFIG_DIR`/HOME + 使い捨て repo + capture 専用 hooks のみ + `AGENT_MEMORY_INTERNAL_RUN=1` marker。ユーザー実環境の plugin/hook/メモリ DB を汚染しない構成を `harness/rig/` に作る（Claude Code 自ら）
- [X] T020 Claude Code capture: exact version 記録（capture 時点で毎 fixture に pin）+ hook lifecycle / timeout / first injection / compact / tool failure phase / interrupt / subagent を fixture 化 → `harness/fixtures/claude/`（実 CLI 実行 = Claude Code 自ら。子セッションは最小 prompt・拡張思考なし）
- [X] T021 Codex capture: 同上 → `harness/fixtures/codex/`
- [X] T022 golden matrix 組立: fixtures → §7.2 AdapterCapabilities（unknown 既定・coverage/limitations 明記・evidenceKind=real-cli-e2e）を version-pin 付きで `harness/matrix/` へ（組立コード委譲可、データ確定は Claude Code）
- [X] T023 [P] Codex sidecar の provider ToS / documented-permission 一次ソース確認（§13.6 manifest 必須欄。OpenAI ToS + Codex CLI docs。researcher 委譲）。結果不明瞭 = 未認定でよい
- [X] T024 sidecar hostile harness: 不活性 hostile fixture（marker file を書くだけの偽 hook/plugin/AGENTS）+ process-group kill / pipe close / FD・descendant 残存検査を stub subprocess に対して実証 → `harness/sidecar/`（Claude Code 自ら）。Claude sidecar は `ANTHROPIC_API_KEY` 不在のため hostile E2E 不能 = **未認定（default disabled）が正当な判定**
- [X] T025 certification 可否判定を §13.6 manifest 形式で `harness/sidecar/certification-decision.md` に記録（Claude / Codex 各 verdict。否も正当な Exit）
- [X] T026 Exit gate 照合（SC-0B）→ speckit-verify-tasks → 2 本立てレビュー → main へマージ

## Phase 1 — Safety Boundary / Sole Writer（2026-08-12 着手時詳細化）

**Goal**: vendor snapshot から fatal 13 経路（Phase 0A の 10 + 計画時発見 3）を物理削除し、
daemon 唯一 writer 化（HI-4）を blocking 検証付きで確立。spool 契約・peer auth・redaction 2 層・
install ownership manifest・backup baseline を導入。Exit: SC-1。v6.1 §29 Phase 1 + §5。

**設計判断の正本**: [phase-1-design.md](phase-1-design.md)（「判断 #N」は同ファイルの表を指す。ADR-002 = peer auth 同梱）

**実行体制**: ブランチ `phase-1-safety` + worktree。**セキュリティ中核 = Claude Code 自ら（従来どおり・委譲禁止）**:
auth 削除 T030/T031、daemon 書込み境界 T033–T048、redaction T038、peer/viewer auth T037/T043、cutover T051、
manifest/backup 設計判断部。**列挙した機械的作業のみ委譲可（2026-08-12 ユーザー指示「週次制限が近い、委譲できるものは委譲」）**:
T028 一括削除の実行（対象リスト確定後。UI 画面・frontend API client の削除は frontend 例外につき Claude Code 自ら）/
T028–T029 収集・集計（test snapshot・retire リスト初稿・inventory 照合下書き）/
T053–T057 の harness runner 骨格（assert 内容と判定条件は Claude Code 確定・検収）。委譲回は 2 本立てレビュー必須。

### Setup

- [X] T027 Spec Kit 事前確認（read-only `specify self check` + integration status）→ ブランチ `phase-1-safety` + worktree 作成 → repo 変更系 upgrade は worktree 内（local 変更時 force 禁止）。実施記録: 0.16.2 最新・integration OK・upgrade 不要。worktree = `~/projects/free-mem-wt/phase-1-safety`

### A7 carve-out

- [X] T028 事前 snapshot（vitest 完全修飾名 → `evidence/phase1-test-baseline-pre.txt`）→ A7 物理削除: sync 全面 + coordinator 全面（cloudflare-coordinator-worker・D1 migrations 13 本）+ sharing/recipient-policy/scope-membership-cache + viewer sync routes + MCP HTTP transport 全体 + UI 側 sync/coordinator/sharing 画面・API client・設定。検証: CLI help 清浄 / viewer sync API 404 / pnpm build + 実 viewer smoke / tsc green
- [X] T029 A7 成果物化 + 実装開始条件の disposition 確定: (a) `evidence/phase1-test-retire-list.md`（retire 完全修飾名 + 理由 + failure signature + 再現コマンド + 追加予定 test の事前登録 manifest）(b) disposition 表 — inventory 全数 + 発見 3 経路 → {削除 / RPC 化（endpoint・schema・daemon 不在時挙動 1 対 1）/ daemon 内 jobs / typed 無効化} + mutation class 列 {A/B/C}（class B = endpoint ごと operation ID 生成主体・payload hash conflict・journal 状態遷移・結果再取得。class C = job ID 照会 or 自動再試行禁止）+ surface authority 列（agent-callable / user-authority。`memory_forget`・confirm/pin/unpin/retract/mark_wrong・destructive bulk = MCP から物理削除 or typed-disable、HI-30/31・§18.5）。**T033 以降の開始条件**。実装後に新設 surface（lock.db・backup verify・staging・viewer 認証）含め再生成し T053 が完全一致検証 (c) classification 補遺（凍結 evidence 非改変・日付付き追記、3 経路の file:line）

### A1–A4 / A6（T030→T031 直列）

- [X] T030 A1+A2+A3: `_callAnthropicConsumer`(2031-2073)+dispatch、`_callCodexConsumer`(1992-2030)+dispatch、`buildCodexHeaders`、`observer-auth.ts:49-183`+`:185-206` 削除。cascade = `explicit -> env -> file`。検証: `anthropic`×`oauth` 到達ゼロ / 第三者 credential path ゼロ / schema 残キーなし / tsc green
- [X] T031 A4 両 sidecar: Claude 側（2074-2255 + dispatch 1889）+ Codex 側（2256-2590 の 3 関数 + dispatch 1894 + 分岐 1381/1558-1559/1590/1638）削除。0B certification 両者 default disabled が根拠。**削除のみ、再実装は Phase 6**。検証: `bypassPermissions` ゼロ / sidecar 到達ゼロ / tsc green
- [X] T032 [P] A6: bootstrap template 削除。検証: 参照ゼロ

### daemon 本体

- [X] T033 writer actor + migration 分離（判断 #15）+ audited wrapper 一本化 + trace 内蔵 + storage layout（判断 #16、platform interface module 含む）+ legacy migration runner 実装（実行は T051）。raw Database 非 export。前提: T029
  - 実装証拠: `vendor/codemem/packages/core/src/writer-actor.ts` (`WriterActor` / `ReadOnlyActor`)、`vendor/codemem/packages/core/src/migration-runner.ts` (`runDatabaseMigrations`)、`vendor/codemem/packages/core/src/storage.ts` (`runLegacyMigration` / `recoverStorageJournal`)、`vendor/codemem/packages/core/src/storage-platform.ts`、`vendor/codemem/packages/core/src/daemon-foundation.test.ts` (P1-T033-01..04)
- [X] T034 daemon lifecycle: entrypoint + lock.db BEGIN IMMEDIATE writer lock + force-kill identity 契約（判断 #14）+ clean shutdown/force-kill fallback + data_dir 単位 + Unix socket（0700/0600）+ health + data_dir preflight（network fs / WSL⇔Windows 共有 path 拒否）
  - 実装証拠: `vendor/codemem/packages/core/src/daemon-lifecycle.ts`、`vendor/codemem/packages/core/src/storage-platform.ts`（preflight / process identity）、`vendor/codemem/packages/core/src/daemon-lifecycle.test.ts` (P1-T034-01..04)
- [X] T035 RPC 基盤: socket server + §7.5 handshake + schema allowlist + §8.7 size bound + typed error + §19.6 version 管理 + RPC hard deadline + health/doctor/backup endpoint（T050 が使う最小面）。viewer createApp seam 再利用
  - 実装証拠: `vendor/codemem/packages/core/src/daemon-rpc.ts`、`vendor/codemem/packages/core/src/daemon-rpc.test.ts` (P1-T035-01..04)。local API と normalized event schema は独立 version。`/v1/backup/create|verify` は allowlist 済みの最小面（本体は T050）
- [X] T036 mutation dispatcher 有効化: receipt schema migration（backup verify 後に 1 回）→ 共有 dispatcher（class A、判断 #11）+ 残り全 endpoint（events, events/batch, context/pack, search, memories/:id, memories/record, checkpoints + export/import + maintenance trigger + viewer read 系）。前提: T050
  - 実装証拠: `vendor/codemem/packages/core/src/mutation-dispatcher.ts` + `vendor/codemem/packages/core/src/daemon-canonical.ts`、RPC allowlist 拡張、`vendor/codemem/packages/core/src/mutation-dispatcher.test.ts` (P1-T036-01..08、05b scope visibility)。class B/C（export/import/restore/jobs 本体）は allowlist + `not_implemented`（T045/T047）。viewer 個別 query は `GET /v1/view` collection allowlist + 既存 ownership/scope filter
- [X] T037 peer auth = ADR-002 実装（判断 #2）。前提: T035
  - 実装証拠: `mapPeerConnectError`（EACCES→peer_denied / ECONNREFUSED→daemon_unavailable）、`daemon-peer.test.ts` (P1-T037-01..02)。別 UID 実接続は permission bits で代替
- [X] T038 redaction 2 層: (a) adapter 共通前処理 lib（allowlist→size→stripping→normalization→secret redaction→sensitivity→private/local-only を RPC/spool 前必須。判断 #9 実行契約 + private tag fail-closed grammar（nested/unclosed/複数 field fixture）+ 日本語誤検出 fixture）(b) daemon intake 第 2 層（両ルート）。`secret_rules_version`（degraded 込み）/ plaintext log 禁止 / secret body 非永続化 / `.agent-memory.toml`（未認識キー警告・型エラー制限側・privacy fail-closed / 処理 fail-open）。前提: T035
  - 実装証拠: `vendor/codemem/packages/core/src/redaction-pipeline.ts`、`vendor/codemem/packages/core/src/redaction-pipeline.test.ts` (P1-T038-01..06)、`vendor/codemem/packages/core/src/mutation-dispatcher.test.ts` (P1-T038-07 daemon→DB 非漏えい)。killable worker は `ponytail:` 残件（判断 #9）
- [X] T039 spool 契約: 固定パス（control 配下）+ idempotencyKey spool 前確定 + quota（判断 #4）+ 80% 警告 + counter + 予約枠 + 並行 writer・tmp 残骸・disk full・両枠満杯 fixture + 旧形式残量 drain + spool lock deadline。前提: T038, T050
  - 実装証拠: `vendor/codemem/packages/core/src/spool.ts`（`control/spool/{tmp,ready,quarantine}`、tmp write+fsync+rename、adapter redaction metadata保持、通常128MiB/予約16MiB/隔離32MiB、固定4KiB counter、PID/start identity + inode照合付き100ms exclusive-file lock、旧 Claude/Codex spool drain）、`vendor/codemem/packages/core/src/normalized-event.ts`（RPC/spool 共通 allowlist・schema validation）、`vendor/codemem/packages/core/src/spool.test.ts` (P1-T039-01..04)。health/doctor は80%・drop・quarantine critical を表示。import/commit/delete は T040
- [X] T040 spool importer: startup + 定期 sweeper、commit-before-delete、T036 dispatcher 経由 receipt。同一 key 異 payload = conflict → quarantine。**完了条件は synthetic spool producer による importer 単体検証まで**（実 surface を使う daemon 停止 exactly-once 検証は T055(a)）。前提: T036, T039, T050
  - 実装証拠: `vendor/codemem/packages/core/src/spool.ts`（canonical entry/hash/filename/schema 再検証、valid tmp 回復、broken/conflict quarantine、quarantine 満杯・handler失敗時 ready 保持、commit成功後のみfsync付き削除）、`vendor/codemem/packages/core/src/daemon-rpc.ts`（RPC と importer が `handleEvent` / `handleRemember` + T036 dispatcher を共有し、adapter redaction metadataをdaemon第2層へ統合）、`vendor/codemem/packages/core/src/daemon-lifecycle.ts`（起動時 sweep + 1秒定期 sweep + shutdown timer解放）、`vendor/codemem/packages/core/src/spool-importer.test.ts` (P1-T040-01..04 synthetic producer、tmp recovery、commit-before-delete、startup conflict/broken/tamper quarantine、periodic event、secret/private/local-only direct replay一致)

### A5 + thin client 化

- [X] T041 A5: directEnqueue×2 → spool のみ化、flushBoundaryRawEvents 除去、claude/codex inject buildLocalPack + file-context → RPC read + event 投入。全経路 T038(a) + Agent 別 deadline/spool 予約 + 外側 watchdog（判断 #3）。実 hook timeout 直前保全テスト。前提: T037, T038, T040
  - 実装証拠: `vendor/codemem/packages/cli/src/commands/hook-rpc-client.ts`（共通 normalize/redaction、daemon RPC、deadline、shared spool）、`vendor/codemem/packages/cli/src/hook-runtime.ts` + `vite.hook.config.ts`（256KiB bounded standalone runtime）、Claude/Codex ingest・inject・file-context thin client、`vendor/codemem/packages/core/src/daemon-rpc.ts`（file-context retrieval ledger + SessionStart timestamp）、`vendor/codemem/packages/cli/src/commands/setup.ts`（legacy 2-hook migration + bundled runtime）。旧 agent 専用 spool module と direct DB/HTTP/boundary flush 経路は削除
  - 検証証拠: `hook-thin-client.test.ts` (P1-T041-01..04、pre-RPC/spool/session-state policy、tool allow/deny、ignore/local-only/apply_patch、実 1500/1000ms cutoff + 500ms reserve)、`daemon-rpc.test.ts` (P1-T041-04..05)、`hook-runtime.test.ts`、packed artifact の install×2 idempotency。workspace build / tsc / lint / full Vitest / plugin / smoke green
- [X] T042 [P] MCP stdio → RPC client 化。Phase 1 surface = read + remember + status 最小 allowlist。user-authority mutation（memory_forget 等）は物理削除 or typed-disable + tool-list 検査 + 拒否試験（正式 surface = Phase 5）。前提: T036, T037, T038, T039/T040（remember の spool fail-over 経路）
  - 実装証拠: `vendor/codemem/packages/mcp-server/src/rpc-client.ts`（固定 endpoint allowlist、project policy pre-RPC redaction、typed read failure、remember RPC→shared spool）、`server.ts` / `stdio.ts` / `tools/*.ts`（DB handle・viewer autostart・user-authority mutation・learn/distill を除去）、`vendor/codemem/packages/core/src/daemon-rpc.ts`（filtered get-many）
  - 検証証拠: `rpc-client.test.ts`（daemon 不在 read、direct redaction、P1-T042-03 spool exactly-once）と `server.test.ts`（P1-T042-01/02/04、全 read endpoint/mode）。workspace build / tsc / lint / full Vitest / plugin / smoke green
- [X] T043 [P] viewer read-only 化 = 判断 #12 契約。blocking test: 未認証 401 / 誤 token 401 / 悪意 Origin 403 / nonce 再利用・期限切れ・同時交換 race / TTL 失効・daemon 再起動失効・logout・session 上限 evict・history.replaceState・Referrer-Policy / 別 UID 拒否（可用時）。前提: T036, T037, T038
  - 実装証拠: daemon-owned `control/token`（256-bit・0600）、single-use nonce、HMAC session（TTL/logout/restart/上限 eviction）、`GET /v1/view` read relay。viewer は origin/port 単位の `sessionStorage` + `Authorization: Session`、Origin/CSP/no-store を強制し、host-scoped cookie・DB handle・mutation/ingest/config-write route・remote script/font を削除。CLI は nonce を URL fragment で渡し、fetch 前に `history.replaceState` で除去
  - 検証証拠: `viewer-auth.test.ts` / `daemon-rpc.test.ts` / `viewer-server/src/auth.test.ts` / `ui/src/lib/api/internal.test.ts` の `P1-T043-01..13`。実 Chromium で旧 cookie の別 loopback port 送信を再現し、回帰 test で cookie 非発行・cookie-only 401・Session auth・fragment 除去・data API・read-only settings を確認。別 UID live 実行は権限不足のため、0700/0600/EACCES の既存 peer test を fallback とした。serial full suite 396 suites / 1,894 tests / failed 0
- [X] T044 CLI: production 分 RPC 化 + 後続 Phase 機能 typed stub（判断 #13）。前提: T036, T037, T038, T039/T040（spoolable mutation の fail-over 経路）
  - 実装証拠: `vendor/codemem/packages/cli/src/commands/{enqueue-raw-event,memory,pack,recent,search,stats,status,serve}.ts` が共有 `@codemem/mcp` RPC client を使用。spoolable event/remember は redacted spool、forget/read は typed failure。distill/embed/extraction replay/benchmark は Phase 6/7 typed stub、旧 prompt-pack-ledger は削除
  - 検証証拠: `cli-rpc.test.ts` (P1-T044-01..03)、`rpc-client.test.ts` の direct event redaction、`evidence/phase1-t044-cli-rpc-validation.md`。serial full suite 390 suites / 1,836 tests / failed 0、test 集合は `1,894 - 61 + 3 = 1,836`
- [X] T045 maintenance/backfill → daemon 内 jobs 移設。前提: T033
  - 実装証拠: `vendor/codemem/packages/core/src/daemon-jobs.ts` / `daemon-jobs-schema.ts`（durable `daemon_jobs`、全 class C registry、`maxAttempts=1`、再起動 orphan fail、内部 backfill 吸収）、`daemon-rpc.ts`（POST/GET jobs）、CLI `commands/daemon-job.ts`（POST 一度だけ + GET polling）。旧 `maintenance-worker-runtime` と worker PID 管理は削除
  - 検証証拠: `daemon-rpc.test.ts` (P1-T045-01/03)、`daemon-jobs.test.ts` (P1-T045-02)、`cli-rpc.test.ts` (P1-T044-03 拡張)、`evidence/phase1-t045-t046-daemon-jobs-validation.md`
- [X] T046 破壊的 maintenance = daemon maintenance mode（判断 #6）。前提: T045, T050
  - 実装証拠: destructive job は sweeper/spool import/通常 write を停止し、T050 online backup の作成・検証成功後だけ mutation を開始。backup/verify 失敗時は job failed・DB unchanged、maintenance response は retryable で spoolable client が共有 spool へ保全
  - 検証証拠: `daemon-jobs.test.ts` (P1-T046-01)、`mcp-server/src/rpc-client.test.ts` (P1-T046-02)。serial full suite 390 suites / 1,833 tests / failed 0
- [X] T047 [P] export/import → daemon RPC（destructive import = maintenance mode + backup precondition）。前提: T036, T046, T050
  - 実装証拠: `vendor/codemem/packages/core/src/daemon-operations.ts`（canonical payload hash、fsync journal、atomic export、restart result、verified backup-before-import）、`daemon-rpc.ts` / `daemon-lifecycle.ts`（RPC + daemon ownership）、CLI `commands/{daemon-operation,export-memories,import-memories}.ts`（POST 一度 + GET polling、stdin/stdout temp compatibility、DB fallback なし）
  - 検証証拠: `daemon-operations.test.ts` (P1-T047-01..03)、`alias-deprecation.test.ts`（実 daemon CLI compatibility）、`evidence/phase1-t047-operations-validation.md`。serial full suite 392 suites / 1,835 tests / failed 0
- [X] T048 daemon 外 DB handle 完全ゼロ（判断 #7）: 全 read → RPC read client、connectReadOnly 残 2 箇所も RPC 移行。前提: T041–T047（全 client/jobs 移設後にのみ判定可能）
  - 実装証拠: `vendor/codemem/packages/core/src/{index,store,test-utils,sole-writer-boundary.test}.ts`。production の `connect` / `connectReadOnly` / `MemoryStore` / actor / raw SQLite opener は daemon 所有実装だけに限定し、public runtime export と path 自己 open wrapper・旧 runner class を削除。test opener は exact `test-utils.ts` のみ
  - 検証証拠: `P1-T048-01-zero-external-db-handles`、focused 156 tests、forced tsc、lint、serial full 393 suites / total 1,828 / passed 1,825 / todo 3 / failed 0。`evidence/phase1-t048-zero-external-db-handles-validation.md`

### install / backup / cutover

- [X] T049 [P] install ownership manifest（判断 #10、roundtrip 検証込み）。前提: T034
  - 実装証拠: `vendor/codemem/packages/core/src/install-manifest.ts`、CLI `commands/setup.ts`、`install-manifest.test.ts` (P1-T049-01)。source checkout の共通 built CLI/hookを事前検査し、manifestはbuilt CLIと選択editorのconfig/installed runtimeをfingerprintする。OpenCodeはwrapper/plugin sourceも記録。laneごとのatomic config write・manifest commit・rollbackを実装し、packed Codex-only laneは非選択OpenCode artifactを要求しない
- [X] T050 pre-migration backup 機構: `db.backup()` 実測（不在なら §20.2 ADR 後実装）→ online backup + verify を migration（判断 #15）・破壊的操作（T046/T047）の precondition に配線。失敗 = 破壊操作開始しない（HI-25）。前提: T034, T035
  - 実装証拠: `vendor/codemem/packages/core/src/online-backup.ts`（`createOnlineBackup` / `verifyOnlineBackup` / `runGatedMigration` / `requireVerifiedBackup`）、`vendor/codemem/packages/core/src/online-backup.test.ts` (P1-T050-01..07)。v18→receipt schema migration も online backup 後だけ実行。binding は `WriterActor.backup()` を露出済みのため §20.2 ADR 不要。T052 の manifest 全欄・retention・CLI・restore は対象外
- [X] T051 legacy layout cutover: 旧配置 DB への EXCLUSIVE 排他 handoff（旧 codemem process の検出・停止要求。取得不能 = cutover 開始せず spool + 診断）。WAL では lock だけで接続消滅を証明できないため、journal prepared 前と committed／lock 解放直前の 2 点で「旧 DB の open-handle owner set = {cutover daemon} のみ」を機械検証（proc-fd/lsof 走査、0B rig の FD 検査再利用）。旧 process 終了未確認 or 残存 handle = fail-closed で cutover 中止 → 旧 DB online backup + verify → staging 構築 → journal 付き current 公開。lock は journal committed まで保持。解放前に managed hook/MCP/CLI の実行先 fingerprint が新 thin client であることを検証（取得元 = T049 manifest の managed-block 記録）。旧 path には tombstone fence（旧 binary の commit 後再起動が canonical DB 書込みにも別 DB 新規生成にも成功しない契約 = split-brain 防止）。**受入条件の順序固定: tombstone atomic 設置・parent fsync → 最終 owner-set 検査 → 旧 handle close・lock 解放**。旧 spool drain 込み。前提: T048, T049, T050
  - 実装証拠: `vendor/codemem/packages/core/src/{legacy-cutover,daemon-lifecycle,install-manifest}.ts`、CLI `commands/setup.ts`。既存 T050 online backup と判断 #16 storage journal を再利用し、`/proc` + trusted `lsof` owner set、setup target hash、recovery hardlink、atomic tombstone、legacy spool durable handoffを実装
  - 検証証拠: `legacy-cutover.test.ts` (P1-T051-01..04)、forced tsc、lint、workspace build、serial full 395 suites / total 1,833 / passed 1,830 / todo 3 / failed 0、`evidence/phase1-t051-legacy-cutover-validation.md`
- [X] T052 backup baseline 完成: §20.2 manifest 全欄 + hash + manifest は完成 artifact を read-only で開いて生成（backup 中の継続 write 試験で manifest ↔ 復元 rows 一致を blocking 判定）+ trigger 全種（migration/update/repair/import-merge 前 + daily）+ retention 7d/4w + owner-only + create/list/verify/restore サブコマンド（CLI 名現行維持）+ restore = 判断 #16 の journal 方式 + privacy 表示（backup は private/local-only を含み得る旨。off-device/export は Phase 1 非提供と明示）+ 署名 ADR（hash-only 期間の脅威・鍵管理・Phase 8 blocking task 引継ぎ）。前提: T046, T048, T050, T051
  - 実装証拠: `vendor/codemem/packages/core/src/{online-backup,daemon-canonical,daemon-jobs,daemon-lifecycle,daemon-rpc}.ts`、CLI `commands/backup.ts`、`docs/adr/0002-backup-manifest-authenticity.md`。完成 snapshot の read-only manifest、7 daily + 4 weekly、canonical DB/backup owner-only、migration・全 user-authority maintenance/repair・import・daily trigger、journal restore、FTS rebuild/vector degraded path、local-only privacy copy、hash-only authenticity の Phase 8 blocking 制約を実装。MCP adapter の custom secret rule も backup reason の永続化前に適用
  - 検証証拠: `online-backup.test.ts` (P1-T052-01..03)、CLI `backup.test.ts` (P1-T052-04)、forced tsc、lint、workspace build、serial full 397 suites / total 1,838 / passed 1,835 / todo 3 / failed 0、`evidence/phase1-t052-backup-restore-validation.md`

### Exit gate（SC-1 = §29 Phase 1 Exit。**T028–T052 全完了 barrier 後**）

- [X] T053 Exit-1a static scan: linter restricted-import（better-sqlite3/MemoryStore/connect( を daemon 外禁止、test 例外 exact path）+ 専用 scan（disposition 表全数照合 + deep/alias import + new Database( + DDL 文字列 + 旧 direct path・両 sidecar 残存 + wrapper bypass。readonly 例外なし）。違反 = 非ゼロ終了。`harness/` 配置
  - 実装証拠: `harness/phase1-static-scan.ts`（TypeScript AST、production 276 files、opener/DDL exact allowlist、T048 5 rows + RPC 27 methods disposition照合）、`vendor/codemem/biome.json`（restricted imports + exact test/daemon exceptions）、既存 `P1-T048-01-zero-external-db-handles` を同一 harness へ統合
  - 検証証拠: normal scan 0 violations、injected AST self-test、Biome negative fixture `noRestrictedImports` exit 1、tsc、lint 388 files、focused boundary test pass、`evidence/phase1-t053-static-scan-validation.md`
- [X] T054 Exit-1b runtime DB-open trace: process matrix（daemon up/down/maintenance/backup/schema migration/legacy cutover/restore × hook ingest/inject/MCP/CLI/viewer/jobs）で状態別 owner set assert（up = {daemonPid} / down = ∅ / maintenance・backup・migration・cutover・restore ⊆ {daemonPid} / 全状態 PID≥2 独立 fail。legacy DB と canonical DB の双方を判定対象に）。wrapper trace / lsof / 停止時 mtime+hash 3 系。**HI-4 blocking**。0B rig 再利用
  - 実装証拠: `harness/phase1-runtime-db-open-trace.mjs`（7 states × 6 independent process surfaces、canonical / legacy inode owner set、`CODEMEM_DB_OPEN_TRACE` + `/proc` + trusted `lsof`、down fingerprint、実 rogue PID negative controls）、`vendor/codemem/package.json`（build 込み gate）。runtime gate が standalone hook bundle の DB actor 混入 / ESM builtin require と timeout peer 復帰時 daemon `EPIPE` を検出したため、pure `storage-layout.ts` と shared RPC socket error handling で根本修正
  - 検証証拠: self-contained build + runtime gate 全7 states pass、focused 5 files / 34 tests、tsc、lint 389 files、static scan 277 production files / 0 violations、static scan self-test、standalone built hook fail-open、`evidence/phase1-t054-runtime-db-open-validation.md`
- [X] T055 Exit-2 fault injection: (a) daemon kill 中 → spool → exactly one commit（実 surface を個別列挙: hook ingest / hook inject 系 event 投入 / MCP remember / spoolable CLI mutation） (b) class A 全 mutation の replay/dup x10・RPC 応答消失→spool→再取込 = 1 commit・commit 直後 kill・delete 失敗 + class B 代表（backup/export）crash・replay (c) spool 各点 fault (d) quota/counter/quarantine 満杯/並行 writer (e) lock race（並行起動・crash 直後再起動・handle 存命・子プロセス非継承）+ force-kill identity（stale record・PID 再利用 = kill 拒否）+ stale socket・起動途中 kill (f) secret + private markup fixture の spool raw bytes/DB/ログ非出現（HI-7）
  - 実装証拠: `harness/phase1-fault-injection.mjs`（4 built surfaces、Class A 9 method x10、backup/export crash、20回 lock race、lifecycle/privacy controls）、`vendor/codemem/packages/core/src/{spool,online-backup,daemon-operations,daemon-lifecycle}.ts`、`vendor/codemem/packages/core/src/spool.test.ts` (`P1-T055-01-spool-fault-boundaries`)、`vendor/codemem/package.json`（build込み gate）
  - 検証証拠: self-contained gate全pass、focused 5 files / 34 tests、tsc、lint 393 files、static scan 279 production files / 0 violations、Semgrep 378 rules / 0 findings、`evidence/phase1-t055-fault-injection-validation.md`
- [X] T056 Exit-3 no Agent blockage: daemon 不在/認証 fail/handshake 不適合/disk full/size 超過/hung daemon/partial response/backpressure/長時間 lock holder/catastrophic regex で hook fail-open + Agent 継続。exit code + 最大実時間（Agent 別 deadline + spool 予約発動）の両 assert。実 hook timeout 直前保全。p95 記録 + doctor 表示
  - 実装証拠: `vendor/codemem/packages/cli/src/{hook-runtime,commands/hook-rpc-client}.ts`（絶対期限 supervisor、bounded RPC/output、spool reserve）、`vendor/codemem/packages/core/src/{redaction-worker,redaction-pipeline,secret-scanner,gitleaks-pinned-rules,spool,daemon-rpc}.ts`（全 rule killable worker、Gitleaks pin、metadata-only degraded + durable doctor）、MCP memory degraded伝播、Codex SessionEnd 5本目、同期済み3 plugin runtime
  - 検証証拠: `harness/phase1-no-agent-blockage.mjs`、`evidence/phase1-t056-no-agent-blockage-validation.md`、13 files / 314 tests、packed artifact、lint、static scan 279 files / 0 violations、artifact SHA-256一致。closure candidate 再実行の最終 p95 は Claude 132.5ms / Codex 131.9ms で 150ms 目標内（過去の nonblocking warning は evidence に保持）
- [X] T057 Exit-4 backup restore smoke（Phase 1 実体限定）: 現行 canonical rows + manifest/hash + fresh-dir + FTS rebuild + degraded（HI-17: vector 不在で FTS-only 独立成功）+ journal 堅牢性: 破損ケース（空 / 部分書込み / checksum 不一致 / 不正状態値）+ durable 境界ごとの個別 fault point（各 journal の tmp write・fsync・rename・parent fsync / staging fsync / pointer rename・parent fsync / reopen・integrity failure / rollback pointer 更新中断 / committed 更新直後 — 各点で期待回復先を固定）+ 旧 writer の backup 中書込み / cutover 前から idle RW handle を保持する旧 process / 旧 binary の commit 後再起動 — 各ケースで回復先一意・canonical loss ゼロ・split-brain DB ゼロを機械 assert、曖昧状態 = 非ゼロ終了。後続 entity（後続 Phase の新テーブル）は各 Phase で追加し Phase 8 で統合検証
  - 実装証拠: `harness/phase1-backup-restore-smoke.mjs`（built CLI/RPC fresh-dir restore、FTS/vector degraded、破損journal、legacy process fencing）、`vendor/codemem/packages/core/src/backup-restore-smoke.test.ts`（21 durable fault境界）、`storage.ts`（rollback pointer一致時のparent fsync再確立）、`vendor/codemem/package.json`（build込みgate）
  - 検証証拠: real-process gate全pass、focused 4 files / 19 tests、forced tsc、lint 393 files、static scan 279 production files / 0 violations、correctness/security + Ponytail二本立てreview、`evidence/phase1-t057-backup-restore-smoke-validation.md`
- [X] T058 SC-1 candidate 照合 + clean checkout gate（cwd = 一時 worktree の `vendor/codemem`、exact node v24.16.0 / corepack pnpm 11.8.0）: frozen-lockfile install → build → check + CLI help + viewer smoke + test 集合機械比較（`4,037 - 2,376 + 193 = 1,854`、CI check `1,851 passed + 3 todo`）+ §32.12 upgrade/rollback コマンド記録 → speckit-verify-tasks（machine 25/31 を維持し manual 31/31 と分離）→ correctness / security review gate（妥当 finding を修正、user Class-C daemon-jobの自動retry concernは契約どおりreject、内部 backfill resumeは別契約）→ ponytail-review → T029/T053 static scan 照合。full gate bundle = `01440d5`（serial full JSON SHA-256 `32f3ceea08f11a9939300dd60ded15dcd26f0d45952caa7ab1f3ba2e6a821b25`）、product-code candidate = `cfbcd6f`（historical validated head `87195f5`、test-only coverage closure `55323dd`、JSON/JSONC root hardening `a263513`、Codex TOML structure scanner `721e3e0`、OpenCode attestation `22dfd34`を含む）。最終Codex comment-preservation deltaは setup 23 / 23、tsc、Biome、build、Semgrep 0 findings、correctness/security/Ponytail blocker-free。PR #7 の code/test head は push run `31920200434` / PR run `31920202238`、Sonar new-code coverage `86.70487106017193%`（A/A/A）、CodeQL、Codacy、Semgrep、secrets を再確認し、review thread unresolved 0。**`main` merge と Sonar long-lived branch移行は、この repository evidence commit 後に行う external step として分離する。** 実行根拠: `evidence/phase1-t058-final-validation.md`

### Phase 1 依存 DAG

```
T027 → T028 → {T029, T030, T032[P]} ; T030 → T031
T029 → T033 → T034 → T035 → {T037, T038}
{T034,T035} → T050 → {T036, T039, T040, T046, T047}
T036 → {T040, T042, T043, T044} ; T038 → T039 → T040
{T037,T038,T040} → T041 ; {T036,T037,T038} → T042[P]/T043[P]/T044
T033 → T045 → T046 ; {T041..T045, T047} → T048
T034 → T049[P] ; {T048, T049, T050} → T051 ; {T046, T048, T050, T051} → T052
T039/T040 → {T042, T044}（spool fail-over 経路）
barrier: {T028..T052 全完了} → {T053..T057} → T058
```

## Repository source publication（Phase 1 Exit と独立、2026-08-14 ユーザー決定）

**Goal**: 未完成であることを明示した source を公開し、Core 1.0 release と誤認させずに継続開発を可視化する。

- [X] T059 [P] public-readiness: root README / security policy / third-party notice / license status、全履歴 secret scan（合成 credential fixture の2 exact path のみ除外）、clean checkout の frozen install → build → check、commit SHA pin 済み CI
- [X] T060 GitHub public repository を作成して現 WIP branch を push。private vulnerability reporting、CI、secret scan、公開 visibility を live 確認。tag / package / release は作成しない。前提: T059

Sonar coverage は source 公開の blocking gate にしない。公開後に Sonar project が自動作成されたため、full-suite 由来 LCOV と CI-based analysis を設定済み。focused test の coverage は全体値として公開しない。

## Runtime barrier — #1 Stage 0（Phase 2 開始条件）

Phase 2 以降の大規模 TS product 実装は、issue #1 Stage 0 の完了まで着手しない。
runtime-neutral な schema / fixture / harness（Phase 3 preflight, issue #13）は並行可。

- [X] S0-1 Rust Local Core 再評価 ADR（ADR-001 は破棄せず、却下理由 L1–L3 に分解して hybrid での回避度を証拠化）→ `evidence/adr-003-rust-local-core.md`
- [X] S0-2 Phase 1 の外部接触面を versioned contract として凍結 → `specs/001-agent-memory-core/contracts/{rpc,writer-boundary,spool-format,error-taxonomy}-v1.md`
- [X] S0-3 Rust prototype の cutover 必須条件 G1–G7 と比較指標・測定条件を確定 → ADR-003「Cutover gate」
- [ ] S0-4 Stage 1 narrow prototype（別 branch / worktree、`vendor/codemem` に触れない）→ Stage 1 が決めるのは roadmap を進めてよいかと候補 scope の評価までである。実際の Core 1.0 の cutover scope と時期は、Cutover gate 1–10 と #84 の完了後に確定する。**採否そのものは ADR-005 で決着済み**（Rust を標準実行基盤とする方向で段階移行）。G1–G7 未達は言語選択の再考ではなく、default 切替の延期と TS reference の暫定継続を意味する → [`evidence/adr-005-rust-core-product-direction.md`](../../evidence/adr-005-rust-core-product-direction.md)

## Phase 2 — Canonical Identity / Event State Machine（着手時に詳細化）

Exit: SC-2。v6.1 §29 Phase 2 + §6 + §8 + §10.1。前提: 上記 Runtime barrier の S0-1〜S0-3 完了（S0-4 が決めるのは実装言語ではなく roadmap の進行可否と候補 scope。実際の cutover scope と時期は ADR-005 の Cutover gate 1–10 と #84 の完了後）。

## Phase 3 — Continuity State Machine [US1]（着手時に詳細化）

Exit: SC-3（= US1 Independent Test）。v6.1 §29 Phase 3 + §11。

## Phase 4 — Claude/Codex Vertical Routes [US3]（着手時に詳細化）

Exit: SC-4（= US3 Independent Test）。v6.1 §29 Phase 4。

## Phase 5 — Retrieval / Injection / MCP [US2]（着手時に詳細化）

Exit: SC-5（= US2 Independent Test）。v6.1 §29 Phase 5 + §16 + §18。

## Phase 6 — Generation Roles / Free Candidate [US4]（着手時に詳細化）

Exit: SC-6（= US4 Independent Test）。v6.1 §29 Phase 6 + §13–14。

## Phase 7 — Optional Embeddings（着手時に詳細化）

Exit: SC-7。v6.1 §29 Phase 7 + §15。

## Phase 8 — Core 1.0 Gates / Release（着手時に詳細化）

Exit: SC-8。v6.1 §29 Phase 8 + §27。

---

## Dependencies

- Phase 順序は直列: 0A → 0B → 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8（v6.1 §29/§30 固定。並べ替え不可）
- User story の完成順: US1（Phase 3）→ US3（Phase 4）→ US2（Phase 5）→ US4（Phase 6）。
  各 story は該当 Phase の Exit で独立検証されるが、基盤 Phase（0A〜2）を共有する
- Phase 0A 内: T002 → T004–T009（[P] 並列可、Codex 複数ジョブ）→ T010–T013（直列・判定）→ T014–T016

## Parallel Example

- T004 / T005 / T006 / T008 は別リポ対象で衝突なし → Codex background job 並走（`delegate-batch` 相当、各ジョブの成果物パスが分離済み）

## Implementation Strategy

- MVP = US1（same-agent continuation）だが、v6.1 の順序制約により最短経路は 0A→0B→1→2→3。
  Phase 3 完了時点が最初のユーザー価値実証点（observer/embedding/sync 全 off の継続性）。
- 各 Phase のマージは Exit gate + 2 本立てレビュー + （委譲回）speckit-verify-tasks を通過条件とする。
