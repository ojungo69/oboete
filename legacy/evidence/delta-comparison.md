# T011 — fork / vendor / greenfield delta 比較

日付: 2026-08-12。比較軸は v6.1 §29 Phase 0A の 4 指標 + 言語適合。
入力: 各リポの evidence（upstream-test.log / sbom.md / inventory）+ T010 分類。

## 候補プロファイル

| | codemem `26438e75` | ai-memory `a9e9a24d` | remem `cde8bc05` |
|---|---|---|---|
| 言語/スタック | **TypeScript / Node / better-sqlite3**（v6.1 決定スタックと一致） | Rust / rusqlite | Rust / rusqlite（+ Node plugin app） |
| License | MIT | MIT | MIT |
| upstream tests（ホスト実測） | **4028 passed / 6 failed / 4037**（HOST RETRY2 = `pnpm build` 後。失敗 6 は逐件確認済みの環境依存 — 下記「codemem 既知 env-fail baseline」参照。tsc / biome は green） | 666 passed / 2 failed / 669（失敗 2 は tmpdir 環境依存） | 3777 passed / 1 failed / 3779（失敗 1 は並行 migration の "database is locked" flake — 単一 writer 境界不在の症状） |
| write-capable 表面 | MemoryStore 集中型（単一クラス + 周辺 connect()。fatal 計 10 経路は局在、T010 参照） | **単一 writer actor 既備**（`WriterHandle` mpsc、1 origin） | 分散型: write-capable wrapper 取得 101 箇所 / 62 ファイル、単一 writer 境界なし |
| unsafe auth path | fatal 計 10 経路（auth 系 5: OAuth consumer×2 / plain -p sidecar / 第三者 OAuth cache 読出し / credential command + sole-writer 系 5、T010 集計）— すべて局在・除去可能見込み | 13 env 名 / OAuth・Copilot token の auth-file 集中管理 1 モジュール。第三者 credential store の読出しはなし（importer が読むのは自身の companion server 用 bearer を env から） | macOS benchmark の Codex auth.json コピー 1 件（eval 限定）。runtime は CLI 委譲で credential 非解析 |
| 移植資産（コード再利用） | **直接再利用可**: DB/schema bootstrap、FTS 基盤、hook 配線（Claude/Codex/OpenCode）、MCP stdio、viewer、sqlite-vec 0.1.9 統合（v6.1 pin と一致）、ingest pipeline、spool | 設計のみ（Rust）: writer actor パターン、reader pool 分離、hook spool 設計 | 設計のみ（Rust）: SQLCipher 運用、pack import/export 形式、native transcript ingest 対象一覧 |

## 3 戦略の delta（v6.1 §29 の 4 指標）

対象基準: Core 1.0 の到達点（Phase 1 完了時の姿 = daemon 単独 writer + thin clients、carve-out 無効化）。

| 指標 | fork（codemem を追随） | **vendor（codemem pin + 選択改変）** | greenfield（新規 + MIT 資産移植） |
|---|---|---|---|
| 残る write handle 数 | upstream 追随のたびに多写経路が再流入（120 構築サイトの上流変化を継続吸収） | Phase 1 で daemon 集約後は **1**（+ read-only 2）。上流再流入なし（pin 固定） | 定義上 1 だが、DB/schema/FTS/hook を全部書き直してから |
| 壊す test 数 | 上流 sync/coordinator/sharing テスト群と恒常競合（carve-out が rebase 地獄化） | carve-out 領域のテスト（viewer sync routes / coordinator / cloudflare-worker / sharing。初回 fail 9 ファイルのうち carve-out 域は 2 = cloudflare-coordinator-worker / viewer-server index — 残 7 は core 系で build 後に green 化し**維持対象**）を**削除対象として明示的に retire**。core の ingest/store/検索系テストは維持・流用 | 全テスト新規作成（4,037 件級の safety net をゼロから） |
| 移植資産数 | n/a（全部保持 = 不要資産も保持） | **大**: core store/schema/FTS/hook/MCP/viewer + sqlite-vec 0.1.9。不要領域（sync 7.6k 行 routes ほか）は捨てる | 中: 設計 + 断片コピーのみ（TS だが構造が密結合なため単純切り出しは限定的） |
| unsafe auth path 数 | 上流の auth 経路追加を毎回再監査 | **auth 系 5 → 0**（F1–F4, F8。加えて sole-writer 系 5 = F5–F7 + 補遺 2 も同時に 0 化 — 計 10 経路、T010 集計の正本どおり Phase 1 で物理削除。局在性確認済み） | 0（ただし hook 配線・sidecar 知見を再発見するコスト） |

## 補足所見

- ai-memory の writer actor は v6.1 Hard Invariant 4 の理想形だが、**Rust 全面のため base にすると v6.1 の決定スタック（TS/Node）と衝突**し、5 CLI adapter・MCP・viewer を含む全面書き直し = 実質 greenfield。設計参照元としてのみ利用（Phase 1 の daemon writer 設計に actor パターンを採用）。
- remem は多写・非集中型で、v6.1 が要求する境界からの距離が 3 候補中最遠。SQLCipher・pack 形式は将来の参考。
- codemem の初回実行（build 前提不足）で fail した 9 ファイル中、carve-out 領域は 2（viewer-server index / cloudflare-coordinator-worker）。`pnpm build` 後の正実行（HOST RETRY2）では **6 fail まで減少し、逐件確認で全件が環境依存**（`/tmp` project 解決 ×3、timeout ×2、host env var による auth cascade 分岐 ×1 — 下記 baseline）。upstream suite は実質 green。
- codemem は sqlite-vec **0.1.9** を既に統合しており、v6.1 §15 の pin と完全一致（embedding は既定 off にする改変のみ）。

## codemem 既知 env-fail baseline（Phase 1 の「壊す test 数」差分の基準線）

HOST RETRY2 時点で fail した 6 件の全数。エラー本文は `codemem/upstream-test.log` の HOST RETRY2 節が正。

| # | テスト | エラー | 分類 |
|---|---|---|---|
| 1 | core `src/ingest-pipeline.test.ts` › creates memories from observer response | `expected 'tmp' to be 'test-project'` | `/tmp` project 解決（host の git 検出が `/tmp` 配下の期待値を上書き） |
| 2 | core `src/ingest-pipeline.test.ts` › falls back to cwd basename when payload project is missing | `expected 'tmp' to be 'codemem'` | 同上 |
| 3 | core `src/project.test.ts` › returns cwd basename when no git repo exists | `expected 'tmp' to be 'bar'` | 同上 |
| 4 | core `src/claude-hooks.test.ts` › returns {inserted:0, skipped:1} for unsupported hook event | `Test timed out in 5000ms` | 並列負荷での 5s timeout |
| 5 | mcp-server `src/project-scope.test.ts` › prefers CODEMEM_PROJECT when set | `Test timed out in 5000ms` | 同上 |
| 6 | core `src/observer-client.test.ts` › passes configured reasoning overrides to the codex consumer request | `expected 'api_direct' to be 'codex_consumer'` | host env var 漏洩: 実行 shell に `OPENAI_API_KEY` が SET のため auth cascade の env 段が oauth（codex consumer）段より先に解決（本経路は Phase 1 A2 で削除される F2 そのもの） |

Phase 1 でテストを retire / 変更した際は、この 6 件を除いた差分で「壊す test 数」を集計する。

## 結論（T013 base ADR への入力）

**vendor（pinned snapshot + 選択改変）が 4 指標すべてで優位**。fork は carve-out と上流の恒常競合、greenfield は safety net と実装量で劣後。§4.3 gate の成立条件（fatal 除去可能性）は T010 で確認済み。
