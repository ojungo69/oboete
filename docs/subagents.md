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
- **A subagent's files (rows X17, X23).** At its SubagentStop claude-mem reads the last 2 MiB of
  the subagent's own transcript (`agent_transcript_path`) and copies the files it wrote or
  declared, as a turn's at Stop, under the session's privacy; a failure there still keeps the stop.
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
    the manifest's facts are; a rewind drops every device's rows and builds them again from seq 1.
    `oboete rebuild` makes them from raw alone. An expired record gives no fact, its body being gone:
    a subagent whose records expired leaves My Work with its conversation's other work.
  - It runs after `products` and before `compress`, and reads every device raw holds, as `fts-text`
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

- **Which calls.** A turn end (a reply, or the session's end) copies the files of the calls between
  the session's previous reply and it, from the turn's first prompt or harness message on
  (`deliverables::candidates`), so a foreground subagent's calls, inside its parent's turn, are its
  parent turn's. A background subagent's calls after its parent's reply and before the session's
  next prompt or harness message belong to no turn, and nothing copies their files. Slice 3 copies
  those at the subagent's stop. A stop looks at the subagent's calls (its `step` facts in
  knowledge.db, of this device's hook records) after its previous stop and before it: the newer of
  the newest stop of the same subagent the phase took (`subagent_scan` in deliverables.db, which a
  forget or a mask of that stop leaves) and the newest the facts hold that raw reads as a hook's
  of this device (the facts keep no source). Of those it takes a call in a gap: the session's
  newest hook reply before the call is there and not forgotten, and no prompt or harness message
  of the session lies between them (nor a purged record of this device that was one, its session
  gone). A call is so taken once, by its turn's end or by the first stop after it the phase took;
  a stop it did not take (forgotten or masked before the phase read it) leaves its calls to the
  next. A raw.db restored below the phase's checkpoint empties the device's `subagent_scan` as the
  checkpoint moves.
- **Whose.** Only a subagent launched from a turn the hooks saw public: its launch record (the
  parent's `Agent` or `Task` call that `launched` it, the newest before the stop, a hook record of
  this device and session) reads now as an event marked public, and a prompt or harness message of
  the session reads before it in its own turn, as a turn's copy asks of a call. A subagent with no
  launch record, or one forgotten, masked or expired, or launched from a private or unobserved
  turn, copies nothing, though a later public prompt has cleared the session's private flag; and
  each call must be marked public too.
- **When.** The deliverables phase takes a Claude Code hook `subagent_stop` record (the hook's, or
  the owner's Mark finished) as it takes a turn end, in seq order. As it reads the subagent's calls,
  stops and launch from `consumer::subagents`' facts, it takes a stop only once that consumer's
  checkpoint for this device has reached it with no rebuild owed (no checkpoint is none reached);
  else it stops just before the stop, its own checkpoint below it, and a later round takes it, the
  consumers draining before the phase.
- **What.** The files those calls wrote or declared, found in one pass as `candidates` finds a
  turn's (one path, its last write's call; no failed call; `MAX_FILES` at most), copied as a
  turn's: the row's turn the gap's reply (`turn_seq`, `turn_ts`), its call the call that wrote or
  declared the file, by which My Work links the copy to that call's request. Two gaps' rows stay
  apart under the table's unique key, identical content at two paths too; a gap's row whose copy its
  reply's own turn made is that turn's row, as two calls of one turn are one; and `copy_into` keeps
  one file for one content. A forget of the call or of the gap's reply denies the row from its
  registration on; a forget of the stop or of the launch alone does not (the launch is looked at
  again before each row is written). After the rows, the stop's seq goes into `subagent_scan` with
  the phase's checkpoint: a stop taken again after an interruption makes the same rows (unique) or
  none.
- **Kept.** `subagent_scan` holds a subagent's device, session, id and a seq, nothing said in it; a
  forget of the session leaves the row, as its denial keeps the session's id.
- **Not covered.** A foreground subagent's call is its parent turn's: a parent that never ends its
  turn (no reply, no session end) leaves it uncopied, where claude-mem's SubagentStop copies it.
- **Not as claude-mem.** claude-mem's SubagentStop hook copies from the last 2 MiB of the
  subagent's transcript, the files the subagent wrote inside its parent's turn too; oboete's parent
  turn copies those, and the stop the rest.

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

- A file a background subagent wrote after its parent's reply, with no prompt or harness message
  between, is copied at the subagent's stop, once, its row the gap's reply's turn and the writing
  call's; a foreground subagent's file, written inside its parent's turn, is copied by the turn's
  end and not again at its stop; a subagent whose calls straddle the reply has each copied once, by
  the turn or the stop; a stop after the session's next prompt still takes the gap's calls; a
  session's end with no reply copies the prompt-backed file, and the stop nothing again.
- One stop over two gaps: identical content at two paths gives two rows, each its gap's turn and
  its call's request on the page and the card; one path written in both gives one row, the last
  write's; a forget of either call leaves the other's row and file.
- A second stop of the same subagent takes only its calls since the first, also when the first
  was forgotten or masked after the phase took it (its copy kept, its origins the call and the
  reply), and after its copy was swept; a first stop forgotten before the phase read it leaves its
  calls to the second; a stop taken again after an interruption makes no row twice; a resumed
  subagent's new file is copied though its earlier calls named `MAX_FILES` files; a stop with no
  call copies nothing and moves the checkpoint; a replay's or a transcript's stop between a call
  and the hook's stop is no boundary.
- A subagent launched from a private turn copies nothing, though a later public prompt cleared the
  flag (through the hooks); one with no launch record, a launch forgotten or masked, or launched
  from a turn with no prompt observed copies nothing; a failed call and a call not marked public
  name nothing; a Codex record, another device's records, a replay's and a transcript's stop or
  launch copy nothing. Each beside a control that copies.
- A gap whose reply was forgotten takes nothing (purged too); a prompt forgotten and purged between
  the reply and a call makes it no gap's.
- With the `subagents` consumer behind: reply A, a stop it has not reached, reply B: A is taken, the
  stop and B wait, the phase's checkpoint below the stop; once the consumer has reached it, both are
  taken, with no row twice; with no checkpoint, or a rebuild owed, the stop waits too.
- A forget of a stop's copy's call or reply hides its link and card at once and the sweep removes
  it; retention that expires the call, the reply and the stop keeps the copy; a raw.db restored
  below the checkpoint empties the device's `subagent_scan`.

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

### Slice 2: the subagents on My Work (2026-11-07)

- **Capture** (`capture::agent_field`, `capture::launched`): a Claude Code tool record keeps the
  payload's `agent_type` beside its `agent_id`, bounded at 128 UTF-16 units, cleaned and bounded
  again. An `Agent` or `Task` call keeps `launched` (`agent_id` from the response's `agentId`,
  `async`, `status`), made from the payload before its text is flattened and cut. **Not as
  planned**: it keeps no task, which My Work reads from the call's input (below). A SubagentStop
  payload with `oboete_marked: true` (`capture::OWNER_MARK`, the viewer's alone) keeps
  `marked: true`.
- **Facts** (`consumer::subagents`, after `products` in `worker::consumers()`): rows of
  `subagent_facts` keyed by (device, seq, fact, agent id), with the conversation, the time, the type
  and whether a launch returned (`status` `completed` and not async). An id a mask touched (a
  tombstone's `*`, the capture gate's `[REDACTED]`) names no subagent. A tombstone's target is made
  again from raw; a rewind drops every device's rows and marks each it has a checkpoint of
  (`subagent_rebuild`), as a device's lost tombstone may have taken another device's record (the
  manifest makes the device's alone again); every device raw holds is read.
- **My Work's read** (`my_work_subagents::read`, from `my_work::read_held` by `subagents_of`, which
  takes the read fence), in the page's read (`my_work::listed`) alone: `my_work::reviewing`, which
  the owner's actions read under the reviews' lock, reads none, as a purge holds the read fence
  while it waits for that lock. Its conversations are every request's, whatever the range, as Mark
  finished looks a subagent up (the page reads all and filters itself): the consumer's checkpoints
  first (none: no subagents), then the rows of the listed Claude Code conversations, then, past each
  device's checkpoint (from the first record of one it has none of yet, a copied home's own), the
  `subagent_stop` records (`Raw::stops_after`) and the tombstones' targets
  (`Raw::tombstones_after`), a row read twice counting once; a record expired since is passed over
  (`Raw::expired_among`). claude-mem's imported `agent-lifecycle` rows are the ops of an import's
  source on that list whose `content_session_id` is a listed conversation's
  (`Raw::imported_list_fields`). An older record's output is read as async by `isAsync`, `is_async`
  (true or 1) or the status `async_launched`, as capture reads a response. A launch's task is read
  from its record when My Work is read, as claude-mem reads a launch: its input's description, else
  the response's, else the input's prompt, else the response's, as capture kept them (cleaned
  whole), through the gate as its rules are now, then folded and cut to 160 and 500 characters, as
  My Work's other texts are; none when neither the input nor the response parses (a text cut when it
  was kept). **Not as planned**: at most 300 subagents are listed (`my_work_subagents::MAX_LISTED`),
  those with the newest last steps. claude-mem's agent view is made from its newest 300 observations
  and its recent work state writes, so a subagent with nothing recent leaves it too; all of this
  PC's would have more than doubled each minute's read (below).
- **The answer's `subagents`**: each with `conversation` (`{id, fingerprint}`, as the request's
  `sessions` give it), `id` (`s:` and the keyed print of the agent, session and subagent id),
  `type`, `state` (`working`, `quiet`, `finished`), `finished_at`, `last_step`, `task` (gated) and
  `can_mark`. `view::answered` takes `conversation` and `id` out of the gate's walk and puts them
  back.
- **Mark finished** (`POST /api/my-work/finished`, `Viewer::my_work_finished`): `{id}` only (`s:`,
  at most 64 bytes); its own lock (`my_work::finished_lock`, waited for 10 s at most and by four
  requests at most at once, else 503 `busy`), then raw.db and knowledge.db opened, then the read
  fence (the order a restore takes raw.lock and the fence in), held through a fresh read with that
  handle (`my_work::subagent`, every request built, as the page reads `range=all`) and the append
  (`my_work_subagents::mark`, `hook::record_if` with the event `SubagentStop`). 404 `not_found`, 409
  `finished`, 409 `not_recorded` when the folder of the subagent's newest step record, whichever
  device recorded it (`my_work_subagents::recorded_folder`, read through the tombstones), is empty,
  masked or no directory, when the hook's path records no stop, or when the stop it would record
  names another session or subagent (a rule of now that masks the session or the id: `record_if`
  then appends and keeps nothing). After a stop is recorded the worker is started, as a hook starts
  it, and reaped when it leaves; a stop kept behind older kept files (`hook::Recorded::kept`) is
  recorded when they are written back, and counts. **Not as planned**: the answer is `{}`, and the
  page reads My Work again (`myWorkReread`), which counts the new stop at once, as it is past the
  consumer's checkpoint.
- **The page** (`myWorkAgents`, `myWorkSubagent`, `myWorkFinish`): after each conversation's card,
  one per subagent whose `conversation.id` is the card's: "Claude Code · " and the type (else
  "Subagent"), "Subagent · " and the conversation's newest request's title (else its projects), the
  state (Working, No recent step, "Finished · " and the time), the task, and Mark finished while it
  is offered, disabled with the review's buttons while either saves and described by the card's
  heading. The outcome is said on the status line (`aria-live`). `myWorkAccept` keeps `subagents`
  and `myWorkAnswerSignature` holds it.
- **Not as claude-mem**, besides the plan's: the card names the subagent's type, which claude-mem
  keeps and shows nowhere, and the conversation's title is its newest request's, oboete keeping no
  title of a session's own.
- **Reviewed.** A local Codex review of the built change found a launch task cut before its clean
  and mask, a copied home's own device left out of the read's tail, a forgotten record's folder
  taken before its purge, a stop the rules of now would record under another session, raw.db opened
  after the read fence, a lost tombstone of another device's record never made again, and an id the
  capture gate masked taken for a subagent's. Each is fixed as above, with a test. A second review
  found the task still cut when it was kept, so a rule added since could not mask a secret the cut
  split, and a run of base64 shorter than the cleaning's length after the cut was kept; the task is
  no longer kept, and is read as above. The order of raw.db and the hold has its own test. A third
  found the reviews' lock, which Mark finished and the actions' read (`reviewing`) took before the
  read fence, taken by a purge after it: Mark finished takes a lock of its own (**not as planned**,
  which named the review route's), and only the page's read reads the subagents. On #850 Codex's
  app, cubic and CodeRabbit found the worker not started after the owner's stop, the listing's
  conversations bound to the range, every imported row read, an older record's `is_async` not read,
  the folder looked up on this device alone, and the lock's wait unbounded; on its second head the
  worker left unreaped and the lock's waiters uncounted: each is fixed as above, with a test.
- **Measured** on a copy of this PC's store (2026-11-07, release build): the consumer's first pass
  took 18.1 s and kept 24,596 rows, 1,522 subagents with a step. My Work's subagents part took 71 to
  82 ms over its 277 conversations (106 ms with each checkpoint 20,000 records behind) and the whole
  read 0.49 to 0.54 s. The 1,522 added 326 KB to an answer of 250 KB; all were No recent step, no
  stop being kept before slice 1 and 18 of them having a launch record that still parses.
- **Tests:** `capture::tests::a_subagents_call_keeps_its_type_and_a_launch_what_it_started`,
  `consumer::subagents::tests::each_record_says_what_it_says_of_a_subagent`,
  `consumer::subagents::tests::a_records_rows_follow_its_tombstones_rewinds_and_expiry`,
  `my_work_subagents::tests::a_subagents_state_follows_its_steps_stops_and_launch`,
  `my_work_subagents::tests::the_records_past_the_checkpoint_count_at_once`,
  `my_work_subagents::tests::an_imported_agent_lifecycle_row_is_its_subagents_stop`,
  `my_work_subagents::tests::the_subagents_with_the_newest_steps_are_listed`,
  `my_work_subagents::tests::a_copied_homes_own_records_count_before_its_first_checkpoint`,
  `my_work_subagents::tests::a_launch_task_is_read_whole_through_the_rules_of_now_before_its_cut`,
  `my_work_subagents::tests::a_launch_task_falls_back_as_claude_mem_reads_one`,
  `my_work::tests::the_actions_read_takes_no_readers_hold`,
  `my_work_subagents::tests::the_folder_is_the_newest_steps_of_any_device`,
  `view::tests::mark_finished_opens_raw_before_the_readers_hold`,
  `consumer::subagents::tests::a_rewind_makes_again_what_another_devices_lost_tombstone_took`,
  `my_work::tests::a_listed_conversations_subagents_are_in_its_answer`,
  `view::tests::the_my_work_finished_route_appends_the_owners_stop_and_refuses_the_rest`, and the
  page's in `src/testdata/viewer-readiness/test.mjs`;
  `my_work::tests::imported_work_state_is_neither_read_nor_counted` keeps an imported row out of the
  requests.

### Slice 3: a subagent's files (2026-11-07)

- **The phase** (`deliverables::run`): beside a turn end it takes a Claude Code hook `subagent_stop`
  record, in seq order; one whose id names no subagent in the facts (none, masked, another agent's:
  `consumer::subagents::facts`) is read past. The worker hands it its knowledge.db connection.
- **The wait** (`consumer::subagents::reached`): the consumer's checkpoint for this device at the
  stop or past it, with no rebuild owed (`subagent_rebuild`); else the phase returns before its
  batch's last move, its checkpoint at the last turn end it took, and the worker's next round, which
  the stop's arrival after the round's start wakes, takes it.
- **The launch** (`deliverables::stop_calls`, `launched_public`): the subagent's newest `launch`
  fact before the stop, read in raw as a hook's event (the facts keep no source; a forget's
  registration tombstones it at once, before its purge) that still names the subagent (a mask of
  its id names none: `consumer::subagents::facts`), its `public_turn` mark there, and a prompt
  or harness message read before it in its own turn (`prompted_before`, through
  `Raw::hook_turn_start` and `Raw::turn_seqs`). Else the stop copies nothing, and counts as taken.
- **The calls**: its `step` facts after the newer of `subagent_scan`'s seq and the newest `stop`
  fact raw reads as a hook's, and before the stop; of those, each in a gap (`Raw::gap_reply`: the
  session's newest hook reply before it, by raw's labels, with no hook prompt or envelope of the
  session between them, nor a purged record that was one, whose kind the purge keeps), whose reply
  no forget denies (`Raw::forgets`). A step's gap is looked up again only past a record that may
  end one (`Raw::gap_bound_between`: a hook reply, prompt or envelope of the session, or a purged
  one), so a gap of many steps is read once, and a step aside is looked for every 256 steps. The
  calls go to one `candidates` pass, `prompted` from its start, read one at a time as a hook's
  events, so a long run of them is never held whole.
- **Rows** as a turn's, the turn the gap's reply, each written under the read fence
  (`dispatch::reading`) once the launch is seen public again and neither the call nor the reply is
  forgotten: a forget registered while the file was copied leaves it no row (the copy is the idle
  sweep's). Then `subagent_scan` takes the stop's seq with the phase's checkpoint, in one
  transaction. A restore below the checkpoint empties the device's `subagent_scan` as it moves the
  checkpoint.
- **Not covered**, besides the plan's: a subagent launched by another in a gap (after the parent's
  reply, with no prompt or harness message before the launch in its turn) copies nothing, its
  launch's turn having no prompt; claude-mem's SubagentStop copies its files. The owner's Mark
  finished is a stop as a hook's is: a subagent marked long after its calls copies its files as
  they are then, as a worker that comes late copies a turn's.
- **Reviewed.** A local Codex review of the code found the launch looked at before a forget could
  register while the files were copied, the kept seq committed apart from the checkpoint (a restore
  that kept the checkpoint then skipped a new file at a seq given again), every call of a stop held
  whole until it was copied, each step's gap scanned from its reply (a gap of N steps read N times)
  with no step aside, and tests missing for these and for a registered forget, a taken stop masked
  since, and a replay's or a transcript's launch. Each is fixed as above, with a test. A second
  review found a purged reply passed over: a launch after it then read as in the earlier prompt's
  turn, and a call after two replies whose newer one was purged as in the older one's gap. A purged
  hook reply of the device now ends a turn (`Raw::hook_turn_start`, which ends a turn's copies
  there too) and a gap (`Raw::gap_reply`); a step aside is looked for while the calls are read too
  (`candidates`), and a test holds the read fence at the row. On #851 Sourcery found a launch
  masked while the files were copied still authorizing them, and CodeRabbit a restored record of no
  kind (a backup line that carried none) passed over as a boundary; both are fixed, with a test,
  and the parity row's wording names the harness message that ends a gap.
- **Mutation runs**: 27 mutants of the new code, each built and run alone (6 GB, 240 s): each of the
  boundary, the turn, the gap's purged prompt and denied reply, the launch's mark, prompt and
  harness message, the hook source, the wait and its rebuild, the restore, the kept seq and its
  transaction, the row's fence (looked at and held), the wait's return, the step aside in each
  pass, the gap's bound and its purged records, a purged reply or a restored record of no kind
  ending a turn and a gap, the launch read again naming its subagent, and `prompted` makes a test
  fail. The launch's mark survived the first run, the private
  turn's launch having no prompt either; the notification's turn after it, which has one, kills it.
  A check of the launch's denial survived the second, a forget's registration tombstoning the
  launch already, and is gone.
- **Tests:** `deliverables::tests::a_background_subagents_file_is_copied_at_its_stop_once`,
  `deliverables::tests::one_stop_over_two_gaps_keeps_each_gaps_copies_apart`,
  `deliverables::tests::a_later_stop_takes_only_the_calls_after_the_one_taken`,
  `deliverables::tests::a_stop_forgotten_before_the_phase_read_it_leaves_its_calls_to_the_next`,
  `deliverables::tests::a_stop_after_the_first_run_takes_no_call_before_an_earlier_stop`,
  `deliverables::tests::a_subagent_launched_from_a_private_turn_copies_nothing` (through the hooks),
  `deliverables::tests::a_subagent_with_no_public_launch_or_call_copies_nothing`,
  `deliverables::tests::a_gap_whose_reply_or_prompt_was_forgotten_takes_nothing`,
  `deliverables::tests::a_stop_waits_for_the_subagents_facts`,
  `deliverables::tests::a_stops_copy_goes_with_its_call_or_reply_and_stays_through_retention`,
  `deliverables::tests::a_stops_seq_is_kept_with_the_checkpoint`,
  `deliverables::tests::a_stop_stepped_aside_is_taken_again_whole`,
  `deliverables::tests::a_stops_row_is_denied_by_its_reply_not_by_its_stop_or_launch`,
  `deliverables::tests::a_taken_stop_masked_since_still_bounds_the_next`,
  `deliverables::tests::a_launch_forgotten_while_its_stops_files_are_copied_leaves_no_row_after`,
  `deliverables::tests::a_purged_reply_still_ends_its_turn_and_gap`,
  `deliverables::tests::a_launch_masked_while_its_stops_file_is_copied_leaves_no_row`,
  `deliverables::tests::a_restored_record_of_no_kind_ends_a_turn_and_a_gap`. My Work
  links a stop's copy to the request whose prompt span holds its call (`my_work::copies_of`, as for
  a turn's copy, `my_work::tests::a_requests_copies_join_its_work_product`).
