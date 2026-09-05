# Codex向け: Agent Memory Continuity Platform 実装前設計レビュー依頼

## 入力

最優先で次の仕様書を全文読んでください。

- `agent-memory-final-spec-v6.md`（2026-08-12統合版。これが唯一の正本）

`agent-memory-final-spec-v5.md`と`agent-memory-implementation-spec.md`はv6へ統合済みのアーカイブであり、**読まないでください**（旧版を根拠にした指摘は無効とします）。

この依頼は**実装ではなく、実装前の反証的設計レビュー**です。コード変更、issue作成、PR作成は行わないでください。

## 目的

以下の理想を、この仕様が実際に満たせるか検証してください。

> Claude Code、Codex CLI、OpenCode、Pi、Kimi Codeが同じ永続記憶を共有し、session終了・compact・Agent切り替え・クラッシュ後でも作業を再開できる。CMEMのような月額契約を必須にせず、要約・observation抽出・memory統合・rerank・embeddingのprovider/modelを柔軟に変更でき、claude-mem相当以上の品質を実測で保証する。

## 必須調査対象

READMEだけで判断せず、current source、tests、release、official docsを確認してください。

### Repositories

- `kunickiaj/codemem`
- `thedotmack/claude-mem`
- `akitaonrails/ai-memory`
- `openai/codex`
- Pi official repository

必要に応じて:

- `Gentleman-Programming/engram`
- `majiayu000/remem`
- `rohitg00/agentmemory`
- `asg017/sqlite-vec`

### Official documentation

- Claude Code hooks / headless mode
- Codex CLI hooks / exec JSON output
- OpenCode plugin and compaction APIs
- Kimi Code hooks/plugins
- Pi extension lifecycle
- Cloudflare Workers AI
- Cloudflare SQLite Durable Objects
- MCP Streamable HTTP
- SQLite FTS5 / sqlite-vec

## レビュー原則

1. 仕様書を擁護しない。壊すつもりで検証する。
2. source/testsがREADMEと異なる場合はsource/testsを優先する。
3. 2026-08-12以降に変わり得る仕様はcurrent source/docsで確認する。
4. 「対応している」と「実用上Tier Aで安定している」を分ける。
5. 推測は`inference`、未確認は`unknown`と明記する。
6. 問題を指摘するだけでなく、最小の修正案を示す。
7. greenfieldを提案する場合は、codemem forkでは解決できない理由をfile/module単位で立証する。
8. implementation scopeを不用意に増やさない。

## 最重要レビュー項目

### A. Base selection

- codemem forkは妥当か。
- direct DB fallback、observer auth、sharing/coordinatorを安全に分離できるか。
- fork維持コストがgreenfieldより大きくならないか。
- unsafeなprivate backend/token extraction pathが残らないか。

### B. Five-Agent functional parity

前提: v6はrelease段階制（Core 1.0 = Claude Code + Codex、OpenCode/Pi/Kimiは1.x、25-pathはPlatform 1.0 gate）を採用済みです。段階制の存在自体を欠陥として指摘するのではなく、この段階制の下でcontractが成立するかを検証してください。

各Agentについて次を`native / synthesized / unsupported / unknown`で表にしてください。

- session start
- user prompt capture
- assistant completion
- tool success/failure
- turn completion
- pre/post compact
- session end/interrupt
- query-aware injection
- checkpoint injection
- subagent capture
- stable session ID

Claude→Codex等、5×5の引き継ぎが本当に実装可能か判断してください。

### C. Continuity correctness

- SessionWorkStateとContinuationCheckpointの分離は妥当か。
- mechanical stateだけでprovider停止中にも実用的resumeが可能か。
- PreCompactでDB/spoolへ保存するhot pathにdeadlock/latency riskはないか。
- delivered/accepted/superseded state machineにraceはないか。
- session crash/abandoned判定に誤検知はないか。
- session lineageが不足/過剰ではないか。

### D. Event pipeline

- idempotency key設計
- parallel tool events
- out-of-order / late event
- stale batch correction
- duplicate delivery
- injected-memory self-ingestion prevention
- memory MCP resultの再capture

についてfailure modeを列挙してください。

### E. Project identity

- canonical remote
- root commit/fingerprint
- no-remote repository
- forked repository
- renamed remote
- monorepo
- worktree
- Windows/WSL path mapping

でcollision/leakが起きないか検証してください。

### F. Generation providers

- role-based provider/model設定は妥当か。
- Claude/Codex official sidecarだけで安全なstructured outputを得られるか。
- recursion/plugin re-entryを完全に防げるか。
- provider fallbackとprivacy/billing policyに抜けがないか。
- Cloudflare Workers AIをcustom base URLとして使う際の互換性gapは何か。

### G. Embedding / retrieval

- EmbeddingProvider contract
- sqlite-vec packaging
- generation build/atomic switch
- dual FTS unicode61/trigram
- RRF
- JP/EN/mixed query
- cloud FTS parity

をレビューしてください。

### H. Sync protocol

- immutable revision
- parent revision
- concurrent conflict
- tombstone race
- op canonicalization
- decimal sequence
- snapshot/bootstrap/compaction
- checkpoint sync
- device revoke/replay protection

に欠陥がないか確認してください。

### I. Reliability / security

- daemon sole writer
- atomic spool
- total disk failure
- backup/restore
- migration rollback
- sidecar command injection
- prompt injection
- cross-project leak
- credential storage
- supply chain
- remote MCP auth

をthreat modelとして検証してください。

### J. Evaluation

- 120-session corpusで十分か。
- public/dev/hidden splitは妥当か。
- baseline non-inferiority基準は妥当か。
- CMEM比較を再現可能にする方法はあるか。
- continuation successの判定が曖昧でないか。
- benchmark gaming/judge biasを防げるか。

## 必須出力形式

### 1. Executive verdict

```text
Decision:
  proceed | proceed-with-blockers | redesign | adopt-existing | stop

Recommended base:
  codemem-fork | another-OSS | greenfield

Confidence:
  high | medium | low
```

### 2. Hard Invariant matrix

仕様書の全Hard Invariantを次で評価してください。

```text
pass | risk | fail | unknown
```

各行にsource/evidenceを付けてください。

### 3. Blocking findings

各findingを以下の形式で書いてください。

```text
ID:
Severity: blocker | high | medium | low
Spec section:
Evidence:
Failure scenario:
Why current mitigation is insufficient:
Minimal spec change:
Implementation impact:
Verification test:
```

### 4. Agent capability matrix

5 Agentのfunctional parity表。

### 5. Architecture simplifications

価値を落とさず削れるものだけ提案してください。

### 6. Missing requirements

仕様に存在しないが必須な要件。

### 7. ADR changes

変更すべきADRと推奨default。

### 8. Corrected implementation order

PR/Phaseの依存関係をレビューし、必要なら修正してください。

### 9. Final delta

`agent-memory-final-spec-v6.md`へ反映すべき文言を、section単位のpatch案として提示してください。

## 禁止事項

- 根拠なしに「複雑すぎる」とだけ結論すること
- 未確認のCLI capabilityを断定すること
- undocumented/private auth backendを解決策として提案すること
- Cloudflareを必須providerにすること
- syncとobserverを結合すること
- benchmarkなしにmodelを選定すること
- 実装を開始すること
