# Pi

## What setup writes

`oboete setup --agents pi` writes one file oboete owns outright:

```text
~/.pi/agent/extensions/oboete.js
```

Pi loads every `*.js` in that global extension directory; a project-local `.pi/extensions` entry
does not load until the project is trusted, and headless runs never ask, so setup uses the global
path (FR-031). The loader starts with `// oboete:managed` and imports `piExtension` from the packed
`pi-extension.mjs` next to the engine bundle:

```javascript
// oboete:managed written by `oboete setup`; `oboete setup --remove` deletes it.
import { piExtension } from "<file-url-of-pi-extension.mjs>";
export default (pi) => piExtension(pi, { node: "<node>", bundle: "<bundle>" });
```

A file that does not start with the marker is never overwritten. Setup does not edit Pi's own
settings (FR-043). If `PI_CODING_AGENT_DIR` points somewhere else, setup prints `pi: PI_CODING_AGENT_DIR
points at <dir>, and setup can only write the loader at <home>/.pi/agent/extensions/oboete.js.
Unset the variable and run setup again.` and reports `wired: failed`.

The extension itself only try/catches, generates invocation ids, spawns children, and counts
failures in memory (message code only). Capture is `oboete capture --agent pi --event <name>
--invocation <id> [--prior-failures <codes>]`; the child writes `spool/pi-ack/<invocation>.started`
before reading stdin and renames it to `.done` on completion. Injection is `oboete inject --agent
pi --kind start|prompt` with `AbortSignal.timeout` (1.3 s at session start while a summary is
pending, 300 ms otherwise). There is no in-process file or network write (FR-007, A10).

## Detection

Installed when the `pi` executable is on `PATH` or `~/.pi/agent` (or `PI_CODING_AGENT_DIR`) exists.
Trust is `wired` when the loader file exists, otherwise `absent`. Doctor's `agent:pi` item is
`degraded` when the file is absent. A separate `pi` item reports hung `.started` files (older than
the hang threshold) as `degraded` (`pi_child_hang`) and in-memory failure counters as `warning`.

Pi 0.84.4 requires Node.js >= 22.19; the isolated dogfood account therefore runs 24.x.

## Registration step

No extra registration: every `*.js` in `~/.pi/agent/extensions/` is loaded. After setup, a
headless `pi -p` run should list `oboete_search` in `before_agent_start.systemPromptOptions.selectedTools`
(the 2026-09-03 `pi-tools` probe used a dummy name; the shipped tools are below).

## When packs arrive

`before_agent_start` returns the pack produced by the bounded `oboete inject` child. Capture
subscriptions:

| Pi event | oboete kind |
| --- | --- |
| `session_start` | `session_start` (`reason` is always `startup` in resume, fork, and new; resume is detected by id continuity) |
| `input` | `prompt` (source-filtered) |
| `tool_result` | `tool_call` / `tool_result` (`input`, `content`, `isError`) |
| `agent_settled` | `turn_end` and `last_assistant_message` when available |
| `session_shutdown` | `session_end` (`reason` `quit\|reload\|new\|resume\|fork`) |
| `session_compact` | `compaction_summary`; `compactionEntry.id` is the per-compaction key. `session_before_compact` precedes it. |

## Native-memory coexistence

Pi has no native-memory flag oboete detects (`nativeMemory` is always null). There is no
`native-memory:pi` doctor item.

## Memory tools

The extension registers three tools that spawn the command line as a child (`--json`):

| Tool | Child |
| --- | --- |
| `oboete_search` | `oboete search <query> --json` (`limit` optional) |
| `oboete_timeline` | `oboete timeline --json` (`--session` optional) |
| `oboete_get` | `oboete get <id> --json` |

A repository identifier in the tool arguments is not accepted. Search is lexical in M1. The
2026-09-05 wiring re-check in a fresh repository returned
`{"memories":[],"reason":"No memories matched this query in the current repository.",…}` — the
query reached `oboete search`.

## Known limitations

- **A8 (error surface).** An extension throw is printed to stderr only (`Extension error
  (…): … at before_agent_start`). The session JSONL has no error record; the session continues.
  oboete therefore keeps in-memory counters and hands them to the next child as
  `--prior-failures`, and `oboete doctor` probes the wiring. There is no durable Pi-owned error
  file.
- **Resume and fork.** `--session <file>` keeps `getSessionId()`. `--fork <file>` changes it.
  The bash child's `PI_SESSION_ID` equals the extension's id. `session_start.reason` is `startup`
  in all three cases, so resume is detected by id continuity, not by reason.
- **`after_provider_response`.** Never fires with the `openai-codex` provider (probe
  2026-09-03). Capture does not depend on that event.
- **No hook process.** The unread-stdin-above-1-MB runner probe does not apply to Pi; the
  equivalent is oboete's own capture child (256 KiB read bound, A14).
- **`session_compact`.** `compactionEntry.id` differs per compaction; the next
  `before_agent_start` follows `session_compact`. Manual compact may report `Already compacted`.
- **Tool result shape.** `tool_result` carries `input`, `content`, `isError` (and `details` for
  edit) and is the single subscription point.

## How to remove

```bash
oboete setup --agents pi --remove
```

That deletes `~/.pi/agent/extensions/oboete.js` when it starts with `// oboete:managed`. A file
without the marker is refused (`unmarked_handler`) rather than overwritten or deleted. The
consent record is kept. Hung acknowledgement files, if any, are under `~/.oboete/spool/pi-ack/`;
doctor's recovery for the `pi` item is: `Delete the `.started` files under <pi-ack> and run
`oboete observe`; if it recurs, run `oboete setup --agents pi`.`
