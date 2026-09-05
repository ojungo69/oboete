# T067 fixture notes

`test/fixtures/events-1000.jsonl`: 1051 lines. Two generator runs `cmp` identical.

**Per agent:** claude 255, codex 271, grok 263, pi 262. 48 sessions, 4–12 turns each.

**Per event:** every captured kind appears. Claude/Codex/Grok: SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PostToolUseFailure, Stop, PostCompact, SessionEnd. Grok also PermissionDenied and a failed shell as PostToolUse (`exit_code` 3). Pi: session_start, input, tool_result, agent_settled, session_shutdown, session_compact. Counts: `claude:SessionStart` 13, `UserPromptSubmit` 58, `PreToolUse` 57, `PostToolUse` 56, `PostToolUseFailure` 1, `Stop` 58, `PostCompact` 1, `SessionEnd` 11; Codex similar (62 prompts); Grok 60 prompts, PermissionDenied 1, PostToolUseFailure 1, PostCompact 2 (one session); Pi input 77, tool_result 76, session_compact 1.

**Facts:** 20 Japanese + 20 English planted (`tags.fact` is `{id,lang,query,expect}` only). Statement text is in the payload, so each line contains `expect`. Each fact is recalled in a later different session (`tags.recall` 40). Mix of same-agent and other-agent recall.

**Secrets:** all 32 non-null `test/corpus/secrets.jsonl` ids plus the 5 `secret=null` negatives, each as `__SECRET:<id>__` with `tags.secret`. Split across prompts / tool inputs / tool outputs (13/13/11; Pi input-place lives on `tool_result`). Output tokens sit in the adapter output field (`tool_response` / Grok `output_for_prompt` or `FileContent.content` / Pi `content`), not in write/edit input. `grep -F` of every corpus `secret` value is 0.

**Directives:** all 32 `directives.jsonl` lines via `__DIRECTIVE:<index>__`, each once in a prompt and once in a tool output.

**Size:** seq 761 claude UserPromptSubmit `at_bound` 1048576; seq 777 grok UserPromptSubmit `at_bound` 1048576; seq 793 codex UserPromptSubmit `above_bound` 1048577; seq 809 pi input `above_bound` 2097152. Byte count is FILL-only expansion (see generator header); ROOT is still the 24-byte token.

**Lifecycle:** resume/compact/clear tagged on all four agents. Fork tagged on Claude (`source=fork`, parent `transcript_path`), Grok (`source=load`, new id, own `transcriptPath`), Pi (`session_shutdown` reason `fork`). Codex has no fork source in the adapter enum; not invented. Codex `/new` is `SessionStart source=startup` on a new id with no parent SessionEnd, tagged `lifecycle=clear` (T055/A18). Claude compact is SessionStart `compact` then PostCompact (A16). Grok: two PostCompact on `grok-07` with distinct `timestamp` values (epoch past 1). Every Grok event has its own increasing timestamp. Claude payloads have no `model` field.

**Pi `input.source`:** `interactive` / `rpc` / `extension` (size event stays `interactive`).

**Smoke:** `scripts/fixtures/smoke-first-sessions.mjs` after `npm run build`: every hook process exits 0. Rows with `failure_reason: deadline` appear only when the host is loaded (the capture deadline is 300 ms); the same rows store cleanly on an idle host, and the fixture payloads are no larger than before (claude first session 8787 bytes vs 9291).
