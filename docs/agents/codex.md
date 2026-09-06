# Codex

## What setup writes

`oboete setup --agents codex` writes two files under `~/.codex/` (or `CODEX_HOME`), in this order:

1. `hooks.json` — oboete-owned matcher groups (`"oboete": true`).
2. `config.toml` in that same directory — a managed block with one
   `[hooks.state."<absolute-path>:<event>:<group>:<handler>"] trusted_hash = "sha256:<hex>"` row
   per oboete handler, plus `[mcp_servers.oboete]` (`command` = the node binary,
   `args` = `[<bundle>, "mcp"]`).

The trust key is read back from the merged `hooks.json` after the developer's own groups, so an
earlier group shifts the index. Only oboete's handlers get a row. `additionalContextLimit = 0` on
the injection events disables Codex's 2,500-token spill, which would move the tail of a pack to a
temporary file the model never reads.

Each handler is:

```text
<node> <bundle> hook --agent codex --event <Event>
```

| Event | Matcher | Timeout (seconds) | Role |
| --- | --- | --- | --- |
| `SessionStart` | `startup\|clear\|compact` (`resume` excluded) | 12 | Capture and injection |
| `UserPromptSubmit` | (none) | 12 | Capture and injection, including the A21 session-start fallback |
| `PreToolUse` | (none) | 3 | Capture |
| `PostToolUse` | (none) | 3 | Capture |
| `Stop` | (none) | 3 | Capture |
| `PostCompact` | (none) | 3 | Capture (no summary field; keys `session_id`, `turn_id`, `transcript_path`, `cwd`, `hook_event_name`, `model`, `trigger`) |
| `SessionEnd` | (none) | 3 | Capture only (Codex clamps this event) |

If the TOML write fails, setup rolls `hooks.json` back so Codex is not left with handlers that
have no trust rows (those would be wired and silently inert).

## Detection

Installed when the `codex` executable is on `PATH` or `~/.codex` exists. Trust is `trusted` when
every oboete handler's `trusted_hash` matches, `untrusted` when the file exists but a hash does
not match (or the JSON is not a handler oboete understands), and `absent` when there is no oboete
group. Doctor treats `absent` and `untrusted` as a missing hook: `Codex has not trusted the hook
definition in <hooks.json>.` / `No oboete hook in <hooks.json>.`

## Trust step

Codex will not run a hook until the canonical handler JSON hashes to the `trusted_hash` row
(preimage includes `"async": false`, the group's `matcher` when present, and the configured
`timeout`). With matching rows, the terminal user interface starts with hooks active and no trust
prompt; `--dangerously-bypass-hook-trust` is not needed. If a trust prompt still appears, run
`oboete setup --agents codex` again.

## When packs arrive

Injection is `hookSpecificOutput.additionalContext` on `SessionStart` when `source` matches
`startup|clear|compact`, and on every `UserPromptSubmit`. Codex fires `SessionStart` lazily, at
the start of the next turn rather than at the event itself (codex-cli 0.153.0, isolated-user
measurements 2026-09-05):

- After a TUI `/compact`, `PostCompact` fires at once and `SessionStart source = compact` about
  200 ms before the next turn's `UserPromptSubmit`.
- After `/new`, the parent gets no `SessionEnd` (it arrives at `/quit`) and the new session's
  `SessionStart source = startup` fires about 200 ms before its first `UserPromptSubmit`.

A session that quits before its next turn therefore shows neither hook (A18). As a fallback, the
session-start pack of an epoch is also delivered on the first `UserPromptSubmit` that finds no
session-start injection for the conversation's current `context_epoch` (A21).

## Native-memory coexistence

If `~/.codex/memories` exists, or `features.memories` / `memories` / `memories.generate_memories`
/ `memories.use_memories` is enabled in `config.toml`, setup and doctor warn: `codex: its own
memory feature (codex_memories) is enabled. oboete neither reads it nor changes it; the two run
side by side.`

## Memory tools

`[mcp_servers.oboete]` speaks stdio JSON-RPC. Codex presents the tools as `mcp__oboete__search`,
`mcp__oboete__timeline`, and `mcp__oboete__get`. A repository identifier in the tool arguments is
refused (JSON-RPC `-32602`). Search is lexical in M1.

## Known limitations

- **A16 (compaction identity).** `PostCompact` carries `turn_id` and `trigger` only; there is no
  per-compaction id and no summary text (`compaction_summary` absent by contract). The epoch key
  is the `PostCompact` event id. Ordering is fine: `PostCompact` precedes `SessionStart(compact)`
  by about 24 ms.
- **A18 (`/new`).** The TUI `/new` command does not fire `SessionStart source = clear`. A cleared
  session is detected by the session id changing on the next hook; the session-start pack is
  injected there, one prompt later than on the other agents.
- **A21 (lazy SessionStart).** See "When packs arrive". Harness rule: a Codex hook that belongs
  to the next turn is observed by sending that turn, never by waiting.
- **No read tool.** Reads arrive as Bash commands. Writes and edits arrive as `apply_patch`; the
  path is only inside the patch text.
- **Rollout flush.** The just-finished `tool_use_id` is already in `transcript_path` when
  `PostToolUse` runs (4 of 4 calls in the 2026-09-03 probe), so capture stays hook-stdin based.
- **Hook stdin cap.** The Codex runner delivered about 4.8–41 KB of a 1.2 MB tool result.
- **JSON stream.** `codex exec --json` (the `agent-cli` preset) writes the last message to a file
  and carries no model id.

## How to remove

```bash
oboete setup --agents codex --remove
```

That strips the oboete groups from `hooks.json` and the managed block from `config.toml` in the
same directory. A file oboete created that is left as an empty shell is deleted. The consent
record is kept.
