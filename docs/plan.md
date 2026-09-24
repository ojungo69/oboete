# oboete v2 (Rust) — 進め方の案 (draft, 2026-09-22)

## 合意済みの「作りたいもの」(owner, 12 項目 + 修正)

1. 目的: claude-mem を消して困らない。全 agent・全端末で同じ記憶。スマホ参照は「できたら」。
2. 場面: セッション冒頭の自動コンテキスト、agent からの検索、viewer。レポート系は価値が分かれば入れる。
3. agent: 必須 Claude Code / Codex / Grok Build。Antigravity CLI (`agy`)・OpenCode v2・Pi・Cursor (CLI / IDE 共通) の adapter も実装済み (2026-09-24、残る実環境確認は `docs/research/agent-adapters-2026-09-23.md` を参照)。Gemini CLI は対象外。
4. 要約の頭脳: サブスク CLI (agy / claude / codex / grok)、無料クラウド (OpenRouter free / NIM / Groq / Mistral)、ローカル (Ollama)、有料 API (明示時のみ)。フォールバック連鎖必須。要約が止まらない = 記録漏れ事故ゼロ。
5. 置き場所: 使い勝手が良ければクラウド正本。暗号化/平文は私が決める。有料は都度確認。
6. 覚えるもの: claude-mem / cmem 相当 + 類似 OSS の良い部分。repo 間共有は私が決める。
7. 軽さ: 上限数値は不要。メモリ食い潰しバグが無いこと。
8. 言語: Rust 希望。軽くて壊れなければ言語不問。
9. 公開: 自分用が第一、動いたら OSS。
10. 費用: 基本自由、有料は都度確認、サブスク枠は使ってよい。
11. 完璧: 全 agent・全端末で同じ記憶が本線。スマホはおまけ。
12. 今の TS 版は無駄になってよい。

## 裏取りの要点

- 規約: `claude -p` を hook から呼ぶのは Anthropic の規約で明示許可 (code.claude.com/docs/en/legal-and-compliance)。codex は規約に記載なし。
- claude-mem の実態: 対応 agent = CC / Codex / OpenClaw / OpenCode / Antigravity / Grok Bot(ログ監視)。Pi・Grok Build hook 無し。provider = Claude サブスク / Gemini / OpenRouter のみ。常駐 bun worker + Chroma(Python)。リーク報告 13〜65 GB、ディスク 51〜634 GB の事故。この PC: Chroma 4.65 GB RSS、データ 5.9 GB、観測 15 万件。Chroma は `CLAUDE_MEM_CHROMA_ENABLED=false` で切れる(FTS のみになる)。
- Rust 部品: rusqlite(bundled, FTS5 trigram 内蔵)、sqlite-vec 0.1.9、rmcp 3.4(公式 MCP SDK)、async-openai(base_url 差し替え)、axum + rust-embed、fastembed 7(ONNX, 多言語 e5 可)、cargo-dist、toml。全部保守中。
- 類似 Rust OSS 9 件: 単独で土台にできるものは無し。部品取り: memori-core(FTS5+vec RRF 検索、2.8k 行)、icm `summarizer.rs`(CLI 自動検出)、remem `src/ai/cli.rs`(`claude -p` 起動)、palace-rs `hooks.rs`(CC/Codex/Cursor の hook 方言表)、sessiongrep `providers/*`(CC/Codex/Cursor/Antigravity/Pi のセッション読取)、memory-forge `platforms/grok.rs`、funes の送信前 redaction 二重ゲート、leteo の replication spec。全部 MIT/Apache-2.0。
- 無料枠: Groq 1,000 回/日・strict json_schema(最安定)。OpenRouter free 50 回/日(生涯 $10 購入で 1,000/日)。NIM 約 40 回/分・上限非公開・構造化出力は `nvext.guided_json`。Mistral 約 30 回/分。Gemini free は入力を学習・人間レビュー(規約) → 既定から外す。Workers AI 1 万 neuron/日で窮屈。Ollama local は `response_format` 対応。
- CLI: `claude -p --json-schema --system-prompt --model` / `grok -p --json-schema --system-prompt` / `agy -p --json-schema`(system prompt 無し) / `codex exec --output-schema -o`(system prompt 無し、`--json`)。
- hook: CC / Codex / Grok は同じ JSON 方言 + `hookSpecificOutput.additionalContext`(Grok は UserPromptSubmit で注入不可 → SessionStart/PostToolUse 経由)。Codex は `~/.codex/hooks.json` か plugin manifest `.codex-plugin/plugin.json` の `hooks`。agy は `.agents/hooks.json` / `~/.gemini/config/hooks.json`、PreInvocation/PostInvocation の `injectSteps` で注入。Cursor は `~/.cursor/hooks.json`、`additional_context`。Pi は TS 拡張 `before_agent_start` / `context_with_system`。OpenCode v2 は生成した JS plugin の `ctx.session.hook("context", ...)` で注入する。

## 方針 (案)

1. **新 repo `oboete`(Rust、clean start + 部品移植)**。`free-mem` は参照用に残す(archive)。TS 版から引き継ぐのは仕様として: 4 agent の hook 形式、要約プロンプト/スキーマ、秘密検出規則、1,000 event fixture、bake-off の数字。コードは持ち込まない。
2. **1 バイナリ** `oboete`: `hook <agent> <event>`(stdin JSON → SQLite 追記、目標 ≤ 20 ms)/ `observe`(pending を要約。Stop hook から detached 起動、単一 lease、終わったら exit。常駐なし)/ `inject`(SessionStart / prompt)/ `search|get|timeline` / `mcp`(rmcp stdio)/ `view`(axum + 埋め込み SPA)/ `setup <agent>` / `doctor` / `sync`(M2)。
2b. **意味検索(owner 合意 2026-09-22 夜)**: 埋め込みモデルは差し替え可能な provider にする。`[embedding] provider = "workers-ai" | "local" | "none"`。クラウド派は Workers AI `@cf/baai/bge-m3`(1024 次元、300 件/日で約 100 neuron = 無料枠の 1%)、ローカル完結派は fastembed の EmbeddingGemma-300m(768 次元、約 300 MB、オフライン)。索引は手元の sqlite-vec。クラウド索引は M2〜M3 で Vectorize(REST upsert、15 万件で月約 $0.07)に同じベクトルを流す。AI Search は使わない(短い構造化メモには自動変換の価値が無く、索引の二重化と非公開の遅延が残る)。**制約**: 1 つの store に埋め込みモデルは 1 つ(モデルが違うベクトルは比べられない)。モデル名と次元を DB に記録し、切り替えは `oboete reindex` で全件作り直す(15 万件でも Workers AI なら約 $0.5、ローカルなら数時間)。既定は `none`(全文検索のみ)で、setup が 1 回だけ「ローカル / Workers AI / なし」を聞く。Jev / Laya は評価・分類モデルで検索用途ではないため不採用。Ruri v3(日本語 JMTEB 74.5〜77.2、fastembed 未対応)は追跡し、読める形にできたら候補に足す。M3 で bge-reranker を検討。
3. **データ = SQLite 1 ファイル** `~/.oboete/oboete.db`: sessions / events(生。要約成功後 30 日で削除)/ observations(claude-mem の型)/ summaries / prompts / fts(trigram + CJK bigram)/ vec(sqlite-vec + fastembed multilingual-e5-small)/ injections / provider_calls。約 9 表。work item・共有承認・移行記録は作らない。
4. **要約 = provider chain + fallback**。設定は順序付きリスト。既定: Groq free → claude → OpenRouter free → NIM → Mistral free → codex → grok → (有料 API は明示設定時のみ末尾)。agy は 2026-09-25 に既定から外した: headless の agy は道具を切るスイッチが無く、利用者の設定 (道具の許可・plugin) をそのまま引き継ぐため、信頼できない記録を読む要約役には置かない (明示設定すれば使えるが、`--dangerously-skip-permissions` は付けない)。理由: Groq は 1,000 回/日で最安定、サブスク枠は owner が「腐っている」ので次、OpenRouter free は $10 未購入だと 50 回/日、NIM は上限非公開。429/5xx/timeout → その provider を cooldown して次へ。JSON schema 検証失敗 → 次へ。全滅 → pending のまま次回。生イベントは要約成功まで保持。**provider ごとに日次予算**(fallback 自体が暴走しないため。TS 版 #352 の教訓を仕様として引き継ぐ)。Gemini free は opt-in。codex / agy は system prompt フラグが無いので指示は user prompt に同梱。
5. **注入**: SessionStart = 直近セッション要約 + この repo の上位 observation(新しさ加重)+ personal prefs。UserPromptSubmit = hybrid 検索(FTS + vec, RRF)上位を予算内で。同一セッション再注入なし。注入文は再要約しない印付き。CC / Codex は SessionStart と UserPromptSubmit の両方で `additionalContext` 可(CC はバイナリで確認済み)。**Grok Build は UserPromptSubmit で注入不可**(仕様上 discard)→ Grok はセッション冒頭のみ、既知の非対称として扱う。agy は `injectSteps`、Cursor は `additional_context`、Pi は TS 拡張の `before_agent_start` から一度だけ非表示の custom message を返す (resume は再注入なし、compaction 後は再取得)、OpenCode v2 は JS plugin から system prompt に push (OpenCode は非永続なので呼び出しごとに再注入)。
6. **秘密**: gitleaks 規則(Rust regex)で保存前に伏せ字。外部送信(要約 API / sync)直前にもう一度ゲート(funes 方式)。`.oboete.toml` の path glob で secret 扱い強制。
7. **軽さ**: 常駐プロセスなし。hook は起動 5 ms 級。observe の RSS 目標 < 50 MB(計測して記録)。Chroma のような外部プロセス無し。
8. **同期 (M2)**: Cloudflare Worker + D1 を hub。append-only op log を device が push/pull。content-hash id + tombstone、CRDT 無し。自アカウント内に平文(秘密は伏せ字済み)。**Private MCP link (M3)** = 同じ Worker が MCP over HTTP + token を出す → iMac/スマホ/Claude アプリから検索。暗号化は将来の opt-in。
9. **viewer**: 既存 oboete の Preact viewer を移植、rust-embed で同梱。Pi は TS shim、OpenCode v2 は依存なしの生成 JS plugin。
10. **repo 間共有**: personal prefs(明示的な好み・ルール)だけ全 repo に注入。他は repo 内。

## マイルストーン

- **M0 spike(2〜3 日)**: CC hook → SQLite → **2 段の provider chain(Groq free → agy)、1 段目を強制失敗させてフォールバックを実証** → SessionStart 注入。既存 1,000 event fixture を replay。hook 時間 / RSS / 要約成功率 / フォールバック回数を計測 → go/no-go。必要なキーは Groq 1 つ(agy はサブスク)。
- **M1(約 2 週)**: CC / Codex / Grok Build、fallback 連鎖、伏せ字、hybrid 検索、MCP 検索、viewer、setup/doctor。本環境に claude-mem と併用で導入 (2026-09-23)。1 週間の実使用判定は owner の決定で廃止 (2026-09-23)、判定を待たずに M2 / M3 へ進む。
- **M2**: iMac / Windows ビルド(cargo-dist。VPS は使うと決まったら)、Cloudflare 同期。
- **M3**: Pi / agy / OpenCode / Cursor、Private MCP link、意味検索の調整、レポート(価値があれば)。
- **M4**: OSS 公開(README、インストーラ)。

## M0 結果 (2026-09-22 夜、debug build、commit 480b761) — **GO**

`events-1000.jsonl` の Claude Code 分 (255 イベント、12 セッション) を replay:

| 指標 | 目標 | 実測 | 参考 (TS 版 / claude-mem) |
| --- | --- | --- | --- |
| hook 1 回 (プロセス起動込み) | ≤ 20 ms | p50 7 ms / p95 8 ms / max 9 ms | TS 161 ms p50 |
| hook 1 回 (プロセス内) | — | p50 79 µs / max 230 µs | — |
| observe の最大常駐 (VmHWM) | < 50 MB | 13 MB | TS 106〜163 MB / claude-mem worker 120 MB + Chroma 4.65 GB |
| 要約成功 | 全セッション | 12/12、観測 61 件、生イベント残 0 | — |
| フォールバック | 実証 | Groq 失敗 4 回 (429 ×3、400 schema ×1) → 全部 agy が救済 (平均 23 s) | — |
| 冒頭注入 | 動く | 3,864 字、`hookSpecificOutput.additionalContext` で出力 | — |
| DB サイズ | — | 124 KB | — |

学び: 中身の無いセッション (同じプロンプトの繰り返し) からも観測が 1 件出た → M1 のプロンプトで「覚える価値が無ければ observations は空」を明示する。agy は英語セッションでも日本語で書くことがある → 出力言語をプロンプトで固定する。Groq の strict schema は稀に 400 を返す → 連鎖で吸収できているが、`additionalProperties:false` の付け方を M1 で見直す。

## 手順(この repo 用、軽量版)

- 仕様 = この文書 1 枚 + マイルストーンごとの checklist。Spec Kit の 48 タスク儀式は使わない。
- 実装 = Claude Code が spike と芯を直接書く(品質と速度で有利)。部品移植・adapter など並列可能なものは Codex / Grok。
- レビュー = PR ごと Codex 1 巡 + 修正後 1 巡。CI = `cargo fmt --check` / `clippy` / `test`。bot は CodeQL のみ。
- 完了 = 全機能 (M2 / M3) が 3 台 (WSL / Windows / M1 iMac) で動くこと。arm64 VPS は owner が使うと決めたときに同じ条件で足す (提案書 §0.1 決定 10)。OSS 公開は後で別に決める (owner 2026-09-23)。意味検索と同期の設計は `docs/research/search-sync-proposal-2026-09-23.md` で見直し中で、§2b・§3・§8 はそれで置き換える。

## owner に聞くこと

1. 新 repo 名 `oboete`(GitHub 作成)、`free-mem` は archive でよいか。
2. 無料キーの取得(Groq / NIM / Mistral / OpenRouter)は owner 作業。OpenRouter に $10 一度だけ入れて 1,000 回/日にするか(任意)。
3. spike の頭脳: agy と claude のどちらを先に試すか(agy = 腐っている枠、claude = 品質)。
