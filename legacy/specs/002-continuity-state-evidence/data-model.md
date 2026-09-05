# Phase 1 Data Model: 継続状態に証跡の置き場を作る（Cluster C）

R-001〜R-005 の決定を、凍結 schema への具体的な差分に落とす。**追加は任意欄のみ**、型名の版は
据え置き（R-002）。正本は `harness/schema/continuity.schema.json`（生バイトが contract hash の
入力）で、`harness/schema/continuity.ts` はその TypeScript 表現。両方を同じ差分で変える。

---

## 1. `PendingOperation` への追加（3 欄・すべて任意）

| 欄 | 型 | 由来 | 何のために |
|---|---|---|---|
| `startIngestSeq` | decimal string | #35 | §4.3「terminal は start より後」を状態だけで判定する |
| `startTurnIdSource` | `TurnIdSource` | #35 | §4.3 rule 2 の turn 種別両立を状態だけで判定する |
| `terminalFingerprint` | string | #44 | 配送 ID をまたいだ payload 衝突を検出する |

### 制約

- `startIngestSeq`: `lastIngestSeq` と**同じ**制約を使う（`pattern: ^(0|[1-9][0-9]*)$`、
  `maxLength: 8192`）。別の綴りを許すと、同じ値に複数の表現ができて比較が割れる。
- `startTurnIdSource`: `#/$defs/TurnIdSource` を `$ref` する。語彙を複製しない。
- `terminalFingerprint`: `NormalizedContinuityEvent.canonicalFingerprint` と同じ制約
  （`type: string`, `maxLength: 8192`）。格納するのは受理した terminal の
  `canonicalFingerprint` そのもので、新しく計算し直さない。
- 3 欄とも `required` に**入れない**。`additionalProperties: false` は維持する。

### 書き込み規則

- **`startIngestSeq` / `startTurnIdSource` は、その operation の start を最初に受理したときだけ
  書く**（FR-003）。同じ operation への再配送 start では**触らない**。現行の
  `operationStarts` も同じ規則で運用されており（「再配送された start で `operationStarts` を
  埋めない」）、その規則をそのまま引き継ぐ。
- `terminalFingerprint` は terminal を受理して `status` を `succeeded` / `failed` に変えるときに
  書く。`unknown` へ倒す経路では書かない（受理していないため）。
- 一度書いた 3 欄を**上書きしない**。値が食い違う入力は上書きではなく衝突として扱う。
  **これは確定済みの候補だけの規則ではない**: 凍結 schema は `started` / `unknown` の要素が
  `terminalFingerprint` を持つことを妨げないので、復元した状態では open な候補が指紋を持ちうる。
  照合が open な候補を 1 件に決めた場面でも、その候補が名乗る指紋と入ってくる指紋が違えば
  `terminal_conflict` で隔離する（閉じる経路は指紋を無条件に上書きするので、ここで見ないと
  この規則が open な候補についてだけ破れる）。入ってくる側が `unknown` に倒れる経路は指紋を
  書かないので発動しない。

### 読み取り規則（欄が無いとき）

| 欄が無い | 現行の挙動 | 変更後 |
|---|---|---|
| `startIngestSeq` | `terminal_order_unverifiable` → `unknown` | 同じ |
| `startTurnIdSource` | rule 2 の種別検査を免除（材料なし扱い） | 免除は**候補選びだけ**。検査は未実施なので rule 2 では閉じず `terminal_turn_unverifiable` → `unknown`（rule 1 は影響なし） |
| `terminalFingerprint` | `terminal_already_applied` として台帳に入れる | 同じ |

**材料が無いことを「検査に合格した」と読み替えない**（FR-004 / FR-012）。復元直後・旧い状態・
退避後のいずれでも、現行の fail-closed 経路にそのまま落ちる。

---

## 2. `CanonicalWorkStateV1` への追加（1 欄・任意）

```
droppedEvidence?: DroppedEvidenceEntryV1[]   // maxItems: 256
```

live な集合（`pendingOperations`）から外れたものの有界な記録（R-003）。`pendingOperations` と
同じ `maxItems: 256`（§10 `arrayItems`）。`required` に入れない。

## 3. 新しい `$def`: `DroppedEvidenceEntryV1`

| 欄 | 型 | 必須 | 内容 |
|---|---|---|---|
| `reason` | `"evicted"` \| `"orphaned_terminal"` | ✓ | なぜ live から外れたか |
| `recordedAt` | `IsoTimestamp` | ✓ | 記録した event の `occurredAt` |
| `eventId` | string (maxLength 8192) | | この記録を名指す event。孤児は terminal、退避は start |
| `operationId` | string (maxLength 8192) | | 退避された operation の識別 |
| `status` | `"started"`\|`"succeeded"`\|`"failed"`\|`"unknown"` | | 退避時点の状態 |
| `terminalFingerprint` | string (maxLength 8192) | | 孤児 terminal の `canonicalFingerprint` |
| `adapterDeliveryId` | string (maxLength 8192) | | 孤児 terminal の配送鍵 |
| `sensitivity` | `#/$defs/Sensitivity` | ✓ | 記録自体の機密度 |

`additionalProperties: false`。**payload・引数・出力・`description` を持たない**（R-005 / FR-007）。

### `reason` ごとに何が入るか

- `evicted`: `operationId` + `status` + `recordedAt` + `eventId`（退避された operation の start の
  event id）。**`operationId` は凍結 schema で一意ではない**ので、同名の兄弟が並ぶ状態では
  id と status だけでは「どちらが落ちたか」を特定できない。
- `orphaned_terminal`: `eventId`（terminal の event id）+ `recordedAt` + `terminalFingerprint` +
  `adapterDeliveryId`。`operationId` / `status` は入れない（相手が居ないので書けるものが無い）。

**重複判定の鍵は後ろ 2 欄で、優先順位は §8.2 と同じ**（`adapterDeliveryId`、無ければ
`canonicalFingerprint`。keyspace は台帳の `ledgerKeyOf` と同じく `d:` / `f:` で分ける）。
指紋だけを鍵にすると、配送鍵も相関材料も違う terminal が同じ指紋を名乗るだけで 1 件に潰れる。
`eventId` を鍵にすると、封筒の値なので再配送のたびに足してしまう。鍵が一致していても指紋が
食い違う場合は再送ではなく corruption なので、記録も状態も動かさず `delivery_conflict` を出す
（台帳経路が同じ条件でそうしているのに合わせる）。

schema では両方を任意にし、**組み合わせの妥当性を検査する層は置かない**。この配列を書くのは
還元器だけで、理由ごとに書く欄はそこで決まっている。別実装が書いた状態が `evicted` に想定外の
組み合わせを載せていても、schema も還元器も落とさない（読む側は `reason` で分岐すること）。
移植先が runtime 検査を足すと、参照実装が運ぶ状態を移植先だけが拒否して parity が崩れる。
`oneOf` で分岐させると
`additionalProperties: false` との組み合わせで別言語の validator の挙動差が出やすく、
contract hash が同じでも判断が割れうる（この repo が避けている形）。

### `sensitivity` をどう決めるか

退避された operation の `sensitivity` を**引き継ぐ**。孤児 terminal は相手が居ないので、
fail-closed の既定（`private`）にする。記録側で内容を見て決め直さない（Constitution III）。

---

## 4. 追加・脱落の規則

- 追加は**末尾**。
- `maxItems` に達していたら、**配列の先頭から** 1 件落としてから追加する（R-004 / FR-008）。
  時刻で並べ替えない。`pendingOperations` の退避と同じ規則・同じ理由。
- 追加も脱落も診断に出す（FR-009）。新しい診断コード 2 つ:
  - `dropped_evidence_recorded`
  - `dropped_evidence_overflowed`

---

## 5. 削除できるもの（この変更で不要になる）

R-001 の帰結として、凍結 schema の外の索引がまるごと不要になる。

- `TaskWorkStateSnapshotV1.operationStarts`（`ReadonlyMap<operationId, OperationStartFactsV1>`）
- `OperationStartFactsV1` 型
- `startFactsFor()` — 自 lineage の同名 pending を数えて曖昧なら材料なしに倒す関数。
  材料が要素そのものに載れば、鍵が要らないので曖昧さが発生しない
- 退避時の索引同期（`for (const evictedId of evicted) operationStarts.delete(evictedId)`）

**これらを消すときは、対応する変異ゲートも一緒に付け替える**。消した関数を守っていた変異が
アンカー切れで黙って飛ぶと、`mutate.sh` の 実行/期待 の突き合わせで落ちる（設計どおり）。

---

## 6. 波及するもの

| 対象 | 何をするか |
|---|---|
| `harness/schema/continuity.schema.json` | 上記の欄と `$def` を追加 |
| `harness/schema/continuity.ts` | 同じ形を TypeScript で追加 |
| `harness/continuity/schema-freeze.test.ts` | 新しい欄・`$def` の凍結を固定。任意であることも固定する |
| `harness/continuity/reference-model.ts` | 書き込み・読み取り・記録・退避を実装、索引を削除 |
| `harness/continuity/reference-model.test.ts` | FR ごとの回帰。特に FR-003（再配送で上書きしない）と FR-004/FR-012（欄が無いときの fail-closed） |
| `harness/continuity/mutate.sh` | 新しい規則ごとに変異を追加、消える規則の変異を削除 |
| `harness/fixtures/continuity/*.json` | 新しい欄を持つ状態と、持たない状態の両方を残す |
| `harness/contract-hashes.json` | 再生成（手で書かない） |
| `specs/001-agent-memory-core/resume-continuity-addendum-v6.2.md` | §4.3 の文面を実装と一致させる（FR-017）。§0.1 に改定行を足す |
| `evidence/phase3-reference-model.md` | 索引方式をやめた理由と、新しい規則の根拠を記録 |

---

## 7. 検証で最初に落ちるべきもの（実装前に決めておく）

各 FR に対して「これを壊す変異」を先に決めておき、実装後に `mutate.sh` へ入れる。

| FR | 壊し方 | 落ちるべきテスト |
|---|---|---|
| FR-003 | 再配送 start でも `startIngestSeq` を書く | 遅れて届いた再配送の後、正当な terminal が順序違反で落ちないこと |
| FR-004 | 欄が無いときに検査を素通りさせる | 復元直後の terminal が `terminal_order_unverifiable` になること |
| FR-004 | turn 種別が無い候補を rule 2 で閉じる／逆に rule 1 まで止める | `startIngestSeq` だけを持つ状態が `terminal_turn_unverifiable` → `unknown` になり、rule 1 の terminal は普通に閉じること |
| FR-008 | 記録を `recordedAt` 昇順で落とす | 配列先頭が落ちて末尾が残ること |
| FR-009 | 脱落の診断を出さない | 診断コードの集合 |
| FR-012 | 指紋が無いときも衝突扱いにする | 旧い状態で `terminal_already_applied` のままであること |
| FR-015 | 記録の上限検査を外す | 記録が 256 件で頭打ちになること |
