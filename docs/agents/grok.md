# Grok Build

## What setup writes

`oboete setup --agents grok` writes two files under `~/.grok/` (or `GROK_HOME`):

1. `hooks/oboete.json` — oboete-owned handlers (`"oboete": true`) with an explicit `timeout` per
   event. Grok Build kills a hook at its own timeout (5 s by default, 1.5 s on `SessionEnd`), so
   every handler states one.
2. `config.toml` in that same directory — a managed block `[mcp_servers.oboete]` with `command`,
   `args = [<bundle>, "mcp"]`, and `enabled = true` (the same table `grok mcp add --scope user`
   writes).

Each handler is:

```text
<node> <bundle> hook --agent claude-or-grok --event <Event>
```

`GROK_HOOK_EVENT` or `GROK_SESSION_ID` selects the Grok adapter. Grok Build also reads
`~/.claude/settings.json` as a compatibility layer unless `GROK_CLAUDE_HOOKS_ENABLED=0`. If that
file already holds oboete handlers, setup does not edit it (FR-043) and prints: `grok: it also
reads the Claude Code settings, so <events> would fire twice per turn. Set
GROK_CLAUDE_HOOKS_ENABLED=0 in the shell that starts Grok Build.`

| Event | Timeout (seconds) | Role |
| --- | --- | --- |
| `SessionStart` | 12 | Capture; pack is stored `pending` (FR-045) |
| `UserPromptSubmit` | 12 | Capture; pack merged into the pending record |
| `PreToolUse` | 12 | Delivery: emit the pending pack as `additionalContext` |
| `PostToolUse` | 12 | Confirm delivery (`execution = ran`, `delivery = delivered`); a failed shell call arrives here with `exit_code`, not as `PostToolUseFailure` |
| `PostToolUseFailure` | 3 | Capture |
| `PermissionDenied` | 3 | Capture; fires only for a permission-rule deny, never for a hook deny; no reason field |
| `Stop` | 3 | `reason: end_turn` maps `lastAssistantMessage`; a still-pending pack becomes `omitted` |
| `PostCompact` | 3 | Capture; no summary field; `timestamp` is the per-compaction key |
| `SessionEnd` | 3 | Capture only |

If the TOML write fails, setup rolls the hooks file back.

## Detection

Installed when the `grok` executable is on `PATH` or `~/.grok` exists. Trust is `wired` when
`hooks/oboete.json` exists, otherwise `absent`. Doctor's `agent:grok` item is `degraded` when the
file is absent.

## Registration step

The managed `[mcp_servers.oboete]` table is the registration. The 2026-09-03 probe also confirmed
`grok mcp add --scope user` writes the same table under `GROK_HOME`. Hooks then see the tools as
`oboete__<tool>`.

## When packs arrive (deferred)

Grok Build has no channel that reaches the model before a turn starts (SessionStart and
UserPromptSubmit output never reach the model; PreToolUse output arrives with the tool result).
M1 therefore defers delivery (FR-045, User Story 1):

1. At `SessionStart` or `UserPromptSubmit` the pack is built and stored `pending`. A second pack
   in the same conversation merges into that record.
2. On every `PreToolUse` of the turn while the record is not yet `emitted`, the hook attaches the
   pack as `additionalContext`.
3. The first `PostToolUse` (or `PostToolUseFailure` when the attached context survives) that
   actually ran marks the record `emitted` and its items `included`.
4. A denied call produces no `PostToolUse`; the next `PreToolUse` attaches the pack again.
   `PermissionDenied` marks that attempt `execution = denied`, `delivery = dropped`.
5. At `Stop` (`end_turn`) a record still pending becomes `omitted` with reason `no_tool_call`
   when no tool hook ran, otherwise `not_delivered`.

`oboete why` reports `deferred: delivered with tool calls (<n> deliveries)` and each attempt's
`execution` and `delivery`. Doctor does not have a separate deferred item; the pack's
`> degraded:` line carries `no_tool_call` or `not_delivered` when that is what happened.

## Native-memory coexistence

If `~/.grok/memory` exists, setup and doctor warn: `grok: its own memory feature
(grok_native_memory) is enabled. oboete neither reads it nor changes it; the two run side by
side.`

## Memory tools

Hooks see `oboete__search`, `oboete__timeline`, and `oboete__get`. A repository identifier in the
tool arguments is refused (JSON-RPC `-32602`). Search is lexical in M1.

## Known limitations

- **Deferred delivery.** Packs arrive with the first tool call of the turn, not at session start.
  A turn that runs no tool receives nothing that turn.
- **A15 (parallel batch).** `additionalContext` attached to two calls of one parallel batch
  reaches the model once per call (twice in the transcript; the model echoed the marker once).
  Per-call duplicates inside one batch are accepted and counted in `why` (never excluded).
- **Failed shell calls.** A `run_terminal_command` that exits non-zero arrives as `PostToolUse`
  with `exit_code`, not `PostToolUseFailure`. The attached context still reached the model
  (probe 2026-09-03).
- **`PermissionDenied`.** Fires only for a permission-rule deny (`--deny 'Bash(*)'` or
  `[permission] deny`), never for a hook `deny`. Keys include `toolName`, `toolUseId`,
  `toolInput`; there is no reason field.
- **Resume and fork.** `--resume` yields `SessionStart.source = "load"` (not `resume`) with the
  same `sessionId` and a `transcriptPath`. `--fork-session` yields a new id with `source = "load"`.
  Headless new sessions use `source = "new"`.
- **PostCompact.** No summary text. The nanosecond `timestamp` distinguishes compactions.
  `window_unknown`: Grok Build reports no model, so the pack uses the smallest verified window
  and is labelled `window_unknown`.
- **Hook stdin cap.** The Grok runner delivered about 165–190 KB of a 1.2 MB tool result.
- **`config.toml` rewrites.** Grok re-serializes `~/.grok/config.toml` without comments when it
  updates itself or changes a setting, so the `# oboete:begin` / `# oboete:end` markers disappear
  and the bare `[mcp_servers.oboete]` table stays. Setup recognizes that table as its own (the
  marked handler in `hooks/oboete.json` runs the same bundle) and restores the block; doctor
  reports the dropped markers with recovery `oboete setup --agents grok` (observed 2026-09-06
  after the 1.0.21 update).
- **`agent-cli` JSON.** `grok -p --output-format json` puts text in `text`, which concatenates
  every assistant message; the Stop hook's `lastAssistantMessage` is the clean source.
- **Dogfood.** The 2026-09-04 isolated-user pair run failed all six Grok Build legs with HTTP 402
  "Grok Build usage balance exhausted" before a hook ran. That is an account-balance failure, not
  an oboete miss ([docs/evidence/m1-dogfood.md](../evidence/m1-dogfood.md)).

## How to remove

```bash
oboete setup --agents grok --remove
```

That strips the oboete handlers (and deletes `hooks/oboete.json` when it holds nothing else) and
removes the managed `[mcp_servers.oboete]` block from `config.toml` in that directory (also when Grok's own rewrite dropped the markers). The consent
record is kept. The Claude Code `settings.json` file is not edited on removal either.
