# Phase 0B contract harness

v6.1 §29 Phase 0B / §7.2 capability matrix を real CLI fixture から組み立てる最小 harness。依存ゼロ・Node 24 直接実行。

```bash
node --experimental-strip-types harness/assemble.ts harness/fixtures/claude harness/matrix/claude.json
node --experimental-strip-types harness/assemble.ts --self-test     # PASS を出す
node --experimental-strip-types harness/phase1-static-scan.ts       # T053 sole-writer static gate
node --experimental-strip-types harness/phase1-static-scan.ts --self-test
harness/rig/rig.sh setup && harness/rig/rig.sh claude-run <label> "<prompt>"   # 隔離 capture
harness/sidecar/run-tests.sh                                        # supervisor self-check
harness/sidecar/hostile-e2e.sh                                      # hostile 設定下の side effect 検査
harness/rig/rig.sh teardown                                         # 資格情報コピーごと削除
```

- `schema/` — Capability / CaptureFixture 型（§7.2 逐語）+ JSON Schema
- `rig/` — 隔離 capture rig。scratch HOME / `CLAUDE_CONFIG_DIR` / `CODEX_HOME` + capture 専用 hook のみ + `AGENT_MEMORY_INTERNAL_RUN=1` + 使い捨て workspace。ユーザー実環境の plugin・hook・メモリ DB を汚染しない
- `fixtures/<cli>/*.json` — CaptureFixture（人手で所見を確定したもの）。`fixtures/<cli>/raw/` は rig が吐いた生 JSONL
- `matrix/` — assemble 出力（version-pin 付き AdapterCapabilities）
- `sidecar/` — §13.6 supervisor（deadline / process-group kill / reap / 残存検査）と hostile fixture E2E、認定判定 `certification-decision.md`
- `phase1-static-scan.ts` — production source の DB opener / DDL / deep import / 旧 direct path / 未認定 sidecar / public wrapper と T029 disposition の完全一致を検査。違反時 exit 1

assemble の方針: 観測できた EventKind のみ `native`、合成が必要なものは fixture 側で `capability: "synthesized"` + `sourceEvents` を明示（空だと検証エラー）。未観測 cell は `unknown` のまま残す（HI-23。Tier A は宣言しない）。fixture 間で `nativeVersion` が食い違えば version-pin 違反として exit 1。同じ `fixtureId` を持つ fixture が 2 つあっても exit 1（cell 間の「同一の実測か」の照合キーであるため）。

高位 cell（`sessionStartInjection` / `promptAwareInjection` / `promptDeliveryBeforeModel` /
`compactSingleDelivery` など）は fixture の `highLevel` が書いたときだけ `unknown` を上書きする。
複数 fixture が同じ cell を観測した場合は強いほう（native > synthesized > unsupported）を残し、
`observed in <fixtureId>` は統合する（ファイル名順で先の fixture の provenance を消さない）。

`resumeDeliveryStrategy` は上の cell から導出する生成値で、手で書かない。tier の定義は
addendum §8 のとおり: `native_prompt_gate` は pre-model 配送 cell が native かつ実 CLI 実測、
`next_prompt_synthesized` は pre-model 配送と prompt-aware injection の **両方** が同一
fixture / evidence hash の実測で synthesized、`session_start_full` は SessionStart のみ実測、
既定は `manual_only`。`capabilityHashInputs` は hash の入力列（exact version と各 fixture の
evidence hash）で、hash 値そのものは scenario manifest が揃う Task 11 で計算する。
