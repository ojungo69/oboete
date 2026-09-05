# Agent Memory Continuity Platform v6.0 実装前・反証的設計レビュー

レビュー日: 2026-08-12（Asia/Tokyo）
正本: `agent-memory-final-spec-v6.md` のみ
性質: read-only の実装前レビュー。コード、issue、PR は作成していない。

## 調査範囲と証拠の扱い

README の機能表だけではなく、2026-08-12 時点の release、固定 commit の source、tests、公式 docs を照合した。主な固定点は以下である。

- `kunickiaj/codemem`: commit [`26438e75`](https://github.com/kunickiaj/codemem/commit/26438e75ce1d0fec6be34981f15045a15c89658b)、release [`v0.40.2`](https://github.com/kunickiaj/codemem/releases/tag/v0.40.2)
- `thedotmack/claude-mem`: release [`v13.15.0`](https://github.com/thedotmack/claude-mem/releases/tag/v13.15.0)、commit `f792a27eaf0f7c3906592c97c6484d5097b4798f`
- `akitaonrails/ai-memory`: commit [`a9e9a24d`](https://github.com/akitaonrails/ai-memory/commit/a9e9a24d50f59e970fc01ae48efe647abf20702e)、tree version `v1.26.0`
- `openai/codex`: stable [`0.147.0`](https://github.com/openai/codex/releases/tag/rust-v0.147.0)、current source commit [`ca4d532b`](https://github.com/openai/codex/commit/ca4d532b2a5803159bfa8c8f56213948e068b62f)。`main` は release より先行し得るため、source 上の存在と stable binary の保証を分けた。
- Claude Code: official release [`v2.1.228`](https://github.com/anthropics/claude-code/releases/tag/v2.1.228)。CLI 本体 source/tests は公開 tree で確認できないため、未公開内部挙動は公式 hook/headless 契約より強く断定しない。
- OpenCode: release [`v1.18.16`](https://github.com/anomalyco/opencode/releases/tag/v1.18.16)、commit `a3647eb025c7615159d417dcc49fc39fdaeba65b`
- Pi: official `earendil-works/pi` [`v0.84.1`](https://github.com/earendil-works/pi/releases/tag/v0.84.1)。`can1357/oh-my-pi` は別 fork なので根拠に混ぜていない。
- Kimi Code: official `MoonshotAI/kimi-code` [`v0.34.0`](https://github.com/MoonshotAI/kimi-code/releases/tag/v0.34.0)
- SQLite: [`3.53.4`](https://www.sqlite.org/changes.html)。`sqlite-vec`: stable [`v0.1.9`](https://github.com/asg017/sqlite-vec/releases/tag/v0.1.9)、最新は prerelease `v0.1.10-alpha.4` なので stable と alpha を混ぜていない。
- Cloudflare、MCP、Git、SQLite、評価方法は current official docs/RFC を使用した。

判定語は次の意味で使う。

- `pass`: v6 の規範文だけで要件を一貫して実装・検証できる。
- `risk`: 方針は成立するが、契約または release gate が不足している。
- `fail`: v6 内部の別規定、current source、または公式契約により反証される。
- `unknown`: 一次情報で確認できず、実機 probe 前に断定できない。
- `inference`: source/docs からの推論であり、公開互換契約ではない。

本レビューで残した主要な `unknown` は、各repositoryの固定commitでのfull test green、Codex/Claude sidecarのhostile environmentにおける実効tool集合、Cloudflare SQLite DO上のtrigram tokenizer実動、採用予定Node bindingのOnline Backup API露出、各candidateの実測移植量/latencyである。source/testsの現物は確認したが、依頼が設計レビューかつ出力ファイル以外を作らない制約のためclone/install/live E2Eは行っていない。hook retryなし、sidecar isolation不能、fork改修量など公開契約でない結論は本文で `inference` またはrelease前E2E対象として限定する。

---

## 1. Executive verdict

```text
Decision:
  proceed-with-blockers

Recommended base:
  codemem-fork

Confidence:
  medium
```

ただし `codemem-fork` は「通常の upstream 追随 fork を即確定する」という意味ではない。推奨するのは `26438e75...` の vendor snapshot を比較仮説として固定し、Phase 0 の sole-writer vertical spike 後に継続可否を確定する方式である。現在の codemem では direct DB write が例外 fallback ではなく、Claude/Codex hook、inject、file-context、OpenCode、MCP、CLI/admin、sync に横断している。代表例は [`claude-hook-ingest.ts`](https://github.com/kunickiaj/codemem/blob/26438e75ce1d0fec6be34981f15045a15c89658b/packages/cli/src/commands/claude-hook-ingest.ts)、[`codex-hook-ingest.ts`](https://github.com/kunickiaj/codemem/blob/26438e75ce1d0fec6be34981f15045a15c89658b/packages/cli/src/commands/codex-hook-ingest.ts)、[`mcp-server/src/tools/items.ts`](https://github.com/kunickiaj/codemem/blob/26438e75ce1d0fec6be34981f15045a15c89658b/packages/mcp-server/src/tools/items.ts) である。したがって「fallback 1 本を削除」ではなく storage ownership API の再境界化になる。

一方、codemem には raw ingest、adapter parser/spool/tests、SQLite/FTS/sqlite-vec、MCP、viewer、eval scaffolding が既にあり、TypeScript/Node を維持するなら greenfield が小さいとは立証できない。`ai-memory` は [`writer.rs`](https://github.com/akitaonrails/ai-memory/blob/a9e9a24d50f59e970fc01ae48efe647abf20702e/crates/ai-memory-store/src/writer.rs) の sole writer と [`hook_spool.rs`](https://github.com/akitaonrails/ai-memory/blob/a9e9a24d50f59e970fc01ae48efe647abf20702e/crates/ai-memory-cli/src/commands/hook_spool.rs) を持つ有力な反証候補だが、Rust、Markdown source-of-truth、非 sqlite-vec storage への仕様変更が必要で、現時点で `another-OSS` を選ぶ証拠まではない。

base候補の反証結果を要約すると次のとおりである。

| 候補 | 再利用価値 | 致命的な差分 | 判定 |
|---|---|---|---|
| codemem `26438e75` | raw ingest、Claude/Codex/OpenCode、spool、SQLite/FTS/sqlite-vec、MCP、viewer、eval | direct DBとsharingがcore横断、private Codex/OpenCode credential経路 | conditional vendor base |
| ai-memory `a9e9a24d` | sole writer、bounded queue、durable hook spool、thin HTTP、cross-harness | Rust、Markdown canonical store、独自vector、[`openai_oauth.rs`](https://github.com/akitaonrails/ai-memory/blob/a9e9a24d50f59e970fc01ae48efe647abf20702e/crates/ai-memory-llm/src/openai_oauth.rs) のprivate backend | Phase 0対抗候補 |
| remem `cde8bc05` | Rust/SQLCipher/sqlite-vec、raw capture、replay/quarantine、広いeval | OpenCode/viewer/strict daemon-onlyが未成立。[manifest](https://github.com/majiayu000/remem/blob/cde8bc05504c74794d044ef118f74d8f828adbf5/Cargo.toml) | components参照、base不採用 |
| engram `509e6762` | Go single binary、FTS5、MCP、OpenCode plugin | agent-selected memory中心でraw event hose/vectorがなく、stdio MCPがdirect writer。[store](https://github.com/Gentleman-Programming/engram/blob/509e6762fdd9417ff7a39d30f426a9566220eaf0/internal/store/store.go) | base不採用 |
| agentmemory `2973e4ec` | TypeScript、adapters、viewer | `iii-sdk` KV/vector依存、SQLite/FTS/sqlite-vecなし。[manifest](https://github.com/rohitg00/agentmemory/blob/2973e4ec4c40d323a08daa34220118010e73a2c3/package.json) | base不採用 |

claude-memはbaseではなく品質/移行baselineとしても注意が要る。v13.15.0の実hookは Setup、SessionStart (`startup|clear|compact`で`resume`なし)、UserPromptSubmit、async PostToolUse、async PreToolUse(Read)、async Stopだけで、PostToolUseFailure、Pre/PostCompact、SessionEnd、Subagent lifecycleを持たない。[`hooks.json`](https://github.com/thedotmack/claude-mem/blob/f792a27eaf0f7c3906592c97c6484d5097b4798f/plugin/hooks/hooks.json) はarchitecture説明より狭い。query-aware injectionはworkerのoptional pathで、server runtimeではunsupported。[`session-init.ts`](https://github.com/thedotmack/claude-mem/blob/f792a27eaf0f7c3906592c97c6484d5097b4798f/src/cli/handlers/session-init.ts) また、配布testはhook JSON artifactを検査するものでreal CLI continuity E2Eではない。[`plugin-distribution.test.ts`](https://github.com/thedotmack/claude-mem/blob/f792a27eaf0f7c3906592c97c6484d5097b4798f/tests/infrastructure/plugin-distribution.test.ts) したがってcurrent claude-memをそのまま「推奨provider品質」の再現可能baselineにせず、固定tagの独立fixture/evalとone-way importerを作る必要がある。

実装着手を止めるべき blocker は、(1) base gate、(2) checkpoint delivery の local/cross-device race、(3) deterministic idempotency、(4) project identity、(5) sidecar の auth/isolation、(6) embedding 世代切替、(7) sync の multi-head/tombstone/revoke、(8) remote MCP auth、(9) release claim の評価統計である。これらは全体 architecture を捨てず、v6 の契約を狭く・明示的にする修正で解消できるため、`redesign` や `stop` ではなく `proceed-with-blockers` とする。

---

## 2. Hard Invariant matrix

| # | Status | Source / evidence | 反証的評価 |
|---:|:---:|---|---|
| 1 | risk | Claude の matching hooks は並列、Pi の `before_agent_start` には公式 timeout 契約がない。[Claude hooks](https://code.claude.com/docs/en/hooks)、[Pi extension types](https://github.com/earendil-works/pi/blob/v0.84.1/packages/coding-agent/src/core/extensions/types.ts) | v6 は `always fail-open` と書くが、native hook 自体が prompt/turn を待たせる面を持つ。agent別 deadline、強制終了、stdout 上限、失敗時の無注入継続を release test にしない限り「主作業を停止させない」は未保証。 |
| 2 | risk | §8.2 は atomic spool を定義する一方、上限後は通常 event を破棄する。codemem の現行 fallback は daemon を迂回する。[codemem ingest](https://github.com/kunickiaj/codemem/blob/26438e75ce1d0fec6be34981f15045a15c89658b/packages/core/src/ingest-pipeline.ts) | 「bounded record を書ける supported 状態」の範囲では成立し得るが、予約枠自体の枯渇、fsync/rename failure、spool importer の commit-before-delete を fault injection で固定していない。 |
| 3 | fail | §8.2 の native ID 不在 fallback は `occurredAt + sourceHash`。retry ごとに時刻が変われば key も変わる。 | `idempotency: approximate` は「処理結果を idempotent」にする代替ではない。adapter 発火時に永続化する stable delivery ID、または volatile field を除外した event fingerprint が必要。 |
| 4 | fail | codemem の hook、inject、file-context、MCP、CLI、OpenCode が write-capable `MemoryStore` を開く。[store](https://github.com/kunickiaj/codemem/blob/26438e75ce1d0fec6be34981f15045a15c89658b/packages/core/src/store.ts)、[MCP server](https://github.com/kunickiaj/codemem/blob/26438e75ce1d0fec6be34981f15045a15c89658b/packages/mcp-server/src/server.ts) | v6 の目標は正しいが、選定済み base と PR 記述は改修範囲を過小評価している。全 process の write-capable handle=daemon のみ、を静的 scan と runtime trace の両方で証明するまで fail。 |
| 5 | pass | §12.1、§12.9、§15.5、§20 は canonical rows と derived index/view を分ける。SQLite Online Backup API も一貫 snapshot を作れる。[SQLite backup](https://www.sqlite.org/backup.html) | durable memory/checkpoint/evidence を canonical row とし、FTS/vector/summary/context pack を rebuild 対象にする方向は一貫している。restore manifest の不足は #32 と finding B-12 で扱う。 |
| 6 | risk | Claude/Codex は checkpoint primitive を持たず hook 上で合成する。Codex compact の正規再注入面は `SessionStart(source=compact)` か次 prompt。[Codex hooks](https://developers.openai.com/codex/hooks) | provider/sync 非依存は成立するが、「SQLite/spool へ保存可能」だけでは注入面の発火・deadline・first-prompt ordering まで保証しない。agent-version別 E2E が必要。 |
| 7 | fail | §9 の detector は既知形式と entropy heuristic。`<private>` の malformed/nested/streamed input、未知 secret、crash 前の echo suppress state は網羅できない。 | 「secret」の定義を detector ruleset が検出した secret に限定しない限り、絶対的な secret leak 0 は実装不能。known-secret fixture 0 と unknown-secret residual risk を分ける必要がある。 |
| 8 | pass | §9.2、§9.4、§13.7 は sensitivity/billing/execution-location を routing の先頭 gate にし、fallback で緩和しない。 | policy lattice を provider health より先に評価する規定は十分。fallback の各 edge を property test することが実装条件。 |
| 9 | pass | §13.7、§13.8 は paid fallback default false、free profile 外への fallback 禁止を明記。 | billing profile を immutable request policy として保持すれば成立する。 |
| 10 | fail | Claude `--bare` は API key を要求し login/keychain auth を使わない。Anthropic は third-party product が Free/Pro/Max credential を代理利用することを認めていない。[Claude legal](https://code.claude.com/docs/en/legal-and-compliance)、[headless](https://code.claude.com/docs/en/headless) | §13.7 の「Claude/Codex sidecar 検出→Zero Incremental Cost」一括既定は誤り。claude-mem は [`EnvManager.ts`](https://github.com/thedotmack/claude-mem/blob/f792a27eaf0f7c3906592c97c6484d5097b4798f/src/shared/EnvManager.ts) で credential store の OAuth を抽出し、codemem には private Codex backend があるため再利用禁止。 |
| 11 | fail | Git worktree は `$GIT_COMMON_DIR` を共有するが clone/fork identity ではない。shallow commit は root として扱われ、remote URL は protocol/rewrite/rename で変わる。[git-worktree](https://git-scm.com/docs/git-worktree.html)、[git-shallow](https://git-scm.com/docs/shallow)、[git-clone](https://git-scm.com/docs/git-clone) | canonical remote + root commit は fork collision、shallow/full drift、renamed remote、URL rewrite を安全に解けない。Git evidence は alias 候補に下げ、明示 project UUID を canonical authority にする必要がある。 |
| 12 | pass | §12.4–12.6 と §16.2 は superseded/retracted/expired/confirmed_wrong を candidate retrieval 前に除外。 | filter-before-ranking は正しい。既に注入済み context の訂正通知は別途不足するが、新規自動注入の禁止自体は規定されている。 |
| 13 | pass | §17.4 の envelope は `historical-evidence`、非 instruction、current source/tests/user request で再検証する文言を固定。 | authority separation は仕様として明確。prompt-injection fixture と MCP/UI rendering の escaping は必要。 |
| 14 | fail | §14.8 の marker/hash だけでは transcript replay で marker が落ちた場合、MCP wrapper、UI export、user paste を同一 provenance と判定できない。 | 「既知の memory-owned surfaces」なら防げるが、現在の絶対表現は過大。source provenance、injection/export ID、capture-time owned-hash set、agent別 stripping を揃えて scope を限定すべき。 |
| 15 | pass | §12 の canonical memory は provider/model から独立し、generation run は provenance。 | provider 変更で再生成品質は変わっても過去 memory row は失われない。 |
| 16 | fail | §15.5 は count/dimension/smoke 後の atomic switch とだけ定義し、build 中に更新された memory revision の catch-up watermark と pointer CAS がない。 | count が一致しても stale revision の vector 世代を active 化できる。immutable build set、input hash、catch-up、single-transaction compare-and-swap が必須。 |
| 17 | pass | §15.5、§16.1、§17、§20.4 は vector off/extension unavailable 時の FTS/checkpoint/MCP を明記。 | vector は派生 acceleration に留まっている。 |
| 18 | pass | §5.1、§22.1、§32.4 は observer と sync を別 failure domain とし、local continuation を cloud に依存させない。 | architecture boundary は妥当。 |
| 19 | fail | §22.8 は concurrent heads を保持するが、`resolve_conflict` は単一 `parentRevisionId` しか持たず全 competing heads を consume できない。checkpoint acceptance は client clock の `max(acceptedAt)` で一つを選ぶ。 | multi-head resolution を原子的に閉じられず、競合 resolution が再競合する。client timestamp は authority でないという §22.6 とも矛盾する。 |
| 20 | risk | §22.11 は verified snapshot を条件にするが、transaction-consistent membership、manifest署名、row count/root hash、tombstone floor、import完了 CAS を定義しない。 | 「verified」の判定が実装者依存。破壊的 compaction を止める gate の入力が不足している。 |
| 21 | pass | §20.1 は sync≠backup、daily/weekly local backup、restore verification を独立要件にする。 | 方針は十分。具体 manifest は #32/B-12 で補う。 |
| 22 | risk | §25 は hook/config の差分管理と uninstall を規定するが、host別 config merge、concurrent installer、partial rollback の exact manifest がない。 | current tool config を壊さないことは実現可能だが、install/update fault matrix と ownership manifest が release gate に必要。 |
| 23 | risk | §7.3 は real CLI E2E を要求する一方、Platform target は5 Agent Tier A。OpenCode true end、Pi/Kimi subagent parity には native limitation がある。 | UI/doctor の honesty rule は正しい。Tier A を単一ラベルにせず capability coverage と approved Tier B exception を機械可読にしないと誤表示しやすい。 |
| 24 | pass | §27 は Track 1 で quality claim を禁止し、CMEM direct comparison がない release では CMEM claim を禁止。 | claim discipline は明確。 |
| 25 | pass | §20.6、§25、§32 は destructive repair/migration 前 backup と dry-run/apply 分離を要求。 | recovery test と migration manifest を満たす限り成立。 |
| 26 | pass | §21.1 は Windows/WSL の同一 DB file 共有を禁止し separate owner を要求。SQLite WAL も network/shared filesystem 利用を制約する。[SQLite WAL](https://www.sqlite.org/wal.html) | platform boundary は正しい。 |
| 27 | pass | §13.2、§14、§19.4 は provider failure/quota を job state として保持し、raw batch を削除しない。 | provider job と event canonical storage を分離している。retry budget/idempotency は B-04/B-11 で補う。 |
| 28 | pass | §10.3/10.4 は canonical observed fields と `SemanticResumeNote` を型分離。 | v6 の重要な改善であり、observer停止時の最低保証を守る。実用性 claim は canonical evidence recovery と semantic next-action quality に分けるべき。 |
| 29 | risk | §12.9 は raw TTL 前の evidence snapshot を precondition とする。 | transactionally「snapshot commit と参照更新が完了するまで raw delete 不可」とする DB constraint/job CAS、private snapshot omission の表現が不足。 |
| 30 | pass | §12.5、§18.5 は confirm/pin 等を user-authoritative UI/CLI に限定。 | retrieved memory だけで権限昇格できない。 |
| 31 | pass | §12.3–12.5 は observer/model の pin 生成と user-confirmed/pinned の単独破壊を禁止。 | authority model は妥当。ただし Candidate schema の `pinned` 出力許可は B-07 で修正する。 |
| 32 | pass | §20、§29/Phase 1、§32 は backup/restore smoke と migration gate を release 条件にする。 | 規範としては明確。binding が Online Backup API を露出しない場合の代替まで実装前に ADR 化する。 |
| 33 | fail | Codex は thread-spawn のみ、Pi は official subagent example が独立 subprocess、Kimi hook は child instance ID を持たない。[Codex runtime](https://github.com/openai/codex/blob/ca4d532b2a5803159bfa8c8f56213948e068b62f/codex-rs/core/src/hook_runtime.rs)、[Kimi hooks](https://www.kimi.com/code/docs/en/kimi-code-cli/customization/hooks.html) | unsupported agent では通常 assistant/tool eventに混ざった子情報を親完了前だと証明できない。識別不能経路は durable auto-promotion を禁止する conservative gate が必要。 |

集計: `pass 17 / risk 7 / fail 9 / unknown 0`。`unknown 0` は不明点がないという意味ではなく、Hard Invariant 自体は current evidence で pass/risk/fail のいずれかへ分類できたという意味である。

---

## 3. Blocking findings

### B-01 — Base selection が gate より先に確定している

```text
ID: B-01
Severity: blocker
Spec section: §4.1–4.4, §29 Phase 0, §31 Local Core
Evidence: codemem current sourceではdirect DB writeがhook ingest/inject/file-context、OpenCode、MCP、CLI/admin、syncに横断する。core ingestもreplicationへ直接依存する。一方raw ingest、FTS/vector、MCP、viewer、testsの再利用資産は大きい。
Failure scenario: fallback一箇所を削ったつもりでMCPやread系CLIのconstructorがDDL/writeを行い、daemonと別processが同じDBを更新する。障害時だけ発火するため通常testでは通り、WAL contentionまたはschema raceで破損・欠落する。
Why current mitigation is insufficient: 「Phase 0 fatal conflict時のみ再審議」は比較結果を出す前にforkをdefault確定しており、write-handle inventoryとsharing dependency切断をexit criteriaにしていない。
Minimal spec change: codememをpinned vendor snapshotの仮説へ下げる。全write-capable handle inventory、4経路vertical spike、sharing import切断、exact test green、ai-memory/remem/greenfield deltaの測定後にbase ADRを確定する。
Implementation impact: Phase 0のみ増える。後続実装の全面手戻りを防ぐ。通常のupstream追随はせず、security/bugfix単位のcherry-pickに限定する。
Verification test: static scanとruntime DB-open traceでdaemon以外のwrite-capable SQLite handleが0。daemon停止時、Claude/Codex ingest、MCP remember、CLI rememberはDBへ触れず同一spoolへ入り、再起動後exactly one commitになる。
```

### B-02 — Agent capability の正規化が native coverage を過大化する

```text
ID: B-02
Severity: blocker
Spec section: §7.1–7.6, §27.9
Evidence: Claude tool failureはpermission/schema/unknown-toolを全て捕捉しない。Codex tool success/failure、assistant/turn completion、query/checkpointは合成でexplicit interruptがない。OpenCode true session endはunsupported。Pi interruptとsubagent、Kimi child instance IDはunsupported/synthesized。
Failure scenario: adapterが欠落eventを「成功したturn/session end」とみなしcheckpointをacceptedまたはsubagent findingをpromotionする。別versionではundocumented hookが変わり、Tier A表示のまま初回注入を取り逃がす。
Why current mitigation is insufficient: `native/synthesized/unsupported`だけではpartial coverage、unknown、evidence version、failure phase、source eventを表せない。Tier Aが一枚ラベルで、session end不能とmain-session handoff成功を分離できない。
Minimal spec change: Capabilityへ`unknown`、`coverage`, `evidenceVersion`, `evidenceKind`, `sourceEvents`, `limitations`を追加。tool failureをexecuted/denied/schema-invalid/unknown-toolに分ける。Tierはcapability profileの集合として表示する。
Implementation impact: adapter schema/doctor/UI/conformance fixturesが増えるが、無理なtranscript scrapingは不要になる。
Verification test: exact stable binaryごとにstartup/resume/clear/compact、success/executed-failure/denied/schema-invalid tool、interrupt/SIGTERM、first-prompt injection、subagent parent mappingをgolden matrix化。未観測cellは自動的にunknown/Tier Bへ落ちる。
```

### B-03 — Checkpoint claim/acceptance は single-deviceでもrace、multi-deviceでは重複注入する

```text
ID: B-03
Severity: blocker
Spec section: §10.1–10.2, §11.2–11.8, §22.4
Evidence: claim DB commit後にhook stdoutが失われると10分間再配送されない。turnが10分を超えるとlease再claim可能。accepted updateにclaim token/revision条件がない。cloudではdeliveredをsyncしないため2 deviceが同じcheckpointを同時claim/injectできる。
Failure scenario: Aが注入後に長いturnを実行中、lease expiry後Bが同じcheckpointを注入する。A/B双方がacceptedをsyncし、client clock maxで一方を「勝者」にしつつ作業は二重に分岐する。duplicate checkpoint injection 0を破る。
Why current mitigation is insufficient: local leaseはdelivery transportとのatomic commitではなく、cross-device authorityでもない。`max(acceptedAt)`はordering authorityでないclient clockを使い、二重実行を解消しない。同一sourceSessionIdからlineage継承する規則は、1 native session内の別タスクも誤統合する。
Minimal spec change: claimにfencing token/revisionを発行しheartbeat延長、acceptはtoken+destination+revision CAS。local profileの保証を「一device内at-most-one active claim」に限定。cloud sync対象checkpointはserver-authoritative claim、または重複配送を許容して別fork lineageとして保持する。明示task boundaryを追加する。
Implementation impact: checkpoint schema、resume ledger、cloud endpoint、UI conflict表示が変わる。observer/syncの独立性は維持できる。
Verification test: response-loss、11分turn、daemon restart、lease reclaim、同時2 local session、2 device partitionをmodel-check/property testし、同一fenceのacceptは最大1、stale tokenは拒否、cross-device duplicateはforkとして可視化される。
```

### B-04 — Event idempotency/order/correction が再現可能な状態機械になっていない

```text
ID: B-04
Severity: blocker
Spec section: §8.2–8.5, §14.8
Evidence: IDなしeventのkeyはoccurredAt依存。parallel toolの「tool batch completion」はnative boundaryがないagentで未定義。late material eventはstale_batchを作るが、既存summary/vector/injectionへの無効化watermarkを定義しない。
Failure scenario: retryでtimestampが変わり同じtool resultが二重抽出される。tool failureがturn finalize後に届き、成功memoryが既に注入済みのままcorrection memoryだけ増える。重複async hookが別batchへ入り二重課金する。
Why current mitigation is insufficient: approximate idempotencyとcorrection jobの作成だけではderived artifactの因果関係を閉じない。unordered setを閉じるterminal条件、late resultのrevision propagation、cache invalidationがない。
Minimal spec change: adapterDeliveryId/sourceEventFingerprintを固定し、volatile fieldをhash対象外にする。turn graphにopened/terminal tool-use setとclose reasonを持たせる。correctionはaffected batch/memory/summary/embedding/context-generationをsupersedeし、次注入でcorrection noticeを出す。
Implementation impact: event/batch ledgerとderived artifact provenanceにwatermark/invalidatedByが増える。LLM job本体は増やさない。
Verification test: duplicate x10、timestamp変更retry、parallel success/failure、terminal欠落、finalize後late mutationを注入し、canonical event一件、派生revision一系列、stale contextの次turn訂正をassertする。
```

### B-05 — Project identity は自動同一視と自動分離の両方で誤る

```text
ID: B-05
Severity: blocker
Spec section: §6, §16.2, §17
Evidence: remote URLはHTTPS/SSH/scp/url-rewriteで複数表現、forkはroot commitを共有、shallow rootは真のrootでない。Git common-dirはlinked worktreeのlocal groupingであってcross-device project identityではない。git-replace/graftもtraversed rootを変え得る。
Failure scenario: upstreamとforkが同じroot fingerprintでmergeされ、秘密memoryが別forkへ注入される。逆にremote rename/protocol変更で同一projectが分裂する。no-remote UUIDは別deviceに伝播せず自動handoffが失敗する。
Why current mitigation is insufficient: 優先順位はcollision時のfail-closed規則、evidence conflict、explicit alias approval、shallow/fork状態を表現しない。
Minimal spec change: opaque project UUIDをcanonical authorityにし、Git remote/root/common-dirはlink candidate evidenceに下げる。自動merge禁止。identity stateをverified/provisional/conflictedとし、scopeにもstable UUIDを持たせる。
Implementation impact: identity schemaとlink UI/CLIが単純化される。host path mappingはproject verification後だけ適用する。
Verification test: protocol違いremote、renamed remote、fork、shallow/full、worktree、repo copy、no-remote二device、monorepo rename、Windows/WSL mappingでcross-project auto injection 0、明示link後だけ共有を確認。
```

### B-06 — Official sidecar transport と安全な observer sandbox を同一視している

```text
ID: B-06
Severity: blocker
Spec section: §13.5–13.8, §26, §27.4
Evidence: Claude --bareはAPI key前提でsubscription loginを使わない。Codex exec/JSON/output-schemaはStableだが、documented all-tools-off、CLI timeout、process-tree cleanup、managed hooks/pluginsを必ず無効化する単一契約がない。codemem/claude-memにはprivate backendまたはcredential extraction pathがある。
Failure scenario: observer sidecarがmemory plugin/hookを再起動し自己取込みする、hosted tool/MCPを呼ぶ、子processを残す、またはsubscription credentialを禁止経路で転送する。structured final JSONがschema-validでもtool side effectが発生済みになる。
Why current mitigation is insufficient: env markerとneutral cwdはmanaged/global configやunknown tool surfaceの無効化を証明しない。JSONLにhook eventが出ないこともhook無効の証拠ではない。
Minimal spec change: Claude subscription sidecarを削除しClaudeはAPI-key/BYOK bareのみ。Codex sidecarはconditional/disabled defaultとし、external supervisor、effective tool/config inspection、hostile fixture、tree-kill E2Eを通ったexact versionだけ認定する。auto順からsidecarを外す。
Implementation impact: Universal Freeはlocal runtimeまたはCloudflare candidateで満たす。sidecarは後から追加でき、provider interfaceは変わらない。
Verification test: hostile hooks/plugins/MCP/apps/AGENTS/web fixture、managed hook fixture、hanging descendant/FD fixture、SIGINT/timeout、invalid/truncated JSONをUnix/Windowsで実行。side effect/PID/FD 0かつclient-side schema validation合格時のみenable。
```

### B-07 — Privacy/self-ingestion/schema authority に内部矛盾がある

```text
ID: B-07
Severity: blocker
Spec section: §9, §12.3–12.7, §14.4–14.8, §18.5
Evidence: DurableMemory.origin enumにagent_explicitがないのにmemory_recordが使用する。rankingはlegacy_unknownを参照するがtruthState enumにない。model candidateがpinnedを返せる形だと「modelはpin不可」に反する。markerのみではMCP/UI exportの再captureを一意に識別できない。
Failure scenario: validatorごとに未知enumのdrop/coerceが分かれ、user authorityが失われる。UI exportのmemoryが再度extractedされ、自分の要約を根拠にconfidenceが上がる。未知secretがruleset外で保存される。
Why current mitigation is insufficient: prose上の禁止がwire/schemaで拒否されない。echo suppress n-gramがephemeralならcrash後に防げず、任意pasteとの区別もつかない。
Minimal spec change: originへagent_explicit、provenanceQualityへlegacy_unknownを追加しtruthと分離。model Candidateからpinned/user_confirmedを除外。owned context/export/MCP resultへstable provenance IDを付けcapture前にstrip。secret 0 claimをversioned detector fixtureへ限定する。
Implementation impact: schema migration前のため低コスト。validatorとUI labelを一つに統一できる。
Verification test: enum exhaustiveness、model pin拒否、nested/malformed private tag、process crash後echo、transcript marker stripping、MCP wrapper、UI export/reimport、user手動paste、known-secret corpusを実行。
```

### B-08 — Embedding/retrieval contract はpartial failureと世代整合性を表現できない

```text
ID: B-08
Severity: blocker
Spec section: §15, §16, §19, §20
Evidence: embed(texts): number[][] はitem ID、partial error、cancel unknown、model revision、preprocess/input hashを持たない。sqlite-vecはpre-v1。FTS trigram MATCHは3文字未満に一致せず、unicode61は日本語形態素解析器ではない。RRFのk/cap/tie-break未定義。
Failure scenario: batchのout-of-order/partial responseを別memoryへ保存する。build中更新を取り逃したstale generationをcount一致でactive化する。2文字CJKが常に0件、同点順がrunごとに変わる。
Why current mitigation is insufficient: downstream dimension checkはitem対応・NaN/Inf・zero norm・alias driftを防がない。old generation維持だけではnew generationの完全性を証明しない。
Minimal spec change: item-addressable embedding request/result、immutable model/preprocessor revision、per-item ledger、finite/dimension/norm validation、immutable build set+catch-up+CAS。sqlite-vec stable artifact/hash/platform pin。lexical schema/query routing/RRF式を固定する。
Implementation impact: vector pathは増えるがoptionalのまま。CoreはFTS-firstでreleaseできる。LanceDBは削除する。
Verification test: out-of-order partial batch、cancel/retry、NaN/Inf/zero/wrong-D、concurrent memory update/switch、artifact mismatch、京都/京都駅/resume/résumé/path/mixed query、RRF missing stream/tieを固定fixtureで検証。
```

### B-09 — Sync operation はmulti-head resolution、offline resurrection、revoke replayを閉じない

```text
ID: B-09
Severity: blocker
Spec section: §22.4–22.12
Evidence: SyncOperationにsignature/keyId/credential epochがなく、resolve_conflictは単一parent。tombstone retentionは「compaction windowより長い」だけで、より長くofflineのdeviceを防げない。snapshot completeness/signature/transaction watermarkが未定義。
Failure scenario: 3 heads中1 headだけをparentにresolutionし、残りが再びcurrent headになる。長期offline deviceがtombstone消去後にold entityを新opとして再作成する。revoke前に署名済みqueueをrevoke後pushする。partial snapshotをhashだけ合わせてimportする。
Why current mitigation is insufficient: globally unique opIdとJCSはauthorization/replay protectionではない。RFC 8785はI-JSON canonicalizationを定義するがsemantic schemaやUnicode normalizationを代替しない。[RFC 8785](https://datatracker.ietf.org/doc/html/rfc8785)
Minimal spec change: operationへkeyId/signature/credentialEpoch、strict per-device sequence rule。resolutionはsorted parentRevisionIds全head+head-set CAS。snapshotはsigned manifest/root hash/count/epoch/seq/tombstone floor。古いbase epochのmutationをrejectし、offline復帰はfull rebootstrap。
Implementation impact: cloud schema/APIを実装前に変更するだけで済む。observerとは結合しない。
Verification test: 3-way conflict、concurrent resolutions、tombstone/update、compaction後year-offline復帰、revoked queued op、sequence replay/gap、partial/page-mixed snapshot、PITR epoch resetをstate-machine/property testする。
```

### B-10 — Remote MCP の Bearer-only 契約は current protocol/security 要件を満たさない

```text
ID: B-10
Severity: blocker
Spec section: §18.8–18.9, §22.10, §26
Evidence: MCP 2026-07-28 Streamable HTTPはPOST-onlyでOrigin検証がMUST。MCP-Protocol-Version、Mcp-Method、該当時Mcp-Nameのheader/body整合が必要。OAuth対象ではProtected Resource Metadata、resource indicator/audience、scope、PKCE等が必要。
Failure scenario: DNS rebinding/悪意Originからlocalhost/cloud MCPへrequest、別resource用tokenのconfused deputy、失効token再利用、header/body smugglingが起きる。
Why current mitigation is insufficient: 「coding agents: Bearer token」とgeneric hosted clientの将来OAuthだけでは、token expiry/revoke/audience/rate limitや現在必須のOrigin/header validationを定義しない。
Minimal spec change: fixed first-party client bearer profileとgeneric OAuth profileを分離。前者にもHTTPS、Origin allowlist、audience/scope、short expiry/revoke、rate limitを要求。protocol version negotiation/header validationを明記する。
Implementation impact: local stdioは変わらない。Personal Cloud remote MCP実装前のAPI contract修正のみ。
Verification test: bad/missing Origin、header/body mismatch、unsupported version、expired/revoked/wrong-audience/wrong-scope token、OAuth resource mismatchを拒否し、sync tokenをMCPで使えないことを確認。
```

### B-11 — Free certification のquota/compatibility前提が代表traceを拘束していない

```text
ID: B-11
Severity: high
Spec section: §13.4, §13.8, §14.2–14.4, §22.13, §27.11
Evidence: Workers AI freeはcurrent 10,000 Neurons/day。例示8B FP8-fastなら80 requestは概算枠内だが、10–20秒idle flushで200 turnsが200 requestsになれば概算超過し得る。Chat/Embeddings/Responses/JSON modeの互換性はmodelごとに一律でない。
Failure scenario: certification traceだけbatch密度が高く、実利用の疎なturnで日次quotaを超える。repair/retryを80 cap外に数える。model alias/ID変更後も期限内manifestを信じてschema errorを連発する。
Why current mitigation is insufficient: 80 requestsは期待値でhard budgetではない。model ID、response_format、usage/idempotencyをcapability probeで固定していない。
Minimal spec change: retry/repairを含むper-day hard budget gate、疎/密両trace、endpoint/format/streaming/usage/idempotencyのprobe manifestを要求。provider値はruntime manifest、business logicへhard-codeしない。
Implementation impact: batching queueとcertification harnessの変更。Cloudflareを必須にしない原則は維持。
Verification test: 200-turn dense/20秒超idle sparse traceでrequest/Neuronを測定しhard cap内を確認。各candidateでChat/Responses/Embeddings/json_object/json_schema/schema failure/streaming/error shapeを実API probeする。
```

### B-12 — Backup/restore はcanonical DBとderived artifactsの互換manifestがない

```text
ID: B-12
Severity: high
Spec section: §19–§21, §25, §32
Evidence: SQLite Online Backupはsnapshotを作れるが、WAL copyは単純file copyで扱えない。sqlite-vecはnative ABI/artifact差があり、採用Node bindingがOnline Backup APIを露出するか未確定。
Failure scenario: DBはrestoreできてもFTS tokenizer configまたはactive vector generationが別versionで、検索が空/誤順位になる。extension不在でdaemon全体が起動せずcheckpointまで読めない。
Why current mitigation is insufficient: daily/weekly世代数とrestore smokeだけでは、schema/SQLite/FTS/vector artifact/checksum/active generationの対応を判定できない。
Minimal spec change: signed/hashed backup manifestへschema、SQLite source version、FTS config、vector artifact/hash/platform、active generation、canonical table checksumsを保存。fresh dirへcanonical restore後、derived indexをrebuild+atomic replaceする。
Implementation impact: backup formatとdoctorを先に確定。extension unavailableでもcheckpoint+FTS-only restoreを成功させる。
Verification test: active WAL writer中backup、fresh directory、extension absent、old/new sqlite-vec mismatch、途中crash、checkpoint/FTS/vector checksums、migration rollbackを実行する。
```

### B-13 — Evaluation は非劣性claimとcontinuation 100%を統計的に定義していない

```text
ID: B-13
Severity: high
Spec section: §27
Evidence: -2%がabsolute/relative、metric aggregation、confidence interval、primary baselineを定義しない。「best baseline」はmetricごとに選ぶとmoving targetになる。120 sessionsのpower根拠、judge blinding/一致度、CMEM nondeterminism/cache条件もない。
Failure scenario: holdoutを繰返し見てpromptを調整し、偶然良いrunだけ採用する。各metricで別baselineを選びclaimを作る。single judgeがprovider/styleを見てcontinuation actionを採点する。
Why current mitigation is insufficient: hidden splitだけでは反復releaseによるholdout消耗、multiple comparisons、judge bias、session内相関を防げない。Track 1のhuman判定を含む100%は再現可能なblocking testでない。
Minimal spec change: deterministic safety/state-machine gateとstatistical quality gateを分離。primary endpoint/baseline/margin、paired unit、CI/bootstrap、power、seed/trials、blind randomized 2-annotator+adjudication+agreement、sequestered holdout refresh policyを事前登録する。
Implementation impact: Track 1はproperty/E2E correctnessだけblockingにでき、scopeは増えない。品質claim時のTrack 2 harnessだけ厳密化する。
Verification test: locked manifestからone-shot holdout run、paired bootstrap CIがmargin内、duplicate/session leakage 0、annotator agreement/adjudication記録、baseline raw artifact/hash/command/provider dateの再実行を確認。
```

---

## 4. Agent capability matrix

### 4.1 Functional parity

値は正規化後の要求能力を評価する。native primitiveが存在しても、要求された境界や因果関係をadapterが追加判定しないと成立しない場合は `synthesized` とした。

| Capability | Claude Code 2.1.228 | Codex 0.147.0 | OpenCode 1.18.16 | Pi 0.84.1 | Kimi 0.34.0 |
|---|---|---|---|---|---|
| session start | native | native | native | native | native |
| user prompt capture | native | native | native | native | native |
| assistant completion | native | synthesized | native | native | synthesized |
| tool success/failure | native | synthesized | native | native | native |
| turn completion | synthesized | synthesized | synthesized | native | synthesized |
| pre-compact | native | native | native | native | native |
| post-compact | native | native | native | native | native |
| session end | native | native | unsupported | native | native |
| session interrupt | unsupported | unsupported | synthesized | synthesized | native |
| query-aware injection | synthesized | synthesized | native | native | native |
| checkpoint injection | synthesized | synthesized | synthesized | native | synthesized |
| subagent capture | synthesized | native | native | unsupported | unsupported |
| stable session ID | native | native | native | native | native |

重要な限定:

- Claude: `PostToolUseFailure` は executed failure の全てですらなく、permission denial/schema validation/unknown tool を同じ `tool_failed` として捕捉できない。Stop はuser interruptで発火しない。[Claude hook reference](https://code.claude.com/docs/en/hooks)
- Codex: `PostToolUse`一種からsuccess/failureを合成し、hosted toolはcoverage外がある。SessionEndはrootのみ、reasonはcurrent `other`。明示Interrupt/SessionIdle hookはない。[Codex hook source](https://github.com/openai/codex/blob/ca4d532b2a5803159bfa8c8f56213948e068b62f/codex-rs/hooks/src/lib.rs)
- OpenCode: `session.deleted`は終了ではなく削除、plugin `dispose`もsession IDを持たない。pre-compactと`chat.message`注入はpublic typedでもexperimental/undocumented面を含む。[plugin types](https://github.com/anomalyco/opencode/blob/v1.18.16/packages/plugin/src/index.ts)、[compaction tests](https://github.com/anomalyco/opencode/blob/v1.18.16/packages/opencode/test/session/compaction.test.ts)
- Pi: handler exceptionはfail-openだがtimeout契約がない。`session_shutdown`はprocess exitだけでなくnew/resume/forkによるactive-session replacement境界。official subagent sampleは独立 subprocessでparent ID contractを持たない。
- Kimi: Pre/PostCompact outputは無視され、次のUserPromptSubmitで戻す。SubagentStart/Stopはagent nameしか持たずchild instanceをjoinできない。[Kimi source](https://github.com/MoonshotAI/kimi-code/blob/v0.34.0/packages/agent-core-v2/src/session/externalHooks/externalHooksService.ts)

### 4.2 5×5 handoff judgment

`C` は「main-sessionのmemory/checkpoint routeは条件付きで実装可能だが、exact-version real CLI E2E未合格」、`B` は「現仕様のまま不可能」を表す。

| source → destination | Claude | Codex | OpenCode | Pi | Kimi |
|---|:---:|:---:|:---:|:---:|:---:|
| Claude | C | C | C | C | C |
| Codex | C | C | C | C | C |
| OpenCode | C | C | C | C | C |
| Pi | C | C | C | C | C |
| Kimi | C | C | C | C | C |

結論は「25 routeはarchitecture上実装可能、現時点で25/25を証明済みではない」である。永続DB/MCPを中立境界にするため、source側がturn/compact/checkpointをcommitでき、destination側がfirst relevant promptへ注入できればmain-session routeは成立する。OpenCodeのtrue end欠落はturn/idle checkpointで補えるが、session-end parityを持つTier Aとは表示できない。Pi/Kimiのsubagent不足もmain-session route自体は止めないが、Hard Invariant 33のため識別不能なchild由来情報はauto-promotion禁止になる。

5×5は「同一Agentを含む25個の有向route scenario」と定義し、各scenario内でmemoryとcheckpointの両artifactを試すべきである。artifactを別pathとして数えるなら50 casesであり、数字を混在させない。Core 2×2=4はClaude→Claude、Claude→Codex、Codex→Claude、Codex→Codexの4 routeであることを明記する。

Tier判定は次のとおり。

- Core候補のClaude/Codex: source contractは十分だが、product adapterのreal CLI E2E前なので現時点ではTier A未証明。
- OpenCode: current contractではtrue session end unsupported。Tier B、または「main continuity Tier A / session-end coverage unsupported」の能力別表示が正確。
- Pi/Kimi: main continuityはTier A候補。subagent captureはunsupportedのまま例外承認が必要。
- Platform claim: 25 main routesに加え、capability limitation matrixを同時公開しない限り「5 Agent完全parity」と言ってはいけない。

---

## 5. Architecture simplifications

価値を落とさず削れるものに限定する。

1. **通常の追随forkをやめ、pinned vendor snapshotにする。** codememのsync/sharing領域はupstream churnが強く、v6が捨てる領域と衝突する。security/bugfixだけ個別cherry-pickすれば、raw ingest/search/MCP/viewer資産は保持できる。
2. **project identityの優先順位アルゴリズムを、opaque project UUID + evidence aliasesへ一本化する。** remote/root/common-dirをcanonical key生成に組み込まず候補照合に使う。機能を減らさずcollision handlingを単純化できる。
3. **Core provider実装をgeneric OpenAI-compatible、local OpenAI-compatible、必要なdocumented API adapterに絞る。** Cloudflareは同じadapterのcapability-probed profile、Claude/Codex sidecarはconditional plugin、CMEM/AI Gateway/OpenRouter/NIMは後続profileとする。role interfaceは維持される。
4. **LanceDBを1.0のenum/config/dependencyから削除する。** 100k超・p95超過等の再評価条件だけADRに残す。現在はinterface一個を増やす価値がない。
5. **FTSはcontentful derived tableを初期defaultにする。** external-content triggerの同期・既存row rebuildの失敗面を避ける。storage測定で問題になった時だけexternal-contentへ移す。
6. **generation roleごとのserviceを作らず、一つのjob runner + role/prompt/schema dataにする。** six role namesとprovider独立性は保持し、factory/service boilerplateは作らない。
7. **WebSocket、Vectorize、R2、AI Gateway、Private Relayを初期schema/packageから外す。** いずれもoptionalであり、HTTP push/pull、SQLite DO FTS、local backupだけでPersonal Cloud 1.0の価値を満たす。ADRの再評価条件だけ残す。
8. **Core UIは既存viewerを流用し、inspect/confirm/retract/link/restoreのauthoritative actionsだけ追加する。** 新しいUI frameworkや別viewerを作らない。
9. **claude-mem runtime/providerはimportしない。** 比較baselineとversion-pinned one-way data importerだけに使う。OAuth extraction、worker、Chroma、observer sessionを切り離すより新provider adapterのほうが小さい。

---

## 6. Missing requirements

| ID | 必須要件 | 理由 |
|---|---|---|
| MR-01 | capabilityに`unknown`、coverage、source event、evidence version/commit、last E2E date、limitationを持つ | native/unsupportedの二値誤表示を防ぐ。 |
| MR-02 | `adapterDeliveryId`とstable source fingerprintの永続化規則 | timestamp非依存のdedupeに必要。 |
| MR-03 | tool failure phase (`executed`, `permission_denied`, `schema_invalid`, `unknown_tool`, `interrupt`) | Agent間でfailure semanticsを揃える。 |
| MR-04 | turn graphのopen tool set、terminal reason、grace/finalize rule | parallel/late tool eventを閉じる。 |
| MR-05 | derived artifact invalidation graphとnext-injection correction notice | stale batch correctionを実際のcontextまで伝播する。 |
| MR-06 | sessionの`hostId`, process start identity, monotonic heartbeat、remote-host liveness禁止 | PID再利用と別device active sessionの誤abandonを防ぐ。 |
| MR-07 | explicit task boundary/new lineage operation | native session内の別taskを同じlineageにしない。 |
| MR-08 | checkpoint claim fence/revision/heartbeat、cross-device authority mode | duplicate injection/late acceptを閉じる。 |
| MR-09 | canonical mechanical resumeの保証を「evidence recovery」と定義し、semantic next actionと別metricにする | provider停止時に意味推論まで100%保証しない。 |
| MR-10 | project identity state (`verified/provisional/conflicted`) とalias approval audit | wrong-project fail closedに必要。 |
| MR-11 | local daemon RPCのpeer identity、per-user auth、socket/file permission、multi-user拒否 | localhost/stdioでも別user/processのwriteを防ぐ。 |
| MR-12 | sidecar isolation certification manifest（effective config/tool set、supervisor、version、OS） | documented transportとsafe sandboxを分ける。 |
| MR-13 | provider endpoint-level capabilitiesとprobe expiry | Chat/Responses/JSON/usage/idempotency差を扱う。 |
| MR-14 | embedding item/revision/preprocess contractとper-item ledger | partial/cancel/model driftに必要。 |
| MR-15 | sqlite-vec artifact SHA-256/platform/ABI allowlistとload後disable | native extension supply-chainを閉じる。 |
| MR-16 | fixed FTS schema、normalization version、query escaping、CJK routing、RRF式/cap/tie | retrieval再現性に必要。 |
| MR-17 | backup manifestとfresh-dir restore/rebuild protocol | WAL/vector/FTSの完全復旧に必要。 |
| MR-18 | sync signature/key epoch、multi-parent resolution、head-set CAS | immutable revisionを安全に収束させる。 |
| MR-19 | tombstone floor/entity epochとoffline-device rebootstrap rule |無期限offline端末の削除復活を防ぐ。 |
| MR-20 | snapshot signed manifest、root hash/count/seq/schema、page consistency | destructive compactionのpreconditionを検証可能にする。 |
| MR-21 | device revoke cutover semanticsとqueued-op rejection | revoke後replayを防ぐ。 |
| MR-22 | MCP 2026-07-28 Origin/header/version validationとOAuth resource/audience profile | current transport/securityに必要。 |
| MR-23 | provider/free daily hard budgetへretry/repair/sparse traceを含める | nominal 80 requestだけのgamingを防ぐ。 |
| MR-24 | evalのprimary endpoint/baseline/margin/CI/power/seed、blind judge、holdout refresh |再現可能な非劣性claimに必要。 |
| MR-25 | import対象tag/schema/row mapping、count/ID/redaction/idempotence fixture | claude-mem等からのunsupported DB importを安全な一方向移行にする。 |
| MR-26 | install ownership manifest、atomic config merge、rollback journal |他tool hook/MCP/configを守る。 |

---

## 7. ADR changes

| ADR | v6 default | 推奨default | 理由 / gate |
|---|---|---|---|
| Local Core | codemem fork確定 | `codemem 26438e75 vendor snapshot`を仮説。Phase 0 spike後にfork/greenfield/ai-memoryを確定 | current direct writeは横断的。再利用資産も大きく、今はどちらも即断不可。 |
| Upstream |通常fork追随 | security/bugfix単位cherry-pick、sync/sharing変更はdefault不採用 | unwanted churnとの長期merge費を抑える。 |
| Runtime | TypeScript/Node維持 | Phase 0までprovisional。sole-writer deltaがai-memory/remem移行deltaを上回る時だけ再審議 | 言語固定でbase比較を循環させない。 |
| Project identity | remote/root/common-dir優先 | opaque project UUID canonical、Git evidenceはexplicit alias候補 | collision時fail closed。 |
| Checkpoint delivery | local claim+10分lease、delivery非sync | fence+heartbeat+CAS。cloud sync checkpointはserver claim、またはduplicateをforkとして保持 | cross-device exact-onceはlocal leaseで保証不能。 |
| Capability | 3値 + Tier | 4値 + versioned coverage profile。Tierはprofile hashに対して付与 | partial/unknownを表示。 |
| Claude sidecar | Zero Incremental Cost auto候補 | BYOK/API-key `--bare`のみ、default disabled | official auth/legal契約。 |
| Codex sidecar | official sidecar auto候補 | conditional plugin、isolation certification合格版のみ | exec transportはStableでもall-tools-off等はunsupported。 |
| Free provider order | sidecar→local→Cloudflare | certified local→certified Cloudflare/他Universal Free。sidecarは別explicit profile | Universal Freeとsubscriptionを混ぜない。 |
| Free budget | observer ≤80 request/day | retry/repair込みhard cap + dense/sparse trace | idle flushで200 request化し得る。 |
| Embedding backend | sqlite-vec optional default、LanceDB enumあり | `none` default、certified sqlite-vec opt-in。LanceDBは未実装ADR | FTS correctnessを先にrelease。pre-v1 native dependencyを必須化しない。 |
| FTS topology |未決定 | contentful derived FTS、`unicode61 remove_diacritics 2`、trigram explicit config | trigger inconsistencyを避ける。 |
| RRF | RRF | fixed `k`/cap/rank/tieをdataset versionと共に固定 | benchmark再現性。 |
| Sync resolution | single parent resolve | sorted multi-parent all-head resolution + head-set CAS | conflict closure。 |
| Tombstone | compaction windowより長く保持 | entity epoch/tombstone floor。古いbaseはrebootstrapまでreject | indefinitely offline device対応。 |
| Remote MCP | Bearer + later OAuth | local stdio、fixed-client scoped bearer、generic OAuth 2.1を別profile | current MCP security要件。 |
| Evaluation | 120 sessions、best baseline -2% | power analysisでN決定、locked primary baseline/endpoint、paired CI | 120はpilot値であって十分性の証明でない。 |
| Release staging | cloud後にAgent expansion | Core→Agent expansion→cloud protocol→Platform full gate | 第二優先の5 Agent parityを先に検証し、cloudを不安定なschemaへ載せない。 |
| Platform claim | 5 Agent Tier A | 25 main routes pass + capability limitation profile公開。unsupported subagent/endはapproved Tier B exception | 実際のnative限界を隠さない。 |

---

## 8. Corrected implementation order

Phase/PRを「後で捨てる可能性の高い基盤を先に作らない」順へ修正する。

### Phase 0A / PR 1 — Evidence freeze and base bake-off（機能変更なし）

- codemem `26438e75...`、ai-memory `a9e9a24d...`、remem `cde8bc05...` をpin。
- exact toolchainでupstream check/testを実行し、license/SBOM/native assetを保存。
- all DB opens/write handles、provider auth/backend、sync/sharing importsを静的inventory。
- fork/vendor/greenfield deltaを、変更LOCではなく「残るwrite handle数、壊すtest数、移植資産数、unsafe auth path数」で比較。
- exit: base ADR。失敗なら新Node daemon shellへMIT資産を選択移植し、全面greenfieldにはしない。

### Phase 0B / PR 2 — Adapter and sidecar contract harness（product DB変更なし）

- Claude 2.1.228 / Codex 0.147.0 real binaryでhook lifecycle、timeout、first injection、compact、tool failure phase、interrupt、subagentをfixture化。
- capability schemaを`unknown/coverage/evidenceVersion`対応。
- sidecarは別harnessでhostile config/tool/process-tree test。合格しなければdefault disabledを確定。
- このPRでTier Aを宣言しない。

### Phase 1 / PR 3 — Safety boundary and sole writer

- undocumented/private provider/auth loaderを先に物理削除/非到達化。
- daemonだけがwrite-capable DB handleを所有。hook/MCP/CLI/viewerはthin RPC client。
- daemon unavailable時はatomic spoolのみ。read-only handleはDDL/bootstrapを行わない。
- local peer auth、version handshake、schema allowlist、size bound、redaction、install ownership manifest。
- exit: runtime DB-open traceでwriter=1、daemon kill/replay/duplicate test、backup baseline。

### Phase 2 / PR 4 — Canonical identity and event state machine

- opaque project/workspace/scope IDs、alias conflict UI/CLI。
- adapterDeliveryId、tool failure phase、turn graph、late correction invalidation。
- session host/process identity、heartbeat、abandon rule。
- exit: identity collision matrix、duplicate x10、parallel/late event property tests。

### Phase 3 / PR 5 — Continuity state machine

- SessionWorkState、explicit task lineage、checkpoint fence/lease heartbeat/CAS。
- mechanical evidence resumeとoptional semantic noteを別metric。
- crash/precompact/next-prompt/accept/dismiss/supersede。
- memory_resumeと最小viewer actions。
- exit: observer/embedding/sync offでClaude・Codex各same-agent continuation。

### Phase 4 / PR 6 — Thin Claude/Codex vertical routes

- Claude/Codex adapterを完成させ、4 directed routeを各memory+checkpointで実行。
- response loss、long turn、simultaneous sessions、managed hook limitationを含める。
- exit: 4/4 route scenario、capability profile公開、Tierは合格evidence hashへ付与。

### Phase 5 / PR 7 — Local retrieval/injection/MCP correctness

- contentful dual FTS、exact/path/CJK routing、fixed RRF、hard filter、envelope/provenance stripping。
- local stdio MCP 5 tools、user-authority CAS。
- vectorはoffのまま100k scale/JP-EN-mixed correctnessを先に通す。

### Phase 6 / PR 8 — Generation roles and one Universal Free candidate

- one job runner、generation contract/run ledger/cache、client-side schema validation。
- generic/local adapters。free providerはprobe manifestとhard daily budget合格候補だけ。
- Claude/Codex sidecarはcertification合格時のみ別optional PRで追加。
- claude-mem importerはtag-pinned one-way canonical rowsだけ。

### Phase 7 / PR 9 — Optional embeddings

- item-addressable contract、stable sqlite-vec artifact pin、per-item ledger、immutable build set/catch-up/CAS。
- extension不在/不一致はFTS-only。
- LanceDBは実装しない。

### Phase 8 / PR 10 — Core 1.0 gates and release

- Track 1 deterministic safety/continuity、backup/restore、install/update/uninstall、72h soak、signed artifacts。
- quality metricsはadvisory、外部quality claimなし。
- Core release後もcloudへ直行せず、schemaをfreeze。

### Phase 9 — Agent expansion before cloud

- OpenCode、Pi、Kimiを順次version-pinし、追加Agentを含む全main routesを通す。
- unsupported session-end/subagent capabilityはTier B exceptionとして公開。
- 25 routes（各memory+checkpoint）を完了し、agent-neutral schemaの欠陥をcloud実装前に潰す。

### Phase 10 — Personal Cloud protocol

- signed op、multi-parent resolution、epoch/tombstone floor、snapshot manifest、device revoke。
- SQLite DO transaction/FTS conformance、quota accounting。
- MCP 2026-07-28 fixed bearer/OAuth profiles。
- multi-device checkpoint claimはserver authorityまたはfork semanticsを選びADR化。

### Phase 11 — Platform 1.0 / Track 2

- power-based corpus、sequestered holdout、blind judging、reproducible baselines。
- cloud convergence/restore/security、25 routes、capability profileを同じrelease artifactで再確認。
- 条件を満たしたprofileに限りdataset/date/CI付きclaimを付ける。

依存関係上の主な修正は、adapter contractをcontinuity実装より前にspikeすること、Codex adapter完成をPR 10まで遅らせないこと、FTS correctnessをembeddingより先に置くこと、Agent expansionをPersonal Cloudより前に置くことである。

---

## 9. Final delta — v6へのsection単位patch案

以下は実装ではなく、`agent-memory-final-spec-v6.md` に反映すべき規範文の提案である。

### §3 Hard Invariants へ追記・置換

```diff
- 3. event配送はat-least-onceを許容し、処理結果はidempotentにする。
+ 3. event配送はat-least-onceを許容する。各adapterはnative発火単位でstableなadapterDeliveryId、またはvolatile fieldを除外したversioned source fingerprintを生成し、同一論理eventの処理・派生job・provider requestをidempotentにする。occurredAtだけに依存する近似keyはrelease対象で使用しない。

- 7. redaction前のsecretを保存・remote processing・syncしない。
+ 7. versioned secret detector rulesetが検出したsecret、および明示private/secret範囲はredaction前に永続化・remote processing・syncしない。release claimのsecret leak 0は固定fixture/rulesetに限定し、未知secretの完全検出を主張しない。

- 14. injected context、memory MCP結果、memory Web UI exportを新規memoryとして自己再取り込みしない。
+ 14. memory subsystemが生成したinjection/MCP result/UI exportはstable provenance IDを持ち、既知のcapture surfaceでは本文保存前に除外する。markerが失われた任意user pasteまで完全識別できるとは主張しない。

- 19. sync conflictで本文を無言上書きしない。
+ 19. sync conflictは全competing headを保持し、全head IDをparentに持つexplicit resolutionとhead-set compare-and-swapなしに本文を確定しない。client timestampをwinner authorityにしない。

- 33. subagent由来の未確定情報を親turn完了前にcross-agent durable memoryへ昇格しない。
+ 33. parent/child/parentTurnを安定識別できないadapterではsubagent由来の可能性があるeventをcross-agent durable memoryへ自動昇格しない。識別できる場合もparent turn successful completion watermarkまでは昇格しない。
```

### §4 Base selection を置換

```text
Phase 0のdefault hypothesisはcodemem commit 26438e75... のpinned vendor snapshotとする。通常のupstream追随forkは前提にしない。次のgateを全て満たした後だけLocal Core base ADRをcodemem-forkへ確定する。

1. exact toolchainでfull checks green
2. daemon以外のwrite-capable SQLite handleがstatic scan/runtime traceとも0
3. hook ingest/inject/file-context、OpenCode、MCP、CLI/adminからsharing/coordinatorへのmutation dependencyがない
4. undocumented/private backendと第三者credential cache loaderが非到達ではなくsource/runtimeから除去済み
5. ai-memory/remem/new Node daemon shellとのdelta比較を記録

gate failure時はgreenfieldを即選ばず、再利用可能なMIT parser/spool/schema/search/MCP/viewer資産の選択移植を比較する。
```

### §6 Project identity を置換

```text
project_idはrandom opaque UUIDをcanonical authorityとする。remote URL、root commit、repository fingerprint、Git common-dir、pathはproject link候補を生成するevidenceであり、自動merge authorityではない。identity stateはverified/provisional/conflictedを持つ。異なるproject_idを同一とする操作はuser-authoritative link/mergeとbackup/auditを必須とする。

shallow repository、fork、multiple/renamed remote、URL rewrite、git replace/graftを検出した場合はautomatic linkingをfail closedにする。Git common-dirはlinked worktreeのlocal workspace groupingにのみ使う。monorepo scopeはpathではなくstable scope UUIDを持ち、renameはalias更新とする。
```

### §7 Adapter Contract を修正

```ts
type Capability = "native" | "synthesized" | "unsupported" | "unknown";

interface CapabilityEvidence {
  value: Capability;
  coverage?: number;
  sourceEvents: string[];
  nativeVersion: string;
  sourceCommit?: string;
  evidenceKind: "official-doc" | "source-test" | "real-cli-e2e";
  verifiedAt: string;
  limitations: string[];
}
```

```text
tool_failedはexecuted failureだけを意味しない。failurePhaseをexecuted/permission_denied/schema_invalid/unknown_tool/interrupt/unknownに分離する。turn_completed/session_ended/session_interruptedを合成するadapterは根拠eventとconfidenceを保存する。

Claude: Stopはgeneric interruptを捕捉せず、PostToolUseFailureはpermission/schema/unknown-toolを網羅しない。
Codex: normalized assistant/turn completion、tool success/failure、query-aware/checkpoint injectionはsynthesized。explicit interrupt/session_idleはunsupported。compact再注入はSessionStart(source=compact)、fallbackは次UserPromptSubmit。
OpenCode: trueSessionEndはunsupported。session.deleted/disposeで代用しない。chat.messageとexperimental compactはexact version E2E必須。
Pi: session_shutdownはactive-session boundary。interruptはsynthesized、subagentCaptureはunsupported。
Kimi: dynamic query injectionはUserPromptSubmit。Pre/PostCompact outputは無視。checkpointは次promptでsynthesized。child instance IDがないためsubagentCaptureはunsupported。

Tierはadapter名だけでなくcapability_hash + native_version + E2E artifactへ付与する。未実行cellはunknownとしstable表示しない。
```

### §8 Event pipeline を修正

```text
idempotencyKeyはadapterDeliveryIdがある場合それをauthorityとする。ない場合はschema-versioned canonical source fingerprintを使い、occurredAt、ingest時刻、delivery attempt、injection ID等のvolatile fieldを除外する。approximate idempotencyのadapterはrelease blocking eventに使用しない。

turn stateはopen toolUseId set、terminal tool results、native turn boundary、close reason、grace deadlineを持つ。late material eventはaffected batchだけでなく、derived memory/summary/vector/context generationをinvalidated/supersededにし、次のinjectionにbounded correction noticeを含める。

spool priority reserveにもhard maximumとcritical stateを持たせ、tmp write/fsync/rename/import commit/delete各点のfault injectionをrelease gateにする。
```

### §10–§11 Session/Checkpoint を修正

```ts
interface Session {
  // existing fields...
  hostId?: string;
  processStartIdentity?: string;
  heartbeatSeq?: string;
}

interface ContinuationCheckpoint {
  // existing fields...
  revision: string;
  claimFence?: string;
  claimDeviceId?: string;
  claimHeartbeatUntil?: string;
  taskBoundaryReason?: "explicit" | "accepted_resume" | "native_fork" | "new_substantive_task";
}
```

```text
PID livenessは同一hostIdかつprocessStartIdentity一致時だけ使用する。remote hostのsessionをlocal process lookupでabandonedにしない。

claimはcheckpoint revisionに対するfencing tokenを発行する。heartbeatでlease延長でき、accept/dismiss/open復帰はcheckpoint ID + revision + fence + destination sessionのCASを必須とする。stale fenceのacceptは拒否する。

local-only profileの自動配送保証は一device内at-most-one active claimに限定する。Personal Cloudで同一checkpointのcross-device重複注入0を要求する場合、claim authorityはcloud endpointとする。offline-firstを優先して重複を許すmodeでは、複数acceptを別fork lineageとして保持し、max(acceptedAt)で潰さない。

同一sourceSessionIdだけを理由にtaskLineageIdを継承しない。explicit resume/native fork/明示task継続、または前checkpoint受理後の同一active taskでのみ継承する。userはnew task boundaryを作成できる。

provider停止時の最低保証はcanonical evidence recoveryであり、goal/completed/next actionの意味的正しさはsemantic refinementまたはhuman評価の別metricとする。
```

### §12 Durable Memory schema を修正

```diff
- origin: "extracted" | "manual" | "imported" | "consolidated";
+ origin: "extracted" | "manual" | "agent_explicit" | "imported" | "consolidated";
+ provenanceQuality: "native" | "mapped" | "legacy_unknown";
```

```text
model output Candidateはdurability=pinned、truthState=user_confirmed/runtime_confirmedを表現できないschemaとする。これらはuser-authoritative commandまたはdeterministic verifierだけが別transaction/CASで設定する。

raw event削除は、参照する全durable memoryのevidence snapshot commitとmemory metadata updateを同一transactionまたはjob CASで確認した後だけ行う。
```

### §13–§14 Provider/Observation を修正

```text
Claude CLI providerはAPI-key/BYOKの--bare profileのみをcandidateとし、subscription Zero Incremental Cost profileとして自動提案しない。credential store/token extractionは禁止する。

Codex exec transportはStableだがsidecar isolationは本製品のsynthesized機能である。all tools/hooks/plugins/managed configを無効化できないversion/environmentではproviderを起動しない。external supervisorがdeadline、process-group/job-object kill、pipe close、wait/reap、descendant/FD検査を行う。--ephemeral profileとresume profileを分離する。

provider capability manifestはmodel_idだけでなくendpoint、response format、streaming、usage、request idempotency、resolved model revision、probe version/expiryを持つ。structured outputはstream=falseかつclient側で同じJSON Schemaを再validateする。

free daily budgetはretry/repair/cache missを含むhard capとし、dense batch traceとidle間隔が長いsparse traceの双方で認定する。quota到達前にjobをqueueへ戻し、paid/provider外fallbackをしない。

injection/MCP/exportはstable provenance ID/content hashを付ける。capture adapterは既知wrapperを本文保存前にstripし、unknown wrapperはsensitivityを上げてauto-promotionしない。
```

### §15–§16 Embedding/Retrieval を置換・追記

```ts
interface EmbeddingItemRequest {
  itemId: string;
  memoryId: string;
  memoryRevision: string;
  inputHash: string;
  text: string;
}

interface EmbeddingRequest {
  requestId: string;
  generationId: string;
  modelRevision: string;
  preprocessorId: string;
  preprocessorVersion: string;
  items: EmbeddingItemRequest[];
  signal?: AbortSignal;
}

type EmbeddingItemResult =
  | { itemId: string; status: "ok"; vector: Float32Array }
  | { itemId: string; status: "retryable_error" | "permanent_error" | "cancelled_or_unknown"; errorCode: string };
```

```text
provider adapterは保存前にfinite、exact dimensions、metric、cosine時L2 norm>0を検査する。unique keyは(generation_id,memory_id,memory_revision,input_hash)。cancelled_or_unknownはread-before-retryする。

generation buildはimmutableなmemory revision/input hash setとstart watermarkを固定し、変更分をcatch-upする。complete coverage、validation、search smoke後にsingle SQLite transactionでactive pointerをcompare-and-swapする。conflict/interruption時はold pointerを維持する。

sqlite-vecはstable release artifact、SHA-256、platform tuple、ABIをpinし、allowlisted absolute pathからdaemonだけがloadして直後にextension loadingをdisableする。不一致/不在時はFTS-only。LanceDBは1.0 scopeから削除する。

初期FTSはcontentful derived tables。unicode61はremove_diacritics 2とexplicit tokenchars、trigramはexplicit options/detailを固定する。CJK 1–2文字はexact/controlled n-gram、3文字以上はtrigram、identifier/pathはexact、mixedは独立streamとする。

RRFはscore(d)=sum_s 1/(k+rank_s(d))、rankは1始まり、k、各stream cap、dedupe key、missing stream、stable tie-break(score desc, authority desc, recency desc, memoryId asc)をdataset/versionごとに固定する。
```

### §18 Remote MCP を修正

```text
local transportはstdio。remoteはMCP 2026-07-28 Streamable HTTP POST-only profileを実装し、MCP-Protocol-Version、Mcp-Method、該当時Mcp-Nameのheader/body整合とOrigin allowlistを検証する。

fixed first-party clientはHTTPS、short-lived scoped bearer、audience、expiry/revoke、rate limitを必須とする。generic hosted clientはProtected Resource Metadata、OAuth 2.1 authorization server discovery、resource indicator/audience、scope、PKCE、refresh rotationを満たすまで正式supportしない。sync credentialとMCP credentialは別audience/keyとする。
```

### §19–§20 Daemon/Backup を追記

```text
daemon以外のprocessはwrite-capable SQLite connectionを開かない。read-only connectionはschema bootstrap/DDL/ledger writeを行わない。release gateはsource scanだけでなくruntime DB-open traceを含む。

backup manifestはschema version、SQLite source version、FTS schema/normalization、sqlite-vec artifact/version/hash/platform、active embedding generation、canonical table row count/checksum、created watermarkを含む。restoreはfresh data directoryへcanonical DBを復元し、FTS/vectorをrebuildして検証後atomic replaceする。vector extension不在でもcheckpoint/FTS-only restoreは成功する。
```

### §22 Personal Cloud を修正

```ts
interface SyncOperation {
  // existing fields...
  credentialEpoch: string;
  keyId: string;
  parentRevisionIds: string[];
  signature: string;
}
```

```text
parentRevisionIdsはJCS順でsortし、resolve_conflictはresolution開始時の全headを列挙してhead-set compare-and-swapする。single parentはlinear reviseだけに使う。

originDeviceSeqはcredential epoch内で一意かつstrictly monotonicとし、duplicate same hashのみidempotent accept、same seq different hash/gap policy違反はquarantine/rebootstrapする。revokeされたcredential epochのopは、作成時刻に関係なくapplyしない。

snapshot manifestはepoch、snapshot seq、schema、row count、chunk hashes、root hash、tombstone floor、transaction watermark、signatureを持つ。全page同一snapshot IDを検証してからatomic importする。

tombstoneを有限期間後に無条件削除しない。entity epoch/tombstone floorより古いbase revisionからのmutationはrejectし、long-offline deviceはfull rebootstrapを完了するまでpush不可とする。

checkpoint delivered状態をsyncしないlocal-first modeはcross-device duplicate injectionを許容する契約とし、複数acceptをfork lineageとして保持する。duplicate 0を要求するmodeはserver-authoritative claim/fence endpointを使用する。

PITR/reset後は新epochを発行し、全old cursor/credential baseをrebootstrapへ送る。PITRはcompaction/backupの代替にしない。
```

### §27 Evaluation を修正

```text
Track 1 blockingはdeterministic safety/state-machine/property testsとreal CLI conformanceに限定する。human judgmentを含むnext-action qualityはadvisoryとし、canonical evidence recovery 100%と分離する。

Track 2は事前にprimary endpoint、primary baseline、absolute/relative margin、paired evaluation unit、aggregation、confidence level、multiple-comparison policy、seed/trial数、missing-run policyをmanifestへ固定する。sample数は120固定ではなくpower analysisで決め、120は最低pilot corpusとする。

holdoutはsequestered one-shot release setとし、prompt/model選定後だけ実行する。反復releaseで消耗したholdoutはrefreshする。session/project/template duplicateをsplit前に除外する。

semantic continuation judgeはprovider名/出力順を隠したrandomized paired formを2名以上が独立採点し、agreementとadjudicationを保存する。deterministic metricをprimaryにする。

baselineはexact version/config/provider/model/date/request hash/raw outputまたは許可されたaggregate artifactを保存する。CMEMが再現不能/利用不能ならCMEM claimを行わない。current claude-mem releaseにはcontinuity品質baseline assetがないため、upstream test greenを非劣性証拠にしない。

Cross-Agentのpathは5×5=25 directed route scenarios（self routeを含む）と定義し、各scenarioでmemoryとcheckpointの両artifactを試す。別artifactを別caseと数える場合は50 casesと明示する。
```

評価方法の根拠として、test/validation setの反復利用によるwear-outと重複回避は[Google ML dataset guidance](https://developers.google.com/machine-learning/crash-course/overfitting/dividing-datasets)、sequestered blind evaluationは[NIST AITE](https://pages.nist.gov/ai-technology-evaluation/)、複数annotator/adjudicationは[NIST ARIA evaluation report](https://nvlpubs.nist.gov/nistpubs/ai/NIST.AI.700-2.pdf)と整合する。

### §29–§31 Phase/ADR を置換

```text
実装順は本レビュー§8のPhase 0A〜11を採用する。特に、base bake-offとreal CLI contract harnessをsole-writer/continuity実装より前に置き、Claude/Codex vertical routesをCoreの中盤で完成させる。FTSをembeddingより先にし、Core release後はAgent expansionをPersonal Cloudより先に行う。

ADR default:
- Local Core: pinned codemem vendor hypothesis; Phase 0 gate後確定
- Project identity: opaque UUID canonical
- Sidecars: default disabled/conditional certification; Claude subscription profileなし
- Vector: none default, certified sqlite-vec opt-in, LanceDB未実装
- Checkpoint: fenced local claim; cloudはserver claimまたはfork semantics
- Remote MCP: stdio/fixed scoped bearer/OAuth profiles分離
- Evaluation: deterministic Track 1, pre-registered statistical Track 2
```

### §34 Final Go Decision を置換

```text
CONDITIONAL GO: Core architectureの方向は維持するが、Phase 0 gate完了までbaseとsidecarを確定せず、B-01〜B-10のblockerを仕様へ反映する。codememは26438e75... vendor snapshotの比較仮説であり、通常のupstream追随forkではない。

Core 1.0 GO条件:
1. daemon以外のwrite-capable DB handle 0
2. Claude/Codex exact-version 4 directed routes pass
3. fenced checkpoint state machineとdeterministic event idempotency pass
4. opaque project identity collision suite pass
5. FTS-only continuation/backup/restore pass
6. Universal Free candidateはcurrent probe/budget認定済み。sidecarを必須にしない
7. secret/echo/wrong-project claimはversioned deterministic fixtureの範囲を明記

Platform 1.0 GO条件:
1. 25 directed main-session routes pass
2. unsupported/partial capability profileを公開し、Tier B exceptionを承認
3. signed multi-head sync、tombstone floor、snapshot/revoke tests pass
4. MCP 2026-07-28 security profile pass
5. pre-registered Track 2でCI付き非劣性claimが再現可能
```

---

## 付記: 一次資料index

- codemem: [`schema/store/search/vector manifest`](https://github.com/kunickiaj/codemem/blob/26438e75ce1d0fec6be34981f15045a15c89658b/packages/core/package.json)、[`observer-client.ts`](https://github.com/kunickiaj/codemem/blob/26438e75ce1d0fec6be34981f15045a15c89658b/packages/core/src/observer-client.ts)、[`observer-auth.ts`](https://github.com/kunickiaj/codemem/blob/26438e75ce1d0fec6be34981f15045a15c89658b/packages/core/src/observer-auth.ts)、[`pack eval`](https://github.com/kunickiaj/codemem/blob/26438e75ce1d0fec6be34981f15045a15c89658b/scripts/eval/pack-eval.ts)
- claude-mem: [`actual hooks.json`](https://github.com/thedotmack/claude-mem/blob/f792a27eaf0f7c3906592c97c6484d5097b4798f/plugin/hooks/hooks.json)、[`session-init.ts`](https://github.com/thedotmack/claude-mem/blob/f792a27eaf0f7c3906592c97c6484d5097b4798f/src/cli/handlers/session-init.ts)、[`ClaudeProvider.ts`](https://github.com/thedotmack/claude-mem/blob/f792a27eaf0f7c3906592c97c6484d5097b4798f/src/services/worker/ClaudeProvider.ts)、[`oauth-token.ts`](https://github.com/thedotmack/claude-mem/blob/f792a27eaf0f7c3906592c97c6484d5097b4798f/src/shared/oauth-token.ts)、[`plugin distribution test`](https://github.com/thedotmack/claude-mem/blob/f792a27eaf0f7c3906592c97c6484d5097b4798f/tests/infrastructure/plugin-distribution.test.ts)
- Claude Code official: [hooks](https://code.claude.com/docs/en/hooks)、[hook guide](https://code.claude.com/docs/en/hooks-guide)、[headless](https://code.claude.com/docs/en/headless)、[CLI reference](https://code.claude.com/docs/en/cli-reference)、[sessions](https://code.claude.com/docs/en/sessions)、[legal](https://code.claude.com/docs/en/legal-and-compliance)
- Codex official: [hooks](https://developers.openai.com/codex/hooks)、[CLI reference](https://developers.openai.com/codex/cli/reference)、[non-interactive](https://developers.openai.com/codex/noninteractive)、[`output_schema` test](https://github.com/openai/codex/blob/ca4d532b2a5803159bfa8c8f56213948e068b62f/codex-rs/exec/tests/suite/output_schema.rs)、[`session_start.rs`](https://github.com/openai/codex/blob/ca4d532b2a5803159bfa8c8f56213948e068b62f/codex-rs/hooks/src/events/session_start.rs)、[`user_prompt_submit.rs`](https://github.com/openai/codex/blob/ca4d532b2a5803159bfa8c8f56213948e068b62f/codex-rs/hooks/src/events/user_prompt_submit.rs)
- OpenCode: [plugin docs](https://opencode.ai/docs/plugins/)、[`session.ts`](https://github.com/anomalyco/opencode/blob/v1.18.16/packages/opencode/src/session/session.ts)、[`prompt.ts`](https://github.com/anomalyco/opencode/blob/v1.18.16/packages/opencode/src/session/prompt.ts)、[`task tests`](https://github.com/anomalyco/opencode/blob/v1.18.16/packages/opencode/test/tool/task.test.ts)
- Pi: [`extensions types`](https://github.com/earendil-works/pi/blob/v0.84.1/packages/coding-agent/src/core/extensions/types.ts)、[`runner tests`](https://github.com/earendil-works/pi/blob/v0.84.1/packages/coding-agent/test/extensions-runner.test.ts)、[`session format`](https://github.com/earendil-works/pi/blob/v0.84.1/packages/coding-agent/docs/session-format.md)
- Kimi: [hooks docs](https://www.kimi.com/code/docs/en/kimi-code-cli/customization/hooks.html)、[plugins docs](https://www.kimi.com/code/docs/en/kimi-code-cli/customization/plugins.html)、[`hook runner tests`](https://github.com/MoonshotAI/kimi-code/blob/v0.34.0/packages/agent-core-v2/test/agent/externalHooks/runner.test.ts)
- SQLite/sqlite-vec: [FTS5](https://www.sqlite.org/fts5.html)、[backup](https://www.sqlite.org/backup.html)、[WAL](https://www.sqlite.org/wal.html)、[`sqlite-vec source`](https://raw.githubusercontent.com/asg017/sqlite-vec/v0.1.9/sqlite-vec.c)、[`correctness tests`](https://github.com/asg017/sqlite-vec/blob/v0.1.9/tests/correctness/test-correctness.py)、[Node SQLite](https://nodejs.org/api/sqlite.html)
- Cloudflare: [Workers AI pricing](https://developers.cloudflare.com/workers-ai/platform/pricing/)、[OpenAI compatibility](https://developers.cloudflare.com/workers-ai/configuration/open-ai-compatibility/)、[JSON mode](https://developers.cloudflare.com/workers-ai/features/json-mode/)、[Workers AI errors](https://developers.cloudflare.com/workers-ai/platform/errors/)、[SQLite DO storage](https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/)、[DO limits](https://developers.cloudflare.com/durable-objects/platform/limits/)、[DO pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/)
- MCP: [Streamable HTTP 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/streamable-http)、[authorization](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization)、[security considerations](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/security-considerations)

sources_reviewed: 84（本報告で直接参照した重複を除く一次URL数）
