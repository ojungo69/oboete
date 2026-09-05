# T067 fixture notes

`test/fixtures/events-1000.jsonl`: 1050 lines. Two generator runs `cmp` identical.

**Per agent:** claude 255, codex 271, grok 262, pi 262. 48 sessions, 4–12 turns each.

**Per event:** every captured kind appears. Claude/Codex/Grok: SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, Stop, PostCompact, SessionEnd. Grok also PermissionDenied and a failed shell as PostToolUse (`exit_code` 3). Pi: session_start, input, tool_result, agent_settled, session_shutdown, session_compact. Counts: `claude:SessionStart` 13, `UserPromptSubmit` 58, `PreToolUse` 57, `PostToolUse` 56, `PostToolUseFailure` 1, `Stop` 58, `PostCompact` 1, `SessionEnd` 11; Codex similar (62 prompts); Grok 60 prompts, PermissionDenied 1, PostToolUseFailure 1; Pi input 77, tool_result 76, session_compact 1.

**Facts:** 20 Japanese + 20 English planted (`tags.fact`), each recalled in a later different session (`tags.recall` 40). Mix of same-agent and other-agent recall.

**Secrets:** all 32 non-null `test/corpus/secrets.jsonl` ids via `__SECRET:<id>__`, 8 per agent, split across prompts / tool inputs / tool outputs. `grep -F -c` of every corpus `secret` value is 0. Null-secret (negative) lines were not required and are not planted.

**Directives:** all 32 `directives.jsonl` lines via `__DIRECTIVE:<index>__`, each once in a prompt and once in a tool output.

**Size:** seq 760 claude UserPromptSubmit `at_bound` 1048576; seq 776 grok UserPromptSubmit `at_bound` 1048576; seq 792 codex UserPromptSubmit `above_bound` 1048577; seq 808 pi input `above_bound` 2097152. Byte count is FILL-only expansion (see generator header); ROOT is still the 24-byte token.

**Lifecycle:** resume/compact/clear tagged on all four agents. Fork tagged on Claude (`source=fork`), Grok (`source=load`, new id, parent transcript), Pi (`session_shutdown` reason `fork`). Codex has no fork source in the adapter enum; not invented. Codex `/new` is `SessionStart source=startup` on a new id with no parent SessionEnd, tagged `lifecycle=clear` (T055/A18). Claude compact is SessionStart `compact` then PostCompact (A16). Grok PostCompact has no compact SessionStart companion.

**Smoke:** `scripts/fixtures/smoke-first-sessions.mjs` after `npm run build`: 68 hook calls, 84 `raw_events`, no `failure_reason`. Claude/Grok use `--agent claude-or-grok` (Grok sets `GROK_HOOK_EVENT`); `--agent grok` would store `unknown`. Extra check of failure/compact/PermissionDenied/session_compact: 9 rows, no `failure_reason`.

Nothing else needed a change outside `scripts/fixtures/` and `test/fixtures/`.
