# oboete contract probes

Verification-gate (R13) runs under the isolated dogfood user. Statuses: pass / fail / blocked / skipped.

## 2026-09-03 run 2026-09-03T10-22-04-666Z

| tool | version |
|---|---|
| date | 2026-09-03T10:22:04.667Z |
| claude | 2.1.259 (Claude Code) |
| codex | codex-cli 0.153.0 |
| grok | grok 1.0.17 (a549186d9d39) [alpha] |
| pi | 0.84.4 |
| node | v24.20.0 |

| id | R13 row | agent | status |
|---|---|---|---|
| claude-payload-shapes | Native tool payload shapes for read/write/edit/bash on all four agents | claude | pass |
| claude-oversized-stdin | Hook runner behaviour when the hook exits with unread stdin above 1 MB | claude | pass |
| codex-payload-shapes | Native tool payload shapes for read/write/edit/bash on all four agents | codex | pass |
| codex-oversized-stdin | Hook runner behaviour when the hook exits with unread stdin above 1 MB | codex | pass |
| grok-payload-shapes | Native tool payload shapes for read/write/edit/bash on all four agents | grok | pass |
| grok-oversized-stdin | Hook runner behaviour when the hook exits with unread stdin above 1 MB | grok | pass |
| pi-payload-shapes | Native tool payload shapes for read/write/edit/bash on all four agents | pi | pass |
| pi-oversized-stdin | Hook runner behaviour when the hook exits with unread stdin above 1 MB | pi | skipped |
| agent-cli-json | `agent-cli` preset: headless JSON output of `claude -p`, `codex exec`, `grok -p` for a summarization prompt | providers | pass |
| provider-nim | NIM / OpenRouter / Gemini / Anthropic transport, auth header, model id | providers | skipped |
| provider-openrouter | NIM / OpenRouter / Gemini / Anthropic transport, auth header, model id | providers | skipped |
| provider-gemini | NIM / OpenRouter / Gemini / Anthropic transport, auth header, model id | providers | skipped |
| provider-anthropic | NIM / OpenRouter / Gemini / Anthropic transport, auth header, model id | providers | skipped |
| provider-workers-ai | NIM / OpenRouter / Gemini / Anthropic transport, auth header, model id | providers | skipped |

- **claude-payload-shapes**: Read input=[file_path] output=[type,file] path=tool_input.file_path (absolute); echoed back at tool_response.file.filePath (camelCase) (match recon; out match recon); Write input=[file_path,content] output=[type,filePath,content,structuredPatch,originalFile,userModified] path=tool_input.file_path ; tool_response.filePath (match recon; out match recon); Edit input=[file_path,old_string,new_string,replace_all] output=[filePath,oldString,newString,originalFile,structuredPatch,userModified,replaceAll] path=tool_input.file_path ; tool_response.filePath (match recon; out match recon); Bash input=[command,description] output=[stdout,stderr,interrupted,isImage,noOutputExpected] path=tool_input.command ; tool_response has NO command echo — Pre/Post must be joined by tool_use_id (match recon; out match recon); exit=0 elapsed_s=14.2 session=b8f0a8f6-5c08-4543-a4ca-8fc7c007beca model=claude-opus-5[1m]
- **claude-oversized-stdin**: exit=0; DONE=true; Stop=true; SessionEnd=true; unread_handlers=1; read_PostToolUse=1 sizes=31438; hook_lines=none; elapsed_s=13.1
- **codex-payload-shapes**: bash-read input=[command] output=[(string)] path=tool_input.command — path is inside the shell command text; apply_patch-add input=[command] output=[(string)] path=tool_input.command *** Add File: <path>; apply_patch-update input=[command] output=[(string)] path=tool_input.command *** Update File: <path>; bash input=[command] output=[(string)] path=tool_input.command; exit=0 elapsed_s=28.6 session=01a066ca-abee-7091-bb4d-7f36c106d305 model=gpt-5.6-sol
- **codex-oversized-stdin**: exit=0; DONE=true; Stop=true; SessionEnd=true; unread_handlers=1; read_PostToolUse=1 sizes=1189; hook_lines=none; elapsed_s=9.7
- **grok-payload-shapes**: read_file input=[target_file] output=[type,FileContent] path=toolInput.target_file (relative); absolute at toolResult.FileContent.absolute_path (match recon; out match recon); write input=[file_path,content] output=[type,EditsApplied] path=toolInput.file_path (relative); absolute at toolResult.EditsApplied.absolute_path; toolResult.type is SearchReplace (match recon; out match recon); search_replace input=[file_path,old_string,new_string] output=[type,EditsApplied] path=toolInput.file_path (relative); absolute at toolResult.EditsApplied.absolute_path (match recon; out match recon); run_terminal_command input=[command,description] output=[type,output,output_for_prompt,exit_code,command,truncated,signal,timed_out,description,current_dir,output_file,total_bytes,was_bare_echo] path=toolInput.command; output is a byte array, output_for_prompt is the string (match recon; out match recon); exit=0 elapsed_s=10.7 session=01a066cb-3fb8-7473-8c38-9d659d74bf3d model=grok-4.6-build
- **grok-oversized-stdin**: exit=0; DONE=true; Stop=true; SessionEnd=true; unread_handlers=1; read_PostToolUse=1 sizes=164527; hook_lines=none; elapsed_s=6.7
- **pi-payload-shapes**: read input=[path] output=[(array)] path=input.path (match recon); write input=[path,content] output=[(array)] path=input.path (match recon); edit input=[path,edits] output=[(array),details] path=input.path (edits is [{oldText,newText}]) (match recon); bash input=[command] output=[(array)] path=input.command (match recon); exit=0 elapsed_s=21.4 session=01a066cb-82e1-778e-ba35-d29b422dd381 model=gpt-5.6-luna
- **pi-oversized-stdin**: Pi has no hook process; the equivalent is oboete's own capture child, probed after the child exists
- **agent-cli-json**: claude pass 6.21s text=result model=claude-opus-5[1m] ver=2.1.259 (Claude Code); codex pass 7.00s text=output-last-message model=none ver=codex-cli 0.153.0; grok pass 7.95s text=text model=grok-4.6-build ver=grok 1.0.17 (a549186d9d39) [alpha]; pi pass 5.74s text=turn_end text blocks model=gpt-5.6-luna ver=0.84.4
- **provider-nim**: dummy-key self-check: HTTP 410 (unexpected) 90ms; credential absent
- **provider-openrouter**: credential absent
- **provider-gemini**: credential absent
- **provider-anthropic**: credential absent
- **provider-workers-ai**: credential absent


## Findings 2026-09-03 (maintainer notes on the runs above)

Rows of research.md R13 closed or narrowed by today's runs; the evaluation against pass conditions is
recorded in docs/research/m1-amendments-2026-09.md (task T011).

- **Native tool payload shapes** (row 1): fixtures for 16 tool payloads are committed under
  `test/contracts/<agent>/`. Shapes that the adapters (T028) must handle: Claude Code `tool_input` is
  snake_case and `tool_response` camelCase with a different shape per tool (Read nests `file`, Edit has no
  `type`, Bash echoes no command); Codex has no read tool (reads arrive as Bash commands), writes and
  edits arrive as `apply_patch` whose path is only inside the patch text, and `tool_response` is a bare
  string; Grok Build `write` returns `toolResult.type = "SearchReplace"` (dispatch on `toolName`),
  `run_terminal_command.output` is a byte array (`output_for_prompt` is the string), and tool events carry
  no `promptId`; Pi's `tool_result` carries `input`, `content`, `isError` (and `details` for edit) and is
  the single subscription point.
- **Hook runner behaviour with unread stdin** (row "unread stdin above 1 MB"): Claude Code, Codex and
  Grok Build all completed the turn, ran the later hooks (Stop, SessionEnd) and surfaced no hook error
  when the PostToolUse handler exited 0 without reading. The runners also cap what a hook receives: after
  a 1.2 MB tool result the normally reading handler saw about 31 KB (Claude Code), 4.8 KB (Codex) and
  165–190 KB (Grok Build). So the 1 MB read bound of A7/A14 is never reached through a tool *result*;
  a large tool *input* (a Write with a huge `content`) was not probed.
- **Codex `[hooks.state] trusted_hash`**: the rule in docs/research/oboete-contracts-2026-09-02.md is
  incomplete. The preimage handler object must contain `"async": false`, and the group's `matcher` is part
  of the preimage when present (verified 12/12 hooks firing without `--dangerously-bypass-hook-trust`,
  and against 5 real hashes). `scripts/e2e/probe-lib/trusthash.mjs` is the corrected implementation and
  the installer (T049) must use the same rule.
  Correction 2026-09-04: that rule is only right for handlers without a `timeout` (the probes stripped
  it). A live run of codex-cli 0.153.2 with `"timeout": 12` in hooks.json and two trust rows fired only
  the group hashed with the configured value 12, not the one hashed with the default 600, so the
  installer's rule in `src/setup/codex-trust.ts` (configured timeout in the preimage) is the verified
  one. The clamp for SessionEnd/Interrupt was not probed separately; oboete writes 3 s, which equals
  the cap either way.
- **`agent-cli` preset** (row "headless JSON output"): all four CLIs return the model text in a stable
  place (`claude -p --output-format json` → `result`; `codex exec --json --output-last-message <file>` →
  the file; `grok -p --output-format json` → `text`, but that field concatenates every assistant message,
  so the Stop hook's `lastAssistantMessage` is the clean source; `pi -p --mode json` → the `text` blocks of
  the last `turn_end` message, which can start with a `thinking` block). Codex's JSON stream carries no
  model id. Fence stripping was not exercised (no CLI returned fences today).
- **Model ids reported at runtime**: `claude-opus-5[1m]` (with a `claude-haiku-4-5` side call),
  `gpt-5.6-sol` (Codex), `grok-4.6-build`, `gpt-5.6-luna` (Pi via `openai-codex`). Their documented
  windows and the runtime-id → catalog-id rules are in docs/research/context-windows.md.
- **Provider presets** (rows "transport, auth header, model id" and "response_format"): first run skipped
  (no key); on 2026-09-04 the owner filled `~/.oboete-credentials` (`OBOETE_<PRESET>_API_KEY`, A17) for
  NIM, OpenRouter, Gemini and Cloudflare, and all four pass (see the R13 evaluation table below and the
  run section 2026-09-03T16-48-21-133Z). Cloudflare: the token has Workers AI Read, so the direct
  `/accounts/<id>/ai/run/@cf/...` call works; routing through the owner's AI Gateway `oboete` is the same
  token on `/accounts/<id>/ai/v1/chat/completions` plus the header `cf-aig-gateway-id: oboete`
  (Cloudflare docs, AI Gateway REST API, Aug 2026); a token holding only the AI Gateway permission would
  return 401 code 10000. Optional for M1; worth adopting for logging and caching in the Workers AI adapter.
- **Hermeticity**: every run left the isolated user's real configuration untouched (seven protected paths,
  sha256 or absence identical before and after).
## 2026-09-03 run 2026-09-03T15-43-30-145Z

| tool | version |
|---|---|
| date | 2026-09-03T15:43:30.145Z |
| claude | 2.1.259 (Claude Code) |
| codex | codex-cli 0.153.0 |
| grok | grok 1.0.17 (a549186d9d39) [alpha] |
| pi | 0.84.4 |
| node | v24.20.0 |

| id | R13 row | agent | status |
|---|---|---|---|
| claude-payload-shapes | Native tool payload shapes for read/write/edit/bash on all four agents | claude | pass |
| claude-oversized-stdin | Hook runner behaviour when the hook exits with unread stdin above 1 MB | claude | pass |
| claude-session-start-sources | Compaction identity and order per agent | claude | pass |
| claude-tool-failure | Native tool payload shapes for read/write/edit/bash on all four agents | claude | pass |
| claude-stop-message | Codex and Grok `PostCompact` payload (summary text field); Grok `Stop` `lastAssistantMessage` field | claude | pass |
| claude-postcompact-payload | Compaction identity and order per agent | claude | fail |
| codex-payload-shapes | Native tool payload shapes for read/write/edit/bash on all four agents | codex | pass |
| codex-oversized-stdin | Hook runner behaviour when the hook exits with unread stdin above 1 MB | codex | pass |
| codex-trust-hash | Codex rollout flush at PostToolUse; TUI trust path | codex | pass |
| codex-session-start-sources | Codex `SessionStart` fires with `source = compact` and `clear` | codex | fail |
| codex-postcompact-payload | Codex and Grok `PostCompact` payload (summary text field); Compaction identity and order per agent | codex | fail |
| codex-rollout-flush | Codex rollout flush at `PostToolUse` | codex | pass |
| codex-tui-trust | TUI trust path | codex | pass |
| codex-mcp-legacy-client | Legacy-era MCP server against Claude Code, Codex, Grok clients (raw frames compared) | codex | pass |
| grok-payload-shapes | Native tool payload shapes for read/write/edit/bash on all four agents | grok | pass |
| grok-oversized-stdin | Hook runner behaviour when the hook exits with unread stdin above 1 MB | grok | pass |
| grok-parallel-batch | Grok parallel batches: whether `additionalContext` attached to several calls of one batch reaches the model once or once per call | grok | fail |
| grok-pretooluse-failed-call | Grok Build `PreToolUse` context on an executed-but-failed call (`PostToolUseFailure`) | grok | pass |
| grok-permission-denied | `PermissionDenied` payload | grok | pass |
| grok-postcompact | Codex and Grok `PostCompact` payload (summary text field); Compaction identity and order per agent | grok | pass |
| grok-resume | Grok Build resume: `SessionStart` `source` value and session id continuity | grok | pass |
| grok-mcp-registration | Grok Build user-scoped MCP registration; Legacy-era MCP server against Grok client | grok | pass |
| grok-stop-messages | Grok Build `Stop` `lastAssistantMessage` field | grok | pass |
| pi-payload-shapes | Native tool payload shapes for read/write/edit/bash on all four agents | pi | pass |
| pi-oversized-stdin | Hook runner behaviour when the hook exits with unread stdin above 1 MB | pi | skipped |
| pi-compaction | Pi compaction event; Compaction identity and order per agent | pi | pass |
| pi-tools | Pi tool registration surface | pi | pass |
| pi-resume-fork | Pi `resume` / `fork`: `session_start` firing and `PI_SESSION_ID` continuity | pi | pass |
| pi-error-surface | Pi durable error surface for extension throws | pi | fail |
| pi-after-provider-response | Pi after_provider_response with openai-codex (recon follow-up) | pi | pass |
| agent-cli-json | `agent-cli` preset: headless JSON output of `claude -p`, `codex exec`, `grok -p` for a summarization prompt | providers | pass |
| provider-nim | NIM / OpenRouter / Gemini / Anthropic transport, auth header, model id | providers | skipped |
| provider-openrouter | NIM / OpenRouter / Gemini / Anthropic transport, auth header, model id | providers | skipped |
| provider-gemini | NIM / OpenRouter / Gemini / Anthropic transport, auth header, model id | providers | skipped |
| provider-anthropic | NIM / OpenRouter / Gemini / Anthropic transport, auth header, model id | providers | skipped |
| provider-workers-ai | NIM / OpenRouter / Gemini / Anthropic transport, auth header, model id | providers | skipped |

- **claude-payload-shapes**: Read input=[file_path] output=[type,file] path=tool_input.file_path (absolute); echoed back at tool_response.file.filePath (camelCase) (match recon; out match recon); Write input=[file_path,content] output=[type,filePath,content,structuredPatch,originalFile,userModified] path=tool_input.file_path ; tool_response.filePath (match recon; out match recon); Edit input=[file_path,old_string,new_string,replace_all] output=[filePath,oldString,newString,originalFile,structuredPatch,userModified,replaceAll] path=tool_input.file_path ; tool_response.filePath (match recon; out match recon); Bash input=[command,description] output=[stdout,stderr,interrupted,isImage,noOutputExpected] path=tool_input.command ; tool_response has NO command echo — Pre/Post must be joined by tool_use_id (match recon; out match recon); exit=0 elapsed_s=19.2 session=31654ffb-33da-460d-9fd7-2a0f3b21873b model=claude-opus-5[1m]
- **claude-oversized-stdin**: exit=0; DONE=true; Stop=true; SessionEnd=true; unread_handlers=1; read_PostToolUse=1 sizes=31424; hook_lines=none; elapsed_s=8.8
- **claude-session-start-sources**: A exit=0 source=startup session=b208fc53-5685-401d-87af-f3f0d5ccbbd0 elapsed_s=21.4; A source=startup expected=startup session=b208fc53-5685-401d-87af-f3f0d5ccbbd0 marker_delivered=true answer=DONE PROBE-SS-startup; B source=resume expected=resume session=b208fc53-5685-401d-87af-f3f0d5ccbbd0 marker_delivered=true answer=DONE PROBE-SS-startup PROBE-SS-resume; C source=fork expected=fork session=e162ac6e-fa71-4495-9025-4d2aecf813f3 marker_delivered=true answer=DONE PROBE-SS-startup PROBE-SS-resume PROBE-SS-fork; id_continuity A==B=true C!=A=true; sources_ok=true id_ok=true
- **claude-tool-failure**: exit=0 elapsed_s=12.8 session=f65937f4-f116-4da1-b263-a55fba0ddb90; Bash PostToolUseFailure keys=[session_id,transcript_path,cwd,prompt_id,permission_mode,effort,hook_event_name,tool_name,tool_input,tool_use_id,error,is_interrupt,duration_ms] error_field=string PostToolUse_also=false; Read PostToolUseFailure keys=[session_id,transcript_path,cwd,prompt_id,permission_mode,effort,hook_event_name,tool_name,tool_input,tool_use_id,error,is_interrupt,duration_ms] error_field=string PostToolUse_also=false
- **claude-stop-message**: exit=0; Stop=true; equal=true equal_trim=true; stop_hook_active=false; background_tasks_key=true; session_crons_key=true; Stop.keys=[session_id,transcript_path,cwd,prompt_id,permission_mode,effort,hook_event_name,stop_hook_active,last_assistant_message,background_tasks,session_crons]; result="DONE"; last_assistant_message="DONE"
- **claude-postcompact-payload**: big.txt_bytes=810527; auto exit=0 elapsed_s=74.1 PostCompact=1 PreCompact=1 usage={"input_tokens":6,"cache_creation_input_tokens":230191,"cache_read_input_tokens":41469,"output_tokens":2255,"output_tokens_details":{"thinking_tokens":137},"server_tool_use":{"web_search_requests":0,"web_fetch_requests":0},"service_tier":"standard","cache_creation":{"ephemeral_1h_input_tokens":230191,"ephemeral_5m_input_tokens":0},"inference_geo":"not_available","iterations":[{"input_tokens":2,"output_tokens":5,"cache_read_input_tokens":13105,"cache_creation_input_tokens":113429,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":113429},"type":"message"}],"speed":"standard"}; auto seq=SessionStart:startup@2026-09-03T15:44:48.829Z \| UserPromptSubmit@2026-09-03T15:44:51.224Z \| PreToolUse:Read@2026-09-03T15:44:55.262Z \| PostToolUse:Read@2026-09-03T15:44:55.295Z \| PreToolUse:Read@2026-09-03T15:44:56.532Z \| PostToolUse:Read@2026-09-03T15:44:56.564Z \| PreToolUse:Read@2026-09-03T15:44:57.183Z \| PostToolUse:Read@2026-09-03T15:44:57.212Z \| PreToolUse:Read@2026-09-03T15:44:58.442Z \| PostToolUse:Read@2026-09-03T15:44:58.471Z \| PreToolUse:Read@2026-09-03T15:44:59.092Z \| PostToolUse:Read@2026-09-03T15:44:59.120Z \| PreToolUse:Read@2026-09-03T15:45:00.347Z \| PostToolUse:Read@2026-09-03T15:45:00.376Z \| PreToolUse:Read@2026-09-03T15:45:00.964Z \| PostToolUse:Read@2026-09-03T15:45:00.992Z \| PreToolUse:Read@2026-09-03T15:45:05.090Z \| PostToolUse:Read@2026-09-03T15:45:05.118Z \| PreToolUse:Read@2026-09-03T15:45:06.417Z \| PostToolUse:Read@2026-09-03T15:45:06.448Z \| PreToolUse:Read@2026-09-03T15:45:07.054Z \| PostToolUse:Read@2026-09-03T15:45:07.082Z \| PreToolUse:Read@2026-09-03T15:45:08.327Z \| PostToolUse:Read@2026-09-03T15:45:08.355Z \| PreToolUse:Read@2026-09-03T15:45:09.594Z \| PostToolUse:Read@2026-09-03T15:45:09.621Z \| PreToolUse:Read@2026-09-03T15:45:10.230Z \| PostToolUse:Read@2026-09-03T15:45:10.258Z \| PreToolUse:Read@2026-09-03T15:45:10.949Z \| PostToolUse:Read@2026-09-03T15:45:10.977Z \| PreCompact:auto@2026-09-03T15:45:11.009Z \| SessionStart:compact@2026-09-03T15:45:57.906Z \| PostCompact:auto@2026-09-03T15:45:57.929Z; auto PreCompact keys=[session_id,transcript_path,cwd,prompt_id,hook_event_name,trigger,custom_instructions] trigger=auto compact_summary=absent; auto PostCompact keys=[session_id,transcript_path,cwd,prompt_id,hook_event_name,trigger,compact_summary] trigger=auto compact_summary=len=9137; tui PostCompact=0 PreCompact=0 error=tui login wall (not sending keys) pane_chars=1086; tui seq=none; tui_error=tui login wall (not sending keys); tui_pane= * ██▄█████▄██ * █████████ * .......█ █ █ █.......................................... Claude Code can be used with your Claude subscription or billed based on API usage through your Console account. Select login method: ❯ 1. Claude account with subscription · Pro, Max, Team, or Enterprise 2. Anthropic Console account · API usage billing 3. 3rd-party platform · Amazon Bedrock, Microsoft Foundry, or Vertex AI; (a) n=1 candidates=[] ok=false note=single observation; candidate keys = [] values=[{}]; (b) auto=false injection SessionStart:compact idx 31 BEFORE PostCompact idx 32 (at 2026-09-03T15:45:57.906Z vs 2026-09-03T15:45:57.929Z); (b) tui=null no PostCompact
- **codex-payload-shapes**: bash-read input=[command] output=[(string)] path=tool_input.command — path is inside the shell command text; apply_patch-add input=[command] output=[(string)] path=tool_input.command *** Add File: <path>; apply_patch-update input=[command] output=[(string)] path=tool_input.command *** Update File: <path>; bash input=[command] output=[(string)] path=tool_input.command; exit=0 elapsed_s=33.3 session=01a067f2-e8e0-7061-881c-7465eb4eba9e model=gpt-5.6-sol
- **codex-oversized-stdin**: exit=0; DONE=true; Stop=true; SessionEnd=true; unread_handlers=1; read_PostToolUse=1 sizes=41304; hook_lines=none; elapsed_s=14.3
- **codex-trust-hash**: events=SessionStart,UserPromptSubmit,PreToolUse,PostToolUse,Stop,SessionEnd; trust_rows=8; exit=0
- **codex-session-start-sources**: A_startup sources=[startup] session=01a067f4-2a06-7d41-8d7b-e5153c6b941f exit=0; B_resume sources=[resume] session=01a067f4-2a06-7d41-8d7b-e5153c6b941f same_session_id=true exit=0; C_compact sources=[startup] events=SessionStart>UserPromptSubmit>PreToolUse>PostToolUse>Stop>SessionEnd; C_timeline=[{"event":"SessionStart","at":"2026-09-03T15:48:02.255Z","source":"startup","trigger":null,"turn_id":null,"session_id":"01a067f4-8dee-7663-a1ac-8ab9fa494854","keys":["session_id","transcript_path","cwd","hook_event_name","model","permission_mode","source"]},{"event":"UserPromptSubmit","at":"2026-09-03T15:48:02.280Z","source":null,"trigger":null,"turn_id":"01a067f4-8e00-71c3-a93e-e4476aca41ae","session_id":"01a067f4-8dee-7663-a1ac-8ab9fa494854","keys":["session_id","turn_id","transcript_path","cwd","hook_event_name","model","permission_mode","prompt"]}]; D_tui sources=[startup] clear_session_ids=[] seed_session=01a067f4-e005-7213-bbb7-6ac2583a16f8; D_new_session_id=no-clear-event; D_tui_blocked=TUI /new produced no extra SessionStart (sources=[startup]); pane=12130db534) ⚠ `--dangerously-bypass-hook-trust` is enabled. Enabled hooks may run without review for this invocation. ⚠ The cloudflare-api MCP server is not logged in. Run `codex mcp login cloudflare-api`. ⚠ MCP startup incomplete (failed: cloudflare-api) › Ask Codex to do anything gpt-5.6-sol default · <run>/codex-session-start-sources/d/repo; observed_sources=[startup,resume]; manual: CODEX_HOME=<tmp hooks.json> tmux `codex --sandbox danger-full-access --ask-for-approval never` in the throwaway repo; wait for composer (›); send a short turn; /compact; /new (expect SessionStart source=clear); /quit. Record events.jsonl labels and pane text.
- **codex-postcompact-payload**: C PreCompact n=1 keys=[session_id\|turn_id\|transcript_path\|cwd\|hook_event_name\|model\|trigger]; C PostCompact n=1 keys=[session_id\|turn_id\|transcript_path\|cwd\|hook_event_name\|model\|trigger] summary=[{"field":null,"length":0}] identity={"ok":false,"candidates":[],"values":[{}],"n":1,"note":"single observation; candidate keys = []"}; C timeline=[{"event":"SessionStart","at":"2026-09-03T15:49:08.093Z","source":"startup","trigger":null,"turn_id":null,"session_id":"01a067f5-979f-7061-b677-e5c5a0d2d0ee","keys":["session_id","transcript_path","cwd","hook_event_name","model","permission_mode","source"]},{"event":"UserPromptSubmit","at":"2026-09-03T15:49:08.117Z","source":null,"trigger":null,"turn_id":"01a067f5-97b2-7de0-aead-d215ec52b90c","session_id":"01a067f5-979f-7061-b677-e5c5a0d2d0ee","keys":["session_id","turn_id","transcript_path","cwd","hook_event_name","model","permission_mode","prompt"]},{"event":"PreCompact","at":"2026-09-03T15:49:11.967Z","source":null,"trigger":"auto","turn_id":"01a067f5-97b2-7de0-aead-d215ec52b90c","session_id":"01a067f5-979f-7061-b677-e5c5a0d2d0ee","keys":["session_id","turn_id","transcript_path","cwd","hook_event_name","model","trigger"]},{"event":"PostCompact","at":"2026-09-03T15:49:15.095Z","source":null,"trigger":"auto","turn_id":"01a067f5-97b2-7de0-aead-d215ec52b90c","session_id":"01a067f5-979f-7061-b677-e5c5a0d2d0ee","keys":["session_id","turn_id","transcript_path","cwd","hook_event_name","model","trigger"]},{"event":"SessionStart","at":"2026-09-03T15:49:15.120Z","source":"compact","trigger":null,"turn_id":null,"session_id":"01a067f5-979f-7061-b677-e5c5a0d2d0ee","keys":["session_id","transcript_path","cwd","hook_event_name","model","permission_mode","source"]}]; C order_b=true PostCompact.at=2026-09-03T15:49:15.095Z <= SessionStart(compact).at=2026-09-03T15:49:15.120Z; no UserPromptSubmit after last PostCompact; D PreCompact n=2 keys=[session_id\|turn_id\|transcript_path\|cwd\|hook_event_name\|model\|trigger ; session_id\|turn_id\|transcript_path\|cwd\|hook_event_name\|model\|trigger]; D PostCompact n=2 keys=[session_id\|turn_id\|transcript_path\|cwd\|hook_event_name\|model\|trigger ; session_id\|turn_id\|transcript_path\|cwd\|hook_event_name\|model\|trigger] summary=[{"field":null,"length":0},{"field":null,"length":0}] identity={"ok":false,"candidates":[],"values":[{},{}],"n":2,"note":"no candidate"}; D timeline=[{"event":"SessionStart","at":"2026-09-03T15:49:35.820Z","source":"startup","trigger":null,"turn_id":null,"session_id":"01a067f6-0a44-7420-a2bf-723e331af25d","keys":["session_id","transcript_path","cwd","hook_event_name","model","permission_mode","source"]},{"event":"UserPromptSubmit","at":"2026-09-03T15:49:35.845Z","source":null,"trigger":null,"turn_id":"01a067f6-0aef-7891-9dbf-28d693a67002","session_id":"01a067f6-0a44-7420-a2bf-723e331af25d","keys":["session_id","turn_id","transcript_path","cwd","hook_event_name","model","permission_mode","prompt"]},{"event":"PreCompact","at":"2026-09-03T15:49:36.399Z","source":null,"trigger":"manual","turn_id":"01a067f6-173f-76a2-b977-5a6905216098","session_id":"01a067f6-0a44-7420-a2bf-723e331af25d","keys":["session_id","turn_id","transcript_path","cwd","hook_event_name","model","trigger"]},{"event":"PostCompact","at":"2026-09-03T15:49:40.654Z","source":null,"trigger":"manual","turn_id":"01a067f6-173f-76a2-b977-5a6905216098","session_id":"01a067f6-0a44-7420-a2bf-723e331af25d","keys":["session_id","turn_id","transcript_path","cwd","hook_event_name","model","trigger"]},{"event":"PreCompact","at":"2026-09-03T15:49:41.239Z","source":null,"trigger":"manual","turn_id":"01a067f6-2acb-7493-bf0c-123b22e6c883","session_id":"01a067f6-0a44-7420-a2bf-723e331af25d","keys":["session_id","turn_id","transcript_path","cwd","hook_event_name","model","trigger"]},{"event":"PostCompact","at":"2026-09-03T15:49:45.022Z","source":null,"trigger":"manual","turn_id":"01a067f6-2acb-7493-bf0c-123b22e6c883","session_id":"01a067f6-0a44-7420-a2bf-723e331af25d","keys":["session_id","turn_id","transcript_path","cwd","hook_event_name","model","trigger"]}]; summary_field=null summary_length=0; pass_a=false candidates=[] values=[{},{}] note=no candidate; pass_b=true PostCompact.at=2026-09-03T15:49:15.095Z <= SessionStart(compact).at=2026-09-03T15:49:15.120Z; no UserPromptSubmit after last PostCompact
- **codex-rollout-flush**: id=exec-acf961f9-01db-4140-b24c-cf275bfe4cec tool=Bash in_transcript=true bytes=49405; id=exec-e05588d3-df39-4257-8aea-bbdf3e7fade9 tool=apply_patch in_transcript=true bytes=52682; id=exec-505735b7-0fef-405f-a69f-90efa1079551 tool=apply_patch in_transcript=true bytes=57544; id=exec-39e25c9c-2860-4207-b1a7-758d6a2cd8fb tool=Bash in_transcript=true bytes=61081; calls=4 misses=0 exit=0
- **codex-tui-trust**: trust_rows=8; events=SessionStart,UserPromptSubmit,PreToolUse,PostToolUse; hooks_fired=true; trust_prompt=false; pane_tail=n: echo tui-ok ;. To resume this session run codex resume, then select Use the shell to run: echo tui-ok ; (01a067f6-f996-7ba2-b846-3133b80c7f00) • I’ll run the requested shell command now. • Ran echo tui-ok └ tui-ok Working (6s • esc to interrupt) › Ask Codex to do anything gpt-5.6-sol default · <run>/codex-tui-trust/repo
- **codex-mcp-legacy-client**: protocolVersion=2025-06-18; methods_in=[initialize,notifications/initialized,tools/list,tools/call]; tools/list=true; tools/call=true; PreToolUse_tool_name=[mcp__oboete_probe__search]; echoed_dummy=true; frames=7 exit=0 elapsed_s=16.2
- **grok-payload-shapes**: read_file input=[target_file] output=[type,FileContent] path=toolInput.target_file (relative); absolute at toolResult.FileContent.absolute_path (match recon; out match recon); write input=[file_path,content] output=[type,EditsApplied] path=toolInput.file_path (relative); absolute at toolResult.EditsApplied.absolute_path; toolResult.type is SearchReplace (match recon; out match recon); search_replace input=[file_path,old_string,new_string] output=[type,EditsApplied] path=toolInput.file_path (relative); absolute at toolResult.EditsApplied.absolute_path (match recon; out match recon); run_terminal_command input=[command,description] output=[type,output,output_for_prompt,exit_code,command,truncated,signal,timed_out,description,current_dir,output_file,total_bytes,was_bare_echo] path=toolInput.command; output is a byte array, output_for_prompt is the string (match recon; out match recon); exit=0 elapsed_s=153.8 session=01a067f7-94ec-7d90-9e94-7fac5f306978 model=grok-4.6-build
- **grok-oversized-stdin**: exit=0; DONE=true; Stop=true; SessionEnd=true; unread_handlers=1; read_PostToolUse=1 sizes=164535; hook_lines=none; elapsed_s=90.7
- **grok-parallel-batch**: pre_n=2 same_at_second=true secCount=1 spread_ms=31 ats=2026-09-03T15:55:56.748Z,2026-09-03T15:55:56.779Z; pre_calls=run_terminal_command:call-c46fa3ba-5aaa-4777-b86d-f894a543e946-0@2026-09-03T15:55:56.748Z \| run_terminal_command:call-c46fa3ba-5aaa-4777-b86d-f894a543e946-1@2026-09-03T15:55:56.779Z; hook_context_deliveries=2 marker_in_answer=1 marker_in_transcript=4 files=01a067fb-5035-7ef0-8107-a17f0f9a5e24/chat_history.jsonl,01a067fb-5035-7ef0-8107-a17f0f9a5e24/updates.jsonl; DONE=true answer="DONE PROBE-PB\n"; exit=0 elapsed_s=93.9 session=01a067fb-5035-7ef0-8107-a17f0f9a5e24; once per call (A15 default)
- **grok-pretooluse-failed-call**: PostToolUseFailure_n=0 keys=none error=null; PostToolUse_n=1 keys=hookEventName,sessionId,cwd,workspaceRoot,timestamp,transcriptPath,permissionMode,toolName,toolUseId,toolInput,toolResult,toolInputTruncated,toolResultTruncated,durationMs,isBackgrounded,hook_event_name,session_id,transcript_path,permission_mode,tool_name,tool_input,tool_response,tool_use_id,duration_ms exit_code=3; PROBE-FAIL_reached_model=true delivery=delivered; DONE=true answer="DONE PROBE-FAIL"; exit=0 elapsed_s=38.9
- **grok-permission-denied**: noApprove+hook-deny: PermissionDenied_n=0 keys=[] toolUseId=null reason=null second_keys=[] PreToolUse_n=1 answer="DONE probe" exit=0; always-approve+hook-deny: PermissionDenied_n=0 keys=[] toolUseId=null reason=null second_keys=[] PreToolUse_n=1 answer="DONE probe" exit=0; always-approve+permission-deny-rule: PermissionDenied_n=1 keys=[hookEventName,sessionId,cwd,workspaceRoot,timestamp,transcriptPath,permissionMode,toolName,toolUseId,toolInput,toolInputTruncated,hook_event_name,session_id,transcript_path,permission_mode,tool_name,tool_input,tool_use_id] toolUseId=call-9d5d3845-1f03-4fdf-b43f-d8229046335d-0 reason=null second_keys=[] PreToolUse_n=1 answer="DONE" exit=0; payload_captured=true
- **grok-postcompact**: (a) identity: headless_PostCompact_n=2 tui_PostCompact_n=1 ok=true candidates=[timestamp] note= values=[{"timestamp":"2026-09-03T16:01:11.622755654+00:00"},{"timestamp":"2026-09-03T16:01:37.675324517+00:00"},{"timestamp":"2026-09-03T16:03:47.526376366+00:00"}]; (b) order: all injections after matching PreCompact have at >= PostCompact.at (last@2026-09-03T16:01:37.702Z) \| all injections after matching PreCompact have at >= PostCompact.at (last@2026-09-03T16:03:47.562Z) b_ok=true; payload PreCompact_n=2 keys=hookEventName,sessionId,cwd,workspaceRoot,timestamp,transcriptPath,permissionMode,source,hook_event_name,session_id,transcript_path,permission_mode matcher="auto"; payload PostCompact keys=hookEventName,sessionId,cwd,workspaceRoot,timestamp,transcriptPath,permissionMode,source,hook_event_name,session_id,transcript_path,permission_mode summary=null matcher="auto"; tui ok=true error=none pane="  master <run>/grok-postcompact/tui/repo 1.5K / 6.0K ❯ say hi then wait 1:01 AM █ █ ◆ user_prompt_submit [hooks: 1] █ █ Context 25% full. Compacting… █ █ ◆ pre_compact [hooks: 1] █ ◆ post_compact ["; big.txt_bytes=204800 exit=0 elapsed_s=68.5 model=grok-4.5-build
- **grok-resume**: A-new: source="new" sessionId=01a06803-26f6-70d2-a364-8c6f78544a20 envelope=01a06803-26f6-70d2-a364-8c6f78544a20 transcriptPath=absent keys=[hookEventName,sessionId,cwd,workspaceRoot,timestamp,permissionMode,source,hook_event_name,session_id,permission_mode] exit=0; B-resume: source="load" sessionId=01a06803-26f6-70d2-a364-8c6f78544a20 envelope=01a06803-26f6-70d2-a364-8c6f78544a20 transcriptPath=present keys=[hookEventName,sessionId,cwd,workspaceRoot,timestamp,transcriptPath,permissionMode,source,hook_event_name,session_id,transcript_path,permission_mode] exit=0; C-fork: source="load" sessionId=01a06803-e0d6-7043-a9af-3fa28e2794b4 envelope=01a06803-e0d6-7043-a9af-3fa28e2794b4 transcriptPath=present keys=[hookEventName,sessionId,cwd,workspaceRoot,timestamp,transcriptPath,permissionMode,source,hook_event_name,session_id,transcript_path,permission_mode] exit=0; id_A_eq_B=true C_new_id=true B_source_present=true A=01a06803-26f6-70d2-a364-8c6f78544a20 B=01a06803-26f6-70d2-a364-8c6f78544a20 C=01a06803-e0d6-7043-a9af-3fa28e2794b4
- **grok-mcp-registration**: toml frames=in:initialize,in:notifications/initialized,in:tools/list,in:tools/call; cli frames=in:initialize,in:notifications/initialized,in:tools/list,in:tools/call; PreToolUse toolName=[search_tool,oboete_probe__search]; echoed_dummy=true text="DONE dummy result for hello\nDONE dummy result for hello"; mcp add exit=0 changed=true wrote="[mcp_servers.oboete_probe]\ncommand = \"/usr/bin/node\"\nargs = [\"<run>/grok-mcp-registration/mcp-dummy.mjs\"]\nenabled = true\n\n[mcp_servers.oboete_probe.env]\nPROBE_MCP_LOG = \"<run>/grok-mcp-registration/mcp-cli.jsonl\"\n"; mcp add stdout="Added stdio MCP server 'oboete_probe' with command: /usr/bin/node <run>/grok-mcp-registration/mcp-dummy.mjs to user config\nFile modified: $" stderr=""
- **grok-stop-messages**: Stop_n=2 reasons=end_turn,shutdown; end_turn lastAssistantMessage="DONE" keys=[hookEventName,sessionId,cwd,workspaceRoot,timestamp,transcriptPath,promptId,permissionMode,reason,stopHookActive,lastAssistantMessage,backgroundTasks,sessionCrons,hook_event_name,session_id,transcript_path,permission_mode]; shutdown lastAssistantMessage=null keys=[hookEventName,sessionId,cwd,workspaceRoot,timestamp,transcriptPath,permissionMode,reason,stopHookActive,hook_event_name,session_id,transcript_path,permission_mode]; envelope.text="DONE" match=true; SessionEnd_n=1 SessionEnd.at=2026-09-03T16:09:54.619Z shutdown.at=2026-09-03T16:09:54.647Z SessionEnd_before_shutdown=true; exit=0 elapsed_s=46.8
- **pi-payload-shapes**: read input=[path] output=[(array)] path=input.path (match recon; out missing content; extra (array)); write input=[path,content] output=[(array)] path=input.path (match recon; out missing content; extra (array)); edit input=[path,edits] output=[(array),details] path=input.path (edits is [{oldText,newText}]) (match recon; out missing content; extra (array)); bash input=[command] output=[(array)] path=input.command (match recon; out missing content; extra (array)); exit=0 elapsed_s=21.1 session=01a06808-bdb2-75b8-8b09-d67eb0728df5 model=gpt-5.6-luna
- **pi-oversized-stdin**: Pi has no hook process; the equivalent is oboete's own capture child, probed after the child exists
- **pi-compaction**: run1 exit=0 elapsed_s=143.2 session=01a06809-1106-749f-a839-56da79d70fdd; run2 exit=0 elapsed_s=152.5 session=01a06809-1106-749f-a839-56da79d70fdd; session_before_compact=2 keys=[type,preparation,branchEntries,reason,willRetry,signal] reason=threshold firstKept=d0c8dc1d \| keys=[type,preparation,branchEntries,reason,willRetry,signal] reason=threshold firstKept=a851c0bd; session_compact=2 keys=[type,compactionEntry,fromExtension,reason,willRetry] id=0be9b461 summaryLen=485 firstKept=d0c8dc1d reason=threshold at=2026-09-03T16:12:39.902Z ceKeys=[type,id,parentId,timestamp,summary,firstKeptEntryId,tokensBefore,details,usage,fromHook] \| keys=[type,compactionEntry,fromExtension,reason,willRetry] id=5f69a44d summaryLen=1144 firstKept=a851c0bd reason=threshold at=2026-09-03T16:15:09.239Z ceKeys=[type,id,parentId,timestamp,summary,firstKeptEntryId,tokensBefore,details,usage,fromHook]; session_compact_failed=2 reason=manual \| reason=manual; compactionEntry.ids=0be9b461,5f69a44d unique=2 (a) distinguishing=true candidates=[id,parentId,timestamp,firstKeptEntryId] note=; first compact at=2026-09-03T16:12:39.902Z next inject before_agent_start at=2026-09-03T16:12:43.221Z (b) committed_before_next_inject=true; probe_compact_called=2 complete=0 error=[{"message":"Already compacted"},{"message":"Already compacted"}]; getContextUsage={"tokens":11624,"contextWindow":272000,"percent":4.273529411764706}; DONE r1=false r2=false
- **pi-tools**: exit=0 elapsed_s=8.7; tool_call n=1 toolName=oboete_probe input={} keys=[type,toolName,toolCallId,input]; tool_result n=1 toolName=oboete_probe content=[{"type":"text","text":"oboete_probe ok"}] keys=[type,toolName,toolCallId,input,content,isError]; before_agent_start.systemPromptOptions.selectedTools=["read","bash","edit","write","oboete_probe"] includes_oboete_probe=true; model_text="DONE oboete_probe ok" echoed=true
- **pi-resume-fork**: session_start.reason A=startup B=startup C=startup; sessionId A=01a0680d-b7da-720e-920d-d5ce40a36762 B=01a0680d-b7da-720e-920d-d5ce40a36762 C=01a0680d-e9b6-722f-825f-5a45177a1f26 A==B=true C!=A=true; sessionFile A=<run>/pi-resume-fork/A/piagent/sessions/2026-09-03T16-15-24-634Z_01a0680d-b7da-720e-920d-d5ce40a36762.jsonl B=<run>/pi-resume-fork/A/piagent/sessions/2026-09-03T16-15-24-634Z_01a0680d-b7da-720e-920d-d5ce40a36762.jsonl C=<run>/pi-resume-fork/C/piagent/sessions/2026-09-03T16-15-37-398Z_01a0680d-e9b6-722f-825f-5a45177a1f26.jsonl; bash PI_SESSION_ID B=01a0680d-b7da-720e-920d-d5ce40a36762 C=01a0680d-e9b6-722f-825f-5a45177a1f26 equals_extension B=true C=true; text B="DONE 01a0680d-b7da-720e-920d-d5ce40a36762" C="DONE 01a0680d-e9b6-722f-825f-5a45177a1f26"; exit A=0 B=0 C=0
- **pi-error-surface**: exit=0 elapsed_s=7.1 continued=true text="DONE"; stderr hits=Extension error (<run>/pi-error-surface/piagent/extensions/oboete-probe.ts): probe throw at before_agent_start; stdout error types=none stdout hits=none; session jsonl types=[session,model_change,thinking_level_change,message,message] throw records=none; piagent logs with throw=none; ~/.pi/agent added=none changed=none throw logs=none; durable=no durable record (stderr/in-memory only unless a path is named); tmp tree files=auth.json,extensions,extensions/oboete-probe.ts,models-store.json,sessions,sessions/2026-09-03T16-15-52-008Z_01a0680e-22c8-7bf7-9ee2-187bd1f3ecba.jsonl,settings.json
- **pi-after-provider-response**: exit=0 elapsed_s=6.3 model=gpt-5.6-luna; after_provider_response count=0 keys=n/a; before_provider_request count=1; event names=session_start,resources_discover,input,before_agent_start,agent_start,turn_start,message_start,message_end,context,before_provider_headers,before_provider_request,turn_end,agent_end,agent_settled,session_shutdown,message_update_count
- **agent-cli-json**: claude pass 4.98s text=result model=claude-opus-5[1m] ver=2.1.259 (Claude Code); codex pass 9.66s text=output-last-message model=none ver=codex-cli 0.153.0; grok pass 54.02s text=text model=grok-4.6-build ver=grok 1.0.17 (a549186d9d39) [alpha]; pi pass 6.28s text=turn_end text blocks model=gpt-5.6-luna ver=0.84.4
- **provider-nim**: dummy-key self-check: HTTP 410 111ms; credential absent
- **provider-openrouter**: credential absent
- **provider-gemini**: credential absent
- **provider-anthropic**: credential absent
- **provider-workers-ai**: credential absent


## Findings 2026-09-03, agent-specific rows (job B) and harness review rounds

Evidence: the run section above (2026-09-03T15-43-30-145Z: all 36 probes in one run after two review
rounds; an earlier run of the same code at 14-17-45Z was discarded because the Anthropic API returned
529 Overloaded for every Claude Code call, which the runner now reports as `blocked`); fixtures under
`test/contracts/<agent>/`. Evaluation against the R13 pass conditions: section "R13 evaluation" below.

- **Compaction identity and order** (one evaluator, `compactionIdentity` in `probe-lib/agents.mjs`, applied
  to the recorder shape for all four agents; candidate keys are native id/counter/timestamp fields of the
  payload after removing envelope and turn-scoped keys, never the recorder time). Claude Code `PostCompact`
  keys: `session_id`, `transcript_path`, `cwd`, `prompt_id`, `hook_event_name`, `trigger`,
  `compact_summary` (no candidate; `compact_summary` about 10-11k characters); `SessionStart source=compact`
  is recorded 24-26 ms before `PostCompact` in every headless run (order: PreCompact → SessionStart(compact)
  → PostCompact). Codex `PostCompact` keys: `session_id`, `turn_id`, `transcript_path`, `cwd`,
  `hook_event_name`, `model`, `trigger` (no summary field, no candidate; `turn_id` is turn-scoped); order is
  fine (PostCompact 26 ms before SessionStart(compact)); two TUI `/compact` yield two PostCompact with
  `trigger=manual`. Grok Build `PostCompact` keys: `hookEventName`, `sessionId`, `cwd`, `workspaceRoot`,
  `timestamp`, `transcriptPath`, `permissionMode`, `source` (no summary field; the nanosecond `timestamp`
  distinguishes every compaction, 6 of 6 across headless and TUI; no injection hook runs between PreCompact
  and PostCompact). Pi `session_compact.compactionEntry.id` differs per compaction and the next
  `before_agent_start` follows it.
- **Codex `SessionStart` sources**: `startup`, `resume` (same `session_id`, `codex exec resume <id>`) and
  `compact` (headless, `-c model_auto_compact_token_limit=2000 -c model_auto_compact_token_limit_scope="body_after_prefix"`)
  are verified. `/new` in the TUI ends the session (`SessionEnd`) and fires no `SessionStart` at all, with the
  matcher widened to `startup|resume|clear|compact|new|fork`. TUI driving needs `tui.disable_paste_burst=true`,
  `send-keys -l` and a delayed Enter. The TUI also imported an MCP server from outside `CODEX_HOME`
  (`cloudflare-api` warning with `mcp_servers={}`), so Codex reads MCP registrations from somewhere other
  than its config: to be confirmed before the installer relies on `[mcp_servers.oboete]` alone.
- **Codex rollout flush and TUI trust**: the just-finished `tool_use_id` is already in `transcript_path` when
  `PostToolUse` runs (4 of 4 calls); with `trusted_hash` rows in `config.toml` the TUI starts with hooks active
  and no trust prompt (`--dangerously-bypass-hook-trust` not needed).
- **Grok Build**: `additionalContext` attached to the two calls of one parallel batch appears once per call in
  the transcript (`hook_context_deliveries=2`, calls 32 ms apart) and the model echoed the marker once;
  a shell call that exits 3 arrives as `PostToolUse` with `exit_code` (no `PostToolUseFailure`) and the
  attached context reached the model; `PermissionDenied` fires only for a permission-rule deny
  (`--deny 'Bash(*)'` or `[permission] deny`), never for a hook `deny`, and carries no reason field;
  `--resume <id>` gives `SessionStart.source = "load"` with the same `sessionId` and a `transcriptPath`,
  `--fork-session` a new id with `source = "load"`; MCP registration works both through
  `[mcp_servers.<name>]` in `config.toml` and `grok mcp add --scope user` (which writes `config.toml` under
  `GROK_HOME`), the hook sees `toolName = oboete_probe__search`; the `end_turn` Stop carries the final
  answer in `lastAssistantMessage`, the shutdown Stop carries none, and `SessionEnd` precedes it.
- **Pi**: `pi.registerTool` tools are listed in `before_agent_start.systemPromptOptions.selectedTools`, called,
  and their result reaches the model; `--session <file>` keeps `getSessionId()` and `--fork <file>` changes it,
  the bash child's `PI_SESSION_ID` equals the extension's id, and `session_start.reason` is `startup` in all
  three cases (resume detection by id continuity); an extension throw is printed to stderr
  (`Extension error (...): probe throw at before_agent_start`), the session JSONL and `~/.pi/agent` hold no
  record, and the turn continues; `after_provider_response` never fires with the `openai-codex` provider.
- **Claude Code**: `--resume` gives `source = resume` with the same `session_id`, `--fork-session` gives
  `source = fork` with a new id, and plain-stdout SessionStart text reaches the model in all three cases;
  a failing Bash or Read produces `PostToolUseFailure` only (no `PostToolUse`) with `error` as a string;
  `Stop.last_assistant_message` equals the final answer. The interactive TUI could not be driven under the
  isolated user (first launch shows the theme picker, then the login-method screen, which is separate from
  the headless credentials), so the two-compaction TUI evidence for Claude Code is manual; the identity
  verdict rests on the payload keys of the headless compaction.
- **Harness**: probes share one launcher per agent (`probe-lib/agents.mjs`; the Grok launcher takes
  `matcher`, `homeFrom`, `timeoutMs` and returns the JSON envelope), tmux runs on a dedicated socket
  (`-L oboete-probes`, env passed with `-e`) and a probe timeout kills only that server, fixtures redact the
  run root in all three encodings as `<run>` (`probe-lib/redact.test.mjs`), and every recorded payload is
  redacted with its own run's repo path. Two review rounds (Claude Code `/code-review`, ponytail) were applied
  by Grok Build; the evaluator fixes did not change any R13 verdict.

## R13 evaluation (T011, 2026-09-03)

Probe runs: the run sections above (harness `scripts/e2e/probe-contracts.mjs`, isolated
user, versions Claude Code 2.1.259 / Codex 0.153.0 / Grok Build 1.0.17 / Pi 0.84.4). Status words follow
research.md R13: pass = pass condition holds; fail = probed, condition does not hold, the R13 fallback
applies; blocked = not executable yet; skipped = not applicable.

| R13 row | agent | status | observed | consequence |
|---|---|---|---|---|
| Native tool payload shapes | all four | pass | 16 fixtures + failure fixtures (`test/contracts/`) | adapters (T028) map from fixtures; Codex reads arrive as Bash, writes/edits as `apply_patch` |
| PostCompact payload / Grok Stop `lastAssistantMessage` | Codex, Grok | fail (text absent), pass (Stop) | Codex and Grok `PostCompact` carry no summary text (keys: session_id, turn_id, transcript_path, cwd, hook_event_name, model, trigger / Grok: hookEventName, sessionId, cwd, …, timestamp, matcher auto); Grok `Stop(end_turn).lastAssistantMessage` = final text; Claude Code `PostCompact.compact_summary` present (~10k chars) | `compaction_summary` absent by contract for Codex and Grok (FR-010 input = captured events only); present for Claude Code |
| Grok parallel batches | Grok | fail | `additionalContext` attached to two calls of one batch appears twice in `chat_history.jsonl` (once per call); the model echoed the marker once | A15 default applies: per-call duplicates inside one parallel batch are accepted and counted in `why` and SC-010 |
| Compaction identity and order | Claude Code | fail (a), fail (b) | `PostCompact` has no id / counter / timestamp beyond `compact_summary`; `SessionStart source=compact` fires ~24 ms BEFORE `PostCompact` (order: PreCompact → SessionStart(compact) → PostCompact) | A16 default: epoch key = `PostCompact` event id (byte-identical same-turn compactions collapse); documented ordering limit: the post-compaction SessionStart runs before the epoch advance, so the SessionStart(compact) hook must itself open the new epoch (it carries `source=compact`) and PostCompact only confirms it |
| Compaction identity and order | Codex | fail (a), pass (b) | `PostCompact` keys carry `turn_id` + `trigger` only (no per-compaction id); PostCompact precedes SessionStart(compact) by ~24 ms | A16 default for identity; ordering is fine |
| Compaction identity and order | Grok Build | pass (a), pass (b) | `PostCompact.timestamp` (nanosecond) distinguishes compactions; no injection hook runs between PostCompact and the next turn | none |
| Compaction identity and order | Pi | pass (a), pass (b) | `session_compact.compactionEntry.id` differs per compaction (`480afbf2` / `4283239e`); the next `before_agent_start` follows `session_compact` | none |
| Detector 1 MB on Node 22.16 | — | blocked (needs oboete code) | runners cap hook payloads at ~31 KB (Claude Code), ~4.8 KB (Codex), ~165–190 KB (Grok) for tool results | probe after T025; A14 is unlikely to trigger through tool results |
| Codex `SessionStart` source = compact and clear | Codex | pass (compact), fail (clear) | headless auto-compaction (`-c model_auto_compact_token_limit`) yields `source=compact`; `resume` yields `source=resume` with the same session_id; `clear` needs the TUI `/new`, which ended the session (`SessionEnd`) without any `SessionStart`; matcher widened to `new|fork` still saw nothing. Status: fail (the TUI ran, so not blocked) | FR-024 on Codex holds for compact; `clear` is handled by A18: a cleared session is detected by the session id changing on the next `UserPromptSubmit`, where the session-start pack is injected |
| Codex rollout flush at PostToolUse; TUI trust path | Codex | pass (flush), pass (TUI trust) | the just-completed tool_use_id was already in `transcript_path` when PostToolUse ran (`transcript_has_tool_use_id=true` for every call); trusted_hash rows fire hooks headless without the bypass flag (`codex-trust-hash`); TUI: with `trusted_hash` rows in `config.toml` and no bypass flag the TUI starts with hooks active (`SessionStart`, `UserPromptSubmit`, `SessionEnd` recorded) and shows no trust prompt | capture stays hook-stdin based; installer writes trust rows per `trusthash.mjs` |
| Grok user-scoped MCP registration | Grok | pass | `[mcp_servers.oboete_probe]` in config.toml and `grok mcp add --scope user` both work; frames initialize / tools/list / tools/call recorded; PreToolUse `toolName = oboete_probe__search` | FR-030 on Grok unblocked |
| Pi compaction event; Pi tool registration | Pi | pass, pass | `session_before_compact` / `session_compact` fire (reason `threshold`); `pi.registerTool` tool is offered (`selectedTools`), called and its result reaches the model | none |
| NIM / OpenRouter / Gemini / Anthropic transport | — | pass (nim, openrouter, gemini, workers-ai); anthropic removed as a preset by the owner on 2026-09-04 (A19) | run 2026-09-03T16-48-21-133Z: HTTP 200 with `Authorization: Bearer`, model ids `meta/llama-3.2-11b-vision-instruct`, `openai/gpt-4o-mini`, `gemini-2.5-flash`, `@cf/zai-org/glm-4.7-flash`; NIM retired `meta/llama-3.1-8b-instruct` on 2026-08-26 (HTTP 410), and most catalog entries return 404 "Function not found for account", so the shipped NIM default must be one of the deployed models | no preset row blocked |
| structured output (`response_format`) | — | pass (openrouter, workers-ai honoured), pass (nim, gemini: text-JSON) | `{type: json_object}` honoured by OpenRouter and Workers AI (`json_schema`); NIM and Gemini answered with JSON text without the flag being honoured | text-JSON path for NIM, Gemini (and Anthropic by design) |
| Grok PreToolUse context on a failed call | Grok | pass (delivered) | a non-zero `run_terminal_command` produces `PostToolUse` with `exit_code=3`, not `PostToolUseFailure`; the attached context reached the model | `execution = failed` is recorded from `exit_code`, delivery = delivered |
| Pi resume / fork | Pi | pass | `--session` keeps `getSessionId()`, `--fork` gives a new id; `PI_SESSION_ID` in the bash child equals the extension's id; `session_start.reason` is `startup` in all three cases | resume detection by id continuity, not by reason |
| Grok resume | Grok | pass | `--resume` → `SessionStart.source = "load"` (not the documented `resume`), same sessionId, `transcriptPath` present; `--fork-session` → new id, `source = load` | matcher must accept `load`; the agents.md wording "resume value per R13 probe" resolves to `load` |
| Hook runner with unread stdin > 1 MB | Claude Code, Codex, Grok | pass | sessions complete, later hooks fire, no hook error; runners never deliver > 1 MB (caps above) | A7 stands; Pi not applicable (no hook process) |
| Pi durable error surface | Pi | fail | an extension throw is printed to stderr only (`Extension error (…oboete-probe.ts): probe throw at before_agent_start`); the session JSONL has no error record; the session continues | A8 applies: in-memory counters handed to the next child + doctor probe |
| Legacy-era MCP server vs clients | Grok pass; Codex pass (protocolVersion `2025-06-18`; `initialize`, `tools/list`, `tools/call` frames; PreToolUse tool name `mcp__oboete_probe__search`; the model echoed the dummy result); Claude Code blocked (needs T077 server + `--mcp-config` probe) | dummy legacy server frames | |
| `agent-cli` preset | all four | pass | JSON output parses for claude -p / codex exec / grok -p / pi -p | preset enabled for all four |
| Per-model context windows | all four | pass | docs/research/context-windows.md | |
| Real bundle cold start on 22.16 and 24.x | — | pass | v22.23.1 local proxy and v24.16.0: all `--version` and hook-path maxima are inside the 100 ms / 300 ms budgets ([evidence](../evidence/m1-resource-envelope.md)) | none; the split-entry-point consequence is not triggered |
| Installed size with dependencies | — | pass | 29,852 `du -k` blocks = 29.152 MB after the bundled packages (preact, secretlint, smol-toml) became devDependencies; the first measurement at a9b51fb was 32.695 MB ([evidence](../evidence/m1-resource-envelope.md)) | none; margin 0.85 MB, guarded by the T087 pack-check |

Additional contract facts recorded for the adapters: Grok `PermissionDenied` fires only for a permission-rule deny (`--deny 'Bash(*)'`), never for a hook `deny` (keys: hookEventName, sessionId, cwd, workspaceRoot, timestamp, transcriptPath, permissionMode, toolName, toolUseId, toolInput, toolInputTruncated; no reason field); Claude Code resume/fork `SessionStart` plain stdout still reaches the model (oboete prints nothing there by policy); Claude Code failed tools produce `PostToolUseFailure` only (no `PostToolUse`), `error` is a string; Pi `after_provider_response` never fires with the `openai-codex` provider.

Conditional amendments triggered: A8 (Pi), A15 (Grok), A16 (Claude Code and Codex; Grok and Pi unaffected); new A18 (Codex `/new`). A14 not evaluated yet (needs the detector). Decisions: `docs/research/m1-amendments-2026-09.md`, section "R13 outcome".
## 2026-09-03 run 2026-09-03T16-48-21-133Z

| tool | version |
|---|---|
| date | 2026-09-03T16:48:21.134Z |
| claude | 2.1.259 (Claude Code) |
| codex | codex-cli 0.153.0 |
| grok | grok 1.0.17 (a549186d9d39) [alpha] |
| pi | 0.84.4 |
| node | v24.20.0 |

| id | R13 row | agent | status |
|---|---|---|---|
| provider-nim | NIM / OpenRouter / Gemini / Anthropic transport, auth header, model id | providers | pass |
| provider-openrouter | NIM / OpenRouter / Gemini / Anthropic transport, auth header, model id | providers | pass |
| provider-gemini | NIM / OpenRouter / Gemini / Anthropic transport, auth header, model id | providers | pass |
| provider-workers-ai | NIM / OpenRouter / Gemini / Anthropic transport, auth header, model id | providers | pass |

- **provider-nim**: dummy-key self-check: HTTP 403 103ms; HTTP 200; auth=Authorization; model=meta/llama-3.2-11b-vision-instruct; response_format=text-JSON; elapsed_ms=1368
- **provider-openrouter**: HTTP 200; auth=Authorization; model=openai/gpt-4o-mini; response_format=honoured; elapsed_ms=2199
- **provider-gemini**: HTTP 200; auth=Authorization; model=gemini-2.5-flash; response_format=text-JSON; elapsed_ms=950
- **provider-workers-ai**: HTTP 200; auth=Authorization; model=@cf/zai-org/glm-4.7-flash; response_format=honoured; elapsed_ms=1465

