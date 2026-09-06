# Claude Code

## What setup writes

`oboete setup --agents claude` merges oboete-owned handler groups (`"oboete": true`) into
`~/.claude/settings.json` (or `$CLAUDE_CONFIG_DIR/settings.json`). Nothing else in that file is
rewritten. Each handler is:

```text
<node> <bundle> hook --agent claude-or-grok --event <Event>
```

The shared selector `claude-or-grok` is resolved at run time: `GROK_HOOK_EVENT` or `GROK_SESSION_ID`
present means Grok Build, otherwise Claude Code.

| Event | Timeout written (seconds) | Role |
| --- | --- | --- |
| `SessionStart` | 12 | Capture and injection |
| `UserPromptSubmit` | 12 | Capture and injection |
| `PreToolUse` | 3 | Capture |
| `PostToolUse` | 3 | Capture |
| `PostToolUseFailure` | 3 | Capture (`tool_failure`) |
| `Stop` | 3 | Capture (`last_assistant_message` and `turn_end`) |
| `PostCompact` | 3 | Capture (`compaction_summary` from `compact_summary`) |
| `SessionEnd` | 3 | Capture only (Claude Code shares 1.5 s among all SessionEnd handlers) |

Setup then runs `claude mcp remove oboete --scope user` (harmless when nothing is registered) and
`claude mcp add oboete --scope user -- <node> <bundle> mcp`. User scope is required: without
`--scope user` the tools exist only in the directory setup happened to run in. Removal names the
same scope.

## Detection

Installed when the `claude` executable is on `PATH` or `~/.claude` (or `CLAUDE_CONFIG_DIR`) exists.
Trust is `n/a` (Claude Code has no hash gate). Doctor's `agent:claude` item is `degraded` when no
oboete-owned handler is present in `settings.json`.

## Registration step

After setup, `claude mcp get oboete` from any directory should report user scope. Capture and
injection still work if that registration failed; setup prints that the memory tools could not be
registered, that capture and injection are wired, and the exact command to run to add the tools.

## When packs arrive

Plain stdout on `SessionStart` when `source` is `startup`, `clear`, or `compact`. Nothing on
`resume` or `fork` (the transcript already carries the earlier pack). Plain stdout on
`UserPromptSubmit`. The pack is framed by `oboete memory context` and `end of oboete memory
context` and never starts with `{` (Claude Code would parse that as JSON and drop it).

## Native-memory coexistence

If any project under `~/.claude/projects/` has a `memory` directory, setup and doctor warn:
`claude: its own memory feature (claude_auto_memory) is enabled. oboete neither reads it nor
changes it; the two run side by side.` Doctor does not change that setting (FR-032, FR-043).

## Memory tools

The stdio server (`oboete mcp`) exposes `search`, `timeline`, and `get`. Claude Code presents them
as `mcp__oboete__search`, `mcp__oboete__timeline`, and `mcp__oboete__get`. A repository identifier
in the tool arguments is refused (JSON-RPC `-32602`). Search is lexical in M1.

## Known limitations

- **A16 (compaction epoch).** `PostCompact` has no per-compaction id beyond `compact_summary`.
  `SessionStart source = compact` fires about 24 ms before `PostCompact` (order: PreCompact →
  SessionStart(compact) → PostCompact). The SessionStart(compact) hook therefore opens the new
  context epoch itself; `PostCompact` only confirms it. Byte-identical same-turn compactions
  collapse to one epoch key (the `PostCompact` event id).
- **Failed tools.** A failing Bash or Read produces `PostToolUseFailure` only (no `PostToolUse`);
  `error` is a string. Joining Pre/Post for Bash uses `tool_use_id` because the response does not
  echo the command.
- **Hook stdin cap.** The Claude Code runner delivered about 31 KB of a 1.2 MB tool result to a
  reading handler (probe 2026-09-03). oboete itself reads at most 256 KiB (A14) and stores a
  redacted prefix marked truncated.
- **TUI under the isolated user.** The interactive terminal could not be driven for two-compaction
  evidence (first launch shows the theme picker, then the login-method screen). Headless
  compaction identity is what A16 rests on.
- **Resume and fork.** `--resume` keeps `session_id` with `source = resume`. `--fork-session`
  starts a new root with `source = fork`. Plain-stdout SessionStart text still reaches the model
  in all three cases; oboete prints nothing on resume/fork by policy.

## How to remove

```bash
oboete setup --agents claude --remove
```

That strips the oboete handlers from `settings.json` (leaving the rest of the file) and runs
`claude mcp remove oboete --scope user`. The consent record in `~/.oboete/` is kept. If the
remove of the tools fails, setup prints `claude: the memory tools could not be unregistered
because <reason>.`
