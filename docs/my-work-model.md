# My Work's goals, rules and progress, written by the curators (parity rows X12 to X15)

Plan of the slices that build what claude-mem 15.1 writes with a model for My Work (decision 57,
2026-10-25), written before the code, as the other slices' notes are. It covers My Goals (parity
row X12), My Rules (X13), each request's progress and build story (X14), one card for a request
across conversations (X15), and which prompts are the owner's own (X21), which goals and rules
read. The behaviour of record is claude-mem v15.2.0; every claim about it below names its file at
that tag. Decision 57's other part, removing a card or a session summary on the page, reversibly,
has a note of its own (card-hide.md, slice 9).

## What claude-mem does (15.2.0)

### The automatic refresh

- The worker checks every 10 minutes, and once at its start (`auto-refresh-worker.mjs:12,39`). A
  check reads the database and the ledger files and calls no model. Work is due when a
  conversation of the last 14 days is not grouped, a request of the last 14 days has no current
  review (new activity after its last review, or a changed set of conversations), the goals are
  missing or older than 24 hours, or a request needs its goal links decided: it has none yet, it
  changed since they were decided, one of its goals is gone, or a goal was added, which makes every
  request due; with no goals saved this check is skipped (`request-groups.mjs:34,426-479`).
- When something is due, it runs one job on the first logged-in agent among Claude Code, Codex
  and Gemini. It tries the next agent only when the job fails (`agent-runner/run.ts:272-304`).
  The steps, in order: goals, grouping, reviews, goal links, story (`refresh-task.ts:193-305`).
- Models (`refresh-task.ts:29-35`): Claude Sonnet for goals, reviews and the story, and Haiku for
  grouping and goal links; Codex's default model and `gpt-6-luna` at low effort; Gemini `flash`
  and `flash-lite`.
- The agent only answers. It runs read-only, its own sessions are not recorded
  (`CLAUDE_MEM_INTERNAL=1`, no session kept), and the worker checks and saves each answer
  (`run.ts:92-175`). A step asks twice at most, the second time with the refusal. A step refused
  twice fails the job, and the next agent starts over from what was saved
  (`refresh-task.ts:171-185`).
- It is on by default (`CLAUDE_MEM_WORK_VIEW_AUTO_REFRESH` is `true`,
  `SettingsDefaultsManager.ts:627`); it pauses while the setup page's funnel is active and stops
  when the setting is `false`, checked at every tick (`WorkViewRoutes.ts:203-204`). There is no
  daily cap, no time limit and no backoff after a failure. No token or cost is recorded:
  `refresh.json` keeps the run's state, agent, start and end, what was due, the result or error,
  and each step's name, agent, start and end (`auto-refresh-worker.mjs:1-6,82-102`).
  With an Anthropic key in `~/.claude-mem/.env`, the Claude runs bill that key
  (`EnvManager.ts:368-446`).
- The live page has no refresh button, only a status line (`components/bits.tsx:255-300`).
  `runNow` exists, and nothing calls it.

### Which prompts are the person's own (row X21)

- Goals read the agents' own transcripts, not the prompts claude-mem's hooks stored
  (`refresh-task.ts:112`). The worker's reader covers Claude Code, Codex, Gemini CLI, Antigravity
  CLI and OpenCode, for 30 days (`prompt-import/readers.ts`, `prompt-import/index.ts:26-27`).
- A Claude Code line counts when its `origin.kind` is `human`, its `entrypoint` is `cli` or
  `claude-desktop`, and it is neither a sidechain nor meta (`readers.ts:165-192`). claude-mem's
  own note says headless (`claude -p`), SDK, subagent and observer runs never carry a human origin
  (`export_owner_prompts.py:5-6`); the code checks only those four fields.
- A Codex session counts when it is not a `codex exec` run and not a subagent's thread. Codex's
  own blocks (`# AGENTS.md`, `<environment_context` and the like) are skipped
  (`readers.ts:214-267`). Only a session whose `source` is `exec` or not a string (a subagent's
  thread), or whose originator is `codex_exec`, is skipped (`readers.ts:244`), so a session Claude
  Code's Codex plugin starts counts as the person's: on this PC those have `source: "vscode"` and
  originator `Claude Code`.
- Two text tests then drop a prompt as an agent's (`owner-prompt-filter.ts:20-45`):
  - its first 120 normalized characters occur in text an agent wrote earlier (replies and tool
    inputs, from 9 days before the window). Only prompts of 160 normalized characters or more
    that do not open in lowercase are tested. Normalizing lowercases the text and turns every run
    of other characters than a to z and 0 to 9 into one space (`owner-prompt-filter.ts:25-27`), so
    a Japanese prompt keeps only its Latin words and digits, stays under 160, and is not tested;
  - it opens with "You are " and the, a, an, one, on or in, or with "Task: " or "Branch: ", and
    runs past 400 characters.
- Prompts of excluded projects drop out (`index.ts:103-104,142`). The reader ignores
  `CLAUDE_CONFIG_DIR` (`readers.ts:151`).
- Grouping and reviews skip only the prompt ids the setup scan saved as an agent's. A prompt the
  hooks record after the scan is not tested (`request-groups.mjs:30-31,88-91`).

### My Goals (row X12)

- **Input.** One file of the person's typed prompts, oldest first, each headed by its local
  minute, agent and project, each cut at 3,000 characters, with no limit on their number
  (`owner-prompts-file.ts:14-41,63`).
- **First run**: the model reads the whole file and answers `{map, goals}`. **Update**: with goals
  saved, it reads only the prompts typed since the last save and the saved goals
  (`refresh-task.ts:205-230`). No model call when the window has no typed prompt (empty goals are
  saved), or when nothing was typed since the last save (the saved goals are saved again).
- **The goals**: up to 10 priorities and any number of open asks. Each has a rank, a title, a
  product, the times asked, the latest time and 2 to 4 quotes, newest first, each under 240
  characters (`owner-goals/SKILL.md:39-55`). Rank and times asked are the model's judgement; no
  code computes or checks them.
- **No schema** is passed; the answer is the text from the first `{` to the last `}`
  (`refresh-task.ts:84-89`). The worker replaces `generated_at` with the time the export started
  (`refresh-task.ts:118-124`).
- **Quotes** are checked by `save_goals.py` (`:69-86,125-138`): lowercased, whitespace collapsed,
  every quote mark taken as the same mark, pieces split on "..." or "…" and trimmed of edge
  punctuation, each piece at least 3 characters, every piece somewhere in the prompts typed in the
  quote's minute. A quote older than the file's first prompt is dropped, and so is an item left
  with none (`:89-104`); on an update, a quote from before the last save that no longer matches is
  dropped instead of refusing the save (`:107-122`). A quote is cut to 240 characters, with no mark
  (`:141-160`).
- **Size check.** An item's row must fit 2,000 characters of JSON. The script counts with Python's
  ASCII escapes, so a Japanese character counts 6 (`save_goals.py:155-159`); the server counts 1.
  The script drops the oldest quotes until the row fits, so a Japanese item keeps as many newest
  quotes as fit, often one or two; the save aborts when even one quote with the title (written
  twice), the product and the other fields does not fit.
- **Unchecked fields.** The script does not check rank, times asked, latest time or window. The
  page refuses all goals, with an error, when an item has no rank or no times asked, or when the
  window, `generated_at` or a time is unreadable; a missing latest time or quote time shows no time
  (`to-work-model.ts:230-260,296-327`).
- **Identity.** An item is its task string, `Priority: <title>` or `Open ask: <title>`
  (`request-groups.mjs:156-172`). Each save replaces the snapshot; items not in it are marked
  dropped (`save_goals.py:201-219`).
- **Months.** A finished month is kept only when someone runs the script by hand with `--start`
  and `--end` (`save_goals.py:168-176`). The refresh never does (`refresh-task.ts:199-203`).
- **The window** is 30 days in the reader, but the refresh often writes `window_days: 31`, and the
  page says "31 days to <day>" for a 30-day scan (`to-work-model.ts:310-316`). That day is the UTC
  date of `generated_at`.
- **Where kept**: `work_state_entries`, list `owner-goals`, under the home folder's project key.
  The goals are filtered out of session start and out of the request cards
  (`WorkStateRenderer.ts:144-146`, `agent-view.mjs:285`). The model's `owner-map.md` is saved and
  never read again (`refresh-task.ts:204,216-217`). Each day's prompt files stay on disk; no code
  deletes them.
- **The page** (`my-goals.tsx`): its own page, beside My Work and My Rules. "This month" and
  "Month by month" tabs. A lead sentence over the top 5 priorities. Each priority shows its rank,
  a status word computed from the linked requests (Nothing on it, Waiting on you, Moving, Asked
  again after ship, Shipped, Stalled), how often and when it was asked, and the average percent of
  its reviewed requests. Opened, it lists those requests and its quotes with their times. The open
  asks sit in a side panel; requests linked to no goal are listed last. Read-only. "Asked again
  after ship" compares month and day without the year (`lib/prompt-time.ts:9-11`).

### One card for a request across conversations (row X15)

- **A conversation** is an agent's session (`request-projection.mjs:68-71`). It is a candidate when
  it was active in the last 14 days and has an opening ask (`request-groups.mjs:58-62,85-92`). The
  ask test is a set of rules (`request-projection.mjs:6-46`): any non-ASCII text of two letters
  passes, so Japanese filler such as a bare "yes" or "thanks" is an ask.
- **A batch** is up to 20 undecided conversations of one project root, oldest first, with every
  open request of that root (no cap; each with its title, first ask and up to 3 recent asks) and
  links code can prove: two conversations that wrote the same work-state list or `request_id`
  (`request-groups.mjs:518-566,107-153`). A list used by more than 8 conversations proves nothing
  (`:36`).
- **The model decides** each conversation once: continue an open request, start a new one, or
  ignore it (`request-progress/SKILL.md:20-42`). It needs evidence to continue; when unsure it
  makes a new one. Code checks that every conversation is decided and that a group stays in one
  project root (`request-groups.mjs:210-264,571-589`). Only Codex gets a strict JSON schema
  (`refresh-task.ts:37-47`).
- **Once and for all.** A decided conversation is never asked about again
  (`request-groups.mjs:521-528`). Continuing a request replaces its title and its links with the
  new decision's (`:230-252`). No page control merges, splits, renames or ignores a request.
- **The title** the model writes is never shown on a card. A card's title comes only from
  model-written copy, which runs only with OpenRouter as claude-mem's provider
  (`to-work-model.ts:169`, `WorkViewRoutes.ts:180-189`).
- **The project root** is the project key before its first `/`, so worktrees share one; with
  project names taken from the git remote (`CLAUDE_MEM_PROJECT_NAME_SOURCE`), the whole `org/repo`
  key is the root (`session-project.mjs:14-20`). A session in a scratch folder takes the
  repository its tool calls touched most (`:98-184`).

### Progress and the build story (row X14)

- **Due.** A request with a conversation active in the last 14 days is due for review when it has
  no review, its set of conversations differs from the one last reviewed, or its latest activity is
  newer than the moment its last review's input was read
  (`request-groups.mjs:360-366,426-434,654`). So an active request is due at nearly every check,
  and one with no activity in 14 days never is.
- **Input** (`request-groups.mjs:409-422,629-655`): every human ask of its conversations in full,
  its observations' titles, and the previous review. With the same conversations as last time
  ("amend"), only what came after the last review. Claude Code may also search memory
  (`refresh-task.ts:316-323`).
- **Calls.** 8 requests a call; all calls start together (`refresh-task.ts:27,251-282`).
- **The review** (`request-groups.mjs:308-343`): a state (shipped, in-progress, stalled, abandoned,
  unknown), a percent, start, ship and last-activity dates, a summary (280 characters), a narrative
  (1,200), up to 12 dated milestones, up to 10 preferences with a quote, and an optional outcome.
  Code requires percent 100 and a ship date for shipped, and a ship date only then. Nothing checks
  that a quote is real.
- **Readings.** Each review adds a dated percent; the newest 20 are kept (`:42-43,353-358`). The
  card projects a finish date from the last two when they rise below 100, with no minimum gap
  (`measured-estimate.ts:9-19`).
- **What the card uses.** Shipped makes it Shipped; 100% not shipped makes it "Done, not checked".
  The other states look alike (`status-normalization.ts:72-87`). A status an agent recorded after
  the review wins (`to-work-model.ts:129-160`). The outcome is stored and never shown
  (`to-work-model.ts:137-225`).
- **The build story** (`request-groups.mjs:671-689`): one markdown text of up to 12,000 characters,
  a short recap paragraph first, and up to 30 merged preferences. Each update gets the previous
  story and the reviews whose facts changed, and returns the whole text again. Updates kept adding
  text until the story filled its cap, so the input states the room left (`:677-683`). A stale story
  never starts a run by itself (`:471-479`).
- **Goal links** (`refresh-task.ts:284-294`, `request-groups.mjs:436-466,690-717`): one call on the
  light model for every request due, each linked to the goals it serves (at most three, in the
  prompt only: `request-progress/SKILL.md:100`) or to none. A link names a goal by its key, the
  string `Priority: <title>` or `Open ask: <title>`, so a goal whose title or kind changes is a new
  goal and makes every request due again; a re-rank or new quotes do not. The card shows one goal
  chip, "No goal" or "Goal not linked yet" (`components/bits.tsx:75-92`).

### My Rules (row X13)

- **Suggestions** come only from the story's preferences (`my-rules.mjs:46-66`). A suggestion's id
  is a hash of its wording (`:19`). A story that rewords a preference makes a new suggestion; a
  rejected one comes back under its new wording (`:70,76-101`). The kind (directive, correction,
  praise, frustration) is a label and changes nothing (`:123-139`).
- **Actions** (`WorkViewRoutes.ts:595-632`): approve, edit, turn off, turn on, reject (sent as
  `dismiss`), add, and restore a suggestion. The page's Undo reverses its last change of any kind
  while the page stays open; an approve or a reject is undone by restoring the suggestion.
  "Replace" exists on the server; the page never sends it. Each change is a row in the `my-rules`
  list. Agents may not write that
  list (`WorkStateRoutes.ts:74-79`); rows from cloud sync are not checked
  (`SyncApply.ts:900-931`).
- **The file.** The rules set or approved and on are written to `~/.claude-mem/MY-RULES.md`, by
  scope, the text trimmed: `- <then>`, or `- When <when>: <then>` with the first letter of the
  "then" lowercased (`my-rules.mjs:151-168`). A newline and `## ` in a rule start a new
  heading there. A button adds `@~/.claude-mem/MY-RULES.md` once to `~/.claude/CLAUDE.md`
  (`WorkViewRoutes.ts:672-689`), so Claude Code loads the rules as instructions. No other agent
  gets them.

### My Focus and the judges (15.4.0, rows X12 and X15)

Read at tag v15.4.0 (2026-10-28); paths under `src/services/work-view/` unless named.

- **My Focus replaces My Goals and the goal links** (`pages/my-goals.tsx` deleted; Align's
  `align-input` and `record-alignment` removed). Every request sits in one focus area, in Other, or
  is "Sorting…" until placed. Areas and placements are rows of the work-state list `my-focus`,
  append-only and replayed in write order (`my-focus.mjs:1-8,14`).
- **An area** has a random id `fa_` and 12 hex digits, a name (80 characters, never a page word:
  my focus, my work, my rules, other, suggested, past focus, monitor, steer), a goal (300), an
  about, a status (suggested, kept, dismissed, done), a priority rank from the goals run and an
  owner rank from reordering, a source (owner-goals, clustering, promotion, seed, owner), the area
  it was merged into, evidence, and who decided (owner, system, owner-restore). A system row is
  skipped once the owner decided the area; Undo writes an owner-restore row
  (`my-focus.mjs:43-57,153-206`, `nav-words.mjs:8-14`).
- **A placement** is keyed by the request's anchor conversation, by the router or the owner, the
  owner's winning while its area is live; a dismissed area holds none (`my-focus.mjs:61-71,213-234`).
- **Where areas come from.** The goals run is shown the saved areas and names, for each mined area,
  the area it already is (`matches`), never by name; a new one is a suggested area
  (`my-focus.mjs:94-124`, `refresh-task.ts:286-332`). The focus step (the light model, 20 requests
  a call) places each queued request (no placement, a placement into an area that no longer takes
  requests, or a changed title, anchor ask or membership) in a kept or suggested area or Other,
  and may propose promoting one request that holds several asks. Then the Other pass, when the
  router's Other set changed, proposes new areas from two or more Other requests that are steps of
  one thing (`request-groups.mjs:452-627`). A dismissed area is never proposed again.
- **The owner's actions** (`WorkViewRoutes.ts:782-836`): keep, rename, edit goal, merge, dismiss,
  done, reorder the kept areas, move a request to an area or Other, and put back in Suggested (the
  Undo of a decision on a suggestion).
- **The page**: a "My focus" rail (kept areas, Other, "N suggested", Past focus) and chips on narrow
  screens filter My Work; an area's header (goal, rollup, status word, shipped count, times asked,
  Not started asks, Why it's here, a menu); a Suggested page (Keep, Rename, Merge, Dismiss, Undo);
  Past focus (done areas, month by month); the card's chip names its area; Move to… on the request
  (`components/focus-nav.tsx`, `focus-header.tsx`, `pages/suggested-areas.tsx`, `past-focus.tsx`).
- **The judges**, with Typesafe's decision model Jev (`typesafe/jev-1.14`), only through
  OpenRouter's Decisions API or the CMEM Pro gateway (`request-judge.mjs:49-50,176-191`). The
  request judge runs before grouping: a new conversation of the last 14 days that no single code
  link places gets a choice over its project's 19 most recently active open requests and none,
  then a yes/no check that it is the same work; at 0.8 it joins, from 0.5 it becomes its own request
  with a suggestion to join (Join or Not this: a worker route, no button on the 15.4 page), below
  it its own request; a failure leaves it to the grouping model, with no retry
  (`request-judge.mjs:27-35,115-144,225-299,306-348`). The focus judge asks, for each queued
  request, one choice over the kept areas and none, and gives the pick to the focus step as a hint;
  its threshold to place alone is null (`focus-judge.mjs:15-23,30-72`).

## What oboete has

- **The chain** (`provider::Chain`): every JSON call tries the configured entries in order, skips
  one resting, held or refused by its budget, and returns the first answer that passes the
  caller's check (`src/provider.rs`). Cooldowns, the three-failure breaker and the daily call count
  are per entry, shared by every role; the USD cap is one monthly pool for every role but
  embedding (`src/budget.rs`, `src/providers_db.rs`). The chain gates nothing: each caller gates
  what it sends.
- **Two roles today**, both from the resident worker: `curator` (a window) and `summary` (a turn,
  `src/turns.rs`). A worker round sends one call: the oldest due summary first, else a window
  ([summaries.md](summaries.md) T1). Hooks, view routes and capture never call a provider.
- **The op log.** raw.db's `ops` table holds what must survive a rebuild (window, claim,
  correction, turn, work_state and other kinds; a body is at most 64 KB). Consumers read it into
  knowledge.db; `oboete rebuild` replays it with no provider call (spec 1.7). Forget rewrites an op
  whose inputs it takes, as `Judge::turn` does for turn ops (`src/raw.rs`).
- **Typed prompts.** A prompt record is `prompt`; harness traffic with a known opening is
  `envelope` (`src/hook.rs`). Codex prompts another program sent carry `agent_sent`: an exec run,
  Codex as an MCP server, a session Claude Code's Codex plugin started, or a subagent's thread
  (`transcript::codex_agent_sent`). Nothing marks Claude Code's or Grok's.
- **The owner's words at session start**: the "Owner's directives" lines taken from typed prompts
  by marker words (D13, `src/manifest.rs`), and the global preferences only `oboete pref add` and
  the viewer make (spec 3.3), fenced as data (spec 6.5). `oboete mute` and `oboete correct` act on
  any claim.
- **My Work** ([my-work.md](my-work.md)): requests (`c:<agent>:<session>`, or `r:<request_id>`),
  their keyed version and the owner's check, the session summaries as their copy, and the work
  product read live. A My Work read calls no provider.

Volumes on this PC from 2026-10-15 (the switch) to 2026-10-25, counts only: 126 conversations
with a prompt, about 11 a day: 102 Codex, 14 Claude Code (106 prompts) and 10 Grok. Of the 294
Codex sessions started in that time, 3 were started by a person; the rest were `codex exec` runs,
subagent threads and Claude Code's Codex plugin.

## What oboete does

### A. Ground rules

A1. **The curator chain writes everything.** One new role, `my_work`, built as `summary` is: the
same entry list in its order, its cooldowns, the shared monthly USD cap, the caller's check and
gate (`Chain::new(&cfg.providers, db).paid_cap(..).check(..).gate(..)`). No provider of its own.
The ledger's span names the job (`group <root>`, `review <request>`, `links <request>`, the first
of its batch, `story <week>`, `rules`, `goals <day>`). The jobs run only while `[summary] curate` is on.

A2. **Never on a hook's path.** Only the resident worker calls. Hooks, MCP and the viewer read
what the worker kept. There is no refresh button: the viewer is a separate process, and claude-mem
has none either. The page shows a status line instead (V1).

A3. **One call a round, and its place.** A due check is a read of knowledge.db and providers.db,
no call. A round sends one call, in this order: the oldest due turn summary; then a My Work job
due for 60 minutes or more; then a window; then any due My Work job. So the jobs go in the owner's
pauses, and still within an hour while windows go a step at a time (decision 56). (Claude;
overrulable.)

A4. **Every call fits an 8,000-token request and a 4,000-token answer.** Each job's fixed
instructions stay under 1,500 estimated tokens, and its data within `[summary] window_tokens`
(5,000), as a turn summary's cards are: the free entries can then take every job. The data fills
in the order the job's rule gives, while it fits; what does not fit waits for the next call. An
answer is at most the entry's `max_output_tokens` (4,000 by default), so each job's batch size and
field caps below are set so its largest answer, written in Japanese (0.8 tokens a character),
fits. An answer cut at that limit fails as invalid and costs a call; caps that fit avoid it.
claude-mem's caps do not fit, so some are smaller here.

A5. **The gate, in and out.** Each piece of recorded text is gated alone before it is cut, as
`hook::clip` keeps a field: each prompt, each work-state value beside its key. Summaries and cards
come through their readers, gated when read (summaries.md T8). Before each send and each retry the
job takes the dispatch lock and checks the exclusion list, the forget fence and the rules' version
(`Reading::still`, `rules_still`), as a turn summary does; a change sends nothing. Each kept field
of the answer is gated again; a field over its cap is dropped, never cut. A prompt kept private,
an excluded session or repository, a forgotten or expired record, and a card or summary removed on
the page (card-hide.md) are never shown to a model.

A6. **Off until the owner turns it on, the estimate first.** `[my_work] curate`, false by default.
Its row on the settings page, and `oboete doctor`, show the estimate before anything is sent:
calls a day and USD a day, typical and at most, computed from this home's last 7 days (new
conversations, request-hours with new work, typed prompts) and priced at the first paid entry's
prices, with its model's name. The "at most" figure is the daily bound times one call's
reservation. Turning it on is a person's act, on the settings page.

A7. **At most 100 calls a day.** `[my_work] calls_a_day`, default 100, decision 57's figure. It is
counted from the ledger's rows of role `my_work` over a rolling 24 hours, a new role filter beside
`calls_in_a_day`. At the bound the jobs wait, held for budget. Money stays in the one monthly pool,
with no share kept back for cards. (Claude; overrulable.)

A8. **Waiting and giving up.** A job every entry fails waits in providers.db's `my_work_pending`
(its key, reason, hold kind, attempts, next attempt), as `turn_pending` does: again in 10 minutes,
hourly when only the owner can act. After `ATTEMPTS` (3) counted attempts it is kept as a skip op
and not asked again until its input changes. `oboete resume` and saving a key wake it, as they
wake `pending` and `turn_pending`.

A9. **A refused answer rests no entry for the curator.** An answer that fails a My Work check says
nothing about the entry's health for windows. Such failures do not count toward the entry's
three-failure breaker; 429s, outages and refused keys rest it for every role, as now. (Claude;
overrulable.)

A10. **Prompts and schemas.** Each job's instructions are fixed bytes, the fence sentence last
(`curate::FENCE_SENTENCE`), so the Anthropic entry caches them. Data sits between fence lines as
recorded text, never instructions. Each schema has `additionalProperties: false` and every key
required, and no length or count keywords, which the Anthropic entry strips. The check enforces
every cap and returns only outcomes the ledger counts (`shape`, `empty`, `prose`, `over_cap`,
`unanchored`). CLI entries get a system prompt per role, as safe as `CURATOR_SYSTEM`, whose first
sentence speaks of transcripts. Answers are in `[summary] language`.

### O. The owner's typed prompts (row X21)

O1. **What counts.** A record of kind `prompt` with its text stored (`store_prompts` on), not
private only, from a session neither excluded nor forgotten, and not marked as another program's
(O2). Envelopes, `directive` events and imported claude-mem history are not typed prompts here.

O2. **Marks at capture.** Codex keeps `agent_sent` as now. That rule is wider than claude-mem's: it
also takes the sessions Claude Code's Codex plugin starts, which claude-mem counts as the
person's. The Claude Code hook stores the session's entry point in the prompt's body, as Codex's
hook stores `agent_sent`: `CLAUDE_CODE_ENTRYPOINT`, which Claude Code sets in the environment of
its tools (`cli` in a terminal session on this PC). A prompt counts when it is `cli` or
`claude-desktop`, as in claude-mem, or the VS Code extension's value, which the probe names. A
transcript import reads each line's `entrypoint`, and marks a line whose `origin.kind` or
`turnOrigin` is not `human` (a task's notification, a scheduled turn, the Agent SDK's) as another
program's; claude-mem counts a line only when its `origin.kind` is `human`.
Whether hooks get the variable, and what `claude -p` and the SDK set it to, is slice 1's first
probe. If hooks do not get it, the hook takes the newest transcript line that carries
`entrypoint`, from one bounded read of the file's end, as `rollout_agent_sent` reads a Codex
rollout's first line.

O3. **Text tests, in every script.** claude-mem's two tests, with one change: normalizing keeps
the letters and digits of every script, lowercased after NFKC, so a Japanese prompt is tested too.
A prompt whose opening 120 normalized characters occur in a reply or a tool input recorded earlier
on this device (from 9 days before it), and that has 160 normalized characters or more and does
not open in lowercase, is an agent's. So is one that opens with claude-mem's role-assignment words
and runs past 400 characters. A delegated prompt is usually caught here: the agent that sent it
wrote it first in a tool input (a Bash command, a written prompt file). The tests run in the
worker when a job reads prompts, streaming the agent text with a rolling hash, as claude-mem's
importer does; they are code, so they need no op.

O4. **Where it is used.** Goals read typed prompts only (M1). A review takes a preference's quote
only from the request's typed prompts (P4). Grouping shows every conversation, labelling an
opening ask that is not typed `[agent prompt]`, as curation labels such a line.

O5. **What is still missing.** Grok, agy, OpenCode, Pi and Cursor carry no mark: their prompts
count unless a text test flags them. A delegated `grok --prompt-file` run is caught only when its
prompt was written in a recorded tool input first. A Claude Code prompt recorded before slice 1
has no entry point: it counts, unless a text test flags it, when a hook recorded it, and not when
only the transcript import found it, as an Agent SDK session runs no hooks (measured in As built);
31 days after slice 1 the goals window holds none. My Work's original ask still reads an agent-sent prompt as an ask, as claude-mem's
cards do (row X21). (Claude; overrulable.)

### C. One request across conversations (row X15)

C1. **Candidates.** A conversation (an agent's session) with an opening ask by My Work's rule
(two letters, not a greeting or an acknowledgement, in English or Japanese), active in the last 14
days, undecided, and naming no `request_id` in its work. A conversation that names request ids
keeps my-work.md's rule 1: those requests are named by its agent. (Claude; overrulable.)

C2. **A batch** is the undecided conversations of one repository (its `repo` key, which linked
worktrees share), up to 10, oldest first (claude-mem: 20). Each shows its key, agent, time, the
opening ask (400 characters, labelled per O4) and its work-state list names. With them, the
repository's open requests, each with its title, anchor ask (400 characters) and up to 3 recent
asks (200 each), and the proven links. The fill order (A4): the open requests a proven link names,
then the conversations, at least one, then the other open requests, newest first.

C3. **Proven links**, offered to the model, as claude-mem offers them: the same work-state list
(used by at most 8 conversations) or the same `request_id`; and, beyond claude-mem, a delegation:
a conversation whose opening ask O3 found in another conversation's earlier tool input is offered
to that conversation's request. (Claude; overrulable.)

C4. **The answer**, checked by code: each conversation in the batch decided once, as continue (an
open request's anchor listed first), new, or ignored with a reason (160 characters). A title of
120 characters at most. A group of two or more needs a link with evidence (160 characters), every
member reachable from the anchor. One repository per group. An open request is one not reviewed
as shipped or abandoned.

C5. **When.** A repository is asked about at most once every 30 minutes, all its undecided
conversations together. A decided conversation is never asked about again, as in claude-mem.
Continuing a request adds the new links to the earlier ones, where claude-mem replaces them. The
title is the newest the model gave. (Claude; overrulable.)

C6. **Its identity.** A group is `g:` and 12 hex digits of the keyed SHA-256 of its anchor
conversation, keyed with the home's own key as a masked id is (`state/my-work.key`). My Work shows
a group as one request: the earliest of its asks, the newest 6 follow-ups, all its conversations'
summaries, cards and work. An undecided conversation is shown as its own request, as now. The
group's title is the card's title, before my-work.md's rule 3; claude-mem's card never shows it.
(Claude; overrulable.)

C7. **Versions and the by-agent view.** A conversation joining a request changes the request's
version, as a new follow-up does, so a check is asked again; a model's title, review or link does
not. The by-agent view keeps one card a conversation (decision 55), showing its request's title.

### P. Progress and the build story (row X14)

P1. **The unit** is each request My Work shows from a group or a `request_id`. A lone undecided
conversation gets no review, as in claude-mem.

P2. **Due.** Never reviewed; or its conversations changed; or a new summary, card or work-state
write after the last review's input. And the last review at least 60 minutes old. claude-mem's
check every 10 minutes is the difference that keeps A7. A skip counts as a review at its time: due
again after new activity, an hour on. A forgotten review (S5) is none: due at once; one an
exclusion hides (S6) is passed over. (Claude; overrulable.)

P3. **Input.** The previous review, then the request's asks (the original and the follow-ups,
typed or labelled), its session summaries, its cards' titles, its work items' lists, tasks and
statuses, and its work product's commits, branches and pull requests (my-work.md slice 4), all
since the previous review when its conversations are the same ("amend"), newest first within A4.
One request a call (claude-mem: 8).

P4. **The review.** claude-mem's fields and code rules, with caps that fit one answer (A4): a
state, a percent (whole, 0 to 100), start, ship and last-activity dates, a summary (280
characters), a narrative (1,000; claude-mem 1,200), up to 8 dated milestones of 160 characters
(claude-mem 12 of 200), and up to 3 preferences, each a statement (160) and a quote (240;
claude-mem 10 of 200 and 300). Shipped needs percent 100 and a ship date; only shipped has one. A
preference's quote must be found in a typed prompt the review was shown, by curation's matcher
(`curate::locate`'s rules: exact, then whitespace and Unicode compatibility), or the preference is
dropped; one the previous review kept keeps its quote, found then. A mask in a quote is quoted as
shown: it stands for nothing. The outcome block, which claude-mem never shows, is not asked for.

P5. **On the card.** The newest two reviews of a request are its readings (claude-mem keeps 20:
As built 4b). The card shows "<percent>% · reviewed <time>" or "Completed <date>" for shipped,
nothing before a review (As built 4b), and "at this pace, about <date>" when the two readings rise
below 100. Reviews are at least an hour
apart, so two readings minutes apart cannot project today. A review newer than the request's
recorded status makes shipped `shipped` (a new status and tab, shown when it counts one) and 100%
`ready`. A status recorded after the review wins. The request's page shows the narrative and
milestones as "Build story".

P6. **The build story** is a recap (500 characters) and one chapter a week, Monday to Sunday in
local time (1,600 characters each), in the configured language. A run rewrites the recap and at
most two chapters, the newest weeks first, whose reviews' facts changed: state, percent, dates,
milestones or preferences, compared exactly, as claude-mem's story facts are. The page shows the
recap, and the chapters newest first up to 12,000 characters. claude-mem's one text, sent back
whole, would pass one answer's 4,000 tokens. (Claude; overrulable.)

P7. **When.** At most every 3 hours, when a fact changed; at once while weeks are left from the
last run. Unlike claude-mem, a story behind its reviews is due by itself. (Claude; overrulable.)

P8. **What reviews leave out.** A review's summary feeds the links and the story and is not
shown, as in claude-mem.

### L. Goal links

Replaced by F3 (decision 61): the placement job takes the links job's place, and no goal link is
read once it is built.

L1. **Due.** A request reviewed since its last link; every request once when a goal is added; a
request whose linked goal is gone. Up to 5 due requests a call (claude-mem: all at once).

L2. **Input and answer.** The current goals by id (kind, rank, title, product, without quotes),
and each due request's title, first ask (200 characters), state, percent and summary. Each
request gets up to three links to goal ids, each with evidence (160 characters; claude-mem 300),
or none. Code enforces the three, which claude-mem asks for in its prompt only.

L3. **By id.** A link names a goal by its id (M4), so a retitled goal keeps its requests. The card
shows the link to a priority first, else to an open ask, "No goal", or "Goal not linked yet".

### M. My Goals (row X12)

M1. **Input.** The owner's typed prompts (O1 to O3) of the last 31 days, each gated alone, cut at
3,000 characters, oldest first, each as `P<n> <local time> <agent> <repository>` and its text. The
code name is `owner_goals`, as `goals` already names what a turn op read.

M2. **When.** At most once a day, when a typed prompt came after the last goals; none came, no
call and no op. The first run reads the 31 days in chunks, oldest first, each chunk an update of
the goals so far that fits A4: about 10 to 15 calls once at this PC's volume. After that, one
update a day reads the new prompts with the current goals.

M3. **The answer.** claude-mem's update rules, shaped to fit one answer (A4). The input shows the
current goals by id, kind, rank, title, product, times asked and latest date, without their
quotes. The answer lists every item's id in rank order, names the ids it drops, and gives in full
only what is new or changed: up to 4 new items with up to 2 quotes each, and up to 4 changed items
with up to 1 new quote each. An item has a title (80 characters), a product, how often it was
asked, and quotes that each name their prompt (`P<n>`) and run under 240 characters. A summary of
the period (400 characters) heads the month in the history. Code carries the unchanged items and
keeps an item's newest 3 quotes. Up to 10 priorities and 10 open asks. No owner map: claude-mem
never reads its own.

M4. **Checked by code.** Each quote must be found in the prompt it names, by curation's matcher;
one that is not is dropped. An item left with no quote is dropped. Ids are given by code: a new
item gets one, a kept one keeps it. Ranks are made unique by order. "Asked" is at least the number
of distinct prompts its quotes name. Lengths are counted in characters, never in escaped bytes.

M5. **The window and months.** Current goals are the last 31 days, rolling. A quote older than the
window is dropped, and an item left with none. The goals of a finished calendar month are kept as
that month's: the last goals op written in it, with no call. (Claude; overrulable.)

M6. **Not elsewhere.** Goals are not in session start, search or what agents are given, as in
claude-mem.

M7. **The page**: V2.

### F. My Focus (decision 61, row X12)

F1. **Areas**, as claude-mem's: a random id (`fa_` and 12 hex digits), a name (80 characters, never a
page word: claude-mem's eight and the page's Japanese tab names), a goal (300), an about, a status
(suggested, kept, dismissed, done), a priority rank and an owner rank, a source (owner-goals,
clustering, promotion, owner), the area it was merged into, evidence, and who decided (owner,
system, owner-restore). Other is no area: it is the id `other`, used only by placements.

F2. **Placements.** A request sits in one live area, in Other, or is not sorted yet ("Sorting…"). A
placement is keyed by the request's anchor conversation, by its stored key, as claude-mem's: a
group's id is keyed by this home (C6) and changes when grouping moves a conversation. It resolves to
the request at read time through the groups (`my_work_group::groups`): a group is read by its
anchor's placement alone, as claude-mem reads its placements by a group's anchor; a member's own
placement is kept, and is its own again when it leaves the group (CodeRabbit on #746). The owner's
placement wins while its area is live; a merged area's placements follow the merge; a dismissed area
holds none.

F3. **The placement job** (job `place`, not `focus`: the runner reads a job's newest op about a
subject for due, skip and hold, and the owner's placement op has the same subject;
`my_work_focus::Focus` folds both jobs' ops in one time order), in place of the links job (L). A request is queued when it
has no placement, when its router placement points to an area that takes no requests now, or when
its version (C7) moved since this device's router placement. Up to 20 a call, in A4's room: the
kept and suggested areas (id, name, goal, about), the dismissed ones as never to propose again, and
each request's title, first ask (400 characters), state, summary, repository (a tag, never a
grouping) and its conversations' opening asks (400 each). The answer places every request shown
once, in a listed area or Other, with evidence (300); it may propose promoting one request that
holds several separate asks. Checked by code as claude-mem checks it: an unlisted or dismissed id
refuses the answer, never maps to Other.

F4. **The Other pass.** When the requests the router put in Other, with their versions, changed since
this device's last pass (claude-mem's mark is synced, so any device's pass counts; oboete's ops sync
with milestone 6), one call proposes new areas: two or more Other requests that are steps of
one larger thing (`new`), or one that holds several asks (`promote`), each with a name, goal,
about, its requests, the live area it is the same as or none, and evidence. A proposed area is
suggested. A dismissed area is never proposed again.

F5. **The goals job** (M) keeps its prompts, window and months. Its items become claude-mem's kinds,
focus areas and asks. It is shown the areas (id, name, goal, status; one the system withdrew left
out) and names, for each area it mines, the area it already is or none, never by name: a new one
is a suggested area (source owner-goals) with its rank; a match to a suggested area no owner
decided may update its rank and fill an empty goal. An ask sits under its area, else under Other.

F6. **The owner's actions**, through `save_gate` on `POST /api/my-focus`, each an op so it survives a
rebuild: keep, rename (a page word refused), edit goal, merge into a live area (not itself, not a
dismissed or merged one), dismiss, done, reorder (exactly the kept areas), move a request to a live
area or Other, and put back in Suggested (only an area the system suggested). Undo is the action
that puts the last change back, as claude-mem's page builds it.

F7. **The page**, in place of V2: My Work's "My focus" rail and chips (kept areas in order, Other,
"N suggested", Past focus), each with the count of its requests that need the owner; the area's
header (goal, the rollup of its reviewed requests, the status word by claude-mem's rules, shipped
count, times asked and the latest, its Not started asks, Why it's here, and the menu of F6's
actions); Other's header; the Suggested page with Keep, Rename, Merge and Dismiss, each with Undo;
Past focus (done areas, and the months from the goals ops, as V2's month by month); the card's
chip naming its area or "Sorting…"; and Move to… on the request's page. Both languages.

F8. **Kept** as ops of job `focus` (S1): an area op (subject its id, the area's fields), a placement
op (subject the conversation's stored key) and the Other pass's op (its set's digest), each a patch
(a null field removes it). The fold reads them oldest first with claude-mem's two rules. Not in
session start, as in claude-mem.

F9. **Not built**: claude-mem's seed from an earlier sorting and Cloud's areas at setup (row X22),
none of which any store here has, and the calibration script, which needs the owner's own
placements. No older op is read again in the new shape: no store holds a `links` or a `goals` op
(the owner's, and the dogfood user's two, counted 2026-10-28).

### J. The judges (decision 62, rows X12 and X15)

J1. **Jev**, Typesafe's decision model: answers with probabilities, not text. Called through
Typesafe's own API, `POST https://api.typesafe.ai/v1/systemone`, model `jev-latest` (it answered
as `jev-1.14.0` on 2026-10-28), with a key in a key file as `config::read_key` reads one (the token
on line 2), set on the settings page. Off until a key is set. Not in the curator chain: its own
`[judge]` table. Cloudflare's Workers AI serves `typesafe/jev` too, but answered "Insufficient
balance" without prepaid credit, so it is not built. Every state goes through the outbound gate
(S6); a conversation or request of a repository the owner excluded is never sent (spec 5.5),
checked before the call; each call is in the provider ledger; the spending cap counts its input
tokens at USD 0.042 a million (output is free). No retry, as claude-mem: what a failed call would
have decided goes to the model that decides it without a judge.

J2. **The context** of a conversation, as claude-mem's: its repository, when it started, its first 3
asks (250 characters each) and its first 8 card titles (120 each), each text gated; a request's adds
its title.

J3. **The request judge**, before grouping (C). For each candidate conversation (C1) a single proven
link to one open request (C3: the same work list, a delegation) joins it with no call.
Otherwise Jev chooses among the repository's 19 most recently active open requests and none, then
answers whether the conversation is the same work as the pick. From 0.8 it joins: a grouping op as
the model's, its link of kind `judge` with the scores as evidence. From 0.5 it is its own request,
with a suggestion to join. Below, or none, its own request. A failure leaves it to grouping.

J4. **Join or Not this**, on the card of a conversation the judge suggested: "Looks like part of
<title>", with Join and Not this, each an owner op. Join moves it, and anything it anchored, into
that request; Not this keeps it its own and it is never suggested again. claude-mem 15.4 has the
route and no button.

J5. **The focus judge**, before placement (F3), when a kept area exists: one choice per queued request
over the kept areas and none, 19 areas a question, more asked in sets and a final question over
the winners. Its pick is a hint shown to the placement model; it places nothing alone, as
claude-mem's threshold is null.

J6. **Measured** (2026-10-28, a made-up conversation and 19 requests of that size): a choice 9,378
input tokens in English and 14,222 in Japanese, a check 1,106 and 1,532, 0.16 to 0.25 s a call.
Both languages picked the continued request (0.74, 0.94) and judged it the same work (0.96, 0.97).
Jev's documentation says English is where it is best and CJK is handled less well: the owner's own
pairs measure it once it runs.

### R. My Rules (row X13)

R1. **Suggestions** come from the reviews' preferences, merged by a `rules` job that runs with the
story (P7) when a review added preferences. It is shown up to 10 new preferences and the current
suggestions by id, as many as the window holds, and says for each preference which suggestion it
joins, or gives a new statement (200 characters). The suggestion keeps its id and wording, so a
reworded preference is not a new suggestion, and a rejected one stays rejected. "Seen" counts the
requests whose preferences joined it, each once while it is among the newest 200 kept; its quotes
(up to 3) are theirs, checked in P4.

R2. **Your rules** are the decided preferences the owner made: `oboete pref add`, the viewer's, and
approved suggestions (R3). Global ones, and ones for one repository. A situation is written into
the rule as "When …: …", as claude-mem's file renders it.

R3. **The owner's actions**, each through `save_gate`, each an op so it survives a rebuild:
- approve: the statement is recorded as the owner's directive and a decided preference, as the
  viewer's `pref add` records one (spec 3.3: global scope only through it or the viewer). A rule
  for one repository is the same claim with that repository's scope. A `my_work` op records which
  suggestion it came from. The statement is the model's wording, which the owner may edit first;
  approving makes it the owner's, as `pref add` takes the owner's own;
- edit: `oboete correct`'s text correction of that claim;
- turn off and on: mute and unmute;
- reject, and undo of an approve or a reject: a `my_work` op. Undo works after a reload too;
- add: the viewer's `pref add`.
"Replace" is not built; claude-mem's page never sends it.

R4. **Delivery.** An approved rule is a decided preference, so it reaches session start in the
fenced "Global preferences" block, or, for one repository, in the "Preferences of this repository"
block after it, every one and not again among the decisions, in every agent: Claude Code, Codex
and Grok. claude-mem writes a file that Claude Code alone loads as instructions. No file is
written here (decision 60).

R5. **Forget and approved rules.** An approved rule stands on the owner's own directive, as a
`pref add` does. A forget of the prompts it was learned from takes the suggestion (S5), not the
rule the owner adopted.

### S. Where it is kept

S1. **One new op kind, `my_work`**, whose `job` is `group`, `review`, `links`, `story`, `rules`,
`goals`, `rule` (the owner's action), `focus` (My Focus, F8) or `place` (F3). Approved, edited and turned-off rules also use the
existing kinds: `claim` through `pref add`, and `correction` through correct, mute and unmute.
Checks stay in `state/my-work-reviews.json`. Nothing goes to `state/` or providers.db that a
rebuild needs.

S2. **The body.** The job, its key (repository, request, week or day), the entry that wrote it, a
time, `read` (the spans of this device's records it was shown, and the op seqs of the turn, window
and `my_work` ops whose text it was shown, the previous review or goals included), `removed` (as a
turn op lists it), and the fields kept, or `skipped`. The `rule` job holds no model text: the
suggestion's id, the action, and the claim's uid. At most 64 KB: an op over it drops its oldest
quotes first, keeping one an item, then is kept as a skip, as `turns::fitted` does.

S3. **The format a home earns.** `raw::FORMAT` rises from 4 to 5 (4 is the hide op's,
card-hide.md). The first `my_work` op stamps the home 5 in the transaction that appends it, as
retention's first expiry and the first hide op do (store-format.md). A home whose owner never
turned the jobs on still opens in the binary before.

S4. **The consumer**, `consumer::my_work`, keeps knowledge.db tables: the conversation-to-group
map, reviews, links, story chapters, suggestions, goals and kept months. The newest op per key
wins, the older marked replaced, as the cards consumer does. A rewind removes a device's rows
above its point. `oboete rebuild` makes the same rows with no provider call.

S5. **Forget.** A forget takes a `my_work` op when it takes a record in its `read` spans that the
op was shown, or an op it read: a turn or window op taken in the same pass, or an earlier
`my_work` op. Ops are judged in op order, so an update built on a taken op is taken too. Taken, an
op keeps its job and key, its fields blanked and a `forgotten` mark, as a turn op is, and the job
is due again. The backups are rewritten by the same rule. An op that lists no `read` is never
taken by this rule, as `Judge::turn` passes over a turn op without `from`; so every job but
`rule` must list it.

S6. **Exclusion, rescans, retention.** While an exclusion stands, every `my_work` op that read a
record of the excluded session or repository is hidden, and so is every op built on one, followed
through the same `read` lists as S5: a goal's title and product, a review's summary or a story
chapter written from the excluded prompts is shown nowhere and sent to no job, not only their
quotes. Each such job is due again at once for what the exclusion still allows, reading only
that; for the goals that is a first run (M2). A request of an excluded session or repository is
hidden too, and not reviewed. Taking the exclusion back shows the ops again, unless a newer op of
the same key replaced them meanwhile.
A new redaction rule masks `my_work` op text, as it masks every op's; its labels (job, ids, spans)
are kept from masking, as a turn op's are (`OP_NAMES`). Retention leaves the ops' text, as it
leaves summaries'.

S7. **Devices.** `my_work` ops are this device's, written from what it holds; they do not travel
until sync decides how goals from two devices meet. (Claude; overrulable.)

### V. The page

V1. **The status line**, on My Work, My Goals and My Rules: "Written <time>", "Waiting: <reason>"
(the daily bound, the monthly cap, every provider resting), or "Off" with a link to the setting.
Read from providers.db and knowledge.db, no call.

V2. **My Goals**, a tab after My Work, until F7 replaces it (decision 61). Tabs "This month · 31 days to <local date>" and "Month by
month". The lead sentence, top priorities (5, then the rest), the open asks panel and the "No
goal" list, as claude-mem's page. Each priority's status word from its requests' statuses, by
claude-mem's rules, with full dates for "Asked again after ship". The progress bar is the average
percent of its reviewed requests, with "k of n requests reviewed". Quotes with their local time.
Read-only, as claude-mem's.

V3. **My Rules**, a tab after My Goals. The suggestions waiting, strongest first (claude-mem's
strength: times seen over one plus the weeks since the newest quote), each with approve, edit
then approve, and reject. Your rules by scope, each with edit, an on/off switch and its quotes.
Add rule. One line on where rules go (R4).

V4. **My Work's card and page** gain P5's progress, the goal chip (L3) and the build story (P5);
the list gains `shipped`. Labels in Japanese and English, as the page's.

V5. **New data** arrives as the page's other data does: the store's version moves, and the page
reads again behind "Show updates" while a person reads (my-work.md slice 2).

V6. **No call from the page.** Every route reads; each write is one of R3's actions.

### Cost on this PC's volume

Claude Haiku 5.5 at its prices in `src/config.rs` (USD 0.10 a million input tokens, 0.50 output;
`platform.claude.com` pricing, checked 2026-10-09), tokens as `budget::estimate` counts them (0.28
a character, 0.8 a CJK character), before any cache saving. Typical, from the volumes above:

| Job | Calls a day | Tokens in / out, a call | USD a day |
|---|---|---|---|
| Grouping (C5) | 8 | 6,500 / 300 | 0.006 |
| Reviews (P2) | 20 | 4,000 / 1,000 | 0.018 |
| Goal links (L1) | 6 | 4,000 / 300 | 0.003 |
| Story (P7) | 4 | 4,000 / 1,700 | 0.005 |
| Rule suggestions (R1) | 2 | 2,000 / 300 | 0.001 |
| Goals (M2) | 1 | 4,000 / 2,000 | 0.001 |
| **Total** | **41** | | **0.035** |

At the bound, 100 calls each at its largest (6,500 in, 4,000 out): USD 0.27 a day, about USD 8 a
month. The first goals read adds about 10 to 15 calls once, about USD 0.02. Decision 57 estimated
about USD 0.20 and at most about 100 calls a day; the typical day is well under it. With a
subscription CLI first in the chain the money is USD 0, and the calls use its allowance and wait
while the owner works (decision 15). The real figures are the ledger's, by role, after a week.
From slice 10 the placement job (F3) takes the goal links' row, at about the same size, and the
Other pass (F4) adds a call when Other changes, about one a day. The judges (J) are Jev's, not
the chain's: at J6's sizes a judged conversation is about 10,500 to 15,800 input tokens, USD
0.00044 to 0.00066, and a focus pick is less; at about 9 new conversations a day, under USD 0.01 a
day. These jobs draw on the same monthly pool as windows and summaries. When the pool is spent, a paid
entry refuses them, and they go to the free entries and subscriptions, which A4 sizes every call
to fit.

## Differences from claude-mem

| | claude-mem 15.2 | oboete | Why |
|---|---|---|---|
| Who writes | the first logged-in agent CLI (Sonnet, Haiku) | the curator chain, its order, cooldowns and cap | decision 57 |
| When | every 10 minutes, whatever is due | one call a round; 30 minutes, 1 hour, 3 hours, 1 day per job | at most 100 calls a day (decision 57) |
| Cost | no record, no cap, no backoff | estimate before it runs, a daily bound, the ledger, holds | shown first |
| Default | on | off until the owner turns it on | money |
| Typed prompts | read from transcripts; text tests on a to z only | marks at capture; text tests in every script | Japanese prompts; delegated Codex sessions |
| Codex plugin's sessions | the person's | another program's | they are Claude Code's |
| Ask test | any non-ASCII text of two letters | not a greeting or acknowledgement, English or Japanese | Japanese filler |
| Goals window | 30 days read, "31 days" shown, UTC date | 31 days, local date | one number |
| Finished months | only by hand | kept automatically, no call | the page's history |
| Quote check | words anywhere in the same minute | in the prompt the quote names | exact |
| Item size | 2,000 characters counted in ASCII escapes | characters; 64 KB an op | Japanese |
| Goals answer | all goals again, open asks uncapped | changes only; 10 priorities, 10 open asks | one answer (A4) |
| Goal identity | its kind and title | an id | a retitle keeps links |
| Links a request | up to 3, in the prompt only | up to 3, checked | stated, so enforced |
| Owner map | written, never read | not written | unused |
| Grouping batch | 20 conversations, every open request | 10, the linked open requests first, then the newest, within A4 | fits every entry |
| Delegation | not a link | offered as a proven link | most Codex sessions are delegated |
| Continuing a request | replaces title and links | adds links | evidence kept |
| Group title on the card | never shown | the card's title | claude-mem shows none without OpenRouter |
| A conversation naming request ids | grouped; its explicit request is a card outside the group, unreviewed | not grouped; its requests are reviewed | one membership |
| Reviews | 8 a call, every check; 12 milestones, 10 preferences | one a call, at most hourly; 8 and 3 | one answer (A4); the bound |
| Review input | asks, observation titles, memory search | asks, summaries, card titles, work, work product | stateless calls |
| Outcome block | stored, never shown | not asked | unused |
| Story | one text up to 12,000, sent back whole | recap and weekly chapters | one answer's size |
| Story trigger | never by itself | due when facts changed | stale story |
| Suggestion identity | a hash of its wording | an id | rejections stick |
| Preference quotes | unchecked | found in a typed prompt | shown as the owner's |
| Rules delivery | a file Claude Code loads as instructions | decided preferences at every agent's session start | spec 6.5, decision 60 |
| Undo after a reload | none | an op | kept |
| Multi-device | rows sync | this device's ops | sync not built |
| Forget | none of derived text | ops taken with what they read | forget is oboete's |
| Cloud at setup (row X22) | goals, rules and groups from cmem.ai | none | decisions 56 and 57 |
| My Focus storage (15.4) | rows of the work-state list `my-focus` | ops of job `focus` | S1; work state is what agents read |
| Jev's route (15.4) | OpenRouter's Decisions API or the CMEM gateway | Typesafe's own API | the owner's key; no account of claude-mem's |
| Join or Not this (15.4) | a worker route, no button | buttons on the card | a suggestion needs an answer |
| A page word in an area's name | eight English words | those and the page's Japanese tab names | both languages |

## What needed the owner

Decision 60 settled it; slice 8 waited for it.

1. **Where approved rules go.** claude-mem writes approved rules to a file and adds one line to
   Claude Code's instructions file, so Claude Code alone obeys them as instructions. oboete's
   spec keeps memory as data (6.5) and lets only `pref add` and the viewer make a rule for every
   repository (3.3). Claude proposes: an approved rule becomes a decided preference, shown at the
   start of every session in Claude Code, Codex and Grok, as quoted data. Its words are the
   model's, which the owner approves or edits, and approving records them as the owner's own. The
   file is the exact copy of claude-mem; it puts memory outside the fence and changes spec 6.5.
   Decided 2026-10-27 (decision 60): the decided preference, as proposed.

Decided by Claude, overrulable, as their rules above say: the jobs are off until turned on, with
the estimate shown first (A6); one monthly pool, with no share kept back for cards (A7); the
goals' window is the last 31 days, rolling, and each finished calendar month is kept by itself
(M5); and the intervals that keep a day near decision 57's 100 calls (C5, P2, P7, M2).

## Slices (build order)

1. **The marks** (O1 to O3). The Claude Code entry point at capture and in the transcript import,
   and the text tests as a function. It needs nothing else, and goes first: a prompt recorded
   without its mark cannot get one after its transcript is gone (about 30 days). It measures, on a
   copy of this PC's store, how many prompts each test flags and how long 40 days of agent text take
   to stream. No model call.
2. **The job frame** (A1 to A10, S1 to S5). The role, the switch and its estimate on the settings
   page and in doctor, the daily bound, `my_work_pending` with resume and key saves, the breaker
   rule, the op kind with format 5, the consumer, rebuild, the forget and backup rules, and the CLI
   system prompt per role. It needs nothing; its jobs come with the next slices. It merges with the
   switch off.
3. **One card across conversations** (C1 to C7). Grouping, the group in My Work's answer, its
   version, the by-agent title. It needs slice 2.
4. **Progress** (P1 to P5, P8). Reviews, readings, the card's bar and estimate, `shipped` and
   `ready`, the request page's build story. It needs slice 3.
5. **Goals** (M1 to M7, V2). The goals job, the kept months, the My Goals tab. It needs slices 1
   and 2.
6. **Goal links** (L1 to L3). The links job, the goal chip, the priorities' requests and status
   words. It needs slices 4 and 5.
7. **The story and suggestions** (P6, P7, R1). The story's chapters and recap on My Work, the rules
   job. It needs slice 4.
8. **My Rules** (R2 to R5, V3). The page, the actions, delivery. It needs slice 7; decision 60
   settles the delivery.
9. **Removing a card or a summary** (decision 57's other part): reversible, as a correction is, in a
   note of its own ([card-hide.md](card-hide.md)), built first, after the page's forget (row 3).
10. **My Focus** (F1 to F9, decision 61). 10a: the focus ops, their fold, the owner's actions and
    `/api/my-focus`, no call. 10b: the goals job's kinds and matches, the placement job and the
    Other pass; the links job removed. 10c: the page, My Goals' tab removed. It needs slices 5 to 8.
11. **The judges** (J1 to J6, decision 62). 11a: the `[judge]` provider, its key on the settings
    page, its probe in doctor, the gate, the cap and the ledger. 11b: the request judge and Join or
    Not this. 11c: the focus judge's hint. It needs slice 3 and, for 11c, slice 10.

After slice 11, the side by side with claude-mem that decision 50 asks for.

## Tests

### Slice 1

- `a_claude_prompt_keeps_the_entry_point_its_hook_was_given` (O2)
- `a_headless_claude_prompt_is_not_typed` (O2; `claude -p`'s value from the probe)
- `an_imported_claude_line_is_typed_only_when_its_origin_is_human` (O2)
- `a_codex_prompt_the_codex_plugin_sent_is_not_typed` (O2)
- `a_prompt_whose_opening_an_agent_wrote_earlier_is_not_typed`, in English and in Japanese (O3)
- `a_prompt_that_opens_in_lowercase_or_is_short_is_not_fingerprinted` (O3)
- `a_role_assignment_opening_over_400_characters_is_an_agents` (O3)
- `agent_text_after_the_prompt_or_from_10_days_before_does_not_flag_it` (O3)
- `a_prompt_written_into_a_tool_call_is_found_across_its_lines` (O3)
- `an_envelope_a_private_prompt_and_an_omitted_one_are_not_typed` (O1)
- `a_grok_prompt_counts_unless_a_text_test_flags_it` (O5)

### Slice 2

- `no_hook_mcp_tool_or_view_route_calls_a_provider` (A2)
- `a_my_work_job_waits_while_its_switch_or_curation_is_off` (A1, A6)
- `the_settings_row_shows_the_estimate_before_the_switch_turns_on` (A6)
- `a_round_sends_one_call_in_the_order_summary_old_job_window_job` (A3)
- `every_job_request_fits_8000_estimated_tokens` (A4)
- `every_jobs_largest_answer_in_japanese_fits_4000_estimated_tokens` (A4; from the caps)
- `the_data_fills_in_the_rules_order_and_the_rest_waits_for_the_next_call` (A4)
- `each_piece_is_gated_alone_and_a_rules_change_since_reading_sends_nothing` (A5)
- `a_field_over_its_cap_is_dropped_never_cut` (A5)
- `the_daily_bound_counts_only_my_work_rows_over_24_hours` (A7)
- `a_job_every_entry_fails_waits_then_is_kept_as_a_skip_after_three` (A8)
- `resume_and_a_saved_key_wake_my_work_pending` (A8)
- `a_refused_my_work_answer_does_not_rest_an_entry_for_windows` (A9)
- `rebuild_makes_the_same_rows_with_no_provider_call` (S4)
- `a_forget_of_a_record_a_job_read_takes_its_op_and_the_job_is_due_again` (S5)
- `an_update_built_on_a_taken_op_is_taken_too` (S5)
- `a_backup_holds_the_taken_op_as_raw_db_does` (S5)
- `an_older_binary_refuses_a_home_with_a_my_work_op` (S3)
- `a_home_that_never_wrote_one_opens_in_the_older_binary` (S3)
- `an_op_over_64_kb_drops_its_oldest_quotes_then_is_a_skip` (S2)
- `a_new_rule_masks_my_work_op_text_and_not_its_labels` (S6)
- `the_status_line_says_off_waiting_or_written_with_no_call` (V1)

### Slice 3

- `a_conversation_is_decided_once_and_never_asked_again` (C5)
- `a_group_stays_in_one_repository_and_worktrees_share_it` (C2, C4)
- `an_answer_that_leaves_a_conversation_undecided_is_refused` (C4)
- `open_requests_fit_the_budget_linked_first_then_newest` (C2)
- `a_delegated_conversation_is_offered_to_its_senders_request` (C3)
- `continuing_keeps_earlier_links` (C5)
- `a_conversation_naming_request_ids_is_not_grouped` (C1)
- `a_group_is_one_request_with_the_earliest_ask_and_the_newest_six_follow_ups` (C6)
- `a_joined_conversation_changes_the_version_and_a_new_title_does_not` (C7)
- `the_by_agent_view_keeps_one_card_a_conversation` (C7)

### Slice 4

- `a_review_is_due_after_new_work_at_most_hourly` (P2)
- `shipped_needs_percent_100_and_a_ship_date` (P4)
- `a_preference_whose_quote_is_not_in_a_typed_prompt_is_dropped` (P4)
- `the_newest_two_reviews_are_the_readings` (P5)
- `a_hidden_or_another_devices_review_is_no_reading` (S6, S7)
- `the_estimate_needs_two_rising_readings_below_100` (P5)
- `a_status_recorded_after_the_review_wins` (P5)
- `a_review_at_100_makes_a_quiet_todo_or_unknown_request_ready` (P5)
- `a_recorded_ship_shows_100_and_its_day` (P5)
- `a_new_review_does_not_undo_a_check` (C7, P5)
- `the_build_story_is_the_reviews_narrative_and_milestones` (P5)
- `the_build_story_is_gated_as_it_is_read` (A5)
- `a_review_a_forget_took_is_no_reading_before_its_purge` (S5)
- `a_review_built_on_a_summary_or_a_card_a_forget_took_is_no_reading` (S5)
- `a_group_a_forget_took_decides_nothing_before_its_purge` (S5)
- `a_forget_past_its_rebuild_leaves_the_walk_off` (S5)
- `forget::tests::a_restore_bringing_text_back_owes_the_rebuild_again` (S5)
- `forget::tests::a_denial_whose_text_came_back_with_no_job_keeps_the_backstop` (S5)

### Slice 5

- `goals_read_only_typed_prompts_of_31_days` (M1)
- `no_typed_prompt_since_the_last_goals_makes_no_call` (M2)
- `the_first_read_goes_in_chunks_that_each_fit_the_budget` (M2)
- `a_quote_must_be_found_in_the_prompt_it_names` (M4)
- `a_kept_item_keeps_its_id` (M3, M4)
- `quotes_older_than_31_days_drop_and_an_item_with_none_goes` (M5)
- `a_finished_month_is_kept_with_no_call` (M5)
- `lengths_are_counted_in_characters` (M4)
- `the_label_uses_the_local_date` (V2)
- `goals_are_not_in_session_start_search_or_what_agents_are_given` (M6)
- `goals_that_read_an_excluded_repository_are_hidden_and_sent_nowhere_until_rewritten` (S6)
- `an_op_built_on_a_hidden_op_is_hidden_and_the_exclusion_taken_back_shows_both` (S6)

### Slice 6

- `a_link_names_a_goal_id_so_a_retitle_keeps_it` (L3)
- `a_request_gets_at_most_three_links` (L2)
- `a_new_goal_makes_every_request_due_once` (L1)
- `asked_again_after_ship_compares_full_dates` (V2)

### Slice 7

- `only_the_weeks_whose_facts_changed_are_rewritten` (P6)
- `a_story_behind_its_reviews_is_due_by_itself` (P7)
- `a_reworded_preference_joins_its_suggestion` (R1)
- `a_rejected_suggestion_stays_rejected_when_seen_again` (R1)

### Slice 8

- `an_approved_rule_is_a_decided_preference_in_every_agents_session_start` (R3, R4)
- `edit_turn_off_and_undo_survive_a_rebuild` (R3, S4)
- `undo_works_after_a_reload` (R3)
- `a_forget_of_a_suggestions_source_leaves_the_approved_rule` (R5)

## As built

### Slice 1: the marks (2026-10-26)

- **The entry point** (O2): the Claude Code hook stores `CLAUDE_CODE_ENTRYPOINT` in a prompt's
  body as `entrypoint`, after dropping any `entrypoint` the payload carries, so only the hook's
  environment says it. The probe: Claude Code sets the variable for every process it starts (its
  tools' and its MCP servers' environments hold `cli` in a terminal session on this PC), so the
  hook reads no transcript. Claude Code 2.1.310 names 27 entry points: `cli`, `claude-desktop` and
  `claude-vscode` count (`typed::TYPED_ENTRYPOINTS`); the others never do, among them `sdk-cli`
  (`claude -p`), `sdk-ts` and `sdk-py` (the Agent SDK), `mcp`, `remote_trigger` and
  `claude-code-github-action`. A value is kept when it is 1 to 32 lowercase letters, digits, `-`
  and `_`, the form Claude Code's own take; any other is dropped.
- **The transcript import** keeps a line's `entrypoint` on its prompt, and marks a line whose
  `origin.kind` or `turnOrigin` is not `human` as another program's. A queued prompt's enqueue
  line names neither: it takes the entry point the session's lines last named, and the marks of
  the line that delivers it. The marks are no part of a line's identity, so a transcript imported
  before them keeps its checkpoint and its records. A line with neither is not marked: on this PC the 2026-10 lines with neither are a compaction's summary, a
  local command or its output, a shell command typed after `!`, an interruption and another
  session's message, which the import leaves out or records as an envelope; an older Claude Code
  wrote neither. Meta and subagent lines were left out before.
- **What counts** (`typed::typed`): a record of kind `prompt` with its text, no `agent_sent`, and
  for Claude Code an entry point that counts. A Claude Code prompt with no entry point counts only
  when a hook wrote it (O5): on a copy of this PC's store, 1,584 of the last 31 days' 1,896 Claude
  Code prompts are single-prompt Agent SDK sessions (`sdk-py`) that only the transcript import
  recorded, as the SDK ran no hooks there.
- **The text tests** (`typed::AgentText`, O3): claude-mem's rolling hash over every position of
  agent text, each hit compared in full, normalized by NFKC, lower case and the letters and digits
  of every script. `typed::agent_text` reads a reply's text, and a tool's input with JSON's escapes
  of line breaks and tabs undone, as claude-mem undoes a line break's, so a prompt written into a
  command or a file is found across its lines.
- **Measured** on a copy of this PC's store (`typed::tests::measure_the_marks_on_a_copy_of_a_store`,
  a release build, 2026-10-26), the last 31 days' prompts:

  | Agent | All | Counted out by the marks | Typed by the marks | Checkable | Opening in agent text | Role assignment | Typed after the tests |
  |---|---:|---:|---:|---:|---:|---:|---:|
  | Claude Code | 1,896 | 1,584 | 312 | 27 | 10 | 0 | 302 |
  | Codex | 647 | 141 | 506 | 345 | 185 | 23 | 319 |
  | Grok | 34 | 0 | 34 | 12 | 9 | 2 | 25 |

  The 40 days of agent text before them, 287,417 replies and tool inputs of 182.7 million
  characters, streamed in 15.4 s, 5.1 s of it in the tests; reading the 31 days' prompts took
  10.8 s, both by one pass over every record. The peak resident size was 94 MB.
- **Not read yet**: no reader takes the marks; slices 3 to 5 do (grouping, reviews, goals). Until
  then only `typed::entrypoint` runs outside the tests, where a prompt is captured.

### Slice 2a: the providers' side of the job frame (2026-10-26)

- **`[my_work]`** in config.toml (`config::MyWork`): `curate`, false unless set, and `calls_a_day`,
  100 unless set. A key it does not know is an error, as in `[worker]`; recording's reader of the
  file passes the table over, as it passes every table of the curators' (CodeRabbit on #660). No job reads them yet; the settings page's row, with its estimate first (A6),
  comes with slice 2c, after the marks.
- **The role `my_work`** (`my_work::ROLE`). A CLI entry is told it turns a developer's recorded
  requests, session summaries and work state into the JSON My Work asks for
  (`provider::MY_WORK_SYSTEM`, A10); the rest is `CURATOR_SYSTEM`'s: no tools, the JSON alone, the
  records' text data.
- **A refused answer** (A9): an answer a My Work check refuses rests no entry and counts toward no
  breaker, as an answer that did not anchor leaves it (`Chain::run_report`).
- **The daily count** (A7): `providers_db::my_work_calls_in_a_day`, the role's requests of every
  entry in a rolling 24 hours, counted as an entry's daily budget is; a partial index on the role
  keeps it a seek.
- **Waiting** (A8): providers.db's `my_work_pending`, by the job's ledger span, holds the request's
  digest, the reason, the hold, the attempts and the next attempt. `oboete resume` wakes a job held
  for the owner, and a saved key one held for time or the owner, as they wake windows and turns.
  The table and the index are ones an older binary ignores: no format change.
- **Not yet**: the job runner (the due check, the round's order of A3, A5's gate, the wait at the
  bound), with the op kind (slice 2b) and the first job (slice 3).

### Slice 2b: the op, its consumer and its forget (2026-10-26)

- **The op kind `my_work`** (`raw::OpKind::MyWork`), its body `my_work_ops::MyWorkOp` (S2): the
  job (one of `my_work_ops::JOBS`), `about` (what it is about: a repository, a request, a week, a
  day or a suggestion), the entry that wrote it, its time, `read` (this device's record spans and
  the op seqs it was shown), `removed`, and the fields kept or `skipped`. `my_work_ops::fitted`
  keeps an op within the op cap: whole, else without its removals, else as a skip of its labels,
  else not at all; a job that drops quotes first does so before it.
- **The format** (S3): `raw::FORMAT` is 5; the first `my_work` op raises the home to 5 in the
  transaction that appends it, and a rebuild that brings one back stamps it too
  (docs/store-format.md).
- **Masking** (S6): `about` and `entry` join the op names no redaction rule masks (`job` was one);
  the fields are text, masked as any op's.
- **The consumer** (S4, `consumer::my_work`) keeps every op in knowledge.db's `my_work`, and
  `my_work_ops::newest` reads the newest of a job and subject, by time, then device and op seq,
  whichever device wrote it, so a rewind of one device leaves the other devices' as they were. The
  design's "older marked replaced" is that read: no mark is kept to undo. An op of a job this
  binary does not know is passed over. `oboete rebuild` makes the same rows with no provider call.
- **The forget** (S5): `Raw::record_ops` judges the `my_work` ops after the windows and turns, in
  op order, each by what it read: a forgotten record it was shown in its spans, or an op in its
  `read` whose text a forget took before or takes in the same pass (a window, a turn, or a My Work
  op judged before it, so an update built on a taken op is taken too). A taken op keeps its job,
  subject, entry, time and what it read; its fields go, it is a skip, and it holds the forget's
  job (`forgotten_my_work_body`), which the consumer keeps (`MyWorkOp::forgotten`), so the job is
  due again. The backup's ops segments and the copies of raw.db are judged by the same rule
  (`Judge::purged`): an op only they hold sees the window, turn and My Work ops only they hold
  that the forget takes, those a purge rewrote there before too, in op order (`Judge::widen`;
  Codex on #664), every body they hold under an op seq, since a restore that reused op seqs can
  leave a copy's differing from a segment's. A restore that gives an op its text back makes the
  purge go on again.
  The forget's preview does not count My Work ops yet.
- **Not yet**: the exclusion's rule (S6), which applies when a reader shows or sends an op, with
  the job runner; nothing writes a `my_work` op until the first job.

### Slice 3a: the job runner (2026-10-26)

- **`my_work_run::Runner`**, made by the worker for each curation round from `[my_work]` and the
  chain, runs the jobs `my_work_run::jobs()` names: none yet; the first, grouping, comes next. A
  job (`my_work_run::Job`) says which of its subjects are due and since when (a read, no call),
  and what it sends for one (`Ask`: the prompt, the schema, what it shows, the check, and what an
  answer keeps). The runner does the rest.
- **The order** (A3): `turns::round` asks for the oldest turn summary due; then, when it sent
  none, a job due an hour or more; then a window; then, when no window was sent, any job due, the
  one due the longest first. A round sends one My Work call at most, whatever it answered.
- **The switch and the bound** (A1, A6, A7): nothing runs while `[my_work] curate` is off, and the
  worker reaches a round only while `[summary] curate` is on. At `calls_a_day` calls of the role in
  the rolling day (`providers_db::my_work_calls_in_a_day`, which now gives the oldest call's time
  too), every job waits until the oldest leaves it, without keeping the worker up. The bound is
  checked again at each request's gate, so a chain's retry or fallback past it is not sent either
  (`BoundReached`; Codex on #668): the job waits with its attempts as they were.
- **The gate** (A5): before each send and each retry the job takes the dispatch lock and checks
  the exclusion list, the forget fence and the rules' version, as a turn summary does; a change
  sends nothing and keeps nothing, and the next pass reads again. The answer is appended under the
  forget fence the job read with, and a job waits while knowledge.db lags a forget.
- **Waiting and giving up** (A8): a job every entry fails waits in `my_work_pending` by its span,
  for the hold `curate::hold` gives it (a refused answer is tried again at once and counted), and
  after `ATTEMPTS` counted attempts is kept as a skip op of what it read. A request that changed
  (its digest, with the chain's) is asked whatever an older one waits for.

### Slice 3a-2: grouping (2026-10-26)

- **The job** (`my_work_group::Grouping`, S1's `group`), the first `my_work_run::jobs()` names.
  Its candidates (C1): a conversation whose opening ask, by My Work's rule, is a prompt of this
  device, last active in the 14 days before, not decided, and whose work names no request id.
  `my_work::seen` gives what grouping reads of a conversation, through `gather`, the first half of
  the page's read, which the page and the job share (the page's answer is unchanged).
- **A call** (C2): one repository by its stored key; up to 10 candidates, the oldest first, each
  with its agent, time in UTC, the work lists it wrote and its opening ask (400 characters); the
  repository's open requests (every group of it, until reviews close some), each with its title,
  opening ask and three recent asks, the opening ask not repeated; and the proven links (C3): a
  work list written by two to eight of the repository's conversations, read from every write a
  call of theirs sent (`my_work::writers`), each request by its member that wrote it. They fill
  `[summary] window_tokens` in A4's order. Every recorded text is gated alone before it is cut.
  The prompt is the same for the same records (lists and members in one order), so a wait is kept
  by its digest (A8).
- **[agent prompt]** (O4): an opening ask the marks count out (O2), or whose opening the text tests
  find in agent text recorded in the nine days before it (O3), is labelled. The scan starts at the
  record of that time, found by a search over the seqs, and reads to the end of the device's
  records.
- **The check** (C4): every C label decided once; an R label only first, to continue that request;
  a group of two or more joined by links naming its own labels, with evidence, that reach every
  member from the first; evidence over 160 characters refuses the answer (`over_cap`), a missing
  link or a member not reached `unanchored`, the rest `shape`. A title over 120 characters or a
  reason over 160 is dropped from what is kept, never cut, and each kept text is gated again.
- **When** (C5): a repository is due when it has a candidate the newest op about it was not shown,
  from 30 minutes after that op. A skip shows its batch, so a repository given up on is asked again
  when another candidate comes, the given-up ones with it.
- **The op** (S2): `about` is the repository; `read` lists the prompt records the call showed and
  this device's grouping ops behind the open requests it showed (their members, titles and links,
  so a forget that takes one takes this op too: Codex's security review of #669), its removals as
  a turn op's; the fields are `groups` (each its conversations by stored key, the
  anchor first, its title or none, its links) and `ignored` (each conversation and its reason or
  none).
- **The fold** (`my_work_group::groups`): every grouping op in time order, as S4's "newest wins"
  for a request's title. A later decision moves a conversation; continuing a request adds its links
  where claude-mem replaces them. S4 named a knowledge.db table for the map: it is this fold over
  the ops the consumer keeps, because S6 must leave ops out when they are read. An op shown a
  record of a session or repository excluded now is left out while the exclusion stands, and so is
  every op built on one through its `read.ops` (Codex on #669), so what they decided is undecided
  again and asked again; a forgotten op decides nothing (S5).
- **The budget** (A4): the first conversation takes its room first, whatever it costs, and the
  linked requests, the other conversations and the other requests are shown only in the room it
  leaves; a conversation shows 20 of its work lists at most, then how many more, where claude-mem
  shows them all (Codex on #669).
- **Measured** on a copy of this PC's store (`my_work_group::tests::measure_grouping_on_a_copy_of_a_store`,
  a release build, 2026-10-26), before any grouping: 126 conversations read in 2.3 s cold and the
  due check in 0.19 s warm; 124 candidates in 7 repositories, so about 13 calls to decide them all,
  each repository at most every 30 minutes; one call's prompt 650 to 1,440 estimated tokens, read
  in up to 6.4 s, most of it the agent text scan; a peak of 79 MB.
- **Then**: the delegation link (C3), slice 3c below.

### Slice 3b: a group on My Work (2026-10-26)

- **One request** (C6): a conversation that names no request id and that grouping put in a group
  is shown as that group's request, read through `my_work_group::groups` with the exclusions as
  they stand (S6), so an op shown an excluded record groups nothing while the exclusion stands. Its
  ask is the earliest of its conversations', its follow-ups the newest six of all of them, and its
  summaries, cards and work all of theirs, as a request of a `request_id` shows them; work that
  names no request id follows its conversation into the group.
- **Its id** is `g:` and the first 12 hex digits of the keyed SHA-256 of its anchor conversation
  as stored (`g:<agent>:<session>`), keyed with the home's own key as a masked id is; it tells
  nothing of the conversation. A check is filed under it (rule 6).
- **Its title** is the newest grouping gave it, gated as the page reads it, before my-work.md's
  rule 3; a group with none takes rule 3's.
- **Versions** (C7): the version a check is of holds the request's conversations, so one joining
  is a new version, as a new follow-up is; a new title is not part of it.
- **The agents view** (C7) keeps one card a conversation, each its own id, showing its request's
  title, as the page already does for a request of several conversations.

### Slice 2c: the switch and its estimate (2026-10-26)

- **The settings page** has a My Work section after the summaries', and the simple page a block
  after its AI list: the estimate first, then the switch (`[my_work] curate`) and, on the full
  page, the daily bound (`calls_a_day`, 1 to 10,000). Both say it runs only while summaries are on.
  An older page that sends no `my_work` leaves both as the file has them; a bound out of range is
  refused (`range`, `my_work.calls_a_day`), on the page before it is sent.
- **The estimate** (`my_work_run::estimate`, A6), from this home's last 7 days: a repository's new
  conversations of one half hour count one grouping call; a conversation's hours with prompts one
  review each, and the links, story and rules as the cost table derives them from the reviews; a
  day with prompts one goals update, typed or not. Each job's calls are priced at the sizes of the
  cost table, at the first paid entry of the chain the worker builds (an entry off, or turned off
  in `[chain]`, is not priced), whose name and model are shown; over the bound every job waits
  alike. "At most" is the bound, each call the largest request A4 lets a job send (its
  instructions and the configured `window_tokens`, or the cost table's largest when that is more)
  and the entry's largest answer. Both count a call's input at the entry's learned factor
  (`budget::factor`), as admission does (Codex on #671). With no paid entry it says so and shows
  no USD. None before anything is recorded.
- **Drafts**: the full page's switch and bound, edited and not saved, stay through another save
  on the page, the simple page's included, with the estimate that save's answer brings (Codex on
  #671).
- **Doctor** prints the switch, "on, waiting for [summary] curate" while summaries are off, and the
  same estimate (`my_work_run::doctor_line`; the page and doctor share `home_estimate`).

### Slice 3c: the delegation link (2026-10-26)

- **Who sent an ask** (C3): the text tests (O3) look for every opening ask of a batch in the
  agent text before it, marked or not, since a Codex session another agent started is marked at
  capture and is the commonest delegation. A tool's input that holds an ask's opening names its
  conversation as the ask's sender (`typed::AgentText::sender`): the one, other than the ask's
  own, that held it last before the ask in the lookback. A reply names none: it may only print a
  prompt for the person to send. Which asks are labelled `[agent prompt]` (O4) is as before.
- **A call that waits** (#692): a call that starts another agent and waits for it, such as
  `codex exec '<ask>'`, is recorded when it ends, after that agent's opening ask. So a tool's
  input is compared at its call's start (`typed::written_at`), in O3 as in C3. The hook keeps a
  shell call's start at its PreToolUse (Claude Code's and Codex's `Bash`, Grok's
  `run_terminal_command`), under the call's id, and puts it in the call's record as `started`;
  an imported call's start is its call's line time, which leaves the line's identity as it was. A
  tool's record after a batch's last ask is read for its start up to a day after that ask.
- **The link**: a conversation to decide whose sender is decided in the same batch is linked to
  it; a sender that belongs to an open request links that request, which is then shown with the
  requests other proven links name (A4). The line keeps the direction: `- C1 sent C2's opening ask
  in a tool call`, or `- a conversation of R1 sent C2's opening ask in a tool call`. It is charged
  to the room and ordered as the work-list links are (#677, #686), and the instructions name an ask
  one sent the other among a link's evidence.
- **Not covered**: an ask shorter than O3's floor (160 normalized characters, or one that opens in
  lowercase) has no fingerprint, so no sender, marked or not. A call whose PreToolUse gives no call
  id, or that ran over a day, is compared at its end, so it sends nothing it started. The sender is the older of the two,
  so it is decided first or in the same batch; one decided before as ignored, or in no open
  request, gives no link.
- **#689** (with this slice): a work item that waited for its group's request (#686) takes its
  place in the request's work, newest first, from which the request's next step is read.

### Slice 4a: the review job (2026-10-27)

- **The job** (`my_work_progress::Progress`, S1's `review`) runs in the worker's runner after
  grouping. Its unit (P1) is each request My Work shows from a group or a request id, with
  activity in the last 14 days, none of whose conversations is excluded now (S6):
  `my_work::review_inputs` reads them as the page does (`build`, `parts`), without its answer.
  S6's walk is one function every job reads (`my_work_ops::hidden`), grouping too.
- **Due** (P2): never reviewed, due since it was asked; its review forgotten, due at once; a
  review an exclusion hides is passed over. Else due an hour after the last review, on a change of
  conversations or on anything the last review could not read. A review keeps marks of what it
  could read beside the model's fields: its conversations' digest, a number, since a redaction rule
  added later masks every string of a kept op's fields; and of this device's records and ops, the
  newest prompt, summary and card it could see, by seq; and when it read the work items, whose
  time is their write's. Seqs, not times: a summary's time is its reply's and a card's its window's
  last record, not when they were written, so a summary written after a review, of a turn that
  ended before it, would never count by time. A skip has no marks (the runner writes it): it waits
  for activity after its time, so such a summary does not wake it (a `shortcut:` in the code).
- **The call** (P3): one request. The previous review first, whole, whatever it costs; the original
  ask; then later asks, session summaries (what was completed, what is next), cards' titles, work
  items and work product, each part the newest first, each piece cut (the original 600
  characters, an ask 300, a summary's field 200, a title 120) and at most 8 to 12 pieces a part. A
  piece that does not fit is passed over for a smaller one. With the same conversations ("an
  update", claude-mem's amend), the parts show only what the previous review could not read, and
  not the original ask, which it read; else all. The data names today's date, not the time: the
  same records make the same prompt, whose digest a wait is kept by (A8).
- **What does not fit is lost**: the marks move past it, so no later review reads it. After a
  previous review at its caps (about 3,100 estimated tokens in Japanese), about 1,800 of the 5,000
  are left in an update: some 7 later asks at their cut, or 5 summaries. The trade is the newest
  over the whole: a review always reads what came last, and the oldest asks of a busy hour may
  never be read.
- **S6 for what a review read**: `my_work_ops::hidden` leaves out an op that read a record of an
  excluded session (any record of a span, `Raw::holds_session`), or a summary or a card of one.
  Summaries and cards are judged by their session, as their readers show or keep them back (D13):
  a review left out for one the readers still show would be due again at once, call after call.
  They are found in knowledge.db's `turns` and `cards`, one query a table a walk, not by reading
  each op a review lists; a card of two sessions has none, and no reader shows it.
- **This device only** (S2, S7): a review lists what it was shown by this device's seqs, so it
  shows only this device's prompts, summaries, cards and previous reviews, and the products read
  from this device's records or cards, or from work items. The original ask is listed by where it
  was read from: a prompt record, a summary's turn op, or a work item, which no forget takes. A
  product is listed by its call's record or its card's window op (`my_work::Origin`; the cards'
  files now name their op). A request shows only the prompts it owns: one that owns some prompts
  of a conversation shares the others on the page, not in its review.
- **Labels and quotes** (O4, P4): the asks shown and the original are tested as grouping tests
  opening asks (`my_work_group::agent_written`: the marks, then O3's text tests over the agent text
  in the lookback); one an agent wrote is labelled `[agent prompt]`. A preference is kept only when
  its quote, unchanged by the gate, is found in a typed ask as it was shown, by curation's matcher
  (`curate::compatible`: whitespace and compatibility forms aside), or among the quotes of the
  previous review's preferences, found when it kept them. A mask is quoted as shown: a record's
  capture masks each character with `*` and the gate shows `[REDACTED]`, and neither shows what it
  hides.
- **The answer** (P4, A10): a state of the five, a whole percent from 0 to 100, three dates as
  YYYY-MM-DD or an empty string, kept as null (no nullable types in the schema, which some
  entries' strict modes refuse), a summary, a narrative, milestones and preferences. The check
  refuses a wrong shape or date, shipped without percent 100 and a shipped date, or a shipped date
  on another state (`shape`), and more than 8 milestones or 3 preferences (`over_cap`). The keep
  drops a text over its cap, with the milestone or preference it is part of.
- **Measured** on a copy of this PC's store (`my_work_progress::tests::measure_review_on_a_copy_of_a_store`,
  a release build, 2026-10-27), with five grouping ops written to the copy only, since this home
  has never grouped: four requests to review, read in 2.1 s cold and 0.2 s warm, the due check in
  0.2 s; one call's prompt 1,240 to 3,216 estimated tokens, read in 1.1 to 6.8 s, most of it the
  agent text scan; a peak of 80 MB.
- **#692 (2)**, with this slice: the tool record that proves a delegation is in the grouping op's
  read spans, so a forget of it takes the op. **#692 (1)**, a blocking call's start time, came
  later: slice 3c's "A call that waits".
- **Then**: slice 4b, the readings, the card's percent and estimate, shipped and ready, and the
  build story on the request's page.

### Slice 4b: the card's progress and the build story (2026-10-27)

- **Readings** (P5): `my_work_progress::readings`, this device's newest two reviews of a request
  that were answered, not forgotten (S5) and not left out (S6): the newest's percent and the pace
  of the two. claude-mem keeps 20; its page uses their count only to tell a reviewed request from
  one not yet reviewed, and draws no history of them. The page's read walks S6 once for all its
  requests (`my_work_ops::hidden`).
- **Which review counts** (claude-mem's `reviewIsNewerThanLifecycle`): the newest, unless the
  request's status was recorded after it (`request_status` or `request_complete`, by its write's
  time). One that counts makes shipped `shipped`, and 100% `ready` from in progress, to do, no
  known state or quiet (`normalizeRequestStatus`). A recorded shipped or delivered is its own
  state now, `shipped`, apart from done, as in claude-mem 15.2; the Shipped tab shows when it
  counts one, after Checked.
- **The progress** in My Work's answer (claude-mem's `progress`): the percent of the review that
  counts, or of the newest below 100 when a status was recorded after it; 100 for shipped; when it
  was reviewed; for shipped, the day: the newest recorded status's when it completes the request
  (done or shipped, claude-mem's `completedAtEpoch`), whatever review comes after it, else the
  review's shipped date; and the estimate, none for shipped. None with no review and no ship.
- **Checked** shows over a result ready to check only, as in claude-mem 15's
  `normalizeRequestStatus`: a request checked before a review said it shipped is Shipped, with its
  day.
- **The estimate** (claude-mem's `measuredEstimate`): when the two readings rise below 100, the
  time the line through them reaches 100. A day already past is shown as it is, as claude-mem's is.
- **The card** shows a meter with "70% · reviewed 1h ago", or "Completed Oct 26" for shipped, and
  "At this pace, about Oct 27". It shows no "Not reviewed yet": with the switch off no request is
  ever reviewed and every card would say it, as decision 55 leaves out the rows claude-mem shows as
  "Not recorded".
- **The build story** on the request's page: the narrative and the milestones, the oldest first,
  each day as claude-mem's work view shows it ("Oct 25"), of the review the progress shows; none
  when it has neither. P6's recap and chapters come with slice 7. Its texts are gated as the page
  reads them, so a rule added after the review masks them before any rescan.
- **C7**: a review is no part of a request's version, so a new one undoes no check.
- **A forget before its purge** (spec 6.2's local purge, step 1, the backstop; S5): from a forget's
  tombstone to its purge, S6's walk (`my_work_ops::hidden`) also leaves out an op shown a record
  the forget took that the op does not list as removed, or a summary or a card its reader leaves
  out for one (T7, K4: `turns::taken`, `cards::taken`), and every op built on one. So no review's
  progress or story, nor a group's title, made from a forgotten record is shown, nor sent as a
  previous review, until the purge marks the op forgotten (Codex's review of this slice; 4a's
  previous review had the same gap). The walk runs while a forget waits for its rebuild (step 3,
  `Raw::forget_unrebuilt`: a finished forget stays in `forget_jobs`, past it), or a session is
  excluded. A restore that brings a forget's text back sets its job back to step 2 at once
  (`forget::owe_rebuild_again`, as its purge would at its next look), so the walk runs again until
  the purge marks what it took again; a forgotten record whose text a restore from segments alone
  brought back, with no job row, keeps it running too, until a purge takes the text again. A rescan's masks are the gate's: every text is gated with the rules as
  they are when it is read.
- **The review's age** on the card is a time each poll's tick rewrites, as the card's own.
- **On the screen** (Playwright, a home of made-up records, 2026-10-27): the cards' meter, percent
  and estimate, the Shipped badge and tab, and the request's build story, in English and Japanese.
- **Then**: #692 (1), provenance slice 2 (#690), then slice 5, goals.

### Slice 5a: the goals job (2026-10-27)

- **The job** (M1, S1): `my_work_goals::OwnerGoals`, op job `goals`, its subject the local date
  (S2's day). It reads this device's live prompts of the last 31 days that the marks tell typed
  (O1, O2), none of an excluded session's (S6), each gated alone and cut at 3,000 characters, the
  oldest first, each headed `P<n> <local time> <agent> <repository>`. The text tests (O3) run in
  the call, not in the due check, since they read agent text.
- **When** (M2): a typed prompt the last goals op did not read is due at once when that op stopped
  at the budget with prompts left (a first run's next chunk; the op says so in `more`, which a skip
  kept in its place over the op cap keeps too, #719), else a day after the op, a prompt typed while
  the call was out too (#716).
  A first run reads in chunks within `[summary] window_tokens`; a first prompt that does not fit is
  cut to the room left, to at most 40 characters when the current goals take it all, so each call
  reads at least one. A call whose prompts the text tests all flag sends nothing: what it read is
  kept as a skip, with no call (`my_work_run`: an ask with no prompt), so M2's "no op" means no
  goals op.
- **The answer** (M3) gives the order of the current ids, the dropped ids, up to 4 new items with
  up to 2 quotes, up to 4 changed items with 1 quote, and a summary. Code keeps (M4, M5): it
  carries the items the answer leaves alone and applies the changes; a quote must be found in the
  prompt it names (`my_work_progress::quoted`, curation's matcher) and keeps that prompt's seq and
  time; an item with no quote goes; each keeps its newest 3 quotes of the window; `asked` is at
  least the distinct prompts its quotes name; ids are `g<n>` from a counter kept in the op
  (`next_id`); ranks follow the answer's order, unique within each kind, at most 10 priorities and
  10 open asks. Caps are characters: title 80, product 80, quote 240, summary 400. The largest
  answer in Japanese fits the 4,000-token answer (a test).
- **Months** (M5): `my_work_goals::months`, the last goals op of each finished local month, read
  with no call; the page shows it (slice 5b).
- **Not elsewhere** (M6): the goals are in knowledge.db's `my_work` alone, which search and
  session start do not read (a test).
- **Exclusion** (S6): `my_work_ops::hidden` hides a goals op that read an excluded session's
  prompt, and every op built on it; the next call is a first run, and its prompt leaves the
  excluded prompts out.
- **Measured** on this PC's store, read only (2026-10-27): 287 live prompts in 31 days, 152 typed
  by the marks and 135 marked as an agent's; the typed ones, each cut at 3,000 characters, come to
  about 16,900 estimated tokens, so the first read takes about 5 calls. No model call: `[my_work]
  curate` is off here.
- **Then**: slice 5b, the My Goals page (V2) and its months view.

### Slice 5b, the reads (2026-10-28)

- **`/api/my-goals`** (V2, `my_work_goals::page`), no call: the current goals are this device's
  newest answered goals op's, each item with its quotes of the last 31 days and their times, the
  newest first. An item with no quote left is not shown, and the ranks are counted again within
  each kind, since the op can be older than the window (M5: the job runs only while `[my_work]
  curate` is on). With them: the prompts the goals read in the window and in this local month,
  and each finished month's goals (`months`), the newest month first, with its summary, its
  priorities and each one's newest quote, and the prompts read in that month. The prompts are
  counted from the read spans of the answered goals ops S6 shows, by one query that reads no
  body (`raw::prompt_times_of`): a prompt a forget took is not counted, one retention expired
  is. Every text goes through the gate as it is read, so a rule added later masks it (S6).
- **The status line** (V1, `my_work_run::status`), no call, from config.toml, providers.db and
  knowledge.db: `off`; `summaries_off` (on, `[summary] curate` off); `waiting` at the daily bound
  (A7, with the bound and when the oldest call counted leaves the day) or on the hold the runner
  last left one of the page's jobs in (A8), `budget`, `resting` or `owner`, until its next attempt;
  else `written` at the newest answered op of those jobs this device wrote, or `none`. The runner
  keeps that hold in providers.db's `my_work_round`, a row per job (#720): set when it finds the
  job's request held or a call fails, the earliest of a round's holds kept (#724), the one the
  runner waits for; removed when the round finds nothing due now for the job, asks it with
  success or gives it up, or passes its last subject due now with no call and none held (#724).
  A subject an overdue round does not reach keeps the row. A held request whose subject has gone,
  as a goals request of an earlier day, so no longer reads as waiting. A ledger
  older than My Work holds nothing it waits on (#680). `/api/my-goals` carries it for the goals
  job, and `/api/my-work/status` for My Work's other jobs, a route of its own so that the
  requests' answer stays as the page's held updates compare it. V1 was in slice 2's tests but not
  built then.
- **Then**: the page itself (V2's tabs and panels), and V1's line on My Work.

### Slice 5b, the page (2026-10-28)

- **My Goals** (V2), a tab after My Work, as claude-mem's page without slice 6's links:
  - **Two inner tabs**: "This month · 31 days to <local date>" and "Month by month".
  - **This month**: the top 5 priorities, then "<n> more priorities" behind a disclosure. A row
    gives the rank (rank 1 stands out), the title, how often and when it was last asked, and the
    product. "Why it's here · <n> quotes" opens to the quotes, each with the time it was typed, to
    the minute. Then the open asks, and how many of them came up more than once. Last, the prompts
    the goals read in the window, read on this machine from what oboete recorded.
  - **Month by month**: this month (Current), its top priority and its prompts, then each
    finished month (Archived), the newest first. A finished month opens to its summary and its
    priorities, each with its newest quote, and says that no request review covers it, as
    claude-mem does before its links.
  - **No goals yet**: what turning My Work on brings, and the status line.
- **The status line** (V1) on My Goals and on My Work. When off, or when summaries are off, it
  says so and opens Settings; when waiting, it says why and when the next try is; else, when it
  last wrote. My Work reads its line after its requests, into a place of its own, so the line is
  no part of the requests' held updates.
- **Redraws**: a poll that brings the same answer draws nothing, and a redraw keeps the inner tab,
  the quotes and months opened and "more". A language switch draws the page again, its dates
  too. Every text is put in as text, never as markup.
- **Checked** in a browser (2026-10-28, a debug build over a home of made-up goals, its providers
  empty): both inner tabs, the disclosures and the status line, in English and in Japanese.
- **Reads** (#722): the page reads again each minute, as My Work does, so that a wait that ended
  and a quote that left the window go with no change of the version, and the inner tab's date
  follows midnight. Of two reads of My Work's status line in flight, only the later one's answer
  is drawn. Each drawing, a reread's and a new day's too, gives the keyboard's focus back to the
  control it was on when that control is drawn again (#722, #724): an inner tab, a goal's or a
  month's disclosure, "more", the status line's button.
- **Then**: slice 6.

### Slice 6a: the links job (2026-10-28)

- **The job** (L1, S1): `my_work_links::Links`, op job `links`, its subject the request (S2's
  key), as the review job's. Its units are the requests the review job reads (P1: activity in 14
  days, none of an excluded session's, S6) that have a review this device shows; a request never
  reviewed stays "Goal not linked yet". The current goals are the newest goals op with items this
  device shows (M5); with none, nothing is due, as in claude-mem (`request-groups.mjs:459-466`).
- **Due** (L1): never linked, since its review; its links forgotten (S5), at once; reviewed since
  its last links op, since that review; a goal added, or a goal it links gone, since the goals op.
  Each links op keeps two numbers for this, as a review keeps its marks: the review op it read and
  the highest goal number (`g<n>`) it was shown, since ids are given from a counter (M4). A goal
  retitled or re-ranked keeps its id and makes no request due (L3); claude-mem keys a goal by its
  title, so a retitle is a new goal there. A skip (A8) waits for a new review or new goals.
- **The call** (L2): the goals by id, kind, rank, title and product, without quotes, whatever they
  cost; then the request the runner chose and up to four more due, the oldest first, as far as
  they fit `[summary] window_tokens`, each labelled `R<n>` with its group's title (when every
  grouping op behind it is this device's), its first ask cut at 200 characters, its review's state,
  percent and summary. claude-mem also shows each request's previous links; L2 leaves them out.
- **One answer, an op for each request**: the runner keeps a batch's answer as one op per subject
  (`Ask::subjects`), in one append, each listing everything the call was shown (S2): the goals op,
  each request's review op, first ask and grouping ops. So a forget or an exclusion of one
  request's record makes the batch's requests due again (S5, S6). A give-up keeps a skip for each.
  The ledger's span is `links <request>`, the request the runner chose (A1 said `links`); the
  prompt's digest covers the rest, so a batch that changes is a new input (A8).
- **The answer** gives each request's goals, at most three, each an id and evidence. The check
  refuses a request missed, named twice or not shown (`shape`), more than three goals
  (`over_cap`), and a goal not shown or named twice (`unanchored`). The keep gates each evidence
  again and drops one empty or over 160 characters, keeping its link (A5).
- **Then**: slice 6b, the reads and the page.

### Slice 6b: the reads and the page (2026-10-28)

- **Joined in the page**, as claude-mem's My Goals reads its goals' requests from the work model
  (`my-goals.tsx` `useGoalRequests`, v15.3.0): the tab reads `/api/my-work` beside
  `/api/my-goals`, so a request's state there is the one My Work shows, and computes the rest.
- **Each My Work row** gains `goals` (L3, V4): none when no answered links op is shown or each goal
  it names is gone ("Goal not linked yet"), an empty list when it was linked to none ("No goal"),
  else the current goals it links, each its id, kind, rank and title as the gate shows it, the
  priorities first by rank. The answer counts the current goals (`goals`); while there are none,
  no card shows a goal.
- **The status word**, claude-mem's `statusOf` over its `presentStatus`: needs you, blocked and
  checked are the owner's move; ready is done but not checked; working and to do work; closed and
  quiet are idle; an unknown one updated in 6 hours works. No request: "Nothing on it"; any the
  owner's move: "Waiting on you"; any working or not checked: "Moving"; all shipped: "Asked again
  after ship" when the goal was last asked on a later day than the last ship, by full dates (V2;
  claude-mem compares the month and day alone), else "Shipped"; else "Stalled".
- **The rollup**: the average percent of the requests serving it that have a review, with "k of n
  requests reviewed" and the titles it leaves out; "Not reviewed yet" with none reviewed; "No
  requests, so no progress to measure" with none.
- **The page**: the lead over the top 5 priorities and the open asks nothing serves, the one asked
  most named; each priority opens to the requests serving it (each opens on My Work) or "Start a
  request for this"; the open asks split into "Nothing is working on these" and "Being worked
  on", a part with none left out; "No goal" lists the requests linked to none. A goal on a card
  opens My Goals on it, in view, with the focus on it. Until the links job has decided any request,
  the tab is as slice 5b drew it: no word can say what serves a goal.
- **#728** (Codex on #727): the due requests go in batches of five, the oldest first, each asked by
  its first, so a batch held after a failure keeps its requests out of the others while it waits.
  The current goals, for the job, its due check and the cards, are those My Goals shows: each
  goal with a quote in the 31 days, ranked again within each kind; a goal whose quotes all left is
  gone, so a request linked to it is due again and no card names it.
- **Checked** in a browser (2026-10-28, a debug build over a home of made-up requests, reviews,
  goals and links, its providers empty): the lead, the words, the rollups, a request opening on
  My Work with its goal, the cards' goals and a card's goal opening My Goals.
- **Then**: slice 7, the story and the rule suggestions.

### Slice 7a: the build story (2026-10-28)

- **The facts** (P6): each request's newest answered review this device shows, one only, as the
  card's. Each milestone goes in the local week (Monday to Sunday) of its date, the start and ship
  days in theirs, the state and percent in the week of the review. A week with no facts has no
  chapter.
- **The ops** (S1, S2): one a chapter, its subject the week's Monday (`YYYY-MM-DD`), and one the
  recap (`recap`), written together from one answer (`Ask::subjects`). A chapter keeps its text
  and the facts it was told from as a number, a digest that a later redaction rule cannot change,
  as a review's marks.
- **Due** (P7): a week with facts and no chapter, a chapter forgotten, or a digest that differs
  from its week's facts now; a skip waits for a newer review. The newest such week is the subject,
  the next one rides along. Due at once with none written, or while weeks are left from the last
  run; else three hours after the last run once a fact changed after it, where claude-mem's stale
  story starts no run.
- **The call** (A4): the recap so far, then each week's chapter so far and its facts, the newest
  first, then each request's review summary (labelled `R<n>`) and the other weeks' counts, as far
  as `[summary] window_tokens` go. What does not fit is lost, as a review's input.
- **The answer** gives each week shown its text and the recap. The check refuses a week missed,
  named twice or not shown (`shape`), an empty text (`empty`), a chapter over 1,600 characters
  and a recap over 500 (`over_cap`): the text is the op, so dropping it would keep nothing.
- **The page** (V4): My Work's answer carries `story`, the recap and the chapters, the newest week
  first, as far as 12,000 characters go, each gated as it is read; none before one is written.
  My Work's top shows it: the recap, then the chapters behind one disclosure.
- **#730** (CodeRabbit on #729): a batch's subjects each wait as their batch does. When every
  entry fails a call of a batch (`Ask::subjects`), providers.db's `my_work_held` keeps each subject
  with the batch's hold and next attempt; the links job leaves a held request out of every batch,
  as a lead or a member, until its wait ends, so batches cut again cannot ask it early. The runner
  reads the holds at each pass: a job with a subject held keeps its status line's row on the
  earliest, a success of another batch included, and waits for it. A success or a give-up of the
  batch clears its subjects; `oboete resume` and a saved key wake them as they wake
  `my_work_pending`.
- **#731** (Codex and CodeRabbit on #729): "Asked again after ship" compares the ask's day and the
  ship day both in UTC, as the ship day is recorded. A request opened from My Goals drops a read
  of its work product still out, and opens at its top with the focus on its heading. A card's goal
  opens "more" by the goal's rank in the answer My Goals draws, not the card's.
- **#733** (the final round of reviews of #732):
  - A story call asks only for the weeks whose heading it shows, so an answer that writes each
    week shown passes the check; with none shown, there is no call.
  - The story job keeps the holds of a failed batch, as the links job does: a held chapter is in no
    batch until its wait ends, and while the recap is held, which every batch rewrites, none is due.
  - A week whose facts a newer review took away has no chapter on the page.
  - The story so far that a call is shown is the newest answered recap and chapter. A give-up's
    skips still decide when the story is due.
  - A give-up that leaves another subject of the job held keeps the status line's row on the
    earliest of them.
  - The keyboard on the build story's disclosure counts as reading, so a changed read waits behind
    "Show updates".
- **Then**: slice 7b, the rule suggestions.

### Slice 7b: the rule suggestions (2026-10-28)

- **The job** (R1, S1): `my_work_rules::Suggestions`, op job `rules`. Since #737 each suggestion
  is an op of its own, its subject its id (`s<n>`), and one more op, subject `through`, says the
  newest review op whose preferences were merged: one op could not carry them all within the op
  cap (a suggestion with 50 requests and three Japanese quotes is about 5 KB). A suggestion keeps
  its wording for good, `seen` (how many requests' preferences joined it), the newest 200 of those
  requests (a request is counted once while it is among them), and their newest three quotes, each
  with its review's time (`at`, which My Rules ranks by, V3). A call writes `through` and the
  suggestions it changed, each gated again as it is written, carried text too. A new id is past
  every `s<n>` an op of this device names, the owner's `rule` ops and purged ops included (a purge
  clears fields, never the subject).
- **The new preferences**: those of each request's newest review this device shows past
  `through`, the oldest review first, whole reviews while ten fit; the rest are due at once. Due
  with the story's cadence (P7): at once with none merged, else three hours after the last run once
  a review added preferences, and at once while any preference is left from before the last run,
  newer ones beside it or not (#734). A skip waits for a newer review.
- **The call**: the current suggestions by id, each with how many requests saw it and the owner's
  decision, then the new preferences labelled `P<n>` with their statement and quote. Each answer
  says which suggestion a preference joins, rejected ones included, or gives a new statement of at
  most 200 characters. The check refuses a preference missed, named twice (by its number:
  `P01` is `P1` again) or not shown (`shape`),
  a join of an id not shown (`unanchored`), and a new statement empty (`empty`) or over 200
  (`over_cap`). The keep gates a new statement again; two new ones of one wording are one. A
  suggestion keeps the newest three quotes by their reviews' times, in whatever order the answer
  named its preferences (#734).
- **The owner's decision** is never the rules op's: it is the newest `rule` op of the suggestion's
  id (R3, slice 8 writes it: `action` approve, reject or undo), read by `my_work_rules::decided`. So
  a rejected suggestion a preference joins again stays rejected, where claude-mem's suggestion,
  whose id is a hash of its wording, comes back reworded.
- **Then**: slice 8, My Rules.

### Slice 8a: My Rules, rules of every repository (2026-10-28)

- **The page** (V3): a My Rules tab after My Goals, read from `/api/my-rules` with no call
  (`my_work_rules::page`): the rules job's status line (V1); the suggestions waiting, strongest
  first by claude-mem's strength (the requests that saw it over one plus the weeks since its
  newest quote, in whole days; Light, Steady and Strong at 1.5 and 4), each with how often it was
  seen, its quotes with their times, and Approve, Edit then approve, and Reject; the owner's rules
  for every repository (`claims::global`), on ones first, then the newest, each with Edit, an on/off
  switch, its quotes and, for one approved from a suggestion, Undo approval; the rejected
  suggestions, each with Undo; Add rule; and one line on where rules go (R4). Every text through
  the gate as it is read (S6). The answer keeps rules by scope (`yours: [{scope, rules}]`), so
  slice 8b adds one.
- **The actions** (R3), `POST /api/my-rules` through `save_gate` (`settings::claims::rule`):
  approve, with the suggestion's words or edited ones, and `scope` `global` (it stands for
  `/api/preferences`' confirmation, spec 3.3); reject; undo. An approve records the preference as
  `pref add` does (`claims::pref_add_recorded`, its caps and refusals), then the `rule` op naming
  its uid; when that op cannot be written after the preference was, the answer says so
  (`rule_partly_recorded`), so the owner does not approve it twice. An undo of an approve retracts that preference (`correct` with status `retracted`;
  one forgotten since has nothing left to retract), then writes the undo op; the suggestion waits
  again, and a second approve makes a new preference. A decided suggestion is refused another
  approve or reject (409), an undecided one an undo. Edit, on/off and add are the existing
  `/api/claims/correct`, `/api/claims/mute` and `/api/preferences`. After an approve or a reject
  the page shows what it did with Undo, and the rule's Undo approval and the Rejected list undo it
  after a reload too.
- **Delivery** (R4, decision 60): none of its own. An approved rule is a decided global
  preference, in the "Global preferences" block of every agent's session start.
- **My Work's status line** leaves the rules job out, as it leaves the goals job.
- **#736, #737** (the reviews of #734): a call shows the new preferences first (whole reviews, at
  least one), then the suggestions while `[summary] window_tokens` holds them: the rejected ones
  first, so a preference saying one again joins it and it stays rejected, then the undecided
  strongest first, then the approved. A join is of an id shown; a new statement matching one left
  out still joins it by its wording. The runner writes an op for each subject a keep gives beyond
  the ones it waits on; links and the story give exactly theirs.
- **#739** (the reviews of #738): an action on a suggestion whose last decision is recorded but not
  applied yet is refused (`rule_pending`, 409), so an approve is not sent twice; only an applied
  answer closes a form or offers Undo, and a partial one says what it kept; a write that ends
  after the owner opened another tab draws nothing over it; and `/api/my-rules` keeps its uids
  through the answer gate, as the routes that hand claim uids to the page do.
- **Then**: slice 8b, a rule for one repository.

### Slice 8b: a rule for one repository (2026-10-29)

- **Made** (R2, R3) as `pref add` makes a global one, with the repository named: `oboete pref add
  --repo <key>` (`.` for the current directory's), `/api/preferences` with `repo` and
  `apply_to_all_repos` false, and My Rules' approve with `scope` `repo` and its `repo`. The
  directive event is labelled with that repository alone (no folder, no branch) and its body's
  scope is `repo`; its claim op's scope is `repo` (`claims::preference_op`), and the purge that
  writes a masked preference again keeps it. Refused before anything is appended: a repository no
  record and no rule of the owner's names (`preference_repo_unknown`, `claims::owned_repos`: #748)
  and one the owner stopped recording, as every writer
  asks it (`preference_repo_excluded`). The directive is no session's work: it makes no manifest
  fact, so it is never an active session or a record waiting for curation.
- **The owner's own** (`claims::owned`): a repository's current claims whose active derivation is
  `pref add`'s. A preference the curator found in the same repository is not one: it stays among
  the decisions, and My Rules does not list it.
- **Delivery** (R4): session start gives every one of the owner's preferences of the session's
  repository, not turned off, under "Preferences of this repository" after the global ones, and
  leaves them out of the decisions and the index.
- **The page**: `yours` holds every repository's rules first, then each repository's with its key
  and name, each under its heading. Edit then approve and Add rule have "Applies", as claude-mem's
  dialog does: every repository, or one of the 20 repositories sessions started in most recently,
  none the owner stopped recording; Approve as it is stays every repository's. An approved
  suggestion whose rule is gone (retracted with `oboete correct`, or forgotten) is listed under
  "Rule removed" with Undo.
- **#743** (the reviews of #742): each form keeps its own "Applies", and a chosen repository
  stays offered when a reread leaves it out; a repository's directive has a session of its own
  (`owner:<repository>`), and no directive is a raw search hit or a session start; a repository
  name the gate masks is refused (`preference_repo_masked`), and so is one whose directives'
  session it masks; and the decisions' pool of 200 counts no owner rule. A chosen repository the
  list left out is shown as "the repository you chose", never by its name, which a rule added since
  may mask.
- **#741** (the reviews of #740): a suggestion's op keeps its fields within half the op cap, the
  oldest requests and then the oldest quotes going first, `seen` keeping its count; and a My Rules
  write that ends on another tab reads nothing again and shows no message there.
- **Then**: My Focus (decision 61).

### Slice 10a: My Focus's ops and the owner's actions (2026-10-28)

- **The ops** (F8): job `focus`, one of S1's jobs. An area op is about the area's id, its fields
  `kind: area` and the area's own (F1); a placement op is about the request's anchor, as the
  stores hold its id (`Id::held`, not the id keyed by the home's key: CodeRabbit on #746; a
  group's is its anchor conversation's, `c:`: `my_work_focus::subject`), its
  fields `kind: placement`, `area` (an id, `other`, or none to clear it) and `by` (`router` or
  `owner`). Each is a patch: a null field removes it.
- **The fold** (`my_work_focus::Focus`): every focus op oldest first (time, device, op seq), past
  those an exclusion leaves out (S6) and the forgotten ones (S5). claude-mem's two rules: a system
  row is skipped once the owner decided the area, until an owner-restore; the owner's placement
  wins while its area is live, else the router's. A merge forwards through every merge; a loop, an
  unknown area or a dismissed one resolves to nothing. A row that would leave an area unreadable
  (no name, an unknown status, source or decider, or the id `other`) is not applied, where
  claude-mem's fold throws.
- **My Work's answer**: each request's `focus` (its area's id, `other`, or null: not sorted yet),
  and `areas`, the areas not merged away, by the owner's rank, the goals' rank, then the name,
  every text through the gate.
- **The owner's actions** (F6), `POST /api/my-focus` through `save_gate`, `{action, area, into,
  name, goal, order, request}`: keep, dismiss, done, rename (80 characters; a page word is
  `focus_page_word`, 422), edit goal (300), merge (`focus_merge_self`, `focus_merge_target`),
  reorder (exactly the kept areas, `focus_order`), move a request (by the id My Work shows) to a
  live area or Other (`focus_move_target`), and put back in Suggested (not an area the owner wrote,
  `focus_restore`); any action but that one on a merged area is `focus_merged` (409); an unknown
  area or request `focus_not_found` (404). Each writes its change alone, decided by the owner, a
  patch on the area, through `my_work_ops::record`, which the `rule` op shares: claude-mem's
  `ownRow` writes the whole area, which would keep the model's text in an op a forget or an
  exclusion cannot follow (Codex on #746). A name and a goal go through the gate as they are
  written. An area's id and each request's `focus` pass the answer's gate as selectors, and the
  area a placement or a merge names passes the op text rescan (`raw::masked_op_text`, #747).
- **No call.** Areas come with slice 10b, the page with 10c.
- **Then**: slice 10b, the placement job and the Other pass (10b-1), the goals job's areas (10b-2).

### Slice 10b-1: the placement job and the Other pass (2026-10-28)

- **The job** `place` (`my_work_place`), one of S1's jobs, after the links job in
  `my_work_run::jobs`. It places the requests `my_work::review_inputs` gives for 14 days: groups and
  request ids, S6 applied. A lone conversation is not placed, as claude-mem places its groups only.
- **The basis** of a placement: a number from the digest of the request's conversations and the
  time of its original ask, the parts of C7's version that move it. Its title and review are a
  model's text and are left out; claude-mem's basis also holds the title. A number, as the review
  job's digest is (`my_work_progress`), since a rule added later masks a string of a kept op's
  fields.
- **Due** (F3, claude-mem's `focusDue`): never while the owner's placement is in a live area (Other
  and done areas too); a request with no placement; one whose router placement's area takes no
  requests now; one whose newest place op is this device's and of another basis (another device's
  is not compared); after a refused call, once the request has newer activity. A request a failed
  batch holds waits (#730).
- **The call** (F3), 20 requests at most within A4's room, the first whatever it costs: the areas
  that take requests (id, status, name, goal, about), the areas the owner dismissed by name, never
  to propose again, and each request's title, first ask (400), its other conversations' opening
  asks (400 each, 8 at most), state and summary of its review, and repositories. Its read (S2): the
  asks' records, the titles' grouping ops, the reviews, and this device's ops of every area shown,
  so an exclusion of a repository hides the placements made on what it showed.
- **The check** (A10), as claude-mem's validation: each request shown placed once (`shape`), in a
  listed area or Other; a dismissed id is `dismissed`, an unlisted one `unanchored`, never taken for
  Other; evidence of 1 to 300 characters (`over_cap` above); a proposal of kind `promote` with one
  request (no `new` while placing), its name of 1 to 80 characters and no page word (`page_word`),
  its goal and about of 300 at most, each request in one proposal, `same_as` a listed area or empty.
- **What it keeps**: each request's placement about its anchor (`kind: placement`, `area`, `by:
  router`, `basis`, `evidence`); a proposal with no `same_as` makes an area under a new id (`fa_`
  and 6 random bytes in hex), suggested, `decided_by: system`, sourced `promotion` (or `clustering`
  from the Other pass), its texts gated again, and its requests placed in it; one with `same_as`
  places its requests in that area.
- **The Other pass** (F4), subject `other`, while no request is queued: the requests the router put
  in Other, none the owner placed. Due when the digest of their subjects and bases differs from this
  device's newest `other` op, or, after a refused pass, once one of them is placed again; a pass a
  failure holds waits (#730). One call
  shows the areas and the set as far as they fit; it places nothing and proposes `new` (two or more
  requests) or `promote` (one). Its op keeps the digest of the set less the requests its proposals
  took, as claude-mem's `otherPassHash`; a set larger than one call waits for the set to move.
- **Not built here**: the goals job's areas (10b-2), the page (10c), the focus judge's hint (11).
  The links job stays until 10c removes My Goals with it.
- **Then**: slice 10b-2, the goals job shown the areas.

## Not built

- claude-mem's owner map, its outcome block, "Replace" on My Rules, and the files of each day's
  prompts.
- The setup page's goals, rules and groups from claude-mem Cloud (row X22).
- A refresh button: the viewer would need a way to ask the worker.
- Grouping of a session in a scratch folder by the repository its tool calls touched: it stays its
  folder's.

## Stop rule for this document's review

This PR merges when the review of its final head adds no finding that changes an owner decision or
a rule here. After two consecutive rounds whose findings are mechanism only, the rest moves into an
issue as acceptance tests for the implementing PRs, each finding is answered and resolved, and the
PR merges without a further commit (CLAUDE.md, "Design-doc PRs").
