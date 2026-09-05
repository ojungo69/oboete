# Research: cmem pro パリティ目標の一次ソース調査

- Date: 2026-09-02
- Status: informational (north star 裏付け。決定は別途 ADR / issue で行う)
- Context: [#136 north star 追記コメント](https://github.com/ojungo69/free-mem/issues/136) — free-mem は claude-mem 有料化 (cmem pro) 後も無料で同等機能を使い続けるための OSS。対象 agent は Claude Code / Codex / Grok Build / Pi。cmem pro 同等機能は Cloudflare サービス (または優れた代替) での再現を目指す。
- Method: 一次ソース (公式ドキュメント・GitHub API・npm registry・ローカルインストール実体) の直接取得。未確認事項は明示。

## 1. cmem pro が課金で囲っているもの

### 製品名と価格 (ページ間で矛盾あり — 要注意)

| ティア | 価格 | ソース |
|---|---|---|
| claude-mem (OSS エンジン) | 無料 / Apache-2.0 | <https://cmem.ai> (Pricing セクション) |
| CMEM Pro | $30/mo | <https://cmem.ai/pro>, <https://cmem.ai/pricing> |
| CMEM Cloud (同一物と思われる) | $20/month · cancel anytime | <https://cmem.ai> トップページ |
| Team Cloud | $333/seat/month (3〜50 seats、FDE 込み) | <https://cmem.ai/pricing> |

⚠️ $20 と $30 は同日取得の別ページの値。どちらが現行かは断定できない (トップの marketing copy が古い可能性が高いが未確認)。比較資料では両方併記のこと。

### 有料機能の実体 (<https://cmem.ai/pro> の 4 項目)

1. **Cloud sync** — "Your DB, mirrored to the cloud"
2. **Private MCP link** — "One endpoint, every agent"
3. **Hybrid search** — "Full-text + recency recall, fast"
4. **Dreaming** — "Live observation generation"

### 最重要の課金ゲート: observer を回す LLM の負担者

README (`github.com/thedotmack/claude-mem` README.md L139):

> "Signing in provisions a memory key for your account and unlocks the claude-mem observer: memory that runs off-plan, free for your first 30 days, so you get up to 100% more usage from your plan. When the free trial ends, memory automatically falls back to your Anthropic plan unless you subscribe. After sign-in you pick your memory protocol — the claude-mem observer, your own OpenRouter or Gemini key, or your Anthropic plan."

**Pro の本体は「要約を生成する LLM を誰が払うか」**。無料では自分の Anthropic プラン (または自前 OpenRouter / Gemini キー) を消費し、Pro ではベンダー側 observer が off-plan で回る。裏付け: release note v13.21.0 (`--provider claude` は "configures memory against your own Anthropic plan and never contacts cmem.ai")、v13.16.1 (Cowork: "Pro runs the observer server-side")。トライアルは 30 日 (v13.18.0, v13.21.1)。決済は Stripe Checkout。

### SyncHub のアーキテクチャ (ローカルインストール `13.21.2/skills/cloud-sync/SKILL.md` より)

- 接続情報は 3 つ: `CLAUDE_MEM_CLOUD_SYNC_TOKEN` / `CLAUDE_MEM_CLOUD_SYNC_USER_ID` / `CLAUDE_MEM_CLOUD_SYNC_HUB_URL` (`~/.claude-mem/settings.json`、mode 0600)
- "one client, one durable operation log, and no separate sync daemon" — worker が write 時に同期
- デバイス識別: worker 初回起動時に deviceId 採番、device name はホスト名既定
- Hub API: 認証付き `GET /v1/sync/status`、head / checkpoint、pending counts、`lastFlushAt`、`lastError`
- プライバシー注記 (原文): "Cloud sync uploads your observation narratives and full prompt text to your cmem.ai account."
- 既知の制限: "setup does not migrate a pre-launch local corpus"

### 無料ティアに残るもの (`docs/ip-boundary.md` = open-core 境界の正本)

Apache-2.0 側: Core memory engine / Claude-Mem Server / CLI / SDKs / REST API schemas / MCP tools, resources, prompts / Claude Code adapter / Generic agent adapters / Storage adapters / Reference knowledge agents / Tests / Examples / Public documentation。

予約された商用領域: Magic Recall hosted cloud, Team/org memory sync, Admin dashboard, SSO/SAML/SCIM, Enterprise RBAC, Enterprise audit log UI, DLP/policy engine, Premium knowledge agents, Managed evals, Customer deployment tooling, Enterprise observability, Support/SLA workflows, Internal eval datasets, Private customer connectors。

**読み取り**: 囲われているのは「ホスト型クラウド」「チーム/組織同期」「エンタープライズ統制」の 3 系統。free-mem が狙う個人向け機能はほぼ全て OSS 側に存在する。

### ライセンス: AGPL-3.0 → Apache-2.0 に緩和済み (コミット単位で裏付け)

| 時期 | ライセンス | 根拠 (`gh api` 実取得) |
|---|---|---|
| 2025-09-06 | 独自 "Claude Mem License" | `repos/thedotmack/claude-mem/contents/LICENSE?ref=598369e8` |
| (中間) | AGPL-3.0 | `36b0929f` の親 `0a43ab76` の LICENSE 実体 |
| 2026-05-08 (`36b0929f`) | Apache-2.0 | LICENSE patch −630 (AGPL) / +202 (Apache)。コミット: "Server-beta: Postgres storage + independent runtime + BullMQ queue (Phases 1–3) (#2351)" |
| 現在 | Apache-2.0 | `npm view claude-mem license` / `license.spdx_id` |

有料化 (Server-beta) と同一コミットで AGPL→Apache へ**緩和**。`docs/license.md`: 制約は商標のみ ("Apache-2.0 licenses code. It does not grant rights to third-party trademarks or brand names")。**free-mem にとって OSS コアの参照・借用・fork にコピーレフトの障壁はない** (2026-05 以前の AGPL 期より条件が良い)。ただし Apache-2.0 の条件はそのまま残る: 借用・fork・再配布のときは LICENSE と NOTICE の同梱、改変の明示、出所 (repo・commit・license) の記録と `THIRD_PARTY_NOTICES.md` の更新 (`legacy/evidence/adr-004-licensing.md` の運用ルール)。「障壁なし」は義務なしの意味ではない。

## 2. Grok Build CLI と Pi のフック面

### Grok Build CLI — 統合が最も容易

xAI 公式 Rust バイナリ。実測 `grok 1.0.16 (a0239a2688c1) [alpha]`。npm の `@vibe-kit/grok-cli` / `grok-cli` は無関係な第三者パッケージ (取り違え注意)。

**フック**: JSON 宣言型、Claude Code 互換。`~/.grok/docs/user-guide/10-hooks.md` "Hook Events" 表 (L89-105) の全 15 イベント: `SessionStart` / `UserPromptSubmit` (block可) / `PreToolUse` (deny可) / `PostToolUse` (stdout で tool 出力置換可) / `PostToolUseFailure` / `PermissionDenied` / `Stop` (block可) / `StopFailure` / `StopCancelled` / `Notification` / `SubagentStart` / `SubagentStop` (block可) / `PreCompact` / `PostCompact` / `SessionEnd`。

- 設定: `~/.grok/hooks/*.json` (グローバル)、`<project>/.grok/hooks/*.json` (要 `/hooks-trust`)。HTTP フック対応 (`url`)
- **互換層**: `~/.claude/settings.json` と `~/.cursor/hooks.json` をネイティブ読込 (同 doc L115-130 にマッピング表)
- MCP: `~/.grok/config.toml` `[mcp_servers.<name>]`、stdio / HTTP / SSE。`.claude.json` / `.cursor/mcp.json` / `.mcp.json` 自動取込 (`07-mcp-servers.md`)
- コンテキスト注入: `AGENTS.md` / `CLAUDE.md` / `CLAUDE.local.md` + `~/.grok/rules/*.md` (`12-project-rules.md`)
- プラグイン: `.mcp.json` + `hooks/hooks.json` + `skills/` + `agents/` を `grok plugin install` (`09-plugins.md`)

設計判断に効く 2 点:
1. **Grok にはネイティブのクロスセッションメモリが既にある** (`13-memory.md`、`~/.grok/memory/`、`[memory.initial_injection]`)。free-mem が置換するか共存するかは明示的な決定が必要。
2. **`~/.grok/hooks/claude-mem.json` が実在** — claude-mem は Grok 統合済み。free-mem の直接の参照実装として読める。

### Pi — `earendil-works/pi`

同定: stars 100,385 / MIT / pushed 2026-09-01 (`gh api` 実測)。元 `badlogic/pi-mono`、2026-05 に Earendil Works へ移管。npm 現行 `@earendil-works/pi-coding-agent` v0.84.4 (`@mariozechner/pi-coding-agent` は deprecated)。

- **フック**: TypeScript 拡張の code-based API (`pi.on(event, handler)`)。JSON 宣言型ではない。主要: `session_start` / `before_agent_start` / `turn_start` / `turn_end` / `tool_call` (`{block:true}` 可) / `tool_result` (結果書換可) / `session_before_compact` / `session_shutdown` ほか
- **MCP: first-party 対応なし** — サードパーティ拡張 (`nicobailon/pi-mcp-adapter`) 経由のみ
- コンテキスト注入: `~/.pi/agent/AGENTS.md` + 階層 `AGENTS.md`/`CLAUDE.md` (`AGENTS.override.md` 優先) + Agent Skills (agentskills.io 仕様)
- 拡張: `~/.pi/agent/extensions/*.ts` / `.pi/extensions/*.ts` (要 trust)、`pi.registerTool()` 等

出典: <https://pi.dev/docs/latest/extensions>、`packages/coding-agent/examples/extensions/README.md`、`docs/quickstart.md`、`docs/skills.md`、<https://github.com/nicobailon/pi-mcp-adapter>

⚠️ 検証レベルの差: Grok のイベント表はローカル公式ドキュメントの直接確認。**Pi のイベント一覧は二次取得** (repo identity のみ直接確認) — 実装着手前に `pi.on` のイベント名を一次ソースで再確認すること。

### 統合方針

- **Grok**: `~/.grok/hooks/free-mem.json` 1 枚で Claude Code 版とほぼ同型 (`SessionStart` 注入 / `PostToolUse` 観測 / `Stop` 要約)。`~/.claude/settings.json` 互換層により Claude Code 用フックが設定ゼロで効く可能性 (要検証)。
- **Pi**: TypeScript 拡張の配布が必要。`before_agent_start` (注入) + `tool_result` (観測、書換可で Claude Code の PostToolUse より表現力は上)。MCP first-party 非対応は、MCP サーバとして検索を出す設計には制約。

## 3. Cloudflare へのマッピング

### 有料機能 → Cloudflare プリミティブ

| cmem Pro の機能 | Cloudflare での再現 | 無料枠 |
|---|---|---|
| SyncHub (durable op log / head / checkpoint / per-device) | Durable Objects (SQLite-backed) — ユーザー単位 1 DO | 無料プラン可 (SQLite-backed のみ)。100K req/日、13,000 GB-s/日、5GB、単一 10GB — [pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/) |
| Observations DB のクラウドミラー | D1 | 5M rows read/日、100K rows written/日、5GB、DB 500MB、10 DB — [pricing](https://developers.cloudflare.com/d1/platform/pricing/) |
| バックアップ / 大きい narrative | R2 | 10GB-month、Class A 1M/月、Class B 10M/月、egress 無料 — [pricing](https://developers.cloudflare.com/r2/pricing/) |
| Private MCP link | Workers (remote MCP server) + Access service token / bearer | 100K req/日、CPU 10ms/req、subrequest 50/req — [limits](https://developers.cloudflare.com/workers/platform/limits/) |
| Hybrid search | D1 FTS5 + recency 重み (Vectorize 不要) | D1 枠内 |
| セマンティックのベクトル | Vectorize | 30M queried dims/月、**5M stored dims (全体)** — [limits](https://developers.cloudflare.com/vectorize/platform/limits/) |
| 埋め込み生成 | Workers AI (bge 系 / qwen3-embedding) | 10,000 Neurons/日 — [pricing](https://developers.cloudflare.com/workers-ai/platform/pricing/) |
| ホスト型ビューア | Workers static assets | Workers 枠内 |
| ライブフィード | DO WebSocket hibernation | DO 枠内 |
| 認証 / チーム共有 | Cloudflare Access | 50 seats 無料 (⚠️ 一次ソース未確認、下記) |
| 自宅サーバ公開 | Cloudflare Tunnel | 無料 (Access ポリシー無しなら seat 不要) — [routing-to-tunnel](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/routing-to-tunnel/) |
| (参考) キュー | Queues (2026-02 無料化) | 10,000 ops/日、保持 24h — [changelog](https://developers.cloudflare.com/changelog/post/2026-02-04-queues-free-plan/) |

Workers Paid ($5/月): リクエスト無制限、CPU 30 秒 (最大 5 分)、subrequest 10,000、script 10MB。

### 無料枠が足りない箇所 (6 フラグ)

1. **Vectorize 5M stored dims が唯一の実質ブロッカー** (50k obs × 768d = 38.4M で 7.7 倍超過)。**回避策 (推奨): ベクトルはローカル維持** — claude-mem 無料版も "Semantic vector recall, fully local"。クラウドは op log 中継 + ビューアに絞る。代替: Workers Paid で超過 $0.02/月未満、Turso + sqlite-vec、Supabase pgvector、Neon + pgvector。
2. KV は書き込み 1,000/日 — head/checkpoint 用途に不適。D1 / DO storage へ (100K writes/日)。
3. Workers 無料 CPU 10ms/req — 同期ハンドラでのマージ/dedup/署名検証は静かに落ちる。
4. Queues 無料保持 24h — 1 日以上オフラインの pending が落ちる。キューは D1/DO で持つ。
5. ⚠️ **D1 無料枠は 2026-09-01 からハード強制** (超過はエラー) — [changelog](https://developers.cloudflare.com/changelog/post/2026-09-01-d1-free-tier-limit-enforcement/)。
6. ⚠️ 未確認 2 件: Vectorize の無料プラン適格性は Cloudflare 文書間に矛盾あり (workers/pricing は "paid only" と書きつつ Free の数値表を掲載、vectorize/get-started は "Free or Paid")。Access 50 seats は一次ソース未確認 (コミュニティ裏取りのみ)。

## まとめ (設計に効く 3 点)

1. **課金の本丸は LLM コストであって同期基盤ではない。** free-mem が observer を無料枠プロバイダ (Workers AI 等) と、同意ゲート付きの agent CLI レーンで回す設計なら、Pro の主要訴求は最初から無効化される。agent CLI レーンは M1 では Grok Build のみで、ユーザーが setup で明示的に選んだときだけ使う。Claude Code / Codex のサブスク資格情報を黙って流用することは仕様で禁止 (`legacy/specs/005-product-reset/spec.md` の Alpha 制約、`007-oboete-m1-alpha` の consent ゲート) であり、この結論はそれを前提にしない。同期・MCP link・hybrid search は Cloudflare 無料枠でほぼ再現可能。
2. **ベクトル検索はローカルに残す。** Vectorize 5M dims 制限の最短回避で、claude-mem 無料版と同じ構え。
3. **コピーレフト障壁なし。** Apache-2.0 (AGPL から緩和済み)。商標 (名前に "claude-mem" / "cmem" を使わない) と Apache-2.0 の同梱・記録義務を守れば借用・fork できる。
