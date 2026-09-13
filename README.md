# oboete

## What it is

oboete captures what happens in a coding session on this machine, writes summaries in the
background, and injects the relevant decisions, discoveries, and next actions into the next
session of Claude Code, Codex, Grok Build, or Pi. All four agents share one SQLite store; the
boundaries are sensitivity and repository, never which agent produced a memory. There is no
subscription: capture and lexical search work with zero credentials, and a remote summarizer is
optional after an explicit consent screen.

## Status

This is milestone M1, version `0.1.0-alpha.0`. It is a self-use alpha: the maintainer dogfoods it
under an isolated Linux user, and it is not a supported public package. Install it from a packed
tarball built from a checkout. Publication to the npm registry is milestone M3. Encrypted remote
sync and semantic (vector) search are milestone M2 and are not implemented. macOS support is
milestone M4. Windows support is milestone M5.

The previous implementation is preserved under [`legacy/`](legacy/README.md) as read-only evidence.

## Shape

- One SQLite file, `~/.oboete/memory.db`, is the product. There is no resident daemon or remote
  procedure call. The only port oboete ever binds is the loopback port of `oboete view` while that
  command runs in the foreground.
- Hooks are short-lived processes with a 300 ms budget; summarization runs in a detached worker.
- All four agents share one store. Boundaries are sensitivity and repository, never the agent.
- Sensitivity is decided at capture and fails closed; availability fails open. Secrets are
  redacted before storage.
- Observer LLM: Cloudflare Workers AI free tier by default, a named OpenAI-compatible preset
  otherwise, and a rule-based fallback when neither is reachable. There is no Anthropic preset
  (owner decision A19).
- TypeScript on Node.js >= 22.16 with `node:sqlite`; the schema and command-line contract are the
  seam for any later rewrite.

The full set of rules is in [`CONSTITUTION.md`](CONSTITUTION.md).

## Milestones

| Milestone | Scope |
| --- | --- |
| M1 | Self-use Alpha: four agents, web viewer, doctor, isolated dogfood |
| M2 | Encrypted R2 sync, hybrid lexical + vector search |
| M3 | npm publication |
| M4 | macOS, private MCP link |
| M5 | Windows |

## Requirements

| | |
| --- | --- |
| Node.js | 22.16 is the engine minimum (`engines.node` is `>=22.16`, and `node:sqlite` is unflagged there). 24.x is recommended and is what the isolated dogfood account runs, because Pi 0.84.4 requires Node.js >= 22.19. Continuous integration exercises 22.16.0 and 24.x. |
| Operating system | Linux today (including this Windows Subsystem for Linux host). macOS is milestone M4. Windows is milestone M5. Paths go through `os.homedir()` and `node:path`; there is no Unix-socket, `flock`, or bash-only hook. |

## Install

From a checkout of this repository, after `npm ci` and `npm run build`:

```bash
npm pack
npm install -g ./oboete-0.1.0-alpha.0.tgz
```

The packed tarball name follows `package.json` `version`. `npm run pack-check` builds, packs,
installs into an empty prefix, and prints the installed size; the recorded pass on 2026-09-06 is
20.280 MB against a 30 MB limit ([docs/evidence/m1-resource-envelope.md](docs/evidence/m1-resource-envelope.md)).

Data lives in one directory: `~/.oboete/`, or the directory named by `OBOETE_HOME` when that
variable is set (a relative value is resolved against the home directory, so every process agrees).
That directory holds `memory.db`, `config.toml` (preset, model, consent record), `spool/`,
`logs/`, and the `paused` marker. Credentials never live in `config.toml`; they come only from
`OBOETE_*` environment variables.

## Setup

```bash
oboete setup
oboete setup --agents claude,codex,grok,pi --provider workers-ai --accept-egress
oboete setup --provider ollama
oboete setup --provider none
oboete setup --yes
oboete setup --remove
oboete setup --agents claude --remove
```

`--agents` is a comma-separated list of `claude`, `codex`, `grok`, and `pi`. When omitted, setup
wires every installed agent. Named agents are reported even when they are not installed. `--provider`
is one of `workers-ai`, `ollama`, `nim`, `openrouter`, `gemini`, `agent-cli`, or `none`. `--json`
prints the same report as a JSON object.

What setup writes, one line each:

- Claude Code: oboete-owned handlers (`"oboete": true`) in `~/.claude/settings.json` (or
  `CLAUDE_CONFIG_DIR`), then `claude mcp add oboete --scope user -- <node> <bundle> mcp`.
- Codex: matcher groups in `~/.codex/hooks.json` (or `CODEX_HOME`) and a managed block in
  `config.toml` next to it, holding `[hooks.state."<path>:<event>:<group>:<handler>"]
  trusted_hash` rows plus `[mcp_servers.oboete]`.
- Grok Build: `~/.grok/hooks/oboete.json` (or `GROK_HOME`) and a managed block in `~/.grok/config.toml`
  with `[mcp_servers.oboete]` (`enabled = true`). Grok rewrites that file without comments on an
  update; setup recognizes its own table without the markers and puts the block back.
- Pi: the loader `~/.pi/agent/extensions/oboete.js` (or under `PI_CODING_AGENT_DIR`), which imports
  `piExtension` from the packed `pi-extension.mjs`.

The consent screen exists because a remote preset would send memory material off this machine.
Setup prints the tuple it is bound to — preset, destination host, credential source, cost class,
and sensitivity classes that would be sent — and refuses to write the destination until that tuple
is accepted. `--accept-egress` accepts the tuple shown now. `--yes` accepts only when the stored
consent hash already equals the hash of that same tuple; a changed host, credential source, cost
class, or egress class refuses `--yes`. Local presets (`ollama`) and `none` do not require that
confirmation. A refused run leaves the previous destination unchanged (exit 2) and prints:

```
Setup changed nothing: this destination has not been consented to.
Accept it with `oboete setup --accept-egress`, or with `oboete setup --yes` once a stored
record matches the tuple above. `oboete setup --provider ollama` keeps everything on this machine.
```

`oboete setup --provider ollama` keeps summarization on this machine (`127.0.0.1:11434`). The
`none` preset stores no provider: memories are written by rule alone. When the chosen preset has
no credentials, setup prints the Cloudflare free-account steps (for `workers-ai`) or `Export that
variable in the shell that runs the agents.`, then continues without a provider instead of failing.

Every run that gets past the gate prints a per-agent table of `wired`, `probe`, `trust`, and
`native memory`, and this launch line:

> Open the memory viewer with `oboete view --open`.

Exit 1 if any probe failed or the database could not be opened. `--remove` takes the handlers back
out and keeps the consent record.

### Provider presets

| Preset | Destination host | Credential variables | Cost class | Daily cap |
| --- | --- | --- | --- | --- |
| `workers-ai` (default) | `api.cloudflare.com` | `OBOETE_CF_API_TOKEN` and `OBOETE_CF_ACCOUNT_ID` | `free-tier` | 150 HTTP attempts per Coordinated Universal Time day |
| `ollama` | `127.0.0.1:11434` | none | `local` | none |
| `nim` | `integrate.api.nvidia.com` | `OBOETE_NIM_API_KEY` | `remote` | 150, shared with the other capped presets |
| `openrouter` | `openrouter.ai` | `OBOETE_OPENROUTER_API_KEY` | `remote` | 150, shared |
| `gemini` | `generativelanguage.googleapis.com` | `OBOETE_GEMINI_API_KEY` | `remote` | 150, shared |
| `agent-cli` | `agent-cli child process` (`claude -p`, `codex exec`, or `grok -p`) | the agent's own login (`observer.agent_cli`, default `claude`) | `own-subscription` | none (the subscription is billed instead) |
| `none` | none | none | `none` | none |

There is no `anthropic` preset. Consent for `agent-cli` also prints that every summary is billed to
that subscription rather than to an oboete allowance. The default Workers AI model is
`@cf/zai-org/glm-4.7-flash`; the observer asks it not to emit its reasoning
(`chat_template_kwargs.enable_thinking = false`), which keeps a call at a few seconds and a few
neurons instead of 25 to 45 seconds and 60 to 136 neurons (measured on 2026-09-06). `[observer]
model` in `config.toml` overrides the model.

## Doctor

```bash
oboete doctor
oboete doctor --probe-provider
oboete doctor --no-probe-agents
oboete doctor --json
```

`--probe-provider` makes one live summarizer call and counts it against the daily cap when the
preset is capped. Without that flag, the `provider` item is `unverified` and quotes the last worker
outcome. `--no-probe-agents` does not run a headless wiring probe; each `agent:*` item is
`unverified` and quotes the last setup result, never `healthy`.

Every item has the shape `{ item, status, reason, consequence, recovery }`. The four statuses are
`healthy`, `warning`, `unverified`, and `degraded`. `warning` and `unverified` do not change the
exit code. Exit 0 when nothing is `degraded` and storage integrity passed. Exit 1 when any item is
`degraded`. Exit 3 when storage integrity failed (the file is not a database, the header is
corrupt, or `PRAGMA quick_check` is other than `ok`). Invalid flags exit 2.

Items include the configuration-file check, `paused`, `storage`, `fts`, `migration`, `worker`,
`spool`, `provider`, `allowance`, `catalog` (Workers AI only), `agent:claude`, `agent:codex`,
`agent:grok`, `agent:pi`, `native-memory:<agent>` when that agent's own memory feature is on,
`unrecognized-agents`, and `pi` (Pi capture-child diagnostics). Doctor always prints:

> M1 search is lexical (word match). Semantic search arrives in M2.

> Open the memory viewer with `oboete view --open`.

Example of a degraded storage item (exit 3):

```
item     status    reason
storage  degraded  The file is not a SQLite database (its header is not the SQLite format).
  consequence: Hooks spool every event; nothing is summarized, injected or searchable until storage is repaired.
  recovery: Back up the file; run `oboete export` if it is readable; move the database aside; run `oboete setup`; then `oboete import` the export.
```

## Everyday commands

Exit codes shared by these commands: 0 success or an explicit empty result, 1 target not found or
partially degraded, 2 invalid input, 3 storage or input/output failure. Agent-invoked commands
(`hook`, `capture`, `inject`) always exit 0.

- `oboete search <query> [--limit N]` — same-repository active memories by lexical relevance
  (working directory); an empty result exits 0 with `No memories matched this query in the current
  repository.` and the lexical note.
- `oboete timeline [--session <id>]` — sessions, turns, and memory metadata of the current
  repository; empty list exits 0.
- `oboete get <memory-id>` — one memory inside the current repository; exit 1 if absent or outside
  that boundary (`Memory <id> was not found in the current repository.`).
- `oboete pin <id> [--order N]` / `oboete unpin <id>` — pin state; exit 1 if the memory is not in
  the current repository.
- `oboete delete <id>` — tombstone; the same normalized title and body is not re-created; exit 1 if
  not found.
- `oboete why <session-id> [--turn N]` — injection ledger (included, omitted, trims, staleness,
  deferred deliveries, degraded sentence plus reason code); exit 1 if the session is not in this
  repository.
- `oboete pause` / `oboete resume` — create or remove `~/.oboete/paused` without opening the
  database. Pause prints: "Capture and injection are paused. Run `oboete resume` to continue;
  existing memories are untouched." Exit 0.
- `oboete view [--port N] [--open]` — Preact viewer on `127.0.0.1` with a per-launch token in the
  printed URL (`http://127.0.0.1:<port>/?token=...`); `--open` launches the browser on that URL;
  a non-loopback host exits 2.
- `oboete export [file|-]` — JSON Lines `oboete-export/1`; secret rows and tombstones travel as
  hashes with empty title and body.
- `oboete import [file|-] [--dry-run]` — per-line validated merge; imported rows land as
  `local_only` / `review_state = imported` and stay out of search and injection until the worker
  classifies them; `--map-repo <old-id>=<current-id>` maps a machine-local (`common_dir`) repository
  identity from another installation onto one here; 64 KB per line, 256 MB per file, and a secret
  row that carries any text, concepts or sources is refused; exit 2 on an invalid file.
- `oboete mcp` — stdio JSON-RPC server exposing tools `search`, `timeline`, and `get` under the
  current working directory; a repository identifier in the tool arguments is refused (JSON-RPC
  `-32602`); extra command arguments exit 2; otherwise exit 0 when stdin closes.

`oboete observe` is the detached worker (a hook starts it when work is queued). `oboete sync` is
milestone M2 and is not implemented.

## Privacy model

**What is captured.** Prompts, tool inputs and outputs, last assistant messages, and compaction
summaries of the four agents, after secret detection. Observation granularity follows claude-mem:
those events are given to the observer; only summaries are stored as memories.

**What never is.** Secret values (redacted to `[REDACTED:<rule>]` before the first write, including
the spool). Text wrapped in `<private>` tags, including an unclosed tag through the end of the
field (removed, not stored). Credentials of other agents' sessions or subscription stores (FR-016,
FR-043). The producing agent as an eligibility key. Injected pack text, which is marked
`oboete memory context` … `end of oboete memory context` and is not summarized again. Verbatim
tool output in a pack (packs quote summarizer output or rule-based records, each line prefixed
with `> `).

**Sensitivity classes** (lattice `secret` > `private` > `local_only` > `eligible`; stricter wins):

| Class | How it is reached | Where it may go |
| --- | --- | --- |
| `eligible` | A `local_only` row whose worker detector and entropy checks pass | Remote summarizer; local summarizer of the same repository; injection of the same repository; sync in milestone M2 |
| `local_only` | Default at capture | Local summarizer of the same repository; injection of the same repository; never a remote summarizer until promotion |
| `private` | Never promoted once set; import may carry it. Capture does not assign this class: `<private>` tags are stripped instead | Local summarizer of the same repository; injection of the same repository; never a remote summarizer |
| `secret` | Secretlint, gated entropy, a repository path rule in `.oboete.toml`, or a detector failure that fails closed | Nowhere. `isAllowed` returns false for every destination. The export file carries hashes only |

**Secret detection before any write.** The hook runs the detector (private strip, path rules,
`@secretlint/core` with the recommend preset, gated entropy, and the process's own `OBOETE_*`
values) before the first write anywhere, including the spool. A detector throw, a deadline, or a
malformed `.oboete.toml` stores metadata only (`classification_state = failed`) and never the
unsanitized payload. Availability fails open: capture still exits 0.

**Repository boundary.** Identity is the normalized git remote (userinfo, query, and fragment
removed) or the realpath of `git rev-parse --git-common-dir`. Injection, search, timeline, get,
the Model Context Protocol tools, and the viewer all use that same-repository scope. No setting in
M1 widens it (FR-044). Cross-repository search is milestone M2 or later.

**What leaves the machine.** Only after the consent screen, and only `eligible` rows plus an
opaque repository id, to the destination host of the consented remote preset (Cloudflare
`api.cloudflare.com` by default). `ollama` stays on `127.0.0.1:11434`. `none` and the rule-based
fallback make no network call. The consent hash is recomputed before every reservation and again
immediately before send; a mismatch makes no call and degrades with `consent_changed`.

**Agent boundary.** Another agent's memories are shared by design. The `agent` column is
provenance only. Eligibility is sensitivity and repository, never the producing agent (FR-005,
User Story 1). Setup and doctor warn when Claude auto-memory, Codex memories, or Grok native
memory is enabled; oboete neither reads those stores nor changes them (FR-032, FR-043).

**Export.** Format `oboete-export/1`. A secret row or a tombstone is written with empty `title`,
`body`, `concepts`, and `sources`; the hashes remain so the other side can still recognize the
same content.

## Degraded modes

When no summarizer is reachable, or the daily allowance is exhausted, a rule-based observer writes
records in the same schema. Packs add a `> degraded:` line that is a full sentence; the reason
code stays in the ledger, `oboete why`, and `oboete doctor`.

| Reason | Sentence in a pack | What doctor says |
| --- | --- | --- |
| `summary_pending` | The summary of the previous session is not finished yet, so these are its most recent raw notes. | Not a doctor item; session-start waited up to 1 second, then injected labelled raw activity. |
| `index_unavailable` | The memory index could not be read this time, so some notes are missing. | `fts` degraded: "Search and injection return nothing until full-text search is back (packs say `index_unavailable`)." |
| `empty` | There is nothing recorded for this repository yet. | Not a doctor item. |
| `window_unknown` | The context window of this model is not documented yet, so a deliberately small amount of text was selected. | Not a doctor item; Grok Build reports no model, so the smallest verified window is used. |
| `no_tool_call` | This turn ran no tool, so these notes could not be handed over. | Not a doctor item; Grok Build deferred delivery, recorded in `oboete why`. |
| `not_delivered` | These notes could not be handed over during this turn and stay available for the next one. | Not a doctor item; Grok Build deferred delivery, recorded in `oboete why`. |
| `no_provider` | No summarizer is configured, so these are rule-based notes. | `provider` degraded: "No observer provider is configured." Consequence: "Summaries come from the rule-based fallback only (packs say `Degraded:`)." |
| `unreachable` | The summarizer could not be reached, so these are rule-based notes. | `provider` degraded with the worker's sentence (`Provider request failed.` or `… with HTTP <status>.`). |
| `unusable_output` | The summarizer returned an unusable answer, so these are rule-based notes. | `provider` degraded with the worker's sentence (for example `Provider response was not valid JSON.`). |
| `language_mismatch` | The summarizer answered in another language than the content, so these are rule-based notes. | Surfaced on the provider item when that was the last outcome; otherwise in `oboete why`. |
| `daily_cap` | Today's free summary quota is used up, so these are rule-based notes. | `allowance` / `provider` degraded: "The daily cap of 150 calls is used up." |
| `provider_exhausted` | The summarizer's free allowance is used up, so these are rule-based notes. | `allowance` / `provider` degraded: "The provider reported exhaustion today." |
| `provider_paid` | The configured model is not on the free plan, so these are rule-based notes. | `provider` degraded as `provider_paid`; `catalog` warns when the Workers AI list includes paid-only models. |
| `auth_failed` | The summarizer rejected the credentials, so these are rule-based notes. | `provider` degraded: `Provider authentication failed.` Recovery: the credential steps for the preset. |
| `consent_changed` | The summarizer settings changed after consent was given, so these are rule-based notes. | `provider` recovery: `oboete setup --accept-egress`. |
| `model_alias` | The configured model resolved to a different one, so these are rule-based notes. | `provider` degraded: `The provider returned a different model id.` |
| `timeout` | The summarizer did not answer in time, so these are rule-based notes. | `provider` degraded: `The provider call timed out after 30 seconds.` (the probe deadline; the worker allows 60 seconds). |
| `rule_based` | These notes were written by the built-in rules rather than by a summarizer. | Used when local-only rows went to the fallback by design next to a healthy remote preset. |

The daily cap is 150 HTTP attempts per Coordinated Universal Time day, summed across the capped
presets (`workers-ai`, `nim`, `openrouter`, `gemini`). Attempt 150 is allowed; attempt 151 is
refused and the batch is labelled `daily_cap`. When 10 or fewer calls remain, ten-turn batches go
to the fallback and the remainder is reserved for session-end batches. `ollama` and `agent-cli`
are not counted. A provider 429 with body code 3036 writes `exhausted_at` (labelled
`provider_exhausted`) even if the worker then loses the lease. Search is lexical in M1 (word
match, with a Chinese/Japanese/Korean bigram index); semantic search arrives in M2.

## Agents

**Claude Code.** Setup merges command handlers into `~/.claude/settings.json` and registers the
stdio server with `claude mcp add oboete --scope user`. Packs print on `SessionStart` when `source`
is `startup`, `clear`, or `compact`, and on `UserPromptSubmit`. Tools appear as
`mcp__oboete__search`, `mcp__oboete__timeline`, and `mcp__oboete__get`. Details:
[docs/agents/claude.md](docs/agents/claude.md).

**Codex.** Setup writes `~/.codex/hooks.json` and the `trusted_hash` rows Codex requires before it
will run a handler, plus `[mcp_servers.oboete]`. Injection is `hookSpecificOutput.additionalContext`
on `SessionStart` (`matcher` `startup|clear|compact`) and on `UserPromptSubmit`, which also carries
the session-start pack when the current epoch has none yet (A21). Tools appear as
`mcp__oboete__search` and the same for `timeline` and `get`. Details:
[docs/agents/codex.md](docs/agents/codex.md).

**Grok Build.** Setup writes `~/.grok/hooks/oboete.json` and `[mcp_servers.oboete]` in
`~/.grok/config.toml`. Session-start and prompt-submit packs are stored `pending` and delivered
with the first tool call of the turn that actually runs (FR-045). Tools appear to hooks as
`oboete__search`, `oboete__timeline`, and `oboete__get`. Details:
[docs/agents/grok.md](docs/agents/grok.md).

**Pi.** Setup writes `~/.pi/agent/extensions/oboete.js`. Capture is a detached `oboete capture`
child; injection is a bounded `oboete inject` child from `before_agent_start`. Tools are
`oboete_search`, `oboete_timeline`, and `oboete_get`, each spawning `oboete search|timeline|get
--json`. Details: [docs/agents/pi.md](docs/agents/pi.md).

## Troubleshooting

**A hook is not firing.** Run `oboete doctor` (without `--no-probe-agents`) and read the `agent:<name>`
item. Then run `oboete setup --agents <name>`. Doctor treats a missing Claude handler, a Codex
`untrusted` or `absent` hash, or a missing Grok/Pi file as `degraded` with consequence `<Agent>
sessions capture nothing and receive no memories.` and recovery `` `oboete setup --agents <name>` ``.

**The database is corrupt.** Doctor sets `storage` to `degraded` and exits 3. Recovery, quoted:

> Back up the file; run `oboete export` if it is readable; move the database aside; run `oboete setup`; then `oboete import` the export.

**Codex shows a trust prompt.** Codex skips a handler that has no matching
`[hooks.state."…"] trusted_hash` and says nothing, or the terminal user interface asks before
running an untrusted command. Setup writes those rows from the merged `hooks.json`. If the prompt
still appears, run `oboete setup --agents codex` again so the hash matches the handler JSON. The
isolated-user terminal user interface probe with matching rows started with hooks active and no trust prompt.

**Grok Build updated itself and doctor says it dropped the oboete markers.** Grok re-serializes
`~/.grok/config.toml` without comments, which removes the `# oboete:begin` and `# oboete:end` lines
around the MCP table. Run `oboete setup --agents grok`; setup recognizes the table it wrote and
restores the block. A table that runs another command is left alone and reported instead.

**Grok Build seems to have no memory at the start of a turn.** That is deferred delivery, not a
missed hook. Grok Build has no channel that reaches the model before a tool runs, so the pack
arrives with the first tool call of the turn. `oboete why` reports `deferred: delivered with tool
calls`. A turn with no tool call is labelled `no_tool_call` and the memories stay available for
the next turn.

## Development

```bash
npm ci
npm run build          # esbuild: dist/engine.mjs + dist/oboete.mjs launcher, build/test/*.mjs, viewer assets
npm run typecheck      # tsc --noEmit and the viewer project
npm run lint           # eslint
npm test               # node --test on the compiled tests
npm run pack-check     # npm pack, install into an empty prefix, unpacked size limit 30 MB
```

Measured on this host ([docs/evidence/m1-resource-envelope.md](docs/evidence/m1-resource-envelope.md)):
largest `--version` cold start 59.7 ms against a 100 ms budget; largest hook 252.1 ms against
300 ms (Node.js v24.16.0, secret-dense 200 KB stdin). Installed size after the bundled packages
became development dependencies: 29.152 MB by `du -k`; `pack-check` file-size sum 20.280 MB.

Automated end-to-end validation runs under a separate Linux user, `oboete-dogfood`, with its own
home and its own logins for all four agents (FR-041). oboete is not installed in the maintainer's
own agent environment during M1. The account creation steps, Node.js 24 for that user (Pi requires
>= 22.19), and per-agent login commands are in
[docs/research/isolated-user-setup.md](docs/research/isolated-user-setup.md). Daily dogfood
append-only evidence lives in [docs/evidence/m1-dogfood.md](docs/evidence/m1-dogfood.md).

## Layout

- `CONSTITUTION.md` — project principles and constraints (authoritative).
- `docs/research/` — verified third-party contracts (hook payloads, provider APIs); created with the M1 specification.
- `docs/agents/` — per-agent setup, detection, pack delivery, and removal notes.
- `docs/evidence/` — measured cold start, installed size, fixture replay, and isolated-user dogfood.
- `specs/` — Spec Kit features for oboete milestones (created per milestone).
- `scripts/` — build, pack-check, fixture replay, isolated-user probes, and the DCO checker.
- `legacy/` — the free-mem era, read-only.

## License

Apache-2.0. See [`LICENSE`](LICENSE), [`NOTICE`](NOTICE), and [`CONTRIBUTING.md`](CONTRIBUTING.md).
