# Task verification report — 継続状態に証跡の置き場を作る（Cluster C）

**Date**: 2026-08-17
**Feature**: `specs/002-continuity-state-evidence`
**Branch**: `feat/continuity-state-schema-v2`（`origin/main` = `d517a8b` から 6 commit）
**Scope**: `all`（`origin/main..HEAD` + 未コミット差分。未コミットはゼロ）
**Completed tasks assessed**: 44（`T001`–`T044`）。`T045` / `T046` は未完了なので対象外

> ⚠️ **FRESH SESSION ADVISORY**: 信頼性を上げるなら `/speckit.verify-tasks` は
> `/speckit.implement` を行ったのと**別のセッション**で実行する。実装したエージェントの
> 文脈は、自分の成果を肯定する方向に偏る。**このレポートは実装と同一セッションで生成した**
> ため、その偏りが乗っている前提で読むこと。緩和として、判定は極力「機械が出した値」
> （テスト件数・変異の kill 数・hash の diff）に紐付け、目視の所見は分けて書いた。

## Scorecard

| Verdict | 件数 |
|---|---|
| ✅ VERIFIED | 41 |
| 🔍 PARTIAL | 2 |
| ⚠️ WEAK | 0 |
| ❌ NOT_FOUND | 0 |
| ⏭️ SKIPPED | 1 |

## Flagged items

### 🔍 T019 — 「既存テストの期待値は 1 件も書き換えない（SC-003）」

**Evidence gap**: タスクの受け入れ条件そのものを満たしていない。
`git diff origin/main -- harness/continuity/reference-model.test.ts` の削除行に、期待値へ触れた
行が 37 ある。

**判定の根拠**: 機械層は 3 つとも positive（ファイル存在・diff に出現・記号一致）。意味層で
受け入れ条件との不一致を確認したので PARTIAL に落とす。

**内訳**（tasks.md の T044 節に同じ表がある）:

| 分類 | 件数 | 新しい欄が**無い**経路の挙動を変えたか |
|---|---|---|
| `operationStarts` の読み書きを要素の欄へ置き換え | 7 | いいえ（構造変更。同じ値を別の場所から読む） |
| 側索引の欠陥を仕様として固定していた test | 5 | **はい。ただし意図した変更**（その欠陥を消すのが US1 の目的） |
| 診断の網羅列挙に `dropped_evidence_recorded` を追加 | 7 | いいえ（FR-009 が診断を足した結果） |
| parity fixture の loop 化 | 2 | いいえ（構造変更） |
| #44 で再配送の指紋を受理済みと揃えた | 5 | いいえ（**入力**の変更。期待値は不変） |

**残る疑問**（この検証では潰していない）: 「意図した変更」5 件の妥当性は、変更後の期待値が
正しいことに依存する。それを支えているのは変異ゲート（179 件・生存 0）で、
独立した第三者のレビューではない。`T045` の `/code-review` がその役目。

### 🔍 T029 — 「隔離の判断自体は変えない（記録は隔離の代わりではない）」

**Evidence gap**: 記録を状態に入れるために、隔離の経路が `quarantine`（状態も台帳も変えない）
から `quarantineWithRecord`（**状態は変え、台帳は変えない**）へ分岐した。タスクの文面は
「判断を変えない」としか書いておらず、状態を変えてよいかは決めていない。

**判定の根拠**: 機械層は 4 つとも positive（`quarantineWithRecord` は定義され、1 箇所から
呼ばれている）。タスクが明示していない設計判断を含むので PARTIAL とし、判断の内容を
`tasks.md` の T029 節に残した。

**確認済みの性質**（いずれも test と変異で固定）:

- 配送鍵は消費しない（`ledger.size` 不変）。後から start が届けば同じ terminal で閉じられる
- 状態が変わる以上 revision を採番し、`history` にも 1 件積む
- 同じ `eventId` は 2 度記録しない（再送で収束する。`stateRevision` が動かないことまで確認）
- `outcome === "quarantined"` で分岐して snapshot を捨てる消費者は harness にゼロ

## Unassessable items

| Task | 理由 |
|---|---|
| ⏭️ T002 | 「作業ブランチを `origin/main` に載せ直す」。git 操作のみでファイル痕跡が無い（`git log` 上は rebase 済み） |

## Verified items

| Task | 検証に使った値 |
|---|---|
| T001 | `origin/main` に PR #51/#52/#55 が入っている（`git log`） |
| T003 | ベースライン: tsc clean / 286 tests / 変異 164・164・0 / hash 差分なし |
| T004–T006 | 凍結 schema に 3 欄 + `DroppedEvidenceEntryV1` + `droppedEvidence`。差分は**挿入のみ**（65 insertions / 0 deletions） |
| T007 | `continuity.ts` に同じ形（`schema-freeze` の 3 点突き合わせが通る） |
| T008 | `schema-freeze.test.ts` 16/16。新欄が `required` に無いこと・`maxItems` が 256 であることを固定 |
| T009 | hash 再生成で **schema の 1 行だけ**が動いた（fixture は不動） |
| T010–T013 | 実装前に 5 件とも赤であることを確認済み。現在 green |
| T014–T018 | `startFactsFor` / `OperationStartFactsV1` / `operationStarts` が 0 件（コメント除く）。`reference-model.ts` は 29 行純減 |
| T020–T025 | 実装前に 7 件とも赤。現在 green |
| T026–T029 | 診断コード 2 つ・`recordDroppedEvidence`・退避経路・孤児経路すべて配線済み（参照 1 件以上） |
| T030–T034 | 実装前に 2 件赤。`terminalFingerprint` の書き込みと `fingerprintConflict` の比較が配線済み |
| T035 | `restored-state-reduction.json` を追加し、parity test が 2 本とも読む。旧 fixture は削除していない |
| T036 | hash 再生成で新 fixture 1 件の追加と `tool-lifecycle` の 1 行のみ |
| T037 | 消えた機構の変異（索引の同名判定・退避同期）を明示的に削除。アンカー切れではない |
| T038 | §7 の 6 変異を含む 15 件を追加。全件が 1 件以上の test を落とす |
| T039 | **実行 179 / 期待 179 / 生存 0**（黙って飛ばされた変異ゼロ） |
| T040 | addendum §4.3 に 2 段落。3 検査の材料と `droppedEvidence` の規則 |
| T041 | Revision log に 2 行。**節見出しの差分はゼロ**（番号もファイル名も不変） |
| T042 | 限界節を書き直し、kill table を live run から再生成（164 → 179 行）。表と実行の突き合わせが空 |
| T043 | 5 ゲート通し: tsc clean / 307 tests / 変異 179・179・0 / hash 一致 / 未追跡の生成物なし |
| T044 | 上の T019 の表がその結果 |

## Machine-parseable verdicts

| T001 | ✅ VERIFIED | origin/main に 3 PR が入っている |
| T002 | ⏭️ SKIPPED | git 操作のみで痕跡が無い |
| T003 | ✅ VERIFIED | ベースライン 4 ゲート緑を記録 |
| T004 | ✅ VERIFIED | PendingOperation に任意欄 3 つ・required 不変 |
| T005 | ✅ VERIFIED | DroppedEvidenceEntryV1・oneOf 無し |
| T006 | ✅ VERIFIED | droppedEvidence（maxItems 256・任意） |
| T007 | ✅ VERIFIED | TS 側が同じ形（凍結 test が 3 点で突き合わせ） |
| T008 | ✅ VERIFIED | schema-freeze 16/16 |
| T009 | ✅ VERIFIED | schema の hash だけが動いた |
| T010 | ✅ VERIFIED | FR-001/FR-002 の回帰が赤→緑 |
| T011 | ✅ VERIFIED | FR-003 の回帰が赤→緑 |
| T012 | ✅ VERIFIED | FR-004 の回帰が赤→緑 |
| T013 | ✅ VERIFIED | SC-001 の回帰が赤→緑（主張は書き換え済み・tasks.md に明記） |
| T014 | ✅ VERIFIED | start 受理で 2 欄を書く |
| T015 | ✅ VERIFIED | 順序検査が startIngestSeqOf を読む |
| T016 | ✅ VERIFIED | rule 2 が startTurnIdSourceOf を読む |
| T017 | ✅ VERIFIED | startFactsFor が 0 件 |
| T018 | ✅ VERIFIED | 索引と型が 0 件・実装 29 行純減 |
| T019 | 🔍 PARTIAL | SC-003 未達（期待値 37 行に接触・内訳は表） |
| T020 | ✅ VERIFIED | FR-005 の回帰が赤→緑 |
| T021 | ✅ VERIFIED | FR-006 の回帰が赤→緑 |
| T022 | ✅ VERIFIED | FR-007（欄集合そのものを固定） |
| T023 | ✅ VERIFIED | FR-008（先頭から落とす）を変異で kill |
| T024 | ✅ VERIFIED | 診断 2 種が出ること |
| T025 | ✅ VERIFIED | FR-013/FR-014（欄なし状態を読む・空配列を作らない） |
| T026 | ✅ VERIFIED | 診断コード 2 つ |
| T027 | ✅ VERIFIED | recordDroppedEvidence（末尾追加・先頭脱落） |
| T028 | ✅ VERIFIED | 退避経路から呼ばれ、sensitivity を引き継ぐ |
| T029 | 🔍 PARTIAL | 隔離経路が状態を変える設計判断を含む（tasks.md に記録） |
| T030 | ✅ VERIFIED | FR-010/FR-011 の回帰が赤→緑 |
| T031 | ✅ VERIFIED | 同じ指紋なら適用済み |
| T032 | ✅ VERIFIED | FR-012（指紋なしは従来どおり） |
| T033 | ✅ VERIFIED | 確定時に指紋を書き、unknown では書かない |
| T034 | ✅ VERIFIED | fingerprintConflict の比較 |
| T035 | ✅ VERIFIED | 新 fixture 追加・旧 fixture 保持 |
| T036 | ✅ VERIFIED | hash の差分が想定どおり |
| T037 | ✅ VERIFIED | 消えた機構の変異を明示的に削除 |
| T038 | ✅ VERIFIED | §7 の 6 変異を含む 15 件を追加 |
| T039 | ✅ VERIFIED | 実行 179 / 期待 179 / 生存 0 |
| T040 | ✅ VERIFIED | addendum §4.3 に 2 段落 |
| T041 | ✅ VERIFIED | Revision log 2 行・節見出しの差分ゼロ |
| T042 | ✅ VERIFIED | 限界節と kill table を実測から更新 |
| T043 | ✅ VERIFIED | 5 ゲート通しで緑 |
| T044 | ✅ VERIFIED | SC-003 の結果を表で提示 |

## この検証の限界

- **同一セッションで生成した**。上の advisory どおり偏りが乗っている。緩和として判定を
  機械の出力に紐付けたが、「test の期待値が正しいか」は変異ゲート（自分が書いたもの）が
  根拠なので、独立していない。`T045` の `/code-review` がその穴を埋める役目
- **意味層は目視**。stub の検出は「空の関数・`TODO`・固定値の返却」を探しただけで、
  論理の正しさは見ていない
- **`T045` / `T046` は未完了**なので対象外。この検証自体が `T046`

## Walkthrough Log

flagged 2 件をどちらも **I（investigate）** で追い、判定は動かさずに処置だけを記録する。
上の scorecard と Flagged Items / Verified Items は監査記録なので書き換えない。

| # | タスク | 元の判定 | 処置 | 結果 |
|---|---|---|---|---|
| 1 | T019 | 🔍 PARTIAL | 調査した。接触した 37 行の期待値を 1 行ずつ内訳に落とし、**5 行が側索引方式の欠陥を仕様として固定していた**ことを確認した（#35 が消す対象そのもの）。残りは欄の追加に伴う形の変更 | **判定は PARTIAL のまま**。タスク文の「既存の期待値を書き換えない」は満たしていないので、緩めずに記録として残す。内訳は本レポートの当該節と PR 本文にある |
| 2 | T029 | 🔍 PARTIAL | 調査した。孤児の記録により隔離経路が状態を変えるようになった点を確認した。タスクが約束したのは「**判断**が変わらない」ことで、状態の不変性ではない | **判定は PARTIAL のまま**。設計判断として `tasks.md` と正本 §4.3 に記録済み。後続の指摘（B-3 ほか）もこの経路を前提に評価している |

修正は当該タスクに対しては行っていない（どちらも「記録して残す」処置）。判定を動かす必要が
出た場合は `/speckit.verify-tasks` を新しいセッションで走らせ直すこと。
