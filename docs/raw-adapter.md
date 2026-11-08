# An agent with no adapter of its own (X18)

Parity row X18, decision 70's item after X17 and X26. claude-mem's hook command takes a platform
name and falls back to its raw adapter for any name it has no adapter for (`src/cli/adapters/`
at v15.12.0): it reads the common field names, snake_case or camelCase, and since 15.1 the turn's
last answer from the Stop payload, so such a session gets a summary. Before this change,
`oboete hook <agent> <event>` recorded nothing for a name outside the agents `oboete setup` sets
up.

## As built

- R1. **Which names.** A name outside `setup::AGENTS` that is a word of 1 to 32 lowercase ASCII
  letters, digits, `-` and `_`, starting with a letter, is an agent with no adapter
  (`hook::raw_agent`). Any other name records nothing, as before. The name is the person's, from
  their hook command, never the payload's; records carry it as their agent.
- R2. **The payload.** It is read with claude-mem's raw field names (`hook::raw_fields`): a
  camelCase `sessionId`, `toolName`, `toolInput`, `toolResponse`, `toolUseId` or
  `lastAssistantMessage` stands for the snake_case field when that one is missing. A missing
  `cwd` is the hook's working directory, as for every agent. No transcript is read, its format
  being unknown, so a `Stop` with an empty answer records nothing.
- R3. **The events**, by Claude Code's names, which claude-mem's raw hosts use (`RAW_EVENTS`):
  `SessionStart` (a start), `UserPromptSubmit` (a prompt), `PostToolUse` and `PostToolUseFailure`
  (a tool call with its result), `Stop` (the turn's last answer) and `SessionEnd`. Others record
  nothing.
- R4. **Nothing is given back.** The hook writes nothing to stdout for such a host, not even the
  line a failed recording gives a known agent at its session start: no answer format of its is
  known, and a hook must never break its agent. No manifest is built and no prompt point runs, so
  no claim is counted as shown to it. Difference from claude-mem, which passes its result through:
  the session start's memory and the per-prompt block do not reach such a host.
- R5. **Capture's settings hold** as for every agent: exclusions, mute, `store_prompts`, masking
  and forget. Copies of a turn's files (X23) stay for the agents oboete sets up.
- R6. **Setting it up.** `oboete setup` sets up no such agent. A host that runs Claude Code's
  hook events points each one at `oboete hook <name> <Event>`, with the event's JSON on stdin, for
  example `oboete hook kimi UserPromptSubmit`.

## Acceptance tests

1. A host's start, prompt, tool call (camelCase) and `Stop` answer are recorded under its name and
   session; `PreToolUse` and `PreCompact` record nothing; an empty `Stop` with a transcript path
   records nothing (`a_host_with_no_adapter_is_recorded_under_its_name`).
2. A name that is not such a word (`Kimi`, `a b`, `1kimi`, empty, 33 letters) records nothing and
   writes no store; a known agent's name is never such a host
   (`a_hook_name_that_is_no_word_records_nothing`).
3. With a store that cannot be opened, a known agent's session start says the failure and such a
   host is given nothing (`a_host_with_no_adapter_is_given_nothing_even_a_failure`).
4. An excluded folder keeps such a host out (`an_excluded_folder_keeps_a_host_with_no_adapter_out`).
5. The curator reads its prompt as the developer's and its answer as the assistant's
   (`curate::tests::a_host_with_no_adapter_is_curated_as_any_agent`).
6. The known agents' behaviour is unchanged (the existing suites).
