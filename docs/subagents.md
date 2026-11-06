# Subagents: their stops, their memory at start, and their state on My Work (parity rows X17, X26)

claude-mem 15.1 to 15.10 records when a Claude Code subagent finishes, shows each subagent's state
on My Work's agent view, copies the files a subagent made, and gives a fresh Codex subagent its
memory when it starts. oboete does the same (decision 70's order: X17 and X26 first). Read against
claude-mem at `origin/main` 4f88958cf (15.10.0 and 26 fixes after it):
`src/cli/handlers/subagent-stop.ts`, `src/cli/handlers/subagent-context.ts`,
`src/cli/adapters/claude-code.ts`, `src/cli/adapters/codex.ts`, `src/shared/hook-spool.ts`,
`src/services/worker/hook-spool-drain.ts`, `src/services/worker/http/shared.ts`
(`ingestSubagentStop`), `src/services/work-view/agent-lifecycle.mjs`,
`src/services/work-view/agent-view.mjs`, `src/shared/subagent-predicate.ts`,
`src/ui/my-work/src/live/to-work-model.ts`, `src/ui/my-work/src/pages/my-work.tsx`,
`src/cli/deliverables-capture.ts`, `plugin/hooks/hooks.json` (Apache-2.0).

## What claude-mem does

- **A subagent's stop (row X17).** Claude Code's `SubagentStop` runs `hook claude-code
  subagent-stop` (asynchronous, 30 s). With a session id and an agent id (the id a non-empty string
  of at most 128 UTF-16 units, `pickAgentField`; the agent type the same, else absent; the session
  id is not bounded), in a tracked folder, it keeps the
  session, the platform, the agent's id and type and the hook's time, nothing of the subagent's
  text: one closed row of the work-state list `agent-lifecycle` (`stop_source`
  `subagent-stop-hook`), which session start never shows. No observation, summary or model call
  comes of it. Without an agent id nothing is kept.
- **A subagent's state (row X17).** My Work's agent view gives each agent a card: a conversation's
  main agent, and each Claude Code subagent, a Claude Code record's agent id (`agent-view.mjs`
  `isSubagent`: Codex tags its main thread too, so a Codex thread keeps the main agent's layout).
  A subagent's card is named as every card is, by its agent ("Claude Code"; its type is kept and
  shown nowhere), its role reads "Subagent · " and the conversation's title, else its folder's
  name, and its one row is the task it was launched with (the parent's Agent or Task call's
  description, else its prompt, matched by the agent id the call returned), once the subagent has
  an observation. Its state, from recorded evidence only: **Finished** when a stop came at or after
  its last recorded step (its SubagentStop, the parent's foreground Agent call returning, or the
  owner's **Mark finished**; its tool calls count as steps here), with the time; else **Working**
  when its last observation or work state write is within 15 minutes, else **No recent step**. A
  later step after a stop means it resumed. Mark finished, shown on a subagent no stop covers,
  writes the same row with `stop_source` `owner-marked-finished`. A main agent's card shows no
  state.
- **A subagent's files (rows X17, X23).** At its SubagentStop claude-mem reads the subagent's own
  transcript (`agent_transcript_path`) whole and copies the files it wrote or declared, as a turn's
  at Stop, under the session's privacy; a failure there still keeps the stop.
- **A Codex subagent's memory at its start (row X26).** Codex starts a spawned subagent with
  `SubagentStart`, never `SessionStart`. claude-mem's Codex hooks hold seven events: SessionStart,
  UserPromptSubmit, PreToolUse, PostToolUse, Stop, PostCompact and SubagentStart (no matcher, 20 s).
  At SubagentStart it reads the first line of the subagent's rollout (`transcript_path`, at most
  1 MiB): a `session_meta` whose `payload.forked_from_id` is a string is a fork, which starts from
  its parent's history, memory included, and gets nothing. A fresh subagent gets the session start's
  context, as `hookSpecificOutput {hookEventName: "SubagentStart", additionalContext}`; an empty one
  is not sent, as Codex records even an empty context as a message. An unreadable rollout counts as
  fresh: a duplicate costs tokens, a missing one the memory.

## What oboete did before slice 1

- `oboete hook <agent> <event>` takes any event name; one it does not know records nothing and
  prints nothing (`capture::shaped`, `hook::injects`). Claude Code is wired for 8 events, Codex for
  7 (`setup::CLAUDE_EVENTS`, `CODEX_EVENTS`), neither SubagentStop nor SubagentStart.
- A subagent's tool calls are recorded with their `agent_id` (the hook's payload, or the transcript
  import's): in the 30 days to 2026-11-06, 24,174 of Claude Code's 59,243 hook tool records and
  11,296 of Codex's 20,797 carried one. No view reads it; My Work's agents view is one card a
  conversation (decision 55).
- A Codex subagent gets memory only after a compaction, at its next tool call (row N7).
- A turn's copies are made by the worker from the turn's hook records (docs/deliverables.md),
  whichever agent made them; a subagent that finishes after its parent's reply is read with no turn
  of its own.

## The slices

### Slice 1: the two hooks

- **SubagentStop.** `setup::CLAUDE_EVENTS` gains `("SubagentStop", 5)`. The hook keeps the stop as
  a record of kind `subagent_stop` in the session: the subagent's id and type (each a non-empty
  string of at most 128 UTF-16 units, as claude-mem's `pickAgentField` counts, and still one after
  the capture's cleaning; the type left out when it is not one) and the hook's time, nothing of the
  subagent's text. With no session or no id, or where nothing is recorded, nothing. Nothing is
  printed.
- **A record, not claude-mem's work state row.** A record is what the hook keeps aside and writes
  back when the store cannot take it, what a forget of its session removes and the retention
  expires, and what the capture exclusions and the redaction cover. A work state write is an op,
  which no forget of a session reaches (docs/work-state.md).
- **A mark, not work.** The stop holds no words: the full-text index gives it no row, a curator's
  window no line. It may come after the reply it worked for, in another session's window, so the
  readers of a session's work pass over it: a window's labels and last time, whether a window is a
  subagent's alone, the session's previous window, a turn's first record and its repository, and
  the manifest's facts (an ended session stays ended, and the stop is no record to curate).
- **Nothing made from it.** A curator is shown no line of a stop, nor of a session's start or end,
  so a forget or a mask of one of them takes nothing that another record's window, turn or request
  made: the purge judges no op by it, a card or a summary is not hidden for its removal, and a
  forget's preview counts none of it. Nor does an excluded session's one keep another session's
  cards and summaries from the embedder, or My Work's ops from being shown. Before slice 1 a start
  or an end did: a forget of a session took another session's window that held its start or end.
- **After a forget.** A stop whose time is after its session's forget is recorded, as every event
  of the session after the registration is (docs/milestone-5-plan.md, 3b-1): what stops a session's
  future records is the capture exclusion.
- **The home's format.** The first stop raises the home to format 6 (docs/store-format.md): a
  binary before it would index the stop's id and type as words and read the stop as a session's
  work.
- **SubagentStart.** `setup::CODEX_EVENTS` gains `("SubagentStart", None, 10, true)` (10 s, as
  Codex's SessionStart, whose work it does). `hook::injects` answers it as Codex's SessionStart is
  answered, the session start's context for the checkout, unless the rollout's first line (at most
  1 MiB) is a `session_meta` with a string `payload.forked_from_id`; an unreadable rollout counts as
  fresh. It takes no compaction flag and records nothing, and the root's shown set stays the root's
  whatever the subagent's id says. A recording failure is shown there as at a session start. An
  assembled context that is empty prints nothing; a checkout with no memory still gets the work
  state's rule, as at a session start.
- **On the owner's machines.** A new event is wired only by `oboete setup claude` and
  `oboete setup codex` (or the settings page's agent setup), after the binary is installed, and
  reaches sessions started after that. Tried first in the `oboete-dogfood` user (CLAUDE.md).

### Slice 2: the subagents on My Work

- **Capture.** A Claude Code tool record keeps the payload's `agent_type` beside its `agent_id`,
  bounded and cleaned as the stop's are (slice 1). An `Agent` or `Task` call's record keeps, beside
  its text and before that text is cut, what the facts read of it, gated: `launched` with the
  subagent's id (the response's `agentId`), the response's `status` and whether it ran async, and
  the task (the input's `description`, else the response's, else the input's `prompt`, else the
  response's; cut to 160 and 500 characters as claude-mem cuts them). A record from before this
  slice is read from its output when that still parses.
- **Facts.** A consumer, `subagents`, keeps rows in knowledge.db's `subagent_facts`, keyed by the
  record and what it says (device, seq, fact, subagent id), with no text: the conversation (agent
  and session), the time and the subagent's type when it has one.
  - `step`: a live (`raw::is_live`: a hook's or a replay's) Claude Code tool record with an
    `agent_id`.
  - `stop`: a `subagent_stop` record: the hook's, or the owner's Mark finished.
  - `launch`: a live Claude Code record's `launched`, the subagent it started, and whether it
    returned with it (`status` `completed` and not async, claude-mem's foreground call). A
    subagent's own call that starts another is a step of the one and a launch of the other.
  - A tombstone's target loses its rows, made again from what raw gives now (none once removed), as
    the manifest's facts are; a rewind drops the device's rows and builds them again from seq 1.
    `oboete rebuild` makes them from raw alone. An expired record gives no fact, its body being gone:
    a subagent whose records expired leaves My Work with its conversation's other work.
  - It runs after `rescan` and before `compress`, and reads every device raw holds, as `fts-text`
    does. Its first run walks raw once; 60,725 of this PC's records are Claude Code hook tool calls
    (2026-11-07).
- **My Work's read.** The subagents part takes the read fence (`dispatch::reading`) itself, after
  raw is opened and until its part is made, so a forget registers before it or after it. It reads
  the consumer's checkpoint first, then the facts, then the `subagent_stop` records and the
  tombstones past that checkpoint (picked by their rows, so only their bodies are read): a stop
  appended since counts at once, a tombstone's target since then is hidden as the manifest's reader
  hides it, and a fact read twice (committed meanwhile) counts once. Steps and launches past the
  checkpoint wait for the consumer, seconds behind; with no checkpoint yet (a first run, a rebuild)
  the part is empty. A fact whose record has expired since (retention stopped before its rebuild)
  is passed over. For each Claude Code conversation the answer lists, its subagents, as
  `subagents`, a list of:
  - `conversation`: the conversation's id, made as the request's `sessions` ids are made;
  - `id`: an id of its own, keyed as the page's other ids are (never the agent id);
  - `type`: the newest one its rows give, else none ("Subagent" on the page);
  - its state: **Finished** with the time when the newest stop (a `stop`, a launch that returned,
    or an imported row below) came at or after its last step (its newest `step`); else **Working**
    when its last step is within 15 minutes; else **No recent step**;
  - `task`: its launch's, read from the launch record when My Work is read (so a forget or the
    retention takes it with its record), gated;
  - whether Mark finished is offered: when it is not Finished.
  A subagent is listed from its first `step`: a stop or a launch with no step makes none. All of a
  conversation's subagents are listed, as claude-mem lists them (a Workflow turn can start forty).
  `view::answered` keeps `conversation` and `id` as it keeps the `sessions` ids, and gates the rest.
- **Imported rows.** claude-mem's `agent-lifecycle` rows, which `oboete import claude-mem` (run by
  hand, not by the worker) brings in as work state ops of its source, are stops of the same
  conversation (`content_session_id`, the platform `claude` when none) and id, at their
  `stopped_at_epoch`. They are already no work of My Work, which reads native writes only.
- **Mark finished.** `POST /api/my-work/finished` with `{"id"}`, posted and checked as
  `/api/my-work/review` is (the page's token and origin, a strict body), under the same lock and
  the read fence, held until the answer is made: a fresh read, then 404 when the id is no listed
  subagent, 409 when it is Finished already. Else the stop goes through the hook's own path
  (`hook::record`, the event `SubagentStop`), as if Claude Code had sent it from the conversation's
  folder: the session, the subagent's id and type, the conversation's recorded folder (its newest
  record's `cwd`) and the server's time, marked as the owner's (`marked: true` in the record). That
  path finds the checkout, reads its capture setting and the exclusions as the hook does, gates the
  record and appends it with source `hook`, a live record of this machine that a forget takes as it
  takes the conversation's others. A recorded folder that is missing, masked or no directory now is
  never replaced (not by the viewer's folder): the answer is 409 `not_recorded`, and so it is when
  the path records nothing (a checkout or a folder that records nothing now). The answer is the read
  after it, the new stop counted at once. A hook's stop that lands before the fresh read makes it
  409; one that lands after it stands beside the owner's, the card Finished either way.
- **The page.** The agents view keeps one card a conversation and, after it, a card per subagent:
  the agent's name and the subagent's type, "Subagent · " and the conversation's title (else its
  folder's name), its state (Working, No recent step, "Finished · " and the time), the task it was
  launched with, and Mark finished when offered (disabled while it is saved; a failure says so on a
  line read out to a screen reader). `subagents` is part of the page's accepted answer and of its
  signature, so a minute's poll that changes only a subagent redraws it. Both languages.
- **Not as claude-mem.** Working is read from the subagent's own tool calls; claude-mem reads its
  observations and work state writes, and its tool calls only against a stop. A card comes with a
  first step; claude-mem's with an observation or a work write. A Codex subagent's thread stays its
  conversation's work, as oboete's agents view shows a conversation; claude-mem gives each Codex
  thread a card of its own with no state.

### Slice 3: a subagent's files

- A subagent's stop record makes the worker copy the files that subagent's records wrote (its
  `agent_id` in the session), as a turn's copies are made, so a subagent that finishes after its
  parent's reply has its files copied too. A file its records already had copied is not copied
  twice.

## Acceptance tests

Slice 1:

- A Claude Code SubagentStop with a session and an agent id keeps one `subagent_stop` record with
  the id, the type when it is a short non-empty string, and the hook's time; one with no session or
  no id, an id over 128 UTF-16 units, or in a checkout that records nothing, keeps nothing. It
  prints nothing.
- The stop is no row of the full-text index and no line of a curator's window. It is none of a
  span's labels (session, repository, time) nor of a turn's repository; a span of a subagent's
  tool calls and its stop is the subagent's alone, the stop expired too; it begins no turn, makes
  no previous window of its session, and is no fact of the manifest, live or expired.
- A forget of a session whose start and subagent's stop lie in another session's window keeps that
  window's card shown and its op and claims as they were, and its preview counts the session's own
  window only. A removal or a mask of a start, an end or a stop is no removal from a span, and a
  span's sessions are not theirs.
- The first stop raises the home to format 6; a rebuild that brings one back, live, expired or
  removed, stamps 6; a home without one stays where it was.
- `oboete setup claude` wires SubagentStop and `oboete setup codex` SubagentStart, a rerun keeps one
  of each, `--remove` takes them out, and Codex's trust rows match the new list; the settings page's
  agent setup shows the change before it is made.
- A fresh Codex subagent's SubagentStart prints the checkout's session start context under
  `hookEventName: "SubagentStart"` and nothing Codex's output refuses; a forked one (the rollout's
  `forked_from_id`) prints nothing; an unreadable or missing rollout counts as fresh; no compaction
  flag is taken, nothing is recorded, and the root's shown set stays the root's, with no id or one
  that names no thread too; a recording failure is shown as at a session start.

Slice 2:

- A Claude Code tool record keeps its `agent_type`, bounded and cleaned; an `Agent` or `Task`
  call's keeps `launched` though its output is cut (both cut lengths); a Codex record is as before.
- The facts: a live tool record with an `agent_id` is a step (a hook's and a replay's, not a
  transcript import's), a `subagent_stop` a stop, a `launched` a launch (returned only when
  completed and not async), one record both a step and a launch when a subagent starts another; a
  forget of a record takes its rows, a mask makes them again, an expired record gives none, and a
  rewind and a rebuild give the same rows.
- A subagent with a stop at or after its last step is Finished with the time; one whose step came
  after its stop is Working or No recent step again; Working within 15 minutes of its last step, No
  recent step after; a foreground launch that returned finishes it; a main agent shows no state.
- With the consumer held behind: a hook's stop appended since is Finished at once, a forget or a
  mask registered since hides what it took, and a fact the consumer commits between the
  checkpoint's read and the facts' counts once; a forget registering during the read or Mark
  finished waits for it, and the next read sees the forget; an expired record's fact not yet
  rebuilt is passed over; with no checkpoint the part is empty.
- Mark finished is offered only to a subagent that is not Finished and appends the owner's stop
  (`marked`, the server's time, source `hook`) through the gate, which a session forget, a record
  forget and a backup's purge then take as any record; the answer shows it Finished. Two clicks at
  once append one stop (the second is 409); a hook's stop before the fresh read makes it 409, one
  after it stands beside the owner's; an id that names no listed subagent is 404; a recorded
  folder that is missing, masked or no directory, a checkout with `capture = false`, an excluded
  folder or pattern, and an exclusion committed during the append are 409 `not_recorded` and append
  nothing; a bad token, origin or body is refused as the review route refuses it.
- `view::answered` keeps `conversation` and `id` whole under a rule that matches them, and two
  conversations' subagents stay apart.
- The page redraws when a minute's poll changes only a subagent, and after Mark finished.
- A forget of the conversation leaves no card for its subagents, and the launch's task goes with its
  record (forget or retention).
- A subagent without a type is shown as "Subagent"; the task is gated and cut as claude-mem cuts it.
- An imported claude-mem `agent-lifecycle` row is read as its subagent's stop at its
  `stopped_at_epoch` and is no request of My Work.

Slice 3:

- A file a subagent wrote and its parent's turn never copied is copied at the subagent's stop, once;
  one already copied is not copied again; a private turn's subagent copies nothing.

## As built

### Slice 1: the two hooks (2026-11-06)

- **SubagentStop** (`setup::CLAUDE_EVENTS`, 5 s): `capture::shaped` makes the stop a
  `subagent_stop` record (`raw::SUBAGENT_STOP`), kept as the hook keeps every record. Nothing is
  printed. The full-text consumer gives it no row (`consumer::fts`); `Raw::labels_in`,
  `Raw::subagent_only` (live or expired), `SESSION_LATEST` (`Raw::previous_window_ops`),
  `Raw::turn_start`, `Raw::session_repo` and the manifest's `facts` and `label_facts` pass over it.
  A curator's window renders no line of a kind it does not know.
- **Line-less records** (`raw::LINELESS`: start, end, subagent_stop): `Raw::forgotten_records` and
  `Raw::forgotten_and_masked` (what a purge judges ops by), `Raw::removed_in` (a card's and a
  summary's removals), `Raw::with_lines` (a forget's preview), `Raw::sessions_over` (the
  embedder's excluded sessions) and `Raw::holds_session` (My Work's ops) pass over them.
- **Format 6** (`raw::SUBAGENT_STOP_FORMAT`): `insert_event` and a rebuild's event, expired and
  removed lines raise the home when the record is a stop (`raw::record_format`).
- **SubagentStart** (`setup::CODEX_EVENTS`, no matcher, 10 s, the context whole as
  `additionalContextLimit` 0): `hook::injects` answers it unless `hook::forked` reads the rollout's
  first line as a fork. The context is SessionStart's for the checkout; the subagent's `agent_id`
  makes its thread `compacted-…`, as after a subagent's compaction (row N7), and the root's shown
  set is left alone at any SubagentStart, whatever its id. `hook::codex_thread` counts an id in
  UTF-16 units, as claude-mem does. A recording failure is shown as at a SessionStart
  (`reads_start`). Nothing is recorded.
- **Both ways tried.** Local Codex reviews found the record, with no reader passing over it, read as
  a session's work by the readers above, and claude-mem's work state row lost when the store
  cannot take it, left behind by a forget of its session, and read as work when the redaction
  changes its list's name. The record keeps what every record has, and the readers pass over it. A
  third review found the forget, the masks and the excluded sessions judging what other records
  made by the stop (and so by a start or an end, before slice 1 too), the home's format, a turn's
  repository, and a subagent start's shown set.
- **Tests:** `hook::tests::a_claude_subagent_stop_is_kept_as_its_id_and_type`,
  `hook::tests::a_fresh_codex_subagent_gets_the_session_start_context_and_a_fork_none`,
  `hook::tests::a_home_that_cannot_be_made_is_reported_at_session_start` (a Codex SubagentStart too),
  `consumer::fts::tests::a_subagent_stop_is_no_row_of_the_index`,
  `curate::tests::a_subagent_stop_gives_a_window_no_line`,
  `raw::tests::a_subagent_stop_is_none_of_a_spans_labels`,
  `raw::tests::a_subagent_stop_leaves_a_span_the_subagents_work`,
  `raw::tests::a_subagent_stop_begins_no_turn`,
  `raw::tests::a_subagent_stop_makes_no_previous_window_of_its_session`,
  `raw::tests::a_subagent_stop_is_none_of_a_turns_repository`,
  `raw::tests::a_lineless_records_removal_takes_nothing_made`,
  `raw::tests::a_spans_sessions_are_not_those_of_its_lineless_records`,
  `raw::tests::a_subagent_stop_raises_the_home_to_the_format_that_reads_it`,
  `forget::tests::a_forgotten_sessions_lineless_records_take_nothing_from_another_sessions_window`,
  `consumer::manifest::tests::a_subagent_stop_gives_no_fact`,
  `setup::tests::the_subagent_hooks_are_wired`.
