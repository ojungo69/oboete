# claude-mem and oboete, feature by feature (2026-10-03)

Owner decision 36: finish oboete as a complete superset of claude-mem. This table defines that
comparison; decision 37 sets the scope based on it. Decision 50: claude-mem comes off only once this
table, brought up to date against claude-mem's current release, shows every difference closed or
accepted by the owner.

## Status against claude-mem 13.40.0 (2026-10-17), new features to 14.0.1 (2026-10-20)

### How it was checked

Every row of this table was read again against claude-mem 13.40.0 source and against oboete `main` at 576cb79. The rows that #557 to #564 changed were brought up to date by those PRs, and the counts, the open checks and rows 5 and X2 on 2026-10-17 with `main` at ec084a1. Row 8 and the counts were brought up to date on 2026-10-20 by #566 and decision 52, row N5 by #568, and rows 5 and 31 on 2026-10-21. The 13.41 and 14.0.1 changelogs and source were read on 2026-10-20: My Work got row X10 (decision 53); TeamBrain, a paid team service, and 13.41's work-state telemetry (row 57) add none. The 13.29 to 13.40 changelog was read for new features. A feature that extends a row was merged into that row. A feature with no row got a new id (`G01`, `X1` to `X10`). A second reader re-checked each status and tried to refute it. Where that reader corrected the first reading, the corrected status is used. Where two readers disagreed, the table says so in the row. Rows were checked on the owner's single WSL PC. Only counts of the owner's data are given here. Decision 50 (2026-10-17) is the rule: claude-mem comes off only once oboete stands in for its functions almost completely. So an accepted difference that is worse still counts against removal.

Impact means how often the owner would notice the difference: `daily`, `weekly`, `rare` or `none`. `blocker` means a one-time step that removal depends on.

### Counts

| Status | Rows |
|---|---|
| equal_or_better | 26 |
| partial | 26 |
| missing | 19 |
| accepted_difference | 7 |
| total | 78 |

### Open checks before removal

- claude-mem's history up to the switch is in oboete: 253,413 imported documents of the owner's claude-mem database (its import ops in raw.db, the documents in knowledge.db's `imported`), the newest from 2026-10-15. What claude-mem recorded after that is not; oboete has recorded those sessions itself since the switch.
- `oboete import claude-mem` reads the live claude-mem database, at schema v67 since 2026-10-16, again since #556 (row N1).
- The vectors have caught up: 16 raw records and no imported document waited on 2026-10-20 (doctor, 16:52Z), where 1,042 imported documents and 32,406 raw records waited on 2026-10-17 (row 5).
- claude-mem's exclusion patterns are in oboete since 2026-10-17 (row 14): all 9 of the owner's, none of which names a folder oboete has recorded.

### Table

| Id | Difference | Status | Impact | Evidence (claude-mem / oboete) |
|---|---|---|---|---|
| 1 | Both keep a worker up and serve a page at a fixed address. oboete's page needs a token, which the bookmark carries. A fixed idle exit replaces claude-mem's opt-in `IDLE_EXIT_SEC`. | equal_or_better | none | SettingsDefaultsManager.ts:432 / src/view.rs:741 |
| 2 | oboete has no stop, restart or status command. It does not replace a hung worker that holds the lock. A hung worker stops curation and the page with no notice. claude-mem reclaims a wedged worker using a heartbeat. | partial | rare | worker-service.ts:1533 / docs/resident.md:342 |
| 3 | claude-mem deletes a card or a session from the page. oboete forgets from the CLI only, and its forget is broader (preview, repo or time range, backups purged). The page has no forget route. | partial | rare | DataRoutes.ts:212 / src/view.rs:1100 |
| 4 | The settings page does more than claude-mem's single-provider dialog. It covers the provider chain, connection test, write-only keys, the spending cap, embeddings, redaction, exclusions, retention, agents and a first-run guide. A stale save is refused with 409. Nobody has re-checked how easy it is on the real screen since #512 and #536. | equal_or_better | none | ContextSettingsModal.tsx:358 / src/view.rs:1491 |
| 5 | Meaning search is on, with local bge-m3, over claims, cards, session summaries, imported history and raw records; its queue was down to 16 raw records and no imported document on 2026-10-20 (doctor, 16:52Z). oboete applies no 90-day window to a search by meaning without dates, which claude-mem applies by default (`CLAUDE_MEM_SEARCH_RECENCY_DAYS`, 90 days, 0 for none, the owner's at the default): milestone 1 measured it on the owner's questions, claude-mem's nDCG@10 0.219 with its window and 0.282 without, the window costing most on old prompts, so oboete does better without it; a search names its dates when it wants a window. The model a prompt's query needs is row 31's. | equal_or_better | none | SearchOrchestrator.ts:137 / docs/milestone-1.md:359 |
| 6 | The legacy search has no `offset` and no `platformSource` (per-agent) filter. It has no file or concept filter either. `get` and `timeline` have no total byte cap, while claude-mem caps every tool reply at 32 KiB. Bad dates are refused. | partial | rare | mcp-server.ts:613 / src/mcp.rs:36 |
| 7 | Trigram OR ranking plus a short-word boost. All-words matching is left out by decision 37. Measured nDCG@10 is 0.545 against 0.244. | accepted_difference | none | SessionSearch.ts:575 / src/search.rs:92 |
| 8 | The file note is built for Claude Code, Codex and Grok, with the same rules. Since 2026-10-17 the claude-mem history oboete imported joins the cards in it (docs/file-note.md F10 to F15): an observation's files come in as a files op beside its document, and `oboete import claude-mem --files` brought those of the 143,931 observations imported before on 2026-10-20. Since decision 52 (2026-10-20) an imported observation maps to a checkout by claude-mem's names for it (the top level's folder name, a worktree's with its main repository's), as claude-mem maps, besides its session (#506). Differences: the names come from the hook's `cwd`, not `CLAUDE_PROJECT_DIR`; claude-mem's optional naming modes are not read; the names come from the `.git` files, not git's discovery; a merged row is matched by the project it was merged into only, so a checkout named after the project a merge took it from does not get it; a merge claude-mem made after the last plain import (358 of the 9,555 observations filed under a worktree's name) is followed by the next plain import. On this PC's store the read hook of the ten most named files takes 20 to 32 ms at the median, claude-mem's 3,229 ms. | equal_or_better | none | file-context.ts:461 / src/cards.rs:651 |
| 9 | Session start shows one line (counts and the command to open the page). claude-mem shows the timeline and a link. Decision 37 chose the one line, and the note holds no stored text by design. Since decision 51 the terminal shows the memory list above the line, switchable back to the line alone. | equal_or_better | none | context.ts:337 / src/hook.rs:971 |
| 10 | The first-visit welcome dialog is built. It has no "How it works" link and no docs link. | equal_or_better | none | WelcomeCard.tsx:7 / assets/viewer/app.js:2077 |
| 11 | The stopped-curation line names a class of cause (owner, budget, no answer), not the provider's words or a remedy link. A refused key shows only after 30 minutes. The terminal note appears on Claude Code only, and there is no relay instruction for Codex or Grok. | partial | rare | observer-health.ts:351 / src/curate.rs:3100 |
| 12 | Hooks write raw.db themselves and always exit 0. A failed write is kept under `unwritten/` and written back later. A busy database is waited out. claude-mem uses a hook spool for the same purpose. | equal_or_better | none | hook-spool.ts:10 / src/unwritten.rs:1 |
| 13 | Money spent is shown. The session-start "N obs, tokens read, work, % savings" line is missing. | partial | rare | AgentFormatter.ts:46 / assets/viewer/app.js:1036 |
| 14 | Since 2026-10-17 oboete also excludes by a folder name or claude-mem pattern (`*`, `**`, `?`, a leading `~`), read as claude-mem reads one, and a name with no `/` also covers the folders inside one so named; a pattern that does not start at a root also names the end of a path (`secret/**` names `/work/secret` and the folders in it), where claude-mem tries it against the whole path and the last part alone, so such a pattern with a `/` names no folder there; `oboete import claude-mem` brings claude-mem's patterns over once (docs/settings-privacy.md, docs/claude-mem-import.md I8). | equal_or_better | none | project-filter.ts:68 / src/capture.rs:203 |
| 15 | Private prompts behave the same. oboete strips 4 block names. claude-mem strips 6, and also removes `system-reminder` and `persisted-output` from tool fields. One edge of nested tags differs. | partial | rare | tag-stripping.ts:5 / src/hook.rs:32 |
| 16 | The page has no sessions list, no per-session page and no category chips. Feed items carry no session id. | missing | rare (unmeasured) | SessionDetailPage.tsx:40 / src/view.rs:2078 |
| 17 | Summaries have the same six fields and can be searched. A summary appears after the turn's windows are curated. Since 2026-10-17 a window whose sessions have all replied goes at once (#557, docs/curation-timing.md), so the summary comes about one curator call after the reply, where it could wait about 10 minutes before; a turn that ends with no reply still waits the idle time. claude-mem writes it at Stop. Since 2026-10-17 a Stop whose message is empty (Claude Code's when a session ends in a tool call) takes the transcript's last assistant text, as claude-mem does. | partial | rare | summarize.ts:103 / src/curate.rs:1471 |
| 18 | oboete has no tool, Bash-pattern or agent-type skip settings. The only switch is the coarse `OBOETE_SKIP` (it also skips reads), and it is not documented for users. The owner uses only claude-mem's default skip list, which covers about 0.5% of tool rows. | partial | rare | SettingsDefaultsManager.ts:433 / src/hook.rs:22 |
| 19 | Counts are fixed at claude-mem's defaults (50 cards, 10 summaries). oboete has no type, concept, field or full-count settings, and no switch to turn work state off. A Context page preview exists. | partial | none | ContextConfigLoader.ts:52 / src/consumer/manifest.rs:56 |
| 20 | A tool output over about 17.8k characters reaches the curator only as its size. 262 of 1,471 windows (18%) had one. claude-mem condenses such an output with a model call or keeps its head and tail. oboete's `shrink` is off by default. Decision 37 left this out. Since 2026-10-17 the curator sees such an output's head and tail, up to 8,000 characters each as the window holds; claude-mem's model condense is not built. | partial | rare | field-optimizer.ts:257 / src/curate.rs:498 |
| 21 | oboete cannot join two repository keys: there is no merge and no `repo alias`, although the spec mentions one. Keys come from the origin URL, so worktrees already share one. | missing | rare | ProjectMerge.ts:21 / src/repo.rs:1 |
| 22 | No team server and no TeamBrain. Decision 37. | accepted_difference | none | mcp-server.ts:765 / docs/spec.md:64 |
| 23 | oboete ships no slash-command skills: no timeline-report, weekly-digests, how-it-works or mem-search skill. The MCP instructions teach mem_search. | partial | rare | plugin/skills/ / src/mcp.rs:636 |
| 24 | No smart_search, smart_outline or smart_unfold (decision 37), and no File Read Gate (decision 40). The gate is on in the owner's claude-mem, and no denial was found in recent transcripts. | accepted_difference | rare | file-context.ts:294 / docs/spec.md:67 |
| 25 | The page serves on 127.0.0.1 only. There is no allowed-origins or public-URL setting. Decision 37. | accepted_difference | none | SettingsDefaultsManager.ts:415 / src/view.rs:1593 |
| 26 | No full-screen TV page. Decision 37. | accepted_difference | none | ViewerRoutes.ts:170 / docs/spec.md:64 |
| 27 | "Never start a worker" exists only as the undocumented `OBOETE_NO_SPAWN` variable. | partial | none | worker-autostart.ts:10 / src/hook.rs:3310 |
| 28 | An empty repository gets a "recording is on, no memory yet" line, and the line is recomputed every time. | equal_or_better | none | SearchRoutes.ts:86 / src/hook.rs:981 |
| 29 | An image-only prompt is dropped instead of stored as a placeholder. A prompt delivered twice is stored twice. Prompts are not numbered. No native prompt id is kept. | partial | rare | SessionRoutes.ts:868 / src/capture.rs:366 |
| 30 | No ranking by re-confirmation. claude-mem's version is off by default and off for the owner. | missing | none | rank.ts:1 / src/cards.rs:432 |
| 31 | Per-prompt hints are on and rank better than claude-mem's. Until 2026-10-21 they reached no prompt: from 2026-10-13 to 2026-10-20 none of the 779 additional contexts of Claude Code's prompt hooks on this PC was oboete's block, where 31 were claude-mem's. On a copy of the store read cold, as WSL's page cache can be after a pause (inferred, not measured on the owner's prompts), the resident viewer's answer took 3.7 to 3.9 s against the hook's 1 s wait, 2.85 s of it decision 52's sessions query. Since 2026-10-21 that query looks for a document's newest row once a session (69 ms cold instead of 3.0 s), the wait is 2 s by default, and on a copy of the store, cold, the prompt hook took 1.3 s with five lines in its block. Each card shows its whole narrative, as claude-mem's block does, within 2,500 characters (docs/prompt-context.md). What is left: the resident viewer drops the model after 30 quiet minutes, and a prompt that comes then gets the words side only while the model loads (about 2 s); decision 44(c) covers only the "nothing passes, inject nothing" part. | partial | weekly | SearchRoutes.ts:569 / src/search/b.rs:671 |
| 32 | No `session_start_context` tool. `timeline` with no arguments partly stands in for it. | partial | rare | mcp-server.ts:437 / src/mcp.rs:1687 |
| 33 | No capture for Kimi, Oh My Pi, Windsurf, Copilot, Goose, Roo, Warp, T3 Code, DeepSeek Harness or Qoder. The owner's DB has 36 Oh My Pi sessions, the last on 2026-09-16. | missing | rare | ide-detection.ts:51 / src/setup.rs:20 |
| 34 | Cursor, OpenCode, Antigravity and Pi are wired and unit-tested but never checked live. The OpenCode plugin is V2-only. Pi has no mem_search. | partial | rare | CursorHooksInstaller.ts:264 / src/setup.rs:85 |
| 35 | No local HTTP ingest or query API and no bot adapter (#344, #345). Decision 33 defers them. | missing | none | SessionRoutes.ts:572 / docs/spec-client-runtime.md:98 |
| 36 | No live transcript watcher and no user-written watch config. Codex's internal helper prompts are not filtered. The owner's claude-mem watches are idle: the state file was last written in May and the bot folder is empty. | missing | rare | transcripts/cli.ts:33 / src/main.rs:343 |
| 37 | No sync between machines yet (milestone 6). The owner's claude-mem has no sync configured. | missing | none | CloudSync.ts:1 / src/worker.rs:205 |
| 38 | No copy of memory over SSH. | missing | none | scripts/claude-mem-sync:2 / docs/spec.md:35 |
| 39 | No Cowork or Claude Desktop support. | missing | none | cowork/README.md:1 / src/mcp.rs:1 |
| 40 | No knowledge corpora or knowledge agent. The owner's corpora folder is empty. | missing | none | mcp-server.ts:1045 / src/mcp.rs:224 |
| 41 | One fixed coding set, equal to claude-mem's `code` mode. No chill mode, no other domains, no custom modes. | partial | none | ModeManager.ts:25 / docs/cards.md:240 |
| 42 | No Telegram alerts or wrap-ups. The owner has not configured them. | missing | none | TelegramNotifier.ts:44 / docs/claude-mem-parity.md:151 |
| 43 | No per-folder CLAUDE.md timeline. It is off for the owner. | missing | none | ResponseProcessor.ts:1107 / src/hook.rs:629 |
| 44 | No import of the agents' own memory-folder notes, no note-folder watcher, and no auto-memory switch. The owner uses none of them. | missing | rare | ingest.ts:1 / src/main.rs:343 |
| 45 | No selective export or import (#346). | missing | none | scripts/export-memories.ts:1 / docs/spec-client-runtime.md:133 |
| 46 | No near-duplicate merge or review. claude-mem's version is off by default and off for the owner. | missing | none | DedupRoutes.ts:38 / src/claims.rs:99 |
| 47 | oboete has a multi-provider chain with fallback and stored cooldowns. It has no key rotation, no named presets, no parallel calls, no separate summary model and no `<think>` stripping. It does not retry without a rejected parameter, does not stream, and sends no cache labels. Prompt caching is not on for the owner's Anthropic entry. | partial | rare | api-key-pool.ts:3 / src/provider.rs:1381 |
| 48 | No per-attempt log of what a curator tried to do. The codex curator still receives `~/.codex/AGENTS.md`. | partial | rare | observer-audit.ts:1 / src/provider.rs:2403 |
| 49 | The page polls every 3 s instead of being pushed updates. | partial | none | useSSE.ts:31 / assets/viewer/app.js:5865 |
| 50 | Uninstall does not stop the worker and says nothing about the data left behind. | partial | rare | uninstall.ts:222 / src/setup.rs:194 |
| 51 | No README, no `oboete update` and no release pipeline. The installer does not preselect detected agents. A JSON config that starts with a BOM stops setup safely. | missing | rare | npx-cli/index.ts:141 / src/main.rs:70 |
| 52 | No log files and no log view on the page. Doctor, the provider-call table and the last outcome stand in for them. | partial | rare | LogsRoutes.ts:73 / src/providers_db.rs:22 |
| 53 | No status-line counter. The owner's status line does not use claude-mem's. | missing | none | statusline-counts.js:1 / src/main.rs:78 |
| 54 | Cards arrive a window at a time and can be up to about 10 minutes late. All agents share them by design (decision 39). A resume injects nothing on either side. | accepted_difference | weekly | ClaudeProvider.ts:1315 / src/curate.rs:21 |
| 55 | No Docker trial image. oboete uses the dogfood user or a scratch home instead. | missing | none | docker/claude-mem/Dockerfile:1 / src/main.rs:69 |
| 56 | No maker account and no hosted summariser. Decision 37. | accepted_difference | none | cmem-gateway.ts:1 / docs/spec.md:64 |
| 57 | oboete sends no telemetry. claude-mem's telemetry is on for the owner. | equal_or_better | none | telemetry/consent.ts:43 / docs/spec.md:1135 |
| 58 | Records are stored before any model runs. A failed window passes through the provider chain, is held, and can be recurated. Spent credit pauses that provider and nothing is lost. | equal_or_better | none | SessionMessageBuffer.ts:23 / src/curate.rs:1552 |
| 59 | Worktrees share the repository key from the first record, so nothing needs adopting. Cards from an unmerged branch show at once. | equal_or_better | rare | WorktreeAdoption.ts:1 / src/repo.rs:27 |
| 60 | Masking is on by default with 222 gitleaks rules plus generic credential rules. An off switch exists. claude-mem's masking is off for the owner. | equal_or_better | none | redaction.ts:75 / src/redact.rs:203 |
| N1 | Since #556 the import reads claude-mem's schemas up to v67 (13.40.0); v66 and v67 add only sync columns. The history up to the switch is imported. | equal_or_better | none | sqlite/connection.ts:19 / src/import.rs:68 |
| N2 | A newer home is refused before any write. One stamp guards raw, knowledge and providers, and it is raised inside an IMMEDIATE transaction. | equal_or_better | none | sqlite/connection.ts:46 / src/raw.rs:1556 |
| N3 | save_memory is built and passes the capture gate. A note does not appear in the page's Feed. `metadata` is ignored. | partial | rare | save-memory.ts:5 / src/view.rs:2060 |
| N4 | mem_search has the same contract: guided and auto modes, cursors, limits and a 15-minute TTL. | equal_or_better | none | progressive-search.ts:126 / src/mem_search.rs:223 |
| N5 | The native memory-lookup bridge uses the same triggers. Since 2026-10-17 it searches with the meaning side too when the resident viewer makes the lookup's query vector within the prompt's wait, else words only, as the prompt's block does (docs/memory-lookup.md). Its speed fell short: from 2026-10-15 to 2026-10-20, 39 of 59 lookups ended at Claude Code's 5 s with nothing shown, where claude-mem's bridge took a median of 247 ms. Since 2026-10-20 it gives up after 3 s as claude-mem's does, and its search reads far less: on a copy of this PC's store a cold lookup took 2.5 to 2.9 s and a warm one 0.8 to 1.0 s; a cold lookup of words most documents hold (MEMORY.md's) still passed 3 s and gave nothing. Installed and measured on the store itself the same day, the stores' pages dropped before each cold run: 4 of 6 lookups answered in 2.72 to 2.95 s cold and 2 passed 3 s; warm, 0.78 to 0.99 s. | partial | rare | memory-search.ts:154 / src/memory_lookup.rs:256 |
| N6 | The same memory-use instructions at session start, with a switch. | equal_or_better | none | memory-instructions.ts:1 / src/mem_search.rs:27 |
| N7 | A Codex subagent gets its memory back after a compaction. | equal_or_better | none | codex-compaction-gate.ts:1 / src/hook.rs:589 |
| G01 | work_state_write and work_state_read, shown first at session start, with a 3,000-character cap. Any field name is kept. | equal_or_better | daily | WorkStateRenderer.ts:13 / src/work_state.rs:17 |
| X1 | Since 2026-10-17 session start leaves out the cards of a window that is a subagent's work alone (docs/cards.md S4), as claude-mem's `MAIN_AGENT_ONLY` (on by default) leaves out subagent rows. claude-mem judges each tool call; oboete judges a window, so a window that mixes the main agent's records with a subagent's keeps its cards. In this PC's week to 2026-10-17 that is 1,117 of 1,830 cards. Cards kept before the change are marked by the worker at its first step after it. There is no switch to show them. | partial | rare | ObservationCompiler.ts:92 / src/cards.rs:432 |
| X2 | Session start on the live store (raw.db about 4 GB), as Claude Code timed both hooks in the 10 session starts from the switch to 2026-10-17: oboete's hook took a median of 135 ms (186 ms at a startup, 82 ms after a compaction, 442 ms at most), claude-mem's context hook 305 ms in the same sessions (540 ms and 287 ms, 725 ms at most). `oboete inject` in this repository took 74 to 147 ms (median 75 ms, 10 runs, 2026-10-17). The design reads a prepared manifest behind a 1 s limit. | equal_or_better | daily | context-cache.ts:30 / src/hook.rs:25 |
| X3 | `--resume` injects nothing, for every agent. | equal_or_better | daily | context.ts:137 / src/hook.rs:549 |
| X4 | Failed tool calls are recorded together with their error. | equal_or_better | daily | adapters/claude-code.ts:12 / src/capture.rs:391 |
| X5 | Read-only memory tools carry a read-only hint, so they run in plan mode. | equal_or_better | rare | mcp-server.ts:504 / src/mcp.rs:226 |
| X6 | Memory searches and saved notes are not curated again as new memories. | equal_or_better | daily | memory-retrieval-tools.ts:20 / src/curate.rs:608 |
| X7 | Raw-text retention by days, after curation, with a page control. Off by default on both sides. | equal_or_better | rare | tool-uses.ts:494 / src/retention.rs:34 |
| X8 | The feed has a stable newest-first order, and repeated prompts are never merged. | equal_or_better | rare | PaginationHelper.ts:102 / src/view.rs:2110 |
| X9 | Long prompt cards are not collapsed behind "Show more". The server caps a prompt at 2,000 characters. | partial | rare | viewer-template.html:1152 / src/view.rs:2100 |
| X10 | claude-mem 14.0 opens its viewer on My Work: each request given to the agents as a card with a status (in progress, needs you, no recent update, ready to check, checked), a model-written summary that keeps the last good one while a new one is written and shows the recorded wording when no compatible model can write one, list and agent views, and a drill-down to the work and the original request; a person marks a result checked. claude-mem writes the summaries only with OpenRouter as its provider; the owner's writes none, so its cards show the recorded wording. oboete has no such view. Decision 53: built in the table's order, before removal, with the session summaries its curators already write as the cards' summaries, so it adds no model call (docs/my-work.md). | missing | weekly | WorkViewRoutes.ts:1 / src/view.rs:1 |

The sections after this one are the first comparison, of 2026-10-03 against claude-mem 13.28.0.

## How it was made

- claude-mem's source at commit 039c6160 (package version 13.28.0, 2026-10-01) was read area by area into an
  inventory of 225 features. Each was matched against oboete on main (7f8de51), the unmerged spec text of PR #342
  and the takeover plan, and given a status; each status was then checked by readers told to refute it.
- 190 of the 225 differ in some way and were grouped into the 60 differences below. The other 35 are present in
  oboete (20) or do not apply to it (15).
- The 32 differences brought into scope by decision 37 were checked a second time on both sides of the code, against
  the plugin as installed on the owner's PC (13.28.0, built 2026-09-27) and the owner's own settings. That pass
  changed the statement of 18 of them; the rows below carry the corrected text. Rows 33 to 60 were not checked a second time, and
  their text is the first pass's.
- The data is kept outside the repository, on the owner's machine: `../oboete-work/takeover-2026-10-02/`
  (`claude-mem-inventory.json`, `claude-mem-parity.json`, `parity-gap-checks.json`, and the owner's Japanese
  summary `owner-parity-2026-10-03.md`).

## Two baselines

The source at 039c6160 holds features the installed build does not: the sessions tab and session page, deleting
from the page, joining and naming projects, a public address and allowed origins for the page, more provider
options, `/handoff`. The baseline includes all features under development (decision 37), so they are in scope;
the rows say which baseline a feature belongs to. Because that baseline moves, upstream's history is read again at
each milestone and new features are added here.

## What the owner will notice on the day of the switch

- Memory arrives a window at a time, by design (row 54): claude-mem writes a titled card within seconds of each
  tool call; oboete writes the same cards, and claims quoting the owner's or the agent's words, as soon as a
  window is full while the work goes on, and those of the last records once the work pauses for ten minutes (a
  setting; decision 39).
- New sessions have no summary card on the page and none in search until row 17 is built.
- claude-mem's slash commands and its code-structure tools go when its plugin is removed (rows 23 and 24).

## Where oboete is ahead (measured or read in the code)

- Several curators in a chain with fallback; the installed claude-mem has one provider.
- Records are stored before any model runs, so a curator that stops loses nothing.
- The page's API needs a token (the page's own files are served without one) and every request's
  Host header is checked. The installed claude-mem answers any local program without one, its
  settings route included (checked on the owner's PC, 2026-10-03).
- Redaction is on by default, and a change of its rules reaches what is already stored; claude-mem's is
  off by default and masks new records only. Both can be turned off (decision 43).
- Search on the owner's questions: nDCG@10 0.545 with vectors against claude-mem's 0.244 (spec 4.10).
- Memory: 13 MB at its peak for the recording path, against claude-mem's 120 MB worker and 4.65 GB of Chroma
  (docs/plan.md, the M0 measurement). The resident worker and viewer are measured when built (spec 1.8).

## The 60 differences

### Before the switch (planned)

| # | Difference | Size |
|---|---|---|
| 1 | **Always-on background worker and a bookmarkable memory page.** claude-mem keeps a background process running and its browser page is always at the same address. In oboete today the background process stops after a minute and the page exists only while `oboete view` runs in a terminal, at a new address each time. | M |
| 3 | **Deleting a memory or a whole session.** Deleting a memory or a session from the page exists in claude-mem's source under development, not in the installed 13.28.0. oboete has no delete on main; milestone 5's forget (one item, a session, a repository, a time span, with a preview, irreversible) is built before the switch. | XL |
| 4 | **Rest of the settings page: provider add/remove, switches, first-run wizard.** In claude-mem the provider and most settings are chosen in the page or at install. In oboete the page edits only injection, capture detail and the existing provider list; adding a provider, turning summarising on, Gemini, redaction rules, backups, the monthly cap and embeddings still mean editing config.toml by hand. | L |
| 5 | **Search by meaning is off unless set up by hand.** claude-mem finds memories by meaning out of the box, on the machine itself. oboete has the same kind of search built, but it is off by default; turning it on means editing config.toml and sending text to Cloudflare, and old migrated records get no meaning-search at all. | L |
| 12 | **A record that fails to write is not retried.** claude-mem's hooks need its worker running and replay what they could not send. oboete's hooks write directly, so a stopped worker loses nothing, but if a write itself fails (disk busy or full) that one record is dropped and only reported at the next session start. | S |
| 14 | **Excluding a project or folder from recording.** claude-mem can be told not to record certain projects. oboete today records every repository and folder an agent works in. | L |

### Before the switch (added 2026-10-03)

| # | Difference | Size |
|---|---|---|
| N1 | **claude-mem's stored history is not in the everyday store.** `oboete import claude-mem` refuses the default home until repositories map onto claude-mem's project names (src/main.rs: "importing into the everyday store waits for the repository mapping"). Without it, what claude-mem recorded stops being searchable when claude-mem is removed. The mapping and the import are built before the switch (decisions 35 and 37); the spec had the import at milestone 7. Built as docs/claude-mem-import.md: the guard goes (a repository reads its project by name, as search does), claude-mem's schema to v64 (13.35.0), and its work state. | M |
| 6 | **Fewer options in the agent's memory search tools.** claude-mem's search tools filter by type, order by date and fetch several ids in one call. oboete's search, timeline and get take fewer filters and one id at a time. Paging past 100 and a by-date timeline are weaker in claude-mem than they look (no offset on its semantic path; the date anchor is not in the tool's schema) and are not copied. | M |
| 7 | **Looser word matching, and short Japanese words ignored in mixed queries.** In a query such as '設計 worker' the two-character word has no trigram and is ignored (src/search.rs `query_clauses`): fixed before the switch. All-words matching is not built: trigram OR ranking was measured better on the owner's questions (docs/milestone-1.md), and the owner's claude-mem answers by semantic search first anyway. | S |
| 8 | **No 'this file has history' note, and no lookup by file.** claude-mem shows the agent past notes about a file after it reads one (Read tool only, files over 1,500 bytes, once a session, delivered after the read; Codex: cat/head/tail and a few more). oboete has no such hook yet; its cards keep the files the curator was shown (docs/cards.md C3), which the note looks up, so the size is M (it was L when nothing kept a file list). Decision 39 moves it before the switch; lookup by type is row 6. | M |
| 9 | **Nothing shown in the terminal at session start.** When a session starts claude-mem prints the memory timeline and the page link in the terminal for the user to see. oboete gives the memory to the agent only; the terminal shows nothing. | S |
| 10 | **No first-visit explanation on the page.** claude-mem's page shows a welcome card and a 'How it works' text the first time. oboete's page opens straight on the timeline with no explanation of the tabs or how to ask the agent for memory. | S |
| 11 | **Summarising stopped: no notice with the cause where the owner looks.** When curation stops, claude-mem shows a banner with the cause at session start for its own provider failures. oboete shows 'N records not yet curated'; the cause and the command to curate what was skipped are in `oboete doctor` only. | M |
| 13 | **Tokens and money spent are recorded but not shown.** oboete records tokens and cost of every curation call and enforces a monthly cap, and shows only the embedding spend (doctor). The month's curation spend is shown beside the cap on the settings page. claude-mem shows no money figure. | S |
| 28 | **No 'memory is on' note in a brand-new project.** In a project with no memory yet claude-mem says that recording has started and where the page is. oboete says nothing. Folded into the terminal line at session start. | S |
| 31 | **Per-prompt memory hints are more conservative.** Both tools can add matching memories to each prompt. The owner has claude-mem's on; oboete's is off by default. It adds current decisions, lessons and approved open items first, then the repository's cards and session summaries, found by meaning through the resident viewer within a capped wait or by words, each above its threshold, at most 5 (decision 44, [prompt-context.md](prompt-context.md)); claude-mem adds its 5 nearest observations with no cut. It is switched on at the switch; its thresholds are measured in the one evaluation (decision 34). | M |

### Before the switch (added 2026-10-13, decision 43)

| # | Difference | Size |
|---|---|---|
| 60 | **Secret masking can be switched off.** In claude-mem masking of secrets is an optional setting, off by default. oboete has the same switch, on by default, in `[redaction]` and on the settings page; off, nothing is masked at capture, when text is sent, or in what agents read. It was left out on 2026-10-03 (decision 37) and asked for on 2026-10-13. | S |

### Before the switch (added 2026-10-14, from claude-mem 13.36)

| # | Difference | Size |
|---|---|---|
| N2 | **An older binary run on a newer home.** claude-mem 13.36 stamps its database with the newest schema that wrote it and refuses changes from an older build. oboete had no such stamp: an older binary run against a home a newer one wrote would write with the older meaning. Built in docs/store-format.md: raw.db carries the home's format, and every open of it refuses a newer one. | M |

### Before the switch (added 2026-10-14, decision 45, from claude-mem 13.35)

| # | Difference | Size |
|---|---|---|
| N7 | **A Codex subagent gets its memory back after a compaction.** claude-mem 13.39 marks a Codex thread at `PostCompact` and gives the thread's next tool call the session start's context once, since Codex sends a subagent no SessionStart. oboete answered only the root thread's SessionStart(compact). Built the same way per thread (`compacted` for the root, a hash of the subagent's id otherwise); oboete gives a mark back when the call cannot take its read fence or fails its write, where claude-mem spends it. `oboete setup` must run again: Codex trusts the changed PostToolUse hook only then. | S |
| N6 | **The session start says how to use memory.** claude-mem 13.35 ends its session start with a short "memory use" block: search first with `mem_search`, save durable notes with `save_memory`, keep to-dos with `work_state_write`, separate facts from guesses, leave credentials out; a setting turns it off. oboete had the tools' names in one index line. Built with claude-mem's text (its server, hosted connector and folder watcher lines left out), after the work state's rule, out of `[inject] session_start_chars`, and the same switch (`[inject] memory_instructions`); Pi, which has no MCP client, does not get it. | S |
| N5 | **An agent's own memory lookups answered from memory.** claude-mem 13.35 answers a native memory lookup (an agent's notes folder, its memory search) with its own search of the same words, beside the native result, on Claude Code and Codex. oboete had nothing there. Built with claude-mem's recognition and matchers, `mem_search` in auto mode on the words side, in the file note's room, and the same switch (docs/memory-lookup.md). Folders of claude-mem's watcher stay with row 44. | M |
| N4 | **One guided memory search.** claude-mem 13.35 gives the agent `mem_search`: an index, the context around chosen entries and their details, in steps it cannot skip, each taking a short continuation, or all at once in auto mode. oboete had `search`, `timeline` and `get`, which teach the same order and do not hold to it. Built with the same name, arguments, steps and texts on oboete's readers; the older tools stay and say to prefer it (docs/mem-search.md). | M |
| N3 | **The agent saves a note on purpose.** claude-mem 13.35 gives the agent `save_memory`, which stores a note that search finds and the session start never shows. oboete had no such tool (row 32). Built with the same name and arguments: the note is a document of its repository, through the capture gate, refused where nothing is recorded, labelled `note` in search (docs/tools.md, slice 4). | M |

### Before removal (added 2026-10-20, decision 53, from claude-mem 14.0)

| # | Difference | Size |
|---|---|---|
| X10 | **What was asked, and where it stands.** claude-mem 14.0 opens its viewer on My Work: every request given to the agents as a card with a plain summary a model writes (the recorded wording when none can), who worked on it, its project and its status, attention first, and a mark that a person checked the result. oboete keeps the same requests and the work after them but shows no such view. claude-mem's summaries need OpenRouter as its provider, and the owner's writes none; oboete's cards take the session summaries its curators already write, so My Work adds no model call (docs/my-work.md). | M |

### After the switch, first

| # | Difference | Size |
|---|---|---|
| 2 | **Controlling the always-on worker: stop, restart, status, stuck-worker replacement.** claude-mem has commands to stop, restart and check its worker, and a launcher replaces one that is old or fails two health probes. oboete's resident worker (decision 36) is replaced when the binary changes, steps aside for a command that needs its lock, and doctor says whether it runs; stop and restart commands and a watchdog for a hung worker come after the switch. | M |
| 16 | **Sessions list, one-session page and category filter in the viewer.** A sessions tab, a page per session and filter chips by kind are in claude-mem's source under development, not in the installed 13.28.0. oboete's page has one timeline per repository. In scope by decision 37. | M |
| 17 | **Readable 'what this session did' summaries.** claude-mem writes a structured summary of each session (request, what was learned, what was completed, next steps) that can be searched and read in the page. oboete writes a short digest shown only at the next session start; it is not searchable or listed in the page. | M |
| 21 | **No way to say 'these two are the same project'.** Joining two projects and naming one by hand are in claude-mem's source under development, not in the installed 13.28.0, which names a project by its folder. oboete keys a repository by its origin URL, so a rename on GitHub or a remote added later splits its memory; `oboete repo alias` (appendix B, 30-4) joins them. | M |
| 23 | **The 22 slash-command skills that come with the claude-mem plugin.** The claude-mem plugin brings 21 slash-command skills (22 upstream, with /handoff). oboete ships none. Decision 37: the memory ones (search, timeline report, how it works, handoff) come to oboete where needed. | M |

### After the switch, before sync (decision 33)

| # | Difference | Size |
|---|---|---|
| 35 | **Bots and other programs cannot record into or query oboete (no local HTTP API, no bot adapter).** claude-mem's worker has web addresses that any script or bot can post to and query, a generic hook for custom tools, and a watcher that follows a bot's transcript. oboete accepts records only from its seven agents' hooks and answers only its own page, the CLI and MCP. | XL |

### After the switch, one agent at a time (decision 35)

| # | Difference | Size |
|---|---|---|
| 34 | **Cursor, OpenCode and Antigravity are wired but never checked live.** claude-mem supports these tools with hooks, rules files or plugins. oboete has a setup command for each and delivers memory by hook instead of a rules file, but none has been tried in a real session. | M |

### Milestone 6 (sync)

| # | Difference | Size |
|---|---|---|
| 37 | **Sharing memory between the owner's machines (cloud sync).** claude-mem's paid plan syncs memory across devices. oboete has no sync yet; the plan is the owner's own hub on Cloudflare, so WSL, Windows and the iMac keep separate memory until then. | XL |

### Milestones 7 and 8

| # | Difference | Size |
|---|---|---|
| 51 | **Install, update and documents for a published release.** claude-mem installs with one command, updates itself, and ships a README in 32 languages and security documents. oboete is built from source with cargo, updated by rebuilding, and has no README or user-facing documents yet. | L |

### Later, when asked for or when upstream ships it

| # | Difference | Size |
|---|---|---|
| 18 | **No filters on what gets recorded (tool skip list, per-hook switches, advisor and Codex subagent capture).** claude-mem skips five tools by default (ListMcpResourcesTool, SlashCommand, Skill, TodoWrite, AskUserQuestion). oboete records every tool call, and uses two of those on purpose (the owner's answers as the owner's words; the todo list in the manifest). A per-tool skip setting is added if noise shows; the list is not copied. | M |
| 19 | **Fewer controls over what the session-start memory contains.** claude-mem's settings choose how many items and sessions the session-start block holds and which types and fields. oboete switches each kind of injection on or off and sets its size. The owner runs every one of claude-mem's at its default. | M |
| 27 | **Setting for a worker run by an outside process manager.** claude-mem has a setting that tells hooks never to start the background process, for people who run it under a container or service manager. oboete has only an undocumented environment variable for this. | S |
| 29 | **Small recording differences: image-only prompts, duplicate prompts, prompt numbers, subagent type.** claude-mem records an image-only prompt as a placeholder, drops a prompt delivered twice within 10 seconds, numbers prompts within a session and stores the subagent's type. oboete skips image-only prompts, stores a double delivery twice, has no prompt number and stores only the subagent's id. | S |
| 30 | **Ranking by how often a memory was confirmed again.** claude-mem can keep the days on which a memory was confirmed again and rank by them, when a setting is turned on. oboete has no such history; it brings older items in by word match with current work. | M |
| 32 | **Reloading the context from the agent.** claude-mem lets the agent fetch the start-of-session context for any project. oboete's agent has no tool for it. Saving a note on purpose is built (N3). | M |
| 33 | **Agents oboete does not support at all (OpenClaw, Kimi, Windsurf, Oh My Pi, Copilot CLI, Goose, Roo, Warp).** claude-mem installs into these tools. oboete supports seven coding agents and none of these, so their sessions would not be recorded and would get no memory. | L |
| 36 | **Connecting a new tool by writing a transcript watch config.** claude-mem lets the user describe a new tool's transcript format in a config file, with examples and commands to create, check and run it. In oboete a new format has to be built into the program. | L |
| 38 | **Copying memory between machines over SSH without a cloud account.** claude-mem's repository has a script that pushes, pulls and compares memory between two machines over SSH. oboete has nothing like it; its planned sync always goes through the owner's Cloudflare hub. | M |
| 39 | **Claude's desktop app (Cowork and Claude Desktop).** claude-mem has a plugin that records Cowork sessions and gives them memory. oboete plans only read-only memory search from the Claude app, and nobody has tried a hand-written setup for Claude Desktop. | XL |
| 40 | **Knowledge corpora and the question-answering 'knowledge agent'.** claude-mem can save a named slice of memory and load it into a helper that answers questions from it. oboete has nothing like it; the working agent searches and reads memory itself. | L |
| 41 | **Memory modes (non-coding modes, 'chill' level, custom modes).** claude-mem can switch to other kinds of memory (email investigation, law study, a quieter recording level) or a mode the user defines. oboete has one fixed set of memory kinds for coding; only the language is a setting. | L |
| 42 | **Telegram messages (alerts and end-of-session wrap-ups).** claude-mem can send a Telegram message for certain memories and at the end of a session. oboete sends nothing to any messaging app. | M |
| 43 | **Per-folder 'Recent Activity' notes written into CLAUDE.md files.** claude-mem can write a recent-activity timeline into CLAUDE.md files in each folder. oboete never writes memory into files of a repository; it delivers context through hooks. | M |
| 44 | **Claude Code's own memory notes: import them, or switch them off.** claude-mem can import the notes Claude Code keeps in its own memory folder and offers a switch to turn that feature off. oboete does neither, so those notes are not searchable in oboete. | M |
| 45 | **Exporting chosen memories to a file and importing them elsewhere.** claude-mem can write the memories matching a search to a file and load that file on another machine. oboete has only whole-store backup and restore. | M |
| 46 | **Merging near-duplicate memories.** claude-mem has an experimental setting that merges memories saying almost the same thing and counts how often they occurred. oboete merges only exact repeats of the same quoted sentence, so the same fact from several sessions can appear several times. | M |
| 47 | **Provider options oboete lacks (Claude by API key, presets, key rotation, parallel calls, reasoning text).** claude-mem offers Claude through an API key or gateway, ready-made presets for many services and local models, several keys per provider, two summarisers at once and a separate model for summaries. oboete uses the owner's subscription logins and one key per provider, one call at a time, and rejects a model that prints its reasoning before the answer. | M |
| 48 | **No log of what a summariser tried to do, and Codex's summariser reads the owner's AGENTS.md.** claude-mem logs any tool a summariser tried to use and refuses a Codex call if any instruction file was loaded. oboete blocks the tools just as firmly but keeps no such log, and the owner's own ~/.codex/AGENTS.md still reaches the Codex summariser. | S |
| 49 | **The page updates every 3 seconds instead of instantly.** claude-mem's page receives new memory the moment it is stored. oboete's page checks every 3 seconds, only while it is the visible tab. | S |
| 50 | **Uninstall says nothing about the data left behind.** claude-mem's uninstall asks for confirmation and removes the program. oboete's `setup --remove` takes the hooks out and keeps the data, but does not say that ~/.oboete still holds everything, does not remove the program file and does not stop a running worker. | S |
| 52 | **No log history, and no log view in the page.** claude-mem keeps daily log files and shows them in a console on its page. oboete keeps only the latest error and a record of provider calls, read through `oboete doctor`. | M |
| 53 | **Status-line counter script and document batch runner.** claude-mem ships a small script that prints a project's memory counts for a status line, and its repository has a developer script that feeds a folder of documents through sessions. oboete has neither. | S |

### Not built

| # | Difference | Size |
|---|---|---|
| 15 | **A fully private prompt still leaves that turn's work in memory.** No difference: in claude-mem 13.28.0 a prompt written wholly inside <private> does not keep the turn's tool calls out either (the turn is matched to the previous saved prompt). Both hide the prompt text. Stripping harness blocks from tool output stays unbuilt until such blocks are seen in captured fields (docs/m1.md). | S |
| 20 | **Facts inside very large tool outputs do not become memories.** claude-mem has a model shorten a very long tool output so its content can be summarised. In oboete an output past the window (about 17,800 characters by default) is recorded as 'seen, elided' for curation (spec 3.1) and stays findable by raw search. | M |
| 22 | **Team server (shared store, API keys, job commands).** claude-mem has a beta server that a team can share, with a database server, API keys and commands to manage jobs. oboete plans a single-owner sync hub and a local API, not a shared team server. | XL |
| 24 | **Code structure tools (smart_search, smart_outline, smart_unfold).** claude-mem's plugin also brings tools that search code by symbol and show a file's outline. oboete is a memory tool and has none. | L |
| 25 | **Opening the page under another address, or letting another web page call it.** Settings that show the page under another address or let listed web pages call it are in claude-mem's source under development, not in the installed 13.28.0. oboete's page answers on 127.0.0.1 only and sends no CORS headers (spec 6.6). | M |
| 26 | **Observation TV: a full-screen live display, also from a phone or another PC.** claude-mem serves an unlinked full-screen page of fading title cards, watchable from another device with a token (off for the owner). oboete has no such page. | M |
| 54 | **Memory arrives a window at a time, by design.** claude-mem makes a titled card per tool call within seconds and injects a dated table. oboete makes the same cards, and its own quoted claims, in one call per window: at once when a window is full while the work goes on, and for the last records after the owner pauses (10 minutes, a setting; the cadence stays, decision 39); it injects the owner's decisions and current state before the same dated table (docs/cards.md S1), does not re-inject on resume, and always shares memory between agents in the same repository. | - |
| 55 | **Docker image for trying the tool in isolation.** claude-mem's repository has an experimental Docker image for trying it in a sealed box. oboete is tried under a separate OS user (oboete-dogfood) or a scratch home folder. | - |
| 56 | **cmem.ai account, hosted summariser and login shim.** claude-mem can sign in to its maker's service and use a hosted summariser, or a local shim that reuses the host login. oboete has no account: summaries come from the owner's own subscriptions and keys, in a chain with fallback. | - |
| 57 | **Usage statistics and error reports sent to the maker.** claude-mem sends anonymous usage data and error reports to its maker by default. oboete sends nothing. | - |
| 58 | **claude-mem's internal queue, retry and size-limit machinery.** claude-mem keeps waiting work in memory, retries bad answers on the same provider and looks up each model's size limit. oboete writes everything to disk first, sends small fixed batches, and on a bad answer asks the next provider and tries again later. | - |
| 59 | **Worktree 'adopt' step and start-up repair jobs.** claude-mem files a worktree as its own project and merges it into the main one after the branch is merged. oboete files a worktree under the main repository from the first record, so there is nothing to adopt. | - |
