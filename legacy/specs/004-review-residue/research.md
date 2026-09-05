# Research: #92 レビュー残件（issue #109）の回収

**作成**: 2026-08-20
**基準 commit**: `bc42c40`（origin/main。PR #92 のマージ commit）
**作業 branch**: `fix/review-residue-109`

この文書は、実装前に **実際に走らせて確かめた** 環境側の事実と、後続検証で確定した結果を記録する。
issue #109 の各指摘そのものの裏取りは `spec.md` の FR と `tasks.md` に落とす。

## 1. ベースライン検査の実測（開始 2026-08-20、full gate 2026-08-22）

worktree を `origin/main` から作り、CI の harness job と同じ順で回した結果:

| 検査 | コマンド | 結果 |
|---|---|---|
| contract hashes | `node harness/contract-hashes.mjs \| diff harness/contract-hashes.json -` | 一致 |
| capability matrix self-test | `node --experimental-strip-types harness/assemble.ts --self-test` | `PASS` |
| continuity contract tests | `node --experimental-strip-types --test harness/continuity/*.test.ts` | 333 pass / 0 fail |
| evidence verification tests（初回） | `node --experimental-strip-types --test harness/evidence/*.test.ts harness/evidence/rig-manifest.test.mjs` | 167 pass / 0 fail（`bc42c40`、2026-08-20） |
| evidence verification tests（後続） | 同上 | 174 pass / 0 fail（`49b2ef3`、2026-08-22） |
| DCO gate self-test | `node --test harness/dco-check.test.mjs` | 0 fail |
| matrix drift | `node --experimental-strip-types --test harness/evidence/matrix-drift.test.ts` | 2 pass / 0 fail |
| evidence 変異ゲート（初回） | `bash harness/evidence/mutate.sh` | 実行開始のみで verdict 未記録（`bc42c40`、2026-08-20） |
| evidence 変異ゲート（後続） | 同上 | 144 / 144、survivor 0（push CI `32500071210`・PR CI `32500074074`、2026-08-22） |

Node は `v24.16.0`（CI の `node-version: 24.16.0` と一致）。

evidence test の 167 → 174 は、`bc42c40...beefb07` で追加した 7 test による。内訳は
`digestNormalized`、負け側 high-level observation、rig の不正引数と trailing 引数、schema enum 一致・
未登録定数・未知 source event の各 1 test。

## 2. 変異ゲートの自己検査が課す制約（`harness/evidence/mutate.sh`）

実装を触る前に読んで確かめた、**この feature の変更が必ずぶつかる** 仕掛け:

- `MUTABLE=("$ASSEMBLE" "$VERIFY" "$NORMALIZE" "$SCHEMA" "$MSCHEMA" "$SCHEMAV" "$CAP" "$IMPORT" "$RIG" "$HASHES" "$FIXTURE" "$MATRIX" "$CI")`
  — 変異中はこれらが書き換わる。**ゲートの実行中に作業ツリーのこれらを編集してはならない**
  （EXIT trap の `restore_all` が編集を巻き戻す）。
- 変異表との突き合わせは `python3` で行い、`|| exit 1` が付いているので **この python の
  終了状態は伝播する**。issue #109 が言う「python の非ゼロが伝播しない」のは
  **アンカー適用側の別の python** のこと。両者を混同しないこと。
- 件数は **直書き**: `if len(table) != 144:`。変異を増減したら
  `specs/003-evidence-hash-normalization/tasks.md` の表と **この数値の両方** を更新する。
  （直書きの理由も同ファイルにコメントで書かれている: 表の行と実変異を同時に消す縮小を
  通さないため。数え上げに変えてはならない。）
- `bak_key() { printf '%s' "${1//\//_}"; }` — `/` を `_` に潰す。すぐ上のコメントは
  「basename だと別 directory の同名 file で退避が上書きされる」と書いており、`_` 化は
  その対策。ただし `_` 化でも別 path が同じ鍵に落ちうる（#109 の指摘）。

## 3. contract hash と「公開済み版の不変」ゲートの守備範囲（一次確認済み）

`capability.schema.json` を触ってよいかを決める 2 つの CI step を読んだ:

- **`Contract hashes are regenerated`**: `harness/contract-hashes.mjs` は `ROOTS` 配下を
  walk して **各ファイルの生バイトの SHA-256** を `harness/contract-hashes.json` に書く。
  → `harness/schema/capability.schema.json` は対象。**1 byte でも変えたら
  `harness/contract-hashes.json` の再生成が必須**。整形の違いも検出される。
- **`Published manifest versions are immutable`**: 対象は
  `grep -E 'capability-scenarios\.v[0-9]+\.json$'` で絞られた **`capability-scenarios.v*.json` のみ**。
  → `capability.schema.json` は **この不変ゲートの対象外**。したがって enum の重複統合は
  「禁止」ではなく「hash 再生成を伴う変更」として扱える。

## 4. 出荷 matrix の不変条件（#90 kill switch）

`harness/matrix/*.json` の 21 cell はすべて `source-test`、`real-cli-e2e` は 0 件。
`killswitch.test.ts` が manifest の同梱と `real-cli-e2e` の出荷を止めている。
**この feature のどの変更もこれを動かしてはならない。** 動かす提案は却下する。

## 5. 並行レーンとの非重複（衝突回避）

同時に走っている他レーンとファイルが重ならないことを確認済み:

- `fix/continuity-abandon-conflict-73`（issue #73）→ `harness/continuity/*` と
  `evidence/phase3-reference-model.md` に閉じる
- `fix/flaky-spool-89`（issue #89）→ `vendor/codemem/packages/core/src/spool*` に閉じる

この feature は `harness/evidence/*`・`harness/assemble.ts`・`harness/schema/*`・
`harness/rig/*`・`.github/workflows/ci.yml`・`harness/matrix/README.md`・
`specs/003-evidence-hash-normalization/*`・`harness/contract-hashes.json` を触る。
調査記録として本ファイル `specs/004-review-residue/research.md` も対象に含む。

## 6. `synthetic-tmp-cleanup` は nit ではなく実害（2026-08-20 実測）

issue #109 は `harness/evidence/synthetic.ts` の一時 directory 漏れを「test 衛生」として挙げていたが、
この作業中に **実際に別の作業を止めた**。

実害の測定時、変異ゲート（`bash harness/evidence/mutate.sh`）は test 一式を約 99 回回していた。
現行は baseline と 144 mutation で 145 回以上になる。修正前の `newRoot()` は呼び出すたびに
`mkdtempSync(join(tmpdir(), "evroot-"))` を作り、どこでも消さなかった。measurement:

```console
$ find /tmp -maxdepth 1 -name 'evroot-*' | wc -l
273737
$ find /tmp -maxdepth 1 -mindepth 1 | wc -l
309590
```

内訳（/tmp 直下）: `evroot-` 273,737 / `evidence-secrets-` 9,084 / `matrix-drift-` 7,120 /
`evsib-` 5,224 / `evlink-` 5,224 / `evfix-` 5,159。
これは反復作業後の `/tmp` 全体の累積 snapshot で、単一の gate run へ件数を帰属する値ではない。

再現用に 2026-08-22、`bc42c40` と `beefb07` を別々の専用 `TMPDIR` で比較した。
`hash-inputs.test.ts`・`manifest.test.ts`・`promotion.test.ts` は両 commit とも成功し、終了後の残留は
`bc42c40`: `evroot=57 / evfix=1`、`beefb07`: `evroot=0 / evfix=0` だった。

この状態で `grok-delegate.sh` による実装委譲を起動すると、sandbox の構築段階で落ちた:

```text
grok exit=1: error: sandbox deny glob could not be enforced on Linux:
expanding the deny globs ["/tmp/tmp.*/releases"] visited over 2000000 entries
across their roots (stopped in /tmp at /tmp/evroot-.../backed
```

つまり漏れた一時 directory が **カーネル強制の deny glob 展開を破綻させ、sandbox を組めなくした**。
`/tmp` を掃除してから再実行して復旧させた。

**この項目の優先度はこの実測に基づいて上げてよい。** 実装は `scratch.ts` の `newRoot()` で
作成先を記録し、process の `exit` で一括削除する。module 直下の `node:test` `after()` は、test でない
script から import しただけでも runner の status を stdout へ出すため採用しなかった。`promotion.test.ts`
の `evfix-` も同じ `newRoot()` 経路へ寄せた。
