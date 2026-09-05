---

description: "Task list for 継続状態に証跡の置き場を作る（Cluster C）"
---

# Tasks: 継続状態に証跡の置き場を作る（Cluster C）

**Input**: Design documents from `/specs/002-continuity-state-evidence/`

**Prerequisites**: plan.md / spec.md / research.md / data-model.md / quickstart.md（すべて作成済み）

**Tests**: **必要**。spec の SC-003（旧形入力への挙動が変更前と一致する — 判定は T049 の差分
ゲート）と SC-004（規則ごとの変異が生存 0）はテストと変異ゲートでしか示せない。テスト作成は
任意ではなく要件。

**Organization**: user story ごとに phase を分ける。ただし **phase の順序は入れ替えられない**
（plan.md「実装順序」）。凍結 schema を先に固定しないと、どの story も「何を書いてよいか」を
決められないため。

## Format: `[ID] [P?] [Story] Description`

- **[P]**: 並列可（別ファイル・未完了タスクへの依存なし）
- **[Story]**: US1 / US2 / US3（spec.md の user story に対応）
- パスは repo ルート相対。作業ディレクトリは下の `$WT`（この feature の worktree）

## Path Conventions

単一プロジェクト。契約は `harness/schema/`、参照実装とテストは `harness/continuity/`、
正本は `specs/001-agent-memory-core/` と `evidence/`。新しいディレクトリは作らない。

## 共通の実行コマンド（各タスクの検証で使う）

```bash
WT=$(git rev-parse --show-toplevel)   # この feature の worktree
"$WT/vendor/codemem/node_modules/.bin/tsc" -p "$WT/harness/tsconfig.json" --noEmit
node --experimental-strip-types --test "$WT"/harness/continuity/*.test.ts
bash "$WT/harness/continuity/mutate.sh"

# 契約 hash（CI と同じ手順。手で書かず、生成物で上書きする）
cd "$WT" && node harness/contract-hashes.mjs > /tmp/contract-hashes.json
diff harness/contract-hashes.json /tmp/contract-hashes.json   # CI が見るのはこの diff
cp /tmp/contract-hashes.json harness/contract-hashes.json     # 更新するときだけ
```

---

## Phase 1: Setup（前提の確認）

**Purpose**: 着手可能な状態かを機械的に確かめる。ここで赤なら実装に入らない。

- [X] T001 `main` に PR #51・#52・#55 が入っていることを確認する（`git -C "$WT" fetch origin && git log origin/main --oneline | head -20`）。3 本とも `harness/continuity/` と `harness/contract-hashes.json` を触るので、未マージなら着手しない
- [X] T002 作業ブランチを `origin/main` に載せ直す（`git -C "$WT" rebase origin/main`）。plan までの成果物は `specs/` しか触っていないので衝突しないはず。衝突したら内容を確認してから解決する
- [X] T003 vendor の依存を入れてから着手前のベースラインを取る（`tsc` / テスト / `mutate.sh` / `contract-hashes.json` 差分空）。**4 つとも緑でない状態から実装を始めない**。この時点のテスト件数と変異の 実行/期待 件数を記録し、T036 の比較に使う。**実測（2026-08-17、main = d517a8b）: tsc clean / 286 tests / 変異 実行 164・期待 164・生存 0 / contract-hashes 差分なし**。**先に `cd vendor/codemem && corepack pnpm install --frozen-lockfile`**（CI と同じ。`npm ci` は lockfile が無いので失敗する）: `tsc` は `vendor/codemem/node_modules/.bin/tsc` にしか無く、worktree を作った直後は存在しない。入れずに走らせると ENOENT で落ち、**実装の赤と区別が付かない**

---

## Phase 2: Foundational（凍結 schema の差分・全 story を塞ぐ）

**Purpose**: 契約を先に固定する。ここが決まらないと実装が「何を書いてよいか」を決められない。

**⚠️ CRITICAL**: T004〜T009 が終わるまで、どの user story も着手できない

- [X] T004 `harness/schema/continuity.schema.json` の `PendingOperation` に任意欄 3 つを足す — `startIngestSeq`（`pattern: ^(0|[1-9][0-9]*)$` / `maxLength: 8192`、`lastIngestSeq` と同一制約）、`startTurnIdSource`（`$ref: "#/$defs/TurnIdSource"`）、`terminalFingerprint`（`type: string` / `maxLength: 8192`）。3 欄とも `required` に入れない。`additionalProperties: false` を維持する（data-model.md §1）
- [X] T005 `harness/schema/continuity.schema.json` に `$defs.DroppedEvidenceEntryV1` を足す — `reason`（`enum: ["evicted", "orphaned_terminal"]`・必須）/ `recordedAt`（`$ref: IsoTimestamp`・必須）/ `sensitivity`（`$ref: Sensitivity`・必須）/ `eventId`・`operationId`（`type: string` / `maxLength: 8192`・任意）/ `status`（`enum: ["started","succeeded","failed","unknown"]`・任意）。`additionalProperties: false`。**`oneOf` で分岐させない**（data-model.md §3 の理由）
- [X] T006 `harness/schema/continuity.schema.json` の `CanonicalWorkStateV1` に `droppedEvidence`（`type: array` / `items: {$ref: DroppedEvidenceEntryV1}` / `maxItems: 256`・任意）を足す（data-model.md §2）
- [X] T007 `harness/schema/continuity.ts` に T004〜T006 と同じ形を TypeScript で足す — `PendingOperation` の 3 欄は `?:`、`CanonicalWorkStateV1.droppedEvidence?: readonly DroppedEvidenceEntryV1[]`、`DroppedEvidenceEntryV1` 型を新設。**JSON 側と欄名・任意性・語彙が 1 文字でも違わないこと**。**逸脱**: `readonly` を付けず `droppedEvidence?: DroppedEvidenceEntryV1[]` にした。この file の配列欄は `pendingOperations: PendingOperation[]` を含め 1 つも `readonly` を使っておらず、ここだけ付けると新しい欄が別の規約で書かれたように読める
- [X] T008 `harness/continuity/schema-freeze.test.ts` に新しい欄と `$def` の凍結を足す — 存在すること、`required` に**入っていない**こと、`additionalProperties: false` であること、`maxItems` が 256 であること、`startTurnIdSource` が `TurnIdSource` を `$ref` していること（語彙の複製が入ったら落ちる）。既存ゲートが担う分は足していない: `additionalProperties: false` は「object はすべて closed」が、`$ref` の複製は「property の中に直接書かれた enum も凍結する」が拾う（inline enum に置き換えると凍結表と食い違って落ちる）
- [X] T009 契約 hash を再生成して差分を確認する（`node harness/contract-hashes.mjs > /tmp/contract-hashes.json` → `diff harness/contract-hashes.json /tmp/contract-hashes.json` → 一致させる）。**schema の hash だけが動き、fixture の hash は動かないこと**。fixture の hash まで動いたら T004〜T007 で意図しない場所を触っている

**Checkpoint**: 契約が固定された。`tsc` とテストが緑。この時点で実装はまだ 0 行

**実測（Phase 2 完了時）**: tsc clean / 288 tests（+2、いずれも schema-freeze）/ 変異 実行 164・期待 164・生存 0（還元器は未変更なので baseline と同数）/ `contract-hashes.json` は `schema/continuity.schema.json` の 1 行だけが動いた。schema への差分は挿入のみ（65 insertions / 0 deletions）— 凍結 schema は**生バイトが TS/Rust parity の signal** なので、JSON の round-trip で整形し直すと無関係な行まで hash が動く

---

## Phase 3: User Story 1 - 状態だけで権威順序を判定できる (Priority: P1) 🎯 MVP

**Goal**: `startIngestSeq` / `startTurnIdSource` を `PendingOperation` に載せ、凍結 schema の外の
索引（`operationStarts` / `OperationStartFactsV1` / `startFactsFor`）を消す。**実装が減る**段。

**Independent Test**: 状態と event だけを渡す経路（`reduceTaskWorkState` の第 3 引数に空 Map）に、
start より小さい連番の terminal を与えて隔離されること。外部索引を渡さずに済むこと。

### Tests for User Story 1

> 実装より先に書き、**落ちること**を確認してから T014 へ進む

- [X] T010 [P] [US1] `harness/continuity/reference-model.test.ts` に FR-001/FR-002 の回帰を足す — start 受理時に `startIngestSeq` と `startTurnIdSource` が `PendingOperation` に書かれること（spec Acceptance 1・2 に対応）
- [X] T011 [P] [US1] `harness/continuity/reference-model.test.ts` に FR-003 の回帰を足す — 同じ operation への**再配送 start** で 2 欄が**変わらない**こと（spec Acceptance 4）。遅れて届いた再配送の後、正当な terminal が順序違反で落ちないことまで見る
- [X] T012 [P] [US1] `harness/continuity/reference-model.test.ts` に FR-004 の回帰を足す — 2 欄を**持たない** `PendingOperation`（復元直後・旧い状態）に terminal を与えると `terminal_order_unverifiable` → `unknown` に落ちること。**材料が無いことを合格に読み替えない**
- [X] T013 [P] [US1] `harness/continuity/reference-model.test.ts` に SC-001 の回帰を足す — 索引を渡した場合と渡さない場合で、同じ状態・同じ event 列から**同じ判断・同じ状態**が出ること（spec Acceptance 3）。**書き換え**: T018 で索引そのものが消えるので「渡した場合」は書けない。SC-001 の主張を「同じ id の兄弟が並ぶ状態でも、各 pending が自分の材料で判定される」として固定した（側索引の頃はここが**原理的に判別できず**、両方 `terminal_order_unverifiable` に倒すしか無かった箇所）

### Implementation for User Story 1

- [X] T014 [US1] `harness/continuity/reference-model.ts` の start 受理経路で `startIngestSeq` / `startTurnIdSource` を書く。**新規 pending を作るときだけ**書き、既存 pending への再配送では触らない（FR-003）
- [X] T015 [US1] `harness/continuity/reference-model.ts` の §4.3 rule 1（権威順序）を `PendingOperation.startIngestSeq` から読むように付け替える。欄が無ければ従来どおり `terminal_order_unverifiable`
- [X] T016 [US1] `harness/continuity/reference-model.ts` の §4.3 rule 2（turn 種別の両立）を `PendingOperation.startTurnIdSource` から読むように付け替える。欄が無ければ従来どおり検査を免除
- [X] T017 [US1] `harness/continuity/reference-model.ts` から `startFactsFor()` を削除する。同名 pending を数えて曖昧なら材料なしに倒す分岐は、材料が要素に載った時点で発生しえない（research R-001）。**`terminal_order_unverifiable` へ倒す経路そのものは残す**（欄が無い状態のため）
- [X] T018 [US1] `harness/continuity/reference-model.ts` から `OperationStartFactsV1` 型と `TaskWorkStateSnapshotV1.operationStarts` を削除し、`reduceTaskWorkState` / `correlateTerminalEvent` / `finalizeAbandonedState` の引数から索引を落とす。退避時の索引同期（`operationStarts.delete(...)`）も一緒に消える
- [X] T019 [US1] 索引を渡していた既存の呼び出し側（テストの `new Map()` 引数を含む）を新しい signature に合わせる。**既存テストの期待値は 1 件も書き換えない**（SC-003）。書き換えが要るなら T014〜T018 のどこかで挙動を変えている

**T019 の逸脱（SC-003 を満たしていない 5 件）**: 「期待値を 1 件も書き換えない」は達成できなかった。書き換えたのはすべて**側索引の欠陥そのものを仕様として固定していた test** で、その欠陥を消すのが US1 の目的なので、挙動を意図せず変えた結果ではない。内訳:

| test | 旧 | 新 | なぜ変わるのが正しいか |
|---|---|---|---|
| 自 lineage で id が衝突しているとき | `terminal_order_unverifiable` | 診断ゼロで `succeeded` | 鍵が `operationId` だったので帰属を判別できず材料なしに倒していた。要素に載れば判別が要らない |
| 同名の兄弟が退避されたとき | 生存側も `unknown` | 生存側は `succeeded` | 退避のたび同名の材料をまとめて消していたので、**生き残った側の証跡まで失われていた** |
| 状態側で operationId が衝突 | `["unknown","started"]` | `["succeeded","started"]` | 同上。「1 件しか閉じない」という本来の主張は保っている |
| 同名兄弟の start facts で順序検査 | 両方 `terminal_order_unverifiable` | A の材料で A を判定（前なら `terminal_out_of_order`、後なら `succeeded`） | test 名どおり「他人の材料で検査しない」が、材料が要素に載れば**自分の材料で検査できる** |
| 名乗っている兄弟の 2 件（互換・非互換） | 末尾で `terminal_order_unverifiable` | 診断ゼロ | 旧 test のコメント自身が「これは #35 の欠落で、この test の主題とは無関係」と書いていた残渣 |

**実測（Phase 3 完了時）**: tsc clean / 294 tests（+6）/ 変異 実行 165・期待 165・生存 0 / `contract-hashes.json` は parity fixture の 1 行だけが動いた（還元器の出力が変わったので当然。**判断・診断・status は 1 件も変わらず hash だけが動いた**ことを diff で確認済み）/ `reference-model.ts` は 62 insertions・91 deletions で **29 行純減**

**変異ゲートの付け替え（data-model.md §7 の先決めどおり）**: 消えた機構の変異 3 件（退避の索引同期 2 件・側索引の曖昧判定 2 件のうち生き残る形に付け替えた分）を削り、#35 の規則を守る変異 4 件（start の連番を記録しない / turn 種別を記録しない / 再配送でも順序材料を書く = FR-003 / 空白を値として読む × 2）を足した。「空白の turn 種別を値として読む」は最初 fail 0 で生存したので、rule 2 の候補が空白で落ちないことを見る test を追加して kill した

**Checkpoint**: US1 が単独で成立。`tsc` / 全テスト / `mutate.sh` が緑で、**実装の行数が着手前より減っている**

---

## Phase 4: User Story 2 - 状態から消えた証跡が状態に残る (Priority: P2)

**Goal**: `droppedEvidence` に退避 operation と孤児 terminal を有界に記録し、追加と脱落を診断に出す。

**Independent Test**: 上限まで埋めた状態に新しい start を入れて退避を起こし、退避された
`operationId` が `droppedEvidence` に現れること。孤児 terminal も同様。

### Tests for User Story 2

- [X] T020 [P] [US2] `harness/continuity/reference-model.test.ts` に FR-005 の回帰を足す — 退避で `{reason: "evicted", operationId, status, recordedAt, sensitivity}` が 1 件足され、`sensitivity` が**退避元から引き継がれる**こと（spec Acceptance 1）
- [X] T021 [P] [US2] `harness/continuity/reference-model.test.ts` に FR-006 の回帰を足す — 候補 0 件の terminal で `{reason: "orphaned_terminal", eventId, recordedAt, sensitivity: "private"}` が足されること。孤児は相手が居ないので fail-closed の `private`（spec Acceptance 2）
- [X] T022 [P] [US2] `harness/continuity/reference-model.test.ts` に FR-007 の回帰を足す — 記録に payload・引数・出力・`description` が**入らない**こと。`DroppedEvidenceEntryV1` の欄集合そのものを固定する
- [X] T023 [P] [US2] `harness/continuity/reference-model.test.ts` に FR-008 の回帰を足す — 記録が 256 件のとき、**配列の先頭**が落ちて末尾に足されること。`recordedAt` で並べ替えると落ちるように、`recordedAt` を降順に仕込んだ fixture を使う
- [X] T024 [P] [US2] `harness/continuity/reference-model.test.ts` に FR-009 の回帰を足す — 追加で `dropped_evidence_recorded`、脱落で `dropped_evidence_overflowed` が診断に出ること
- [X] T025 [P] [US2] `harness/continuity/reference-model.test.ts` に FR-013/FR-014 の回帰を足す — `droppedEvidence` を**持たない**状態を読めて、退避が起きたら欄が新設されること（spec Acceptance 4）

### Implementation for User Story 2

- [X] T026 [US2] `harness/continuity/reference-model.ts` に `DiagnosticCode` の 2 値 `dropped_evidence_recorded` / `dropped_evidence_overflowed` を足す
- [X] T027 [US2] `harness/continuity/reference-model.ts` に記録の追加関数を 1 つ足す — 末尾に足し、`maxItems` に達していたら**先頭から 1 件落としてから**足す。落としたら `dropped_evidence_overflowed`、足したら `dropped_evidence_recorded`。`pendingOperations` の退避と**同じ規則**を使い、時刻で並べ替えない
- [X] T028 [US2] `harness/continuity/reference-model.ts` の退避経路（`retainPendingOperations`）から T027 を呼び、退避された各 operation を `reason: "evicted"` で記録する。`sensitivity` は退避元から引き継ぐ
- [X] T029 [US2] `harness/continuity/reference-model.ts` の孤児 terminal 経路（`terminal_orphaned`）から T027 を呼び、`reason: "orphaned_terminal"` で記録する。**隔離の判断自体は変えない**（記録は隔離の代わりではない）

**T029 の設計判断（tasks.md に無かった論点）**: 孤児 terminal の経路は **quarantine**（状態にも台帳にも入れない）だったので、「記録に残す」と「隔離の判断を変えない」は素直には両立しない。解いた形:

- **配送鍵は消費しないまま、記録だけ状態に入れる**専用の経路（`quarantineWithRecord`）を足した。`outcome` は `quarantined` のまま（`outcome === "quarantined"` で分岐して snapshot を捨てる消費者が harness にゼロであることを確認済み）
- 状態が変わる以上 **revision を採番し、`history` にも 1 件積む**。片方だけ動かすと、状態の revision が自分の履歴の末尾に無いことになる
- **同じ eventId の記録があれば足さない**。隔離は鍵を消費せず還元器は純関数なので同じ terminal は再送され続け、素で足すと記録が再送のたびに伸びて 256 件の枠を食い潰す（この repo の典型的な非収束）。重複時は既存の `quarantine` にそのまま落ち、**`dropped_evidence_recorded` も出さない**（記録していないので）
- 記録は operation が後から閉じても**消さない**。「その時点で相手が居なかった」という履歴であり、消す規則はどの FR も求めていない
- `terminal_conflict` では記録しない。あれは corruption であって「live な集合から落ちた証跡」ではない（#43 の語彙に無い）

**実測（Phase 4 完了時）**: tsc clean / 301 tests（+7）/ 変異 実行 174・期待 174・生存 0（#43 / #39 の変異を 9 件追加）/ `contract-hashes.json` は parity fixture の 1 行だけが動いた。**Phase 3 と違い、fixture は hash だけでなく診断と履歴長も変わっている**（孤児の行に `dropped_evidence_recorded` が付き、`historyLength` が 2 → 3）。これは FR-009 が着地した結果で、drift ではない

**既存の期待値の更新（7 件）**: 退避・孤児を通る既存 test は診断を網羅列挙していたので、`dropped_evidence_recorded` を足した。判断・status・outcome は 1 件も変えていない

**Checkpoint**: US1 と US2 が両方とも単独で成立。状態のサイズが上限で頭打ちになる（SC-005）

---

## Phase 5: User Story 3 - 配送 ID をまたいだ payload 衝突を見つけられる (Priority: P3)

**Goal**: 受理した terminal の `canonicalFingerprint` を `PendingOperation.terminalFingerprint` に
保持し、同じ成否・違う指紋・違う配送 ID の terminal を衝突として扱う。

**Independent Test**: 同じ operation を、同じ成否・違う payload・違う配送 ID で 2 回 terminal し、
2 通目が衝突として隔離されること。

### Tests for User Story 3

- [X] T030 [P] [US3] `harness/continuity/reference-model.test.ts` に FR-010/FR-011 の回帰を足す — 確定時に `terminalFingerprint` が書かれ、同じ成否・違う指紋・違う配送 ID の 2 通目が衝突として隔離され、**状態が変わらない**こと（spec Acceptance 1）
- [X] T031 [P] [US3] `harness/continuity/reference-model.test.ts` に「同じ指紋なら隔離しない」回帰を足す — 別の配送 ID でも指紋が同じなら適用済みの再配送として扱われること（spec Acceptance 2）
- [X] T032 [P] [US3] `harness/continuity/reference-model.test.ts` に FR-012 の回帰を足す — `terminalFingerprint` を持たない旧い状態では新しい検査が**発動せず**、従来どおり `terminal_already_applied` になること（spec Acceptance 3）

### Implementation for User Story 3

- [X] T033 [US3] `harness/continuity/reference-model.ts` の terminal 受理経路で、`status` を `succeeded` / `failed` に変えるときに `terminalFingerprint` へ event の `canonicalFingerprint` を**そのまま**入れる。**計算し直さない**。`unknown` へ倒す経路では書かない（受理していない）
- [X] T034 [US3] `harness/continuity/reference-model.ts` の再配送判定に指紋比較を足す — 確定済み operation に `terminalFingerprint` があり、届いた terminal の `canonicalFingerprint` が違えば衝突。欄が無ければ従来どおり `terminal_already_applied`

**Phase 5 で分かったこと（tasks.md に無かった論点）**:

- **`unknown` に倒した operation には指紋を書かない**。`unknown` は「成否を主張できなかった」= terminal を受理していないので、書くと後から届いた本物の terminal が指紋違いの衝突として隔離され、その operation は永久に閉じられない。変異と test で固定した
- **届く側の空白は考えなくてよい**。`canonicalFingerprint` は必須の identity 材料で、`assertIdentityMaterial` が空白を先に落とす（この検査には届かない）。空白がありうるのは**状態側**の `terminalFingerprint`（任意欄）だけなので、そちらを `declared()` で読む
- **既存 test 5 件の再配送に別々の指紋を振っていた**のを、受理済みの terminal と同じ指紋に揃えた。別値だったのは event を区別するための便宜で、「同じ論理 event の再配送」は canonical fingerprint も同じ（§8.2 が配送 ID 不在時の dedupe authority に使えるのはそのため）。期待値は 1 件も変えていない
- **新しい門が既存の門を隠した**: 成否矛盾ゲートを壊す変異 2 件が生存した。壊しても指紋ゲートが同じ `terminal_conflict` で塞いでいたため。該当 test の指紋を受理済みと同じにして、塞げるのが成否矛盾ゲートだけになるようにして kill した

**実測（Phase 5 完了時）**: tsc clean / 306 tests（+5）/ 変異 実行 179・期待 179・生存 0（#44 の変異 5 件を追加）/ `contract-hashes.json` は parity fixture の 1 行だけが動いた（`PendingOperation` に指紋が 1 欄増えたので hash が動く。診断・status・outcome は不変）

**Checkpoint**: 3 つの story がすべて単独で成立

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: fixture・hash・変異ゲート・正本を、実装が着地した形に合わせる。

**⚠️ 順序**: T035（fixture）→ T036（hash 再生成）→ T037〜T038（変異）の順。quickstart.md の
「hash 差分をテストより先」を守る。fixture を変えて hash を再生成し忘れると、ローカルは緑のまま
CI だけが落ちる。

- [X] T035 `harness/fixtures/continuity/` に新しい欄を持つ状態の fixture を足す。**旧い形（新しい欄を持たない）の fixture は 1 件も消さない** — FR-013/FR-014 の証拠がそこにある
- [X] T036 契約 hash を再生成し（`node harness/contract-hashes.mjs`）、差分が schema 2 件 + 新しい fixture 分だけであることを確認する。**手で書かない**（FR-016）
- [X] T037 `harness/continuity/mutate.sh` から、消えた規則を守っていた変異を削除する — `startFactsFor` の同名 pending 数え上げ、`operationStarts` の退避同期。アンカー切れで黙って飛ぶのではなく**明示的に消す**
- [X] T038 `harness/continuity/mutate.sh` に data-model.md §7 で先に決めた 6 つの変異を足す — FR-003（再配送でも `startIngestSeq` を書く）/ FR-004（欄が無いとき検査を素通り）/ FR-008（`recordedAt` 昇順で落とす）/ FR-009（脱落の診断を出さない）/ FR-012（指紋が無くても衝突扱い）/ FR-015（記録の上限検査を外す）。**各変異が「落ちるべきテスト」だけを赤にすることを 1 本ずつ確かめる**
- [X] T039 `mutate.sh` の 実行件数 / 期待件数 / 生存数 を突き合わせる。ずれたら `comm -23 <(期待の一覧) <(実行された一覧)` でアンカー切れを特定して直す（生存 0 かどうかより先に**実行件数**を見る）
- [X] T040 [P] `specs/001-agent-memory-core/resume-continuity-addendum-v6.2.md` §4.3 の文面を実装に一致させる（FR-017）— 「候補 0 件の terminal は unmatched evidence として保存する」を、実装がとる「隔離しつつ `droppedEvidence` に有界に記録する」に直す。3 つの検査が状態だけで実施できるようになったことも書く
- [X] T041 [P] `specs/001-agent-memory-core/resume-continuity-addendum-v6.2.md` §0.1 の Revision log に改定行を足す。節番号とファイル名は動かさない
- [X] T042 [P] `evidence/phase3-reference-model.md` に、索引方式（`operationStarts`）をやめた理由（R-001: `operationId` に一意性が無いので鍵にできない）と、新しい規則の根拠を記録する。§5 の件数（テスト数・変異の 実行/期待）を実測値に更新する
- [X] T043 5 つのゲートを通しで実行する — `tsc` / 全テスト / `mutate.sh`（生存 0・実行=期待）/ `contract-hashes.json` 差分が再生成と一致 / `git status` に未追跡の生成物が無い
- [X] T044 SC-003 を機械的に示す — `git diff origin/main -- harness/continuity/reference-model.test.ts` に**既存テストの期待値の書き換えが 1 件も無い**こと（追加のみ）を確認する。書き換えがあるなら、新しい欄が無い経路の挙動を変えている
**T044 の結果（SC-003 は満たしていない）**: `git diff origin/main -- harness/continuity/reference-model.test.ts` の削除行を数えると、期待値に触れた行が 37 ある。追加のみでは済まなかった。内訳と、それぞれが「新しい欄が無い経路の挙動を変えた」ものかどうか:

| 分類 | 件数 | 挙動の変更か |
|---|---|---|
| `operationStarts` の読み書きを要素の欄へ置き換え | 7 | いいえ（索引が消えた構造変更。同じ値を別の場所から読む） |
| 側索引の欠陥を仕様として固定していた test（T019 の表） | 5 | **はい。ただし意図した変更** — その欠陥を消すのが US1 の目的 |
| 診断の網羅列挙に `dropped_evidence_recorded` を追加 | 7 | いいえ（FR-009 が新しい診断を足した結果。判断・status は不変） |
| parity fixture を 2 本に一般化（loop 化） | 2 | いいえ（構造変更） |
| #44 で再配送の指紋を受理済みと揃えた | 5 | いいえ（**入力**の変更。期待値は不変） |

**この数え方では SC-003 を判定できない**（T049 で置き換えた）。書き換えた期待値の行を数えても、
「新しい経路が**新たに作る**状態」も「新しい test が初めて覆う挙動」も行として現れないので、
数え落としたまま緑にできる。上の表は「どの期待値になぜ触れたか」の記録としては残すが、
SC-003 の合否はここでは出さない。合否は T049 の差分ゲートが出す。

- [X] T045 `/code-review`（正しさ）→ `ponytail-review`（過剰実装）の 2 本立てを通す。この feature は Constitution III と `rules/security.md` の対象範囲（入力検証・fail-closed の境界）なので、**外部 CLI へ委譲せず Claude Code が実装しレビューする**

**T045 の結果**: `/code-review` が 14 件、並走レビューが 6 件を出した。重複を除いた 15 件を、
`review-routing` の批判的評価にかけて採否を決めた（**盲目的に採用しない**）。**再現できたものだけを
「実害」とした**——9 件は probe で実際に落として直し、5 件は性質としては正しいが実害が無いか
設計判断なので限界節へ書き、1 件は反証した。

| # | 指摘 | 検証 | 採否 |
|---|---|---|---|
| 1 | 指紋ゲートが兄弟の材料で判定し、混在状態で健全な再配送を隔離する（FR-012 違反・無限再送） | 再現（`terminal_conflict` / 鍵未消費） | **修正**（候補集合の判定に変更 + test + 変異 3 件） |
| 2 | `startIngestSeq` の綴り違いで還元器が throw し、無関係な terminal も巻き添え | 再現（`"007"` / `"-1"` / `"1 "` / `"12a"` の 4 形すべて） | **修正**（不在扱い + test + 変異 2 件） |
| 3 | 記録だけの隔離が §4.1 の watermark を進める | 再現（10 → 500・`ledger.size` 0） | **修正**（+ test + 変異 2 件） |
| 4 | `finalizeAbandonedState` が `droppedEvidence` の配列を過去 revision と共有（§4.2） | 再現（object 同一） | **修正**（`nextContent` に正規化を集約 + test + 変異） |
| 5 | 復元状態の空配列がそのまま残り、同じ意味の状態で hash が割れる | 再現（contentHash 不一致） | **修正**（同上 + test + 変異） |
| 6 | 上限超えの復元状態を刈った診断を start 経路が捨てている | 再現 | **修正**（+ test + 変異） |
| 7 | `DroppedEvidenceEntryV1` の `$comment` が存在しない runtime 検査を約束 | 目視（該当コード無し） | **修正**（文言を実態に合わせた） |
| 8 | status の語彙が 6 箇所に複製され、片方だけ増える変更が緑のまま通る | 目視（凍結 test は各コピーを個別に縛るだけ） | **修正**（凍結 test でコピー間を突き合わせ。schema は insert-only 制約のため $ref 化しない） |
| 9 | `outcome: "quarantined"` の doc が「状態も変えない」しか書いておらず、snapshot を捨てる移植が書ける | 目視 | **修正**（契約 doc に明記） |
| 10 | 孤児の重複判定の走査が分岐の外にあり、非孤児でも 256 件を舐める | 目視 | **修正**（分岐の内側へ） |
| 11 | 上限超えの復元配列を terminal 経路が運び続ける | 再現 | **限界へ**（`nextContent` に診断の出口が無く、黙って刈るのは「黙って間引かない」に反する） |
| 12 | 孤児の記録が上限をまたぐと再び足され revision が動く | 再現 | **限界へ**（`history` を見れば厳密だが上限の無い走査になる。枠の中では収束する） |
| 13 | 復元状態の順序材料を検証していない（偽の `startIngestSeq` が通る） | 再現 | **限界へ + test で固定**（同じ状態を書ける相手は `status` を直接書ける。daemon 側の信頼境界） |
| 14 | `DroppedEvidenceEntryV1` の欄の組み合わせが無検査 | 目視 | **限界へ**（`oneOf` は別言語の validator で挙動差。読む側が `reason` で分岐する） |
| 15 | 退避された operation の指紋が記録に残らない | 目視 | **限界へ**（#44 の前から候補ゼロで孤児隔離だったので差が無い） |
| — | `nextContent` の配列複製が冗長 | 反証（複製は §4.2 の guard 本体。実害は `finalizeAbandonedState` 側の**欠落**） | **却下**（#4 として逆向きに採用） |

**ponytail-review**: `startIngestSeqOf` から重複した `declared()` を落とし（pattern 検査が空白も
弾くため）、`quarantineWithRecord` の事前複製を削除、test helper の手写しを `withoutStartFacts` に
寄せた。過剰な抽象の新設は無し。

**semgrep / codex-review mode=security / codex:adversarial-review**: 下の T047 で実施。

- [X] T046 `speckit-verify-tasks` を 1 回通し、`[X]` に実装が伴っているかを確認してから PR を作る

**T046 の結果**: `specs/002-continuity-state-evidence/verify-tasks-report.md`（41 VERIFIED /
2 PARTIAL / 1 SKIPPED）。レポートは `speckit-verify-tasks` の immutability 規則どおり
**生成時のまま**にしてある。生成時点は commit `cdebef8` で、その後の `0d6b4cd` 以降が
数字を動かしている（変異 179 → **187**、tests 307 → **315**、実装は上の T045 表のとおり）。
PARTIAL 2 件（T019 / T029）の walkthrough は**実施済み**で、どちらも調査（investigate）の
うえ判定は **PARTIAL のまま**にしてある（レポート本体の再採点は immutability 規則で禁じられて
いるため）。処置は同レポートの `## Walkthrough Log` に追記した。T019 が根拠にしていた
「期待値の書き換え行数」は SC-003 の判定材料としては T049 の差分ゲートが引き継いでいる。

- [X] T047 `rules/security.md` の必須ツールを通す — semgrep CLI・`/codex-review mode=security`・
  `/codex:adversarial-review`。この feature は入力検証と信頼境界（復元状態の扱い）に触るので
  対象範囲に入る。指摘は同じく批判的評価で採否を決める

**T047 の結果**: semgrep 2 件（どちらも本ブランチが触っていない `harness/schema/validate.ts` の
`detect-non-literal-regexp`。対象は repo 自身の凍結 schema の pattern なので攻撃者制御ではない → 却下）。
`codex-review mode=security` が `ok: false`（high 1 / medium 1）、`codex:adversarial-review` が
blocking 4 / advisory 2。重複を除くと 6 件。**実行して再現できたものだけを実害とした**。

| # | 指摘 | 検証 | 採否 |
|---|---|---|---|
| S1 | 孤児の重複判定が `eventId` — 再配送は eventId が変わるので同じ terminal が何度でも記録され、記録が上限で頭打ちになった後も revision と history が伸び続ける（配送鍵未消費なので再送は止まらない = DoS） | **再現**（同一 adapterDeliveryId・同一 fingerprint の 300 再送 → 300 revision / history 300 / 台帳 0） | **修正**（鍵を `canonicalFingerprint` に変更。凍結 schema へ任意欄 1 つ追加。300 再送 → 1 revision に収束。回帰 test + 変異 3 件） |
| S2 | `quarantined` が 2 通りの状態挙動を持つのに契約が 1 つしか書いていない | 目視（消費者は harness 内ゼロだが型は export 済み） | **修正**（addendum §4.1 に「outcome は配送鍵の話であって状態の話ではない。どの outcome でも返った状態を採る」を MUST として明記。新しい outcome 値の追加は export union の破壊なので採らない） |
| S3 | 指紋を持たない候補が 1 件あれば集合全体の衝突検査が無効化される（囮攻撃） | **反証**（囮を作れる経路を数えた） | **却下 + test で固定**。還元器は閉じるとき必ず指紋を書く。書かないのは `unknown` だけで、`unknown` は `isOpen` が open として数えるので衝突検査の分岐に到達しない。残るのは**状態を書ける相手**だけで、その相手は囮を置くより先に `status` を直接書ける（新しい能力を与えない）。可用性側の失敗（無限再送）は永久に収束しないので FR-012 の選択を変えない |
| S4 | 記録が上限で押し出されたことが状態だけを読む側に分からない。退避と孤児が同じ FIFO なので片方の洪水でもう片方の証跡を消せる | 目視（設計どおり） | **限界へ + issue #61**。閉じるには状態側の scalar が要り、canonical hash の面を変えるので別に切る |
| S5 | 識別子と指紋が生値で状態に入り機密度が `private` 固定。診断も値を出す | 目視（**以前から**。`Observed*.sourceEventIds` が同じ値を同じ機密度で保持している） | **限界へ + issue #62**。#43/#44 が作った露出ではなく、閉じるには event 表面全体の規約が要る |
| S6 | `DroppedEvidenceEntryV1` の欄の組み合わせが無検査 | 目視 | **限界へ**（T045 #14 と同じ。`$comment` に明記済み） |

**PR bot（Sourcery）の 2 件**:

| 指摘 | 判定 |
|---|---|
| 孤児の重複判定が `reduceTaskWorkState` の条件式に直書きされていて、次に記録を足す経路がそれを持たずに書ける。`recordDroppedEvidence` に寄せるべき | **採用**。判定を helper に移し、`added` を返して呼び出し側は「実際に足せたか」だけを見る形にした（足せていなければ状態を変えない隔離に倒す）。T045 #9 と `codex:adversarial-review` も同じ構造を指摘しており、3 者一致 |
| `DroppedEvidenceEntryV1` の `(reason, eventId, operationId, status)` の組み合わせを runtime で検査すべき | **却下**。新しい行を書くのは還元器だけで、理由ごとに書く欄はそこで決まっている（構成で満たしていて、表明で確かめる対象が無い）。検査したい相手は**復元した状態**だが、それは還元器が検証しない側（T047 S5 と同じ信頼境界）。重複判定を helper に寄せた結果、理由ごとの扱いを知っている場所も 1 箇所になった |

**S1 の副産物**: 凍結 test が schema と TS mirror の**欄集合を突き合わせていなかった**ことが判明した
（TS 側から `terminalFingerprint` を消しても凍結 test 17 件が全部 green のまま）。TS 型は object
literal の注釈としてしか出てこず、値は `unknown` 経由で validator に渡るため余剰プロパティ検査が
働かない。`SameSet` の型表明と欄名比較の test を足して塞いだ（tsc がゲート）。

- [X] T048 PR #60 に付いたレビュー（bot のインライン指摘・codex の最終レビュー）を批判的評価にかけ、採否を記録する

**T048 の結果**: `chatgpt-codex-connector` の P2 が 3 件、codex の最終レビューが blocking 1 件 +
advisory 1 件。**還元器を実際に走らせて再現したものだけを実在として扱った**。

| # | 指摘 | 再現 | 採否 |
|---|---|---|---|
| P2-1 | 上限超えの復元配列を、退避が無い start でも刈るのは §4.3 の文面（「次に追記する event で修復」）と食い違う | した | **文面を採用側にして修正**。刈らずに運ぶと自分の凍結 schema に反する状態を revision ごとに出し続けることになる。既存 test 1735 がこの挙動を理由付きで固定している。FR-017 がどちらかを動かすことを求めるので、動かす側を文面にした。刈らない経路（operation を閉じる terminal・放棄）は限界に明記 |
| P2-2 | revision 間で `droppedEvidence` の要素 object を共有している | した（コード上明白） | **却下（#63 として切り出し）**。1 行上の `pendingOperations: [...pendingOperations]` が以前から同じ形で、還元器はどの経路でも要素を書き換えない。片方だけ deep clone すると「塞いだように見えて半分」になる |
| P2-3 | 開いた候補が 1 件も無い `terminal_unmatched` が記録されず状態から消える | した（既存 test 3912 の経路） | **採用**。記録分岐の条件に足し、変異 2 件と回帰 test で固定。§4.3 の「候補が 1 件も無い」は狭すぎたので「保持できる相手が居ない」に改めた |
| C-blocking-1 | 孤児の同一性を `canonicalFingerprint` だけで見るので、同一 `adapterDeliveryId` で指紋を変えると DoS が再現する | した | **増幅としては却下**。同一配送鍵で指紋を変える 300 再送と、配送鍵ごと変える 300 再送が**完全に同じ** revision 300 / history 300 / 記録 256 / 台帳 0 だった。測定を返したところ再レビューで撤回された（`ok:false` は下の別件） |
| C-blocking-2 | 同じ鍵の**逆向き**: 配送鍵も `operationMatchKey` も違う 300 件の孤児が、同じ指紋を名乗るだけで記録 1 件に潰れる | した（history 1 / 記録 1 / 台帳 0） | **採用**。`canonicalFingerprint` は adapter が算出して wire で運ぶ値なので単独では同一性の権威にならず、「配送 ID が違えば同じ指紋でも別の論理 event」は §8.2 の規則として台帳側が既に守っていた（`reference-model.test.ts` に明示 test がある）。記録にも `adapterDeliveryId` を持たせ、鍵を §8.2 の順（配送鍵 → 指紋、keyspace は `d:`/`f:` で分割）にした。**同一配送・指紋変動の再送もこれで 1 件に収束する**ので、C-blocking-1 の経路も塞がった |
| C-advisory-2 | 「記録経路では何も足さなくても刈る」という新文面が、重複だった再送では成り立たない（257 件のまま） | した | **採用（文面）**。start は常に刈るが terminal は記録を足したときだけ、と狭めた。重複の再送は「状態を変えない隔離」に倒す設計なので刈った結果ごと捨てられる |
| C-advisory-3 | 凍結 test は欄名しか見ておらず、TS 側の値型が `string \| number` に広がっても通る | した | **採用**。相互代入可能性で挟む型表明と、schema 側の `type`/`maxLength` 検査を足した |
| C-advisory | 凍結 test が `terminalFingerprint` の型と `maxLength` を見ていない | した | **採用**。string が通る / number が落ちる / 8193 文字が落ちる、を追加 |
| P2-4 | 上限超えの復元配列を持つ状態へ**重複**の孤児を再送すると、刈った結果ごと捨てて 257 件のまま返す | した | **採用（コード側）**。前ラウンドでは文面を狭めて residue にしたが、同じ箇所を 2 人目の reviewer も指摘した。「凍結 schema に反する状態を出さない」は退避ゼロの start で刈る根拠そのものなので、terminal 側だけ例外にする理由が無い。刈りが起きたら記録が増えなくても修復した状態を返す形にした（刈りは 1 度で収まるので収束する） |
| P2-5 | 同名 `operationId` の兄弟が並ぶ状態で退避が起きると、記録が id と status しか持たないので**どちらが落ちたか**分からない | した（コード上明白） | **採用**。凍結 schema が `operationId` に一意性を課さないことは US1 の前提そのもので、その状態でだけ FR-005 の監査記録が意味を失う。既存の `eventId` 欄に start の event id を入れた（欄は増やしていない。`eventId` は「この記録を名指す event」で、理由によって指す先が違う） |
| P2-6 | evidence の孤児記録の説明が「同じ eventId は 2 度足さない」のまま | した | **採用（文面）**。§8.2 の順に直した |
| P2-7 | data-model が「組み合わせの妥当性は runtime で検査する」と書いているが、そんな層は無い（schema の `$comment` は「検査しない」と書いている） | した | **採用（文面）**。移植先が検査を足すと参照実装が運ぶ状態を移植先だけが拒否して parity が崩れる、という理由まで書いた |
| C-blocking-3 | 配送鍵を第一 authority にした結果、**同一配送鍵・指紋違い**の 2 通目が黙って重複扱いになり、`delivery_conflict` が消えた | した（診断は `terminal_orphaned` のみ、保存指紋は初回値のまま） | **採用**。件数が 1 に収束することは満たしていたが、整合性検査まで消していた。記録済みの鍵から指紋も引ける形にし、鍵一致・指紋違いは記録も状態も鍵も動かさず `delivery_conflict` を出す。どちらかが指紋を名乗っていなければ発動しない（FR-012 と同じ） |
| C-advisory-4 | data-model の欄表に `terminalFingerprint` / `adapterDeliveryId` が無く、`reason` ごとの説明も古い | した | **採用（文面）**。2 つの鍵・§8.2 の優先順位と keyspace 分離・corruption の扱いまで書いた |
| C-blocking-4 | 退避記録の start を `sourceEventIds[0]` から取っているが、start を名指す正本は `correlation.startEventId`（凍結 schema の必須欄） | した（`startEventId="actual-start"`・`sourceEventIds=["later-event","actual-start"]` の復元状態で別 event を start として記録） | **採用**。`sourceEventIds` は append-only の provenance 配列で、schema は先頭が start であることも順序も保証しない。**自分が追加した test も `sourceEventIds` しか変えておらず、誤った authority を固定していた**（`correlation.startEventId` も変える形に直し、並べ替えた状態で専用欄を読むことを別途固定した） |
| C-advisory-5 | 上限修復の収束は、重複孤児が配列**先頭**にあると 1 revision では終わらない | した（1 回目が孤児ごと刈り、2 回目が再記録、3 回目で差分ゼロ） | **採用（文面 + 回帰）**。正本と evidence を「最大 2 revision で収束」に狭め、先頭配置の回帰 test を足した。既知の「上限をまたぐと再び足されうる」と同じ境界 |

**独立最終レビュー（新規セッション。resume は追認バイアスがかかるため別セッションで実施）**: blocking 3 件。

| # | 指摘 | 再現 | 採否 |
|---|---|---|---|
| F-1 | 孤児 F1 を記録した後に start が届くと、同じ配送鍵の F2 再送が照合経路へ進んで衝突検査を素通りする | した（`applied` / 診断ゼロ / operation は succeeded / 指紋は F2、記録は F1 のまま） | **採用**。隔離は配送鍵を消費しないので、この経路は start の到着順しだいで検出されたりされなかったりしていた。記録済み孤児の指紋を還元器の**入口**で引く形にした（台帳の衝突検査と同じ位置）。**副産物**: 記録側に置いていた同じ検査が到達不能になったので畳んだ（同じ規則が 2 か所にあると片方だけ直る） |
| F-2 | Node 24.16 では `node --test <file>` が file 全体を 1 test として報告するので、変異ゲートの「test を走らせていない変異」検査が機能せず、evidence の 214/214 も再現しない | **しなかった** | **却下（実測）**。同じ Node v24.16.0 で `node --test` は 214 件、`--test-isolation=none` 付きも 214 件で一致した（`--experimental-strip-types` の有無も結果を変えない。24.16 は型注釈を素で剥がす）。指摘者が見た 1 件は load 失敗そのもので、まさにこの検査が拾う形。ゲートは baseline と実行数を同じコマンドで突き合わせるので、報告方法が変わっても両方が同じだけ動く |
| F-3 | 正本の型ブロックに新しい任意欄・`DroppedEvidenceEntryV1`・`droppedEvidence` が無く、正本だけから移植すると §4.3 の三検査が実装不能 | した（ブロックを目視） | **採用**。§4.1 のブロックに JSON schema と同じ形で足した。FR-017 が求めているのはまさにこの一致 |

**`92d01ec` への bot 指摘（chatgpt-codex-connector、2 件）**:

| # | 指摘 | 再現 | 採否 |
|---|---|---|---|
| B-1 | 記録側の衝突検査は terminal が未照合のあいだしか走らないので、孤児から照合済みへ移ると `delivery_conflict` を素通りして operation を閉じ、配送鍵まで消費する | **しなかった（対象が古い）** | **却下（済み）**。`original_commit_id` が `e3fdd31` で、これは `92d01ec` が直した内容そのもの。現在の検査は phase 分岐より**前**（`reduceTaskWorkState` 入口、台帳検査の直後）にあるので、照合されうる terminal も同じ検査を通る。回帰: 孤児 F1 → start → 同じ配送鍵で F2 → `quarantined` / `["delivery_conflict"]` / operation は `started` のまま / 台帳サイズ不変。対照として同じ指紋の F1 は `applied` / `succeeded` |
| B-2 | `OperationCorrelationV1.startEventId` は schema に `minLength` が無いので空文字が届きうる。`declared()` がそれを「名乗っていない」と扱うため、退避の記録が `eventId` を持たず、同名の兄弟が並ぶと落ちた側を特定できない | した（材料が無いので当然そうなる） | **却下（限界として明記）**。代替の識別子が無い: `sourceEventIds[0]` は C-blocking-4 で差し戻した誤った authority そのもの、`nativeOperationId` は任意欄で記録側に置き場も無い。復元状態を検査して弾くのも取らない——「状態は権威であって検査しない」がこの還元器の立場で、`startIngestSeq` の綴り検査を**例外として**明示しているのと同じ理由（`startEventId` を空白で書ける実装は `status` も直接書ける）。evidence の限界に追記した |

**`92d01ec` への bot 指摘 第 2 波（同じく chatgpt-codex-connector、4 件。すべて `92d01ec` 起点）**:

| # | 指摘 | 再現 | 採否 |
|---|---|---|---|
| B-3 | 復元状態の open な候補（`started` / `unknown`）が既に `terminalFingerprint` を持つとき、違う指紋の terminal がその候補を閉じながら指紋を黙って上書きする。data-model の「一度書いた欄は上書きしない、食い違いは衝突」に反し、先の terminal の唯一の証跡が消える | した（`applied` / 診断ゼロ / `succeeded` / 指紋は F2 に化け、F1 は状態から消えた） | **採用**。確定済みの候補には同じ検査があったが、**open な候補には無かった**——[[defect-shape-closure-needs-second-axis]] の形そのもの。照合が候補を 1 件に決めた直後（`open.length > 1` は既に `terminal_ambiguous`）に、上書きする相手そのものだけを見る検査を足した。**台帳を消費する 2 つの順序分岐より先**に置く（後ろだと順序の穴を通って `unknown` に化け、訂正版が重複 no-op で消える）。入ってくる側が `unknown` に倒れる経路では発動しない（指紋を書かないので上書きが起きず、判定は event 自身の性質なので隔離すると永久に閉じない）。正本 §4.3 に「候補が 1 件に決まる場面では要素単位で判定する」を追記し revision 行を足した |
| B-4 | evidence の限界が「上限超えの `droppedEvidence` を repair するのは start 経路だけ」と書いているが、記録を生む terminal も `recordDroppedEvidence` を通り、記録が重複でも刈った状態を返す | した（文面と実装を突き合わせ） | **採用（文面）**。正本 §4.3 は既に「repair はそれを行う経路（every start, every recorded terminal）で述べる」に直っており、**evidence だけが古かった**。記録を生む terminal と operation を閉じるだけの terminal を書き分けた |
| B-5 | evidence 冒頭の実装対応表が、`turnIdSource` を側索引 `TaskWorkStateSnapshotV1.operationStarts` に持ち退避時に delete せよと今も指示している。この PR は索引も型も消したので、表に従うと非直列化の設計を作り直して復元後のパリティを失う | した（`operationStarts` は harness に 1 箇所も残っていない） | **採用（文面）**。指示になっている行（対応表・退避の説明・`terminal_order_unverifiable` の条件・再配送の規則・信頼境界）を要素の 2 欄に直した。§3 の「索引方式をやめた理由」は経緯として残し、当時の設計を語る箇所はそうと分かる書き方にした |
| B-6 | spec.md の受入シナリオが「連番 50 の terminal は順序違反として隔離される」と書いているが、還元器は `terminal_out_of_order` で `unknown` に倒し配送鍵を消費する | した（コードと正本の両方を確認） | **採用（文面）**。**正本を先に読んで権威を決めた**: §4.3 が隔離を課すのは correlation / hash の衝突で、順序違反は「zero か複数の open にマッチした terminal は何も閉じず候補を `unknown` にして診断を出す」側に落ちる。連番の前後は event と状態だけで決まる定常的な性質なので、隔離すると鍵が消費されず無限再送になる。よって**古かったのは spec.md** で、正本の改訂は不要 |

**`80aa222` の再レビュー（`/codex-review` resume）**: `ok: true`、blocking ゼロ、advisory 2 件。どちらも採用した。

| # | 指摘 | 再現 | 採否 |
|---|---|---|---|
| A-1 | 追加した 3 test は既定 fixture（`nativeOperationId` あり）しか使っておらず rule 1 しか通らない。衝突条件を `rule === "native_operation_id"` に限定しても 217 test 全件が通り、rule 2 だけ検査を失う変異が現ゲートを生存する | **レビュアーが変異で実測**（隔離 archive） | **採用**。`MATCH_KEY_ONLY` から作った rule 2 の組で発火・同指紋の通過・`unknown` 非発火を固定し、rule 1 限定にする変異を `mutate.sh` に足した。**自分の変異ゲートが自分の新分岐の半分を測っていなかった**——[[surviving-mutation-means-narrow-test]] の裏返しで、生存しない変異しか書いていなかった |
| A-2 | spec.md の FR-010 / FR-011 は「確定した operation」の指紋だけを規定しているのに、正本・data-model・実装は復元された `started` / `unknown` にも要素単位の規則を足した。新 test の FR ラベルと feature spec の追跡関係が食い違う | した（文面を突き合わせ） | **採用（文面）**。FR-011a として、open な候補が 1 件に決まる場面の衝突と、`unknown` に倒れる terminal では発動しないことを spec.md に足した |

**`80aa222` への bot 指摘 第 3 波（3 件、いずれも文書の古さ）**: すべて採用。

| # | 指摘 | 再現 | 採否 |
|---|---|---|---|
| B-7 | spec.md の Assumptions が「この spec の範囲で版を上げる」と書いているが、`research.md` は型名の版を上げず `schemaVersion` も `1` 据え置きと決めており、schema の `const: 1` もそのまま。従うと存在しない V2 契約を期待する移植が出る | した（3 文書を突き合わせ） | **採用（文面）**。Assumptions を「契約 hash を作り直す・版は上げない」に直し、理由（連鎖して checkpoint の版まで動くのに読み手が居ない／drift 検出は `contract-hashes.json` が担う）と、**本物の V2 が要る条件は plan.md にある**ことを併記した |
| B-8 | SC-003 が「既存の状態に対する挙動が 1 件も変わらない（既存 test が 1 件も書き換わらないことで示す）」と約束しているのに、`verify-tasks-report.md` の T019 は PARTIAL で、挙動が変わった期待値 5 行を記録している | した（自分のレポートが反証になっている） | **採用（文面）**。**基準を緩めるのではなく除外を明示する**形にした: #35 / #39 / #43 / #44 が名指しした欠陥の修正だけを除外し、その中身は T019 の内訳（37 行のうち 5 行）で数えて示す。挙動を保つとその 4 件は「欠陥を仕様として固定する」ことになるので、除外しないと基準自体が矛盾する |
| B-9 | evidence §2.10 付近に「`CanonicalWorkStateV1` に unmatched evidence の置き場が無く frozen schema のままでは正本どおりに実装できない、schema 側の穴として起票する」という段落が残っている。#43 で置き場ができた後なので、従うと記録を省いた移植ができ状態も revision hash も一致しない | した（段落を目視） | **採用（文面）**。削除せず「#43 で解消済み・経緯として残す」と明示し、現在の置き場（`droppedEvidence`）と、移植は §2.10 と正本 §4.3 に従うことを書いた |

**`2bd2216` への bot 指摘 第 4 波（8 件）**: **再現したものだけを実害とした**。P2 の 2 件は再現したが、どちらも「この PR が作った欠陥」ではなく境界の広い穴だったので issue に切り出した。

| # | 指摘 | 再現 | 採否 |
|---|---|---|---|
| C-1 | open な候補の `sourceEventIds` が 256 件で満杯だと、`unresolved` が空でないので記録の分岐に入らず、append も上限で拒まれ、terminal の identity が状態に残らない | **再現**（`applied` / `[terminal_out_of_order, source_events_truncated]` / status `unknown` / 記録ゼロ / **台帳 1 = 鍵は消費**） | **issue #68 へ + 限界へ**。判定を「保持する相手が居るか」から「実際に保持できたか」へ移す必要があり、correlate と apply の間で truncation の結果を戻す構造変更になる |
| C-2 | 8192 文字を超える識別子が検査されずに状態へ写り、還元器が自分の凍結 schema に反する状態を返す | **再現**（孤児記録で違反 2 件、start の `eventId` 経由で 4 件）。**後者は変更前から同じ** | **issue #69 へ + 限界へ**。既存の穴が新しい欄に広がった形。識別子は切り詰められない（別の値になり重複判定が誤って一致する）ので入口の fail closed 設計が要る |
| C-3 | `nextContent` が `droppedEvidence` を上限なしで引き継ぐので、257 件の復元状態が刈られないまま次の revision へ残る | — | **却下（既出）**。T045 の #11 で同じ形を限界として記録済み。`nextContent` に診断の出口が無く、黙って刈るのは「黙って間引かない」に反する。#61 が state-visible な summary で扱う |
| C-4 | `recordDroppedEvidence` の JSDoc が `orphanKeyOf` の 2 つ目のコメントとして解釈され、関数が説明を持たない | 目視（宣言の間に何も無い） | **修正**（ブロックを関数の直前へ移した） |
| C-5 | addendum の改定行が未来日付（`2026-08-18`）になっている | 反証（当日が 2026-08-18） | **却下**（日付は正しい） |
| C-6 | plan.md が「5 本のゲート」、quickstart.md が「6 本」 | 目視 | **修正**（quickstart に合わせた） |
| C-7 | quickstart の手動確認が、削除した `operationStarts` を渡さない前提で書かれている | 目視 | **修正**（US2 シナリオ 4 は T049 の差分ゲートが自動で見るようにした） |
| C-8 | research.md R-005 の欄の列挙が実装より狭い（`terminalFingerprint` / `adapterDeliveryId` / `sensitivity` が抜けている） | 目視 | **修正**（同一性の材料であって内容ではない、という理由も併記した） |

`ponytail-review`: 実装差分は条件 1 つとコメントのみで削るものなし。test 側の 300 周ループは
「鍵の選択による増幅ではない」ことを示すのに 256 件の飽和を跨ぐ必要があるので残す。

**Codacy**: 12 件のうち 8 件を反映（うち 1 件は欄集合の縛りを両方向にする改善になった）。残る
指摘は `80aa222` 時点で 9 件あり、すべて `schema-freeze.test.ts` の同じ 3 種類:

- `node:test` の `test(...)` の floating promise（5 件）。このリポジトリの約 220 箇所すべてが同じ形。
- `Generic Object Injection Sink`（2 件）。`properties[field]` は test 内のリテラル 2 語の配列を
  回しているだけで、外部入力は入らない。
- `Unnecessary optional chain on a non-nullish value`（2 件）。型の上では `Record<string, T>` の
  index access が常に present に見えるだけで、実際には欠けうる。`?.` を外すと**欄が無いときに
  TypeError で落ちて**、この test が名指ししたい「どの欄の何が違うか」が出なくなる。fail closed の
  読み方を優先して残す。

- [X] T049 SC-003 を機械的に判定できる形にする — 旧形（新しい任意欄を 1 つも持たない）状態と
  event の corpus を `harness/fixtures/continuity/old-shape-parity.json` に置き、この branch の
  分岐点 `d517a8b` の実装に通した結果を baseline として commit する。`harness/continuity/old-shape-parity.test.ts`
  が同じ corpus をこの実装に通し、食い違う JSON path を許可表と突き合わせる

**T049 の結果**: corpus **20 case / 29 step**。差分が出たのは **13 step** で、残り **16 step は
変更前と完全に一致**した。許可表は case 名・event・path・**値**・issue 番号を持ち、表に無い差分でも、
表にあるのに出ない差分でも落ちる。実測した差分は次のとおり:

| 差分 | issue | 中身 |
|---|---|---|
| `pendingOperations[].startIngestSeq` / `startTurnIdSource` | #35 | start が権威順序と turn 種別を状態に載せる |
| `pendingOperations[].terminalFingerprint` | #44 | 閉じた terminal の指紋が状態に残る |
| `droppedEvidence` / `diagnostics` / `updatedAt` / `sensitivity` | #43 | 孤児 terminal と退避を状態に記録する（記録は機密度の下限を上げる） |
| 再送後も `droppedEvidence` が 1 件のまま | #39 | 再起動で台帳が空に戻っても記録側の鍵が 2 件目を止める |
| `state.stateRevision` / `history[].revision` / `history[].contentHash` | 上記の従属 | 状態が動いた step では hash も動く。**状態が動かない step では hash も一致している** |
| `diagnostics[0].detail` のみ（`restored-start-redelivery-ledger-miss`） | #35 | 重複と判定した根拠の文面が、側索引を指す表現から状態の要素を指す表現に変わった。判断・状態・台帳・hash はすべて一致 |

最後の 1 行は**比較面を広げて初めて見えた差分**である。以前の比較面は診断を `code` だけに縮約して
いたので、`detail` の変化は差分として現れなかった（codex のレビューが隔離環境で
`terminal_order_unverifiable` の `detail` を壊しても全 test が緑のままだと実証した）。いまの比較面は
還元結果を素通しし、形を変えるのは台帳（`Map`）を鍵で整列した配列に直す 1 か所だけにしてある。

**corpus が届いていない経路**（黙って間引かない。`old-shape-parity.test.ts` の冒頭に同じ列挙がある）:

- 同じ session 内で `operationId` が衝突する 2 件を作る経路: start の再配送判定が derived id と
  native id の両方を見るので、`duplicate_operation_start` か `start_conflict` のどちらかに倒れる。
  状態側の衝突（復元）は corpus が通す
- `assertSameScope` が投げる入力: 例外は還元結果を返さないので、この比較面では旧新を区別できない

**T019 が名指しした 5 件との対応**: 「同名の兄弟」経路は `restored-collided-siblings-terminal` /
`restored-collided-siblings-eviction` / `restored-cross-lineage-twin` の 3 case が通す。上限 256 件の
退避（#43 の reason `evicted`）は 2 番目の case が通すようになったので、以前ここに書いていた
「corpus が届いていない」は解消した（256 件は generator が組み立てる。手で JSON に書かない）。

**`verify-tasks-report.md` との関係**: 同レポートは `speckit-verify-tasks` の immutability 規則で
**生成時点のまま**であり、T019 の PARTIAL 判定もその時点の記録として残す。SC-003 の現在の合否は
T049 の差分ゲートが出す（レポートを書き換えて合否を移し替えることはしない）。

`.codacy.yml` の `include_paths` は `harness/**` を含んでいないので、そもそも範囲外のはずの
経路が解析されている。必須チェックではないため PR にはこの判断を記録し、設定側の話は #64 に分けた。

**独立最終レビュー 第 2 波（`4ff531d` に対する新規セッション。blocking 4 件）**: 2 件を採用、1 件を実測で却下、
1 件は文書の古さとして採用した。

| # | 指摘 | 再現 | 採否 |
|---|---|---|---|
| G-1 | `startIngestSeq` だけを持つ schema 通りの復元状態に、`nativeOperationId` を名乗らない terminal が `turnIdSource: "synthesized_monotonic"` を主張すると、状態側に照合材料が無いまま rule 2 が閉じる | した（`applied` / 診断ゼロ / `succeeded`） | **採用**。`eligibleOf` が「材料が無い候補を落とさない」のは帰属を取り違えないためで正しいが、絞り込みを抜けた候補には「一致した」と「確認できなかった」が混ざる。**閉じる直前で分ける**新しい門を足し、後者は `terminal_turn_unverifiable` で `unknown` に倒す（順序側の `terminal_order_unverifiable` と対称）。順序の 2 分岐より**後**に置いたので、2 欄とも欠く復元直後の状態はこれまでどおり順序側の診断で観測される。正本 §4.3 の「exemption は候補選びを支配するのであって判定ではない」を明文化し revision 行を足した。PR 本文の見出し「Absent material is never a passed check」が偽だった箇所そのもの |
| G-2 | `--test-isolation=none` 無しでは各 test file が 1 件として数えられるため、変異ゲートの実行件数照合が成立しない | **しなかった** | **却下（実測）**。CI と同じ Node v24.16.0 で `node --experimental-strip-types --test harness/continuity/*.test.ts` は `tests 330 / pass 330`、`--test-isolation=none` を付けても同じ 330。`mutate.sh` の baseline も 2 file で 220 を報告しており、file 単位の 2 ではない。ゲートは baseline と実行数を**同じコマンド**で取るので、仮に報告単位が変わっても両方が同じだけ動く |
| G-3 | 正本 §4.3 の退避方針が「退避された証跡をどこに残すかは schema 次第で #43 / #44 で追跡中」と書いたままで、同じ §4.3 が定めた `droppedEvidence` の契約と矛盾する | した（段落を目視） | **採用（文面）**。置き場は決着済みとして `droppedEvidence` を指し、開いたままなのは `unknown` の失効だけに絞った。revision 行を足した |
| G-4 | evidence が「受理済み terminal の source hash は状態に持っていない（#43）」「退避された operation を状態に残す場所ができれば（#39）」と、この PR より前の設計で書かれている | した（実装と突き合わせ） | **採用（文面）**。`terminalFingerprint` / `droppedEvidence` が現在の置き場であることに直した。**同じ文が実装のコメントにも残っていた**ので合わせて直した（文書だけ直すと次の読者はコードを正本として読む） |

**`5699563` の再レビュー（fresh session の resume）**: blocking 1 件・advisory 1 件、どちらも
「文書とコメントが変更前の挙動を語っている」形。実装・rule 1 の非退行・生成器の provenance 修正は
妥当と評価され、全ゲートもレビュアー側で再現された（G-2 の却下も支持された）。

| # | 指摘 | 再現 | 採否 |
|---|---|---|---|
| H-1 | evidence の実装対応表が「`unresolved` が空なら状態に記録先が無く history にしか残らない」と書いたまま。現在は `orphaned_terminal` を `droppedEvidence` に記録して隔離のまま revision を進める。記録対象も「候補ゼロ」に狭められており、全候補が確定済みで turn 両立する相手が無い経路が漏れている。turn 材料の要約も候補選びの免除で止まっている | した（3 か所とも実装・正本と突き合わせ） | **採用（文面）**。分岐を「記録して隔離する」と「event 自身の corruption なので記録もしない隔離」の 2 つに書き直し、記録対象の綴りが 2 つあること（候補ゼロ／全件確定済みで turn 両立なし）と、turn 材料が無い場合に `terminal_turn_unverifiable` へ倒すことを明記した |
| H-3 | evidence の「隔離する（状態にも台帳にも入れない）」の配下に `terminal_orphaned` と 開いた候補ゼロの `terminal_unmatched` が並んでいる。両者は `quarantineWithRecord` で `droppedEvidence`・revision・history を進めるので、見出しが同じ節の本文と矛盾し、返る状態を捨てる移植を誘発する | した（見出しと本文を突き合わせ） | **採用（文面）**。見出しを「配送鍵を消費しない隔離」に改め、配下を「event 自身の corruption なので状態も不変」と「`droppedEvidence` に記録して状態は進む」に分けた。**隔離という語が 2 つの状態挙動を覆っていた**のは #39 で一度直した形（「caller は outcome によらず返る状態を取る」）と同じで、正本は直っていたが evidence の見出しだけ古かった |
| H-2 | `correlateTerminalEvent` のコメントが「ここに残る open な候補はすべて turn 両立を満たす」と書いているが、材料欠落の候補は意図的に残り最後の門で初めて分かれる。新しい実装理由と逆の説明 | した（コメントを目視） | **採用（文面）**。「具体的な種別不一致は落ちているが、材料欠落は残るので最後の門で分ける」に直した。**文書だけ直してコメントを残すと次の読者はコードを正本として読む**ので、H-1 と同じラウンドで直した |
| H-4 | （レビュー指摘ではなく**こちらの一括 sweep で見つけたもの**）`reduceTerminalEvent` の分岐コメントが「#43 のとおり置き場が凍結 schema に無いので、記録先が無い commit は状態に何も残さない = 隔離との差は鍵を焼くかどうかしかない」と現在形で書いたまま。8 行下の分岐本文が「上のコメントは `droppedEvidence` ができた今は当たらない」と自己訂正しており、**先に読んだほうが変更前の設計**になる | した（1465-1466 行と 1473-1476 行を突き合わせ） | **採用（文面）**。上を現在の設計（commit 側が何も残さないのは候補を 1 件も倒さないから／隔離側は記録して進む）に書き直し、下の「上のコメントは当たらない」という宙に浮いた後方参照を消して、代わりに「返る状態は前の状態ではない」を残した。**3 ラウンド続けて同じ形（この PR が閉じる issue を未来形・条件形で語る記述）が別の場所から 1 件ずつ出ていた**ので、閉じる issue 番号と時制で `specs/` `evidence/` `harness/continuity/` を横断 grep した。他のヒットは経緯として明示された過去形（evidence §「この段落は #43 で解消済み」）、spec の問題記述、research の却下案、まだ閉じない #54 への言及で、いずれも現在形の挙動主張ではない |
| H-5 | H-4 の sweep が同じ文書内の 2 か所を取りこぼしている。evidence §2.5 が「`quarantined` は状態も台帳も更新しない」と全経路を一括しており `quarantineWithRecord` と矛盾する。§2.9 は「addendum に保持・退避の規則が無い」「退避を状態に記録する場所が無く痕跡ごと消える」「この扱いも #39 で決める」と未来形のまま | した（§2.5・§2.9 を実装と正本に突き合わせ。`Retention and eviction` は **origin/main の addendum に既にあり**、`droppedEvidence` はこの PR で入る） | **採用（文面）**。§2.5 を「`quarantined` が言うのは配送鍵を消費しないことだけで、`quarantineWithRecord` は revision と history を進める。呼び出し側は outcome によらず返った状態を取る」に、§2.9 を「当初は正本に無く harness 側で決めた → その後 §4.3 として正本に採用された（残る harness 側の判断は `unknown` の失効規則のみ）」「退避の記録先は #43 で出来た。残る限界は識別材料だけであることと 256 件で有界であること」に直した。**H-4 の sweep が語彙（置き場／記録先／追跡中）で引いたので、同じ主張を別の語で書いた節が漏れた**——語ではなく節ごとに「隔離・退避・記録の挙動を述べているか」で見直し、§2 の全 11 節と §3 の全項目を実装と突き合わせた |
| H-6 | H-5 で §2.9 を「正本の写しである」と宣言したのに、退避順序の記載が `succeeded` → `failed` → `unknown` → `started` の 4 群のままで、正本・実装が**その前に**落とす lineage 外の群が抜けている。群内の唯一の順序が配列位置であって `startedAt` でないことも書いていない | した（`retainPendingOperations` の `passes` は lineage 外を第 1 pass に置き、各 pass は `pending` を配列順に走査。addendum §4.3 も「the earliest surviving array index is evicted first, and the array index is the sole tie-breaker」） | **採用（文面）**。5 群に直し、lineage 外を先頭に置く理由（照合・放棄のどの経路からも候補にならない／絞らないと別 lineage の要素で自 lineage の確定済み証跡を押し出せる）と、`startedAt` で並べ替えない理由（時刻は event を出す側の値）を書いた。**「正本の写し」と宣言した時点で、その節の網羅性の基準が上がる**——写しだと言うなら群を 1 つ落とせない。前 4 件（H-1〜H-5）は「変更前の設計が残っていた」形だが、これは**是正の文面自身が作った不足**で別の形 |

**生成器の provenance（`4ff531d` への bot 指摘）**: 旧形 baseline の生成器は、基準 commit の還元器**だけ**を
作業ツリーの隣に書き出して import していたので、相対 import（`../schema/*.ts`）が **HEAD 側**の schema を
解決していた。「基準 commit の実装」と名乗りながら中身は旧還元器 + 新 schema の混成で、正規化・上限・
kind 語彙を後から変えると固定したはずの基準が黙って動く。`git archive` で基準 commit の `harness/` を
丸ごと一時ディレクトリへ展開し、その中から import する形に直した（未使用のまま作っていた一時
ディレクトリはこれで使い道ができた）。**修正後に再生成した fixture は byte 一致**——今回に限れば混成でも
出力は同じだったが、それは測って初めて言えることで、経路としては塞いだ。

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: 依存なし。ただし T001 が赤なら全体が着手不可
- **Foundational (Phase 2)**: Setup 完了後。**全 user story を塞ぐ**
- **US1 (Phase 3)**: Foundational 完了後
- **US2 (Phase 4)**: Foundational 完了後。US1 とは論理的に独立だが、**同じ関数群（退避経路）を触る**ので直列にする（plan.md）
- **US3 (Phase 5)**: Foundational 完了後。同上
- **Polish (Phase 6)**: 3 story 完了後。T037〜T039 は US1 で関数が消えた後でないと付け替えられない

### User Story Dependencies

- **US1 (P1)**: 他 story に依存しない。単独で成立する。**MVP**
- **US2 (P2)**: US1 に論理依存しない。US1 が `retainPendingOperations` を触るのでファイル上は直列
- **US3 (P3)**: US1 / US2 に論理依存しない。同上

### Within Each User Story

- テストを先に書き、**落ちること**を確認してから実装へ進む
- 契約（Phase 2）→ 書き込み → 読み取り → 索引削除 の順

### Parallel Opportunities

- Phase 3 / 4 / 5 の**テストタスク**（T010〜T013・T020〜T025・T030〜T032）は同じファイルに書くが
  互いに独立なので、内容としては並行して設計できる。**同時編集はしない**（同一ファイル）
- Phase 6 の T040 / T041 / T042 は別ファイルなので真に並列可
- 実装タスクは全て `reference-model.ts` の同じ関数群を触るため**並列にしない**

---

## Parallel Example: Phase 6 の正本追従

```bash
# 別ファイルなので同時に進められる
Task: "addendum §4.3 の文面を実装に一致させる（T040）"
Task: "addendum §0.1 に改定行を足す（T041）"
Task: "evidence に索引方式をやめた理由を記録する（T042）"
```

---

## Implementation Strategy

### MVP First (User Story 1 のみ)

1. Phase 1: Setup（前提の確認）
2. Phase 2: Foundational（契約の固定 — **全 story を塞ぐ**）
3. Phase 3: US1（順序材料の移設 + 索引削除）
4. **STOP して検証**: 状態だけを渡す経路で権威順序が判定できること。実装が減っていること
5. ここで一度 5 ゲートを通す

US1 だけで SC-002 の「実施不能な 1 件」が解消し、SC-001 のパリティが名乗れるようになる。
US2 / US3 は正本との食い違いの解消と診断の追加であり、誤った判断が確定する経路は無い。

### Incremental Delivery

1. Setup + Foundational → 契約が固定
2. US1 → 単独検証 → **MVP**（実装が減る）
3. US2 → 単独検証（記録の置き場ができる）
4. US3 → 単独検証（指紋の比較が効く）
5. Polish → fixture / hash / 変異 / 正本を着地形に合わせる

各 story は前の story の挙動を壊さない（新しい欄はすべて任意で、無ければ従来経路）。

### 単独作業前提

この repo は 1 人 + 委譲で動いている。実装タスクは全て同じファイルの同じ関数群を触るので、
**実装の並列化はしない**。並列にできるのは Phase 6 の正本追従だけ。

---

## Notes

- [P] = 別ファイル・依存なし
- 実装は Claude Code が自ら行う（`rules/coding.md` の委譲ルーティング: 入力検証・fail-closed の
  境界はセキュリティ関連。外部 CLI へ委譲しない）
- **`harness/contract-hashes.json` を手で書かない**。必ず再生成する（FR-016）
- 変異ゲートは**生存数より先に実行件数**を見る。アンカー切れは黙ってスキップされる
- タスクごと、または論理的なまとまりごとに commit する
- 各 Checkpoint で止まって story 単独の成立を確認してよい
