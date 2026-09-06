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
