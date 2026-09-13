# oboete M1 dogfood evidence

Isolated-user cross-agent runs for SC-001, SC-004, and SC-007.

## 2026-09-04 run 2026-09-04T15-18-20-953Z

- 6 of 12 pairs pass
- No provider credentials: no
- Report: <run>/report.json

| seed | receive | status | elapsed ms | missing facts |
|---|---|---:|---:|---|
| claude | codex | pass | 93189 | none |
| claude | grok | fail | 42660 | fact-2026-09-04T15-18-20-953Z-claude-to-grok-1: the build token is cedar.; fact-2026-09-04T15-18-20-953Z-claude-to-grok-2: the release bird is heron.; fact-2026-09-04T15-18-20-953Z-claude-to-grok-3: 配布色は琥珀。 |
| claude | pi | pass | 67683 | none |
| codex | claude | pass | 65183 | none |
| codex | grok | fail | 64271 | fact-2026-09-04T15-18-20-953Z-codex-to-grok-1: the build token is cedar.; fact-2026-09-04T15-18-20-953Z-codex-to-grok-2: the release bird is heron.; fact-2026-09-04T15-18-20-953Z-codex-to-grok-3: 配布色は琥珀。 |
| codex | pi | pass | 75516 | none |
| grok | claude | fail | 2242 | fact-2026-09-04T15-18-20-953Z-grok-to-claude-1: the build token is cedar.; fact-2026-09-04T15-18-20-953Z-grok-to-claude-2: the release bird is heron.; fact-2026-09-04T15-18-20-953Z-grok-to-claude-3: 配布色は琥珀。 |
| grok | codex | fail | 2150 | fact-2026-09-04T15-18-20-953Z-grok-to-codex-1: the build token is cedar.; fact-2026-09-04T15-18-20-953Z-grok-to-codex-2: the release bird is heron.; fact-2026-09-04T15-18-20-953Z-grok-to-codex-3: 配布色は琥珀。 |
| grok | pi | fail | 2506 | fact-2026-09-04T15-18-20-953Z-grok-to-pi-1: the build token is cedar.; fact-2026-09-04T15-18-20-953Z-grok-to-pi-2: the release bird is heron.; fact-2026-09-04T15-18-20-953Z-grok-to-pi-3: 配布色は琥珀。 |
| pi | claude | pass | 62417 | none |
| pi | codex | pass | 72459 | none |
| pi | grok | fail | 63601 | fact-2026-09-04T15-18-20-953Z-pi-to-grok-1: the build token is cedar.; fact-2026-09-04T15-18-20-953Z-pi-to-grok-2: the release bird is heron.; fact-2026-09-04T15-18-20-953Z-pi-to-grok-3: 配布色は琥珀。 |

### Why the six Grok Build pairs failed

Every failing pair is a Grok Build leg. The Grok Build account the isolated user holds has no
credit left, so `grok -p` exits 1 without reaching a hook:

```
{"type":"error","message":"Internal error: {\n  \"message\": \"API error (status 402 Payment Required): Grok Build usage balance exhausted\",\n  \"http_status\": 402\n}"}
```

The three `grok` seeding pairs fail in about 2 s (the account cannot start a session at all) and the
three `grok` receiving pairs fail after the seed and the summary succeeded. Nothing in oboete is
implicated: the six pairs among Claude Code, Codex and Pi pass, including both directions of every
one of those three agents. SC-001 stays open until the balance is restored and the run repeats.

### What this run does and does not prove

The Codex legs run with `--dangerously-bypass-hook-trust`, because the harness copies `hooks.json`
into the pair directory while a Codex trust key names the absolute path of the original file. So
this run does not gate the trust-hash rule; that gap is being closed separately.

## 2026-09-05 wiring re-verification (isolated user, after 037a5bb)

The two defects 037a5bb fixed were found by driving the real CLIs, so the fixes were re-checked the
same way. The bundle built from 037a5bb was installed for the isolated user
(`npm i -g` of `npm pack`, so the shipped bundle rather than the worktree source), and
`oboete setup --yes --accept-egress` reported all four agents `wired` with a passing probe.

| check | command | result |
|---|---|---|
| Claude MCP scope | `claude mcp get oboete` from a temporary directory that is not the setup directory | `Scope: User config (available in all your projects)`; the entry is under the top-level `mcpServers` of `~/.claude.json` |
| Pi tool arguments | `pi -p 'Call the oboete_search tool with query "cedar" and limit 3 …'` in a fresh repository | the tool returned `{"memories":[],"reason":"No memories matched this query in the current repository.",…}` — the query reached `oboete search`, where before the fix every tool answered "oboete could not run that command" |

`claude mcp get oboete` also reports `Status: ✘ Failed to connect`, which is expected: `oboete mcp`
is still the T077 stub and answers "oboete mcp is not implemented yet". The registration is what
this check covers.

The isolated user runs pi-coding-agent 0.84.4 and this machine's developer account runs 0.85.0; the
`ToolDefinition.execute(toolCallId, params, signal, onUpdate, ctx)` declaration is identical in both,
so the fix matches both versions.

## 2026-09-05 run 2026-09-05T08-01-06-892Z

- 8 of 8 lifecycle checks pass.
- No provider credentials: no
- Report: <run>/report.json

| agent | check | status | elapsed ms | asserts | reason |
|---|---|---:|---:|---|---|
| claude | resume | pass | 4440 | The resumed prompt stays in its oboete conversation and context_epoch without repeating its session-start pack. | none |
| claude | compact | pass | 71926 | One compaction advances context_epoch once, re-injects repository memory via SessionStart source=compact, and loses no earlier event. | none |
| claude | fork | pass | 13804 | The fork is a separate conversation whose ledger includes repository memory without changing the parent ledger. | none |
| claude | clear | pass | 11888 | Claude clear injects at SessionStart; Codex /new creates and injects a new root at the first turn's lazy SessionStart source=startup, before UserPromptSubmit, leaving the parent injections unchanged; the parent stays active because /new fires no SessionEnd (run 2026-09-05T07-03-44-495Z). | none |
| codex | resume | pass | 6460 | The resumed prompt stays in its oboete conversation and context_epoch without repeating its session-start pack. | none |
| codex | compact | pass | 31597 | One compaction advances context_epoch once, re-injects repository memory via SessionStart source=compact, and loses no earlier event. | none |
| codex | fork | pass | 18431 | The fork is a separate conversation whose ledger includes repository memory without changing the parent ledger. | none |
| codex | clear | pass | 29511 | Claude clear injects at SessionStart; Codex /new creates and injects a new root at the first turn's lazy SessionStart source=startup, before UserPromptSubmit, leaving the parent injections unchanged; the parent stays active because /new fires no SessionEnd (run 2026-09-05T07-03-44-495Z). | none |


## 2026-09-05 run 2026-09-05T11-10-21-871Z

- 12 of 12 pairs pass
- No provider credentials: no
- Report: <run>/report.json

| seed | receive | status | elapsed ms | missing facts |
|---|---|---:|---:|---|
| claude | codex | pass | 52696 | none |
| claude | grok | pass | 93212 | none |
| claude | pi | pass | 70134 | none |
| codex | claude | pass | 94900 | none |
| codex | grok | pass | 98841 | none |
| codex | pi | pass | 95302 | none |
| grok | claude | pass | 66937 | none |
| grok | codex | pass | 92370 | none |
| grok | pi | pass | 81882 | none |
| pi | claude | pass | 90009 | none |
| pi | codex | pass | 80994 | none |
| pi | grok | pass | 97761 | none |

This run closes SC-001: the six Grok Build pairs that failed on 2026-09-04 with HTTP 402 pass once
the account has balance again (grok 1.0.17 alpha, both as sender with its deferred delivery and as
receiver), and the six Claude Code, Codex and Pi pairs pass as before. Nothing in oboete changed
between the two runs for those legs; the harness itself gained the round-9 simplification pass
and the CodeQL fixes (1f11d087).

## 2026-09-06 MCP clients run 2026-09-06T07-00-54-911Z

- 4 of 4 agents pass
- Report: <run>/report.json

| agent | status | protocolVersion | toolName | frames | reason |
|---|---|---|---|---:|---|
| claude | pass | 2025-11-25 | mcp__oboete_probe__search | 7 | protocolVersion=2025-11-25; notifications/initialized; tools/list search,timeline,get; search memories=0 |
| codex | pass | 2025-06-18 | mcp__oboete_probe__search | 7 | protocolVersion=2025-06-18; notifications/initialized; tools/list search,timeline,get; search memories=2 |
| grok | pass | 2025-11-25 | oboete_probe__search | 7 | protocolVersion=2025-11-25; notifications/initialized; tools/list search,timeline,get; search memories=2 |
| pi | pass | n/a | oboete_search | 0 | oboete_search memories=2 |

- repo -32602: pass (-32602)
- get missing isError: pass (isError: true)
- claude: pass (protocolVersion=2025-11-25; notifications/initialized; tools/list search,timeline,get; search memories=0)
- codex: pass (protocolVersion=2025-06-18; notifications/initialized; tools/list search,timeline,get; search memories=2)
- grok: pass (protocolVersion=2025-11-25; notifications/initialized; tools/list search,timeline,get; search memories=2)
- pi: pass (oboete_search memories=2)

Isolated-user run of `scripts/e2e/mcp-clients.mjs --daily` from `~/oboete`. Each of Claude Code, Codex, and Grok Build listed and called `search` on a second `oboete_probe` registration (tee of `oboete mcp`, raw frames in the run dir); Pi called `oboete_search` and the tool result parsed as `oboete search --json`. Direct stdio rejected `repo` with `-32602` and `get m_missing` with `isError: true`. Probe registrations were removed; the setup `oboete` entries were left in place. Claude's search returned 0 memories because it ran first; Codex/Grok/Pi then saw 2, after that turn was captured.

## 2026-09-06 viewer timing (SC-011) run 2026-09-06T08-16-57Z

- Bundle: 0.1.0-alpha.0 (commit 8fedac9d), Node v24.20.0, isolated account `oboete-dogfood`, repository `github.com/ojungo69/oboete`, database `~/.oboete/memory.db` with 8 memories in scope.
- Method: `/tmp/oboete-viewer-timing.sh` starts `oboete view --port 0`, reads the tokenized URL, opens `/api/events` with the token, drains the change events already queued, then five times inserts a memory row directly into the database and polls `/api/memories` every 25 ms until the row is listed. "Visible" is the time from the insert to the first listing that contains the row; the first `event: change` after the insert is recorded alongside. Each row is deleted before the next insert.

| insert | visible in `/api/memories` | first SSE change |
|---:|---:|---:|
| 1 | 6 ms | 500 ms |
| 2 | 5 ms | 3 ms |
| 3 | 5 ms | 2 ms |
| 4 | 6 ms | 2 ms |
| 5 | 5 ms | 2 ms |

- `oboete view` printed its URL 300 ms after launch; `GET /api/memories` took 34 ms and `GET /api/search?q=busy%20timeout` 7 ms.
- Result: **SC-011 pass**, worst case 6 ms against the 2 s bound. The event stream polls `PRAGMA data_version` every 500 ms, so a change reaches an open browser within one poll interval (the 500 ms on the first insert).
- Observation, not a failure: while a hook-spawned `oboete observe` worker is alive it commits a lease heartbeat about once a second, so the stream reported a change on nearly every poll (12 events in 6 idle seconds on this account, 1 on an idle installation). The browser refetches the list on each event; with a worker alive that is about two 35 ms requests per second for up to twenty minutes after a session. A follow-up may derive the stream's version from the memory tables instead of the connection's `data_version`.

## 2026-09-06 export → import round trip and fixture replay (SC-003) runs 2026-09-06T08-21-16Z and replay-2026-09-06T08-22-16Z

Isolated account `oboete-dogfood`, bundle 0.1.0-alpha.0 (commit 8fedac9d), Node v24.20.0. Installation A is the account's `~/.oboete`; installation B is a fresh `OBOETE_HOME=~/.oboete-b` on the same account. Script: `/tmp/oboete-transfer-run.sh` (kept in the run directory).

### Round trip

- `oboete export` from A wrote 31 memories and 0 tombstones (32 lines, `oboete-export/1`). All 31 rows were `eligible`; no row carried secret text, concepts or sources.
- A's memories belong to one remote repository (`github.com/ojungo69/oboete`, 8 rows) and 22 machine-local (`common_dir`) repositories left behind by finished probe and end-to-end runs. Importing the whole file into B rejected the 23 machine-local rows with `repository <id> is not known here; map it with --map-repo <id>=<local repository id>` and, because a file is applied as a whole, wrote nothing (exit 2). The run then imported the 8 remote-repository rows and, separately, one machine-local repository's 2 rows mapped onto the remote one with `--map-repo`.
- `--dry-run` into B: `8 memories added, 0 raised in sensitivity, 0 tombstones applied, 0 unchanged would be written`, nothing on disk. Import: `8 memories added`. Second import of the same file: `0 memories added … 8 unchanged` (idempotent). Unmapped machine-local file: exit 2. Mapped: `2 memories added`.
- Re-export from B and comparison with A's file by `content_hash`: 10 rows in B (8 + 2 mapped), 0 missing, 0 ids changed for the remote repository (the mapped rows get B's repository identity by design), 0 sensitivities lowered, every active row landed as `review_state = imported`.
- Quarantine: `oboete search exactly --json` in B returned no memory before a worker ran. `OBOETE_HOME=~/.oboete-b oboete observe` reclassified the imported rows (exit 0); afterwards the same search returned the imported session summary and the database held 10 rows as `unreviewed` / `local_only`.
- Result: **SC-003 export/import part pass**.

### Fixture replay on the isolated account

`oboete fixture replay test/fixtures/events-1000.jsonl` from the account's checkout with the installed bundle (`/home/oboete-dogfood/.npm-global/bin/oboete`, 1,552,351 bytes), a temporary `OBOETE_HOME`, no provider credentials, `NODE_ENV=test`. 245 s wall time. Load average at the start `1.85 2.32 1.81`: two Codex review sessions, one Grok Build job and this repository's unit tests were running on the same machine, unlike the quiet-machine T068 measurement.

| Row | This run (isolated account, loaded machine) | docs/evidence/m1-resource-envelope.md T068 (quiet machine) | Bound |
|---|---|---|---|
| SC-003 worker peak RSS (`VmHWM`) | 123,756 kB = 120.9 MB | 113,864 kB = 111.2 MB | < 150 MB, pass |
| SC-003 growth per 1,000 events | 3,558,257 bytes (`memory.db` + `-wal` 221,184 → 3,960,912 bytes) | 3,581,640 bytes | recorded |
| observe runs | 43 spawned by replay, 42 hook-spawned | 43 / 42 | — |
| SC-002 capture p99 | 250.8 ms; 99.9% ≤ 300 ms (n=717) | 181.2 ms; 100.0% (n=717) | p99 ≤ 300 ms and ≥ 99%, pass |
| injection hooks | p99 228.6 ms; 99.8% ≤ 300 ms (n=418); two samples over the bound: `grok/Stop` max 309.4 ms, `claude/UserPromptSubmit` max 311.2 ms | p99 203.1 ms; 100.0% | every sample ≤ 300 ms, **fail on this loaded run** |
| SC-005 secret scan | 0 secret ids in db, wal, spool, logs, packs | 0 | pass |
| SC-010 duplicates | 0 duplicate included groups | 0 | pass |

The SC-003 figures match the quiet-machine measurement within 9 % on memory and 1 % on growth. The injection row's two samples over 300 ms (of 418) appeared only under the concurrent load named above; the T068 row, taken on a quiet machine, holds. A quiet-machine repeat on the isolated account is the open item for this row.

## 2026-09-06 setup timing and doctor break-one-at-a-time (SC-008) run 2026-09-06T08-42-39Z

Isolated account `oboete-dogfood`, bundle 0.1.0-alpha.0 (commit 6f16a7c3 plus the wording commits after it), Node v24.20.0, provider `workers-ai` with the account's credentials in the shell. Script: `/tmp/oboete-doctor-run.sh` (kept in the run directory). Three runs were made today; the first two found product defects that were fixed before this run (see the end of this section).

### Setup timing

`oboete setup --agents claude,codex,grok,pi --provider workers-ai --yes --json` on the already-wired account: exit 0 in **11.9 s** (8.7 s and 9.6 s in the two earlier runs), all four agents `wired: yes`, `probe: pass` (Codex `trust: trusted`). Bound: under 2 minutes. **Pass.**

### Baseline

`oboete doctor --probe-provider`: every item healthy — config, storage, fts, migration, worker, spool, provider (`Provider workers-ai answered with model @cf/zai-org/glm-4.7-flash.`), allowance, agent:claude, agent:codex, agent:grok, agent:pi, pi. The catalog item warns that the catalog lists paid models and native-memory:claude warns that Claude's own memory feature is on; both are informational.

### Break one at a time

Each row: the deliberate break, what doctor said (reason / consequence / recovery, abridged), the recovery step taken, and doctor's item afterwards.

| # | Break | Doctor item | Reason (abridged) | Recovery printed | Step taken | After |
|---|---|---|---|---|---|---|
| 1 | oboete's hook groups removed from `~/.claude/settings.json` | `agent:claude` degraded | `No oboete hook in /home/oboete-dogfood/.claude/settings.json.` | `oboete setup --agents claude` | ran it | healthy |
| 2 | `chmod 0444 memory.db` | `storage` degraded | `The database at …/memory.db is not writable.` | `chmod u+rw …/memory.db` (and the `-wal`/`-shm` files) | `chmod 0600` | healthy |
| 3 | first 100 bytes of `memory.db` overwritten | `storage` degraded, exit **3** | `file is not a database` (now: `The file is not a SQLite database …`) | back up; `oboete export` if readable; move aside; `oboete setup`; `oboete import` | restored the copy | healthy |
| 4 | `worker_lease` row pointed at a dead process with a 60 s old heartbeat | `worker` degraded | `The worker process 424242 holds the lease but its last heartbeat was 60 seconds ago.` | `oboete observe` | started a worker | healthy |
| 5 | HTTPS proxied to a closed port (`NODE_USE_ENV_PROXY=1 https_proxy=http://127.0.0.1:9`) | `provider` degraded | `Provider request failed.` | `Check the network and the host in …/config.toml.` | proxy removed | healthy |
| 6 | `provider_usage.exhausted_at` set for today | `allowance` degraded | `The provider reported exhaustion today.` | `Wait for the reset at 2026-09-07T00:00:00.000Z or switch preset with oboete setup --provider.` | row cleared (reset simulated) | healthy |
| 7 | a 120 s old `.started` file in `spool/pi-ack` | `pi` degraded | `1 Pi capture children never finished (oldest 120 seconds ago), which is pi_child_hang. …` | delete the `.started` files under `spool/pi-ack` and run `oboete observe` | file deleted | **warning** (the worker's `pi_child_hang` count stays visible for 24 h) |
| 8 | node path in `~/.pi/agent/extensions/oboete.js` replaced by `/nonexistent/node` | `agent:pi` degraded | `Pi ran but no capture event reached oboete.` | `oboete setup --agents pi`; if it still fails, run the agent by hand | extension restored | healthy |

Exit codes: 1 for every degraded run, 3 for the corrupted header, 0 for the restored runs. `FAILS=0`, script exit 0. **SC-008 pass**: every deliberately broken component is named with a reason, a consequence and a recovery, and the recovery turns it green (row 7 turns to the documented warning).

### Defects the first two runs exposed (fixed the same day)

- **Grok Build re-serializes `~/.grok/config.toml` and drops comments.** After Grok's 1.0.21 update the `# oboete:begin` / `# oboete:end` markers were gone and the bare `[mcp_servers.oboete]` table remained, so `oboete setup` refused with "would not parse as TOML after the oboete block" and asked for a hand edit. Fixed in the setup writers: a marker-less table that runs oboete's own bundle is recognized (through the marked handler in `hooks/oboete.json`) and replaced or removed; a foreign table is still refused; doctor reports the dropped markers. Assessment: `.specify/bugs/grok-config-rewrite-loses-managed-block/`.
- **The provider probe timed out while `curl` answered in seconds.** The default model `@cf/zai-org/glm-4.7-flash` was answering the one-event probe with 1,600–3,700 completion tokens of reasoning (25–45 s, 60–136 neurons per call), so the worker's 60 s deadline turned e2e summaries into fallbacks and the 10 s probe always failed. The observer now sends `chat_template_kwargs: { enable_thinking: false }`: the same request answers in 1.4 s with 122 tokens (5.7 neurons). The probe deadline is 30 s.
- **The break-3 script corrupted the account's database in run 2** by copying `memory.db` and deleting its WAL while a probe-spawned worker still held the file; `quick_check` later reported "2nd reference to page 134". The doctor recovery text was followed literally (`oboete export` read 61 memories; move aside; `oboete setup`; `oboete import` of the remote repository's 8 rows) and doctor returned to exit 0. The script now kills live workers and checkpoints the WAL before it touches the file.
- Two wording items: probe outcome codes and Pi diagnostics are rendered as sentences; the Pi warning covers the last 24 hours (nothing cleared the rows before, so it was permanent).

## 2026-09-06 no-credentials run 2026-09-06T08-47-25-047Z (SC-004)

- 6 of 6 requested pairs pass (partial run; SC-001 needs all 12)
- No provider credentials: yes
- Report: <run>/report.json

| seed | receive | status | elapsed ms | missing facts |
|---|---|---:|---:|---|
| claude | codex | pass | 28118 | none |
| claude | pi | pass | 24893 | none |
| codex | claude | pass | 30381 | none |
| codex | pi | pass | 31830 | none |
| pi | claude | pass | 22885 | none |
| pi | codex | pass | 32990 | none |


Isolated-user run of `scripts/e2e/isolated-user.mjs --no-credentials --daily --pairs claude:codex,claude:pi,codex:claude,codex:pi,pi:claude,pi:codex` with bundle 0.1.0-alpha.0 (commit 8baf3414): the provider credentials are removed from the agents' environment, so every summary is rule-based. All six pairs recalled the three seeded facts on the first turn, and `report.json` records `degraded_marker: true` for every receiving pack (the `> degraded:` line of contracts/agents.md). Elapsed 23–33 s per pair against 62–93 s on 2026-09-04, which is the provider deadline no longer being waited for.

The six Grok Build pairs ran later the same day (run 2026-09-06T11-19-17-715Z, grok 1.0.21 alpha), once the user released part of the Grok quota for verification:

| seed | receive | status | elapsed ms | missing facts |
|---|---|---:|---:|---|
| claude | grok | pass | 34981 | none |
| codex | grok | pass | 35347 | none |
| grok | claude | pass | 25525 | none |
| grok | codex | pass | 26386 | none |
| grok | pi | pass | 24419 | none |
| pi | grok | pass | 36827 | none |

`report.json` records `degraded_marker: true` for all six receiving packs, so SC-004 holds for all twelve pairs without provider credentials: every fact is recalled and every pack says it is degraded. The exhausted-allowance variant was exercised through doctor's break 6 (`allowance` degraded with the reset time) and the unit tests of the `daily_cap` / `provider_exhausted` degraded reasons (test/unit/degraded.test.ts); an end-to-end pair run with a pre-exhausted counter is still open.

## 2026-09-06 daily run (SC-007, day 1)

- bundle 0.1.0-alpha.0, node v24.20.0, pairs `claude:codex,claude:pi,codex:claude,codex:pi,pi:claude,pi:codex`, 1-minute load at start 0.82, started 2026-09-06T10:00:00Z

### Pairs (harness section, exit 0)

#### 2026-09-06 run 2026-09-06T10-00-00-137Z

- 6 of 6 requested pairs pass (partial run; SC-001 needs all 12)
- No provider credentials: no
- Report: <run>/report.json

| seed | receive | status | elapsed ms | missing facts |
|---|---|---:|---:|---|
| claude | codex | pass | 43282 | none |
| claude | pi | pass | 35852 | none |
| codex | claude | pass | 43490 | none |
| codex | pi | pass | 48333 | none |
| pi | claude | pass | 33946 | none |
| pi | codex | pass | 41787 | none |

### Doctor (credentials sourced)

| item | status | reason |
|---|---|---|
| config | healthy | Configuration at /home/oboete-dogfood/.oboete/config.toml loaded (mode 0o600). |
| paused | healthy | Not paused. |
| storage | healthy | `/home/oboete-dogfood/.oboete/memory.db` opened; PRAGMA quick_check returned ok; 69 memories. |
| fts | healthy | Full-text search is available (lexical in M1). |
| migration | healthy | The schema is at version 3, the latest this bundle knows. |
| worker | healthy | The worker process 305551 is alive (heartbeat 0 seconds ago). |
| spool | healthy | Spool is writable and empty. |
| provider | unverified | Not probed this run; last worker outcome: fallback/no_provider/2026-09-06T09:58:00.101Z. |
| allowance | healthy | Estimated 134 of 150 calls remaining today (2026-09-06); resets at 2026-09-07T00:00:00.000Z. |
| catalog | warning | The catalog lists models that need a paid Workers plan; the configured model @cf/zai-org/glm-4.7-flash is only used if it is free. |
| agent:claude | healthy | The hook fired and the event was stored (5179 milliseconds); trust: n/a. |
| native-memory:claude | warning | claude: its own memory feature (claude_auto_memory) is enabled. oboete neither reads it nor changes it; the two run side by side. |
| agent:codex | healthy | The hook fired and the event was stored (7987 milliseconds); trust: trusted. |
| agent:grok | healthy | The hook fired and the event was stored (4113 milliseconds); trust: wired. |
| agent:pi | healthy | The hook fired and the event was stored (6313 milliseconds); trust: wired. |
| unrecognized-agents | healthy | No invocation from an unrecognized agent. |
| pi | warning | The worker recorded pi_child_hang 2 times, last at 2026-09-06T08:43:51.944Z; the files are gone, this is the history. |

### Metrics

- Provider usage (UTC 2026-09-06): workers-ai 16 calls, 55.7 neurons, @cf/zai-org/glm-4.7-flash
- Memories: 75 total, 75 live, 18 sharing a material hash (duplicates)
- Injection items omitted as duplicate_in_conversation: 0 (cumulative)
- Raw events failed: 0 of 217 (cumulative)
- Spool backlog: 0 files (0 failed)
- Viewer GET /api/memories: median 4 ms, max 37 ms over 5 requests, 8 memories listed (budget 2000 ms)
- finished 2026-09-06T10:04:17Z, 1-minute load 0.96

Notes for day 1: the run is driven by `/etc/cron.d/oboete-dogfood-daily` (hourly tick, one run per JST
calendar day, skipped while the 1-minute load is 3.0 or more; the script lives at
`/usr/local/lib/oboete-dogfood/oboete-daily.sh`). Grok pairs were excluded from this first cron run
for quota; the six Grok legs ran the same day in the no-credentials run above, and from day 2 the
cron runs all 12 pairs. SC-007 was clarified the same evening: M1 is done once day 1 is green, and
the daily run continues as a soak with failures filed as issues. The worker alive during this run
(pid 305551) had been spawned two minutes earlier by a doctor probe without the credential
environment, so its summaries fell back to rules (`provider` shows the last outcome
`fallback/no_provider`); the 16 calls counted for the day come from the earlier runs. The cron
script sources the credentials before it starts, so a worker it spawns carries them.

## 2026-09-07 daily run (SC-007, day 2)

- bundle 0.1.0-alpha.0, node v24.20.0, pairs `all`, 1-minute load at start 0.29, started 2026-09-06T15:05:04Z

### Pairs (harness section, exit 0)

#### 2026-09-06 run 2026-09-06T15-05-04-899Z

- 12 of 12 pairs pass
- No provider credentials: no
- Report: <run>/report.json

| seed | receive | status | elapsed ms | missing facts |
|---|---|---:|---:|---|
| claude | codex | pass | 79200 | none |
| claude | grok | pass | 42937 | none |
| claude | pi | pass | 42501 | none |
| codex | claude | pass | 52282 | none |
| codex | grok | pass | 53802 | none |
| codex | pi | pass | 50555 | none |
| grok | claude | pass | 50670 | none |
| grok | codex | pass | 55621 | none |
| grok | pi | pass | 107086 | none |
| pi | claude | pass | 89452 | none |
| pi | codex | pass | 56658 | none |
| pi | grok | pass | 87474 | none |

### Doctor (credentials sourced)

| item | status | reason |
|---|---|---|
| config | healthy | Configuration at /home/oboete-dogfood/.oboete/config.toml loaded (mode 0o600). |
| paused | healthy | Not paused. |
| storage | healthy | `/home/oboete-dogfood/.oboete/memory.db` opened; PRAGMA quick_check returned ok; 75 memories. |
| fts | healthy | Full-text search is available (lexical in M1). |
| migration | healthy | The schema is at version 3, the latest this bundle knows. |
| worker | healthy | No worker is running; a hook starts one when work is queued. |
| spool | healthy | Spool is writable and empty. |
| provider | unverified | Not probed this run; last worker outcome: fallback/no_provider/2026-09-06T10:04:16.743Z. |
| allowance | healthy | Estimated 134 of 150 calls remaining today (2026-09-06); resets at 2026-09-07T00:00:00.000Z. |
| catalog | warning | The catalog lists models that need a paid Workers plan; the configured model @cf/zai-org/glm-4.7-flash is only used if it is free. |
| agent:claude | healthy | The hook fired and the event was stored (3337 milliseconds); trust: n/a. |
| native-memory:claude | warning | claude: its own memory feature (claude_auto_memory) is enabled. oboete neither reads it nor changes it; the two run side by side. |
| agent:codex | healthy | The hook fired and the event was stored (8445 milliseconds); trust: trusted. |
| agent:grok | healthy | The hook fired and the event was stored (3783 milliseconds); trust: wired. |
| agent:pi | healthy | The hook fired and the event was stored (5085 milliseconds); trust: wired. |
| unrecognized-agents | healthy | No invocation from an unrecognized agent. |
| pi | warning | The worker recorded pi_child_hang 2 times, last at 2026-09-06T08:43:51.944Z; the files are gone, this is the history. |

### Metrics

- Provider usage (UTC 2026-09-06): workers-ai 16 calls, 55.7 neurons, @cf/zai-org/glm-4.7-flash
- Memories: 81 total, 81 live, 20 sharing a material hash (duplicates)
- Injection items omitted as duplicate_in_conversation: 0 (cumulative)
- Raw events failed: 0 of 235 (cumulative)
- Spool backlog: 0 files (0 failed)
- Viewer GET /api/memories: median 4 ms, max 42 ms over 5 requests, 8 memories listed (budget 2000 ms)
- finished 2026-09-06T15:18:05Z, 1-minute load 0.60


## 2026-09-08 daily run (SC-007, day 3)

- bundle 0.1.0-alpha.0, node v24.20.0, pairs `all`, 1-minute load at start 0.08, started 2026-09-07T15:05:08Z

### Pairs (harness section, exit 0)

#### 2026-09-07 run 2026-09-07T15-05-08-149Z

- 12 of 12 pairs pass
- No provider credentials: no
- Report: <run>/report.json

| seed | receive | status | elapsed ms | missing facts |
|---|---|---:|---:|---|
| claude | codex | pass | 94723 | none |
| claude | grok | pass | 49348 | none |
| claude | pi | pass | 49894 | none |
| codex | claude | pass | 46449 | none |
| codex | grok | pass | 58709 | none |
| codex | pi | pass | 52971 | none |
| grok | claude | pass | 56110 | none |
| grok | codex | pass | 44911 | none |
| grok | pi | pass | 44977 | none |
| pi | claude | pass | 64817 | none |
| pi | codex | pass | 43286 | none |
| pi | grok | pass | 99443 | none |

### Doctor (credentials sourced)

| item | status | reason |
|---|---|---|
| config | healthy | Configuration at /home/oboete-dogfood/.oboete/config.toml loaded (mode 0o600). |
| paused | healthy | Not paused. |
| storage | healthy | `/home/oboete-dogfood/.oboete/memory.db` opened; PRAGMA quick_check returned ok; 81 memories. |
| fts | healthy | Full-text search is available (lexical in M1). |
| migration | healthy | The schema is at version 3, the latest this bundle knows. |
| worker | healthy | No worker is running; a hook starts one when work is queued. |
| spool | healthy | Spool is writable and empty. |
| provider | unverified | Not probed this run; last worker outcome: fallback/no_provider/2026-09-06T15:18:04.161Z. |
| allowance | healthy | Estimated 150 of 150 calls remaining today (2026-09-07); resets at 2026-09-08T00:00:00.000Z. |
| catalog | unverified | The cached catalog is stale; the worker refreshes it on the next batch. |
| agent:claude | healthy | The hook fired and the event was stored (3132 milliseconds); trust: n/a. |
| native-memory:claude | warning | claude: its own memory feature (claude_auto_memory) is enabled. oboete neither reads it nor changes it; the two run side by side. |
| agent:codex | healthy | The hook fired and the event was stored (7547 milliseconds); trust: trusted. |
| agent:grok | degraded | Grok rewrote its config.toml and dropped the oboete markers; the MCP table is still there. |
| agent:pi | healthy | The hook fired and the event was stored (4777 milliseconds); trust: wired. |
| unrecognized-agents | healthy | No invocation from an unrecognized agent. |
| pi | healthy | No Pi diagnostics. |

### Metrics

- Provider usage (UTC 2026-09-07): no calls recorded
- Memories: 84 total, 84 live, 21 sharing a material hash (duplicates)
- Injection items omitted as duplicate_in_conversation: 0 (cumulative)
- Raw events failed: 0 of 248 (cumulative)
- Spool backlog: 0 files (0 failed)
- Viewer GET /api/memories: median 3 ms, max 35 ms over 5 requests, 8 memories listed (budget 2000 ms)
- finished 2026-09-07T15:17:02Z, 1-minute load 0.49


- Filed against the release (SC-007): https://github.com/ojungo69/oboete/issues/170

## 2026-09-09 daily run (SC-007, day 4)

- bundle 0.1.0-alpha.0, node v24.20.0, pairs `all`, 1-minute load at start 0.07, started 2026-09-08T15:05:06Z

### Pairs (harness section, exit 1)

#### 2026-09-08 run 2026-09-08T15-05-06-951Z

- 7 of 12 pairs pass
- No provider credentials: no
- Report: <run>/report.json

| seed | receive | status | elapsed ms | missing facts |
|---|---|---:|---:|---|
| claude | codex | pass | 68267 | none |
| claude | grok | pass | 63015 | none |
| claude | pi | pass | 50333 | none |
| codex | claude | pass | 58500 | none |
| codex | grok | fail | 52253 | fact-2026-09-08T15-05-06-951Z-codex-to-grok-1: the build token is cedar.; fact-2026-09-08T15-05-06-951Z-codex-to-grok-2: the release bird is heron.; fact-2026-09-08T15-05-06-951Z-codex-to-grok-3: 配布色は琥珀。 |
| codex | pi | pass | 56984 | none |
| grok | claude | fail | 512 | fact-2026-09-08T15-05-06-951Z-grok-to-claude-1: the build token is cedar.; fact-2026-09-08T15-05-06-951Z-grok-to-claude-2: the release bird is heron.; fact-2026-09-08T15-05-06-951Z-grok-to-claude-3: 配布色は琥珀。 |
| grok | codex | fail | 502 | fact-2026-09-08T15-05-06-951Z-grok-to-codex-1: the build token is cedar.; fact-2026-09-08T15-05-06-951Z-grok-to-codex-2: the release bird is heron.; fact-2026-09-08T15-05-06-951Z-grok-to-codex-3: 配布色は琥珀。 |
| grok | pi | fail | 532 | fact-2026-09-08T15-05-06-951Z-grok-to-pi-1: the build token is cedar.; fact-2026-09-08T15-05-06-951Z-grok-to-pi-2: the release bird is heron.; fact-2026-09-08T15-05-06-951Z-grok-to-pi-3: 配布色は琥珀。 |
| pi | claude | pass | 42979 | none |
| pi | codex | pass | 49026 | none |
| pi | grok | fail | 32707 | fact-2026-09-08T15-05-06-951Z-pi-to-grok-1: the build token is cedar.; fact-2026-09-08T15-05-06-951Z-pi-to-grok-2: the release bird is heron.; fact-2026-09-08T15-05-06-951Z-pi-to-grok-3: 配布色は琥珀。 |

### Doctor (credentials sourced)

| item | status | reason |
|---|---|---|
| config | healthy | Configuration at /home/oboete-dogfood/.oboete/config.toml loaded (mode 0o600). |
| paused | healthy | Not paused. |
| storage | healthy | `/home/oboete-dogfood/.oboete/memory.db` opened; PRAGMA quick_check returned ok; 89 memories. |
| fts | healthy | Full-text search is available (lexical in M1). |
| migration | healthy | The schema is at version 3, the latest this bundle knows. |
| worker | healthy | No worker is running; a hook starts one when work is queued. |
| spool | healthy | Spool is writable and empty. |
| provider | unverified | Not probed this run; last worker outcome: fallback/no_provider/2026-09-08T04:45:45.502Z. |
| allowance | healthy | Estimated 150 of 150 calls remaining today (2026-09-08); resets at 2026-09-09T00:00:00.000Z. |
| catalog | unverified | The cached catalog is stale; the worker refreshes it on the next batch. |
| agent:claude | healthy | The hook fired and the event was stored (5566 milliseconds); trust: n/a. |
| native-memory:claude | warning | claude: its own memory feature (claude_auto_memory) is enabled. oboete neither reads it nor changes it; the two run side by side. |
| agent:codex | healthy | The hook fired and the event was stored (8190 milliseconds); trust: trusted. |
| agent:grok | degraded | Grok rewrote its config.toml and dropped the oboete markers; the MCP table is still there. |
| agent:pi | healthy | The hook fired and the event was stored (4968 milliseconds); trust: wired. |
| unrecognized-agents | healthy | No invocation from an unrecognized agent. |
| pi | healthy | No Pi diagnostics. |

### Metrics

- Provider usage (UTC 2026-09-08): no calls recorded
- Memories: 92 total, 92 live, 23 sharing a material hash (duplicates)
- Injection items omitted as duplicate_in_conversation: 1 (cumulative)
- Raw events failed: 0 of 275 (cumulative)
- Spool backlog: 0 files (0 failed)
- Viewer GET /api/memories: median 4 ms, max 37 ms over 5 requests, 11 memories listed (budget 2000 ms)
- finished 2026-09-08T15:13:11Z, 1-minute load 0.11

### Why the five Grok legs failed

Every failing leg reports `Not signed in.`, including `<run>/grok-to-claude/seed/stdout.txt` and
`codex-to-grok/receive`.

The pairs run alphabetically: `claude → grok`, the second pair, passed in 63 s with the fact recalled.
From `codex → grok` on, every Grok leg failed; the three Grok seeding legs ended in about 500 ms. This
shows that Grok could authenticate at the start of the run and could not later in it.

`prepareGrokAgent` (`scripts/e2e/isolated-user.mjs:928`) copies the account's `~/.grok/auth.json`
into each pair configuration and points `GROK_HOME` at that copy. That is the copied-credential hazard
tracked in issue #175, but this run does not identify a particular refresh or prove token retirement.
`prepareCodexAgent` and `preparePiAgent` also copy `auth.json`, so their exposure is part of that
follow-up.

`prepareClaudeAgent` copies `settings.json` alone and passes it with `--settings`; it does not set a
per-leg credential configuration variable, so Claude inherits its credential configuration. This run
records no Claude authentication failure, not a general guarantee that Claude cannot fail this way.

The T033 candidate verification at `2026-09-08T05-23-31` passed 12 of 12 pairs; it is separate from
the day-3 daily run `2026-09-07T15-05-08-149Z`. Neither result identifies whether a token refresh
occurred. SC-007 records the observed Grok CLI authentication failure, while issue #175 follows the
copied-credential hypothesis and its remediation.


- Filed against the release (SC-007): https://github.com/ojungo69/oboete/issues/180

## 2026-09-10 daily run (SC-007, day 5)

- bundle 0.1.0-alpha.0, node v24.20.0, pairs `all`, 1-minute load at start 0.49, started 2026-09-09T15:05:05Z

### Pairs (harness section, exit 1)

#### 2026-09-09 run 2026-09-09T15-05-05-088Z

- 6 of 12 pairs pass
- No provider credentials: no
- Report: <run>/report.json

| seed | receive | status | elapsed ms | missing facts |
|---|---|---:|---:|---|
| claude | codex | pass | 71512 | none |
| claude | grok | fail | 89832 | fact-2026-09-09T15-05-05-088Z-claude-to-grok-1: the build token is cedar.; fact-2026-09-09T15-05-05-088Z-claude-to-grok-2: the release bird is heron.; fact-2026-09-09T15-05-05-088Z-claude-to-grok-3: 配布色は琥珀。 |
| claude | pi | pass | 37475 | none |
| codex | claude | pass | 64078 | none |
| codex | grok | fail | 42906 | fact-2026-09-09T15-05-05-088Z-codex-to-grok-1: the build token is cedar.; fact-2026-09-09T15-05-05-088Z-codex-to-grok-2: the release bird is heron.; fact-2026-09-09T15-05-05-088Z-codex-to-grok-3: 配布色は琥珀。 |
| codex | pi | pass | 55843 | none |
| grok | claude | fail | 1970 | fact-2026-09-09T15-05-05-088Z-grok-to-claude-1: the build token is cedar.; fact-2026-09-09T15-05-05-088Z-grok-to-claude-2: the release bird is heron.; fact-2026-09-09T15-05-05-088Z-grok-to-claude-3: 配布色は琥珀。 |
| grok | codex | fail | 1780 | fact-2026-09-09T15-05-05-088Z-grok-to-codex-1: the build token is cedar.; fact-2026-09-09T15-05-05-088Z-grok-to-codex-2: the release bird is heron.; fact-2026-09-09T15-05-05-088Z-grok-to-codex-3: 配布色は琥珀。 |
| grok | pi | fail | 1966 | fact-2026-09-09T15-05-05-088Z-grok-to-pi-1: the build token is cedar.; fact-2026-09-09T15-05-05-088Z-grok-to-pi-2: the release bird is heron.; fact-2026-09-09T15-05-05-088Z-grok-to-pi-3: 配布色は琥珀。 |
| pi | claude | pass | 41785 | none |
| pi | codex | pass | 58377 | none |
| pi | grok | fail | 35844 | fact-2026-09-09T15-05-05-088Z-pi-to-grok-1: the build token is cedar.; fact-2026-09-09T15-05-05-088Z-pi-to-grok-2: the release bird is heron.; fact-2026-09-09T15-05-05-088Z-pi-to-grok-3: 配布色は琥珀。 |

### Doctor (credentials sourced)

| item | status | reason |
|---|---|---|
| config | healthy | Configuration at /home/oboete-dogfood/.oboete/config.toml loaded (mode 0o600). |
| paused | healthy | Not paused. |
| storage | healthy | `/home/oboete-dogfood/.oboete/memory.db` opened; PRAGMA quick_check returned ok; 96 memories. |
| fts | healthy | Full-text search is available (lexical in M1). |
| migration | healthy | The schema is at version 3, the latest this bundle knows. |
| worker | healthy | No worker is running; a hook starts one when work is queued. |
| spool | healthy | Spool is writable and empty. |
| provider | unverified | Not probed this run; last worker outcome: fallback/no_provider/2026-09-09T06:47:34.422Z. |
| allowance | healthy | Estimated 150 of 150 calls remaining today (2026-09-09); resets at 2026-09-10T00:00:00.000Z. |
| catalog | unverified | The cached catalog is stale; the worker refreshes it on the next batch. |
| agent:claude | healthy | The hook fired and the event was stored (5551 milliseconds); trust: n/a. |
| native-memory:claude | warning | claude: its own memory feature (claude_auto_memory) is enabled. oboete neither reads it nor changes it; the two run side by side. |
| agent:codex | healthy | The hook fired and the event was stored (7585 milliseconds); trust: trusted. |
| agent:grok | degraded | Grok rewrote its config.toml and dropped the oboete markers; the MCP table is still there. |
| agent:pi | healthy | The hook fired and the event was stored (4783 milliseconds); trust: wired. |
| unrecognized-agents | healthy | No invocation from an unrecognized agent. |
| pi | healthy | No Pi diagnostics. |

### Metrics

- Provider usage (UTC 2026-09-09): no calls recorded
- Memories: 99 total, 99 live, 25 sharing a material hash (duplicates)
- Injection items omitted as duplicate_in_conversation: 3 (cumulative)
- Raw events failed: 0 of 309 (cumulative)
- Spool backlog: 0 files (0 failed)
- Viewer GET /api/memories: median 4 ms, max 37 ms over 5 requests, 13 memories listed (budget 2000 ms)
- finished 2026-09-09T15:13:36Z, 1-minute load 0.74


- Filed against the release (SC-007): https://github.com/ojungo69/oboete/issues/187

## 2026-09-11 daily run (SC-007, day 6)

- bundle 0.1.0-alpha.0, node v24.20.0, pairs `all`, 1-minute load at start 2.88, started 2026-09-10T16:05:04Z

### Pairs (harness section, exit 1)

#### 2026-09-10 run 2026-09-10T16-05-04-950Z

- 6 of 12 pairs pass
- No provider credentials: no
- Report: <run>/report.json

| seed | receive | status | elapsed ms | missing facts |
|---|---|---:|---:|---|
| claude | codex | pass | 65959 | none |
| claude | grok | fail | 34049 | fact-2026-09-10T16-05-04-950Z-claude-to-grok-1: the build token is cedar.; fact-2026-09-10T16-05-04-950Z-claude-to-grok-2: the release bird is heron.; fact-2026-09-10T16-05-04-950Z-claude-to-grok-3: 配布色は琥珀。 |
| claude | pi | pass | 46118 | none |
| codex | claude | pass | 55925 | none |
| codex | grok | fail | 37397 | fact-2026-09-10T16-05-04-950Z-codex-to-grok-1: the build token is cedar.; fact-2026-09-10T16-05-04-950Z-codex-to-grok-2: the release bird is heron.; fact-2026-09-10T16-05-04-950Z-codex-to-grok-3: 配布色は琥珀。 |
| codex | pi | pass | 42851 | none |
| grok | claude | fail | 543 | fact-2026-09-10T16-05-04-950Z-grok-to-claude-1: the build token is cedar.; fact-2026-09-10T16-05-04-950Z-grok-to-claude-2: the release bird is heron.; fact-2026-09-10T16-05-04-950Z-grok-to-claude-3: 配布色は琥珀。 |
| grok | codex | fail | 525 | fact-2026-09-10T16-05-04-950Z-grok-to-codex-1: the build token is cedar.; fact-2026-09-10T16-05-04-950Z-grok-to-codex-2: the release bird is heron.; fact-2026-09-10T16-05-04-950Z-grok-to-codex-3: 配布色は琥珀。 |
| grok | pi | fail | 531 | fact-2026-09-10T16-05-04-950Z-grok-to-pi-1: the build token is cedar.; fact-2026-09-10T16-05-04-950Z-grok-to-pi-2: the release bird is heron.; fact-2026-09-10T16-05-04-950Z-grok-to-pi-3: 配布色は琥珀。 |
| pi | claude | pass | 36913 | none |
| pi | codex | pass | 63806 | none |
| pi | grok | fail | 36723 | fact-2026-09-10T16-05-04-950Z-pi-to-grok-1: the build token is cedar.; fact-2026-09-10T16-05-04-950Z-pi-to-grok-2: the release bird is heron.; fact-2026-09-10T16-05-04-950Z-pi-to-grok-3: 配布色は琥珀。 |

### Doctor (credentials sourced)

| item | status | reason |
|---|---|---|
| config | healthy | Configuration at /home/oboete-dogfood/.oboete/config.toml loaded (mode 0o600). |
| paused | healthy | Not paused. |
| storage | healthy | `/home/oboete-dogfood/.oboete/memory.db` opened; PRAGMA quick_check returned ok; 99 memories. |
| fts | healthy | Full-text search is available (lexical in M1). |
| migration | healthy | The schema is at version 3, the latest this bundle knows. |
| worker | healthy | No worker is running; a hook starts one when work is queued. |
| spool | healthy | Spool is writable and empty. |
| provider | unverified | Not probed this run; last worker outcome: fallback/no_provider/2026-09-09T15:13:36.028Z. |
| allowance | healthy | Estimated 150 of 150 calls remaining today (2026-09-10); resets at 2026-09-11T00:00:00.000Z. |
| catalog | unverified | The cached catalog is stale; the worker refreshes it on the next batch. |
| agent:claude | healthy | The hook fired and the event was stored (5406 milliseconds); trust: n/a. |
| native-memory:claude | warning | claude: its own memory feature (claude_auto_memory) is enabled. oboete neither reads it nor changes it; the two run side by side. |
| agent:codex | healthy | The hook fired and the event was stored (5654 milliseconds); trust: trusted. |
| agent:grok | degraded | Grok rewrote its config.toml and dropped the oboete markers; the MCP table is still there. |
| agent:pi | healthy | The hook fired and the event was stored (4848 milliseconds); trust: wired. |
| unrecognized-agents | healthy | No invocation from an unrecognized agent. |
| pi | healthy | No Pi diagnostics. |

### Metrics

- Provider usage (UTC 2026-09-10): no calls recorded
- Memories: 103 total, 103 live, 26 sharing a material hash (duplicates)
- Injection items omitted as duplicate_in_conversation: 3 (cumulative)
- Raw events failed: 0 of 322 (cumulative)
- Spool backlog: 0 files (0 failed)
- Viewer GET /api/memories: median 5 ms, max 49 ms over 5 requests, 13 memories listed (budget 2000 ms)
- finished 2026-09-10T16:12:12Z, 1-minute load 2.37


- Filed against the release (SC-007): https://github.com/ojungo69/oboete/issues/189
