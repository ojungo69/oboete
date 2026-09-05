# Plugin Reference

This page covers advanced plugin behavior, environment variables, and stream reliability controls.

This free-mem checkout is pre-release. Build it first with `corepack pnpm run build`
from `vendor/codemem`, then use `node packages/cli/dist/index.js ...`. Published
npm packages, global binaries, and remote marketplaces are not supported install
sources for this checkout.

## Observer and settings UI

<img src="images/codemem-settings.png" alt="codemem observer settings" width="520" />

## Running OpenCode with the plugin

1. Run checkout setup once, then start OpenCode in the target project.
2. Every tooling session creates memory artifacts in SQLite.
3. Prompt-time memory injection appends volatile recall output to the latest user message by default, preserving the stable system/history prefix for provider prompt caches.
4. Use `node packages/cli/dist/index.js stats` and `node packages/cli/dist/index.js recent` from this checkout to confirm ingestion.
5. Browse the viewer at the printed URL.

OpenCode prompt-time pack construction and prompt-pack ledger transitions use the
long-lived local viewer first. Retryable connection, timeout, endpoint-version,
server, or malformed-response failures fall back to the compatible CLI path.
Validated request errors are terminal and do not spawn a fallback command. The
HTTP timeout uses `CODEMEM_INJECT_HTTP_MAX_TIME_S` (default: 2 seconds).
Pack and ledger requests include their resolved default or explicit database,
identity/config, compression, and embedding targets. The viewer also rejects a cached store
identity that no longer matches current database/config resolution. A mismatch
uses the CLI fallback instead of accepting context from another local profile.
Arbitrary 4xx responses from a process on the viewer port also fall back; only
structured Codemem validation errors are terminal. A payload-free profile
handshake runs before each POST, and Fetch redirects are disabled so prompt-derived
request bodies are not replayed to another endpoint.

## Claude setup

Install MCP and all seven hook groups directly from the built checkout:

```text
node packages/cli/dist/index.js setup --claude-only
```

Setup writes the absolute built CLI MCP command into
`$CLAUDE_CONFIG_DIR/.claude.json`, copies the bundled standalone runtime into
that directory, and merges only codemem-owned hook groups into `settings.json`.
Without the override it uses `~/.claude.json` for MCP and `~/.claude` for hooks.
Unrelated MCP servers, state, settings, plugins, and hooks are preserved. A
malformed config file or unknown custom codemem MCP entry is left untouched.

Claude hooks run the packaged standalone Node runtime. It applies project policy and redaction before sending normalized events to the local daemon RPC socket; when RPC is unavailable, the same redacted event is written to the bounded atomic spool:

- `node <CLAUDE_CONFIG_DIR>/codemem-hook-runtime.mjs claude-hook-ingest`

Hook clients never open SQLite. Claude's outer watchdog is 3 seconds; the client uses a shorter RPC cutoff so the spool has a reserved completion window.

Redaction worker readiness is bounded separately from the 100 ms scan deadline. Source and daemon
callers allow up to 500 ms from the worker's original start; hooks use the earlier caller deadline
that preserves their 500 ms spool fallback. A caller whose readiness start deadline has elapsed does
not begin a scan; in the standalone hook worker, immediate retry scans are suppressed for 500 ms so
spool fallback keeps its reserve. After a true failed readiness attempt, calls during
the next 500 ms only probe readiness and fail closed instead of paying the wait again. A true
readiness or scan failure keeps only safe metadata; unscanned content is never delivered or
persisted.

The CLI keeps a compatible manual stdin entry point for development:

```bash
printf '%s\n' '{"hook_event_name":"SessionStart","session_id":"sess-1","cwd":"/tmp/demo"}' | node packages/cli/dist/index.js claude-hook-ingest
```

Prompt-time context and file-context reads use daemon RPC. `UserPromptSubmit` performs recall and event delivery in one hook invocation; `SessionEnd` is delivered through the same RPC-or-spool path as other events.

Setup registers these hook events in `settings.json`:
- `SessionStart`
- `UserPromptSubmit`
- `PreToolUse` (`Read` only)
- `PostToolUse`
- `PostToolUseFailure`
- `Stop`
- `SessionEnd`

`UserPromptSubmit` runs the setup-managed `codemem-hook-runtime.mjs claude-hook-inject`, which requests a context pack over daemon RPC while delivering the normalized prompt event. Failures return a continue response and never block the Claude session.

For Claude hooks, project resolution precedence is:

1. `CODEMEM_PROJECT` (if set)
2. repo/cwd-derived project name (`resolve_project(cwd)`)
3. payload `project` fallback (only when cwd is unavailable)

`PreToolUse:Read` requests the existing per-file observation timeline over daemon RPC. Retrieval attempts and delivery status remain recorded in the daemon-owned retrieval ledger.

The tracked Claude plugin remains a hook-only future release template. Setup
disables only an enabled `codemem@codemem-marketplace` entry in the selected Claude
config to prevent duplicate hook delivery. Re-run setup after rebuilding or moving
the checkout; do not use the marketplace cache as the pre-release runtime.

## Codex integration (early beta)

Codex support is early beta. The tracked plugin is a hook-only future release
template; the supported pre-release installation path is direct setup from this
checkout.

Codex hooks use the same standalone runtime, daemon RPC, redaction, and bounded atomic spool as Claude. Hook clients never open SQLite. The Codex outer watchdog is 5 seconds.

When either hook runtime falls back to the spool, `ENOENT` from post-publication
or stale-lock revalidation is treated as contention and retried within the
bounded lock deadline. Other errors from those revalidation reads remain spool
failures.

```bash
printf '%s\n' '{"hook_event_name":"SessionStart","session_id":"codex-1","cwd":"/tmp/demo"}' | node packages/cli/dist/index.js codex-hook-ingest
```

`UserPromptSubmit` runs the setup-managed `codemem-hook-runtime.mjs codex-hook-inject`, which requests a context pack and delivers the prompt event concurrently over daemon RPC. The injected pack is framed as codemem reference data, not instructions, before it is returned as Codex `additionalContext`. It honors `CODEMEM_INJECT_CONTEXT`, `CODEMEM_INJECT_LIMIT`, `CODEMEM_INJECT_TOKEN_BUDGET`, and `CODEMEM_INJECT_MAX_CHARS`. Hook failures always emit `{"continue": true}` so Codex sessions are never blocked.

```bash
printf '%s\n' '{"hook_event_name":"UserPromptSubmit","session_id":"codex-1","prompt":"what did we change","cwd":"/tmp/demo"}' | node packages/cli/dist/index.js codex-hook-inject
```

For Codex hooks, project resolution precedence matches the Claude hook path:

1. `CODEMEM_PROJECT` (if set)
2. repo/cwd-derived project name
3. payload `project` fallback (only when cwd is unavailable)

`Stop` events map the inline `last_assistant_message` when present, and fall back to the last assistant message in `transcript_path` so final responses are captured even when the inline field is omitted.

The packaged Codex template registers `SessionStart`, `UserPromptSubmit`, `PostToolUse`, `Stop`, and `SessionEnd` in `plugins/codex/hooks/hooks.json`. Codex support is early beta; see `docs/plans/2026-05-28-codex-first-class-integration.md` for the rollout plan and validation gates.

### Install and update

Configure Codex directly from the built checkout:

```bash
node packages/cli/dist/index.js setup --codex-only
```

What it does (idempotent; honors `CODEX_HOME`; backs up existing files; `--force` to refresh):

- **MCP:** writes `[mcp_servers.codemem]` with the absolute Node executable and this checkout's absolute built CLI path in `<CODEX_HOME>/config.toml`, preserving comments and unrelated servers.
- **Hooks:** installs the bundled runtime as `<CODEX_HOME>/codemem-hook-runtime.mjs` (mode `0600`) and merges `SessionStart`, `UserPromptSubmit`, `PostToolUse`, `Stop`, and `SessionEnd` into `<CODEX_HOME>/hooks.json`, preserving unrelated user hooks. `UserPromptSubmit` uses one combined capture/recall hook. Existing legacy codemem hook groups are migrated on a normal rerun.

Hooks loaded from the user config layer require a one-time trust approval in Codex (you'll be prompted on first run; MCP recall needs no trust). Plain `codemem setup` installs Claude Code and Codex together; use `codemem setup --opencode-only` for the retained OpenCode compatibility lane.

### Troubleshooting

- **No memories and no raw events captured.** Confirm `<CODEX_HOME>/hooks.json` points to `<CODEX_HOME>/codemem-hook-runtime.mjs`, then check daemon health. Hook failures are fail-open and RPC failures enter the shared `control/spool/ready` queue under the configured data directory.
- **Spool backlog drains automatically** when the daemon starts and during its periodic sweep. A retained backlog means the daemon is unavailable or rejecting the normalized event; inspect daemon health/doctor output and `~/.codemem/plugin.log`.
- **A model rejects injected context** (for example "the conversation must end with a user message"): disable prompt-time injection with `CODEMEM_INJECT_CONTEXT=0`. Capture/ingest keeps working and recall is still available through the MCP tools.

## Post-restart config sanity checklist

After restarting OpenCode or the viewer, run this quick check when behavior looks off:

1. Confirm plugin + viewer are talking to the same DB path.
2. Check backend stats and recent writes with the built CLI (`node packages/cli/dist/index.js stats` and `node packages/cli/dist/index.js recent`).
3. Verify runner mode and source (`CODEMEM_RUNNER`, `CODEMEM_RUNNER_FROM`) match your install strategy.
4. Confirm injection controls are what you expect (`CODEMEM_INJECT_CONTEXT`, `CODEMEM_INJECT_LIMIT`, `CODEMEM_INJECT_TOKEN_BUDGET`).
5. If stream mode is enabled, check backlog health (`node packages/cli/dist/index.js db raw-events-status`).

If needed, restart viewer + plugin flow:

```bash
node packages/cli/dist/index.js serve restart
```

If you override the viewer bind, keep the plugin and viewer aligned on the same target:

```bash
set -lx CODEMEM_VIEWER_HOST 127.0.0.1
set -lx CODEMEM_VIEWER_PORT 38892
```

The plugin now passes that explicit host/port through when it auto-starts, health-checks, stops, or restarts the viewer. Its liveness monitor requires a successful `GET /api/health` JSON response identifying `service: "codemem-viewer"`; `ready: false` still means the viewer process is live. For compatibility, only a `404` from the health route triggers one bounded probe of the legacy raw-event status endpoint. Raw-event ingest availability keeps its separate preflight behavior, now bounded by a 5-second timeout so a hung viewer socket cannot stall event delivery. Do not run multiple viewers against the same DB/runtime folder unless they intentionally share the same bind target; otherwise `viewer.pid` ownership becomes ambiguous.

If compatibility toasts appear after restart, follow the runner-specific guidance in Compatibility guidance behavior below.

## Plugin tools exposed to the model

- `mem-status` - show viewer URL, log path, stats, and recent entries.
- `mem-stats` - show just the stats block.
- `mem-recent` - show recent items (defaults to 5).

These are plugin tools callable by the agent/runtime. They are not user-facing
slash commands in the OpenCode chat input.

## MCP tools exposed to agents

The stdio MCP process is a thin daemon RPC client and never opens SQLite. Phase 1
exposes `memory_search`, `memory_search_index`, `memory_recent`, `memory_timeline`,
`memory_expand`, `memory_explain`, `memory_get`, `memory_get_observations`,
`memory_pack`, `memory_schema`, `memory_remember`, and `memory_status`.

Read calls fail with typed `daemon_unavailable` output while the daemon is down.
`memory_remember` is pre-redacted and queued to the shared atomic spool instead.
User-authority tools such as forget, confirm, pin, retract, and destructive bulk
actions are not registered for agents.

Example agent requests:

- "Find recurring project lessons worth adding to AGENTS.md."
- "Run distill for all projects and show top candidates."
- "Distill without judging so I can see the raw recurrence ranking."

## Observer model defaults

Slice 1 has no runtime model default or tier router. Plain setup requires an exact model ID and
complete endpoint, fingerprints that choice, and freezes it for the daemon lifetime.

### Observer auth modes

The only credential forms are `none` and a named environment-variable reference entered during
setup. Inline secrets, auth files, custom headers, fallback token cascades, runtime URL suffixes,
and mutable provider environment overrides are rejected or recorded only as legacy dispositions.

## Stream-only mode (advanced)

Stream contract:
- Preflight availability: `GET /api/raw-events/status`
- Event streaming: `POST /api/raw-events`
- Non-2xx and network failures are treated as stream failures.
- Raw events are delivered through the viewer ingest API.
- Raw-event batches accepted by the viewer are retained. With an activated Slice 1 capability manifest, the daemon processes them through the scheduled flush and sweeper; in capture-only mode (no activated manifest) they remain retained without processing.
- If the direct CLI fallback reports an explicit SQLite busy/locked result or command timeout, the plugin retries it once with the same event ID. Other failures are reported and dropped rather than requeued or spooled, and logs retain only a bounded failure category rather than raw command output.

Slice 1 fixes the scheduler values in the manifest: 1 s debounce, 30 s sweep, 120 s idle,
and retention disabled (`0`). These are not configurable through legacy `CODEMEM_RAW_EVENTS_*`
settings. Flush and sweeper execution start only when a validated Slice 1 capability manifest
is activated; without one the daemon stays in capture-only mode.

To monitor backlog:

```bash
node packages/cli/dist/index.js db raw-events-status
```

`raw-events-status` can show retained backlog; processing and bounded retries run only under an activated Slice 1 capability manifest.

## Hook lifecycle and processing availability

The plugin captures `tool.execute.after`, `session.idle`, `session.created`, `/new` prompt-boundary,
and `session.error` events. Raw-event flush, retries, and sweeper processing run only under an activated Slice 1 capability manifest; without one the daemon is capture-only.

Failure semantics:
- Stream POST failures are backoff-gated in plugin runtime (`CODEMEM_RAW_EVENTS_BACKOFF_MS`).
- Availability checks are rate-limited (`CODEMEM_RAW_EVENTS_STATUS_CHECK_MS`).
- Accepted raw-event batches remain retained while processing is disabled.

## Project label normalization

When ingesting plugin payloads, CodeMem stores a normalized project label instead of a full path.

- Path-like labels are reduced to the basename (for example, `/Users/adam/workspace/codemem` -> `codemem`).
- Windows-style paths are normalized with Windows path rules on every OS runtime.
  - `C:\Users\adam\workspace\codemem` -> `codemem`
  - `D:/dev/client-demo` -> `client-demo`
  - `\\server\share\team\project-x` -> `project-x`
- `CODEMEM_PROJECT` still has highest precedence and is normalized the same way.

### Multi-adapter project unification

If you run multiple adapters for the same project (for example OpenCode + Claude), set a shared `CODEMEM_PROJECT` value in both runtimes to guarantee unified project grouping in memory retrieval.

## Environment hints

| Env var | Description |
| --- | --- |
| `CODEMEM_RUNNER` | OpenCode runner override. Pre-release setup pins this to the absolute Node executable in its managed local wrapper. |
| `CODEMEM_RUNNER_FROM` | OpenCode runner source. Pre-release setup pins this to the absolute built CLI entry. |
| `CODEMEM_VIEWER` | Set to `0`, `false`, or `off` to disable the viewer entirely. |
| `CODEMEM_VIEWER_HOST`, `CODEMEM_VIEWER_PORT` | Explicit host/port the plugin-managed viewer should start, probe, stop, and restart. |
| `CODEMEM_VIEWER_AUTO` | Set to `0`/`false`/`off` to disable auto-start (default on). |
| `CODEMEM_VIEWER_AUTO_STOP` | Set to `0`/`false`/`off` to keep the viewer running after OpenCode exits (default on). |
| `CODEMEM_PLUGIN_LOG` | Path for the plugin log file (set `1`/`true`/`yes` for `~/.codemem/plugin.log`; Claude hook failures are logged to this path by default). |
| `CODEMEM_PLUGIN_LOG_PATH` | Explicit log file path for Claude hook script logging (overrides `CODEMEM_PLUGIN_LOG` for that script). |
| `CODEMEM_INJECT_HTTP_MAX_TIME_S` | Viewer request timeout for OpenCode packs and ledger transitions (default `2` seconds). Claude/Codex hooks use daemon RPC deadlines instead. |
| `CODEMEM_INJECT_MAX_CHARS` | Max chars returned as Claude/Codex `additionalContext` (default `16000`). |
| `CODEMEM_PLUGIN_CMD_TIMEOUT` | Milliseconds before a plugin CLI call is aborted (default `20000`). |
| `CODEMEM_MIN_VERSION` | Minimum required CLI version for plugin compatibility warnings (default `0.9.20`). |
| `CODEMEM_BACKEND_UPDATE_POLICY` | Backend update behavior on compatibility mismatch: `notify` (default), `auto`, or `off`. |
| `CODEMEM_PLUGIN_DEBUG` | Set to `1`, `true`, or `yes` to log plugin lifecycle events. |
| `CODEMEM_PLUGIN_IGNORE` | Skip all plugin behavior for this process. |
| `CODEMEM_INJECT_CONTEXT` | Set to `0` to disable memory pack injection (default on). |
| `CODEMEM_INJECT_SURFACE` | OpenCode injection surface: `message` by default; set `system` for the legacy system-prompt transform. |
| `CODEMEM_INJECT_LIMIT` | Max memory items in injected pack (default `8`). |
| `CODEMEM_INJECT_TOKEN_BUDGET` | Approx token budget for injected pack (default `800`). |
| `CODEMEM_RAW_EVENTS_BACKOFF_MS` | Backoff window after stream failure before retrying stream POSTs (default `10000`). |
| `CODEMEM_RAW_EVENTS_STATUS_CHECK_MS` | Minimum interval between stream availability preflight checks (default `30000`). |
| `CODEMEM_RAW_EVENTS_HARD_MAX` | Hard upper bound for in-memory plugin queue under sustained failure pressure (default `2000`). |

## Compatibility guidance behavior

When the plugin detects a CLI/runtime version mismatch in this pre-release checkout,
pull the intended revision, run `corepack pnpm run build`, rerun the matching setup
command, and restart the editor.

Update policy:

- `CODEMEM_BACKEND_UPDATE_POLICY=notify` (default): show warning toast with suggested action
- `CODEMEM_BACKEND_UPDATE_POLICY=auto`: retained for future releases; checkout-pinned `node` mode is never auto-updated
- `CODEMEM_BACKEND_UPDATE_POLICY=off`: no compatibility toast (logging still records mismatch)

Compatibility checks do not block plugin startup.
