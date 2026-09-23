# oboete 意味検索と複数端末同期の設計案 (2026-09-23)

この文書は `docs/plan.md` §2b・§3・§8 と `docs/m1.md` 決定 9 の意味検索・同期部分を置き換える案です。数字にはすべて出典 URL を付けています。「未確認」と書いたものは一次情報で確かめられなかった数字で、判断の根拠にはしていません。「実測(未再現)」は調査担当がこの PC (Ryzen 9 5950X、WSL2) で測った値で、第三者の再測定はまだありません。


---

## 0. owner の決定 (2026-09-23)

§6 の 4 点と、そのとき owner が足した問いへの答え。以下の本文とずれるところは、この節が優先する。

1. **claude-mem の過去の記憶を取り込む** (§6 決定 1、推奨どおり)。`source = claude-mem` を付け、検索には出し、冒頭の自動注入には出さない。
2. **prompt も同期する** (§6 決定 3、推奨どおり)。
3. **クラウドの意味検索に Vectorize を使う** (§6 決定 4、推奨どおり)。初月の請求で実額を確かめ、月 $1.5 を超えたら止める。
4. **有料の埋め込み (Gemini 有料版など) は使わない** (§6 決定 2)。評価で bge-m3 の hybrid が合格線に届かなければ再相談する。
5. **埋め込みの置き場は 2 つの形を選べる** (owner:「クラウドを使わずローカルだけの人は手元でベクトル。クラウドも使う人はクラウドが主で、手元でモデルが動かせるならそれも動かす」)。
   - **ローカルだけの形**: 文書も問い合わせも手元の bge-m3 (fastembed) でベクトルにする。通信しない。同期もクラウド検索も無し。
   - **クラウドを使う形**: 文書のベクトルは Workers AI の bge-m3 で作り、全端末で共有する。手元のモデルが動く端末では、オフライン時の問い合わせ変換に使う。PR-A の一致検査 (cos の最小 0.99 以上、上位 10 件が 9 件以上一致) に通れば、オフライン中にできた文書のベクトルも手元で作ってよい (通らなければ、オンラインに戻ってから Workers AI で作る)。
   - どちらも同じ bge-m3 なので、ローカルだけの形から後でクラウドを使う形に移るときも、一致検査に通っていれば作り直しは要らない (通らなければ `oboete reindex`)。1 つの store に埋め込み器は 1 つ、という §2.3 の決まりは変わらない。
6. **クラウドは記憶の共有と検索の置き場にする。要約はクラウドで動かさない** (owner の問い:「共有する人はクラウドで要約も動かすか、クラウドは置き場か」への答え)。
   - 要約に使う provider の連鎖 (サブスク CLI の claude / codex / agy / grok と無料 API) は各端末で動く。Workers の中ではサブスク CLI を動かせない。
   - クラウドで要約するには生イベント (作業の全記録) を毎回クラウドへ送ることになる。送る量も、伏せ字で拾えない情報が外に出る機会も増える。
   - 記録した端末で要約すれば、同じセッションを 2 か所で要約する重複も起きない。
   - 「ローカルだけの形」の人は要約も含めて全部を手元で完結できる (要約に無料 API を使うかどうかは provider の設定で決まり、ローカル LLM を使うという意味ではない)。
7. **別の端末で clone した同じ repo は、ディレクトリ名に関係なく同じ repo として扱う** (owner の問い:「WSL の A を Mac や Windows で clone して引き継ぐとき、どう同じ A だと判別するのか」への答え)。
   - 今のコードは repo をディレクトリの絶対パスで見分けている (`src/repo.rs`)。このままだと別物として扱ってしまう。
   - §4.2 の 1 (PR-C) で、repo の見分け方を `.git/config` に書かれた GitHub などの取得元 URL (例 `github.com/ojungo69/oboete`) に変える。置き場所やフォルダ名が違っても、取得元が同じなら同じ repo になる。
   - 取得元の無い repo はこれまでどおりパスで見分け、`.oboete.toml` の `repo = "名前"` で同じ名前を付けられるようにする。

### 0.1 仕様のすり合わせ (2026-09-23 夜、3 ラウンド)

8. **claude-mem には触らない** (owner:「claude-mem は勝手に消さないでね」)。停止・削除・設定変更は owner が決める。取り込み (決定 1) は claude-mem の DB を読むだけ。Windows 側の claude-mem データ (`C:\Users\jura\.claude-mem`) も取り込む。iMac にあれば同じ。
9. **残り 4 agent の対応順**: agy → OpenCode → Pi → Cursor (`docs/research/agent-adapters-2026-09-23.md`)。
10. **端末**: この PC (WSL)、Windows 本体、M1 iMac。VPS は使うか未定で、決まるまで設定しない。スマホは保留。
11. **同期から repo ごとに外せる** (repo の `.oboete.toml` に `sync = false`。PR-C で読んで session に印を付け、PR-H で印のある session の行を送信から外す)。既定は全部同期する。外す前に送った分は残るので、消したいときは viewer で削除する (決定 17 で全端末から消える)。
12. **プロンプトごとの自動注入** (`oboete inject`) は、評価で無関係な注入が 10% 以下のときだけ既定で ON にする。
13. **個人の好み** (`preference` の観測) は全 repo のセッションに注入する。
14. **費用**: 月 $5 までは owner に聞かずに使ってよい。Vectorize の月 $1.5 の停止線 (決定 3) はそのまま。
15. **更新**: 新しい版が出たら知らせ、`oboete update` の 1 コマンドで入れ替える。自動では入れ替えない (壊れた版が全端末に一度に入らないように)。
16. **同期を始める前に owner が Cloudflare の二段階認証を ON にする** (2026-09-23 時点で OFF)。同期に使う鍵は全権限キーではなく、端末ごとに取り消せる専用のもの (§4.4)。
17. **削除は全端末とクラウドに効く** (§4.3 の tombstone)。1 台だけ消して他に残す形は作らない。
18. **viewer は常駐させない**。見たいときに `oboete view --open` で開く。
19. **普段の環境では claude-mem と oboete の両方が記録・注入する** (二重注入を受け入れる)。実作業の中でしか両者の欠点は見えないため (例: 2026-09-23、claude-mem は owner が答える前の推奨を「owner の決定」として記録し、oboete は取り消し済みの「1 週間の試用」を注入した)。新しい機能 (4 agent、同期、更新) は、普段の環境に入れる前に隔離ユーザー `oboete-dogfood` で試す。そのユーザーに残っていた旧 TS 版と日次 cron は片付ける。
20. **要約用の API キーのファイルは iMac と Windows にもコピーする** (owner 選択。権限は本人だけが読める形)。VPS は使うと決めたときに改めて決める。

---

## 1. 結論

1. **意味検索は 1 つのベクトル空間にそろえます。** モデルは BAAI の bge-m3 (1024 次元、日本語を含む多言語対応)。クラウドでは Cloudflare Workers AI の `@cf/baai/bge-m3`、手元では同じ重みを fastembed で動かします。**「両方」の中身:** 文書のベクトルはクラウド (Workers AI) だけで作り、手元のモデルはネットが無いときの問い合わせ文の変換にだけ使います。同じ重みでも実行環境が違うと数字がわずかにずれ、混ぜると検索結果が端末ごとに変わるためです (§2.3)。ベクトルは文章の意味を数字の並びにしたものです。モデルが違うと数字の並びも違って比べられないので、クラウドと手元で同じモデルを使います ([Workers AI の bge-m3](https://developers.cloudflare.com/workers-ai/models/bge-m3/)、[fastembed 7.1.0](https://crates.io/crates/fastembed/7.1.0))。
2. **検索と同期は 1 本の線でつながります。** ベクトルは要約 (observe) のときに 1 回だけ作り、「どのモデルで作ったか」の印 (`embedder_id`) を付けて、観測・要約・prompt と同じように同期の op log で全端末へ配ります。どの端末も同じベクトルを持つので、手元の検索は全件の厳密検索になり、結果も端末ごとに変わりません。クラウドの索引 (Vectorize) にも同じベクトルを入れます。使うのは、手元に索引を持たない入口 (Claude アプリ、リモート MCP、将来のスマホ) だけです。plan.md に欠けていたのはこのつながりでした。
3. **精度を上げる工夫を戻します。** 全文検索 (trigram) と意味検索の結果を RRF で合わせ、日付の明示指定、要約時のキーワード追加、似た結果の間引き、自動注入のしきい値、MCP 検索に限った reranker を足します。ただし採用するのは、この PC の実データで作る評価セットで**効果が測れたものだけ**です。
4. **同期は Cloudflare Worker と、ユーザーごとに 1 つの Durable Object (SQLite) を hub にした追記型 op log にします。** cmem Pro の SyncHub と同じ形です ([SyncHub の wrangler.jsonc](https://github.com/thedotmack/claude-mem/blob/main/workers/sync-hub/wrangler.jsonc))。15 万件規模でも Workers Paid の含み枠に収まり、追加費用はほぼ $0 です。Vectorize を使う場合だけ月 $0.08〜1.3 かかります ([Vectorize 料金](https://developers.cloudflare.com/vectorize/platform/pricing/))。
5. **暗号化は今は要りません。** 秘密は保存前に伏せ字にしてあり、置き場は自分のアカウントで、Cloudflare 側で AES-256 の保存時暗号化もかかっています ([D1](https://developers.cloudflare.com/d1/reference/data-security/)、[Durable Objects](https://developers.cloudflare.com/durable-objects/reference/data-security/))。本文まで暗号化するとクラウド側で検索できなくなります。
6. **claude-mem / cmem Pro との比較。** claude-mem の意味検索は英語専用モデル (all-MiniLM-L6-v2) だけで動き、全文検索と結果を合わせません。90 日より古い記憶は日付を指定しない限り意味検索に出ず、ベクトルの保管だけで 4.8 GB あります (出典は §2.9)。cmem Pro は同期の形こそ同じですが、月 $20 か $30 (ページで表記が違う) かかり、検索モデルも公開されていません。oboete は多言語モデル、結果の融合、期限なし、無料で、同等以上になります。

---

## 2. 意味検索

### 2.1 「なぜ Workers AI だけ？」への答え

13 社のクラウド埋め込みを調べました。owner の条件に合わせて、次の 6 つを全部満たすものを探しました。

| # | 条件 | 理由 |
|---|---|---|
| a | 入力を学習や改善に使わない | 開発の記憶を送るため |
| b | 「試用のみ」「本番利用禁止」の条項が無い | 毎日使う道具として正規に使うため |
| c | 無料か、ほぼ無料で日常分と全件の作り直しが収まる | 費用は無料かほぼ無料が条件 |
| d | 業者や鍵を増やさない | 同期の hub と同じ Cloudflare アカウントで済ませるため |
| e | 同じモデルが手元でも動く | オフラインでも手元で意味検索するため (owner の希望) |
| f | 1 ベクトル 1,536 次元以下 | Vectorize の上限 ([Vectorize 制限](https://developers.cloudflare.com/vectorize/platform/limits/)) |

結果、6 つを全部満たすのは Workers AI 上の bge-m3 と qwen3-embedding-0.6b だけでした。どちらも元の次元のまま 1,536 以下に収まり、手元でも動きます。embeddinggemma-300m も a・b・d・e・f は満たしますが、Workers AI ではベータで価格が掲載されておらず、c が未確認です ([モデル頁](https://developers.cloudflare.com/workers-ai/models/embeddinggemma-300m/))。各社が落ちた条件は次のとおりです。

| 業者・モデル | 日本語の質 (MIRACL-HN ja nDCG@10) | 落ちる条件 |
|---|---|---|
| Google gemini-embedding-001 | **75.5 (調べた中で最高)** | a (無料枠は入力を製品改善に使い、人が読む場合がある: [Gemini API 規約](https://ai.google.dev/gemini-api/terms))、c (有料版は後述)、e (クラウド専用)、f (既定 3,072 次元。切り詰めたときの日本語の劣化は未計測: [Gemini embeddings](https://ai.google.dev/gemini-api/docs/embeddings)) |
| Qwen3-Embedding-8B (OpenRouter, $0.01/M) | 74.4 | e (8B は手元の機器に重い)、f (4,096 次元。切り詰めの劣化は未計測: [モデルカード](https://huggingface.co/Qwen/Qwen3-Embedding-8B))。どの provider が学習に使うかは未確認 ([OpenRouter](https://openrouter.ai/docs/guides/privacy/provider-logging)) |
| NVIDIA NIM nemotron-3-embed-1b | 74.2 | a と b (試用規約が本番利用を禁じ、入力を製品改善に使う: [NVIDIA 試用規約 PDF](https://assets.ngc.nvidia.com/products/api-catalog/legal/NVIDIA%20API%20Trial%20Terms%20of%20Service.pdf))、f (2,048 次元で縮められない: [support matrix](https://docs.nvidia.com/nim/nemo-retriever/text-embedding/2.3/support-matrix.html)) |
| PFN PLaMo-Embedding-1B (Workers AI) | MIRACL-HN なし (JMTEB v1 検索 73.25) | e (ONNX/GGUF が無い)、f (2,048 次元: [config](https://huggingface.co/pfnet/plamo-embedding-1b/raw/main/config.json)) |
| Cohere embed-v4 | 69.3 | b (試用鍵は本番・商用に使えず、月 1,000 回まで: [料金](https://cohere.com/pricing)、[上限](https://docs.cohere.com/docs/rate-limits))、e |
| Jina v5-text-small | 69.1 | d (新しい業者)、e (手元版の重みが CC-BY-NC-4.0: [HF](https://huggingface.co/jinaai/jina-embeddings-v5-text-small))。学習には使わず、1,000 万トークンまで無料、その後 $0.05/M ([Jina](https://jina.ai/embeddings/)) |
| Voyage voyage-4 | 63.3 | a (オプトアウトしない限り学習に使う。オプトアウトにはカード登録が要り、無料枠が消えることがある: [規約](https://www.voyageai.com/tos)、[FAQ](https://docs.voyageai.com/docs/faq)) |
| OpenAI text-embedding-3-large | 60.8 | c ($0.13/M、無料枠なし: [料金](https://developers.openai.com/api/docs/pricing))、e |
| Mistral mistral-embed | 公開データなし | 学習の扱いが文書ごとに食い違う ([docs](https://docs.mistral.ai/admin/monitor-comply/privacy-data-controls)、[help](https://help.mistral.ai/en/articles/455207-can-i-opt-out-of-my-input-or-output-data-being-used-for-training))、e |
| Groq | 埋め込みが無い | 公開の埋め込みモデルが無い ([models](https://console.groq.com/docs/models)) |

日本語の数字は、どれも MTEB の結果リポジトリにある MIRACLRetrievalHardNegatives の日本語部分 (`ja`) です ([embeddings-benchmark/results](https://github.com/embeddings-benchmark/results))。

**率直な結論:** Workers AI は利用条件・費用・hub との相性で勝っています。日本語の質では首位ではありません。gemini-embedding-001 のほうが 2.6 点高く、bge-m3 の 72.9 は学習データに MIRACL を含むので、実力より高めに出ている可能性があります ([BGE-M3 論文 Table 8](https://arxiv.org/html/2402.03216v4))。それでも有料版 Gemini にしないのは、オフライン検索 (e) と Vectorize の次元 (f) を失うのに、切り詰めたときの日本語の質が未計測だからです。有料案は §6 の決定 2 で owner に確認します。

**AI Search も見直しました。** plan.md の時点では退けましたが、その後に全文 (BM25、trigram 指定可) とベクトルの hybrid、RRF、reranker、MCP 口が入り、公開ベータ中は無料になっています ([制限と料金](https://developers.cloudflare.com/ai-search/platform/limits-pricing/)、[hybrid](https://developers.cloudflare.com/ai-search/configuration/indexing/hybrid-search/)、[keyword](https://developers.cloudflare.com/ai-search/configuration/indexing/keyword-search/))。それでも主役にはしません。理由は 4 つです。

- ベクトルを取り出す API が見つからない。クラウドだけの別索引になり、手元の結果とずれる。
- 選べる reranker が bge-reranker-base だけで、日本語の JQaRA では nDCG@10 0.2445 と、ただの BM25 (0.458) より悪い ([JQaRA](https://huggingface.co/datasets/hotchpotch/JQaRA))。
- 料金はベータ終了の 30 日前に知らされるだけ。
- 1 文書 1 ファイルで数えると 15 万件は無料枠 10 万件を超え、有料でも hybrid は 50 万件まで。bge-m3 の入力は 512 トークンで切られる ([対応モデル](https://developers.cloudflare.com/ai-search/configuration/models/supported-models/))。

AI Search が持つ精度の工夫は、すべて §2.4 の検索手順に入れてあります。

### 2.2 候補の比較 (クラウドと手元の両方)

| モデル | 日本語検索の質 | 費用 | 大きさ・手元の RAM | 動く場所 | 判定 |
|---|---|---|---|---|---|
| **BAAI bge-m3** (1024 次元、MIT) | MIRACL-HN ja 72.9 ([results](https://github.com/embeddings-benchmark/results/tree/main/results/BAAI__bge-m3)、MIRACL で学習済みのため高めの可能性)、JMTEB v1 検索平均 72.15 ([JMTEB](https://github.com/sbintuitions/JMTEB/blob/9b1e683bc6a2cd2b6b3e170bd94c29041038c4bb/leaderboard.md)) | Workers AI $0.012/M トークン = 1,075 neuron/M、毎日 10,000 neuron まで無料 ([料金](https://developers.cloudflare.com/workers-ai/platform/pricing/)) | 568M。fp32 で最大 1.81 GB、1 文 77.5 ms (4 スレッド、実測・未再現)。ONNX データ 2.27 GB ([HF](https://huggingface.co/BAAI/bge-m3)) | Workers AI、fastembed、Ollama、OpenRouter | **採用 (既定)** |
| Qwen3-Embedding-0.6B (1024 次元、Apache-2.0) | MIRACL-HN ja 63.1。コード検索の RTEB JapaneseCode1 は 70.6 で bge-m3 の 60.4 より上 ([results](https://github.com/embeddings-benchmark/results/tree/main/results/Qwen__Qwen3-Embedding-0.6B)) | bge-m3 と同額 | 手元は fastembed の candle 側 (2 つ目の ML 基盤) か Ollama 2.4 GB (実測・未再現) | Workers AI、手元 | 評価の対抗馬。Workers AI が付ける既定の指示文を手元でも一字一句同じにしないと一致しない ([Workers AI](https://developers.cloudflare.com/workers-ai/models/qwen3-embedding-0.6b/)) |
| Google EmbeddingGemma-300m (768 次元) | MIRACL-HN ja 68.8 ([results](https://github.com/embeddings-benchmark/results/tree/main/results/google__embeddinggemma-300m))。JMTEB v1 では 65.91 と食い違う (指示文なしで測った疑い、未確認) | Workers AI ではベータで価格未掲載 ([モデル頁](https://developers.cloudflare.com/workers-ai/models/embeddinggemma-300m/)) | fp32 で 855 MB、46 ms。int8/q4 版は fp32 と混ぜられない (cos 最小 0.987 / 0.944、実測・未再現) | Workers AI (β)、手元 | 評価の対抗馬 (RAM が半分) |
| cl-nagoya Ruri v3 30m / 130m (256 / 512 次元、Apache-2.0) | JMTEB v1 検索平均 72.84 / **76.45 (表で最高)** ([JMTEB](https://github.com/sbintuitions/JMTEB/blob/9b1e683bc6a2cd2b6b3e170bd94c29041038c4bb/leaderboard.md))。英語や日英をまたぐ検索の質は未公開 | 手元のみ無料 | 30m int8 は 37 MB、1 文 6.6 ms、最大 315 MB (実測・未再現) | **手元のみ** (クラウドの提供元なし: [HF](https://huggingface.co/cl-nagoya/ruri-v3-310m)) | 評価の対抗馬。勝った場合はクラウド側の問い合わせ用モデルを oboete が自前で動かす必要がある (§2.3) |
| PLaMo-Embedding-1B | JMTEB v1 検索 73.25 | Workers AI $0.019/M | 1B、手元で動かす形式なし | Workers AI のみ | 不採用 (2,048 次元、手元なし) |
| multilingual-e5-small (plan.md §3 の案) | JMTEB v1 検索 63.91 | 手元のみ | 118M | 手元、HF | 不採用 (Workers AI に無く、Ruri 30m より弱い) |
| Gemini / Qwen3-8B / NIM / Cohere / Jina / Voyage / OpenAI | §2.1 の表 | §2.1 | — | クラウド専用 | 不採用 (§2.1) |

**仕様の整理:** plan.md §2b は「手元 = EmbeddingGemma、クラウド = bge-m3」、§3 は「multilingual-e5-small」と書いていて、互いに矛盾しています (3 つとも別のベクトル空間です)。この案で両方を「bge-m3 1 本」に置き換えます。§3 の「trigram + CJK bigram」も実装は trigram だけなので、m1.md 決定 9 に合わせて削ります。m1.md の「fastembed は 5.17 (7.x は無い)」は古い情報で、7.1.0 が 2026-09-22 に出ています ([crates.io](https://crates.io/crates/fastembed/7.1.0))。

### 2.3 ベクトル空間は 1 つだけ: 決まりごと

1. **アカウントごとに正規の埋め込み器を 1 つ決めます。** hub がそれを記録します。`embedder_id` はベクトル空間の名前で、モデル名・精度・pooling・前置き文・次元をつないだ文字列です (例 `bge-m3/dense/fp32/cls/noprefix/1024`)。作った実行環境 (Workers AI か fastembed か) は含めず、ベクトルごとの `producer` 列に診断用として記録します。3 の一致検査に通った実行環境は同じ `embedder_id` を名乗れ、通らなかった実行環境は文書のベクトルも問い合わせのベクトルも作りません。bge-m3 は前置き文が要りません ([HF](https://huggingface.co/BAAI/bge-m3))。
2. **クラウドを使う形では、文書のベクトルは Workers AI で作ります。** 要約と同じ detached の observe で作ります。オフライン中にできた文書はベクトル無しで保存し (全文検索にはすぐ出る)、次にオンラインで observe が走ったときに作ります。手元の fastembed が 3 の一致検査に通った端末だけは、オフライン中の文書も手元で作ってよく、同じ `embedder_id` で送ります (§0 の 5)。ローカルだけの形は全部を手元で作ります。
3. **手元モデルの役目は、オフライン時に問い合わせ文をベクトルにすることだけです。** 同じ重みでも、精度・pooling・切り詰めが違えば数字は変わります。EmbeddingGemma では int8 版で cos 0.987 まで下がりました (実測・未再現)。そこで、Workers AI と手元 ONNX で同じ文 100 件を変換し、**cos の最小が 0.99 以上、評価クエリの上位 10 件が 9 件以上一致**したときだけ、手元の問い合わせを有効にします (一致の度合いはまだ未計測、§7 の PR-A)。
4. **モデルを変えるときは全件作り直しです** (`oboete reindex`)。hub は正規の `embedder_id` のほかに「準備中」の `embedder_id` を 1 つだけ登録でき、準備中のベクトルも受け取って別に保持します (検索には使わない)。準備中のベクトルが全件そろったら hub の正規を切り替え、古い世代を捨てます。それまでの検索は古い世代のままなので、切り替えで検索が止まる時間はありません。各端末は切り替えのあとにベクトルを pull し直します。
5. **Ruri が評価で勝った場合**、文書ベクトルは手元か VPS で作れます。ただしクラウド側の問い合わせ (リモート MCP、スマホ) のために、Cloudflare Containers か VPS で Ruri を常に動かす必要が出ます。Containers は Workers Paid の含み枠が月 25 GiB 時間なので、常時起動なら 1 GiB の instance で約 25 時間分しかありません ([Containers 料金](https://developers.cloudflare.com/containers/pricing/))。この費用と手間が、日本語の上積みに見合うかで判断します。

### 2.4 検索の手順 (期待できる効果と費用)

| 段 | 中身 | 期待できる効果 (出典) | 費用・遅さ | 使う経路 |
|---|---|---|---|---|
| 1 全文 | 既存の FTS5 trigram + bm25。3 文字未満の語は LIKE (m1.md 決定 9)。候補 100 件。LIKE だけで当たった文書には順位の根拠が無いので、bm25 で当たった文書の後ろに新しい順で並べて RRF に渡す (3 文字未満の語だけの問いでは、全文側の順位はほぼ日付順になる。ここは意味検索側が補う部分で、区画「識別子・パス」と短い日本語の問いで測る) | 基準。日本語 QA の JQaRA では BM25 が nDCG@10 0.458 ([JQaRA](https://huggingface.co/datasets/hotchpotch/JQaRA)) | 既存 | 全部 |
| 2 意味 | 問い合わせ文を bge-m3 でベクトルにし (オンラインは Workers AI、オフラインは手元、どちらも無ければ省略)、sqlite-vec で全件の厳密 kNN。候補 100 件 | 英語の長期記憶評価では、密ベクトル (Stella) が BM25 を recall@5 で 0.660 対 0.472 と上回る ([LongMemEval Table 9](https://arxiv.org/html/2410.10813v2))。この PC のデータでは未計測 | 15 万件 × 1024 次元の全件走査は fp32 で 234〜261 ms・約 620 MB、repo で分けると 9〜26 ms、int8 で 134〜140 ms、bit + 再採点で 20〜80 ms (合成ベクトルで 2 回測った値、数字は一致せず傾向だけ一致: [sqlite-vec vec0](https://alexgarcia.xyz/sqlite-vec/features/vec0.html))。Workers AI 1 回の往復時間は未計測 | MCP / CLI / viewer / 注入 |
| 3 融合 | RRF (k=60)。重み付き版は評価の dev 分でだけ調整 | TREC で Condorcet や学習型の融合に勝った ([Cormack 2009](https://plg.uwaterloo.ca/~gvcormac/cormacksigir09-rrf.pdf))。**ただし basic-memory の多言語評価では hybrid が MRR を下げた** (0.7895 対 ベクトルのみ 0.8684: [basic-memory](https://github.com/basicmachines-co/basic-memory/blob/main/docs/multilingual-embedding-benchmark.md))。だから hybrid 自体も評価で合否を決める | ほぼ 0 | 全部 |
| 4 日付 | MCP `search` に `since` / `until` を足し、両方の候補に先にかける。黙って期限を切ることはしない | 時期を意識した問い合わせ拡張で recall が平均 +11.3% (rounds) / +6.8% (sessions) ([LongMemEval §5.4](https://arxiv.org/html/2410.10813v2)) | 0。日付は呼び出し側の agent が埋める | MCP |
| 5 索引時のキーワード | 要約の JSON schema に `keys` (日英の主要語、識別子、ファイル名、エラー文字列を 5〜10 個) を足し、全文と埋め込みの両方に入れる | 事実でキーを拡張して recall@k +9.4%、QA 正答 +5.4%。複数の経路を順位で合わせるより索引時に合わせるほうが良かった ([LongMemEval §5.3, E.3](https://arxiv.org/html/2410.10813v2))。英語データでの結果 | 既存の要約呼び出しの出力が数十トークン増えるだけ。古い行は reindex で埋める | 全部 |
| 6 重複の間引き | 選んだものと cos が近いもの (しきい値は評価で決める) を飛ばす。MMR なら λ=0.5 (Graphiti の既定: [search_utils](https://github.com/getzep/graphiti/blob/main/graphiti_core/search/search_utils.py)) | 公開された効果の数字は無い | 候補 50 件の中だけなので 0 に近い | 注入 |
| 7 注入のしきい値 | 問い合わせに関係する記憶が無いときは何も注入しない。しきい値は「答えの無い問い合わせ」で較正する | mem0 / basic-memory (0.55) / supermemory (0.5〜0.6) が使う。basic-memory はモデルごとに較正し直す必要があった ([basic-memory](https://github.com/basicmachines-co/basic-memory/blob/main/docs/semantic-search.md)) | 0 | 注入 |
| 8 reranker | **MCP の `search` だけ**。候補上位 50 件を並べ直す。Workers AI の bge-reranker-base は日本語で BM25 以下 (0.2445 対 0.458) なので**使わない**。候補は japanese-reranker-xsmall-v2 (MIT、37M、arm64 用 int8 ONNX あり) と bge-reranker-v2-m3 (多言語、fastembed 内蔵) | 4 つの日本語データの平均で xsmall-v2 0.8699、v2-m3 0.8512 ([hotchpotch](https://hotchpotch.dev/articles/japanese-reranker-v2/))。JQaRA の点は、評価と学習データの分布が似ているため日本語モデルが高めに出る可能性がある (学習データは japanese-reranker-v2-hard-negatives: [モデルカード](https://huggingface.co/hotchpotch/japanese-reranker-xsmall-v2))。reranking を足すと検索失敗の減り方が 49% から 67% になった (英語: [Anthropic](https://www.anthropic.com/engineering/contextual-retrieval)) | xsmall-v2 は M4 Max の CPU で 1 組約 15 ms、50 件で約 0.77 s。M1 と A1 は未計測。v2-m3 は fp32 で約 2.2 GB | MCP のみ |
| 採らない | 問い合わせの書き換え・HyDE (LLM の無料枠を要約と奪い合う。呼び出し側の agent が言い換えて何度でも検索できる)、新しさの減衰 (古くても正しい決定を沈める。claude-mem の 90 日問題と同じ)、事実ごとの多ベクトル (claude-mem は 1 件あたり 6.7 本) | — | — | 評価で必要と出たら再検討 |

bge-m3 は手元の fastembed なら密ベクトルと疎ベクトルを 1 回で出せます。JQaRA では疎が 0.5088、全部合わせると 0.576 です ([JQaRA](https://huggingface.co/datasets/hotchpotch/JQaRA))。ただ Workers AI が疎ベクトルを返すかは未確認で、使うとクラウドと手元の結果がずれるので、今回は入れません。

### 2.5 何を埋め込むか

| 文書 | 埋め込む文 | 備考 |
|---|---|---|
| 観測 | `{kind}: {title}\n{body}\n{keys}` | repo・日付・agent は文に入れず、絞り込み条件にする |
| 要約 | 本文 | |
| prompt | 先頭約 1,000 字 | 問いは多くの場合先頭にある。Workers AI の bge-m3 が 1 文に受け付ける長さは資料で食い違う (AI Search の表では 512、モデル本来は 8,192、モデル JSON の context_window は 60,000: [bge-m3.json](https://raw.githubusercontent.com/cloudflare/cloudflare-docs/production/src/content/workers-ai-models/bge-m3.json)) ので PR-A で 8,000 字の prompt を実際に投げて確かめる。分割して一番近い塊で採点する方式は評価で比べる実験にする (claude-mem の prompt は平均 1,849 字、8,000 字超が 662 件: 調査担当がこの PC の claude-mem DB を読み取り専用で集計、未再現) |
| 埋め込まない | 生イベント、注入記録、harness の通知 prompt (m1.md 決定 15 でカードにしないもの) | |
| claude-mem から取り込んだ文書 | 同じ規則。`source = claude-mem` を付ける | §6 の決定 1 |

費用は、1 日 300 件なら約 65 neuron で無料枠の 0.65% です。16.5 万件を全部作り直す場合は 3,000 万〜5,250 万トークン (観測 150〜250、prompt 500〜1,000 トークンと仮定)、32,000〜56,000 neuron になります。無料枠を毎日全部使えば 3.2〜5.6 日、全部有料で数えても $0.35〜0.62 です ([料金](https://developers.cloudflare.com/workers-ai/platform/pricing/))。日本語はモデルによってトークン数が大きく変わるので、仮定は PR-A で実測値に置き換えます。作り直しは provider の日次予算と同じ仕組みで 1 日 9,000 neuron までに抑えるので、約 4〜6 日かかります。text embedding の上限は毎分 3,000 回、bge-m3 は 1 回に 100 件まで送れます ([limits](https://developers.cloudflare.com/workers-ai/platform/limits/)、[bge-m3.json](https://raw.githubusercontent.com/cloudflare/cloudflare-docs/production/src/content/workers-ai-models/bge-m3.json))。Cloudflare は入力を学習にも改善にも使わず、埋め込みも顧客データとして扱います ([data usage](https://developers.cloudflare.com/workers-ai/platform/data-usage/))。

### 2.6 オフラインのとき

- 全文検索はいつでも動きます。
- 意味検索の問い合わせは、手元モデルがあれば手元で変換し、無ければ全文検索だけで返します。結果の 1 行目に「意味検索: クラウド / 手元 / なし」を出して、劣化が見えるようにします。
- オフライン中の新しい文書は、ベクトル無しのまま文書の op として送れます。ベクトルは文書とは別の `vec` op (§4.3) なので、オンラインに戻った後の observe が埋め込み、あとから送ります。文書だけ先に届いた端末でも、その文書は全文検索には出ます。
- 手元モデルはオフラインになってからは取得できません。そこで `setup` のときに 1 回だけ取得します (約 2.3 GB)。WSL、Windows、M1 では既定でオン、VPS では取得しません。読み込むのはオフライン時の検索だけで、そのプロセスが終われば RAM は戻ります。fastembed を入れるとバイナリは大きくなります (fastembed 入りの最小バイナリで 29.65 MB、今の oboete は 10.7 MB。実測・未再現)。

### 2.7 hook の速さは変えない

- **記録用の hook (≤ 20 ms、現在 p50 / p95 = 10 / 17 ms: m1.md PR9 の計測) には、通信もモデルもベクトル計算も入れません。** 埋め込みは detached の observe、問い合わせの変換は MCP / CLI / viewer と、記録とは別プロセスの注入 hook (下記) の中で行います。PR ごとに replay で hook 時間が変わっていないことを確かめます。
- **prompt ごとの自動注入** (plan.md §5。まだ作っていません。claude-mem も既定ではオフで `CLAUDE_MEM_SEMANTIC_INJECT=false`) は、記録 hook とは**別の hook として登録**します。
  - 記録は今までどおり `oboete hook <agent> UserPromptSubmit` (≤ 20 ms) で行い、注入は同じイベントに登録した 2 つ目のコマンド `oboete inject <agent>` が行います。同じイベントに登録した hook は並行に動き、複数の `additionalContext` はすべてモデルに届きます (Claude Code: [hooks](https://code.claude.com/docs/en/hooks)、Codex: [hooks](https://developers.openai.com/codex/hooks))。別プロセスなので、記録 hook の時間は変わりません。
  - UserPromptSubmit の `additionalContext` は、送った prompt と一緒に、モデルが最初に答える前に届きます。
  - 注入 hook には厳しい時間予算を付けます。全体で 300 ms、Workers AI での問い合わせ変換は 150 ms まで、超えたら全文検索だけにします。hook の timeout は 1 秒にするので、最悪でも「今回は注入なし」で済み、prompt が止まることはありません。timeout になった hook の出力は捨てられ、prompt はそのまま進みます ([hooks](https://code.claude.com/docs/en/hooks))。
  - この hook の中では手元モデルを読み込みません (読み込みだけで予算を超えるため)。オフライン時は全文検索だけで注入します。
  - 開発者から見える変化は、prompt を送ってから答えが始まるまでが最大で約 0.3 秒 (timeout 時は 1 秒) 延びることです。PR-F で実測します。
  - 長さの上限: Claude Code は 10,000 字です。Codex は約 2,500 トークンを超えるとファイルに退避され、先頭と末尾だけがモデルに見えます (`additionalContextLimit`: [Codex hooks](https://developers.openai.com/codex/hooks)) ので、Codex への注入文はそれより短くします。
  - Grok は UserPromptSubmit の出力を捨てるので、M1 と同じく最初の PreToolUse の経路で渡します (m1.md 決定 3)。
  - M1 (VPS を使うならそれも) で予算に収まらない場合の代案として、detached の `recall` が結果を DB に置き、次の PreToolUse で渡す方式を残します。このとき結果は (session id、prompt id) で引き、同じ prompt のときだけ注入します。遅れた前の prompt の結果が次の prompt に混ざらないようにするためです。ただしこの代案は、tool 実行の後に届き、tool を呼ばない turn では届きません。
  - `async: true` の hook は出力が次の turn に回るので、この用途には使えません ([hooks](https://code.claude.com/docs/en/hooks))。

### 2.8 端末ごとの手元索引

- 手元索引はまず fp32 で、repo ごとに分けて (sqlite-vec の partition key) 全件厳密に検索します。M1 と VPS の計測で遅ければ int8 か bit + 再採点に切り替えます。そのときの recall の落ち方は評価で測ります。sqlite-vec の資料では bit 化で 5〜10% 落ちるとされています ([binary quant](https://alexgarcia.xyz/sqlite-vec/guides/binary-quant.html))。
- 15 万件 × 1024 次元のベクトルは fp32 で約 614 MB、int8 で約 154 MB です (計算値)。

### 2.9 比較の出典 (claude-mem / cmem Pro)

- claude-mem 13.25.3 の検索は Chroma だけで行います。Chroma が落ちたか 0 件のときだけ FTS5 に切り替え、RRF の要望は採用されずに閉じられました ([#2000](https://github.com/thedotmack/claude-mem/issues/2000)、[#1789](https://github.com/thedotmack/claude-mem/issues/1789))。90 日の窓は `RECENCY_WINDOW_MS:7776e6` です (`~/.claude/plugins/cache/thedotmack/claude-mem/13.25.3/scripts/worker-service.cjs`)。モデルは Chroma 既定の all-MiniLM-L6-v2 で ([HF](https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2))、変更できません ([#3029](https://github.com/thedotmack/claude-mem/issues/3029))。この PC ではベクトル 1,010,427 本に対し観測 151,543 件 (1 件あたり約 6.7 本)、chroma.sqlite3 が 3.3 GB、ディレクトリ全体が 4.8 GB でした (読み取り専用で集計)。メモリ肥大の報告も続いています ([#3684](https://github.com/thedotmack/claude-mem/issues/3684)、[#3905](https://github.com/thedotmack/claude-mem/issues/3905))。日本語の問い合わせは順位付けの無い LIKE になります。
- cmem Pro はユーザーごとに 1 つの SQLite Durable Object を持ち、上限は 64 台 ([SyncHub.ts](https://github.com/thedotmack/claude-mem/blob/main/workers/sync-hub/src/do/SyncHub.ts))。中身は Turbopuffer に写しますが、埋め込みモデルは公開されていません ([launch plan](https://github.com/thedotmack/claude-mem/blob/main/plans/2026-07-22-cmem-launch.md))。料金はトップが月 $20、/pro が 30 日無料の後に月 $30 と表記が食い違っています ([cmem.ai](https://cmem.ai)、[cmem.ai/pro](https://cmem.ai/pro))。hosted-server 版の検索は英語設定の Postgres 全文検索だけです ([hosted-server](https://docs.claude-mem.ai/hosted-server))。

---

## 3. 精度の測り方

### 3.1 評価セット (この PC の実データから作る)

| 材料 | 量 | 作り方 | 測れること |
|---|---|---|---|
| claude-mem の prompt → そのセッションの観測 | 129,924 組 / 6,391 prompt (調査担当の集計、未再現) | `user_prompts` → `sdk_sessions` → `observations` をセッションと prompt 番号で結ぶ。**同じセッションの文書は正解候補から外し**、他のセッションから見つかった文書だけを判定にかける | 日本語の問い → 英語の記憶 (日英をまたぐ検索) |
| agent が実際に打った記憶検索 | 38 件 (調査担当の集計、未再現) | `~/.claude/projects` の transcript から抜き出す。日英の混ざったキーワード列 | 本番に近い問い |
| 合成の既知項目 | 約 100 件 | LLM に、英語の記憶には日本語の問いを、日本語の記憶には英語の問いを書かせる。言い換えを強制して、字面が重なるだけの有利を消す | 日英双方向 |
| oboete 自身の日本語の記憶 | 今は 53 件 (全件日本語)。1 週間の実使用で増える | 同じ手順で問いを作る。数が溜まるまでは、claude-mem の観測 2,000 件を oboete の要約器で日本語にした写しで代用する | **日本語 → 日本語 (oboete の将来の主な形)** |
| 答えの無い問い | 50 件 | どの記憶にも関係しない作業の依頼文 | 注入のしきい値 (誤注入率) |
| 公開ベンチ (参考) | JQaRA / JaCWIR | そのまま | モデル単体の日本語の力の確認 |

データの言語の偏り (全件集計): claude-mem の観測タイトルで日本語を含むのは 2.94%、本文 (narrative) は 5.24%、prompt は 63.76% (15,220 件中)、oboete の観測は 53 件すべてです (読み取り専用で集計)。つまり **claude-mem だけで作った評価は日英をまたぐ検索しか測れません**。日本語 → 日本語の区画を別に持つのはこのためです。

**正解の付け方:** 各方式 (全文のみ、bge-m3、対抗馬、hybrid) の上位 50 件を合わせて候補の束にします (JQaRA と同じ作り方。§3.2 で recall@50 を測るので、判定の深さも 50 にそろえる。判定しなかった順位を「関係なし」と数える偏りを作らない)。これを UMBRELA の 0〜3 段階の LLM 判定で採点します ([UMBRELA](https://arxiv.org/abs/2406.06519))。判定には要約器と別系統のモデルを使い (サブスクの claude か codex。無料の Gemini は入力を学習に使うので使いません)、約 50 組を人が付けた正解と照らします。一致が低ければ (Cohen の κ が 0.6 未満を目安) LLM 判定は信用しません。

### 3.2 指標

- 候補を集める段: recall@10 / @50
- 最終の並び: nDCG@10、MRR@10
- 誤りの段: 1 位が外れた割合、答えの無い問いに注入してしまう率
- 重さ: 端末ごとの p50 / p95 の遅さ、最大 RSS。WSL、Windows、M1 で測ります (VPS は使うと決まったら足す)
- 区画: 日→日、日→英、英→日、識別子・パス
- 道具: oboete が TREC 形式の結果ファイルを出し、ranx で指標と有意差検定を計算します ([ranx](https://github.com/AmenRa/ranx))。測るのは実際に出荷するコードです

### 3.3 合格線 (先に決めて動かさない)

| 対象 | 合格の条件 (提案値) |
|---|---|
| 意味検索そのもの | hybrid が全文検索だけに比べ、test 分の nDCG@10 で +0.03 以上、有意 (p < 0.05)。どの区画でも recall@10 が 0.02 より大きく下がらない。**満たさなければ既定は `none` (全文検索のみ) のまま** |
| 個々の工夫 (§2.4 の 4〜8) | nDCG@10 +0.02 以上 (候補集めの工夫は recall@50 +0.03 以上)、p < 0.05。区画ごとに 0.02 を超える低下がない。その経路の時間予算に収まる: 記録 hook は p95 が変わらない、MCP 検索は最も遅い端末で p95 1.5 秒以内、注入 hook は最も遅い端末で p95 300 ms 以内 |
| モデルの入れ替え (Ruri など) | 日→日で nDCG@10 +0.03 以上、他の区画で 0.02 を超える低下がない。そのうえで §2.3 の 5 の費用を owner が受け入れた場合だけ |
| 自動注入 | 答えの無い問いへの誤注入が 10% 以下になるしきい値で、正解のある問いの 50% 以上に注入できること。注入 hook の p95 が 300 ms 以内で、timeout で捨てられる割合が 2% 以下 (提案値)。できなければ prompt ごとの注入は出さない |
| クラウドと手元の一致 | Workers AI と手元 ONNX で cos の最小が 0.99 以上、上位 10 件が 9 件以上一致 |

- 調整に使うのは dev 分 (70%) だけで、合否は手を付けていない test 分 (30%) で決めます。小さな評価セットで重みを合わせ込みすぎるのを防ぐためです。
- 1 回の比較で決めます。「直して測り直す」を合格まで繰り返すことはしません。

---

## 4. 同期

### 4.1 方式の比較

| 方式 | オフラインで書ける | クラウドで検索できる | 自分の CF アカウント内 | 成熟度 | 15 万件の費用 | 判定 |
|---|---|---|---|---|---|---|
| **Worker + Durable Object (SQLite) の op log** | 可 | 可 (FTS5 あり) | 可 | cmem Pro が本番で使う形 ([SyncHub](https://github.com/thedotmack/claude-mem/tree/main/workers/sync-hub)) | 含み枠内で $0。行の料金は D1 と同じ、保存は 5 GB を超えた分が $0.20/GB-月 ([DO 料金](https://developers.cloudflare.com/durable-objects/platform/pricing/)) | **採用** |
| Worker + D1 の op log (plan.md §8) | 可 | 可 | 可 | GA | 保存は超過分 $0.75/GB-月 ([D1 料金](https://developers.cloudflare.com/d1/platform/pricing/))。管理画面と 30 日の Time Travel が付く | 次点 |
| R2 にログを置くだけ | 可 | 不可 | 可 | — | ほぼ $0 ([R2](https://developers.cloudflare.com/r2/pricing/)) | 不採用 (クラウド検索が無い) |
| Turso Sync | 可 (最後に push したほうが勝つ) | 別の FTS (Tantivy、実験段階) | 不可 | — | 無料枠内 ([Turso](https://turso.tech/pricing)) | 不採用 (SQLite FTS5 の trigram と sqlite-vec を捨てることになり、置き場も外部) |
| CRDT 拡張 (cr-sqlite / sqlite-sync) | 可 | 不可 (Workers で拡張を読めない) | 不可 | cr-sqlite の最後の release は 2024-01-17。sqlite-sync は Elastic License 2.0 | — | 不採用 |
| Litestream / LiteFS | 不可 (書き手は 1 つだけ) | 不可 | — | — | — | 不採用 ([Litestream](https://litestream.io/tips/)) |
| PowerSync / Electric | 可 | Postgres 側で | 不可 | Rust SDK は alpha ([PowerSync](https://docs.powersync.com/resources/feature-status)) | Postgres が要る | 不採用 |
| VPS に自前の hub | 可 | 可 (同じコード) | 不可 (VPS) | — | $0 だが本番の箱と取り合う | 不採用 |

- DO を選ぶ理由: 1 ユーザー 1 つの DO が自然に 1 本の順序を作るので、hub が単調に増える `seq` を配れます。端末の時計を信じる必要がありません。SQL は同じスレッドで走り、30 日の時点復元もあり、保存は D1 より安く、比較基準の cmem Pro と同じ形です。10 GB まで入ります ([DO 制限](https://developers.cloudflare.com/durable-objects/platform/limits/))。
- 失うのは D1 の管理画面だけなので、書き出し用の口を自前で 1 つ作ります。
- Workers の中では fts5 / fts5vocab / rtree 以外の仮想表を作れないので、sqlite-vec も CRDT 拡張も hub では動きません ([workerd sqlite.c++](https://github.com/cloudflare/workerd/blob/main/src/workerd/util/sqlite.c++))。
- **CRDT は要りません。** 観測・要約・prompt は追加と削除しかされません。session も値が一方向に進むだけです。

### 4.2 先に直すもの (今のコードの障害)

1. **repo のキーが絶対パスです** (`src/repo.rs` の `key()`)。WSL の `/home/jura/...`、macOS の `/Users/...`、Windows の `C:\...` で同じ repo が別物になります。これを、`.git/config` の origin URL を正規化したもの (例 `github.com/ojungo69/oboete`、git は起動しない) に変えます。remote が無い repo は従来どおりパスにし、`.oboete.toml` の `repo = "..."` で名前を付けられるようにします。既存の行も移し替えます。
2. **session id が取れないときに `"unknown"` になります** (`src/hook.rs` 129 行)。これは端末をまたいで衝突します。取れないときは端末 id 付きの一意な値を作ります。
3. **文書 id が端末ローカルの連番 (`o12`) です。** 同期用に `uid` 列を足します。形は `{端末 id 8 桁}:{種類}{連番}` (例 `7f3a9c21:o123`) で、cmem の `sha256(device, kind, local_id)` と同じ考え方です ([METADATA-CONTRACT](https://github.com/thedotmack/claude-mem/blob/main/workers/sync-hub/METADATA-CONTRACT.md))。
   - 連番は AUTOINCREMENT なので使い回されません (m1.md 決定 14)。
   - 端末 id は DB を作ったときに乱数で決めるので、入れ直しても衝突しません。新しい crate も要りません (`getrandom` は導入済み)。
   - DB ファイルを別の機械へコピーすると、2 台が同じ端末 id を持ってしまいます。そこで、端末 id と一緒にその機械の識別 (ホスト名と OS のマシン ID) を保存しておき、起動時に食い違ったら新しい端末 id を作り直します。
   - plan.md の「内容ハッシュ id」はやめます。「進めて」のような同じ文の prompt が 1 件にまとまってしまうからです。
   - 手元の表示は `o123` のまま、`get` は uid も受け付け、リモート MCP は uid を返します。

### 4.3 何を送るか・消し方・ベクトル

| 表 | 同期 | 合わせ方 |
|---|---|---|
| sessions (id、agent、repo、端末、開始、最終イベント) | する | 開始は小さいほう、最終イベントは大きいほう。注入済みの印は端末ローカル |
| observations / summaries / prompts | する (prompt は §6 決定 3) | 追加のみ。同じ uid の op が 2 回来ても 1 回分 (op の uid で冪等) |
| 文書のベクトル | する (文書とは別の `vec` op。`embedder_id` 付きの fp32、1 件 4 KB) | 文書の uid と `embedder_id` の組で冪等 (同じ組が 2 回来ても 1 回分)。正規か準備中 (§2.3 の 4) の `embedder_id` 以外は hub が受け取らない。tombstone 済みの文書の `vec` は捨てる (削除が勝つ)。送信は文書を先、`vec` を後に並べるので、同じ端末の `vec` が文書より先に届くことはない。オフライン後の埋め込み (§2.6) も reindex の新しい世代もこの op で送る |
| 削除 (tombstone = 消した印) | する | **削除が常に勝つ** (届く順番に関係なく)。文書の削除は文書の tombstone、session の削除は **session 自体の tombstone** を作る。hub はその session を指す op を、あとから届いたもの (別の端末でまだ送っていなかった文書) も含めて全部捨てる。印は小さいので永久に保持する |
| events / injections / provider_calls / fts / vec 索引 | しない | 端末ローカル。fts と vec は受け取った文書から各端末が作り直す |

**通信の流れ (常駐プロセスなし):**

- **送信待ちは、各行の `synced_at` 列 (NULL = まだ hub に無い) と、削除などの変更だけを入れる小さな outbox 表の 2 つです。** claude-mem の同期クライアントと同じ形です ([CloudSync.ts](https://github.com/thedotmack/claude-mem/blob/main/src/services/sync/CloudSync.ts))。送るのは `synced_at IS NULL` で、自分の端末で作った行だけです (hub から受け取った行は送り返さない)。同期を初めて有効にしたときや、ローカルだけの形からクラウドを使う形に移ったときは、それまでの行もすべて NULL なので、別の移行手順なしに全部送られます。uid で冪等なので、途中で止まっても再実行で続きから送れます。
- **送信**: detached の observe の最後と `oboete sync` で送ります。
- **受信**: SessionStart hook が detached の `oboete sync` を起動します (hook 自体は通信しません)。受け取った内容は次の検索から効きます。望めば cron / launchd / タスク スケジューラで定期実行もできます。
- 受信は `GET /ops?after=<seq>&limit=500` のページ送りです。新しい端末も同じ口で最初から取ります。16.5 万件でベクトル込み約 0.8 GB (推計) で、遅ければ R2 のスナップショットを足します。
- **削除した中身は log からも消します。** hub は tombstone を受け取った時点で、対象の文書の op から本文を、その `vec` op からベクトルを消し、`seq` と uid と「削除済み」の印だけを残します (session の tombstone ならその中の全 op)。新しい端末が最初から取っても、消した prompt の本文やベクトルは届きません。端末側も、受け取った tombstone に合わせて手元の本文・fts・ベクトルを消します (既存の `delete_doc` / `delete_session` と同じ)。
- hub は op を受け取ったら順序付きで保存し、表に反映して、FTS5 も更新します。

**Vectorize** (§6 決定 4 で承認された場合) は DO が表に反映するときに一緒に upsert / delete します。

- 絞り込みに使う metadata の索引 (repo、kind、agent、source) は、**最初の挿入より前に**作っておく必要があります。索引は最大 10 個、文字列は先頭 64 byte だけが索引に入ります ([metadata filtering](https://developers.cloudflare.com/vectorize/reference/metadata-filtering/))。そこで索引に入れる repo は、repo キー (origin URL) の SHA-256 の先頭 16 桁 (16 進) にします。長い URL の先頭が同じ 2 つの repo も区別でき、表示用の URL は別の metadata に持ちます。
- 書き込みが検索に出るまでの時間は、中央値 30 秒未満、p99 は 2 分未満です ([changelog](https://developers.cloudflare.com/changelog/post/2026-06-30-improved-wal-throughput/))。
- 検索は近似で、IVF と直積量子化の後に精密化して 95% 超です ([Cloudflare blog](https://blog.cloudflare.com/building-vectorize-a-distributed-vector-database-on-cloudflare-developer-platform/))。

**未確認:** DO / D1 の SQLite で FTS5 の trigram tokenizer が使えるかは文書に書かれていません。FTS5 自体は使えます ([D1 SQL](https://developers.cloudflare.com/d1/sql-api/sql-statements/))。PR-A で `CREATE VIRTUAL TABLE ... tokenize='trigram'` を 1 文だけ実行して確かめます。使えなければ、端末側で bigram に分けた列を送る方式にします (basic-memory の [script_ngrams](https://github.com/basicmachines-co/basic-memory/blob/main/src/basic_memory/repository/script_ngrams.py) と同じ考え方)。

### 4.4 認証

- Worker の前に Cloudflare Access を置きます。workers.dev ならワンクリックで有効にでき、Worker 側で `Cf-Access-Jwt-Assertion` を検証します ([changelog](https://developers.cloudflare.com/changelog/post/2025-10-03-one-click-access-for-workers/)、[JWT 検証](https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/validating-json/))。
- 端末ごとに Access の service token を発行するので、1 台ずつ取り消せます。ヘッダは `CF-Access-Client-Id` / `CF-Access-Client-Secret` か、JSON 形式の `Authorization` 1 本です ([service tokens](https://developers.cloudflare.com/cloudflare-one/access-controls/service-credentials/service-tokens/))。
- 鍵は既存の決まりどおり owner の鍵ファイルから読み、子プロセスの環境変数には渡しません (`provider.rs`)。
- Workers AI への埋め込みは、端末から REST で呼びます。Workers AI の権限だけに絞った token を使い、既存の provider と同じ形にします。Cloudflare API 全体の上限は 5 分に 1,200 回ですが ([API limits](https://developers.cloudflare.com/fundamentals/api/reference/limits/))、1 回に 100 件送れるので足ります。
- Access 自体の料金は未確認です。

### 4.5 15 万件での費用 (Workers Paid の含み枠と比べて)

| 項目 | 見込み | 含み枠・単価 | 追加費用 |
|---|---|---|---|
| DO への要求 | 月に数千回 (推計) | 月 100 万回 ([DO 料金](https://developers.cloudflare.com/durable-objects/platform/pricing/)) | $0 |
| 書き込み行 | 初回取り込みで約 100 万行 (FTS の書き込み込みの推計、未計測) | 月 5,000 万行 | $0 |
| 保存 | 本文・FTS・fp32 ベクトル (16.5 万 × 4 KB ≈ 676 MB) で 1〜2 GB (推計) | 5 GB | $0 |
| Workers AI 埋め込み | 毎日約 65 neuron、全件作り直しは 32,000〜56,000 neuron | 毎日 10,000 neuron 無料、超過は 1,000 neuron あたり $0.011 ([料金](https://developers.cloudflare.com/workers-ai/platform/pricing/)) | 日常は $0。作り直しは 1 日 9,000 neuron に抑えて約 4〜6 日か、急げば $0.35〜0.62 |
| Vectorize (決定 4) | 保存 1 億 6,900 万次元、月 3,000 回の検索 | 検索 5,000 万次元・保存 1,000 万次元が含み枠 ([料金](https://developers.cloudflare.com/vectorize/platform/pricing/)) | 保存分だけなら**月 $0.08**。公式の式 `(検索したベクトル数 + 保存したベクトル数) × 次元` を文字どおり読むと**月 $1.3**。どちらの読み方になるかは未確認で、初月の請求で確かめる。plan.md の $0.07 は保存分だけの数字 |
| R2 (スナップショットを使う場合) | 1 GB 未満 | 月 10 GB 無料 ([R2](https://developers.cloudflare.com/r2/pricing/)) | $0 |

### 4.6 4 台の端末

| 端末 | ビルド対象 | 手元索引 | 手元モデル | 注意 |
|---|---|---|---|---|
| この PC の WSL2 (x86_64) | x86_64-unknown-linux-gnu | 全件 fp32 | あり (オフライン時だけ読む) | — |
| 同じ PC の Windows (WSL の外) | x86_64-pc-windows-msvc | 全件 | あり | WSL とは**別の端末**として同期する。DB ファイルを `/mnt/c` 越しに共有しない (ネットワーク FS 越しのロックは SQLite が保証しない: [How to corrupt](https://www.sqlite.org/howtocorrupt.html))。origin URL の repo キーで同じ repo として扱う |
| M1 iMac 8 GB | aarch64-apple-darwin | 全件。遅ければ int8 | あり。読み込み時に最大 1.8 GB (x86 での実測・未再現、M1 は未計測) | 走査の速さと RAM を PR-A で測る |
| VPS (OCI A1 arm64、本番と同居。**使うか未定**、決定 10) | aarch64-unknown-linux-gnu | 全件 (ディスク上、走査時だけ page cache) | **なし** (常に Workers AI) | 箱の stop / resize は絶対にしない。RAM の取り合いは PR-I で計測する |

fastembed 7.1.0 が固定する ort 2.0.0-rc.13 には、4 つとも ONNX Runtime 1.28.0 のビルド済み版があります ([ort-sys 2.0.0-rc.13](https://crates.io/crates/ort-sys/2.0.0-rc.13))。ort が release candidate に固定されている点と、ビルド時に pyke の CDN から取ってくる点は、依存のリスクとして記録しておきます。

### 4.7 リモート MCP と viewer

- **リモート MCP**: 同じ Worker に Agents SDK の `createMcpHandler` (状態を持たない Streamable HTTP) を置きます。tool は `search` / `get` / `timeline` で、手元と同じ意味を持たせます ([handler API](https://developers.cloudflare.com/agents/model-context-protocol/apis/handler-api/))。
- 検索は DO の FTS5、Vectorize、同じ RRF の組み合わせです。手元と同じ問い合わせベクトルで上位 10 件が 9 件以上一致することを、評価セットで確かめます。
- 登録方法:
  - Claude Code: `claude mcp add --transport http --scope user ... --header ...`。project の `.mcp.json` に `${VAR}` で秘密を書くと、決まった名前の変数が空として読まれることがあるので、user scope で登録します ([Claude Code MCP](https://code.claude.com/docs/en/mcp))。
  - Codex: `bearer_token_env_var` / `env_http_headers` を使います ([Codex MCP](https://developers.openai.com/codex/mcp))。
  - claude.ai の Web / スマホの custom connector に OAuth が必須かどうかは未確認です。スマホは保留中なので、そのときに workers-oauth-provider で対応します。
- **viewer**: 手元の viewer は同期された全端末分を表示します (追加の作業はありません)。クラウド版の viewer は、スマホを対象にするときに同じ静的ファイルを Worker から配り、Access の後ろに置きます。

---

## 5. 暗号化は必要か

**答え: 今は不要です。** 代わりに入口を固めます。理由は次のとおりです。

1. **中身はすでに保護されています。** 秘密は保存前 (hook) と送信前 (observe) の 2 か所で、gitleaks の 222 規則による伏せ字を通ります (m1.md 決定 5)。置き場は owner 自身の Cloudflare アカウントです。D1 と Durable Objects は AES-256 で保存時暗号化されますが、鍵は Cloudflare が管理します ([D1](https://developers.cloudflare.com/d1/reference/data-security/)、[DO](https://developers.cloudflare.com/durable-objects/reference/data-security/))。Workers AI は入力を学習に使いません ([data usage](https://developers.cloudflare.com/workers-ai/platform/data-usage/))。
2. **本文だけ暗号化しても効果は小さいです。** ベクトルから元の文を戻す研究があり、32 トークンの文の 92% が完全に復元されています ([vec2text](https://arxiv.org/abs/2310.06816))。論文の対象は GTR などで、bge-m3 では未計測です。観測はちょうどその長さなので、本文を暗号化してもベクトルを平文で置けば、守れるものは少なくなります。
3. **全部を端末側で暗号化すると、クラウドでの全文検索・意味検索・リモート MCP・クラウド viewer がすべて動かなくなります。** 検索の索引を作るには、hub が中身を読める必要があるからです。これは owner の希望 (クラウドでの意味検索) と両立しません。
4. **本当の危険は別の場所にあります。**
   - token の漏れ → 端末ごとの service token と Access で、1 台ずつ取り消せるようにする
   - アカウントの乗っ取り → Cloudflare アカウントの二段階認証 (owner の設定を確認してもらう)
   - gitleaks が拾えない社外秘 (顧客名、社内 URL、貼り付けたログ) → repo 単位の同期除外 (`.oboete.toml` の `sync = false`)、prompt の `<private>` タグ (実装済み)、prompt を同期するかの設定 (§6 決定 3)
5. **将来の選択肢:** OSS 版で欲しい人のために、「中継のみ」モードを後から opt-in で足せます。op log を暗号文のまま R2 に置き、クラウド検索は無しにします。今は作りません。

---

## 6. owner に決めてもらうこと (4 つ)

1. **claude-mem の過去の記憶 (観測約 15 万件、prompt 約 1.5 万件) を oboete に取り込みますか。**
   - 推奨: **取り込む**。
   - 取り込んだ分には `source = claude-mem` の印を付け、自動注入からは既定で外し、検索には出します。目的 1「claude-mem を消して困らない」のためです。
   - 費用は埋め込み 1 回分で、無料枠の中なら約 4〜6 日、急ぐなら $0.35〜0.62 です。
   - 取り込むと英語の記憶が大半になるので (タイトルの日本語は 2.94%)、日英をまたいで強い多言語の bge-m3 が既定として正しくなります。取り込まない場合は、日本語専用の Ruri が勝つ見込みが上がります。
2. **有料で日本語の質が高いクラウド埋め込み (Gemini 有料版など) を使いますか。**
   - 推奨: **使わない**。
   - Gemini-001 は日本語の指標で 75.5 対 72.9 と上です。ただしクラウド専用なので、オフラインの意味検索を失います。3,072 次元を 1,536 次元以下に切り詰めたときの劣化も未計測です。
   - 費用は全件の作り直しで $6.0〜10.5 ($0.20/M、[料金](https://ai.google.dev/gemini-api/docs/pricing)。batch なら半額)、その後は月数十セントです。
   - 評価で bge-m3 の hybrid が合格線に届かなかったときに、もう一度相談します。
3. **打った文 (prompt) も他の端末とクラウドへ同期しますか。**
   - 推奨: **同期する** (既定オン)。
   - 「全端末で同じ記憶」と、prompt のカードも全部欲しいというこれまでの指示に沿います。暗号化しない方針もすでに決まっています。
   - ただし、prompt は gitleaks が拾えない顧客名や貼り付けたログを最も含みやすい表です。repo 単位の同期除外と `<private>` を併せて使ってください。慎重に行くなら既定オフも選べます。
4. **クラウドの意味検索に Vectorize (含み枠を超えた分、月 $0.08〜1.3) を使いますか。**
   - 推奨: **使う**。書くコードが一番少なく、手元と同じベクトルをそのまま入れられます。初月の請求で実額を確かめ、$1.5 を超えたら止めます。
   - 代案は 2 つです。1 つは DO の中で全件を総当たりする方式 ($0。遅さは未計測、DO のメモリ上限 128 MB に収めるには bit 化が要る: [Workers limits](https://developers.cloudflare.com/workers/platform/limits/))。もう 1 つは、クラウドでは全文検索だけにする方式です。

技術的な選択 (DO か D1 か、id の形、索引の形式など) は、claude-mem を基準にこちらで決めます。

---

## 7. 実装順 (PR 単位)

1 週間の実使用判定は owner の決定で廃止しました (2026-09-23)。この順序で PR-A から続けて進めます。残り 4 agent (agy / Pi / OpenCode / Cursor) の対応と Windows / macOS / arm64 のビルドは、この表とは別の並行の流れです (`docs/research/agent-adapters-2026-09-23.md`)。

| # | PR | 中身 | 証明すること |
|---|---|---|---|
| PR-A | 計測 spike (出荷しないコード) | Workers AI と fastembed の bge-m3 を実データ 100 件で比べる (cos と上位 10 件の一致)。8,000 字の prompt で Workers AI の入力上限と `truncate_inputs` を見る。日本からの往復の p50 / p95。oboete と claude-mem の実際のトークン数。DO / D1 で trigram FTS5 を作る 1 文。M1 での手元モデルの読み込み時間・RAM と、実ベクトル 15 万件の走査時間 (VPS は使うと決まったら) | §2・§4 の未確認の数字をすべて実測に置き換える。一致しなければ手元の問い合わせを無効にする (§2.3 の 3) |
| PR-B | 評価器 | `oboete eval` が qrels から TREC 形式の結果を出し、ranx で報告する。claude-mem DB は読み取り専用の写しを使う。全文検索だけの基準値を出す | 測れる状態になったこと。基準の数字 |
| PR-C | id と repo キー | 端末 id 付きの `uid`、origin URL の repo キーと移し替え、`"unknown"` session の修正、`embedder_id` とベクトル表 (`producer` 列つき)、各行の `synced_at` 列と変更用の outbox 表。`.oboete.toml` の読み取り (`repo = "名前"` と `sync = false`)、session に同期除外の印を記録 | WSL と Windows で同じ repo が同じキーになる (テスト)。`sync = false` の repo の session に印が付く (テスト)。replay で hook 時間が変わらない |
| PR-D | 意味検索の本体 | observe で文書の埋め込み: クラウドを使う形は Workers AI (100 件ずつ、日次 neuron 予算付き)、ローカルだけの形 (§0 の 5) は手元の fastembed。sqlite-vec の索引、search / MCP / viewer の hybrid RRF、オフライン時の手元の問い合わせ (fastembed)、`oboete reindex` | §3.3 の「意味検索そのもの」の合格線。落ちたら既定は `none` のまま |
| PR-E1〜E6 | 精度の工夫 (1 つ 1 PR) | E1 `since` / `until`、E2 要約の `keys`、E3 重複の間引き、E4 MCP 検索の reranker (xsmall-v2 と v2-m3 の比較)、E5 prompt の先頭か分割か、E6 M1 での int8 / bit | それぞれが合格線を越えたものだけ残す。越えなければその PR は閉じる |
| PR-F | prompt ごとの自動注入 | UserPromptSubmit に別の hook として `oboete inject` を登録する (予算 300 ms、timeout 1 秒、超えたら全文検索だけ)。Grok は最初の PreToolUse。しきい値を答えの無い問いで較正する。予算に収まらない端末だけ detached `recall` → PreToolUse の代案に切り替える | 誤注入 10% 以下、注入 hook の p95 300 ms 以内、timeout の割合 2% 以下。記録 hook の時間は変わらない (replay) |
| PR-G | hub (Worker + DO) | `/push` と `/pull` (seq のカーソル)、op の冪等、tombstone、Access の service token、書き出しの口 | `--home` を 2 つ使ったテストで、順番を入れ替えても削除が勝ち、最後に文書とベクトルが一致する |
| PR-H | 端末側の同期 | 未送信の行と outbox の送信 (observe の最後)、SessionStart からの detached pull、`oboete sync`、初回のページ送り、claude-mem の取り込み (決定 1)。同期除外の印がある session の文書・`vec`・prompt は送信の対象から外す (印は書き込み時に session に付く) | WSL と Windows の実機で往復する。同期を有効にする前からあった記憶が、2 台目の端末に全部届く (テスト)。`sync = false` の repo で作業しても hub に 1 件も入らない (テスト)。hook の時間が変わらない |
| PR-I | 3 台への配布 | cargo-dist で WSL / Windows / M1 iMac のビルド対象を作り、各端末で setup・hook・同期・検索を実行する | 3 台で同じ記憶が見える。M1 の RAM と速さの記録。VPS は owner が使うと決めたときに、同じ条件で 4 台目として足す (決定 10) |
| PR-J | クラウド検索とリモート MCP | DO の FTS5、Vectorize (決定 4)、同じ RRF、`createMcpHandler`、Access | 評価セットでクラウドと手元の上位 10 件が 9 件以上一致する。Claude アプリから検索できる |
| 後回し | クラウド viewer、スマホ、OAuth、Ruri への入れ替え、暗号化の「中継のみ」モード | — | 必要になったときに、同じ合格線で判断する |

**docs の直し (PR-C で一緒に):** plan.md §2b・§3・§8 をこの案に合わせます。削るものは EmbeddingGemma / e5-small / CJK bigram、「内容ハッシュ id」、D1、「Vectorize 月 $0.07」です。m1.md の「fastembed 5.17 (7.x は無い)」も直します。m1.md 決定 9 の `[embedding] provider = none|workers-ai|local` は、ベクトル空間を 1 つにしたことで「意味検索のオン / オフ」と「オフライン時の手元の問い合わせ (`local_queries`) のオン / オフ」の 2 つに畳みます。