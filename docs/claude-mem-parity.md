# claude-mem and oboete, feature by feature (2026-10-03)

Owner decision 36: 「完成品が完全にclaude-memの上位互換なら今のoboeteを完成させたい」. This is the table that
decision asks for, and decision 37 is what the owner chose from it.

## How it was made

- claude-mem's source at commit 039c6160 (package version 13.28.0, 2026-10-01) was read area by area into an
  inventory of 225 features. Each was matched against oboete on main (7f8de51), the unmerged spec text of PR #342
  and the takeover plan, and given a status; each status was then checked by readers told to refute it.
- 190 of the 225 differ in some way and were grouped into the 60 differences below. The other 35 are present in
  oboete (20) or do not apply to it (15).
- The 32 differences the owner's choice turned on were checked a second time on both sides of the code, against
  the plugin as installed on the owner's PC (13.28.0, built 2026-09-27) and the owner's own settings. That pass
  changed the statement of 18 of them; the rows below carry the corrected text. Rows 33 to 60 were not checked a second time, and
  their text is the first pass's.
- The data is kept outside the repository, on the owner's machine: `../oboete-work/takeover-2026-10-02/`
  (`claude-mem-inventory.json`, `claude-mem-parity.json`, `parity-gap-checks.json`, and the owner's Japanese
  summary `owner-parity-2026-10-03.md`).

## Two baselines

The source at 039c6160 holds features the installed build does not: the sessions tab and session page, deleting
from the page, joining and naming projects, a public address and allowed origins for the page, more provider
options, `/handoff`. The owner chose the wider baseline (decision 37: 「開発中の機能も全部」), so they are in scope;
the rows say which baseline a feature belongs to. Because that baseline moves, upstream's history is read again at
each milestone and new features are added here.

## What the owner will notice on the day of the switch

- Memory looks and arrives differently, by design (row 54): claude-mem writes a titled card within seconds of each
  tool call; oboete writes short claims, each with a quote of the owner's or the agent's words, once the work
  pauses (about ten minutes).
- New sessions have no summary card on the page and none in search until row 17 is built.
- claude-mem's slash commands and its code-structure tools go when its plugin is removed (rows 23 and 24).

## Where oboete is ahead (measured or read in the code)

- Several curators in a chain with fallback; the installed claude-mem has one provider.
- Records are stored before any model runs, so a curator that stops loses nothing.
- The page needs a token and checks the Host header. The installed claude-mem answers any local program without
  one, its settings route included (checked on the owner's PC, 2026-10-03).
- Redaction is always on.
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
| N1 | **claude-mem's stored history is not in the everyday store.** `oboete import claude-mem` refuses the default home until repositories map onto claude-mem's project names (src/main.rs: "importing into the everyday store waits for the repository mapping"). Without it, what claude-mem recorded stops being searchable when claude-mem is removed. The mapping and the import are built before the switch (decisions 35 and 37); the spec had the import at milestone 7. | M |
| 6 | **Fewer options in the agent's memory search tools.** claude-mem's search tools filter by type, order by date and fetch several ids in one call. oboete's search, timeline and get take fewer filters and one id at a time. Paging past 100 and a by-date timeline are weaker in claude-mem than they look (no offset on its semantic path; the date anchor is not in the tool's schema) and are not copied. | M |
| 7 | **Looser word matching, and short Japanese words ignored in mixed queries.** In a query such as '設計 worker' the two-character word has no trigram and is ignored (src/search.rs `query_clauses`): fixed before the switch. All-words matching is not built: trigram OR ranking was measured better on the owner's questions (docs/milestone-1.md), and the owner's claude-mem answers by semantic search first anyway. | S |
| 9 | **Nothing shown in the terminal at session start.** When a session starts claude-mem prints the memory timeline and the page link in the terminal for the user to see. oboete gives the memory to the agent only; the terminal shows nothing. | S |
| 10 | **No first-visit explanation on the page.** claude-mem's page shows a welcome card and a 'How it works' text the first time. oboete's page opens straight on the timeline with no explanation of the tabs or how to ask the agent for memory. | S |
| 11 | **Summarising stopped: no notice with the cause where the owner looks.** When curation stops, claude-mem shows a banner with the cause at session start for its own provider failures. oboete shows 'N records not yet curated'; the cause and the command to curate what was skipped are in `oboete doctor` only. | M |
| 13 | **Tokens and money spent are recorded but not shown.** oboete records tokens and cost of every curation call and enforces a monthly cap, and shows only the embedding spend (doctor). The month's curation spend is shown beside the cap on the settings page. claude-mem shows no money figure. | S |
| 28 | **No 'memory is on' note in a brand-new project.** In a project with no memory yet claude-mem says that recording has started and where the page is. oboete says nothing. Folded into the terminal line at session start. | S |
| 31 | **Per-prompt memory hints are more conservative.** Both tools can add matching memories to each prompt. The owner has claude-mem's on; oboete's is off by default and adds only current decisions, lessons and approved open items from a prepared shortlist. It is switched on at the switch; its threshold is measured in the one evaluation (decision 34). | M |

### After the switch, first

| # | Difference | Size |
|---|---|---|
| 2 | **Controlling the always-on worker: stop, restart, status, stuck-worker replacement.** claude-mem has commands to stop, restart and check its worker, and a launcher replaces one that is old or fails two health probes. oboete's resident worker (decision 36) is replaced when the binary changes, steps aside for a command that needs its lock, and doctor says whether it runs; stop and restart commands and a watchdog for a hung worker come after the switch. | M |
| 8 | **No 'this file has history' note, and no lookup by file or type.** claude-mem shows the agent past notes about a file after it reads one (Read tool only, files over 1,500 bytes, once a session, delivered after the read; Codex: cat/head/tail and a few more). oboete has no such hook and claims carry no file list. | L |
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
| 32 | **Saving a memory on purpose, and reloading the context, from the agent.** claude-mem lets a tool or the agent save a note on purpose and fetch the start-of-session context for any project. In oboete only a global preference can be added by hand, and the agent has no tool to save a note or reload the context. | M |
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
| 54 | **Memory looks and arrives differently by design.** claude-mem makes a titled card per tool call within seconds and injects a dated table. oboete makes short quoted claims in batches after the owner pauses (about 10 minutes), injects sections of decisions and current state, does not re-inject on resume, and always shares memory between agents in the same repository. | - |
| 55 | **Docker image for trying the tool in isolation.** claude-mem's repository has an experimental Docker image for trying it in a sealed box. oboete is tried under a separate OS user (oboete-dogfood) or a scratch home folder. | - |
| 56 | **cmem.ai account, hosted summariser and login shim.** claude-mem can sign in to its maker's service and use a hosted summariser, or a local shim that reuses the host login. oboete has no account: summaries come from the owner's own subscriptions and keys, in a chain with fallback. | - |
| 57 | **Usage statistics and error reports sent to the maker.** claude-mem sends anonymous usage data and error reports to its maker by default. oboete sends nothing. | - |
| 58 | **claude-mem's internal queue, retry and size-limit machinery.** claude-mem keeps waiting work in memory, retries bad answers on the same provider and looks up each model's size limit. oboete writes everything to disk first, sends small fixed batches, and on a bad answer asks the next provider and tries again later. | - |
| 59 | **Worktree 'adopt' step and start-up repair jobs.** claude-mem files a worktree as its own project and merges it into the main one after the branch is merged. oboete files a worktree under the main repository from the first record, so there is nothing to adopt. | - |
| 60 | **Secret masking cannot be switched off.** In claude-mem masking of secrets is an optional setting. In oboete it is always on; a value masked by mistake can only be kept by allowing that one value. | - |
