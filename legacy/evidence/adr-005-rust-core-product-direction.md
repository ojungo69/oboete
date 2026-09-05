# ADR-005: Local Core の標準実行基盤を Rust へ段階移行する

- Status: **Accepted**
- Date: 2026-08-18
- Decider: repository owner
- Related: ADR-001, ADR-003, GitHub issues #1, #8, #9, #13
- Supersedes: ADR-003 の「Rust へ移行するか自体を Stage 1 で決める」という判断範囲
- Preserves: ADR-003 の contract freeze、G1–G7、比較指標、shadow / rollback 方針

## 決定

`free-mem` のローカル Core Runtime は、最終的に **Rust を標準実装・標準配布物とする方向で段階移行する**。

この決定は「リポジトリ内の全コードを Rust に統一する」という意味ではない。常駐・永続化・復旧・検索・MCP を担う Core Runtime を Rust に置き、Agent 固有 adapter、hook、plugin、React viewer、Cloudflare Worker などは、それぞれの実行環境に最も適した言語を維持してよい。

Stage 1 narrow prototype の役割は、以後次のように再定義する。

1. Rust を採用するか否かをゼロから決めるのではない。
2. **Stage 1 が決めるのは roadmap を進めてよいかと候補 scope の評価までである。実際の Core 1.0 の cutover scope と時期は、Cutover gate 1–10 と #84 の完了後に確定する**。Stage 1 が実測するのは、Core 1.0 から Rust を default runtime にできる見込みがあるか、
   一部 cutover を後続 release へ分ける必要があるかである。
3. Rust 実装が既存の安全契約を満たさない場合は、切替時期・slice・実装方法を見直す。
4. Rust という言語だけを理由に、G1–G7、migration、rollback、behavioral quality gateを免除しない。
5. 戦略自体を撤回する場合は、ADR-003 の defer 記録だけではなく、代替 runtime が長期目標をより良く満たす証拠を伴う新しい owner ADR を要求する。

## この ADR が変えないもの

要件・スキーマ・ゲートの基礎正本は `agent-memory-final-spec-v6.md`（v6.1）であり、優先関係は
`specs/001-agent-memory-core/spec.md` の冒頭が定めている。本 ADR はその連鎖を書き換えない。したがって
**この ADR だけでは Core 1.0 が何の上に出荷されるかは変わらない**:

- v6.1 は Runtime を「TypeScript/Node 維持」とし、rewrite 条件（measured runtime bottleneck、
  cross-platform packaging blocker、process instability、上流追従より rewrite が小さい）を列挙している
- [ADR-001](adr-001-base.md) は Accepted のまま、Core 1.0 の実装ベースを codemem pinned vendor snapshot としている
- `specs/001-agent-memory-core/plan.md` の Language/Version も TypeScript / Node のままである

これらを書き換えないと、Core 1.0 の runtime authority が本 ADR と正本の 2 つになる。**正本が勝つ**。
本 ADR が確定させたのは戦略目標と Stage 1 の役割であって、Core 1.0 の出荷基盤ではない。Rust が
Core 1.0 の default になるには、Stage 1 が pass に達することと、正本連鎖の改訂が完了していることの
両方が要る。**その改訂をどの手続きで、どの時期に行うかは本 ADR では決めない**——
[#84](https://github.com/ojungo69/free-mem/issues/84) の決定に委ねる。v6.1 §4.3 は Phase 0A の
base gate と再審議トリガーを定めるだけで、正本連鎖の改訂手続きは定義していない。

## 目標アーキテクチャ

```text
Claude Code ─┐
Codex CLI   ─┤
OpenCode    ─┤
Pi / Kimi  ─┼── thin native adapter / hook / plugin
other MCP  ─┘                    │
                                 │ versioned RPC / MCP contract
                                 ▼
                      ┌────────────────────────┐
                      │ free-mem Core Runtime  │
                      │ Rust                   │
                      ├────────────────────────┤
                      │ daemon lifecycle       │
                      │ event/state machine    │
                      │ SQLite sole writer     │
                      │ checkpoints / resume   │
                      │ spool / durable jobs   │
                      │ retrieval / FTS        │
                      │ provider routing       │
                      │ backup / repair        │
                      │ CLI / MCP / local API  │
                      └────────────┬───────────┘
                                   │
                         SQLite + FTS5
                         optional derived vector index

React / TypeScript viewer ── authenticated local API ───────┘
Cloudflare TypeScript Worker ── versioned sync contract ────┘
```

## Rust に置く範囲

| 領域 | 方針 |
|---|---|
| daemon lifecycle / single-instance | Rust Core |
| local RPC / MCP server | Rust Core |
| SQLite / FTS5 / migration | Rust Core |
| normalized event intake | Rust Core |
| continuity state machine | Rust Core |
| checkpoint claim / delivery / acceptance | Rust Core |
| spool / durable jobs / retry | Rust Core |
| backup / restore / repair | Rust Core |
| provider routing / timeout / circuit breaker | Rust Core |
| CLI / setup / doctor / rollback | Rust Core |
| local embedding execution | 原則 Rust 側の optional component |
| React viewer | TypeScript を維持し、静的 assets として配布可能 |
| Agent adapter / hooks | host-native な shell / JSON / TypeScript 等を許可 |
| OpenCode / Pi extension | TypeScript を許可 |
| Cloudflare Workers | TypeScript を許可 |
| schema / fixture / benchmark | runtime-neutral artifact を正本にする |

利用者の clean install では、Core Runtime の実行に Node.js、Bun、Python、Chroma、Redis、Postgres を必須としない。Viewer の build-time toolchain は runtime dependency と区別する。

## TypeScript 実装の位置付け

`vendor/codemem` を基盤とする現 TypeScript 実装は、Rust cutover 完了まで次の役割を持つ。

- Phase 1 safety boundary の canonical reference
- frozen RPC / writer / spool / error behavior の参照実装
- differential testing の oracle
- existing DB / spool / setup ownership の migration source
- Rust 実装へ適用する golden fixture を導出するための evidence source
- cutover 失敗時の期間限定 rollback target

一方、Rust 側で再実装予定の Core 領域へ、比較用fixtureなしに大規模な新規 product logicを追加しない。

TypeScript Core に許可する変更は原則として次に限定する。

- security / correctness / data-loss bug fix
- frozen contract を成立させるための修正
- runtime-neutral fixture / benchmark / migration support
- Rust shadow comparisonに必要なinstrumentation
- releaseまでユーザーを保護するために不可欠な保守

新規機能を TypeScript に先行実装する場合は、Rustへの二重実装コスト、fixture再利用、retirement条件をPR本文で明示する。

## ADR-003 との関係

ADR-003 で凍結した以下は、そのまま有効とする。

- `rpc-v1.md`
- `writer-boundary-v1.md`
- `spool-format-v1.md`
- `error-taxonomy-v1.md`
- G1–G7
- cold / warm start、RSS、ingest latency、kill recovery、spool replay、migration、artifact size等の比較指標
- Stage 2 shadow daemon
- migration / rollback evidence

変更されるのは判定の意味である。

### 旧判定

```text
Stage 1 が成功したら Rust 移行を Go
失敗したら TypeScript を継続
```

### 新判定

判定は ADR-003 の cutover gate と同じ 2 値にする。3 値にすると、同じ実測結果から違う結論を
導ける余地が残る。

```text
pass  = G1-G7 をすべて満たす
  -> cutover roadmap へ進む資格を得る（default 切替そのものではない）

defer = G1-G7 に 1 つでも欠けがある
  -> Core 1.0 の default は TypeScript のまま据え置く
```

`pass` = G1–G7 をすべて満たす——これが唯一の条件で、ADR-003 の判定規則と同一。性能差の大きさは追加の gate ではない。

`pass` は default 切替の十分条件ではない。**default 切替の条件は Cutover gate 1–10 をすべて満たし、かつ #84 の正本連鎖の改訂が完了していること**。Cutover gate 1–10 は下に列挙する
（behavioral gate、fault injection、migration / rollback、clean install、fail-open、
signed artifacts / SBOM、`doctor` など）。

`defer` の下でできることは次の 4 つに限る。

- 不合格の原因を直して再測定する
- Stage 1 の対象 slice を縮小して測り直す
- Rust 実装を shadow として動かし続ける（canonical writer は TS のまま。ADR-003 の決定 1）
- cutover を後続 release へ送る

**本番 cutover は含まない。** G1–G7 は Stage 1 prototype と default runtime の全体に対する gate で、
slice 単位で適用する規則を持たない（G3 は全 RPC method と全 TS adapter、G5 は clean install した
Core 全体、G7 は現行 harness 全体を対象にする）。部分 cutover を認めるなら、slice ごとの gate、
全体との composition 条件、canonical writer と rollback の境界、正本改訂の条件を別に定義し、
ADR-003 側も同じ per-slice contract へ改訂する必要がある。それは本 ADR の範囲外。

長期方向としての Rust Core を撤回するには、`defer` の記録ではなく別の owner ADR が要る。

## 採用理由

### 1. ローカル常駐 runtime の配布と運用を単純化する

単一または最小数のnative artifactにより、Core実行時のNode/Bun/Python依存、package-manager差異、background worker ownership、PATH上の重複runtimeを減らす。

### 2. lifecycle・永続化・復旧を一つの所有境界へ置く

single-instance、sole-writer、process lifecycle、transaction、spool、backup、repairを同じruntimeで所有し、adapterやviewerがcanonical DBを直接開かない構造を維持する。

### 3. Windows nativeを含むcross-platform lifecycleを正式対象にする

現TypeScript Local CoreはLinux専用であり、Windows / macOS対応はどちらの言語でも新規作業になる。Rust採用を理由に自動的に解決したとは扱わず、実機failure matrixをrelease gateにする。

### 4. 長期のfirst-party identityを明確にする

`vendor/codemem` は重要な移行資産だが、free-memの恒久的な製品中心ではない。Rust Coreをtop-level first-party runtimeにすることで、upstream provenanceを保ちつつ、free-mem独自のcontinuity contractと製品責任を明確にする。

## Rust 自体を差別化として扱わない

Rust、単一バイナリ、SQLite、FTS5、local-first、MCP、複数Agent対応は、複数の競合が公開資料上「提供している」とpositioningしている（install / 実挙動は未検証。#8 / #79 で実測する）。したがって、Rust化をmarketing上の唯一の優位性として扱わない。

free-mem の中核製品価値は次とする。

> **過去を検索するだけでなく、現在のworkspaceと未完了operationを照合し、安全に作業を続きから再開する coding-agent continuity runtime。**

特に次を差別化のrelease evidenceとして扱う。

- task-lineage scoped state
- workspace reconciliation
- unknown operationの`verify_first`
- exact-version capability evidence
- 同一 device 上で 1 つの checkpoint に対する未失効の active delivery attempt が最大 1 件
  （valid な reclaim 後の再配送と cross-device の fork は対象外。v6.1 §22.4 / §27.7）
- `claimed -> delivered -> engaged -> accepted` の分離
- wrong resume / unsafe replay / duplicate injectionのzero-tolerance gate
- provider停止時にも成立するdeterministic continuity
- TS / Rust / Agent間で共有する公開conformance fixture

## Cutover gate

Rustをdefault runtimeへ切り替える前に、最低限次を満たす。

1. ADR-003 G1–G7をすべてpassする。
2. TypeScript adapterを無改造でRPC接続できる。
3. fault injectionでdata loss、duplicate commit、split brainが0件。
4. migration、rollback、再migrationを実証する。
5. Linux / WSL / Windows native / macOSのsupport dispositionを実測する。
6. Core実行時にNode / Python等を要求しないclean installを証明する。
7. #8のbehavioral benchmarkを公開baselineに対して実行し、レポートを残す。合否を決めるのは
   #8のfrozen claude-mem non-inferiority gateと、#79のmanifestで事前に`releaseBlocking=true`と
   宣言された項目だけである（レポートの作成は必須、全baselineへの非劣性は合否条件ではない）。
8. Rust runtime停止時にもAgent本体がfail-openする。
9. signed artifacts、checksums、SBOM、ownership manifestを提供する。
10. `doctor`からruntime、schema、DB、spool、adapter、migration状態を説明できる。

## 帰結

- #1 は「Rust採否」ではなく「Rust Core cutover roadmap」のumbrellaへ更新する。
- #8 はclaude-mem単独比較から、役割別の直接競合baselineを含むbenchmarkへ拡張する。
- #9 はRust Coreのnamespace、installer、data-path migrationを前提に進める。
- #13のruntime-neutral continuity contractをRust実装の入力とする。
- Rust default切替前はTypeScriptをcanonical writerとしてshadow比較できる。
- cutover後もTypeScript referenceは期間限定で保持し、retirement条件を別途記録する。

## 非目標

- React viewerや全adapterをRustへ書き換えること
- 既存TypeScript実装を即時削除すること
- benchmarkやmigrationを省略したbig-bang rewrite
- Rustという理由だけで性能・安全性を主張すること
- Core 1.0へ全Agent・Cloud・Team機能を同時投入すること
- claude-mem、codemem、他OSSの非公開実装を推測・複製すること
