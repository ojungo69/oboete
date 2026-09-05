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
