# Claude Code 引き継ぎ — 009 memory core

2026-09-10。ユーザーの最新依頼は「Claude Code が引き継げる区切りまで進める」。
移行の基本経路と確認済みの不具合修正まで進めた。009 全体・US5 全体は未完了。

## 最初に行うこと

1. `/home/jura/projects/free-mem-wt/009-memory-core` で作業する。
   branch は `009-memory-core`、基点 HEAD は `c9a9e585ca89e1b2191baa14fe309bdd58f98fb6`。
   `/home/jura/projects/free-mem` は今回の実装先ではない。
2. `git status --short` と HEAD を確認し、既存の未コミット変更・未追跡ファイルを維持する。
   この worktree に累積実装がある。commit、push、PR、merge、deploy は行っていない。
3. [spec](spec.md)、[plan](plan.md)、[tasks](tasks.md)、[migration contract](contracts/migration.md)
   と [quickstart の最新記録](quickstart.md) を読む。Spec Kit を process owner とし、
   合意済みの全体設計を聞き直さず、下記 US5 残件から続ける。

既存の許可は隔離 worktree 内の実装・ローカル検証まで。日常用インストール、実エージェント起動、
実モデル・有料 API、クラウド転送・アカウント変更・公開は別の activation。
ソースを編集する writer は一人にする。

## 現在の実装

- T001–T019、T021–T022、T025–T028、T046 は証拠付きでチェック済み。
  保持・再処理、work/context/checkpoint、共通スコープ、共有承認、測定の切り分けを実装した。
  各範囲の限界は `quickstart.md` に残してある。
- US5: native V1/V2 の streaming input、private SQLite plan、preview/apply、origin receipts、
  tombstone 優先、既存データの所有権維持、ローカル再分類を実装済み。
- 今回、公開 claude-mem query export の pinned adapter を CLI に接続した。
  `--from claude-mem`、`--map-project`、`--map-project-hash`、`--map-context` を受け付ける。
  V2/external は preview が既定。V1 の implicit apply は互換性のため残す。
  V2/external の apply は既存の current schema が必要。preview は schema を作成・移行しない。
- Fixture は [claude-mem-query-export-8bc631a.json](../../test/fixtures/migration/claude-mem-query-export-8bc631a.json)。
  9 memory origins が 8 local memories に収束する。本文・元ファイル・未知メタデータの保持、
  repeated import、native 再 export/import、unsupported records を検証した。
- セッションと一意に関連するプロンプトの全 payload も、各 memory の private source receipt に
  含めて再分類する。50 sources / 2 MiB の境界内で複製する簡単な方式を採用した。
  共有参照用テーブルは追加していない。大量データでこの上限に達する場合の扱いは測定対象。
- 再分類は本文だけでなく、evidence・typed JSON lists・未知メタデータ・関連 session/prompt を検査する。
  policy/context/lease と全 snapshot を書き込み直前に再確認し、sanitation で identity が変われば
  既存 active/tombstone への収束を再判定する。foreign IDs/roots は local authority にしない。
- native export は DB 本体、symlink/別名、WAL/SHM/journal を出力先として拒否する。
  完全な staged graph を reader で検証し、fresh `data_version` 確認後に公開する。
  混在する source/checkpoint cycles、欠落 repo、NULL を含む proposal mismatch、
  terminal parent からの payload 持ち越しを拒否する。

主な入口は [transfer.ts](../../src/transfer.ts)、[transfer-plan.ts](../../src/transfer-plan.ts)、
[transfer-format.ts](../../src/transfer-format.ts)、[transfer-merge.ts](../../src/transfer-merge.ts)、
[transfer-claude-mem.ts](../../src/transfer-claude-mem.ts)、[worker/imported.ts](../../src/worker/imported.ts)。
Schema は未出荷の 0007。0001–0006 を今回の移行修正では変更していない。
0007 の開発途中に作った保存 fixture DB は checksum が異なり得るため、新しい fixture を作る。
checksum 検証を弱めて古い試験 DB を再利用しない。

## 再開順序と未完了事項

1. **US5 を閉じる**。T029–T032 はすべて未チェックのまま。
   - `migration.md` の wire appendix を現在の validator と一致させ、外部 schema の一次資料を
     `research.md` にまとめる。既存 appendix draft は stale なのでそのまま貼らない。
   - `oboete import promote` を実装する。契約は確定済み: clean な held candidate、現在の verified
     cwd/context、既存 work の明示 mapping から、新しい pending/inferred proposal を作る。
     source event IDs は空。新たな人間の `share approve` で承認する。外国の承認を引き継がない。
   - preview の context 候補・omission accounting、schema/WAL ケース、mapping 変更と競合、
     terminal/approved/rejected/personal の完全 round trip を補完する。
     source dependency 側の terminal state、personal grant 削除後の identity domain 保持も確認対象。
   - export publication の concurrent writer、high-cardinality JSON、多数行、scratch amplification、
     native 256 MiB / external 5 MiB の packed CLI RSS を測る。上限値を実証済みとは扱わない。
   - 最新 US5 全体の correctness/security、Standards/Spec、Ponytail、必要な CLI review を完走する。
2. **US6**: `contracts/sync.md` はまだない。暗号/transport の調査は済んでいるが実装・依存追加はない。
   artifact の `sync-api-research.md` と `us6-implementation-handoff.md` から T033 を具体化する。
   migration merger は historical/quarantine 用なので、そのまま sync の現行 work 更新に使わない。
3. **US7 + owner amendment**: T037–T039、T047 resident worker、T048 configured model/provider fallback。
   常駐・fallback の許可は spec/constitution に記録済み。具体的な lifecycle/consent/cost 契約は未実装。
4. **実測と最終 gate**: T020、T023–T024、T040–T045。C3 の recall・hook timing・一部 Grok delivery は
   未達/未評価が残る。全 12 agent pairs、実 Linux/WSL/macOS、選択した実モデル、100k events、
   7 日運用を synthetic tests の成功で置き換えない。

## 今回の検証とレビュー制約

生ログと補助調査は `/var/tmp/oboete-009-20260909.jJ5grc/`。

| 確認 | 結果 | Receipt |
| --- | --- | --- |
| build / typecheck / lint | PASS | `us5-handoff-build.log`, `us5-handoff-typecheck.log`, `us5-handoff-lint.log` |
| Node 24.16.0 unit/migration/scripts | 1,134 PASS | `us5-handoff-unit-node24.tap` |
| Node 22.16.0 unit/migration/scripts | 1,134 PASS | `us5-handoff-unit-node22.tap` |
| Node 24 serial E2E/fault | 202 PASS | `us5-handoff-serial-node24-isolated.tap` |
| Node 22 serial E2E/fault | 202 PASS | `us5-handoff-serial-node22-isolated.tap` |
| packed install / version | PASS, 20.698 MB | `us5-handoff-pack.log` |
| actual CLI help | PASS | `us5-handoff-help.log` |

両 Node とも unit/migration/scripts と serial E2E/fault の合計は 1,336 PASS。
Markdown lint・ローカル参照チェック・`git diff --check` も実施済み。
未コミットファイルの SHA-256 manifest は artifact の `us5-handoff-snapshot.json`。

初回 Node 24 serial run は 201 PASS / 1 FAIL。unit 実行と pack-check の rebuild を同時に走らせ、
巨大入力の partial-row テストが失敗した。テストが指定する単独実行に直すとソース変更なしで
202 PASS。失敗ログ `us5-handoff-serial-node24.tap` も保持した。今後も build/pack、parallel unit、
serial E2E/fault は順番に実行する。

Native integrity の 9 ケースはすべて RED を確認して修正し、GREEN にした
(`us5-native-integrity-red.tap`, `us5-native-integrity-green.tap`)。
今回のローカル review 記録は `us5-handoff-review.md`。追加の独立レビューは agent の利用上限で
中断した。最後の独立 classifier security review は今回の external 接続より前の snapshot が対象。
全 US5 のレビュー済みとは扱わない。以前の formal security finalizer も evidence path 拒否で
未完了であり、T043 の残件。finalized security report は存在すると主張しない。

再検証の command は `package.json` が正本。変更後は影響範囲だけ RED/GREEN を回し、US5 を閉じる
時点で両 Node の全 unit/migration/scripts → serial E2E/fault → pack を順番に実行する。

## Claude Code への開始指示

```text
/home/jura/projects/free-mem-wt/009-memory-core で作業してください。
specs/009-memory-core/HANDOFF-claude-code.md と、そこで指定された spec/plan/tasks を読み、
既存の未コミット実装を保持して US5 の残件から続けてください。
commit/push/PR/merge/deploy、日常用インストール、実モデルやクラウドの activation は未許可です。
```
