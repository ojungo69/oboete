# Agent adapters: Antigravity CLI, Pi, OpenCode, Cursor (research 2026-09-23)

Researched against the versions installed on the owner's PC (read-only), then fact-checked by a second agent.
Each section ends with the fact-check's corrections; where a correction disagrees with the text above it, the
correction wins. Points marked uncertain must be confirmed in one live session before the adapter ships
(docs/m1.md decision 12).

## Antigravity CLI (agy)

Installed: 1.2.9 (`agy --version`; binary /home/jura/.local/bin/agy is a stripped Go ELF, go1.28 RC03; language server log says "Language server version: 1.2.9")

### Verified live (2026-09-24, agy 1.2.9)

A throwaway named hook in `~/.gemini/config/hooks.json` dumped every payload of four `agy --print` runs to files and was removed right after (the owner's global file did not exist before and does not now). Results override the rest of this section:

- **Payloads carry only the common fields.** Every event had `conversationId`, `workspacePaths`, `transcriptPath` (always `…/brain/<id>/.system_generated/logs/transcript_full.jsonl`, the untruncated one), `artifactDirectoryPath`, `modelName`. Event extras: PreInvocation/PostInvocation `invocationNum` (0-based, counts up across the whole run) and `initialNumSteps`; PostToolUse `stepIdx`, `toolCall{name,args}`, `error`; Stop `executionNum`, `fullyIdle`, `terminationReason` (seen: `NO_TOOL_CALL`), `error`. **There is no `lastUserInput`, `executionId`, `result`, `finalModelOutput` or `parentConversationId`** (the binary's proto fields are not filled). So the prompt, tool output and assistant text must come from `transcriptPath`.
- **transcript_full.jsonl** lines are steps: `{"step_index", "source", "type", "status", "created_at", "content", "thinking"?, "tool_calls"?}`. Lines are not in `step_index` order. The prompt is the `USER_INPUT` step (`source: USER_EXPLICIT`), wrapped in `<USER_REQUEST>…</USER_REQUEST><ADDITIONAL_METADATA>…` (plus `<USER_SETTINGS_CHANGE>` on the first turn). A tool's output is the step whose `step_index` equals the PostToolUse `stepIdx` (`content` like `The command exited with code 2.\nOutput:\n…`). A failed tool call can show up as a `GENERIC` step with `status: ERROR` and an `error` field while the hook's `error` is empty. The assistant's answer is the last `PLANNER_RESPONSE` with `content` (some have only `tool_calls`).
- **`workspacePaths` is empty in `agy --print` without `--new-project`**, and then the model's tools run in `$HOME`, not the launch directory. With `--new-project` it is `["/abs/launch/dir"]`, a plain path. `conversation_summaries.db` shows interactive and `--new-project` conversations with a workspace URI. oboete should skip events with no workspace (no repo to file them under) rather than guess.
- **SessionStart fires** (flat array works), once, before the first PreInvocation.
- **PreInvocation injection works**: printing `{"injectSteps":[{"ephemeralMessage":"…"}]}` made the model answer from that text on the first invocation. Printing `{}` is accepted for every event registered here.
- **Do not register PreToolUse**: a PreToolUse handler that prints `{}` made agy refuse the tool call ("Pre-tool hook rejected").
- **`OBOETE_SKIP=1` set on the `agy` process reaches the hook** (inherited through the language server and `sh -c`), next to `ANTIGRAVITY_CONVERSATION_ID`. The self-capture guard can rely on it.
- **Hooks run with the working directory `~/.gemini/config`**, as documented. A hook command under the Claude Code scratch directory (`/tmp/claude-1000/…`) did not run at all; one under `$HOME` did. Register the oboete binary by its absolute path.
- **Workspace `.agents/hooks.json` was not picked up** by `agy --print` in a git-initialized directory under a trusted root (`/hooks` listed only the plugin hook). Use the global file.
- Not verified: interactive TUI sessions (a tmux attempt produced no output), resume, and whether an ephemeral message is still visible on later turns.

### Implemented (2026-09-24)

- Implemented SessionStart, PreInvocation prompt capture, PostToolUse/failure, and Stop capture with prompt recovery. Workspace paths and `file://` URIs identify the repo; empty workspaces are skipped. Transcript reads use a single 256 KiB tail and step indices. Prompts reuse privacy/redaction handling; the transactional `sessions.last_prompt_step` cursor survives raw-event cleanup.
- PreInvocation injects once through `injected_at` and `injectSteps[].ephemeralMessage`; all other responses, including skips and errors, are `{}`. Setup/`--remove`/`all` and doctor cover hooks and MCP with merge, one backup, and both files staged before either is replaced (an emptied file stays `{}`). PreToolUse is never registered. Windows rejects spaces and shell metacharacters in executable/custom-home paths.
- Tests use the anonymized fixtures and temporary directories. Still unverified: interactive TUI sessions, resume, ephemeral-message visibility on later turns, and native Windows execution.

### Mechanism

JSON command hooks in a `hooks.json` file. Global path: `~/.gemini/config/hooks.json`. It does not exist on this PC yet. Changelog: "/hooks wrote to ~/.gemini/antigravity-cli/hooks.json instead of the shared ~/.gemini/config/hooks.json", fixed. Other places agy loads hooks from: workspace `<repo>/.agents/hooks.json` (also `.agent/`, `_agents/`, `_agent/`; only after the folder is trusted) and plugin `plugins/<name>/hooks.json`. Do not use `~/.gemini/settings.json`, which is the Gemini CLI path.

File shape (builtin docs/hooks.md, the log line "loaded %d named hooks from %d hooks.json file(s)", and the JSONHookSpec struct whose reflect fields are Enabled/PreToolUse/PostToolUse/PreInvocation/PostInvocation/SessionStart/Stop). Each top-level key is a HOOK NAME, and its value is a spec object:
{"oboete": {
  "PreInvocation": [{"type":"command","command":"/abs/oboete hook agy PreInvocation","timeout":10}],
  "PostToolUse":  [{"matcher":"*","hooks":[{"type":"command","command":"/abs/oboete hook agy PostToolUse","timeout":10}]}],
  "Stop":         [{"type":"command","command":"/abs/oboete hook agy Stop","timeout":10}]
}}
- Tool events (PreToolUse/PostToolUse) are grouped: `matcher` is a regex on the tool name ("*" or "" = all), and handlers go under `hooks`.
- PreInvocation/PostInvocation/Stop are flat arrays of handlers.
- Each handler has `type` ("command"; "prompt" also exists in 1.2.9), `command`, and `timeout` in SECONDS (default 30).
- Named hooks from several files are merged and run in order.
- `"enabled": false` on the named hook turns it off.
- The named key "oboete" is our ownership marker. `setup --remove` just deletes that key, so no command-shape matching is needed.
- Commands run via `sh -c` on Unix and `cmd /c` on Windows. `~` is expanded.
- The hook's working directory is the directory that holds hooks.json (`~/.gemini/config`), NOT the workspace.
- Hooks run synchronously and block the agent loop.
- There is no event-name field in stdin, so the event must come from argv: `oboete hook agy <Event>` fits decision 1.
- claude-mem 13.25.3 writes event names as top-level keys with matcher groups on every event. That was written against 1.2.1 and contradicts the shipped docs, so do not copy it.

### Events

| Event | oboete event | Payload | Injection |
| --- | --- | --- | --- |
| PreInvocation | SessionStart (context injection, once per conversation) + UserPromptSubmit (prompt capture) | stdin is flattened camelCase protojson: HookArgsCommon fields plus the event's args at top level (binary: 'failed to marshal flattened hook args'). Common fields (decoded exa.hooks_pb.HookArgsCommon): conversationId (use as session_id), workspacePaths[] (use [0] as cwd; the doc example shows a plain path, but accept a file:// URI too), transcriptPath (brain/<conversationId>/.system_generated/logs/transcript.jsonl), artifactDirectoryPath, executionId (one per turn), modelName, isBattleMode, lastUserInput, agentName, parentConversationId (non-empty = subagent). Event args: invocationNum, initialNumSteps. Fires before EVERY model call, so it fires several times per turn. lastUserInput is likely the raw USER_INPUT text, wrapped as <USER_REQUEST>…</USER_REQUEST><ADDITIONAL_METADATA>…</ADDITIONAL_METADATA>[<USER_SETTINGS_CHANGE>…]. The transcripts show that wrapper; whether lastUserInput carries it is unverified. oboete must EXTRACT the inner USER_REQUEST text, not strip blocks. Store the prompt once per turn: on the first PreInvocation seen for a new executionId (or invocationNum==first, but the numbering base is unverified). | Yes. Print {"injectSteps":[{"ephemeralMessage":"<context>"}]}. Other documented step kinds: {"userMessage":"…"} and {"toolCall":{…}}. The proto also has systemMessage/hookUserMessage/hookEphemeralMessage/checkpoint, which are undocumented. Gate with sessions.injected_at, the same as the Grok PreToolUse pattern in hook.rs; this also gives decision 3 (no re-inject on resume, since conversationId is reused). Output is strict protojson: do NOT print hookSpecificOutput/additionalContext (claude-mem #4057: unknown fields caused the whole output to be discarded). 1.2.9 has jsonhook.dropUnsupportedFields, but don't rely on it. With nothing to inject, print {}. Limit: EPHEMERAL_MESSAGE_PERSISTENCE_LEVEL_LATEST_ONLY exists in the binary, so a once-only ephemeral message may be pruned after the first model call. Ephemeral steps do persist in the transcript as EPHEMERAL_MESSAGE. No documented size cap. |
| PostToolUse (grouped, matcher "*") | PostToolUse, or PostToolUseFailure when `error` is non-empty | Common fields + stepIdx, toolCall{name, args(Struct)}, error, result (decoded PostToolHookArgs, 1.2.9). The shipped docs only show stepIdx+error. The binary has hookutils.GetToolOutputFromStep to fill `result`, but whether it is populated is unverified. Fallback: read the step at stepIdx from transcript_full.jsonl. Its `content` holds the tool output, e.g. RUN_COMMAND 'The command exited with code 0. Output: …'. transcript.jsonl truncates (truncated_fields); transcript_full.jsonl does not. Tool names are the lowercased step type without CORTEX_STEP_TYPE_: run_command, view_file, find, grep_search, list_directory, code_action, search_web, invoke_subagent, mcp tools, etc. | No context channel (PostToolHookResult has only overwrite_result, which rewrites the tool result). Print {}. |
| Stop (flat) | Stop (the per-turn assistant final message; triggers spawn_observe) | Common fields + executionNum, terminationReason (model_stop \| max_steps_exceeded \| error), error, fullyIdle, finalModelOutput (decoded StopHookArgs). Fires once per turn/execution, not once per session. The changelog says hooks.json Stop hooks now run before the built-in termination checks. Fallback for the assistant text: the last PLANNER_RESPONSE.content in transcript_full.jsonl. Compaction summaries have no hook; they appear as CHECKPOINT steps ('{{ CHECKPOINT n }}\n **The earlier parts of this conversation have been truncated…**') in the transcript. Most sessions get a 'CHECKPOINT 0' at step ~3 that is not a real compaction, so if you capture on Stop, take only CHECKPOINT steps after the first PLANNER_RESPONSE, deduped by step_index. | Print {}. NEVER print decision "continue": that blocks the stop and re-enters the loop, and `reason` is injected as a system message. |
| SessionStart | (optional) SessionStart injection; not needed | Present in the 1.2.9 binary (JSONHookSpec field, hookcaller.CallSessionStartHook, 'failed to construct JSON session-start hook %q', SessionStartHookArgs has no fields beyond common) but ABSENT from the shipped docs/hooks.md. Unknown: when it fires (new conversation only? resume? -p?) and whether it is flat or grouped. The live superpowers plugin hooks.json uses grouped SessionStart with matcher 'startup\|clear\|compact'; it parses (1 named hook), but the logs show no execution evidence. | SessionStartHookResult{injectSteps}. Not recommended until verified; PreInvocation + injected_at covers it. |
| PostInvocation | not registered (Stop already carries finalModelOutput) | Common fields + invocationNum, initialNumSteps, modelOutput, modelThinking. Runs after every model call. | injectSteps + terminationBehavior (force_continue\|terminate). Print {} if ever used. |
| PreToolUse | not registered (nothing to store; injection is done in PreInvocation, so Grok's PreToolUse trick is unnecessary) | Common fields + toolCall{name,args}, stepIdx. | No context channel. The output is a permission decision (allow/deny/ask/force_ask) + overwrite; it would add a synchronous hop per tool and could interfere with permissions. |

### What is missing and the workaround

1. No SessionEnd. Treat each Stop as a turn end (spawn_observe as for Claude Code). In -p mode, the one Stop with fullyIdle:true is effectively the end. There is no session-close signal, so rely on oboete's idle/pending summarization.
2. No UserPromptSubmit. Use lastUserInput on the first PreInvocation per executionId. Fallback: the last USER_INPUT (source USER_EXPLICIT) in transcript_full.jsonl. Either way, extract the <USER_REQUEST> inner text; the remaining blocks are agy metadata.
3. No PostCompact. Compaction is only visible as CHECKPOINT steps in transcript_full.jsonl (optional tail-read on Stop, like last_assistant_in_transcript for Codex). The SDK proto (genai OnCompactionArgs) exists but is not reachable from hooks.json.
4. No event-name field. The event comes from argv (`oboete hook agy <Event>`).
5. No cwd field, and the hook's own cwd is ~/.gemini/config. handle()/is_agent_internal must read workspacePaths[0] (array, possibly a file:// URI). Today they read only cwd/workspaceRoot.
6. The output formatter needs an agy branch: {"injectSteps":[{"ephemeralMessage":…}]} or {}. No hookSpecificOutput, and never empty stdout (print {}).
7. Self-capture. There is no flag to disable hooks in `agy -p`. Hooks are active headless: every oboete summarizer run logged 'loaded 1 named hooks from 1 hooks.json file(s)' with workspaceDirs=[/tmp/oboete-cli-*]. The language server is a per-run child process, and OBOETE_SKIP=1 on the agy process should be inherited (hooks run via sh -c with an extra ANTIGRAVITY_CONVERSATION_ID env), but inheritance is unverified. Second guard: skip when workspacePaths[0] starts with std::env::temp_dir()/oboete-cli-. conversation_summaries.db shows every summarizer conversation's workspace_uris = file:///tmp/oboete-cli-<id>.
8. Subagents (INVOKE_SUBAGENT) get their own conversationId, with parentConversationId set in the hook common fields. Recommendation: skip events whose parentConversationId is non-empty. The parent's tool call records the delegation, and storing the subagent's Stop would trigger early summaries.
9. Side effect (not capture): each `agy -p --new-project` summarizer run leaves a conversation in ~/.gemini/antigravity-cli/brain/, a row in conversation_summaries.db, a project file in ~/.gemini/config/projects/, and a log file. That is 60+ already.
10. Unverified until one live session (decision 12 already requires one): whether lastUserInput/result/finalModelOutput are populated, the invocationNum base, the wrapper in lastUserInput, and whether the injected ephemeral message is still visible to the model on turn 3 (LATEST_ONLY persistence). A fallback plan is in `uncertain`.

### MCP registration

Global stdio MCP config: `~/.gemini/config/mcp_config.json` (builtin docs/mcp_servers.md). Shape: {"mcpServers": {"oboete": {"command": "/abs/path/oboete", "args": ["mcp"], "env": {…optional}}}}. The file exists on this PC with claude-mem, codex-security, github, github_github {disabled:true}, and gitnexus, so merge and keep the others. Take a .oboete.bak once and write atomically (same pattern as toml_mcp/claude_mcp). A per-server `"disabled": true` exists (github_github), so doctor should check for it, like mcp_disabled for Codex. `agy mcp add [--env K=V] <name> <cmd> [args…]` / `agy mcp remove <name>` would also write it. Prefer writing the file directly: the CLI starts an agy process with the hook system loaded (the same trap as `claude mcp` spinning SessionStart, decision 2), or at least pass OBOETE_SKIP=1. claude-mem also dual-writes `~/.gemini/antigravity/mcp_config.json`, which exists here and belongs to the Antigravity IDE/2.0 app, not the CLI; not needed for agy. Unknown: which cwd agy gives the MCP child. oboete mcp scopes searches to its cwd repo, so check this in the live session; if it is not the workspace, the model must pass `repo`. Doctor probe with no quota and no agent turn (changelog 1.1.x): `agy -p "/hooks" --output-format json` lists the parsed hooks. Run it with OBOETE_SKIP=1.

### Headless runs and self-capture

`agy -p/--print "<prompt>"` with `--output-format text|json|stream-json`, `--json-schema <json|file>` (result in `structured_output`; per m1 decision 7), `--print-timeout`, `--new-project`, `--project`, `--conversation <id>` / `-c`, `--dangerously-skip-permissions`, `--model`, `--effort`, `--mode`, `--sandbox`, `--input-format stream-json`, `--disable-slash-commands`. No system-prompt flag, no flag to disable hooks or customizations, and no --setting-sources equivalent.

Hooks DO load in print mode. The CLI logs of oboete's own summarizer runs (e.g. log/cli-20260923_065540.log, workspaceDirs=[/tmp/oboete-cli-1190299]) show hooks_manager 'loaded 1 named hooks from 1 hooks.json file(s)'. So once oboete registers hooks, every `agy -p` summarizer call would fire them. That is the self-capture risk.

Guards:
- (a) OBOETE_SKIP=1 already set by provider.rs; it should reach hooks via env inheritance (language-server child process + sh -c). Likely, but unverified.
- (b) Also skip when workspacePaths[0] is under temp_dir()/oboete-cli-*. agy uses the cwd as the workspace; conversation_summaries.workspace_uris confirms it.

Read-only slash commands (`-p "/hooks"`, `/help`, `/config`, `/changelog`, `/permissions`) answer without an agent turn or quota.

### Windows and macOS

macOS and Linux use the same paths: ~/.gemini/config/hooks.json, ~/.gemini/config/mcp_config.json, and ~/.gemini/antigravity-cli/ (brain/, log/, conversation_summaries.db). Commands run via `sh -c`.

Windows native: %USERPROFILE%\.gemini\config\hooks.json and mcp_config.json, and commands run via `cmd /c` (builtin docs/hooks.md). claude-mem reports that on Windows agy splits the hook command on spaces and does NOT strip quotes, so a quoted path is passed literally and fails. They work around it with 8.3 short paths and forward slashes. So on Windows, write the absolute exe path unquoted and space-free (or resolve the 8.3 short path). On Unix, shell_quote only when needed.

The transcript directory name is product-specific: `antigravity-cli/` for the CLI, `antigravity/` for Antigravity 2.0, `antigravity-ide/` for the IDE. Parse transcriptPath from the payload instead of building it.

Admin override file on Linux: /etc/antigravity/admin_settings.json (seen in the binary; not relevant to hooks).

All Windows/macOS details are doc- or claude-mem-sourced, not tested here.

### Reference implementations

1. claude-mem 13.25.3 (local clone ~/.claude/plugins/marketplaces/thedotmack, commit 4520de9 2026-09-21). Files: src/cli/adapters/antigravity-cli.ts (input normalization: workspacePaths[0] as cwd, conversationId as session, prompt and response read from the transcript's USER_INPUT/PLANNER_RESPONSE, <USER_REQUEST> unwrap, output limited to injectSteps/decision/{}); src/services/integrations/AntigravityCliHooksInstaller.ts (~/.gemini/config/hooks.json, 5 events, timeout 10000 which is wrong since timeout is in seconds). Also injects context through ~/.gemini/GEMINI.md and ~/.agents/rules/claude-mem-context.md. Caveats: written against agy 1.2.1 from string inspection (issue https://github.com/thedotmack/claude-mem/issues/4057, closed and consolidated into #3611, with no live schema fixture). Its hooks.json uses event names as top-level keys, which contradicts the shipped docs' named-hook format. It predates the 1.2.9 payload fields lastUserInput/result/finalModelOutput. Not installed for agy on this PC (no ~/.gemini/config/hooks.json), though its MCP server is registered.
2. gitnexus ~/.gemini/config/hooks/gitnexus/gitnexus-antigravity-hook.cjs targets the Gemini CLI contract (BeforeTool/AfterTool, tool_name/tool_input/tool_response, hookSpecificOutput.additionalContext). Those names have 0 occurrences in agy 1.2.9, it is not registered anywhere, and it is not applicable.
3. The superpowers agy plugin ~/.gemini/antigravity-cli/plugins/superpowers/hooks.json is a live example of a Claude-style grouped SessionStart that agy parses as a named hook called "hooks".
4. oboete src/hook.rs already has the patterns to reuse: the Grok PreToolUse injected_at gate becomes the agy PreInvocation gate, last_assistant_in_transcript is the tail-read template for transcript_full.jsonl, and the setup.rs merge/backup/atomic write is the template for hooks.json/mcp_config.json. docs/plan.md line 26 already names .agents/hooks.json / ~/.gemini/config/hooks.json and PreInvocation injectSteps correctly.

### Uncertain

- Whether 1.2.9 actually fills lastUserInput, PostToolUse result/toolCall, and Stop finalModelOutput. They are proto fields with filler functions in the binary, but the shipped docs list fewer fields. Fallback for each: transcript_full.jsonl (USER_INPUT, the step at stepIdx, the last PLANNER_RESPONSE).
- Whether lastUserInput includes the <USER_REQUEST>/<ADDITIONAL_METADATA>/<USER_SETTINGS_CHANGE> wrapper (the transcript USER_INPUT content does).
- The invocationNum base (0 or 1) and whether it resets per turn. Deduping on executionId avoids depending on it.
- Whether workspacePaths entries are plain paths (as in the docs example) or file:// URIs (as in conversation_summaries.workspace_uris). Accept both.
- Whether OBOETE_SKIP=1 reaches the hook process (env inheritance through the language-server child and sh -c). The temp-dir workspace prefix check is the backup.
- Whether an injected ephemeralMessage stays visible to the model after the first invocation (EPHEMERAL_MESSAGE_PERSISTENCE_LEVEL_LATEST_ONLY exists). Fallbacks: userMessage, or the undocumented systemMessage step, or re-injecting on each PreInvocation.
- The SessionStart hook: when it fires, flat vs grouped shape, and resume behaviour. It is undocumented, though it exists in the binary and the superpowers plugin uses it grouped.
- Whether strict output parsing still rejects unknown fields in 1.2.9 (claude-mem #4057 says yes for 1.2.1; 1.2.9 has dropUnsupportedFields). Emit only documented fields either way.
- The cwd agy gives MCP server children, which affects oboete mcp's default repo scope.
- Command splitting: the docs say sh -c / cmd /c; claude-mem says space-split without quote stripping (observed on Windows). Keep the exe path unquoted and space-free.
- Whether a hook that prints nothing (empty stdout) is treated as an error. Always print {} to be safe.

### Fact-check corrections

- **wrong**: hook.rs's session_id extraction (`str_field(payload, &["session_id","sessionId","conversation_id"])`) will resolve agy events, which is the most load-bearing gap the spec doesn't call out — agy sends `conversationId` (camelCase) per the shipped docs example and the binary's `HookArgsCommon.GetConversationId`. That key is absent from hook.rs's fallback list (only snake_case `session_id`/`conversation_id` and Grok's `sessionId` are there), so every agy event today would resolve to session_id "unknown" and all agy activity would collapse into one fake session. Add `conversationId` to the field list in src/hook.rs:129 before implementing the agy branch. (src/hook.rs:129 (`str_field(payload, &["session_id", "sessionId", "conversation_id"])`) vs confirmed agy field name `conversationId` (docs/hooks.md common-fields example + binary symbol `hooks_go_proto.(*HookArgsCommon).GetConversationId`))
- **unverifiable**: PreInvocation's `executionId` is asserted as "one per turn" (used to dedupe stored prompts) without hedging — Nothing in the docs or binary getters establishes executionId's granularity (per-turn vs per-conversation-static). Move it to the spec's own `uncertain` list alongside invocationNum, and validate/derive turn boundaries from Stop's `executionNum` (a confirmed per-turn integer, `StopHookArgs.GetExecutionNum`) or by comparing new `lastUserInput` text against the last stored prompt for the session, rather than trusting executionId's granularity for the store-once-per-turn logic. (binary symbol table only shows `HookArgsCommon.GetExecutionId` exists, not its update cadence; cross-checked against `StopHookArgs.GetExecutionNum` which is confirmed per-turn)
- **wrong**: HookInjectedStep's undocumented oneof step kinds are listed as systemMessage/hookUserMessage/hookEphemeralMessage/checkpoint — The list is incomplete: the binary also exposes `GetErrorMessage` and `GetModelApiContentId` as HookInjectedStep oneof variants. Add `errorMessage` and `modelApiContentId` to the undocumented-step-kind list. (strings dump: `hooks_go_proto.(*HookInjectedStep).GetStep/.GetToolCall/.GetUserMessage/.GetEphemeralMessage/.GetSystemMessage/.GetErrorMessage/.GetHookUserMessage/.GetHookEphemeralMessage/.GetCheckpoint/.GetModelApiContentId`)
- **unverifiable**: transcriptPath is given as `brain/<conversationId>/.system_generated/logs/transcript.jsonl` — That path is confirmed as a real, populated file location on this machine (including for oboete's own summarizer runs), and truncated_fields/transcript_full.jsonl behave exactly as claimed. But the shipped docs' example transcriptPath is workspace-relative (`/path/to/workspace/.gemini/<product>/transcript.jsonl`), and no such workspace-local `.gemini/` tree exists anywhere under the oboete repo or elsewhere under $HOME on this machine — all real transcripts live under the global `~/.gemini/antigravity-cli/brain/<id>/` tree instead. The spec's own mitigation ("parse transcriptPath from the payload instead of building it") is the right call, but should explicitly flag that the doc's example shape and the observed on-disk layout disagree, so the literal runtime value still needs confirming in one live session rather than assumed to match either shape. (docs/hooks.md common-fields example (`/path/to/workspace/.gemini/antigravity/transcript.jsonl`) vs `find` for any workspace-local `.gemini` dir (none found) vs real transcripts under `~/.gemini/antigravity-cli/brain/*/.system_generated/logs/{transcript,transcript_full}.jsonl`)
- **wrong**: "That is 60+ already" (count of side-effect artifacts left by oboete's own agy summarizer runs) — State the actual counts instead of an estimate: `conversation_summaries.db` has 46 rows today with `workspace_uris LIKE '%oboete-cli%'`, and `~/.gemini/config/projects/` has 72 total project files (not oboete-specific only, since that count includes real work projects). Cite the query rather than a round number, since it grows every run. (`sqlite3 -readonly ~/.gemini/antigravity-cli/conversation_summaries.db "SELECT count(*) FROM conversation_summaries WHERE workspace_uris LIKE '%oboete-cli%'"` → 46; `ls ~/.gemini/config/projects | wc -l` → 72)

### Sources

- ~/.gemini/antigravity-cli/builtin/skills/agy-customizations/docs/hooks.md (shipped with 1.2.9; the official hooks contract)
- ~/.gemini/antigravity-cli/builtin/skills/agy-customizations/SKILL.md, docs/plugins.md, docs/json_configs.md, docs/mcp_servers.md (discovery roots, plugin hooks, MCP config path)
- ~/.gemini/antigravity-cli/cache/CHANGELOG.md (1.2.9 back to 1.1.x: shared ~/.gemini/config/hooks.json, print-mode /hooks, Stop ordering, PostToolUse matcher fix, workspace hooks after trust)
- /home/jura/.local/bin/agy 1.2.9: embedded FileDescriptorProto third_party/jetski/hooks_pb/hooks.proto decoded by hand (HookArgsCommon, PreTool/PostTool/PreInvocation/PostInvocation/Stop/SessionStart args and results, HookInjectedStep)
- /home/jura/.local/bin/agy strings: jsonhook package (ParseHooks, JSONHookSpec, marshalHookArgs, dropUnsupportedFields), 'failed to marshal flattened hook args', 'loaded %d named hooks', 'ANTIGRAVITY_CONVERSATION_ID=%s', 'failed to construct JSON session-start hook', EPHEMERAL_MESSAGE_PERSISTENCE_LEVEL_LATEST_ONLY/ALL, hookutils.GetLastUserInput/GetToolOutputFromStep
- ~/.gemini/antigravity-cli/log/cli-*.log (hooks_manager loads hooks in print-mode runs with workspaceDirs=[/tmp/oboete-cli-*])
- ~/.gemini/antigravity-cli/conversation_summaries.db schema + rows (workspace_uris, parent_conversation_id), opened read-only with sqlite3
- ~/.gemini/antigravity-cli/brain/*/.system_generated/logs/transcript.jsonl and transcript_full.jsonl (step types USER_INPUT/PLANNER_RESPONSE/RUN_COMMAND/CHECKPOINT/EPHEMERAL_MESSAGE/INVOKE_SUBAGENT; truncation)
- ~/.gemini/config/mcp_config.json (existing servers; values masked)
- ~/.gemini/antigravity-cli/plugins/superpowers/hooks.json
- `agy --help`, `agy mcp --help`, `agy mcp add --help`
- claude-mem src/cli/adapters/antigravity-cli.ts and src/services/integrations/AntigravityCliHooksInstaller.ts (local clone, v13.25.3)
- https://github.com/thedotmack/claude-mem/issues/4057
- ~/.gemini/config/hooks/gitnexus/gitnexus-antigravity-hook.cjs
- /home/jura/projects/oboete/docs/m1.md decisions 1-4,7,10,12; src/hook.rs; src/setup.rs; src/provider.rs (agy -p invocation, scratch dir, OBOETE_SKIP)

## Pi

Installed: 0.87.1 (`pi --version`; package.json of the installed package). It has not been used on this PC yet: ~/.pi/agent holds only auth.json and models-store.json (both 2 bytes, meaning no provider is logged in), plus skills/. There is no settings.json, no extensions/ and no sessions/.

### Verified live (2026-09-24, pi 0.87.1)

Two `pi -p` runs with `PI_CODING_AGENT_DIR` pointed at a temp dir (a `models.json` with the local Ollama model `qwen3.5:9b`, `apiKey: "ollama"`, and a probe extension in `extensions/probe.ts` that logged every event). The owner's `~/.pi/agent` was not touched. Results override the rest of this section where they differ:

- **Factory shape works**: `export default function (pi) { pi.on(event, (e, ctx) => …) }`, loaded from `<agent dir>/extensions/*.ts` with no build and no imports beyond `node:`. `pi` has `on, registerTool, exec, sendMessage, …`.
- **Order in print mode**: `session_start` (`reason: "startup"`), `input` (`text`, `source: "interactive"` for the `-p` prompt), `before_agent_start` (`prompt`), `message_end` (system, user, custom, assistant), `turn_end`, `agent_end` (`messages`), `session_shutdown` (`reason: "quit"`).
- `ctx` gives `cwd` (the launch directory), `mode` (`"print"`), `hasUI` (false), `sessionManager.getSessionId()`, `sessionManager.getSessionFile()` (JSONL under `<agent dir>/sessions/…`) and `getEntries()`.
- **Injection works**: returning `{message: {customType: "oboete", content, display: false}}` from `before_agent_start` put the text in front of the model (it answered with the injected codename). The custom message is stored in the session.
- **`tool_result`** = `{type, toolName, toolCallId, input, content: [{type: "text", text}], isError}`.
- **`OBOETE_SKIP=1` on the `pi` process is visible in the extension** (`process.env.OBOETE_SKIP`).
- `~/.pi/agent/extensions/` now exists and holds a third-party `git-ai.ts` (installed 2026-09-24 01:02, not by oboete).
- Not verified: interactive mode, `--continue`/`--resume`, compaction, and whether a tool registered with `pi.registerTool` reaches the model.

### Implemented (2026-09-24)

- `oboete setup pi` and `setup all` generate `<agent-dir>/extensions/oboete.ts` from `src/pi.ts` via `include_str!`. The prefix contains only the executable and optional `--home` arguments, encoded with `serde_json::to_string`. The agent directory follows tilde-expanded `PI_CODING_AGENT_DIR`, otherwise `~/.pi/agent`; setup skips when both the directory and `pi` on PATH are absent. Writes reuse atomic staging and the first-write backup. `--remove` deletes only the marked extension, preserving other extensions such as `git-ai.ts`. Doctor distinguishes missing, current, and stale content, including the binary and home. No MCP configuration is written.
- The extension sends Claude-shaped SessionStart, UserPromptSubmit, PostToolUse/failure, Stop, PostCompact, and SessionEnd payloads through one ordered subprocess chain, with session identity captured before enqueueing. Hook failures are swallowed. Context fetches and shutdown wait at most two seconds including the queue; each subprocess also has a two-second timeout. Startup context is returned once as a hidden `before_agent_start` custom message, with a new stash after compaction. Reload events and extension-generated input are skipped; existing message entries select resume. `OBOETE_SKIP` registers neither handlers nor tools. Pi's SessionEnd uses the existing observer path with no delayed observer.
- Native `oboete_search`, `oboete_get`, and `oboete_timeline` tools use Pi 0.87.1's `registerTool` and `exec` signatures and the virtual `typebox` module. CLI calls receive the current cwd, abort signal, and a ten-second timeout. Search and timeline limits are capped at 100, matching the MCP tools. Query and document-id arguments follow `--`, so user text cannot become CLI options. Nonzero exits and killed commands become tool errors.
- Rust tests cover setup, escaping, removal, doctor, and hook storage/injection. A Node harness loads the generated file with stubbed Pi and subprocess APIs; it is skipped only when Node is absent. Tests and CLI checks use temporary directories. OpenCode PR #35 was still open at implementation start, so the Pi template follows the same generation pattern without depending on that branch.
- **Checked live with this adapter (2026-09-24, pi 0.87.1, print mode, temp agent dir and temp `--home`, Ollama `qwen3.5:9b`)**: one `read` call stored SessionStart (`startup`), UserPromptSubmit, PostToolUse, Stop and SessionEnd in order under agent `pi` and the workspace's repo. With a seeded summary, the injected context reached the model (it answered the seeded codename) and was stored in the session as the `oboete` custom message. The model called `oboete_search` and used its result. `OBOETE_SKIP=1` stored nothing. `--continue` kept Pi's session id, sent SessionStart with `source: "resume"` and injected nothing.
- Still unverified: interactive mode, live compaction, and Windows/macOS.

### Mechanism

Pi has no command hooks. It integrates through an in-process TypeScript extension. Pi loads it with jiti, so there is no build step. An extension is a module whose default export is a factory `(pi: ExtensionAPI) => void | Promise<void>`, and the factory calls `pi.on(event, handler)`.

User-level location: `<agent-dir>/extensions/oboete.ts`. Pi auto-discovers direct .ts/.js files, or directories with an index.ts/index.js. `<agent-dir>` is `$PI_CODING_AGENT_DIR` (tilde-expanded), else `join(homedir(), ".pi", "agent")` (dist/config.js getAgentDir). User extensions need no trust prompt; project `.pi/extensions/` needs trust. Another route is `settings.json` `"extensions": ["/abs/path"]`. It is not recommended because it means merging JSON into Pi's settings, while the file drop needs no merge. Changes apply on the next pi start or after `/reload`.

The extension has no dependencies. A type-only `import type { ExtensionAPI } from "@earendil-works/pi-coding-agent"` is erased. `node:child_process` is allowed (the bundled examples import `node:fs`). If a tool is registered, `typebox` is provided as a virtual module.

`pi.exec(cmd, args, {signal, timeout, cwd})` cannot write stdin (see dist/core/exec.d.ts). So the hook call must use `spawn(OBOETE_BIN, ["hook","pi",event], {stdio:["pipe","pipe","ignore"]})` with `child.stdin.end(JSON.stringify(payload))`, spawning the exe directly with no shell.

`oboete setup pi` then only has to write this one file, with the `current_exe()` absolute path baked in as a JSON string literal (decision 12). The write is atomic, with a first-time .bak. `--remove` deletes the file only if it carries an oboete marker comment. `doctor` checks that the file exists and that its embedded path matches.

Rust side: `hook.rs` `run_stdin`/`handle` already accept any agent string (only "claude" is special-cased in resolve_agent). If the extension emits the Claude snake_case dialect, `oboete hook pi <Event>` works unchanged, and the SessionStart injection rule (`agent != grok && source != resume`) already fits. Only setup.rs and doctor need a `pi` arm.

Skeleton (load-bearing parts):
```ts
import { spawn } from "node:child_process";
const BIN = "/abs/oboete"; // baked by setup
export default function (pi) {
  if (process.env.OBOETE_SKIP) return;               // self-capture guard, register nothing
  let chain = Promise.resolve(); let pending = "";
  const call = (event, ctx, extra, timeoutMs = 2000) => new Promise(res => {
    const sm = ctx.sessionManager;
    const payload = { session_id: sm.getSessionId(), cwd: ctx.cwd, transcript_path: sm.getSessionFile() ?? null, hook_event_name: event, ...extra };
    let out = ""; const c = spawn(BIN, ["hook","pi",event], { stdio: ["pipe","pipe","ignore"] });
    const t = setTimeout(() => { c.kill(); res(""); }, timeoutMs);
    c.on("error", () => { clearTimeout(t); res(""); });
    c.stdout.on("data", d => out += d); c.on("close", () => { clearTimeout(t); res(out); });
    c.stdin.on("error", () => {}); c.stdin.end(JSON.stringify(payload));
  });
  const fire = (e, ctx, x) => { chain = chain.then(() => call(e, ctx, x)); }; // ordered, not awaited
  const ctxOf = out => { try { return JSON.parse(out).hookSpecificOutput?.additionalContext ?? ""; } catch { return ""; } };
  pi.on("session_start", async (ev, ctx) => {
    if (ev.reason === "reload") return;
    const resumed = ctx.sessionManager.getEntries().some(e => e.type === "message");
    await chain; pending = ctxOf(await call("SessionStart", ctx, { source: resumed ? "resume" : "startup" }));
  });
  pi.on("before_agent_start", () => { if (!pending) return; const t = pending; pending = "";
    return { message: { customType: "oboete", content: t, display: false } }; });
  pi.on("input", (ev, ctx) => { if (ev.source !== "extension") fire("UserPromptSubmit", ctx, { prompt: ev.text }); });
  pi.on("tool_result", (ev, ctx) => { fire(ev.isError ? "PostToolUseFailure" : "PostToolUse", ctx, {
    tool_name: ev.toolName, tool_input: ev.input,
    tool_response: ev.content.filter(p => p.type === "text").map(p => p.text).join("\n") }); });   // return undefined: do not transform
  pi.on("agent_end", (ev, ctx) => { const a = [...ev.messages].reverse().find(m => m.role === "assistant");
    fire("Stop", ctx, { last_assistant_message: a ? a.content.filter(p => p.type === "text").map(p => p.text).join("\n") : "" }); });
  pi.on("session_compact", async (ev, ctx) => { fire("PostCompact", ctx, { compact_summary: ev.compactionEntry.summary, trigger: ev.reason });
    await chain; pending = ctxOf(await call("SessionStart", ctx, { source: "compact" })); });
  pi.on("session_shutdown", async (ev, ctx) => { if (ev.reason === "reload") return; fire("SessionEnd", ctx, { reason: ev.reason }); await chain; });
}
```

### Events

| Event | oboete event | Payload | Injection |
| --- | --- | --- | --- |
| session_start | SessionStart | Event: {type, reason: "startup"\|"reload"\|"new"\|"resume"\|"fork", previousSessionFile?}. Session id comes from ctx.sessionManager.getSessionId(); cwd from ctx.cwd; transcript path from ctx.sessionManager.getSessionFile() (the session JSONL under ~/.pi/agent/sessions/<cwd-group>/, undefined with --no-session). The extension sends {session_id, cwd, transcript_path, hook_event_name, source}. Source cannot come from reason alone: `pi --continue/--resume/--session` at process start creates the initial runtime with reason "startup" (agent-session.js:166 default). So use source="resume" when ctx.sessionManager.getEntries() already has a message entry (this also covers /resume and fork, whose transcripts carry the earlier injection), else "startup". Skip reason "reload": the same session continues with a fresh extension runtime. | No. The return value is ignored. The handler is awaited (agent-session.js:2313), so it can await `oboete hook pi SessionStart`, parse hookSpecificOutput.additionalContext and stash it for before_agent_start. It fires in all modes, print mode included (print-mode.js calls bindExtensions, which emits it). |
| before_agent_start | (injection point for the SessionStart context; not stored) | {prompt (after template/skill expansion), images?, systemPrompt (readonly), systemPromptOptions (mutable; the base is cloned per run by normalizeBuildSystemPromptOptions)} | Yes, once per agent run. This is the only way to reach the model at run start. Recommended: return {message:{customType:"oboete", content:text, display:false}}. It is appended after the user message, sent to the LLM as role user (messages.js case "custom"), and saved as a custom_message entry. The transcript keeps it, so resume needs no re-inject (decision 3 unchanged); send it once, then clear the stash. Alternative: set event.systemPromptOptions.sections.oboete = text. That survives compaction and stays in the system prefix, but sections are diffed against the transcript's last system message, so it must be set to the same text on every run (and on resume) or Pi appends a removal delta. Avoid returning {systemPrompt}: it forces a full-prompt replacement. No size cap was found in the code. It does NOT fire for steer/followUp prompts queued while streaming (prompt() returns before emitBeforeAgentStart). |
| input | UserPromptSubmit | {text (raw typed text, before skill/template expansion), images?, source: "interactive"\|"rpc"\|"extension", streamingBehavior?: "steer"\|"followUp"}. Send {prompt: text}. Filter source==="extension" (pi.sendUserMessage traffic). Extension /commands are handled before input and never reach it. Chosen over message_end(role user) because that one carries the expanded text (a whole skill body for /skill:x) and extension-sent messages. input catches steer/followUp, which before_agent_start misses. | No (it can only transform or handle the input). Return undefined. Fire-and-forget. |
| tool_result | PostToolUse / PostToolUseFailure (by isError) | {toolCallId, toolName (bash\|read\|edit\|write\|grep\|find\|ls\|powershell\|custom), input: Record<string,unknown>, content: (TextContent\|ImageContent)[], details, isError, usage?}. Send {tool_name, tool_input: input, tool_response: text parts of content joined}. This one event carries both input and output (tool_execution_end has no args). | It can rewrite the tool result, but oboete must return undefined. Do NOT await the spawn: this is a transform hook that delays the result reaching the model. Sibling tool calls run in parallel, so serialize the spawns through a promise chain to keep row order. |
| agent_end | Stop | {messages: AgentMessage[]} for the run. Take the last role==="assistant" message and join its content parts with type==="text" (skip thinking/toolCall). stopReason/errorMessage are also available. Send {last_assistant_message}. hook.rs then spawns observe, as for Claude. turn_end is per model call (including tool-call turns), so it is the wrong granularity. agent_settled is final but carries no payload. | No. (agent_before_settle/turn_end could append entries and force a continuation; oboete does not use them.) |
| session_compact | PostCompact (+ re-inject: SessionStart source "compact") | {compactionEntry: {summary, firstKeptEntryId, tokensBefore, ...}, fromExtension, reason: "manual"\|"threshold"\|"overflow", willRetry}. Send {compact_summary: compactionEntry.summary, trigger: reason}. Then call SessionStart with source "compact" and stash the context for the next before_agent_start. This matches Claude's "compact gets it again", since the injected custom message may have been summarized away. | No, not directly. It works through the stash plus the next before_agent_start (the next user prompt, not mid-run). |
| session_shutdown | SessionEnd | {reason: "quit"\|"reload"\|"new"\|"resume"\|"fork", targetSessionFile?}. It is emitted before teardown, so ctx.sessionManager still reports the old session id. Send {reason}; skip "reload". | No. It is awaited on quit (AgentSessionRuntime.dispose awaits emitSessionShutdownEvent; print-mode.js:29 calls dispose; interactive mode routes SIGTERM/SIGHUP to a graceful shutdown that emits it). So await the chain plus this spawn, with a ~2 s timeout. A hard kill (SIGKILL) loses it. |
| tool_call | PreToolUse (not needed) | {toolCallId, toolName, input (mutable)} | It can block or mutate input only; it has no context channel. Not needed: pi injects through before_agent_start, and the PreToolUse path in hook.rs is Grok-only. |

### What is missing and the workaround

1) Pi has no command hooks and no stdout protocol. Everything is in-process TS, and oboete has to ship a small .ts file.
2) pi.exec has no stdin and no env, so the extension uses node:child_process.spawn.
3) There is no event where Pi reads context at session start. Injection happens at before_agent_start (the first prompt of each run). The workaround is to stash SessionStart's additionalContext and return it as a custom message.
4) Pi reports reason \"startup\" for `--continue/--resume/--session` at launch. The workaround is to call it a resume when the session already has message entries.
5) before_agent_start misses steer/followUp prompts, so prompts are captured from `input` instead.
6) Pi has no first-party MCP (0 occurrences of \"mcp\" in dist/bundle/cli.js, docs, README or CHANGELOG). See the mcp field.
7) The session transcript (JSONL, getSessionFile) is available if the summarizer ever needs a fallback like Codex's (decision 4). It is not needed, because agent_end carries the messages.
8) There is no pi-side cap on injected text; oboete's own inject limit applies.
Rust changes: none in hook.rs (the agent string \"pi\" passes through and the Claude dialect parses). setup.rs/doctor need a `pi` arm that writes, removes and checks `<agent-dir>/extensions/oboete.ts`. `all` should include pi only when `pi` is on PATH or `<agent-dir>` exists.

### MCP registration

Pi has no MCP client. Two options.

(a) Recommended: register three native tools in the same extension with pi.registerTool: oboete_search {query, all?, limit?}, oboete_get {id}, oboete_timeline {all?, limit?}. Each execute() runs `pi.exec(BIN, ["search", ...terms, "--limit", n, ("--all")], {cwd: ctx.cwd, timeout, signal})`, likewise `get <id>` and `timeline`, and returns {content:[{type:\"text\", text: stdout}], details: undefined}. Throw on a non-zero code so the model sees an error result. Parameters use `import { Type } from \"typebox\"` (a virtual module Pi provides). Nothing else to install, and it stays one file. Limit: the CLI search has no `repo` argument (the MCP tool does); cwd scoping plus --all covers the normal case.

(b) Third-party pi-mcp-adapter (npm pi-mcp-adapter 2.37.0, github.com/nicobailon/pi-mcp-adapter; install with `pi install npm:pi-mcp-adapter`). It reads `mcpServers` from ~/.config/mcp/mcp.json, ~/.agents/mcp.json, <agent-dir>/mcp.json (Pi override), .mcp.json or .pi/mcp.json, e.g. {\"mcpServers\":{\"oboete\":{\"command\":\"/abs/oboete\",\"args\":[\"mcp\"]}}}. Tools are exposed through a single proxy `mcp` tool unless \"directTools\": true. It is not installed here, and none of those files exist. oboete setup should not write MCP config for pi. At most, doctor could note the adapter if present.

### Headless runs and self-capture

`pi -p/--print \"<prompt>\"` (runs the prompt and exits), `--mode json` (JSON event stream), `--mode rpc` (RPC over stdio). Extensions load in interactive, RPC, JSON and print modes (docs/extensions.md \"UI and modes\"), so every capture event fires headless too. session_start is emitted via bindExtensions in print-mode.js, and session_shutdown(\"quit\") via runtimeHost.dispose(). ctx.hasUI is false in print/json and ctx.mode is \"print\"/\"json\"; the extension must not use ctx.ui.

Self-capture guards: (1) `if (process.env.OBOETE_SKIP) return;` at the top of the factory, so no handler is registered and no process is spawned. The extension runs inside the pi process, whose env is inherited from whoever launched pi. (2) The existing hook-side OBOETE_SKIP exit. (3) If oboete ever uses pi as a summarizer, launch `pi -p --no-extensions --no-session --no-skills --no-context-files --no-tools` with OBOETE_SKIP=1. `--no-extensions` disables discovery; explicit -e still loads.

Note: Pi sets PI_CODING_AGENT=true (plus PI_SESSION_ID/PI_SESSION_FILE) for commands run by its bash tool. A nested `pi` started by the agent is therefore detectable, but it is genuine agent work, so capture it as its own session.

### Windows and macOS

Same layout on all platforms: agent dir = join(os.homedir(), \".pi\", \"agent\"), with no APPDATA or XDG variant (dist/config.js getAgentDir); the override is PI_CODING_AGENT_DIR. So:
- Windows native: %USERPROFILE%\\.pi\\agent\\extensions\\oboete.ts
- macOS: ~/.pi/agent/extensions/oboete.ts
Sessions default to <agent-dir>/sessions/, overridable by PI_CODING_AGENT_SESSION_DIR, --session-dir or the sessionDir setting.

Setup must embed the exe path as a JSON-escaped string literal (backslashes on Windows) and spawn it directly (no shell, so no quoting issues). On Windows the model-facing shell tool may be `powershell` instead of `bash` (toolName differs; the capture handles any toolName). Git Bash is needed for the default bash tool (docs/windows.md). No platform differences were found in the extension API.

### Reference implementations

- @remnic/plugin-pi (npm, github.com/joshuaswarren/remnic): session_start; before_agent_start recalls once and then returns {systemPrompt: base + cachedContext} on every run (the older forced-prompt style; sections or a message is preferred now); message_end(role user) and turn_end for capture; session_shutdown.
- pi-hermes-memory (github.com/chandra447/pi-hermes-memory): tool_result, message_end, turn_end, session_before_compact, session_shutdown.
- pi-memory (github.com/jayzeng/pi-memory) and pi-observational-memory (github.com/elpapi42/pi-observational-memory): memory extensions (not inspected).
- pi-mcp-adapter (github.com/nicobailon/pi-mcp-adapter): MCP bridge.
- Bundled examples under the installed package's examples/extensions/: prompt-customizer.ts (systemPromptOptions.sections every run), claude-rules.ts (session_start scan plus before_agent_start), custom-compaction.ts, subagent/ (spawning pi children).

### Uncertain

- Not exercised live: Pi has no provider logged in on this PC (auth.json is {}), and starting a session was out of scope. Event order, payloads and the print-mode session_start/session_shutdown were traced in the installed JS, not observed. Verify with one real `pi -p` session after setup (decision 12).
- Whether the 'resumed = getEntries() has a message entry' rule is right for fork-at-entry (a forked file copies history, so it should read as resume) and for a /new session that later gets --continue'd. Checking for an existing custom_message with customType "oboete" on the branch is an alternative.
- Whether built-in interactive slash commands (/compact, /model, …) reach the `input` event or are handled earlier by interactive-mode. If they do reach it, filter text starting with "/" that is not a skill or template.
- No size cap on injected text was found in pi. Whether very long custom messages hurt prompt caching is untested.
- Latency of awaiting the SessionStart spawn inside session_start (it blocks pi startup until done; the 2 s timeout is a guess). Measure with `oboete replay`-style timing or a real run.
- After compaction the injected custom message may or may not still be in the kept range (firstKeptEntryId). Re-injecting on session_compact could duplicate it occasionally, the same trade-off as Claude's source=compact.
- Whether the jiti loader lets a user extension import node:child_process in the compiled Bun binary distribution (not this install; the npm install runs on Node 24, where it is fine).

### Fact-check corrections

- **wrong**: Pi has no first-party MCP client / no way for a user to configure local MCP servers for extensions or the CLI, proven by "0 occurrences of 'mcp' in dist/bundle/cli.js, docs, README or CHANGELOG" — The practical conclusion (no reachable, user-configurable local MCP client for extensions — no mcpServers-style setting anywhere in config.d.ts/settings.md/docs, confirmed by grep) still holds, so the adapter design (register native tools via pi.registerTool, or point to the third-party pi-mcp-adapter) is unaffected. But the specific proof is misleading: dist/bundle/cli.js is a 169-byte shim that just calls main() — checking it proves nothing. The actual bundle Pi ships and runs (dist/bundle/chunks/*.js, which cli.js imports) DOES contain real MCP-related code: chunk-FUXEF6JQ.js has functionNameToMcpClient/isMcpCallableTool/McpCallableTool/mcpServerToVertex$1, and another chunk plus anthropic.js reference mcp_servers/mcp_tool_use/mcp_oauth_validate. This is vendored provider-SDK code (Anthropic's and Google Vertex's own server-side/remote "hosted MCP connector" API feature, reachable only if a caller sets provider-specific beta params) — not a Pi-facing local MCP client — and it is unreachable from dist/core (confirmed no "mcp" hits there, which is what the unbundled/jiti-loaded extension code and settings actually use). Restate the claim as: "no reachable/exposed MCP client" rather than "0 occurrences of mcp in the bundle," which is false. (pi-coding-agent 0.87.1 dist/bundle/chunks/chunk-FUXEF6JQ.js, dist/bundle/chunks/chunk-DTD7JQ7Y.js, dist/bundle/chunks/anthropic.js (contra dist/bundle/cli.js and dist/core, which are clean))

### Sources

- ~/.nvm/versions/node/v24.16.0/lib/node_modules/@earendil-works/pi-coding-agent/package.json (0.87.1, repo earendil-works/pi packages/coding-agent, piConfig.configDir .pi)
- …/pi-coding-agent/docs/extensions.md (locations, lifecycle, events, modes, errors)
- …/pi-coding-agent/docs/configuration.md and docs/environment-variables.md (agent dir, PI_CODING_AGENT_DIR, PI_CODING_AGENT_SESSION_DIR, PI_SESSION_ID/FILE, PI_CODING_AGENT=true)
- …/pi-coding-agent/docs/sessions.md (sessions under ~/.pi/agent/sessions/), docs/settings.md (extensions: string[])
- …/pi-coding-agent/dist/core/extensions/types.d.ts (all event interfaces, result types, ExtensionAPI.on overloads, ExtensionContext, registerTool, exec)
- …/pi-coding-agent/dist/core/exec.d.ts (ExecOptions: signal/timeout/cwd only; no stdin)
- …/pi-coding-agent/dist/core/extensions/runner.js emitBeforeAgentStart / emitSessionShutdownEvent
- …/pi-coding-agent/dist/core/agent-session.js prompt() (extension commands → input → expansion → steer/followUp return before before_agent_start), _preparePromptAndToolLoadout (section diffing), :166 default reason startup, :586 custom message persisted, :2313 session_start emit
- …/pi-coding-agent/dist/core/agent-session-runtime.js dispose() awaits session_shutdown quit; new/resume/fork createRuntime reasons
- …/pi-coding-agent/dist/modes/print-mode.js (bindExtensions, runtimeHost.dispose)
- …/pi-coding-agent/dist/core/messages.js (custom → role user for LLM)
- …/pi-coding-agent/dist/core/system-prompt.{js,d.ts} (BuildSystemPromptOptions.sections, normalize clones per run)
- …/pi-coding-agent/dist/config.js getAgentDir
- …/pi-coding-agent/dist/core/extensions/virtual-modules.js (typebox virtual module)
- …/pi-coding-agent/examples/extensions/prompt-customizer.ts, claude-rules.ts
- `pi --help` (0.87.1 flags: -p, --mode json|rpc, --no-extensions, -e, --no-session, --continue/--resume/--session)
- npm pack pi-mcp-adapter@2.37.0 README (config file precedence, proxy tool, directTools)
- npm pack @remnic/plugin-pi dist/index.js (event usage), pi-hermes-memory
- /home/jura/projects/oboete/src/hook.rs (handle(): accepted fields and inject_now rule), docs/m1.md decisions 1-4, 7, 10, 12

## OpenCode

Installed: opencode v2.0.12. That is the v2 line: git tag v2.0.12 on anomalyco/opencode, which has no GitHub release entry. The newest v2 is 2.0.15, published on npm as @opencode/cli and @opencode/plugin. The v1 line (npm opencode-ai@1.18.32, plugin package @opencode-ai/plugin) is a different API. The files in ~/.opencode are v1 leftovers: node_modules/@opencode-ai/plugin@1.18.12, opencode.json with a v1 "plugin" key, and plugins/graphify.js, which uses the v1 named-export hook shape. The v1 hook names in the task hints do not exist in the installed binary: grep -c finds 0 hits for "tool.execute.after", "experimental.chat.system.transform" and "chat.message", but does find "execute.after", "session.compaction.ended" and "Plugin must export a default definition".

### Verified live (2026-09-24, opencode v2.0.12)

A probe plugin in a scratch project's `.opencode/plugins/probe.js` logged every bus event and hook of two `opencode run --standalone --format json --auto` runs (free model `opencode-go/space-bunny-free`). The owner's global config and background service were not touched. Results override the rest of this section:

- **Module shape works as documented**: `export default { id, async setup(ctx) { … return () => abort() } }`, no imports. `ctx.location = {directory, project:{id, directory, canonical}}`. `ctx.session` has `hook, create, get, synthetic, context, …`; `ctx.tool` has `hook, list, …`.
- **Injection works**: `ctx.session.hook("context", e => e.system.push({type:"text", text}))` made the model answer from the pushed text. The hook runs before every model call (twice in a one-tool turn).
- **`ctx.tool.hook("execute.after", e => …)`** gets `{tool, sessionID, agent, messageID, id, input, status:"completed", result:{output, content:[{type:"text", text}], metadata}}` or `{…, status:"error", error:{_tag:"Tool.Error", message}}`.
- **Bus events seen, in order**: `session.inbox.enqueued` (`data.item = {type:"user", payload:{text, files}, delivery}`), `session.execution.started`, `session.instructions.updated`, `session.inbox.delivered`, `session.renamed`, `session.step.*`, `session.reasoning.*`, `session.tool.input.started|ended`, `session.tool.called`, `session.tool.success|failed`, `session.text.started|delta|ended` (`data.text` on `ended`), `session.execution.succeeded`, `location.shutdown`, plus `*.updated` config events at start. **No `session.created` was published**, so a session's first appearance is its first `session.inbox.enqueued`.
- Some events have no `location` (`session.execution.*`, `session.usage.updated`): filter by the session ids this plugin instance has seen, not only by `event.location.directory`.
- The prompt text from `opencode run "<msg>"` arrived wrapped in literal double quotes (`"\"Read …\""`); interactive prompts were not observed.
- **`OBOETE_SKIP=1` on `opencode run --standalone` reaches the plugin** (`process.env.OBOETE_SKIP === "1"` inside the server).
- `~/.config/opencode/plugins/` now exists and holds a third-party v1-style `git-ai.ts` (installed 2026-09-24 01:02, not by oboete).
- **MCP shape**: `opencode mcp add oboete --global -- /x/oboete mcp` with `OPENCODE_CONFIG_DIR` pointed at a temp dir wrote `{"mcp":{"servers":{"oboete":{"type":"local","command":["/x/oboete","mcp"]}}}}` to `$OPENCODE_CONFIG_DIR/opencode.json` without starting a session.
- Not verified: the TUI and the long-lived background service (hot reload of a new plugin file, env inheritance there), `session.compaction.ended`, subagent sessions.

### Implemented (2026-09-24)

The OpenCode v2 adapter follows the verified subsection above. This implementation note supersedes the preliminary mechanisms below where they suggest `session.created`, SessionEnd, or injection through SessionStart stdout.

- `oboete setup opencode` (also included in `setup all`) generates `<config dir>/plugins/oboete.js` from `src/opencode.js`. Only the absolute executable and optional `--home` are machine-specific; `serde_json::to_string` writes both as JavaScript-compatible JSON literals. The plugin uses only `node:child_process`, with argv arrays and no shell. Config directory precedence is `OPENCODE_CONFIG_DIR`, `XDG_CONFIG_HOME/opencode`, then `~/.config/opencode` on every OS.
- The first event with a matching location establishes a session and sends SessionStart. A tool or context hook can also be the first sign of a session (hooks carry no location but only reach their own plugin instance), so those use the plugin's own `ctx.location`. User inbox entries, successful and failed tools, the last assistant text at each terminal execution event, and compaction summaries become Claude-shaped hook payloads. Events from other locations and unlocated events for unknown sessions are ignored. Captures use a serial promise chain without blocking hooks, and spawn failures are swallowed. OpenCode and agy Stop hooks request the same 65-second delayed observer because neither agent has SessionEnd.
- The context hook fetches `oboete inject` once per known session per plugin instance, with a 3-second timeout. Concurrent calls share the same promise; empty and failed results are cached too. Each model call receives the cached text without persisting it in history. `OBOETE_SKIP` registers nothing, and cleanup aborts the event subscription.
- Setup stages the plugin and JSON MCP config before replacing either, backs up existing `opencode.json` once as `.oboete.bak`, and preserves other servers, plugins and optional fields on `mcp.servers.oboete`, including `disabled`. JSONC-only or unparsable configs remain untouched; setup still manages the plugin and prints an `opencode mcp add oboete --global -- ...` command for the owner. Removal deletes only our plugin and MCP entry; unsupported config formats require manual MCP removal. Missing OpenCode installations are skipped. Doctor compares the whole generated plugin against the current executable/home and reports the MCP command and disabled state.
- Verification uses temporary directories, Rust unit tests, `node --check`, and a Node harness with v2 callback payloads and stubbed subprocesses. The harness covers ordering, location isolation, tool results, all three terminal events, compaction mapping, injection caching/failure, self-capture suppression and cleanup. No real config or OpenCode service is changed or started.
- The real oboete CLI also passed temporary-directory checks for help, absent-install skipping, config-directory precedence, setup/removal, JSONC preservation and capture through stdin. `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` pass. In this sandbox, `cargo test` passes 64 of 66 tests: the existing repository test finds the sandbox's `/tmp/.git`, and the existing viewer socket test cannot bind (`Operation not permitted`). Both failures reproduce on unchanged `origin/main` (`1b01660`); neither test nor its gate was weakened.
- **Live check (2026-09-24, in review)**: `oboete setup opencode` with `OPENCODE_CONFIG_DIR` and `--home` pointed at temp dirs, then real `opencode` runs in a scratch repo.
  - Through a separate long-lived `opencode serve` (like the background service): SessionStart, the prompt, the `read` tool output and Stop (with the answer) were stored, and the delayed observer started.
  - Injection worked: the model answered from an observation seeded into that repo (`oboete inject`).
  - `opencode run --standalone` lost the final Stop: the private server shuts down right after `session.execution.succeeded`. Headless standalone runs are not the owner's usage; their last turn is summarized from the other events.
  - Prompts typed as `opencode run "<msg>"` arrive wrapped in literal double quotes and are stored that way.
- Still unverified live: the TUI and the owner's own background service (plugin hot reload, environment inheritance), compaction delivery, subagent sessions, and Windows/macOS runtime behavior.

### Mechanism

A v2 in-process JS/TS plugin with no dependencies. It runs inside the OpenCode server (Bun runtime), not in the CLI process. There are no command hooks and no stdout protocol.

FILE: ~/.config/opencode/plugins/oboete.js. The global config dir is $OPENCODE_CONFIG_DIR, else $XDG_CONFIG_HOME/opencode, else ~/.config/opencode. Today that dir holds only service.json and skills/; the plugins/ dir does not exist yet. The loader is core/src/plugin/source-directory.ts plus config/plugin/source.ts. It auto-discovers direct *.js / *.ts files and package dirs under <each config dir>/{plugin,plugins}/. The config dirs are the global one and every project .opencode/ found walking up from the cwd. Discovered files are on the config watch feed, so a file written by setup should hot-load into a running server without a restart. I read this in the source and did not test it.
- Avoid the configured-path route. In v2.0.12, "plugins": ["/abs/file.js"] in opencode.json is dropped with the warning "configured plugin path must be a directory", although the v2 docs show file paths. Only directories work there.

MODULE SHAPE: the loader is core/src/plugin/module.ts. It decodes default = {id: string, setup: fn} or {id, effect}. Plugin.define is the identity function, so no import is needed:
  export default { id: "oboete", async setup(ctx) { ...register...; return () => abort() } }
- The module may also carry `async server(input){ return {v1 hooks} }` in the same default export. OpenCode v1 1.18.29+ calls server(); v2 calls setup(). That covers v1 users from one file (per the migrate-v1 guide).

CONTEXT: ctx.location = {directory, workspaceID?, project:{id, directory, canonical}}. There is one plugin instance per location (project dir), and bus delivery is scoped to that location. gentle-ai additionally filters on event.location.directory === ctx.location.directory. Other context members: ctx.session.hook(name, cb), ctx.tool.hook(name, cb), ctx.event.subscribe({signal}) (an AsyncIterable of {type, data, location?, durable}), ctx.session.get / synthetic, ctx.storage, ctx.options.

HOW TO CALL OBOETE: from the plugin, use node:child_process spawn of the absolute oboete path that setup bakes into the generated JS: [exe, (--home h), "hook", "opencode", <Event>]. Write a Claude-shaped JSON payload to stdin, so hook.rs handle() is reused. Set cwd = the session directory, because the service chdir's to $HOME.
- v2 hooks have no timeout; only AbortSignal exists. A slow await stalls the agent. So capture calls must be fire-and-forget: stdio ["pipe","ignore","ignore"], unref(), serialized through one promise chain so event order is kept.
- Only the context fetch awaits stdout, with its own timeout of about 3 to 5 s.
- The v1 graphify.js in ~/.opencode/plugins is found as a project .opencode dir for any cwd under ~, and it fails to load on v2. That is not oboete's problem.

### Events

| Event | oboete event | Payload | Injection |
| --- | --- | --- | --- |
| session.created (bus, ctx.event.subscribe) | SessionStart | event.data = {sessionID, projectID, location:{directory, workspaceID?}, subpath?, parentID?, slug, title?, agent?, model?, metadata?, version}. cwd = location.directory joined with subpath. parentID present = subagent (task tool) child session: map it to the parent session_id (Claude's subagent tool hooks report the parent session) or skip it. Resuming with --continue / --session emits nothing. Send {session_id, cwd, source:"startup"}. | No: this is a bus observation and its return is ignored. Injection happens in the context hook (below). |
| session.inbox.enqueued (bus), item.type=="user" | UserPromptSubmit | event.data = {sessionID, inboxID, item:{type:"user", payload:{text, files?, agents?, skills?, metadata?}, delivery:"steer"\|"queue"}}. Published once per admitted prompt (SessionInbox.admit, first admission wins), for both steer and queue delivery. Skip item.type synthetic, compaction and move. Send {session_id, cwd, prompt: payload.text}; oboete's strip_blocks / envelope filter still apply. The alternative ctx.session.hook("prompt", e => e.prompt.text) runs before admission, is documented as not exactly-once, and is mutable, so prefer the bus event for capture. | The prompt hook can rewrite prompt.text, which would put the injection into persisted history; not recommended. The bus event cannot inject. |
| ctx.tool.hook("execute.after") | PostToolUse (status completed) / PostToolUseFailure (status error) | {tool, sessionID, agent, messageID, id(callID), input, status:"completed", result:{output?, content: string \| [{type:"text",text}\|{type:"file",uri,mime,name?}], metadata?}} or {..., status:"error", error:Tool.Error(.message)}. Triggered in core/src/tool.ts for every registered tool. Send {session_id, cwd, tool_name: tool, tool_input: input, tool_response: joined text content or output (or error.message)}. Bus alternatives: session.tool.input.started (has the name), session.tool.called (input), session.tool.success / failed (content, error). | It could mutate result (what the model sees), but oboete should not; this is fire-and-forget only. |
| session.text.ended (bus), then session.execution.succeeded \| failed \| interrupted (bus) | Stop | text.ended.data = {sessionID, assistantMessageID, ordinal, text}. There are several per turn, one per text part and step. Cache per session the texts of the latest assistantMessageID. execution.succeeded / failed (error) / interrupted (reason: user\|shutdown\|superseded\|inactivity) is published once per busy period, which covers coalesced queued prompts (core/src/session/execution.ts settled()). On that event send {session_id, cwd, last_assistant_message: cached text} and clear the cache. oboete then spawns observe as it already does on Stop. The deprecated session.idle (ephemeral) also exists. | No |
| session.compaction.ended (bus) | PostCompact | event.data = {sessionID, reason:"auto"\|"manual", text (the summary), recent, model, cost, tokens, ...}. Send {session_id, cwd, compact_summary: text}. session.compaction.started and failed also exist. ctx.session.hook("compaction") runs before the summary request (it can replace the summary via event.result) and is not needed for capture. | No. The system context still reaches later calls because the context hook re-pushes it on every call. |
| session.deleted (bus) / plugin cleanup | SessionEnd (partial) | session.deleted.data = {sessionID}. There is no per-session end event. execution.interrupted with reason "shutdown" fires for running sessions when the server stops, and the setup() cleanup function runs on unload. Send {session_id, reason:"deleted"\|"shutdown"}. | No |
| ctx.session.hook("context") | SessionStart injection (the replacement for hookSpecificOutput.additionalContext) | {sessionID, agent, model, system: SystemPart[] (mutable), messages, options, tools}. Runs before every agent-loop model call, including tool continuations. Title, compaction and generate requests have their own hooks. Change: event.system.push({type:"text", text}). | Yes, but only for that one outgoing call: it is not persisted (docs: 'Changes affect only the outgoing model call, not persisted history'). So the plugin must: (1) on the first context call per session per plugin instance, await `oboete hook opencode SessionStart` with a timeout, parse hookSpecificOutput.additionalContext, and cache it (an empty string is cached too); (2) push the cached text on every call. oboete side: for agent opencode, SessionStart must inject even on resume and even if injected_at is set, because nothing persists in the transcript. Suggested change to handle(): "SessionStart" => agent != "grok" && (agent == "opencode" \|\| source != "resume"). Size is unlimited except by context. Skip child sessions (parentID). Durable alternative: ctx.session.synthetic({sessionID, text, delivery:"queue"}) once; it is visible in the transcript and needs no re-push, but it adds a message turn. |

### What is missing and the workaround

1) There is no transcript path and no hook stdin. Everything arrives through in-process callbacks, so the plugin JS itself is the adapter. oboete ships a generated oboete.js with its absolute exe path, written by `oboete setup opencode`.
2) No SessionEnd. Use Stop, which is execution.succeeded, failed or interrupted; oboete already spawns observe on Stop. Add session.deleted and the shutdown interrupt as best-effort end markers.
3) No "assistant final message" field. The plugin caches session.text.ended per session and emits it on the execution terminal event.
4) No persistent once-per-session injection. The context hook is per call, so cache the text in the plugin and re-push it every call. oboete must not gate opencode injection on resume or injected_at.
5) Tool events carry no cwd, and the service process cwd is $HOME. Map sessionID to directory from session.created (location.directory + subpath). For a session first seen mid-stream (resume, or a service restart), fall back to ctx.session.get({sessionID}) or ctx.location.directory.
6) Plugin state is lost when the service restarts. That is harmless: re-fetch the context on the first call.
7) Subagent sessions are separate sessions with parentID. Map them to the parent to match Claude Code, or drop them.
8) Hooks have no timeout, so every oboete spawn except the context fetch must be fire-and-forget.
9) Removal: there is no `opencode mcp remove` and no plugin-file remove command. `oboete setup --remove opencode` deletes plugins/oboete.js and edits mcp.servers.oboete out of the config itself.
10) If the config is JSONC with comments, serde_json cannot edit it. Either use `opencode mcp add` (jsonc-parser, preserves comments) for adding and a hand-written minimal JSONC edit for removal, or refuse with a message.

### MCP registration

v2 shape (docs /v2/docs/mcp-servers and schema/src/mcp.ts LocalConfig): in ~/.config/opencode/opencode.json(c):
{"$schema":"https://opencode.ai/config.json","mcp":{"servers":{"oboete":{"type":"local","command":["/abs/path/oboete","mcp"]}}}}
The command may include ["--home","<dir>"] before "mcp".

Optional fields:
- "environment": {..}
- "cwd": defaults to the workspace (project) dir. That matches decision 10, where the server cwd sets the repo scope.
- "disabled": true. v2 uses disabled, not enabled.
- "codemode": false. The default true exposes tools through Code Mode instead of directly; for 3 small tools, direct exposure (false) seems preferable.
- "timeout", "protocol".

Legacy v1 flat shape: {"mcp":{"oboete":{"type":"local","command":[...],"enabled":true}}}. v2 still accepts it (core/src/config/normalize.ts migrates it and warns on conflicts), so this shape works on both v1 and v2 if v1 compatibility matters.

CLI: `opencode mcp add oboete --global -- /abs/oboete mcp`. It writes the file directly via jsonc-parser modify; it does not start a session or server, so OBOETE_SKIP is not needed. Target file: the first existing of opencode.json, opencode.jsonc, .opencode/opencode.json(c) under the config dir, else opencode.json is created. It cannot set codemode. There is no remove subcommand.

File selection when OpenCode reads config: opencode.json and opencode.jsonc are both loaded. Neither exists yet on this machine.

### Headless runs and self-capture

`opencode run [--format json] [--model p/m] [--agent a] [--title t] [--auto] [--standalone] "<msg>"`. Other headless surfaces are `opencode serve` (v2 API) and `opencode acp`.

Critical architecture point: plugins run in the SERVER process. By default every CLI command, run included, connects to a long-lived detached background service (`opencode service`, port and password in ~/.config/opencode/service.json, which must not be printed). That service was spawned with {...process.env} of whichever client started it first, and it chdir's to $HOME (client/src/service-contender.ts, cli/src/server-process.ts).

Consequences:
(a) Hooks fire for headless runs exactly as for the TUI, so self-capture is real.
(b) `OBOETE_SKIP=1 opencode run ...` without --standalone does NOT reach the plugin.
(c) Worse, if that run is the one that starts the service, the service inherits OBOETE_SKIP=1. Every later interactive session would then go uncaptured until the service restarts, because the spawned oboete hook sees the variable.

Rule: any opencode run that oboete starts (not in today's provider chain, decision 6, so this is forward-looking) must be `OBOETE_SKIP=1 opencode run --standalone --format json ...`. --standalone spawns a private `opencode serve --stdio` child with extendEnv:true (cli/src/services/standalone.ts), so the variable is inherited and the server dies with the run.

The plugin should check process.env.OBOETE_SKIP in setup() and return without registering anything. oboete hook's own OBOETE_SKIP check is a second guard, because the spawned hook inherits the server env.

Also: `opencode plugin list` and `debug config` connect to the service, and will spawn it if needed.

### Windows and macOS

global-roots.ts hard-codes XDG-style roots on every OS (only XDG_* or OPENCODE_CONFIG_DIR override them):
- Config: macOS ~/.config/opencode, not ~/Library; Windows %USERPROFILE%\.config\opencode.
- Plugins go in <config>/plugins/oboete.js.
- Data: ~/.local/share/opencode (opencode.db, log). State: ~/.local/state/opencode (service.json, locks).
- ~/.config/opencode/service.json holds the service password.

Platform differences:
- The v2 docs ship standalone binaries for macOS (arm64 / x64 / baseline) and Windows (x64 / ARM64), but "Windows package managers are not supported". Install is via curl https://opencode.ai/v2/install, brew anomalyco/tap/opencode-v2, or npm @opencode/cli.
- On Windows, spawn the oboete .exe by absolute path with an args array (no shell). The graphify plugin notes that PowerShell 5.1 rejects '&&', so avoid shell strings.
- The plugin JS itself is platform-neutral (node:child_process in Bun).

### Reference implementations

None of these is oboete-shaped for v2; the shapes to copy come from OpenCode's own docs and core plugins.
- claude-mem official: thedotmack/claude-mem src/integrations/opencode-plugin/index.ts (https://github.com/thedotmack/claude-mem/blob/main/src/integrations/opencode-plugin/index.ts). v1 only: its default export is a function returning v1 hooks (tool.execute.after, chat.message, event, experimental.session.compacting), so it fails the v2 module decode. Its assistant-capture branch in chat.message never fires, because v1 chat.message carries the user message. Don't copy its hook names for v2.
- v2-shaped examples: Gentleman-Programming/gentle-ai internal/assets/opencode/plugins-v2/telemetry-runtime.ts (https://github.com/Gentleman-Programming/gentle-ai). It uses `for await (const event of ctx.event.subscribe({signal}))`, filters event.location against ctx.location, reads event.data.*, and uses node:child_process execFile. This is the closest pattern to oboete capture.
- remorses/kimaki docs/opencode-v2-plugin-migration.md.
- joshuadavidthomas/opencode-agent-memory src/plugin.ts (memory blocks; 338 stars).
- OpenCode's own in-tree plugins: packages/core/src/plugin/plan.ts (tool execute.after) and tool-input-repair.ts (execute.before).
- Official docs: https://opencode.ai/v2/docs/build/plugins and https://opencode.ai/v2/docs/build/plugins/migrate-v1/. The latter has the v1-to-v2 hook table: chat.message → session.hook prompt, tool.execute.after → tool.hook execute.after, experimental.chat.system.transform → session.hook context + event.system, experimental.session.compacting → session.hook compaction, event → ctx.event.subscribe.

### Uncertain

- Hot reload of a newly written ~/.config/opencode/plugins/oboete.js into an already-running service comes from the source.ts comment ('config change feed already covers {plugin,plugins} directories'). I did not test it. Fallback: `opencode service restart` or `opencode reload`, both of which touch the service, so the owner should run them.
- Configured absolute file paths in "plugins": the v2.0.12 source rejects them ('must be a directory') while the v2 docs, which track 2.0.15, show '/absolute/path/plugin.ts'. This may have changed after 2.0.12; the drop-in file avoids the question.
- Whether the task tool's child sessions emit session.created with parentID in the parent's location stream before their first tool event. The schema has parentID, but I did not trace the task tool.
- Whether `opencode run --title` or prompt metadata could serve as a self-capture marker for non-standalone runs. Not verified; --standalone plus env is the verified path.
- codemode default=true for MCP: I did not verify how Code Mode exposes oboete's search/get/timeline to the model, or whether the MCP server instructions still reach it. codemode:false is the conservative choice.
- Whether the location filter on event.location is needed in addition to the bus's own location scoping. gentle-ai does it defensively, and it is cheap to copy.
- Behaviour of v1 (1.18.29+) with the dual default export {id, setup, server}. It comes from the migrate-v1 doc only; OpenCode v1 is not installed here.
- Whether `session.execution.succeeded` also fires for `opencode run` before the CLI exits. The event is server-side and should, but I did not observe it (no session was started, by constraint).

### Fact-check corrections

- **wrong**: 'v2 hooks have no timeout; only AbortSignal exists' as the justification for fire-and-forget capture calls — The 'no timeout' half is right, but 'only AbortSignal exists' is not: session.hook and tool.hook callbacks receive no AbortSignal at all. packages/plugin/src/promise/registration.ts defines `Hooks<Spec> = (name, callback: (input: Spec[Name]) => Promise<void>|void) => Promise<Registration>` — the callback gets only `input`, no signal, no timeout, no way for the host to bound it. The only AbortSignal in the plugin API is (a) the caller-supplied `signal` option to `ctx.event.subscribe({signal})`, used to stop iterating the bus stream, and (b) `ToolContext.signal` inside a *tool's own* execute() implementation — neither bounds a session.hook/tool.hook callback. State the reasoning as 'hook callbacks are entirely unbounded (no timeout, no signal)', which still supports the same fire-and-forget conclusion, just for the accurate reason. (packages/plugin/src/promise/registration.ts, promise/tool.ts, promise/session.ts, promise/event.ts @ v2.0.12)

### Sources

- ~/.opencode/bin/opencode --version / --help / run --help / mcp add --help / plugin --help / service --help (v2.0.12)
- grep -a over ~/.opencode/bin/opencode: v1 hook names absent; 'Plugin must export a default definition', 'execute.after', 'session.compaction.ended' present
- https://github.com/anomalyco/opencode/tree/v2.0.12 packages/core/src/plugin/module.ts (default {id, setup|effect} schema)
- packages/core/src/plugin/source-directory.ts and packages/core/src/config/plugin/source.ts ({plugin,plugins}/*.js|.ts discovery; configured file path rejected)
- packages/core/src/config/discovery.ts, packages/core/src/config.ts, packages/util/src/global.ts, packages/util/src/global-roots.ts (config dirs, XDG, OPENCODE_CONFIG_DIR)
- packages/plugin/src/promise/{plugin,session,tool,event,registration}.ts (Context, SessionHooks prompt/context/compaction, ToolHooks execute.before/after)
- packages/core/src/plugin/host.ts (event.subscribe = location-scoped bus) and packages/core/src/bus.ts
- packages/schema/src/session-event.ts (created, inbox.enqueued, execution.*, text.ended, tool.*, compaction.ended, deleted), session-inbox.ts, prompt.ts, location.ts, tool.ts, session-status-event.ts, event.ts, event-manifest.ts
- packages/core/src/tool.ts lines 100-150 (execute.after trigger payload)
- packages/core/src/session/execution.ts (Execution.Succeeded once per busy period) and session/inbox.ts (InboxEnqueued on every admission)
- packages/schema/src/mcp.ts, schema/src/config/mcp.ts, schema/src/config/plugin.ts, core/src/config/normalize.ts, core/src/v1/config/mcp.ts (v2 and legacy MCP shapes)
- packages/cli/src/commands/handlers/mcp/add.ts (writes config via jsonc-parser, no server)
- packages/cli/src/services/standalone.ts, server-connection.ts, server-process.ts, packages/client/src/service-contender.ts (service env inheritance, --standalone extendEnv)
- https://opencode.ai/v2/docs/plugins, https://opencode.ai/v2/docs/build/plugins, https://opencode.ai/v2/docs/build/plugins/migrate-v1/, https://opencode.ai/v2/docs/mcp-servers, https://opencode.ai/v2/docs/migrate-v1, https://opencode.ai/v2/docs (install)
- npm view: opencode-ai latest 1.18.32 (v1); @opencode/plugin latest 2.0.15
- ~/.opencode (v1 leftovers: opencode.json 'plugin' key, plugins/graphify.js, node_modules/@opencode-ai/plugin 1.18.12) and ~/.config/opencode contents (read-only)
- v1 types for the optional dual export: ~/.opencode/node_modules/@opencode-ai/plugin/dist/index.d.ts and @opencode-ai/sdk/dist/gen/types.gen.d.ts (session.created/idle/compacted/deleted, message.updated, message.part.updated)
- https://github.com/thedotmack/claude-mem/blob/main/src/integrations/opencode-plugin/index.ts; https://github.com/Gentleman-Programming/gentle-ai/blob/main/internal/assets/opencode/plugins-v2/telemetry-runtime.ts
- /home/jura/projects/oboete/src/hook.rs, /home/jura/projects/oboete/src/setup.rs, /home/jura/projects/oboete/docs/m1.md (decisions 1-4, 10, 12)

## Cursor (IDE and cursor-agent)

Installed: cursor-agent 2026.09.15-d2fe57e. `~/.local/bin/{cursor-agent,agent}` link to `~/.local/share/cursor-agent/versions/2026.09.15-d2fe57e/`, and `~/.local/bin/cursor` is a shim that falls back to `agent`. I found no Cursor IDE install on this PC. There is no `C:\Program Files\Cursor` or `%LOCALAPPDATA%\Programs\cursor`, and no Cursor entry in the HKCU uninstall list. Leftovers exist: `C:\Users\jura\.cursor` (hooks.json from June holding git-ai hooks, plus the remote-wsl extension) and `%APPDATA%\Cursor` (logs up to 2026-08-11). So everything below about the IDE comes from the docs only. The CLI details come from reading the installed bundle.

### Mechanism

Hooks are JSON command hooks, one JSON object in on stdin and one JSON object out on stdout. Config file: `~/.cursor/hooks.json`. The CLI and the IDE share it, and Cursor reloads it when the file is saved. Shape: `{"version":1,"hooks":{"<camelCaseStep>":[{"command":"<shell string>","timeout":<sec>,"matcher":"<regex>","failClosed":false,"loop_limit":5|null,"type":"command"}]}}`.

Sources are loaded in this order and all of them run: enterprise (`/etc/cursor/hooks.json`, `/Library/Application Support/Cursor/hooks.json`, `C:\ProgramData\Cursor\hooks.json`), team, project `<root>/.cursor/hooks.json` (trusted workspaces only), user `~/.cursor/hooks.json`, then the Claude Code import from `<root>/.claude/settings.local.json`, `<root>/.claude/settings.json` and `~/.claude/settings.json`, then Claude plugin hooks.

- **Parsing:** the file is parsed as JSONC by stripping `//` and `/* */` comments. Setup must tolerate comments rather than fail in serde_json. Today's `~/.cursor/hooks.json` holds only `postToolUse:[]` and `preToolUse:[]`.
- **Working directory:** user hooks run in `~/.cursor/`, project hooks in the project root, and hooks imported from `~/.claude/settings.json` in `~/.claude/`. The payload's `cwd` exists on tool events only.
- **Default timeout:** 60 s (`yB=60`).
- **Exit codes:** 0 means stdout is used. 2 blocks the action. Any other non-zero exit fails open. Empty stdout is logged as failed but does not block.
- **Invalid JSON:** for the permission steps (preToolUse, beforeShellExecution, beforeMCPExecution, beforeReadFile, beforeTabFileRead, subagentStart), invalid JSON or an invalid response blocks the action even with `failClosed:false`.
- **How the command runs:** Linux and macOS run it through the terminal shell executor; the CLI writes the payload straight to stdin. Windows uses PowerShell (see windows_macos).

Proposed `oboete setup cursor` writes these user hooks, each with the absolute `current_exe()` path:
- `sessionStart` → `oboete hook cursor SessionStart` (timeout 10)
- `beforeSubmitPrompt` → `UserPromptSubmit`
- `postToolUse` → `PostToolUse`
- `postToolUseFailure` → `PostToolUseFailure`
- `afterAgentResponse` → `Stop`
- `preCompact` → `PreCompact` (marker only)
- `sessionEnd` → `SessionEnd`

Do not register `preToolUse`: it is a permission step, and injection works without it. Do not register `stop`: it carries only `status`. `afterShellExecution`, `afterMCPExecution` and `afterFileEdit` repeat what `postToolUse` already gives. Removal: strip only our entries and keep `version`.

Required handler changes:
1. `resolve_agent`: when `agent=="claude"` and the payload has `cursor_version`, drop the event, because Cursor's import of `~/.claude/settings.json` is running the Claude hooks. Alternative: relabel it `cursor` only while `~/.cursor/hooks.json` does not contain our handler, like `grok_delivers`. Either way, drop it once ours is there.
2. cwd = `workspace_roots[0]`, falling back to `$CURSOR_PROJECT_DIR`, then to `cwd`.
3. Failed-tool output comes from `error_message`.
4. Injection output for cursor is flat `{"additional_context": text}`. Keep the text at or under about 9,500 characters, because Cursor measures the limit in JS string units (UTF-16).

### Events

| Event | oboete event | Payload | Injection |
| --- | --- | --- | --- |
| sessionStart | SessionStart (+ injection) | Common fields on every event: conversation_id, generation_id, model, model_id?, model_params?, hook_event_name:"sessionStart", cursor_version, workspace_roots:[abs], user_email\|null, transcript_path\|null, session_id (= conversation_id). This event adds is_background_agent:false and composer_mode (e.g. "agent"). There is no `cwd` and no `source`. In the CLI it fires only for a new chat: it is skipped on --resume/--continue (the `!Mt` guard in run-agent.tsx), which gives decision 3 without a source field. It is fired without waiting (fire-and-forget). | Yes. Output {"additional_context":"..."} adds it to the conversation's initial system context; the CLI delivers it through hooksAdditionalContextPromise in both the TUI and -p. The text is trimmed and must be 10,000 chars or fewer (hooks-carriers), otherwise the whole context is DROPPED, not truncated. Output {"env":{...}} sets env vars for later hooks in the session. The CLI also reads the nested hookSpecificOutput form (compat flag hardcoded true in the TUI and worker), but only when hookEventName is empty or "SessionStart"; use the flat form so the IDE path is not in doubt. |
| beforeSubmitPrompt | UserPromptSubmit | prompt (the typed text), attachments:[{type:"file"\|"rule", file_path}], composer_mode (agent-exec path), plus the common fields. No cwd. | Yes: {"additional_context"} is accepted and carried in the bundle (not in the docs), with the same 10k drop rule. {"continue":false,"user_message"} blocks the prompt. Not a permission step, so bad output fails open. Useful for re-injecting after preCompact. The TUI fires it only when user or project hooks.json has a beforeSubmitPrompt entry; a Claude-imported UserPromptSubmit alone does not trigger it. Not fired by the local code in -p mode. |
| postToolUse | PostToolUse | tool_name (Shell, Read, Write, Grep, Delete, Task, MCP:<tool>, ...), tool_input (object), tool_output (JSON-stringified result, e.g. "{\"exitCode\":0,\"stdout\":...}"), duration (ms), tool_use_id, cwd, plus the common fields. Fires for every tool, MCP included. | {"additional_context"} goes into the conversation after the tool result (10k drop rule). For MCP tools {"updated_mcp_tool_output"} replaces the output. Not needed for oboete. |
| postToolUseFailure | PostToolUseFailure | tool_name, tool_input, error_message, failure_type ("error"\|"timeout"\|"permission_denied"), duration, tool_use_id, is_interrupt, plus the common fields. The docs also list cwd. The handler needs `error_message` added to its output keys. | {"additional_context"} (10k drop rule). |
| afterAgentResponse | Stop (assistant final message; then spawn observe) | text (the assistant's final text for the turn), input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, plus the common fields. In the TUI it fires once per turn in finalizeTurn, without waiting, just before `stop`, and only on success. | No (fire-and-forget, no output fields). The TUI fires it only when user or project hooks.json defines it; not fired in -p mode. |
| stop | (not registered). Could replace afterAgentResponse as the observe trigger. | status ("completed"\|"aborted"\|"error"), loop_count, token counts, plus the common fields. No assistant text. | {"followup_message"} auto-submits a new user turn (a loop). Not wanted. |
| preCompact | PostCompact (a marker only; there is no summary) | trigger ("auto"\|"manual"), context_usage_percent, context_tokens, context_window_size, message_count, messages_to_compact, is_first_compaction, plus the common fields. Arrives through the server-driven agent-exec path. No compaction summary exists and there is no postCompact event. | Only {"user_message"}, which the user sees and the model does not. To restore context after compaction, record the marker and inject on the next beforeSubmitPrompt. |
| sessionEnd | SessionEnd (then spawn observe) | session_id, reason ("completed"\|"aborted"\|"error"\|"window_close"\|"user_close"), duration_ms, is_background_agent, final_status, error_message?, plus the common fields. In the CLI it fires when the process exits; it fires in -p mode too. | No (fire-and-forget). |
| subagentStart / subagentStop | optional: subagentStop.summary as a tool-like event in the parent session | subagentStart: subagent_id, subagent_type, task, parent_conversation_id, tool_call_id, subagent_model, is_parallel_worker, git_branch. subagentStop: subagent_id, subagent_type, status, task, description, summary, duration_ms, message_count, tool_call_count, modified_files, loop_count, parent_conversation_id, agent_transcript_path. | subagentStart: allow/deny plus additional_context. subagentStop: followup_message. |
| preToolUse | (do not register) | tool_name, tool_input, tool_use_id, cwd, plus the common fields. | additional_context is accepted, but this is a permission step: invalid JSON or an invalid response BLOCKS the tool. Not needed, because sessionStart injection works in Cursor. |
| afterShellExecution / afterMCPExecution / afterFileEdit / afterAgentThought / beforeReadFile / beforeShellExecution / beforeMCPExecution / beforeTabFileRead / afterTabFileEdit / workspaceOpen | none (postToolUse already covers the tool data; tab and app lifecycle events are out of scope) | afterShellExecution: command, output, duration, sandbox. afterMCPExecution: tool_name, tool_input (string), mcp_server_name, result_json, duration. afterFileEdit: file_path, edits[{old_string,new_string}]. afterAgentThought: text, duration_ms. workspaceOpen has no conversation_id or session_id. | No, except the permission-style `before*` events, which only allow/deny. |

### What is missing and the workaround

1. **No assistant text on `stop`.** Use `afterAgentResponse.text` instead. The fallback is `transcript_path`: `~/.cursor/projects/<workspace path with non-alphanumerics turned into '-'>/agent-transcripts/<id>/<id>.jsonl` (or `.txt`, or the legacy `agent-transcripts/<id>.jsonl`). It is null when transcripts are off.

2. **No compaction summary and no postCompact event.** `preCompact` is only a marker. Re-inject on the next `beforeSubmitPrompt` through `additional_context`. The summary might be in the transcript JSONL, but I did not verify that.

3. **No `cwd` on session-level events.** Use `workspace_roots[0]` or `$CURSOR_PROJECT_DIR`. The handler's current `.` fallback resolves to `~/.cursor` (user hooks) or `~/.claude` (Claude-imported hooks), so sessions land in the wrong repo and injection searches the wrong repo.

4. **Wrong injection output format for Cursor.** The current `hookSpecificOutput` with `hookEventName: <arg>` works only if the arg is the Claude-cased name and the compat flag is on. Print a flat `additional_context` for `cursor` and keep it under the limit, because oversize context is dropped entirely.

5. **Live bug on this PC today: Cursor already runs oboete's Claude hooks.** cursor-agent reads `~/.claude/settings.json` hooks with no setting check in the CLI code (the docs call it a setting that is "on by default"). Cursor accepts that file because it holds at least one group with a `matcher` key, and it does here. Result:
   - `oboete hook claude SessionStart`, `PostToolUse` and `SessionEnd` already fire in cursor-agent, stored as agent `claude`. `SessionStart` is keyed to repo `~/.claude`, and its `hookSpecificOutput` injection is accepted through compat, so Cursor chats receive the wrong repo's context.
   - `UserPromptSubmit` and `Stop` fire from the import only when Cursor's own user or project hooks define `beforeSubmitPrompt` or `stop`; with today's `~/.cursor/hooks.json` they do not.
   - `PostToolUseFailure` and `PostCompact` are unknown to Cursor and skipped.
   - Cursor's own de-duplication removes a Claude hook only when its command string exactly equals a Cursor hook's, so `hook claude X` and `hook cursor x` would both run.
   - Fix: in `resolve_agent`, treat a payload that has `cursor_version` (or env `CURSOR_VERSION`) arriving at `hook claude` as Cursor's, and drop it, at least once `~/.cursor/hooks.json` holds our handler. This is the same pattern as the Grok compat handling.

6. **Headless `-p` loses prompts and replies.** The bundle shows `beforeSubmitPrompt`, `afterAgentResponse` and `stop` fired only from `ui.tsx`, the interactive TUI. In `-p` only `sessionStart`, `sessionEnd` and the tool hooks fire. A server-driven agent-exec path could still send them; I did not run it.

7. **Subagent tool calls may arrive under the child's conversation_id** (separate session rows). `subagentStop.parent_conversation_id` can link them back. Unverified.

### MCP registration

The CLI and the IDE share one user-level file, `~/.cursor/mcp.json` (project level: `<root>/.cursor/mcp.json`). Shape: `{"mcpServers":{"oboete":{"command":"/abs/path/oboete","args":["mcp"]}}}`. `"type":"stdio"` is documented as required, but the existing entries on this PC omit it. `env` and `envFile` are optional. Interpolation `${env:X}`, `${userHome}` and `${workspaceFolder}` works in `command`, `args` and `env`.

Setup should merge the entry with `read_json_object`/`write_json` like the other agents, touch only `mcpServers.oboete`, and take a backup once. Today the file holds `gitnexus` and `codex-security`.

**Approval in the CLI.** Each server needs approval per project root (the git root). Approvals live in `~/.cursor/projects/<sanitized root>/mcp-approvals.json` as `"<name>-<hash>"` (for example `gitnexus-314fc02b4cc9454d`). The interactive TUI asks the first time. For `-p`, use `--approve-mcps` or run `agent mcp enable oboete` once in that project. Setup cannot pre-approve without reproducing the hash, so this is a manual step for the user or printed by doctor. `agent mcp list` shows status.

**Approval in the IDE.** Settings → MCP toggle. Tool calls ask for approval unless auto-run or an allowlist is set.

**Server working directory.** `McpSdkClient.fromCommand` spawns with `cwd: config.cwd`. With no `cwd` it inherits cursor-agent's own cwd, which is the workspace when you launch it in the project. That matches decision 10's default-repo assumption for the CLI. For the IDE it is unverified, so `search` should keep its `repo`/`all` arguments as the escape hatch.

### Headless runs and self-capture

Print mode is `cursor-agent -p/--print "<prompt>"`, with `--output-format text|json|stream-json`, `--stream-partial-output`, `--force/--yolo`, `--approve-mcps`, `--trust`, `--workspace`, `--mode plan|ask` and `--resume/--continue`.

Which hooks fire in `-p` (from the bundle, not run):
- **Fire:** `sessionStart`, including its `additional_context` (run-agent.tsx, before the headless branch). `sessionEnd`. The generic tool hooks `preToolUse`, `postToolUse` and `postToolUseFailure` (the shared hooks-exec wrapper). Hooks from `~/.claude/settings.json` load here too.
- **Do not fire from local code:** `beforeSubmitPrompt`, `afterAgentResponse` and `stop`. They live only in `src/ui.tsx`, and `src/headless.ts` has no hook calls.
- **Server-driven:** `preCompact` and `subagentStart`/`subagentStop` come through the server-driven agent-exec path.

**Self-capture risk.** oboete's provider chain does not start cursor-agent today, so there is no self-capture from Cursor. The real risk runs the other way: every Cursor session, including headless ones, runs oboete's Claude hooks (see missing, item 5).

If `cursor-agent -p` is ever added as a provider, pass `OBOETE_SKIP=1`. Hook processes get `CURSOR_PROJECT_DIR`, `CURSOR_VERSION`, `CURSOR_USER_EMAIL`, `CURSOR_TRANSCRIPT_PATH`, `CLAUDE_PROJECT_DIR`, `CURSOR_CODE_REMOTE` and the session env on top of the spawn environment. Every spawn helper I found merges `{...process.env, ...extra}`, so `OBOETE_SKIP` should reach the hook, but I did not confirm this for the terminal executor. There is no flag that turns hooks off.

Interactive resume (`--resume`/`--continue`) skips `sessionStart`, which matches decision 3.

### Windows and macOS

**macOS:** the same paths as Linux: `~/.cursor/hooks.json`, `~/.cursor/mcp.json`, and `~/.claude/settings.json` imported. Enterprise file: `/Library/Application Support/Cursor/hooks.json`. The command runs through the shell executor with the JSON on stdin, so the setup code for Linux applies unchanged.

**Windows native (IDE and CLI).**
- Files: `%USERPROFILE%\.cursor\hooks.json`, `%USERPROFILE%\.cursor\mcp.json`, `%USERPROFILE%\.claude\settings.json` imported. Enterprise file: `C:\ProgramData\Cursor\hooks.json`.
- Hook commands run in PowerShell, so the command string must be PowerShell syntax: `& "C:\...\oboete.exe" hook cursor SessionStart`. Cursor adds `& ` itself when the command starts with a quote; claude-mem writes it explicitly on win32.
- Payload in the CLI: direct stdin (`commandHookPayloadTransport: "stdin"`).
- Payload in the `windows_temp_file` mode: `$OutputEncoding=[Text.Encoding]::UTF8; Get-Content -LiteralPath '<tmp>\cursor-hooks-*\payload.json' -Raw | & { $input | <command> }`. The JSON still arrives on stdin, but it may carry a UTF-8 BOM or trailing CRLF. `run_stdin` should strip a leading BOM before `serde_json` parses it.
- `workspace_roots` and `cwd` are Windows paths, which `repo::key` must handle.

**WSL (this PC):** cursor-agent inside WSL reads `/home/jura/.cursor/*` and `/home/jura/.claude/settings.json`. The Windows home's `.cursor` is separate: it currently holds git-ai hooks and no oboete entry. Setup run in WSL never reaches a Windows IDE.

When the Windows IDE opens a WSL folder through `anysphere.remote-wsl`, the hooks probably run on the remote side (the docs define `CURSOR_CODE_REMOTE="true"` for remote workspaces), but which `hooks.json` is read is unverified. The IDE is not installed right now.

### Reference implementations

1. **claude-mem 13.25.3** (github.com/thedotmack/claude-mem; installed at `~/.claude/plugins/cache/thedotmack/claude-mem/13.25.3/scripts/worker-service.cjs`). Its Cursor installer writes `hooks.json` as: `beforeSubmitPrompt: [session-init, context]` (injection happens on the prompt event), `afterMCPExecution` and `afterShellExecution` → observation, `afterFileEdit` → file-edit, `stop` → summarize. Command form: `<"& " on win32>"<bun>" "<worker-service.cjs>" hook cursor <x>`. Supports user, project and enterprise scope.
2. **git-ai** (`C:\Users\jura\.cursor\hooks.json` on this PC). Registers `"C:\Users\jura\.git-ai\bin\git-ai.exe checkpoint cursor --hook-input stdin"` on `preToolUse` and `postToolUse`. Shows the Windows command shape; its IDE extension is `git-ai.git-ai-vscode`.
3. **Cursor docs.** Hooks reference: https://cursor.com/docs/hooks.md. Claude Code import (event mapping, nested-output compat): https://cursor.com/docs/reference/third-party-hooks.md. MCP: https://cursor.com/docs/mcp.md and https://cursor.com/docs/cli/mcp.md.
4. **Installed bundle** (source of record for the CLI) in `~/.local/share/cursor-agent/versions/2026.09.15-d2fe57e/`:
   - `index.js`: module `../hooks/dist/index.js` (step list, Claude map, output normalisation `x`/`W`/`H`, validators); agent-exec hook request switch; MCP spawn.
   - `190.index.js`: hooks-exec (config loader paths, `executeHookForStep` common fields, env, cwd per source, `executeCommandHook` exit and JSON semantics, PowerShell transport); hooks-carriers (10,000-char cap).
   - `7021.index.js`: `src/run-agent.tsx` (sessionStart/End); `src/ui.tsx` (beforeSubmitPrompt, stop, afterAgentResponse); `src/after-agent-hooks.ts`; `src/headless.ts` (no hooks).

### Uncertain

- Cursor IDE: not installed on this PC now (only leftover C:\Users\jura\.cursor and %APPDATA%\Cursor logs up to 2026-08-11). Its version, whether it turns on the nested hookSpecificOutput compat, and the working directory of its MCP servers are unverified. The IDE hook behaviour described comes only from the docs.
- In -p mode: beforeSubmitPrompt, afterAgentResponse and stop appear only in ui.tsx. This is read from the bundle, not run; the server-driven agent-exec path might still send them.
- The TUI fires beforeSubmitPrompt, stop and afterAgentResponse only when user or project hooks.json defines those steps, so Claude-imported UserPromptSubmit and Stop alone do not fire. Read from the ui.tsx gates; not run.
- Whether hook processes inherit the full process.env (so OBOETE_SKIP and PATH reach the hook) through the terminal executor. The other spawn helpers merge process.env; this one was not confirmed.
- Whether tool calls inside subagents carry the child's conversation_id (separate oboete sessions) or the parent's.
- Whether afterAgentResponse can fire more than once per turn in the IDE (the docs say 'after the agent has completed an assistant message'). In the TUI it is once per turn, in finalizeTurn.
- Where Cursor writes the compaction summary (the transcript JSONL?), and the transcript line format.
- A Windows IDE opening a WSL folder through remote-wsl: which ~/.cursor/hooks.json it reads and where hook commands run.
- The IDE's 'Include Third-Party Plugins, Skills, and Other Configs' setting (on by default per the docs) gates the ~/.claude/settings.json import. I found no such gate on the CLI's loader path, so the CLI appears to load it always.
- Whether mcp.json accepts a `cwd` field for stdio servers (the code reads config.cwd, but the docs do not list it), and what ${workspaceFolder} resolves to in the global ~/.cursor/mcp.json.

### Fact-check corrections

- **wrong**: Spec's mechanism section claims: 'The CLI also reads the nested hookSpecificOutput form (compat flag hardcoded true in the TUI and worker), but only when hookEventName is empty or "SessionStart"'. — The compat shim is not SessionStart-only. In index.js (module '../hooks/dist/index.js'), the eligible-for-additional_context set is `v = new Set([n.sessionStart, n.beforeSubmitPrompt, n.preToolUse, n.postToolUse, n.postToolUseFailure])` (also mirrored in 190.index.js as HOOK_STEPS_SUPPORTING_ADDITIONAL_CONTEXT, which throws 'lists X but HookStepAdditionalContextEventName does not include it' if the two lists ever diverge). For each of those 5 steps the nested-form check `W(hookSpecificOutput, expectedName)` accepts an EMPTY/undefined hookEventName OR the step's own Claude name via the inverse map `i` built from `{PreToolUse, PostToolUse, UserPromptSubmit:beforeSubmitPrompt, SessionStart, ...}` -- i.e. 'PreToolUse' gates preToolUse, 'PostToolUse' gates postToolUse, 'UserPromptSubmit' gates beforeSubmitPrompt, 'SessionStart' gates sessionStart. postToolUseFailure has no Claude-name entry in that map, so only the flat form works there. Also note: the code checks a FLAT camelCase `additionalContext` field first (`t.additionalContext`), separately from Cursor's own native snake_case `additional_context` field and from the nested `hookSpecificOutput.additionalContext` fallback -- three different shapes are actually accepted, not the one the spec describes. The spec's practical recommendation (send the flat snake_case `additional_context` for cursor) is still correct and safest, but the stated scope/mechanics of the compat path is materially narrower/wrong. (~/.local/share/cursor-agent/versions/2026.09.15-d2fe57e/index.js, module '../hooks/dist/index.js' (search for `new Set([n.sessionStart,n.beforeSubmitPrompt,n.preToolUse,n.postToolUse,n.postToolUseFailure])`, and functions H/V/W/z/x near offset ~4357260); 190.index.js module '../hooks-carriers/dist/index.js' (HOOK_STEPS_SUPPORTING_ADDITIONAL_CONTEXT check).)
- **wrong**: 'afterAgentResponse ... fires once per turn in finalizeTurn' (mechanism/uncertain notes attribute the firing site to a function named finalizeTurn). — The actual call site that fires `afterAgentResponse` (and `afterAgentThought`) is not named `finalizeTurn`; it's the anonymous exported function `l` (and `i`) in `./src/after-agent-hooks.ts` (7021.index.js), gated by the local `s(...)` helper described above. A function literally named `finalizeTurn` does exist elsewhere in 7021.index.js, but it belongs to an unrelated file-change-tracking class (`getTurnChanges`/`finalizeTurn`/`cleanup` on a turn-snapshot tracker) and has nothing to do with hook dispatch. Drop the 'finalizeTurn' attribution or replace it with 'src/after-agent-hooks.ts, gated by the same-file `s()` helper'. (7021.index.js: `finalizeTurn(e){...}` (file-change tracker, unrelated) vs. module `./src/after-agent-hooks.ts` (actual hook dispatch site).)

### Sources

- https://cursor.com/docs/hooks.md
- https://cursor.com/docs/reference/third-party-hooks.md
- https://cursor.com/docs/mcp.md
- https://cursor.com/docs/cli/mcp.md
- /home/jura/.local/share/cursor-agent/versions/2026.09.15-d2fe57e/index.js (module ../hooks/dist/index.js, ../agent-exec/dist/index.js, McpSdkClient.fromCommand, ../utils/dist/workspace-paths.js)
- /home/jura/.local/share/cursor-agent/versions/2026.09.15-d2fe57e/190.index.js (../hooks-exec/dist/index.js, ../hooks-carriers/dist/index.js)
- /home/jura/.local/share/cursor-agent/versions/2026.09.15-d2fe57e/7021.index.js (src/run-agent.tsx, src/ui.tsx, src/after-agent-hooks.ts, src/headless.ts)
- /home/jura/.local/share/cursor-agent/versions/2026.09.15-d2fe57e/5926.index.js and 3233.index.js (hook executor construction, enableClaudeNestedHookSpecificOutputCompatibility)
- cursor-agent --help, cursor-agent mcp --help (2026.09.15-d2fe57e)
- /home/jura/.cursor/hooks.json, /home/jura/.cursor/mcp.json, /home/jura/.cursor/projects/home-jura/mcp-approvals.json (shape only)
- /home/jura/.claude/settings.json (hook groups: oboete entries have no matcher; developer groups do, so Cursor accepts the file)
- /mnt/c/Users/jura/.cursor/hooks.json (git-ai reference, Windows command shape)
- /home/jura/.claude/plugins/cache/thedotmack/claude-mem/13.25.3/scripts/worker-service.cjs (claude-mem Cursor installer)
- /home/jura/projects/oboete/src/hook.rs, src/setup.rs, docs/m1.md decisions 1-4, 7, 10, 12
