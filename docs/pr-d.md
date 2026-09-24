# PR-D — 意味検索 (2026-09-24 着手)

仕様の正本は提案 (`docs/research/search-sync-proposal-2026-09-23.md`) の §2 (bge-m3 の 1 つのベクトル空間、§2.4 の「2 意味」、§2.5 の埋め込む文)、§3.3 の合格線、§7 の PR-D 行、決定 1・4・5・14。ここは spike の結果と、実装で決めたことだけを置く。

## spike (dev 分、2026-09-24)

評価用の store (約 18 万件、`docs/pr-b.md`) の文書を全部 Workers AI の bge-m3 でベクトルにし、PR-B の束で測った。スクリプトは `docs/spike/pr-d/`。

1. `export.py`: 埋め込む文を作り、関門 (`oboete gate`) を通す。観測は `kind: title` の次の行に本文、要約は本文、prompt は先頭 1,000 字。Workers AI は 8,192 トークンを超える分を切る (`truncate_inputs`)。
2. `embed.py`: Workers AI に 1 回 100 件まで、件数 × 最長の文の長さが 5 万字以下になるように、長さの近い文をまとめて送る (PR-A2 の上限)。
3. `runs.py` / `runs_kf.py`: 全件の cos で厳密に並べる。同じ session の文書は順位を付ける前に除き、深さは 50、RRF は k=60。
4. `vecbench.py`: sqlite-vec の全件検索の速さ。

**費用と時間**: 文書 178,502 件と問い 424 件で入力 7,690 万トークン、82,657 neuron、1 日 1 万 neuron の無料枠を引いて約 $0.80 (Cloudflare の集計、2026-09-24)。4 並列で 14 分。

### 比べた方式

- `vec`: 全文書を cos で並べたもの。
- `vec-kf`: 種類ごとに cos で並べ、知識 (観測・要約) を先、prompt を後にしたもの。
- `hybrid-rrf`: e0 (PR-E0 の全文検索) と `vec` を RRF でまとめたもの。
- `hybrid-kf`: 知識どうし、prompt どうしで e0 と `vec` を RRF でまとめ、知識を先にしたもの。今の検索の並び (#26 の決定: 知識が先、prompt が後) を守る形。

### 結果 (dev、8 方式の束)

8 方式の上位 20 件を全部判定した束で比べる (判定は `claude-sonnet-5`、2 点以上の文書が 1 件でもある問いは 306 問)。束に方式が増えると、新しい方式が見つけた正解の分だけ理想の並びの値が上がるので、同じ方式の値も下がる。`docs/pr-e0.md` の e0 の 0.557 は 4 方式の束の値で、この束では 0.438 になる (e0 の順位は変わっていない)。

| 区画 (問いの数) | hybrid-kf | hybrid-rrf | vec-kf | vec | e0 | claude-mem (窓なし) |
|---|---|---|---|---|---|---|
| 全部 (306) | 0.589 | 0.594 | **0.595** | 0.567 | 0.438 | 0.283 |
| 日本語の問い (266) | 0.579 | 0.580 | **0.588** | 0.556 | 0.421 | 0.253 |
| 英語の問い (40) | 0.653 | **0.687** | 0.643 | 0.640 | 0.552 | 0.489 |
| agent の検索語 (22) | **0.742** | 0.739 | 0.730 | 0.672 | 0.669 | 0.478 |
| 問いより前に書かれた文書だけ (255) | 0.481 | 0.547 | 0.467 | **0.552** | 0.373 | 0.221 |
| 問いを引用した文書を除く (306) | 0.581 | 0.580 | **0.592** | 0.560 | 0.423 | 0.283 |
| 観測だけ (300) | 0.616 | 0.510 | **0.624** | 0.301 | 0.465 | 0.324 |

数字は nDCG@10。区画の意味は `docs/pr-e0.md` と同じ。

### わかったこと

- **意味検索は全文検索 (e0) を大きく上回る**。`hybrid-kf` は表のすべての区画で e0 を上回る (全部で +0.15、どの区画も p < 0.05)。§3.3 の合格線 (test 分で +0.03 以上) には余裕がある。test 分は D2 で 1 回だけ測る。
- **`vec` だけだと上位 10 件の 8 割が prompt になる**。問いが developer の prompt なので、似た言い方の過去の prompt が上に来る。そこに混ざる観測と要約の精度は高い (上位 10 件のうち 2 点以上が観測 67%、要約 72%、prompt 32%)。
- **知識を先にするか、混ぜるかは、この評価では決まらない**。全部の区画では同じ (0.589 と 0.594)。「観測だけ」は知識を先にした形が上だが、この区画は上位 20 件の中で prompt を除いて繰り上げるので、prompt の多い方式ほど 10 件に満たず不利になる (`docs/pr-e0.md` の「区画の意味」)。「問いより前に書かれた文書だけ」は混ぜた形が上 (0.547 と 0.481)。前に打った同じ依頼の prompt も、それだけで答えになるため。
- **既定は `hybrid-kf` にする**。知識が先という今の並びを変える理由は評価からは出ない。「前に書かれた文書だけ」の区画で 0.07 低いのは、この並びの代わりに受け入れる。`vec-kf` との差は有意でないが、全文検索を混ぜておけば、識別子やエラー文をそのまま引く問いに強い (agent の検索語では hybrid がわずかに上、有意ではない)。問いのベクトルが作れないとき (オフラインで手元のモデルも無い) は、そのまま全文検索だけに落ちる。

### 速さ

| 部分 | この PC (WSL x86) p50 / p95 | M1 iMac (8 GB) p50 / p95 |
|---|---|---|
| 全文検索 (e0、`oboete search --all`、起動込み) | 308 / 767 ms | 258〜270 / 889〜951 ms (D2、test 分の 112 問) |
| 問いのベクトル化 (Workers AI、PR-A2) | 128〜171 / 161〜692 ms | (回線しだい) |
| sqlite-vec 全件 fp32、k=100 | 285〜307 / 292〜323 ms | 294〜298 / 306〜315 ms |
| sqlite-vec 全件 int8、k=100 | 282〜287 / 286〜305 ms | 215 / 224〜226 ms |
| bit の候補 400 件 + fp32 の並べ直し、fp32 を普通の表から読む (全体) | 81 / 82 ms | 90 / 113 ms |
| 同じく fp32 を sqlite-vec の表から読む (全体) | 269 / 285 ms | 188 / 208 ms |

- 17.8 万件 × 1,024 次元、100 問、Python の sqlite3 + sqlite-vec 0.1.9 (`vecbench.py`)。幅は 2 回の実行の値。「全体」は bit の候補を引き、その 400 件の fp32 を 1 件ずつ読み、cos で並べ直すまで。上位 10 件の fp32 全件との一致は 0.986〜0.987、上位 100 件は 0.905〜0.906。int8 の上位 10 件の一致は 0.990。
- sqlite-vec の表はベクトルを塊で持つので、1 件ずつ読み戻すと遅い。並べ直しに使う fp32 は、1 行に 1 ベクトルの普通の表に置く。
- M1 でもベクトル検索は fp32 の全件のままで予算に収まる。全文検索とベクトル検索を並行に走らせれば、MCP 検索はこの PC で p95 約 1 秒 (ベクトル化 692 + 全件 323 ms が長いほう) になり、予算 (最も遅い端末で p95 1.5 秒) に収まる。M1 の全文検索は D2 で測った (p95 889〜951 ms、「D2 の確かめ」)。
- bit + 並べ直しは fp32 全件の 3 分の 1 前後の時間で、上位 10 件はほぼ同じ。int8 は M1 で 3 割速いだけ。PR-F の自動注入 (予算 300 ms) ではこの形を使う。MCP 検索も同じ形にするかは D2 で nDCG を比べて決める。
- M1 の計測中、メモリ 8 GB のうち 7.4 GB が使われ、2.7 GB が圧縮に回っていた (ほかのアプリを含む。計測の DB は 3 つの表で 954 MB)。18 万件の store を検索するたびに 730 MB を読む形で、8 GB の端末が苦しくならないかは D1 の実装で測る。
- 大きさ: fp32 のベクトルは 17.8 万件で約 730 MB。今の普段の store (数千件) では数 MB。claude-mem を普段の store に取り込む PR-H の後に 730 MB になる。bit の索引は 23 MB の上乗せ (並べ直し用の fp32 は残る)。fp32 の全件索引を sqlite-vec にも持つと、同じ 730 MB がもう 1 つ要る。

## 分け方

- **D1 = 文書のベクトル**:
  - fp32 のベクトルは 1 行 1 ベクトルの普通の表に置き (並べ直しと `reindex` の元)、sqlite-vec の `vec0` には種類で分けて引ける索引を置く (fp32 の全件か bit かは D2 の比較で決める)。store の埋め込み器を記録する `embedder_id`。
  - observe が新しい文書を関門に通して Workers AI でベクトルにする (1 回 100 件、1 日の neuron の上限付き)。
  - `oboete reindex` で全件を作り直す。
  - 設定は `[embedding] provider = "workers-ai"` を足す (既定は `none` のまま)。
- **D2 = hybrid 検索**:
  - CLI・MCP・viewer の検索を `hybrid-kf` の形にする。問いを関門に通して Workers AI でベクトルにし、sqlite-vec で種類ごとに上位 100 件を取る。全文検索と並行に走らせ、種類ごとに RRF でまとめ、知識を先にする。
  - test 分で §3.3 の合格線を 1 回だけ測り、通れば `workers-ai` の store では hybrid を既定にする。この測定の束は §3.1 どおり各方式の上位 50 件にする (dev の spike は PR-B2 と同じ上位 20 件の束。`docs/pr-b.md` の決定 6: 深さ 50 は判定器の信用が決まってから足す)。
- **D3 = 手元のモデル**: fastembed の bge-m3 で、オフライン時の問い合わせと、ローカルだけの形 (決定 5) の文書のベクトルを作る。`provider = "local"` を足す。

## D1 で決めたこと

1. **ベクトルの正本は普通の表 `embeddings`** (文書ごとに 1 行: 文書の id、モデル名 `bge-m3`、埋め込んだ文の SHA-256、長さ 1 にそろえた fp32 1,024 個、索引に入れたかの印)。sqlite-vec の `vec_docs` はここから作り直せる派生の索引で、fp32 ではなく符号の bit だけを持つ (上の計測: fp32 は sqlite-vec から読み戻すと遅い)。
   - `vec_docs` は repo と種類 (知識 `k` = 観測・要約、prompt `p`) で分けて持つ (sqlite-vec の partition key)。MCP の既定の範囲は今の repo で、PR-F の自動注入は 300 ms の予算なので、repo の中だけを引けるようにする。
   - partition key は書き換えられない (sqlite-vec 0.1.9: `UPDATE on partition key columns are not supported yet`)。repo の移し替え (C1 の `rekey_paths`) は古いキーの索引の行を消し、印を戻す。次の observe が `embeddings` から索引だけを作り直す (Workers AI は呼ばない)。
   - モデル名は「どこで動かしたか」ではなく「どのモデルか」。Workers AI と手元の同じ重み (D3) は同じ空間なので作り直さない (決定 5)。
2. **observe の最後に、ベクトルの無い文書を新しい順にベクトルにする** (`provider = "workers-ai"` のときだけ)。埋め込む文は spike と同じ (観測は `kind: title` と本文、要約は本文、prompt は先頭 1,000 字) で、送る前に関門 (`redact::outbound`) を通す。prompt は関門を通してから 1,000 字で切る (先に切ると、切れ目をまたぐ鍵が規則に合わなくなり、ほぼ全体が送られる)。
   - 1 回の observe で 20 リクエスト (2,000 件) まで。observe はロックを持ったまま動くので、ほかの session の要約を長く待たせない。
   - 1 日 200 リクエストまで (`daily_requests`)。spike の文で 1 件 0.46 neuron なので約 9,000 neuron、無料の 1 日 10,000 の内側。claude-mem を取り込んだ後の 18 万件は、`oboete reindex` を打たなければ約 9 日で埋まる。
   - 文書を消す (`delete_doc`・`delete_session`) と、そのベクトルと索引の行も同じトランザクションで消える。消した記憶のベクトルは残さない。
   - 文書を書き換える経路は今は無い。書き換えを足す PR は、そのベクトルも消す (#46)。
3. **`oboete reindex`**: 索引を `embeddings` から作り直し、ベクトルの無い文書を 1 日の上限なしで全部ベクトルにする。
4. **鍵は Workers AI の読み取りだけの token** (`key_file`、既定 `~/CF_WORKERS_AI_KEY.md` の 2 行目)。アカウント全体の鍵は使わない (提案の 334 行目)。この権限だけでモデルを呼べることを確かめた。`account_id` は `config.toml` に書く。
5. **sqlite-vec はバイナリに組み込み**、store を開く前に全接続へ登録する (`sqlite3_auto_extension`)。記録 hook の速さは変わらない (`oboete replay` の fixture: プロセス内 p50 51 µs、起動込み p50 9 ms / p95 12 ms)。バイナリは 10.9 MB → 11.0 MB。

### D1 の確かめ

- 単体テスト: 1 回の要求は 100 件以下かつ件数 × 最長 5 万字以下 / bit は符号で先頭の bit から / 2 回目の observe は要求を出さない / 保存したベクトルの長さが 1 / repo の移し替えの後、要求なしで新しいキーの索引に入る / 文書を消すとベクトルと索引の行も消える / 1 日の上限が 0 なら要求しない / 送る文から秘密が伏せられる。
- 本物の Workers AI で (普段の store の写し、観測 129・要約 16・prompt 12): 3 回の要求 (100・53・4 件) で 157 件、4.6 秒。保存したベクトルと、同じ文を送り直したベクトルの cos は 0.999998。「リポジトリのキーを取得元のURLにする話」を bit の候補 20 件から fp32 で並べ直すと、1 位は「リポジトリキーは絶対パスから正規化URLへ」の観測。

## D2 で決めたこと

1. **検索の入口 (MCP・CLI・viewer) はすべて同じ `search::find` を通る**。`provider = "workers-ai"` なら `hybrid-kf`、それ以外は今までの全文検索。問いのベクトル化は全文検索と並行に走らせる。ベクトルが作れないとき (オフライン、鍵が無い、3 秒の timeout) は、全文検索だけの結果を返し、stderr に 1 行出す。
2. **全文検索は 1 回だけ引き、上位 100 件を種類で分ける** (#46)。最初は知識と prompt を別々に引いていたが、それだと bm25 の順位付けを 2 回、全件に対してすることになる。17.8 万件の store で p95 1.3〜1.4 秒かかった (全文検索だけなら 0.85〜0.88 秒)。spike と同じ形 (e0 の 1 本を分ける) に戻して p95 1.03 秒。知識が先に並ぶので、全文検索側の 100 件はほぼ知識で埋まる。prompt はほぼベクトル側だけで並ぶ (spike の `hybrid-kf` と同じ)。
3. **ベクトル側は種類ごとに、bit で 400 件を引き、fp32 で並べ直して上位 100 件**。repo を絞るときは partition key で絞る。spike で残した「MCP 検索も bit + 並べ直しにするか」は、test 分の測定がこの形そのものなので、その結果で決める。
4. **文書 1 件の取り出し (`get`) は、元の表から id で引く**。`fts` は `doc` を索引の無い列として持つので、`WHERE doc = ?` は索引を頭から全部読む。17.8 万件の store で 1 件 119 ms かかった。hybrid はベクトルだけで見つかった文書を最大 `limit` 件取り出すので、1 回の検索が 1.0〜1.4 秒になっていた。MCP の `get` と viewer も同じ関数なので、そちらも速くなる。
5. **`oboete eval --method hybrid`** は、問いを 1 回ずつベクトルにする。作れなければ run を止める (黙って全文検索の run にならないように)。

### D2 の確かめ

速さ (17.8 万件の評価用 store、test 分の 112 問、`oboete search --all --limit 10`、起動込み、`latency.py`):

| | この PC p50 / p95 | M1 iMac p50 / p95 |
|---|---|---|
| 全文検索だけ | 305〜312 / 852〜914 ms | 258〜270 / 889〜951 ms |
| hybrid (全文検索 2 回の形) | 610〜611 / 1,302〜1,404 ms | — |
| hybrid (全文検索 1 回、既定) | 448 / 1,029 ms | 約 1.1 s (見積もり) |

- M1 の hybrid は測っていない。測るには Workers AI の鍵を iMac に置くことになり、鍵の置き場所が 1 つ増えるため。見積もりは、M1 の全文検索 p95 (889〜951 ms) に、D0 で測ったベクトル側の全体 (M1 p95 113 ms) を足したもの。問いのベクトル化 (p95 161〜692 ms) は全文検索と並行に走るので、全文検索の時間に隠れる。予算 (最も遅い端末で p95 1.5 秒) の内側。
- 評価用の store には spike のベクトルをそのまま入れた (`load_vectors.py`、Workers AI は呼ばない)。`text_sha` は spike の文 (prompt は 1,000 字で切ってから関門) から作ったもので、D1 の c1ac296 以降 (関門を通してから切る) とは違うことがある。`text_sha` は今は照合に使っていないので、測定には影響しない。

精度 (test 分 112 問、答えのある 110 問。判定は 4 方式の上位 50 件の束、`claude-sonnet-5`、1,528 回の呼び出し。hybrid の値の右肩の字は、hybrid が有意に上回る方式 (a = claude-mem 窓なし、b = claude-mem、c = e0。p < 0.05)):

| 区画 (問いの数) | claude-mem 窓なし | claude-mem | e0 (全文検索) | hybrid (D2) |
|---|---|---|---|---|
| 全部 (110) nDCG@10 | 0.244 | 0.202 | 0.443 | **0.545**ᵃᵇᶜ |
| 全部 recall@10 | 0.053 | 0.040 | 0.144 | **0.197**ᵃᵇᶜ |
| 全部 hit@10 | 0.536 | 0.400 | 0.827 | **0.955**ᵃᵇᶜ |
| 日本語の問い (103) nDCG@10 | 0.228 | 0.191 | 0.439 | **0.546**ᵃᵇᶜ |
| 英語の問い (7) nDCG@10 | 0.491 | 0.363 | 0.514 | 0.527 |
| 90 日以内の prompt (29) nDCG@10 | 0.219 | 0.230 | 0.396 | **0.515**ᵃᵇᶜ |
| 問いより前に書かれた文書だけ (94) nDCG@10 | 0.215 | 0.072 | 0.431 | **0.538**ᵃᵇᶜ |
| 観測だけ (109) nDCG@10 | 0.260 | 0.216 | 0.442 | **0.539**ᵃᵇᶜ |

- **§3.3 の合格線を満たす**。nDCG@10 は e0 より +0.102 で有意。recall@10 はどの区画でも下がらない (いちばん差の小さい英語の 7 問でも 0.145 → 0.151)。どの区画でも claude-mem (窓あり・なし) を下回らない (決定 22)。
- 英語の問いは 7 問しかなく、MRR と hit@10 は e0 のほうが上 (0.692 と 0.643、1.000 と 0.857)。どちらも有意ではない。agent の検索語は 2 問で、採点できない。
- dev 分の spike (`hybrid-kf` 0.589、e0 0.438) より差が小さいのは、test 分が別の問いであることに加え、束が違う (dev は 8 方式の上位 20 件、test は 4 方式の上位 50 件) ため。
- ベクトル側は bit + fp32 の並べ直しで測った。spike で残した「MCP 検索も bit + 並べ直しにするか」は、この形で合格したので、この形にする。
- 評価の run は、問いと同じ session の文書を先に外してから合わせる (Codex の指摘で 2 回直した)。全文検索側はその session 以外の上位 100 件、ベクトル側は bit の候補を取る段階で外し、4k 件に足りなければ深く取り直す (1 つの session に最大 3,955 件の知識があり、候補の 400 件を埋めうるため)。合わせた後で外していた最初の版との差は nDCG@10 で 0.003。
- 全文 (`report-test.txt`) と run は `~/.oboete/eval/runs-test/` にある (実データなので repo には入れない)。

## 再現

```sh
cd docs/spike/pr-d
python3 export.py "$(command -v oboete)"          # ~/.oboete/eval/vec/docs.jsonl (関門を通した文)
uv run --with numpy python embed.py               # docs.npy / queries.npy (約 14 分、約 $0.80)
uv run --with numpy python runs.py                # runs-d0/vec-bge-m3.trec, hybrid-rrf.trec
uv run --with numpy python runs_kf.py             # runs-d0/vec-kf.trec, hybrid-kf.trec
cp ~/.oboete/eval/runs-d0/*.trec ~/.oboete/eval/runs/   # judge.py と report.py は runs/ だけを読む
cd ../../eval
python3 judge.py dev 312 1400                     # 足りない組だけ判定する。0 件になるまで繰り返す
uv run --with ranx python report.py dev
cd ../spike/pr-d
uv run --with numpy --with sqlite-vec python vecbench.py <作業用の DB のパス>   # 約 1.7 GB
```

`runs-d0/` に分けて書くのは、判定の前に run を見比べられるように。`runs/` に写した時点で、束は 8 方式になる。

D2 の test 分 (評価用 store の写し `<home>` で。`config.toml` に `[embedding] provider = "workers-ai"` と `account_id`):

```sh
oboete --home <home> search x                     # 一度開いて表を作る
uv run --with numpy python docs/spike/pr-d/load_vectors.py <home>/oboete.db
oboete --home <home> reindex                      # 索引だけ作る (要求 0 件)
R=~/.oboete/eval/runs-test
oboete --home <home> eval ~/.oboete/eval/queries.jsonl --depth 50 --method hybrid > $R/hybrid-d2.trec
oboete --home <home> eval ~/.oboete/eval/queries.jsonl --depth 50 --method fts > $R/e0-trigram.trec
cp -p ~/.oboete/eval/runs/claude-mem*.trec $R/     # -p: report.py は claude-mem の run の時刻を「今」にする
cd docs/eval
OBOETE_EVAL_DEPTH=50 OBOETE_EVAL_RUNS=$R python3 judge.py test 112 2000
OBOETE_EVAL_DEPTH=50 OBOETE_EVAL_RUNS=$R uv run --with ranx python report.py test
python3 ../spike/pr-d/latency.py "$(command -v oboete)" <home> ~/.oboete/eval/queries.jsonl test
```

## 限界

- 判定器 (sonnet) の信用はまだ測っていない (B3 の人の正解 50 組)。
- 問いの 9 割近くが developer の prompt で、agent の検索語は 22 問しかない。agent が MCP で引く形での差は、B3 と E 系で問いを足して見る。
- test 分は D2 で 1 回開けた。これからの工夫 (E1〜E6 など) の比較は dev 分で行い、test 分は既定を変える PR で 1 回だけ使う。
