# ADR-003: Local Core の Rust 段階移行を評価可能にする（#1 Stage 0）

- Status: **Accepted**（測定・cutover contract として。contract freeze、G1–G7、比較指標、shadow / rollback 方針は有効）
- Superseded in part: 「Rust へ移行するか自体を Stage 1 で決める」という判断範囲だけが
  [ADR-005](adr-005-rust-core-product-direction.md) に置き換わった。Stage 1 が決めるのは roadmap を進めてよいかと候補 scope の評価までである。実際の Core 1.0 の cutover scope と時期は、Cutover gate 1–10 と #84 の完了後に確定する。
  この ADR が Rejected になる経路は無い
- Date: 2026-08-16
- Related: [ADR-001](adr-001-base.md)（実装ベース = codemem pinned vendor snapshot）、[phase-1-design.md](../specs/001-agent-memory-core/phase-1-design.md) ADR-002（peer auth = Unix DAC）、GitHub issue #1 / #8 / #13
- Supersedes: なし。ADR-001 は破棄しない

## 決定

1. **ADR-001 は有効なまま**とする。TypeScript 版 Local Core は Phase 1 完了時点の canonical 実装であり、Stage 2 の shadow 期間中も canonical writer は TS 側に置く。
2. Phase 1 の外部接触面を **versioned contract として凍結**する。凍結対象は次の 4 つで、`specs/001-agent-memory-core/contracts/` に配置する。
   - [`rpc-v1.md`](../specs/001-agent-memory-core/contracts/rpc-v1.md) — local RPC wire contract
   - [`writer-boundary-v1.md`](../specs/001-agent-memory-core/contracts/writer-boundary-v1.md) — sole-writer 不変条件
   - [`spool-format-v1.md`](../specs/001-agent-memory-core/contracts/spool-format-v1.md) — spool on-disk format
   - [`error-taxonomy-v1.md`](../specs/001-agent-memory-core/contracts/error-taxonomy-v1.md) — typed error と fail-open 契約
3. **Stage 1 が決めるのは roadmap を進めてよいかと候補 scope の評価までである。実際の Core 1.0 の cutover scope と時期は、Cutover gate 1–10 と #84 の完了後に確定する**。本 ADR では
   その測定方法と閾値（G1–G7）だけを固定する（後述）。移行するか自体は [ADR-005](adr-005-rust-core-product-direction.md) で
   決着しており、この ADR の実測結果はそれを覆さない。
4. Stage 0 完了までは **Phase 2 以降の大規模 TS product 実装を増やさない**（issue #1 の制約）。runtime-neutral な schema / fixture / harness（Phase 3 preflight、issue #13）は並行して進めてよい。

## 背景 — ADR-001 が Rust を却下した理由の分解

ADR-001 の却下理由は 1 行にまとめられていたが、Stage 0 ではこれを 3 つの独立した損失に分解する。

| # | 損失 | ADR-001 時点の根拠 | 対象範囲 |
|---|---|---|---|
| L1 | 既存資産の喪失 | Rust 候補（ai-memory / remem）はコード移植不可 = 実質 greenfield | daemon / store / retrieval / CLI / viewer / MCP 全体 |
| L2 | テスト資産の再構築 | 4,037 件級（当時計測）のテストをゼロから作り直すコスト | vitest suite 全体 |
| L3 | adapter / viewer / MCP の再実装 | スタック不一致（v6.1 は TS/Node/SQLite 前提） | hook adapter・React viewer・MCP server |

## 各損失を hybrid 案がどこまで回避するか

**L1（既存資産の喪失）— 大部分を回避する。**
issue #1 の hybrid は「Rust で全部を書き直す」ではなく「daemon の内側だけを slice 単位で置換する」。Stage 1 の対象は 3 slice（single-instance + authenticated local RPC handshake / SQLite writer actor + migration・backup smoke / event ingest → durable job・spool → exactly-once commit）に限定され、Stage 2 では TS daemon が canonical writer のまま Rust を shadow で走らせる。置換されない資産（retrieval、maintenance、export/import、viewer、adapter）は TS のまま残る。ただし Stage 3 以降で storage / continuity を cutover する場合、その範囲の TS 実装は最終的に置換対象になる — これは回避ではなく**段階化**である。

**L2（テスト資産の再構築）— 回避には条件が付く。**
Phase 1 の TS suite（現行 114 files / 1,851 passed + 3 todo）は TS 実装に対するテストなので、Rust には自動的には効かない。Rust 側の正しさを担保する資産は次の 2 つで、いずれも**言語非依存として設計済み**である。

- `harness/`（Phase 0B）の capability fixture / golden matrix — 実 CLI 由来の JSON fixture であり実装言語に依存しない
- Phase 3 preflight（#13 / PR #14）の runtime-neutral contract fixture — 「TypeScript と Rust が同一の schema・fixture・report hash を消費する」ことを計画の制約として明記済み

したがって L2 は「4,000 件のテストを Rust に移植する」ではなく「contract fixture を両実装に適用する」問題に変換できる。

本 ADR 作成時点（2026-08-16）では contract fixture がまだ無く、それを未確定要素として記録していた。
2026-08-18 現在は runtime-neutral な基礎——`harness/schema/continuity.schema.json`、
`harness/fixtures/continuity/`、`harness/continuity/reference-model.ts` と契約テスト——が存在する。
**ただし Phase 3 preflight の Task 4–10 が全部通ったわけではない**（#13）。「fixture が無い」ことと
「preflight の全ゲートが未完了」であることは別で、現在地は後者。この変換が成立するかどうかは
引き続き Stage 1 の実測項目に含める。

**L3（adapter / viewer / MCP の再実装）— Phase 1 の成果で構造的に回避済み。**
Phase 1 の T041–T048 により、hook adapter・CLI・MCP server・viewer はすべて daemon RPC のクライアントになり、daemon 外の DB handle はゼロになった（T048、Exit-1a/1b で機械検証済み）。RPC contract を凍結すれば、daemon 実装の言語は adapter から観測できない。よって L3 は「RPC contract v1 を Rust が忠実に再実装できるか」という 1 点に縮約される。凍結文書はそのために作成する。

## 現行 TS 実装で観測された、移行判断に効く事実

- **Local Core は現時点で Linux 専用**である（`assertSupportedStoragePlatform()` が linux 以外で throw する。`vendor/codemem/packages/core/src/storage-platform.ts:23-27`）。Windows / macOS 対応は TS 版でも未着手であり、「Rust なら Windows が動く」ではなく「どちらの実装でも新規作業」である点を cutover の比較で公平に扱う。
- **peer auth は暗号的な peer 認証ではなくファイルシステム DAC** である（0700 control dir + 0600 socket、`daemon-rpc.ts` の `mapPeerConnectError` は connect(2) の errno を typed error に写像するだけ）。Rust 側も同じ前提で実装すれば等価になる。ADR-002 の決定どおり。
- **handshake は完全一致判定**で、`RPC_CAPABILITY_HASH` は RPC method 一覧の SHA-256 である。method の増減で hash が変わるため、Rust 実装は method 集合を 1 件でも変えると既存 client と handshake できない。これは互換性の強い保証であると同時に、Rust 側の自由度を狭める制約でもある。

## Cutover gate（Stage 1 完了時に判定。旧称 Go / No-Go gate）

### 必須条件（1 つでも欠けたら defer）

| ID | 条件 | 測定方法 |
|---|---|---|
| G1 | Phase 1 の sole-writer / auth / redaction / spool 不変条件を維持する | `writer-boundary-v1.md` と `spool-format-v1.md` の各 MUST を Rust prototype に対して再実行。Exit-1b 相当の owner-set 検査を再利用する |
| G2 | fault injection で data loss / duplicate commit / split brain がない | Phase 1 の T055 シナリオ（daemon kill 中の spool → exactly one commit、replay ×10、lock race、force-kill identity）を Rust prototype に適用 |
| G3 | adapter から見た RPC contract を変更せずに置換できる | 既存 TS adapter（hook / CLI / MCP / viewer）を無改造で Rust daemon に接続し、`rpc-v1.md` の全 method で応答形状と typed error codes が一致すること。**method の追加も不可**: `capability_hash` は `RPC_METHODS` 一覧のハッシュなので、method を 1 つ増やすだけで既存 adapter の handshake が `protocol_mismatch` で落ちる（`rpc-v1.md` §1.2）。拡張が要るなら contract version を上げる別の変更として扱う |
| G4 | Windows を含む process lifecycle が TS 版以上に安定する | 同一の lifecycle シナリオを両実装で実行し、失敗数を比較。TS 版が Linux 専用である事実は「TS 版 = 未対応」として記録し、Rust 側の実測値のみで可否を見る |
| G5 | clean install に Node / Python 等の Core 実行時依存を要求しない | 素の環境で配布物のみを install し、daemon 起動 → RPC 疎通まで到達すること |
| G6 | DB migration と rollback を実証できる | Phase 1 の migration + online backup + restore journal 方式を Rust prototype で往復させ、canonical rows と manifest hash が一致すること |
| G7 | 現 harness / golden matrix を再利用できる | `harness/` の fixture を改変せずに Rust prototype へ適用できること。改変が必要なら、その差分を defer 材料として記録する |

### 比較指標（数値は記録必須、単独では判定しない）

cold start / warm start、idle RSS、event ingest p50 / p95 / p99、concurrent hook burst、forced-kill recovery time、spool replay throughput、DB migration / backup time、packaged artifact size、platform 別 failure count、実装行数とテスト行数の差分。

測定条件は両実装で揃える: 同一マシン、同一 data_dir 構成、同一 fixture 集合、各指標 10 回試行の中央値と p95 を記録し、実行コマンドと commit SHA を evidence に残す。spool 系（replay throughput / forced-kill recovery / cold start）は **空の spool と滞留 spool の両方**で測る: 0 件、1,000 件、`SPOOL_NORMAL_QUOTA_BYTES`（128 MiB）に迫る水準の 3 段階。滞留時は quarantine 済みファイルを混ぜた状態も 1 水準含める。理由: quota 使用量は永続カウンタではなく、`scanUsage()` が毎回 `tmp/` と `ready/` を走査して算出する（`spool-format-v1.md` §5）。したがって書き込み 1 件のコストが backlog サイズに依存し、空 spool のみの測定では両実装の差が出ない。

### 判定規則

判定するのは roadmap を進めてよいかであり、言語の採否ではない。語も分ける: **pass**（cutover roadmap へ
進める）と **defer**（見送る）を使い、Go / No-Go とは呼ばない。

- **`pass` = G1–G7 をすべて満たす**。これが唯一の pass 条件で、[ADR-005](adr-005-rust-core-product-direction.md) の
  「Stage 1 の再定義」と同一。
- 性能差の大きさは追加の gate ではない。G1–G7 を満たしていれば、性能差が小さくても
  **運用安定性・配布容易性・依存削減**のいずれかが改善していれば pass を妨げない（issue #1 の方針）。
- pass は default 切替の十分条件ではない。**default 切替の条件は Cutover gate 1–10 をすべて満たし、かつ #84 の正本連鎖の改訂が完了していること**。
- pass の場合、Stage 2 以降を子 Issue に分割し、Phase 2 以降の roadmap を Rust 中心に再編する。
  Stage 2 の shadow 期間中の canonical writer は上記「決定 1」のとおり TS 側に置いたままにする。
- defer の場合、**言語選択を差し戻すのではなく**、Core 1.0 での default 切替を延期し TS reference を暫定継続する
  （[ADR-005](adr-005-rust-core-product-direction.md)「Stage 1 の再定義」）。この ADR を Rejected にはしない。
  凍結した 4 contract は defer 時も破棄しない（Phase 2 以降の adapter 追加と #8 parity benchmark に使う）。

## 帰結

- Stage 0 完了 = 本 ADR + 4 contract 文書 + cutover gate の定義が揃った状態。これをもって Phase 2 の barrier を解除できる。
- Stage 1 の Rust prototype は独立した branch / worktree で行い、`vendor/codemem` の TS 実装には触れない。
- contract 文書は「現状の記述」であり改善提案を含まない。実装のギャップは各文書の Known gaps に記録し、Rust 側が再現すべきか否かを明示する。
- 別リポジトリでの `claude-mem-rs` 新規開発は、Stage 1 が pass に達するまで開始しない。
