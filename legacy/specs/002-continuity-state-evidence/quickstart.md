# Quickstart: 実装が着地したかを確かめる

CI（`.github/workflows/ci.yml` の `harness` job）と同じ検査をローカルで走らせる手順。
「完了」報告ではなく、この 6 本が緑になったことを完了の定義にする。

## 前提

- 作業ディレクトリ: この feature 用の worktree（ブランチ `feat/continuity-state-schema-v2` を
  `git worktree add` で切り出した専用チェックアウト）
- Node.js は repo の設定に従う。`tsc` は vendor 済みのものを使う（追加インストール不要）

## 7 本のゲート

```bash
cd "$WT"   # この feature の worktree のパスを入れておく

# 1. 型
vendor/codemem/node_modules/.bin/tsc -p harness/tsconfig.json

# 2. 契約 hash が再生成と一致する（手で書き換えていないこと）
node harness/contract-hashes.mjs > /tmp/ch.json && diff harness/contract-hashes.json /tmp/ch.json

# 3. capability matrix の自己検査
node --experimental-strip-types harness/assemble.ts --self-test   # PASS が出ること

# 4. continuity のテスト
node --experimental-strip-types --test harness/continuity/*.test.ts

# 5. 変異ゲート（実行数 == 期待数、生存 0）
bash harness/continuity/mutate.sh

# 6. license / notice
node harness/license-inclusion-check.mjs

# 7. 旧形 parity の baseline が生成物であること（手で書き換えていないこと）
node --experimental-strip-types harness/continuity/old-shape-baseline.mjs --output /tmp/osp.json \
  && diff harness/fixtures/continuity/old-shape-parity.json /tmp/osp.json
```

**2 は 4 より先に走らせる。** 順序を入れ替えると、テストが作業ツリーを書き換えた後の状態どうしを
比べることになり、凍結の検査にならない（CI のコメントに同じ理由が書いてある）。

## 期待値

| ゲート | 期待 |
|---|---|
| 1 | 出力なし |
| 2 | 差分なし |
| 3 | `PASS` |
| 4 | `fail 0`。件数は現行 259 から増える（FR ごとの回帰を足すため） |
| 5 | `実行 N / 期待 N、生存 0` |
| 6 | `license inclusion check OK` |
| 7 | 差分なし |

## 受け入れシナリオの手動確認

自動テストとは別に、spec の受け入れシナリオを 2 つだけ手で追う。どちらも「状態だけを渡された
実装」の視点で見る（索引を消したので、渡せる引数は状態と event だけになった）。

1. **US1 シナリオ 3**（パリティ）: 同じ event 列を、索引ありの経路と索引なしの経路に流し、
   最終状態の `stateRevision` が一致することを確認する。索引を消したあとは経路が 1 本になるので、
   代わりに「fixture から復元した状態 + event」と「event を最初から流した状態」の一致を見る。
2. **US2 シナリオ 4**（旧い状態）: 新しい欄を持たない fixture を読み、診断と結果が変更前と
   同じであることを確認する。これは自動化してあるので手で追う必要は無い——
   `harness/continuity/old-shape-parity.test.ts` が、旧形の corpus を分岐点の実装に通した
   committed baseline と突き合わせる（差分は許可表に列挙したものだけ通る）。

## 落ちたときに最初に見るところ

- **5 で `実行 < 期待`**: 変異のアンカーが実装の変更で外れている。`mutate.sh` の header にある
  `comm -23` の突き合わせでどのラベルが飛んだかを出す。
- **2 で差分**: schema か fixture を触ったのに `contract-hashes.json` を再生成していない。
- **4 で旧い fixture が落ちる**: FR-013 / FR-014 に違反している。新しい欄を必須にしていないか、
  欄が無いときの経路を変えていないかを見る。
