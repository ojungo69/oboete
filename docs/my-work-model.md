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
The ledger's span names the job (`group <root>`, `review <request>`, `links`, `story <week>`,
`rules`, `goals <day>`). The jobs run only while `[summary] curate` is on.

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
check every 10 minutes is the difference that keeps A7. (Claude; overrulable.)

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
(`curate::locate`'s rules: exact, then whitespace and Unicode compatibility, outside masked
ranges), or the preference is dropped. The outcome block, which claude-mem never shows, is not
asked for.

P5. **On the card.** The newest 20 reviews of a request are its readings. The card shows
"<percent>% · reviewed <time>", "Completed <date>" for shipped, or "Not reviewed yet", and "at
this pace, about <date>" when the last two readings rise below 100. Reviews are at least an hour
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

### R. My Rules (row X13)

R1. **Suggestions** come from the reviews' preferences, merged by a `rules` job that runs with the
story (P7) when a review added preferences. It is shown the current suggestions by id and up to
10 new preferences, and says for each which suggestion it joins, or gives a new statement (200
characters). The suggestion keeps its id and wording, so a reworded preference is not a new
suggestion, and a rejected one stays rejected. "Seen" counts the requests whose preferences joined
it; its quotes (up to 3) are theirs, checked in P4.

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
fenced "Global preferences" block, or the repository's claims, in every agent: Claude Code, Codex
and Grok. claude-mem writes a file that Claude Code alone loads as instructions. No file is
written here (owner: item 1).

R5. **Forget and approved rules.** An approved rule stands on the owner's own directive, as a
`pref add` does. A forget of the prompts it was learned from takes the suggestion (S5), not the
rule the owner adopted.

### S. Where it is kept

S1. **One new op kind, `my_work`**, whose `job` is `group`, `review`, `links`, `story`, `rules`,
`goals` or `rule` (the owner's action). Approved, edited and turned-off rules also use the
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

V2. **My Goals**, a tab after My Work. Tabs "This month · 31 days to <local date>" and "Month by
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
These jobs draw on the same monthly pool as windows and summaries. When the pool is spent, a paid
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
| Rules delivery | a file Claude Code loads as instructions | decided preferences at every agent's session start | spec 6.5 (owner: item 1) |
| Undo after a reload | none | an op | kept |
| Multi-device | rows sync | this device's ops | sync not built |
| Forget | none of derived text | ops taken with what they read | forget is oboete's |
| Cloud at setup (row X22) | goals, rules and groups from cmem.ai | none | decisions 56 and 57 |

## What needs the owner

The code can be built without it; slice 8 waits for it.

1. **Where approved rules go.** claude-mem writes approved rules to a file and adds one line to
   Claude Code's instructions file, so Claude Code alone obeys them as instructions. oboete's
   spec keeps memory as data (6.5) and lets only `pref add` and the viewer make a rule for every
   repository (3.3). Claude proposes: an approved rule becomes a decided preference, shown at the
   start of every session in Claude Code, Codex and Grok, as quoted data. Its words are the
   model's, which the owner approves or edits, and approving records them as the owner's own. The
   file is the exact copy of claude-mem; it puts memory outside the fence and changes spec 6.5.
   The owner chooses.

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
8. **My Rules** (R2 to R5, V3). The page, the actions, delivery. It needs slice 7 and the owner's
   item 1.
9. **Removing a card or a summary** (decision 57's other part): reversible, as a correction is, in a
   note of its own ([card-hide.md](card-hide.md)), built first, after the page's forget (row 3).

After slice 8, the side by side with claude-mem that decision 50 asks for.

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
- `the_newest_20_reviews_are_the_readings` (P5)
- `the_estimate_needs_two_rising_readings_below_100` (P5)
- `a_status_recorded_after_the_review_wins` (P5)
- `a_new_review_does_not_undo_a_check` (C7, P5)

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
