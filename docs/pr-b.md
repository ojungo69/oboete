# PR-B — 評価器 (2026-09-24 着手)

仕様の正本は `docs/research/search-sync-proposal-2026-09-23.md` の §3 (評価セット・指標・合格線) と §7 の PR-B 行、決定 22 (claude-mem を下回らない)。ここは実装で決めたことと計測結果だけを置く。

PR は 3 つに分ける。

- **B1 = 取り込みと実行 (出荷するコード)**: `oboete import claude-mem` と、隠しコマンド `oboete eval`。
- **B2 = 評価セット・判定・基準値 (出荷しないスクリプトと数字)**: 問いの束を作り、候補を判定し、全文検索と claude-mem の基準値を出す。
- **B3 = 特別な区画と判定器の信用**: 答えの無い問い、前提違い、覆った決定、提案と決定の取り違え、理由の無い変更語り、人の正解 50 組。

## 決めたこと

1. **評価のデータは repo に入れない。** 問いも判定も、owner の作業記録 (claude-mem の prompt と観測) そのものです。この repo は後で公開するかもしれないので、置くのは作るスクリプトと結果の数字だけにする。問い・qrels・run・判定の記録は `~/.oboete/eval/` に置く (owner の機械の中だけ)。claude-mem の DB は `sqlite3 ~/.claude-mem/claude-mem.db ".backup <写し>"` で写してから読む。WAL のままでも一貫した写しが取れ、本体を書き込みで開かない。

2. **claude-mem の記憶は `oboete import claude-mem <db>` で評価用の home に入れる。** 取り込みは決定 1 で出荷する機能 (普段の store へは PR-H)。同じコードを評価に使えば、測るのは出荷するコードになる (§3.2)。
   - 取り込む文は関門 `redact::outbound` を通す (決定 1 の「取り込み時」の 1 回目)。
   - 元の id は新しい表 `imports(source, source_id, doc)` に残す。再取り込みで二重にならず、claude-mem の検索結果を oboete の doc id に直すのにも使う。claude-mem の id は DB ごとに 1 から始まる (Windows の写しにも observation 1 がある) ので、`source` は `claude-mem:<その DB の migration 記録 (`schema_versions`) の最初の時刻のハッシュ>` にする。DB を作った時刻なので、session が消されても写しや移動でも変わらず、写しを取り込んだ後に本体を取り込んでも増えた分だけが入る。
   - 取り込み済みの行は session を書く前に飛ばす (developer が消した session を空の行で戻さない)。session id の無い行は 1 行ずつ別の session にする (無関係な行が 1 つにまとまらない)。
   - 対応: 観測は `type` → `kind` (`KINDS` の外、たとえば壊れた `discovery>` は `discovery`)、`title`、本文は `narrative` に `facts` を 1 行ずつ足したもの (`narrative` が空の 1,926 行は `facts` だけ、両方空なら取り込まない)。要約は `request` / `investigated` / `learned` / `completed` / `next_steps` を見出し付きでつないだもの。prompt は hook と同じ正規化 (harness 通知は捨て、`<private>` などを外す)。session は `sdk_sessions` から。
   - repo は `claude-mem:<project>` にする。claude-mem の project は `free-mem` や `公式サイト` のような名前で、oboete の repo キー (PR-C で origin URL) に機械的には直せない。直すのは PR-H (PR-C の repo 別名を使う)。
   - **B1 では既定の home (`~/.oboete`) への取り込みを断る** (`--home` 必須)。repo の対応が決まる前に普段の store に入ると、注入が repo ごとに引けない行が 16 万件入るため。PR-H でこの制限を外す。

3. **`oboete eval <問いの JSONL>` は隠しコマンドにして出荷する。** feature flag にすると CI で別のビルドが要り、放っておくと壊れる。コードは 1 画面で、出荷している検索関数をそのまま呼ぶ。入力は 1 行 1 問の JSONL (`{"qid","text"}`、任意で `session`)。`session` があれば、その会話の文書 (問いの後に書かれた答えを含む) を順位を付ける前に除いて次の文書を繰り上げる (§3.1)。qid は空白を含まない 1 語に限る。出力は TREC の run (`qid Q0 doc rank score method`)。方式は今は `fts` (今の `search::search` を全 repo で) だけで、PR-D で `vec` / `hybrid`、PR-E で各工夫を足す。
   - もう 1 つの隠しコマンド **`oboete gate`** は、標準入力を関門 (`redact::outbound`) に通して標準出力に出す。評価のスクリプトが外 (判定器) へ送る文のうち、store を通っていないもの (transcript から拾った agent の検索語) をこれに通す。関門の実装を Python に写さないため。

4. **指標は ranx (Python) で出す。** `docs/eval/report.py` が run と qrels を読み、nDCG@10・recall@10/50・MRR@10 と有意差を出す (出荷しない)。ranx は `uv` の一時環境で入れる。

5. **claude-mem の基準値は、動いている worker の検索を読み取りだけで呼ぶ。** `GET http://127.0.0.1:37777/api/search?query=<q>&format=json&type=observations&limit=50` が順位つきの観測 id を返す (2026-09-24 に確認。設定もデータも変えない)。id を `imports` で oboete の doc に直して run にする。写しを取った後に増えた観測は `imports` に無いので捨て、捨てた件数を報告に書く。claude-mem の検索は Chroma (英語用 all-MiniLM-L6-v2) の上位 100 から 90 日より古いものを捨てる作りで、その振る舞いのまま測る。

6. **判定は `claude -p --model sonnet` (サブスク) で、1 回に 1 問 × 10 文書を 0〜3 で採点させる** (UMBRELA の段階をそのまま使う)。要約器 (無料 API の gpt-oss / llama 系と、同じ claude / codex の CLI) とは指示文も役割も別にする。送る問いと文書は関門を通す。
   - **hook を動かさない**: 判定の `claude` は scratch の cwd から `--setting-sources project --strict-mcp-config --no-session-persistence` を付け、`OBOETE_SKIP=1` で呼ぶ。A2 の注入トークンの計測と同じ形で、user 設定を読まないので oboete と claude-mem の hook が動かない (claude-mem の session 数が変わらないことを確認済み)。
   - **予算**: 1 回の実行は 600 回の呼び出し (約 6,000 組、約 300 万トークン) で止め、翌日に続きから再開する。判定済みの組は `~/.oboete/eval/judgments.jsonl` に (問い, 文書, 判定器) で記録して使い回す。同時に走らせるのは 2 本まで。Codex への委譲や PR のレビュー待ちと同じ時間帯に回しても、この PC の作業を止めない量にする。
   - 最初の束は、dev 分の問い 150 件 × 方式ごとの上位 20 件 (全文検索と claude-mem を合わせて最大 40 件)。recall@50 のための深さ 50 は、判定器を信用できると決まってから足す (§3.1 の「束は実験ごとに足す」)。

7. **dev と test の分け方**: session id のハッシュで 70 / 30 に分ける。同じ session の問いは同じ側に入る。test 分は合否を決めるときまで開けない (§3.3)。

## 今の検索で先に分かっていること

今の `search::search` は空白で区切った語の AND です。日本語の文をそのまま問いにすると 1 語の完全一致になり、ほぼ 0 件になります。claude-mem の prompt (日本語 64%) を問いにすると、全文検索の基準値は極端に低く出る見込みです。

これは出荷している検索の本当の弱さで、MCP の `search` に agent が文で問い合わせたときも同じことが起きます。B2 では今の検索をそのまま測り、数字を見てから、語の OR と bm25 で並べる全文検索を PR-E の最初の工夫として比べるかを決めます。意味検索の合格線 (§3.3 の「全文検索だけに比べ +0.03」) の比べる相手は、そのとき強いほうの全文検索にします。

## B1 の回帰テスト

- claude-mem の形の小さな DB (観測・要約・prompt・session 各数行、壊れた type、`narrative` が空の行、harness 通知の prompt、偽の鍵と `<private>` を含む行) を取り込み、件数、kind、本文、repo、fts の行、`imports` の対応を確かめる。鍵と private の中身が store に無いことも確かめる。
- 同じ DB を 2 回取り込んでも行が増えない。
- `eval` が TREC の形で順位 1 から出し、問いごとの件数が深さ以下で、形の壊れた行はエラーになる。
- 既定の home への取り込みを断るのは `main.rs` の 1 行の比較で、手で確かめた。

## B1 の結果

この PC の claude-mem (2026-09-24 の写し、840 MB) を評価用の home に取り込んだ: 観測 152,030 件、要約 13,155 件、prompt 13,185 件、何も残らない行 2,100 件 (harness 通知の prompt と、題も本文も空の観測)。79 秒、ピークの RAM 35 MB、store は 1.3 GB。2 回目は 29 秒で、178,370 行すべてを取り込み済みとして飛ばし、何も足さない。
