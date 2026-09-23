# 記憶系 OSS から取り入れるもの (調査 2026-09-24)

調査の進め方: 6 つの観点ごとに調査担当 (Sonnet) が候補を集め、上位 4 件ずつを別の反証役 (Sonnet) が一次情報で確かめ、Opus が候補表にまとめました (24 件を確認、19 件が通過)。owner の指示 (2026-09-24「claude-mem を下回らず、他のメモリ系 OSS の良いところや設計も、良くなりそうなら順次取り入れて」) による調査です。

6 つの観点 (検索、書き込み時、時間、注入、コーディング用ツール、評価) で記憶系の OSS と論文を調べ、見つけた案ごとに反証役が一次情報 (論文・ソース・公式文書) を開いて確かめました。この文書の候補表に入れたのは、その確認を通った案だけです。確認を通らなかった案や、確認していない案は、最後の 2 節に分けて書きました。

## 要点

1. 2026-09-23 に実際に起きた 3 つの失敗 (提案を決定として記録した、覆った決定を注入し続けた、理由の無い「N 行変更」の語り) は、どれも要約の JSON に欄を 1〜2 個足して指示文を変えれば直せる見込みです。新しいモデルは要りません。
2. 覆った決定は消しません。新しい決定の行が「どの古い決定を覆したか」を指し、指された古い行を注入と検索から外します (Zep / Graphiti の「消さずに無効にする」形)。同期の「追加のみ」の設計はそのまま使えます。
3. 先に評価器 (PR-B) を 2 か所直します。今の「判定器を信用する条件」は UMBRELA 自身も満たせない厳しさで、このままだと判定器が毎回不合格になります。もう 1 つは、上書き・取り違え・前提違いを測る区画を足すことです。
4. claude-mem を下回らないために、SessionStart の注入を claude-mem と同じ「目次 + 必要なら取りに行く」形にします。
5. 新しさで記憶を沈める仕組み、グラフ DB、問い合わせのたびの LLM 呼び出しは、今の方針や制約と合わないので入れません。Jev / Laya と類似モデル (reranker など) の比較は、この調査とは別に提案書 §2.10 にまとめました。

## 取り入れる候補

1 行が 1 PR です。どれも合格線を越えたときだけ残し、越えなければその PR は閉じます。数字のしきい値は、出典が無いものはすべて提案値です。nDCG@10 は「上位 10 件の並びの良さ」を表す指標です。

| 順 | 何をするか | PR | 変えるところ | 合否の決め方 | 一次情報 |
|---|---|---|---|---|---|
| 1 | 判定器を信用する条件を直す | PR-B | 今の条件「人の正解との一致 (Cohen の κ、0〜3 の 4 段階) が 0.6 未満なら信用しない」をやめる。人が正解を付けた小さな問いの束で、比べる方式 (全文のみ・hybrid など) の勝ち負けが人の正解でも LLM の正解でも同じになるかで決める。1 件ずつの一致は「関係あり / なし」の 2 値の κ を参考値にする | 順位を比べる単位は方式の設定で、10 通り以上そろえる (全文のみ、hybrid の k=20 / 60 / 100、E1〜E6 のオン / オフ、reranker の有無など)。その並びが人の正解と LLM の正解で一致すること (Kendall の τ 0.85 以上、提案値)。方式が 4 つ以下しか無い PR-B の時点では τ が粗すぎる (4 つなら τ = 1.0 しか 0.85 を越えない) ので、仮の線を 2 値の κ 0.4 以上 (UMBRELA の実測 0.42〜0.50 の下端、提案値) にし、E 系の PR で方式が増えた時点で τ に切り替える。どちらかを通るまでは LLM の判定を合否に使わない | [UMBRELA](https://arxiv.org/html/2406.06519) 表 2、[再現論文](https://arxiv.org/abs/2507.09483)、[TREC 2024 RAG](https://arxiv.org/abs/2411.08275) |
| 2 | 決定まわりの 2 区画を評価セットに足す | PR-B | (a) 上書き区画: claude-mem の decision で「上書き」を思わせる語を含む行から、同じ話題の「古い決定 → 新しい決定」の組を作り、LLM と人で本当に覆ったか確かめる。問い = 覆った後の prompt。(b) 取り違え区画: 「提案」を思わせる語を含む行の元の会話を oboete の要約器にかけ、「決定」とした観測がユーザーの決定か提案かを判定する | (a) は nDCG と別の 2 値の指標「古い決定が注入と検索の上位 10 件に生きた決定として出ない」を置き、覆った決定を扱う PR (順 4) では 100% を必須にする (提案値)。組は人が確かめた 50 組以上 (提案値) | [MemoryAgentBench](https://arxiv.org/html/2507.05257v1)、[LongMemEval](https://arxiv.org/html/2410.10813v2) と [判定の指示文](https://github.com/xiaowu0162/LongMemEval)、[agent-memory-atlas](https://neoneye.github.io/agent-memory-atlas/benchmarks/) |
| 3 | 提案と決定を分けて記録する (失敗 1) | 新 PR-K1 | 要約の JSON で、`kind` が decision / preference の観測に `status` (`decided` か `proposed`) と `user_quote` (決めたユーザー自身の言葉をそのまま) を足す。指示文に「assistant の提案は、ユーザーが受け入れた言葉が無ければ決定にしない」を入れる。保存前に Rust で `user_quote` が、要約器に渡した文 (伏せ字と途中省略の後) の USER 行にそのまま含まれるかを確かめ、無ければ `proposed` に下げる (この確認は oboete 独自の追加)。SessionStart は `proposed` を決定として出さず、「提案」と表示して検索に残す (`status` の無い既存の行は今までどおり決定として出す。提案書 §7 PR-K1) | 区画 (b) で、「決定」と記録したもののうち実は提案だった割合が今の指示文より下がる (p < 0.05)。本物の決定の取りこぼし (recall) が 5 ポイントを超えて増えない (提案値)。検索のどの区画も nDCG@10 が 0.02 を超えて下がらない | [mem0 の指示文](https://github.com/mem0ai/mem0/blob/main/mem0/configs/prompts.py) (`attributed_to`、No Echo Extraction)、[Copilot Memory](https://docs.github.com/en/copilot/concepts/agents/copilot-memory) (ユーザーの発言をそのまま引用として持つ)、[Augment の Memory Review](https://www.augmentcode.com/blog/how-we-built-memory-review) |
| 4 | 覆った決定を注入から外す (失敗 2) | 新 PR-K2 (PR-C の後) | observe が要約器に「この repo の、まだ生きている決定」の一覧 (id と題) を見せる。新しい決定が古い決定を覆すとき、新しい観測に `supersedes` (古い id の配列) を書かせる。見せた一覧に無い id は捨てる。古い行は書き換えず、「新しい行が指している古い行」を注入と検索で外す。「上書き済み」は各端末が受け取った行から作り直す派生の状態にし (§4.3 の fts / vec と同じ扱い)、同期は追加のみのまま。timeline には「上書き済み」と表示して残す。指す先は端末をまたいで通じる PR-C の `uid` にする | 区画 (a) の 2 値の指標が 100% (提案値)、他の区画で nDCG@10 が 0.02 を超えて下がらない。observe の時間と要約の入力トークンの増え方を記録する (未計測) | [Zep 論文 §2.2.3](https://arxiv.org/html/2501.13956v1)、[Graphiti の edge_operations.py](https://github.com/getzep/graphiti/blob/main/graphiti_core/utils/maintenance/edge_operations.py)、[Graphiti #1728](https://github.com/getzep/graphiti/issues/1728) / [#1729](https://github.com/getzep/graphiti/pull/1729)、[mem0 #4956](https://github.com/mem0ai/mem0/issues/4956) |
| 5 | 変更の記録に理由を必須にする (失敗 3) | 新 PR-K3 | 観測に `why` (なぜそうしたか) を足す。指示文に「`git diff --stat` や行数だけの語りは記録しない。理由が書けない変更は記録しない」を入れる。保存時に Rust で、`kind` が change で `why` が空のもの、または「N 行変更」型だけの観測を注入から外す (検索には残す。この仕分けは oboete 独自) | 同じ会話を今の指示文と新しい指示文で要約し直し、理由の無い変更語りの割合が下がる (p < 0.05)。検索のどの区画も nDCG@10 が 0.02 を超えて下がらない | [GitHub Copilot のメモリ設計](https://github.blog/ai-and-ml/github-copilot/building-an-agentic-memory-system-for-github-copilot/) (1 件 = subject・fact・citations・reason) |
| 6 | 「前提違い」の問いを答えの無い問いに足す | PR-B | 今の「答えの無い問い」50 件は無関係な作業依頼だけ。これに、実在する話題で「記録に無い値」を尋ねる問い (例: 締切を話したが決めていないのに「決めた締切は?」) を約 30 件 (LongMemEval と同じ数) 足す。作り方は、答えのある問いの値を LLM で 1 つ差し替え、その値が元の会話のどこにも無いことを判定器で確かめる | 誤注入率を 2 つの束で別々に出し、自動注入 (PR-F) の「10% 以下」は難しいほうの束にも課す | [LongMemEval](https://arxiv.org/html/2410.10813v2) (ABS: 500 問中 30 問を前提違いに書き換え、専用の判定指示文) |
| 7 | SessionStart を目次形式にする (claude-mem 並み) | 新 PR-K4 | 今は直近の要約 3 件 (各 600 字) と観測 12 件 (各 240 字) で 4,000 字まで (`src/inject.rs` の定数)。これを 1 件 1 行の目次 (id、日付、`kind`、題、全文を取ったときの大きさの目安) にして載る件数を増やし、最後に「詳しくは `get` / `search` / `timeline`」の案内を付ける。長い本文は載せない | 同じ字数の上限で、その session の最初の prompt に関係する記憶が注入に載っている割合が上がる。今の 4,000 字の上限に収まる。SessionStart の時間が変わらない | [claude-mem の progressive disclosure](https://docs.claude-mem.ai/progressive-disclosure)、[Anthropic: context engineering](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)、[Anthropic: context management](https://claude.com/blog/context-management) |
| 8 | 重複の間引きで MMR と DPP を比べる | PR-E3 の中 | E3 (重複の間引き) で、予定の MMR (λ=0.5) と並べて DPP の貪欲選択 (関係の強さと互いの似かたを 1 つの行列で扱う) を試す。新しい依存は要らない (線形代数だけ) | nDCG@10 が 0.02 を超えて下がらず、選んだ記憶どうしの似かた (平均 cos) が下がるほうを残す。時間は注入の予算内 | [Chen ほか NeurIPS 2018](https://proceedings.neurips.cc/paper/2018/file/dbbf603ff0e99629dda5d75b6f75f966-Paper.pdf) |

**進める順番:** 1・2・6 は評価器の PR (PR-B) に入れるので最初に入ります。3 と 5 は PR-B の後ならいつでも始められます。4 は端末をまたいで古い行を指すために PR-C の `uid` が要るので、その後にします。7 は SessionStart の形を変えるだけなので独立しています。8 は E3 の中で比べます。

## 観点ごとの発見

### 検索

- **消さずに無効にする (Zep / Graphiti)**: 事実ごとに「いつから正しいか」「いつから正しくないか」の時刻を持ち、矛盾する新しい事実が来たら古いほうに「無効」の時刻を入れるだけで消しません。検索は無効でないものだけを出します。Zep は LongMemEval で 60.2% → 71.2% (gpt-4o、全文を入れた基準との比較)、待ち時間 -90% (28.9 秒 → 2.58 秒) を報告していますが、これは検索・並べ替え・時間の扱いをまとめた全体の数字で、無効化だけの効果は測られていません ([Zep 論文](https://arxiv.org/html/2501.13956))。→ 候補 4
  - 注意: Graphiti は、新しい事実と「意味が近い」既存の事実を探してから LLM で矛盾を判定します。oboete の案は、要約器が気づいて `supersedes` を書いた覆りだけを拾うので、Graphiti より拾える範囲が狭くなります。
- **時期の絞り込み (LongMemEval)**: 「先週」のような言い方を強い LLM が日付の範囲に直してから絞ると、時期を問う質問の recall が平均 +11.3% (ラウンド単位) / +6.8% (セッション単位) 上がりました (v2 の本文で確認。小さい Llama-8B では範囲の読み違いで効かなかった: [LongMemEval](https://arxiv.org/html/2410.10813v2))。oboete では MCP を呼ぶ agent 自身が強い LLM なので、別の LLM 呼び出しは足さず、E1 (`since` / `until`) の道具説明に「先週・前回などは日付に直して渡す」と一文書けば足ります。
- **DPP による多様化 (Hulu)**: 関係の強さと互いの似かたを 1 つの行列で扱い、関係が強くて互いに似ていない組を一度に選びます。論文の計測では 1 回 p99 で 1.75 ms / 1.46 ms (ノート PC の Python)。同じ論文の 4 週間の実運用 A/B で、DPP は 1.33〜1.52%、MMR は 0.84〜0.86% の改善でした (映画推薦での数字。YouTube の論文の数字ではありません: [Chen ほか 2018](https://proceedings.neurips.cc/paper/2018/file/dbbf603ff0e99629dda5d75b6f75f966-Paper.pdf) 表 2)。→ 候補 8
- **新しさ・よく使うかの点数 (MemoryOS)**: 点数 = 呼ばれた回数 + やり取りの量 + exp(-経過時間/μ)。LoCoMo で F1 +49.11% と報告していますが全体の数字で、この点数だけの効果は測られていません ([MemoryOS](https://arxiv.org/html/2506.06326))。oboete の方針 (§2.4「採らない」) と合わないので入れません (「取り入れないもの」)。

### 書き込み時

- **誰の発言かを区別する (mem0)**: mem0 の抽出指示文は、抜き出す項目ごとに `attributed_to` (誰に帰属するか) を持たせ、assistant の提案は「ユーザーは X を勧められた」と書き、ユーザーが受け入れない限り事実や決定にしません。assistant がユーザーの発言を繰り返しただけのものは抜き出しません (No Echo Extraction)。別の指示文ではユーザーの発言だけから抜き出すよう求めています ([prompts.py](https://github.com/mem0ai/mem0/blob/main/mem0/configs/prompts.py))。この規則だけの効果を測った数字はありません。→ 候補 3
- **追加・更新・削除・何もしない、の 4 択 (mem0)**: 新しい事実ごとに、似た既存の記憶を取り出して LLM に 4 択を選ばせます。仕組みは論文とソースで確認できましたが、調査担当が書いた「LongMemEval 94.4 / LoCoMo 92.5」は論文に無く、論文の主張は LoCoMo で OpenAI の基準より LLM 判定の点が相対 26% 高い、という全体の数字です ([mem0 論文](https://arxiv.org/html/2504.19413))。oboete は「削除」だけを「消さずに無効にする」形で取り入れ (候補 4)、「更新」(行の書き換え) は入れません。
- **予測との差分だけを残す (Nemori)**: 既存の記憶から「この会話には何が書いてありそうか」を LLM に予測させ、実際との差分だけを知識として残します。A-MEM や MemoryOS に重ねると保存量が 45〜64% 減り、性能は保たれました。予測の段を足すだけで正答が相対 +25.0% (gpt-4o-mini、52.0 → 65.0) / +14.4% (gpt-4.1-mini、65.5 → 74.9) 上がっています ([Nemori](https://arxiv.org/html/2508.03341v4))。「N 行変更」の語りのような予測できる内容が消えるので失敗 3 に効くはずですが、要約の呼び出しが倍になり作る手間も大きいので、候補 5 で足りなかったときの次の手にします。

### 時間

- **2 つの時間軸 (Graphiti)**: 「世の中で正しかった期間」と「システムが知っていた期間」を別々に持ちます。矛盾があれば古いほうに無効の時刻と失効の時刻を入れ、新しい辺を作ります ([時間モデル](https://getzep-graphiti.mintlify.app/concepts/temporal-model))。
- **範囲を絞らないと関係ないものまで無効にする (Graphiti #1728)**: 無効にする候補を探す範囲の絞り込みが外れ、同じ実体に触れているだけの事実まで無効にされ、報告者の本番グラフでは辺の 41% が誤って無効になっていました。修正 ([#1729](https://github.com/getzep/graphiti/pull/1729)) では「同じ実体の組」か「共通の実体 + 同じ関係名」に限っています ([#1728](https://github.com/getzep/graphiti/issues/1728))。→ 候補 4 で「要約器に見せた一覧の id しか受け付けない」理由です。oboete の `keys` は Graphiti の型付きの実体より粗いので、最後の安全網は LLM の判定と一覧の制限の 2 つです。
- **追加だけでは新しい事実が負ける (mem0 #4956)**: mem0 の v3 で「追加のみ」にし、覆りの印を持たなかったところ、古い「X 社を辞めた」が新しい「Y 社の新しい役割を楽しんでいる」に勝ってしまう報告が出ています ([#4956](https://github.com/mem0ai/mem0/issues/4956))。追加のみの設計でも、覆ったことを明示する印が要るという根拠です。→ 候補 4
- **mem0 の「矛盾」欄の PR は採用されていない**: 調査担当は mem0 の [PR #4911](https://github.com/mem0ai/mem0/pull/4911) を「採用された設計」としましたが、実際はマージされずに閉じられ、maintainer は v3 では追加のみ + `linked_memory_ids` で扱うと書いています。[PR #6017](https://github.com/mem0ai/mem0/pull/6017) もまだ開いたままで、調査担当が書いた「coffee / tea の誤判定」の例は見つかりませんでした。`status` 欄と `supersedes` 欄は、mem0 の本番の教訓ではなく oboete 自身の設計として扱います。
- **引用の確認と 28 日の失効 (GitHub Copilot)**: 記憶は根拠の場所 (ファイルと行) を持ち、使う前に今のブランチで根拠を確かめ、食い違えば直した版を書かせます。意図的に食い違う記憶を仕込んだ試験で、agent は毎回見抜いて直したと書かれていますが、数字は公開されていません ([GitHub blog](https://github.blog/ai-and-ml/github-copilot/building-an-agentic-memory-system-for-github-copilot/))。28 日使われない記憶は自動で消えます ([Copilot Memory](https://docs.github.com/en/copilot/concepts/agents/copilot-memory)、[2026-03-04 changelog](https://github.blog/changelog/2026-03-04-copilot-memory-now-on-by-default-for-pro-and-pro-users-in-public-preview/))。根拠のパスがまだあるかを注入時に確かめる案は、E2 の `keys` にパスが入った後の次の候補にします (ハッシュで比べる案は、コードが変わったことしか分からず、会話で決定が覆ったことは拾えません)。28 日の失効は入れません (「取り入れないもの」)。

### 注入

- **目次を注入して、本文は必要なときに取りに行く (claude-mem)**: claude-mem の SessionStart は 1 件 1 行の目次 (id、時刻、種類、題、全文を取ったときのトークン数の目安) を出し、最後に MCP の道具で取りに行く案内を付けます。50 件の目次で約 800 トークン、1 件の全文は例で約 51〜193 トークンです ([progressive disclosure](https://docs.claude-mem.ai/progressive-disclosure))。比較基準の claude-mem がこの形なので、oboete も並ぶ必要があります。→ 候補 7
- **Anthropic の文脈管理**: Anthropic は「軽い参照だけを文脈に置き、中身は必要なときに道具で取る」を勧めています ([context engineering](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents))。社内の評価で、古い道具の結果を片付けるだけで +29%、メモリの道具を加えると +39%、100 ターンの評価でトークン -84% でした。ただし記事では 2 つの文に分かれていて、同じ評価の数字かは書かれていません ([context management](https://claude.com/blog/context-management))。
- **全 repo 共通の小さな「好み」ブロック (Letta / Copilot)**: Letta は常に文脈に置く小さなブロック (persona / human) に文字数の上限を付け、それ以外は必要なときに取ります。上限は設定値で、既定値は無く、文書の例は 4,000〜6,000 字です ([memory blocks](https://docs.letta.com/guides/core-concepts/memory/memory-blocks))。Copilot も repo ごとの事実と、ユーザーについて repo をまたぐ好みを分けています。oboete の plan.md §5・§10 にすでに「personal prefs だけ全 repo に注入」があり、まだ作っていません。作るときは `kind = preference` の行を repo をまたいで引き、文字数の上限を付けたブロックにするのが具体的な形です (上限は oboete の予算で決める)。

### コーディング用ツール

- **1 件に理由を持たせる (GitHub Copilot)**: Copilot の記憶を作る呼び出しは `{subject, fact, citations, reason}` の形で、reason は「なぜ大事か」です ([GitHub blog](https://github.blog/ai-and-ml/github-copilot/building-an-agentic-memory-system-for-github-copilot/))。記事にはメモリ全体の効果として、コードレビューの精度 +3% / 再現率 +4%、coding agent の PR マージ率 90% 対 83%、好意的なコメント 77% 対 75% が載っていますが、reason 欄だけの効果ではありません。「N 行変更」型を注入から外す仕分けは Copilot には無く、oboete 独自の追加です。→ 候補 5
- **確定には証拠を求める (Copilot / Augment / ByteRover)**: Copilot の好みの記憶は「ユーザーの発言をそのまま含むことがある」引用を持ちます ([Copilot Memory](https://docs.github.com/en/copilot/concepts/agents/copilot-memory))。Augment は記憶を残す前に承認・編集・破棄を選ばせ ([Memory Review](https://www.augmentcode.com/blog/how-we-built-memory-review))、別の仕組みでは証拠が溜まってから学んだことを昇格させます ([Cosmos](https://docs.augmentcode.com/cosmos/experts-memory))。ByteRover は「下書き → 確認済み → 中核」の段階を持ちます ([論文](https://arxiv.org/html/2604.01599)、LoCoMo 96.1% / LongMemEval-S 92.8% は自己申告で再現されていない。Elastic License 2.0 なのでコードは読んでいない)。oboete には承認の画面が無いので、ユーザー自身の言葉の引用で代える形にしました。→ 候補 3
- **Cline Memory Bank には「上書き」の仕組みは無い**: 調査担当は Cline を「上書きの印」の出典にしましたが、実際は「頻繁に書き直す今の状態のファイル」と「追記だけの履歴ファイル」を人の指示で更新する平らな markdown で、決定ごとの状態も矛盾の検出もありません ([Memory Bank](https://docs.cline.bot/best-practices/memory-bank))。候補 4 は Graphiti を出典にします。

### 評価

- **判定器の信用の測り方 (UMBRELA)**: UMBRELA (GPT-4o) と NIST の人の判定との一致は、4 段階の κ で 0.31〜0.37、2 値の κ で 0.42〜0.50 しかありません。一方、方式の順位の一致は Kendall の τ 0.87〜0.94、Spearman の ρ 0.97〜0.99 と高く、TREC はこの順位の一致で採用しました ([UMBRELA](https://arxiv.org/html/2406.06519) 表 2)。他の LLM での再現でも同じ傾向で、小さな FLAN-T5-large は 4 段階の κ が 0.062 まで下がっても ρ 0.971 / τ 0.868 を保ちました ([再現論文](https://arxiv.org/abs/2507.09483))。TREC 2024 RAG では、人が LLM を手伝っても順位の一致は上がりませんでした ([TREC 2024 RAG](https://arxiv.org/abs/2411.08275))。今の §3.1 の「κ 0.6 未満なら信用しない」は、UMBRELA 自身が越えられない線です。→ 候補 1
  - 調査担当の案は「人が確かめた約 50 組の点数の順位相関を 0.85 以上」でしたが、論文の 0.87〜0.94 は **方式どうしの順位** の一致で、1 件ずつの点数の相関ではありません。そこで候補 1 は「方式の勝ち負けが人と同じか」で決める形に直しました。
- **覆りの扱いはどのシステムも苦手 (MemoryAgentBench)**: 古い事実と新しい事実の矛盾を解く課題 (FactConsolidation の多段) で、Mem0 2%、MemGPT 3%、BM25 3%、NV-Embed-v2 6% など、どの方式も 6% 以下でした ([MemoryAgentBench](https://arxiv.org/html/2507.05257v1))。調査担当は MIRIX も試されたと書きましたが、論文に MIRIX は出てきません。
- **どのベンチも「古い値がまだ出てくるか」は測っていない**: BEAM の矛盾解決の問いは「正しい値で答えたか」だけを見ていて、退けた値がまだ残っていて出てくるかは問わない、という指摘があります ([agent-memory-atlas](https://neoneye.github.io/agent-memory-atlas/benchmarks/))。oboete の失敗 2 はまさにこちらなので、候補 2 の 2 値の指標を自前で持ちます。
- **LongMemEval の上書きの判定は甘い**: 上書き (knowledge-update、500 問中 78 問) の判定指示文は「古い情報が一緒に書かれていても、新しい答えが入っていれば正解」とします ([evaluate_qa.py](https://github.com/xiaowu0162/LongMemEval))。oboete はこれに加え、古い決定が「上書き済み」の印なしで出たら不合格にします。Zep の公開した内訳でも、上書きの正答は gpt-4o-mini で 76.9% → 74.4% と、全体が良くなる中で下がっていました ([Zep 論文](https://arxiv.org/html/2501.13956))。
- **公開ベンチの点数は合格線にしない**: mem0 の論文は Zep の LoCoMo を 65.99% と載せ、Zep が自分で設定を直して 10 回走らせると 75.14% ± 0.17 で、実装と指示文の違いだけで約 9 ポイント動きました ([zep-papers #5](https://github.com/getzep/zep-papers/issues/5)、[mem0 論文](https://arxiv.org/html/2504.19413))。Zep の論文自身も、記憶なしで会話を全部入れるだけで DMR が 94.4% (gpt-4-turbo) / 98.0% (gpt-4o-mini) になり、ベンチとして飽和していると書いています ([Zep 論文](https://arxiv.org/abs/2501.13956))。合否はいつも、この PC での同じ条件の測り直し (§3.3 決定 22) で決めます。§2.9 に 1 段落足す程度で、作るものはありません。

## 評価セットに足すもの

§3.1 の表に足す行です。どれも評価の時だけ動き、記録 hook と注入 hook には触りません。

| 材料 | 量 | 作り方 | 測れること |
|---|---|---|---|
| 上書きの組 (候補 2 の a) | 人が確かめた 50 組以上 (提案値) | claude-mem の decision のうち上書きを思わせる語を含む行 (依頼文の見積りで約 26%、正規表現での推定・未再現) を、repo と `keys` が重なるもので組にし、時刻順に並べ、LLM と人が本当に覆ったか確かめる | 覆った決定が生きた決定として注入・検索に出ないこと (2 値、100% 必須・提案値) |
| 提案と決定の取り違え (候補 2 の b) | 人が確かめた 50 件以上 (提案値) | 提案を思わせる語を含む decision の行 (約 40%、同じく推定・未再現) の元の会話を、oboete の要約器に今の指示文と新しい指示文でかけ、「決定」とした観測がユーザーの決定か提案かを判定する | 提案を決定として記録した割合 |
| 前提違いの問い (候補 6) | 約 30 件 | 答えのある問いの値を 1 つ差し替え、その値が元の会話に無いことを判定器で確かめる | 話題は合っているが答えの無い問いへの誤注入率 |
| 理由の無い変更語り (候補 5) | replay の fixture と、実際の session | 同じ会話を 2 つの指示文で要約し直し、LLM と人で「理由の無い変更語り」かを判定する | 変更語りの割合 |

判定の信用は候補 1 の方法で確かめます。

**候補止まり (反証役の確認を通っていないので、足すかは PR-B で決める):**

- BEAM の「要素ごとに 0 / 0.5 / 1 で採点」する方式を、「この repo の決定を全部」のようなまとめの問いにだけ使う ([BEAM](https://arxiv.org/html/2510.27246v1))。
- MemBench が 1 万トークンと 10 万トークンで性能が大きく落ちた例にならい、関係ない文書を 5 倍に増やした状態でも合格線を測る ([MemBench](https://arxiv.org/html/2506.21605v1))。
- 同じ話題で 3 回以上変わった決定の順番を `timeline` が正しく返すかを、Kendall の τ-b で測る (BEAM の Event Ordering)。
- 要約器に XML やコードの断片を含む会話を与え、出力の `kind` が決められた種類から外れないかを確かめる、決まった結果の出る試験 (claude-mem で 141 行に XML が漏れた件の予防)。

## 取り入れないもの

| 案 | 出典 | 入れない理由 |
|---|---|---|
| 新しさ・使用頻度で記憶を沈める (heat score、忘却曲線) | [MemoryOS](https://arxiv.org/html/2506.06326)、[MemoryBank](https://ojs.aaai.org/index.php/AAAI/article/view/29946) | §2.4 の方針で「古くても正しい決定を沈める」として採らないと決めている。古くなった決定は候補 4 の印で外す。なお `EXP()` は今の rusqlite (bundled) の SQLite では使えず、使うなら Rust 側で計算する |
| 28 日使われない記憶を自動で消す | [Copilot Memory](https://docs.github.com/en/copilot/concepts/agents/copilot-memory) | 上と同じ理由 (claude-mem の 90 日窓の問題と同じ形) |
| 問い合わせのたびに LLM で日付の範囲を出す | [LongMemEval](https://arxiv.org/html/2410.10813v2) | MCP を呼ぶ agent が日付を埋める設計で足り、無料枠を要約と奪い合う。道具説明の一文で代える |
| 問い合わせの書き換え (RAG-Fusion)、HyDE、LightRAG の 2 段キーワード | [RAG-Fusion](https://github.com/Raudaschl/rag-fusion)、[HyDE](https://arxiv.org/pdf/2212.10496)、[LightRAG](https://arxiv.org/html/2410.05779v1) | §2.4 ですでに採らないと決めている。どれも反証役の確認を通っていない。HyDE には「LLM が正解を覚えていたせいで効いて見えた」とする研究もある ([2504.14175](https://www.alphaxiv.org/abs/2504.14175)、未確認) |
| mem0 の「更新」「削除」で行を書き換える・消す | [mem0 論文](https://arxiv.org/html/2504.19413) | 同期は「追加のみ」の設計。覆りは候補 4 の新しい行の印で表す。mem0 自身も v3 で追加のみ + `linked_memory_ids` に移った ([PR #4911](https://github.com/mem0ai/mem0/pull/4911) の maintainer の説明) |
| グラフ DB (Graphiti 本体、Cognee の Graphiti 形、Mem0 のグラフ) | [Graphiti](https://github.com/getzep/graphiti) | Neo4j などの常駐 DB が要り、SQLite 1 ファイルの制約に反する。役に立つ「無効の時刻」の考え方だけを候補 4 で SQLite に移す |
| A-MEM の書き込みごとの関連付け直し | [A-MEM](https://arxiv.org/abs/2502.12110) | 新しい記憶のたびに LLM で既存の記憶を書き直すので、要約の呼び出しが大きく増える |
| Letta の会話中の自己編集、LangMem の会話中の記憶ツール | [Letta](https://docs.letta.com/)、[LangMem](https://langchain-ai.github.io/langmem/concepts/conceptual_guide/) | 会話の途中で agent が記憶を書き換える形で、常駐なし・記録 hook 20 ms の設計と合わない |
| ByteRover の Context Tree、Cline の markdown を記憶の置き場にする | [ByteRover](https://arxiv.org/html/2604.01599)、[Cline](https://docs.cline.bot/best-practices/memory-bank) | SQLite 1 ファイルの制約に反する。ByteRover は Elastic License 2.0 |
| DMR を評価に使う | [Zep 論文](https://arxiv.org/abs/2501.13956) | 記憶なしで 94.4〜98.0% が出るほど飽和していて、方式の差が出ない |
| 他社が公表したベンチの点数を合格線にする | [zep-papers #5](https://github.com/getzep/zep-papers/issues/5) | 設定の違いだけで約 9 ポイント動く。合否はこの PC での測り直しだけで決める |

## 未確認のこと

- Jev / Laya と類似モデルの比較は、提案書 §2.10 に書きました (この 6 観点とは別に、一次情報で確認)。§2.10 (b) の「まず要約の指示文で直す」段が、本書の候補 3・4 にあたります。
- claude-mem の decision 3,902 件のうち提案らしい語を含むのが約 40%、上書きらしい語を含むのが約 26%、`type` に XML が漏れた行が 141 件という数字は、依頼文にある正規表現での見積りで、この調査では再現していません。
- **`kind` の検査が無い (oboete のコードで確認した事実)**: `src/observe.rs` の `parse_observations` は、要約器が返した `kind` を決められた種類と照らさずにそのまま保存しています (JSON schema には種類の一覧がありますが、Groq の strict 以外の CLI の provider がそれを必ず守るかは未確認)。claude-mem の 141 行と同じ種類の事故を防ぐ数行の修正で、評価の合否は要りません。反証役の確認を通っていない案なので候補表には入れていません。
- 反証役が確認していない案 (調査担当が挙げたが、一次情報で確かめていないもの):
  - 「覚えておいて」と明示された訂正だけを、要約を通さずすぐ書く道 ([LangMem](https://langchain-ai.github.io/langmem/concepts/conceptual_guide/) の hot path と background の区別)。
  - 書き込み時に LLM が 1〜10 の重要度を付ける ([Generative Agents](https://arxiv.org/pdf/2304.03442))。
  - 決まった間隔で記憶をまとめ直す、Letta の sleep-time / dreaming ([Sleep-time Compute](https://arxiv.org/html/2504.13171v1))。
  - 提案から決定への格上げに、複数の session での裏付けを求める ([Mdia92/memoryos](https://github.com/Mdia92/memoryos)。1 人のハッカソン作品で、評価も自作の合成データ)。
  - 矛盾を 3 種類 (時間で変わった / 事実の誤り / 条件によってどちらも正しい) に分け、「条件によってどちらも正しい」ものは上書きにしない ([MemConflict](https://github.com/TaoZhen1110/MemConflict))。候補 4 の指示文に一文足す価値はありそうですが、確かめていません。
  - Web や MCP の結果を使った session から出た決定に印を付け、そのまま確定にしない (Codex の `disable_on_external_context`: [設定](https://developers.openai.com/codex/config-reference))。
  - 今の作業ファイルのパスと `keys` が重なる記憶を上に出す (Cursor / Windsurf のルールの発火条件の考え方: [Cursor rules](https://cursor.com/docs/rules))。
  - `keys` の語がどれだけ共通するかで順位を上げる (mem0 のグラフメモリの軽い版: [mem0 論文](https://arxiv.org/html/2504.19413v1))。
  - claude-mem の週報・経緯のレポート (常駐 worker が必要) に並ぶ、1 回で終わる `oboete report`。plan.md では「価値が分かれば入れる」(M3) の扱い。
- Nemori の論文が ACL 2026 に採択されたかは、論文の本文では確認できていません (数字は arXiv v4 の表から)。
- 候補 4 で要約器に見せる「生きている決定」の件数と、それによる入力トークン・時間の増え方は未計測です。Groq の無料枠では長い session で「max completion tokens reached」の 400 がすでに出ているので (m1.md)、PR-K2 で測ります。
- 失敗が起きた 2026-09-23 の session の生イベントが、要約し直しに使える形で残っているかは確かめていません (生イベントは要約成功の 30 日後に消えます)。
- Copilot の「引用の確認で目立った遅れは出ない」は GitHub 自身の報告で、外部の計測はありません。
