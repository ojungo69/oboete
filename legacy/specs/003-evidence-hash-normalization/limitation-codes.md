# limitationCodes: 散文との対応表（T003 の成果物）

`limitations` の散文は fixture 内に残し、matrix へは出さない（data-model.md §5.3）。
matrix 側の `CapabilityEvidence.limitations` には、この表で決めたコードだけを載せる。

**コードは手で並べず、既存 fixture の散文を全件列挙して 1 つずつ割り当てた。**
散文は **27 件**（top-level 11 件 / event ごと 16 件）で、
コードは **22 種**。同じ観測事実を述べた散文には同じコードを当てている。

`key` は散文の SHA-256 先頭 8 桁。散文を書き換えると key が変わるので、
この表と fixture の対応が崩れたことが分かる。

## 配置の規則

`limitationCodes` は `limitations` と**同じ場所**（fixture の top-level と `observedEvents[]` ごと）に置き、
**同じ長さで位置対応**させる。散文を足してコードを足し忘れた状態を検査で落とすため。

## コードの一覧（`capability.schema.json` の enum になる）

```json
[
  "assistant-completion-synthesized-from-stop",
  "codex-home-in-tmp-warns",
  "empty-tool-response-ambiguous",
  "failure-phase-not-directly-observable",
  "failure-phases-not-reached",
  "headless-only-no-tty",
  "hook-timeout-not-observable",
  "hooks-config-requires-table-form",
  "interrupt-evidence-in-subset-of-captures",
  "interrupt-observed-via-signal-only",
  "interrupt-requires-pattern-synthesis",
  "post-tool-use-absent-on-failure",
  "post-tool-use-fires-on-failure-without-status",
  "session-end-reason-always-other",
  "session-start-injection-echoed",
  "stop-not-fired-on-sigint",
  "subagent-identity-present-in-payload",
  "subagent-internals-not-visible-to-parent",
  "subagent-start-appears-as-agent-tool",
  "tool-start-payload-carries-ids",
  "turn-boundary-native-via-turn-id",
  "turn-boundary-via-shared-prompt-id"
]
```

## 散文との対応

| fixture | 場所 | key | code | 散文 |
|---|---|---|---|---|
| `claude/injection-and-subagent` | `limitations[0]` | `bb06b6ad` | `session-start-injection-echoed` | 注入実証: hook stdout に置いた token `RIG_INJECT_5f3a9` を子セッションが逐語で復唱した（session 開始時点で注入が届くことの real-cli-e2e 証跡） |
| `claude/injection-and-subagent` | `limitations[1]` | `9c5db4f0` | `subagent-internals-not-visible-to-parent` | subagent: 親プロセスの hook に SubagentStop が 1 件届く。**subagent 内部の tool 使用・prompt は親 hook に現れない**（親からは Agent tool 1 回として見える） |
| `claude/injection-and-subagent` | `limitations[2]` | `bec66a6f` | `subagent-identity-present-in-payload` | subagent 固有 id は **存在する**: SubagentStop payload に `agent_id`（例 aa16b2026df287771）/ `agent_type`（general-purpose）/ `agent_transcript_path`（subagent 専用 JSONL）があり、session_id は親と共有。[2026-08-12 訂正: 初版は『subagent 固有 id は hook payload に無い』と誤記していた] |
| `claude/injection-and-subagent` | `observedEvents.tool_started.limitations[0]` | `b132463d` | `subagent-start-appears-as-agent-tool` | subagent 起動も親側 PreToolUse として観測される。**tool_name は `Agent`**（`Task` ではない — 生 capture で確認）。同 run には ToolSearch も現れる |
| `claude/interrupt-and-hook-timeout` | `limitations[0]` | `4a3d1b9a` | `hook-timeout-not-observable` | hook timeout: settings の timeout=10 に対し hook を 15s ブロックさせても、捕捉 event 数・session 完了は正常時と同一（4 events）。hook の遅延は session を壊さないが、hook 側の timeout 超過は payload に現れない |
| `claude/interrupt-and-hook-timeout` | `limitations[1]` | `98b601f2` | `interrupt-observed-via-signal-only` | interrupt fixture は timeout(1) 由来の SIGINT。ユーザーの Ctrl-C 経路（TTY）は未観測 |
| `claude/interrupt-and-hook-timeout` | `observedEvents.session_interrupted.limitations[0]` | `d2a4f30d` | `stop-not-fired-on-sigint` | SIGINT 中断時は **Stop が発火せず** SessionEnd のみ届く（interrupt2/3/4 の 3 回で再現。raw/claude-interrupt{2,3,4}.jsonl）。ただし SessionEnd.reason は正常終了時と同じ 'other' で、中断と正常終了を payload だけでは区別できない |
| `claude/interrupt-and-hook-timeout` | `observedEvents.session_interrupted.limitations[1]` | `a5f1cad8` | `interrupt-requires-pattern-synthesis` | 区別には『Stop 無しで SessionEnd』というパターン合成が必要（本 fixture の根拠） |
| `claude/interrupt-and-hook-timeout` | `observedEvents.session_interrupted.limitations[2]` | `55757c8a` | `interrupt-evidence-in-subset-of-captures` | raw/claude-interrupt.jsonl は deadline 前に完走した run で Stop を含む。中断形の証跡は interrupt2/3/4 の 3 本 |
| `claude/lifecycle-basic` | `limitations[0]` | `b6466338` | `headless-only-no-tty` | headless (-p) run のみ。対話 TTY セッションは未観測 |
| `claude/lifecycle-basic` | `observedEvents.assistant_completed.limitations[0]` | `7656750b` | `assistant-completion-synthesized-from-stop` | Stop payload の last_assistant_message から復元。native な assistant_completed hook は無い |
| `claude/lifecycle-basic` | `observedEvents.turn_completed.limitations[0]` | `07d24f27` | `turn-boundary-via-shared-prompt-id` | Stop = turn 終端として扱う。turn 専用の native event は無いが、UserPromptSubmit / Stop / SessionEnd が同一 `prompt_id` を共有するため turn の対応付け自体は native に可能（生 capture で 3 event 共有を確認） |
| `claude/lifecycle-basic` | `observedEvents.session_ended.limitations[0]` | `eef3bf87` | `session-end-reason-always-other` | reason は headless 正常終了でも 'other'。終了理由の判別には使えない |
| `claude/tool-failed-executed` | `limitations[0]` | `54de7bee` | `failure-phases-not-reached` | permission_denied / schema_invalid / unknown_tool phase は本 fixture では未観測（tool-denied 実行では Claude が許可済み範囲に収まり deny 分岐に到達せず） |
| `claude/tool-failed-executed` | `observedEvents.tool_failed.limitations[0]` | `fcf7a260` | `post-tool-use-absent-on-failure` | **PostToolUse は tool 失敗時に発火しない**（2 回実行で決定的に再現: PreToolUse のみ、PostToolUse 無し）。失敗の検出は PreToolUse の未完了 + Stop の last_assistant_message からの合成に依存する |
| `claude/tool-failed-executed` | `observedEvents.tool_failed.limitations[1]` | `b555c98e` | `failure-phase-not-directly-observable` | failurePhase の直接判別は不可。executed failure と他 phase を hook 単体では区別できない |
| `codex/injection` | `limitations[0]` | `95db0ff3` | `session-start-injection-echoed` | 注入実証: hook stdout の token `RIG_INJECT_5f3a9` を子セッションが逐語で復唱（real-cli-e2e 証跡）。Claude Code と同じく session 開始時点の注入が届く |
| `codex/lifecycle-basic` | `limitations[0]` | `6c7f466d` | `hooks-config-requires-table-form` | config は features.hooks=true + [[hooks.X]] 形式が必須（トップレベル `hooks = true` は同名衝突で config 全体が parse error になる — rig で実証） |
| `codex/lifecycle-basic` | `limitations[1]` | `44932ae6` | `codex-home-in-tmp-warns` | CODEX_HOME を /tmp 配下に置くと PATH alias 作成を拒否する警告が出るが、実行自体は継続する |
| `codex/lifecycle-basic` | `observedEvents.assistant_completed.limitations[0]` | `67e98877` | `assistant-completion-synthesized-from-stop` | Stop.last_assistant_message から復元 |
| `codex/lifecycle-basic` | `observedEvents.turn_completed.limitations[0]` | `4c75ac08` | `turn-boundary-native-via-turn-id` | Stop payload に `turn_id` があり turn 境界が native に識別できる。[2026-08-12 訂正: 『Claude 側には無い』は誤り — Claude は UserPromptSubmit/Stop/SessionEnd が同一 `prompt_id` を共有しており turn 対応付けは可能。差は id の名称と、Codex の SessionEnd には turn_id が無い点] |
| `codex/lifecycle-basic` | `observedEvents.session_ended.limitations[0]` | `3a11608a` | `session-end-reason-always-other` | reason は正常終了でも 'other' |
| `codex/tool-lifecycle-and-failure` | `limitations[0]` | `71e792c3` | `failure-phases-not-reached` | permission_denied / schema_invalid / unknown_tool は未観測（rig は approval_policy=never / sandbox 指定で deny 分岐に到達しない） |
| `codex/tool-lifecycle-and-failure` | `observedEvents.tool_started.limitations[0]` | `8fa08c8e` | `tool-start-payload-carries-ids` | payload に tool_use_id / turn_id / tool_input あり |
| `codex/tool-lifecycle-and-failure` | `observedEvents.tool_failed.limitations[0]` | `4a7f5d30` | `post-tool-use-fires-on-failure-without-status` | **Codex は tool 失敗時も PostToolUse を発火する**（Claude Code とは逆の挙動）。ただし payload に exit code / error フィールドは無く、成功時 tool_response="rigtool\n" に対し失敗時 tool_response="" という違いしかない |
| `codex/tool-lifecycle-and-failure` | `observedEvents.tool_failed.limitations[1]` | `fa5295d6` | `empty-tool-response-ambiguous` | 空 tool_response は『失敗』と『出力の無い成功』を区別できないため、失敗判定は transcript 参照か上位層の推定が必要 |
| `codex/tool-lifecycle-and-failure` | `observedEvents.tool_failed.limitations[2]` | `c1dd4f66` | `failure-phase-not-directly-observable` | failurePhase の直接判別は不可 |
