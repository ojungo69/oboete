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

§2・§4 の未確認の数字を実測に置き換える。コードは出荷しない (結果とスクリプトの置き場は A2 の PR で決める)。

| 計測 | 結果 |
|---|---|
| Workers AI と fastembed の bge-m3 を実データ 100 件で比べる (cos の最小、上位 10 件の一致) | 未 |
| 8,000 字の文での Workers AI の入力上限と `truncate_inputs` | **上限は 8,192 トークン**。日本語はおよそ 0.6 トークン / 字で、16,000 字 (9,687 トークン) は `Sequence too long: 9687 > 8192` の 400。`truncate_inputs: true` なら 32,000 字でも 200 で先頭を使う。8,000 字までは切らずに通る。8,000 字全体 (切らずに通る長さ) のベクトルと先頭 1,000 / 2,000 / 4,000 字のベクトルの cos は 0.910 / 0.941 / 0.973 (合成文での値)。出力は 1,024 次元、pooling は `cls` (2026-09-24、合成文) |
| 日本から Workers AI への往復 p50 / p95 | 短文 1 件: **p50 128 ms、p95 161 ms**、最大 240 ms (30 回)。300 字 × 100 件の一括: 507〜878 ms (5 回)。WSL の自宅回線から REST で直接 (2026-09-24、合成文) |
| oboete と claude-mem の注入の実トークン数 | 未 |
| DO / D1 で trigram FTS5 を作る 1 文 | **D1 は使える**: `CREATE VIRTUAL TABLE fts USING fts5(body, tokenize='trigram')` が通り、日本語の部分一致 (`MATCH '精度を上'`)・`snippet()`・`bm25()` も動く (使い捨ての D1 を作って消した)。DO の SQLite は Worker を置かないと試せないので PR-G で確かめる |
| M1 iMac での手元モデルの読み込み時間と RAM | 未 (iMac の電源が入ってから) |
