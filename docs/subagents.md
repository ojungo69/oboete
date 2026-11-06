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

- `/api/my-work` gives each conversation's Claude Code subagents: id, type, the time of its last
  recorded step (its newest record with its `agent_id`), its stop (the newest `subagent_stop`
  record of the conversation and id, the owner's Mark finished, or the parent's foreground Agent or
  Task call returning, whichever came at or after its last step), and the request it was launched
  with. Tool records keep the payload's `agent_type` beside `agent_id` from slice 2 on; one without
  it is named "Subagent".
- claude-mem's `agent-lifecycle` rows that `oboete import` brings in with its work state are read
  as stops, and are no work of My Work.
- The agents view keeps one card a conversation (its main agent) and adds a card per subagent, as
  claude-mem's: its agent's name, "Subagent · " and the conversation's title, with its type beside
  them (claude-mem keeps the type and shows it nowhere), its state (Working, No recent step,
  Finished with the time), the task it was launched with, and Mark finished when no stop covers its
  last step. Working is read from the subagent's own records, the tool calls claude-mem's
  observations are made from. Both languages.
- Mark finished keeps the same mark as the owner's (claude-mem's `stop_source`
  `owner-marked-finished`), its time the click's; the page posts it as its other actions are posted.
- Bounded as the page's lists are: the subagents of the conversations the view shows.

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

- A subagent with a stop at or after its last step is Finished with the time; one whose step came
  after its stop is Working or No recent step again; Working within 15 minutes of its last step, No
  recent step after; a main agent shows no state.
- Mark finished is offered only to a subagent with no stop, writes the owner's row, and makes the
  card Finished; a forget of the conversation leaves no card for its subagents.
- A tool record without `agent_type` is shown as "Subagent"; a Codex subagent thread is shown as
  claude-mem's agent view shows it (its groups by platform, session and agent id: to be read when
  slice 2 is built).
- An imported claude-mem `agent-lifecycle` row is read as its subagent's stop and is no request of
  My Work.

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
