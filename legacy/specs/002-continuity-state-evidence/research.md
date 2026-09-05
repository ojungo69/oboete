# Phase 0 Research: 継続状態に証跡の置き場を作る（Cluster C）

Technical Context に残った未確定を、実コードと正本の照合で潰す。ここで決めた 4 件が
data-model.md の形を決める。

---

## R-001: 追加は `PendingOperation` 本体か、状態に並ぶ別の索引か

**Decision**: 順序材料（`startIngestSeq` / `startTurnIdSource`）と受理済み terminal の指紋は、
**`PendingOperation` の任意欄として本体に持たせる**。凍結 schema に `operationId` 鍵の写像を
足す案は採らない。

**Rationale**: 現行の回避策（`TaskWorkStateSnapshotV1.operationStarts`、凍結 schema の外の
`ReadonlyMap<operationId, OperationStartFactsV1>`）は **`operationId` を鍵にしている**。ところが
凍結 schema は `pendingOperations` の `operationId` に一意性を課していない（`PendingOperation` の
`operationId` は単なる `string`、配列側にも `uniqueItems` 相当の制約が無い）。復元した checkpoint に
同じ id の兄弟が並びうるので、**どちらの兄弟の材料かを原理的に判別できない**。

このため参照実装には `startFactsFor` という関数があり、自 lineage の同名 pending を数え、2 件以上
あれば「材料なし」に倒している。コメントに実測が残っている:

> 兄弟 B の `ingestSeq` 100 を使って A 宛ての terminal が通り、A が診断ゼロで `succeeded` になった。
> 逆向きの値では健全な terminal が `terminal_out_of_order` で弾かれた。

つまり**索引方式は、鍵が一意でないという schema の性質そのものと衝突している**。同じ形の写像を
凍結 schema の中に移しても、この衝突はそのまま持ち込まれる。材料を要素そのものに持たせれば鍵が
要らなくなり、曖昧さが原理的に消える。`startFactsFor` の兄弟数え、その「材料なし」経路、
`operationStarts` の退避同期（`for (const evictedId of evicted) operationStarts.delete(evictedId)`）が
まとめて不要になる。

**Alternatives considered**:

- **凍結 schema に `operationStarts` 相当の写像を足す**: 上記のとおり鍵の非一意性を持ち込む。
  加えて、退避のたびに 2 つの配列を同期させる必要が残り、同期漏れが新しい失敗モードになる。
- **`operationId` に一意性制約を足す**: 凍結 schema の**制約強化**であり、既存の状態を読めなく
  しうる（FR-013 に反する）。しかも一意性は event の出し手が握っており、強制すると復元経路が
  丸ごと失敗する。
- **現状維持（daemon の責務と明記する）**: SC-001（状態だけの実装が同じ判断に到達する）を
  満たせない。#35 が閉じない。

---

## R-002: 型名の版を上げるか（`CanonicalWorkStateV1` → `V2`）

**Decision**: 型名の版は**上げない**。`CanonicalWorkStateV1` / `PendingOperation` に**任意欄のみ**を
足し、`schemaVersion` は `1` のまま据え置く。

**Rationale**: この repo の版の慣行は「版番号は型名の一部」（`ContinuationCheckpointV2` /
`ResumeCapsuleV1` / `SemanticResumeNoteV1`）。`CanonicalWorkStateV1` を `V2` にすると、それを
内包する `ContinuationCheckpointV2.canonicalState` から連鎖して checkpoint 自身の版も動き、
fixture・contract hash・addendum の型記述がまとめて書き換わる。得られるものは「版が動いた」という
札だけで、**読み手がまだ 1 つも存在しない**（Rust 実装は未着手、永続化された状態も無い）。

drift の検出は版番号ではなく **`harness/contract-hashes.json`** が担っている。schema の生バイトが
変われば hash が変わり、CI が再生成との差分で落とす。古い hash に固定された利用者は、静かに
誤読するのではなく**大きな音を立てて失敗する**。これが設計上の検出機構であり、任意欄の追加でも
必ず発火する。

なお `additionalProperties: false` なので、**厳格な旧リーダーは新しい状態を拒否する**。これは
「静かに誤読」ではなく「明示的に失敗」であり、hash 検出と同じ向きに倒れている。

**本物の V2 が必要になる条件**（どれか 1 つでも満たしたら、次の変更は任意欄の追加では済ませない）:

1. `CanonicalWorkStateV1` を**永続化した状態が repo の外に出た**とき（daemon が checkpoint を
   ディスクに書き、そのファイルを次の版が読む必要が生じたとき）。
2. **2 つ目の実装が契約を読み始めた**とき（Rust 側が `contract-hashes.json` に固定された）。
   以後は片方だけが新しい欄を書く期間が必ず生じ、版で区別できないと調整できない。
3. **既存欄の意味・必須性・値域を変える**とき。任意欄の追加は既存の読み手の判断を変えないが、
   制約強化は変える。
4. **欄を削除・改名する**とき。

この 4 条件は plan.md の Complexity Tracking にも転記し、次に schema を触る回の判断材料にする。

**Alternatives considered**:

- **`CanonicalWorkStateV1` → `V2` にする**: 連鎖する書き換えの量に対して、いま守るものが無い。
  上の 4 条件のどれも満たしていない。
- **`schemaVersion` だけ 2 に上げて型名は据え置く**: 型名と版番号が食い違い、慣行が壊れる。
  `schemaVersion: 1` は literal 型なので、実質 `1 | 2` の受理を全経路に足すことになる。

---

## R-003: 「消えた証跡」は 1 つの配列か 2 つか

**Decision**: **1 つの配列**（`droppedEvidence`）にまとめ、`reason` で退避（`evicted`）と
孤児 terminal（`orphaned_terminal`）を区別する。

**Rationale**: 2 つに分けると、上限（§10 `arrayItems` = 256）が 2 つ、あふれたときの規則が 2 つ、
落としたときの診断経路が 2 つになる。状態のサイズ上限も実質 2 倍になり、SC-005 の「頭打ちになる」
を 2 箇所で守ることになる。両者が持つ情報は「何が」「なぜ」「いつ」live な集合から外れたかで
同型なので、`reason` 1 欄で足りる。

**Alternatives considered**:

- **2 配列（`evictedOperations` / `unmatchedTerminals`）**: 型は素直になるが、上記の重複を招く。
  読み分けは `reason` での絞り込みで足りる。
- **診断だけに任せる（配列を足さない）**: 診断は状態の外なので、状態を受け取った側には届かない。
  FR-005 / FR-006 を満たせず、正本 §4.3 の「保存する」とも食い違ったまま。

---

## R-004: 記録の並び順とあふれ方

**Decision**: 追加は**末尾**。上限に達したら**配列の先頭から**落とす。時刻で並べ替えない。
落としたことは診断に出す。

**Rationale**: `pendingOperations` の退避で 2026-08-17 に確定した規則（addendum §4.3）と同じにする。
理由も同じで、`startedAt` は adapter が寄越した `occurredAt` の写しなので到着順と一致せず、event を
出す側が動かせる。ここで別の規則を作ると、同じ event 列から実装ごとに違う記録が残り、
`stateRevision` と内容 hash が割れる（= SC-001 が落ちる）。規則を 1 つに揃えれば、退避と記録で
同じ説明・同じテスト・同じ変異が使える。

**Alternatives considered**:

- **`startedAt` / `recordedAt` の昇順で落とす**: 上記のとおり出し手が動かせる値に順序を預けることに
  なる。addendum が `pendingOperations` について明示的に禁じた形。
- **上限であふれたら新しい記録を捨てる（末尾を落とす）**: 直近の事象ほど診断に使われるので、
  古い側から落とすほうが有用。どちらでも決定的だが、`pendingOperations` と逆向きになるのを避ける。

---

## R-005: 記録に何を入れないか

**Decision**: payload・引数・出力・自由文の `description` を**入れない**。識別（`eventId` /
`operationId` と、重複判定の鍵になる `terminalFingerprint` / `adapterDeliveryId`）、分類
（`reason` / `status`）、時刻（`recordedAt`）、機密度（`sensitivity`）に限る。指紋と配送 ID は
**同一性の材料であって内容ではない**——どちらも adapter が算出・採番した識別子で、payload の
中身は復元できない。`sensitivity` は退避元から引き継ぐ値で、記録側で内容を見て決め直すもの
ではない（Constitution III）。

**Rationale**: 記録は「あった」ことを示すためのもので、内容の保管庫ではない。内容を持ち込むと
`sensitivity` の判定を記録側でもう一度やる必要が生じ（Constitution III のプライバシー境界）、
`arrayItems` × `maxLength` の掛け算で状態サイズの上限も跳ね上がる。内容が要るなら event store を
引く経路が正しい。

**Alternatives considered**:

- **`description` を含める**: 診断の読みやすさは上がるが、`sensitivity` の再判定が要る。
  診断メッセージ側で出せば足りる（状態に残す必要が無い）。

---

## 未解決のまま plan へ送るもの

なし。R-001〜R-005 で data-model.md を書ける。
