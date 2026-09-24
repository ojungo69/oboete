# PR-A — 外へ出す関門と計測 spike (2026-09-24 着手)

仕様の正本は `docs/research/search-sync-proposal-2026-09-23.md` の §4.8 と §7 の PR-A 行。ここは実装で決めたことと計測結果だけを置く。
PR は 2 つに分ける: **A1 = 関門 (出荷するコード)**、**A2 = 計測 spike (出荷しないコードと結果)**。A2 の実データの送信は A1 の関門を通してから行う。

## A1: 関門

今のコードで oboete の外へ出るのは、要約の provider chain だけ (`observe::process_session` → `provider::Chain::summarize` → 無料 API の HTTP 本文 / CLI の stdin・`task.md`)。埋め込み・同期・判定はまだ無い。

### 決めたこと

1. **関門は `redact::outbound(text)` の 1 つの関数**。`<private>` などのブロックを外し (`strip_blocks`)、gitleaks 規則で伏せ字にする。要約の入力も、後の PR の埋め込み・同期・判定も、この関数を通してから外へ出す。
2. **関門の `strip_blocks` は閉じたブロックだけを外す**。閉じていない `<private>` で以降を全部消すのは、今までどおり記録時の prompt だけにする (`docs/m1.md` 決定 15)。理由: 道具の出力や要約には `<private>` という語がそのまま出てくる (この repo の `hook.rs` の説明文、`<private>` について話したセッションの要約)。関門で閉じていないタグの後ろを全部消すと、そのセッションの残りが要約器に渡らなくなる。
3. **要約の入力では、関門をイベントの欄ごとに通す** (`observe::render`)。transcript 全体にまとめて通すと、ある道具の出力の `<private>` と、ずっと後の返答の `</private>` の間が丸ごと消える。`build_prompt` の伏せ字は関門に置き換える (二重に通さない)。
4. **応答の読み取りに上限 (1 MiB)**。無料 API の本文、CLI の stdout、codex の `last.json` のどれも、上限なしで読んでいた。壊れた provider や乗っ取られた provider が巨大な応答を返しても、メモリを食い尽くさないようにする。CLI 側は #32 で入れた (stdout を読まずに終了を待っていたので、64 KB を超える応答で子が止まる不具合も一緒に直した)。無料 API の本文はこの PR。
5. **同期除外の判定は A1 に入れない** (§4.8 の 3)。判定に使う情報 (session が触れた repo の集合、`.oboete.toml` の `sync = false`、`oboete sync exclude`) はどれも PR-C で入る。何も渡さない引数を先に作っても、テストが自分の実装をなぞるだけになる。A2 の実験は決定 11 の「除外が無ければ全部送ってよい」に当たる (今は除外の設定が 1 つも無い)。PR-C で除外の判定を関門に足し、「除外した repo の文書・問い合わせは外への要求が 0 件」の回帰テストもそこで付ける (issue #30 に移す)。

### 回帰テスト

- 偽の鍵と `<private>…</private>` を含むイベントから要約の要求を組み立て、無料 API の本文・CLI の stdin・`task.md` のどれにも鍵と private の中身が無いこと。
- 道具の出力に閉じていない `<private>` があっても、その後ろのイベントが要約の入力に残ること。
- 1 MiB を超える応答はエラー (invalid output) になること (CLI は #32 の `run_cli` のテスト、無料 API は localhost の 1 回だけ答える HTTP サーバーで確かめる)。

## A2: 計測 spike

§2・§4 の未確認の数字を実測に置き換える。コードは出荷しない (スクリプトは `docs/spike/pr-a2/`、結果はこの表)。

| 計測 | 結果 |
|---|---|
| Workers AI と fastembed の bge-m3 を実データ 100 件で比べる (cos の最小、上位 10 件の一致) | **一致検査に通る (この PC、WSL の x86_64)**。関門を通した実データ 271 件 (oboete の観測・要約・prompt 全部と、claude-mem の観測 100 件・prompt 40 件。30〜8,000 字) と問い合わせ 57 件で、同じ文の cos は最小 0.99995・中央値 1.00000。上位 10 件の一致は、手元の問い合わせ × Workers AI の文書 (オフライン時の問い合わせ) で最小 9・平均 9.98、全部手元 (ローカルだけの形) で最小 9・平均 9.95。9 件を下回った問いは 0 件。関門で変わった文は 328 件中 4 件 (2026-09-24、fastembed 7.1.0・`max_length` 8,192) |
| 8,000 字の文での Workers AI の入力上限と `truncate_inputs` | **上限は 8,192 トークン**。日本語はおよそ 0.6 トークン / 字で、16,000 字 (9,687 トークン) は `Sequence too long: 9687 > 8192` の 400。`truncate_inputs: true` なら 32,000 字でも 200 で先頭を使う。8,000 字までは切らずに通る。8,000 字全体 (切らずに通る長さ) のベクトルと先頭 1,000 / 2,000 / 4,000 字のベクトルの cos は 0.910 / 0.941 / 0.973 (合成文での値)。出力は 1,024 次元、pooling は `cls` (2026-09-24、合成文) |
| 日本から Workers AI への往復 p50 / p95 | 短文 1 件 (30 回): 1 回目 **p50 128 ms、p95 161 ms**、最大 240 ms。2 回目 **p50 171 ms、p95 692 ms**、最大 722 ms。回線や時間帯で大きく揺れる (1 回目の p95 は 30 件中 28 番目の値で、2 回目から正しい 29 番目にした)。300 字 × 100 件の一括: 507〜878 ms、2 回目 544〜1,556 ms (各 5 回)。WSL の自宅回線から REST で直接 (2026-09-24、合成文) |
| oboete と claude-mem の注入の実トークン数 | この repo で **oboete 1,955 トークン (2,791 字)、claude-mem 2,393 トークン (5,835 字、観測 50 件)**。claude CLI (opus) の `usage` を、注入文あり / なしの同じ指示で引き算した値 (なしは 31,755)。oboete は日本語で 0.70 トークン / 字。今の 2,791 字は要約 3 件・観測 12 件の件数の上限で決まっていて、字数の上限 4,000 字 (約 2,800 トークン) には届いていない (2026-09-24) |
| DO / D1 で trigram FTS5 を作る 1 文 | **D1 は使える**: `CREATE VIRTUAL TABLE fts USING fts5(body, tokenize='trigram')` が通り、日本語の部分一致 (`MATCH '精度を上'`)・`snippet()`・`bm25()` も動く (使い捨ての D1 を作って消した)。DO の SQLite は Worker を置かないと試せないので PR-G で確かめる |
| M1 iMac での手元モデルの読み込み時間と RAM | **M1 でも一致検査に通る** (2026-09-24、M1 8 GB、macOS 26.6、同じ `spike.rs`)。実データ 297 件と問い合わせ 62 件 (この日の store から `a2_corpus.py` で作り直した 359 件、30〜8,000 字) で、同じ文の cos は最小 0.99999・中央値 1.00000。上位 10 件の一致は、オフライン時の問い合わせで最小 9・平均 9.97、ローカルだけの形で最小 9・平均 9.95。9 件を下回った問いは 0 件。読み込みは 2 回目以降 **2.36 秒** (初回はモデルの取得込みで 105 秒)、359 件を 1 件ずつ変換して 122〜138 秒 (長い文を含む平均 0.34〜0.38 秒 / 件)、ピークの RAM **3.10〜3.30 GB** (`/usr/bin/time -l`)。macOS 版のバイナリは 29.7 MB |

### A2 でわかったこと (後の PR への入力)

1. **この PC では手元の fastembed が Workers AI と同じ `embedder_id` を名乗れる** (§2.3 の 3)。一致検査は端末の実行環境ごとなので、M1 (arm64 の ONNX Runtime) でも同じスクリプトで確かめ、通った (上の表)。
2. **Workers AI の bge-m3 は 1 回の要求で「件数 × いちばん長い文のトークン数」が 60,000 まで** (モデルの context window。短い文も一番長い文の長さまで詰め物をして数える)。20 件の束が `Max context reached 116200 tokens but model supports only 60000` で落ちた。PR-D の一括送信は「100 件まで」だけでなく、長さの近い文でまとめて、件数 × 最長の長さを抑える。
3. **手元の bge-m3 の重さ** (WSL、32 スレッド、2 回目以降の読み込み): 問い合わせ 1 件は読み込み 2.1〜2.5 秒 + 変換 95〜142 ms、ピークの RAM 1.79 GB。8,000 字の文 1 件は 5.3 秒・2.79 GB。1,000 字以下の 100 件 (8 件ずつ) は 10.1 秒・1.89 GB。**長い文を 8 件ずつ束ねると 17.8 GB** まで膨らんだので、手元で文書を作るときは長い文を 1 件ずつにする (`spike.rs` もこの後 1 件ずつに直した。M1 で同じスクリプトを走らせても膨らまない) (初回はモデルの取得込みで 46 秒、約 2.3 GB)。
4. **fastembed を入れると oboete のバイナリは 10.7 MB から 36.1 MB** (rustls、ONNX Runtime を静的に同梱。提案書の 29.65 MB は別構成の値)。
5. **fastembed の int8 版 (`Bgem3Model::BGEM3Q`、`gpahal/bge-m3-onnx-int8`、560 MB) は一致検査に通らない** (2026-09-24、この PC、同じ 359 件)。cos は最小 0.846 (8,000 字の prompt)・中央値 0.979、上位 10 件の一致はオフライン時の問い合わせで最小 7 (62 問中 15 問が 9 件未満)、ローカルだけの形で最小 6 (35 問)。読み込みも 2.35 秒で fp32 と変わらず、ピークの RAM も 3.3 GB で減らない。軽くする手としては使えない。
6. **手元の問い合わせは、読み込みだけで PC・M1 とも約 2.4 秒かかる**。プロセスを起こすたびに読むと、MCP 検索の予算 (最も遅い端末で p95 1.5 秒) に入らない。常駐させると、agent ごとの MCP プロセスがそれぞれ 2〜3 GB を持つ。手元のモデルを使う形 (ローカルだけの形、オフライン時の問い合わせ) は、この前提で D3 で設計する。

### 再現

スクリプトは `docs/spike/pr-a2/` に置いた (出荷しない)。Cloudflare の鍵は `cf.py` が `~/CF_API.md` から読み、表示しない。

- `wai_limits.py` (入力上限と往復時間)、`d1_trigram.py` (使い捨ての D1 を作って消す): 合成文だけを送る。
- 一致検査: `python3 a2_corpus.py > <dir>/raw.jsonl` (oboete と claude-mem の DB を読み取り専用で開く) → 使い捨ての checkout で `cp docs/spike/pr-a2/spike.rs src/ && git apply docs/spike/pr-a2/spike-wiring.diff && cargo build --release --features spike` → `target/release/oboete spike-embed --cache <dir>/cache < <dir>/raw.jsonl > <dir>/local.jsonl` (関門を通した文と手元のベクトル。ピークの RAM は Linux では stderr の `VmHWM`、macOS では `/usr/bin/time -l` の maximum resident set size) → `python3 a2_compare.py <dir>`。Workers AI へ送るのは `local.jsonl` の関門を通した文だけ。
- 注入のトークン数: `oboete inject` と claude-mem の `GET /api/context/inject?project=oboete` の出力を、scratch の cwd から `claude -p --model opus --setting-sources project --strict-mcp-config --no-session-persistence --output-format json` に渡し、`usage` の入力トークンの合計を注入なしの値と引き算する (`OBOETE_SKIP=1`。user 設定を読まないので oboete と claude-mem の hook は動かず、claude-mem の session 数が変わらないことも確かめた)。
