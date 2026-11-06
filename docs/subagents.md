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
  subagent-stop` (asynchronous, 30 s). With a session id and an agent id (each a non-empty string of
  at most 128 characters; the agent type the same, else absent), in a tracked folder, it keeps the
  session, the platform, the agent's id and type and the hook's time, nothing of the subagent's
  text: one closed row of the work-state list `agent-lifecycle` (`stop_source`
  `subagent-stop-hook`), which session start never shows. No observation, summary or model call
  comes of it. Without an agent id nothing is kept.
- **A subagent's state (row X17).** My Work's agent view gives each agent a card: a conversation's
  main agent, and each Claude Code subagent, an agent id with an agent type
  (`isSubagentEvent`; an id alone is main-agent work). A subagent's card is named by its type, its
  role reads "Subagent · " and the conversation's title, else its folder's name, and it holds the
  request it was launched with (the parent's Agent or Task call's description, else its prompt).
  Its state, from recorded evidence only: **Finished** when a stop came at or after its last
  recorded step (its SubagentStop, the parent's foreground Agent call returning, or the owner's
  **Mark finished**), with the time; else **Working** when its last step is within 15 minutes, else
  **No recent step**. A later step after a stop means it resumed. Mark finished, shown on a
  subagent with no stop, writes the same row with `stop_source` `owner-marked-finished`. A main
  agent's card shows no state.
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
  claude-mem does: one closed row of the work state list `agent-lifecycle`, its fields
  claude-mem's (`agentStopFields`): the task `Agent <id> in claude session <session>`, `status`
  `done`, `platform_source`, `content_session_id`, `agent_id`, `agent_type` (each a non-empty
  string of at most 128 characters; the type left out when it is not one), `stopped_at` and
  `stopped_at_epoch` (the hook's time) and `stop_source` `subagent-stop-hook`. With no session or
  no id, or where nothing is recorded, nothing. It goes through the gate and the checks
  `work_state_write` goes through, into the repository of the hook's checkout.
- **Not an event.** A work state write is an op, not an event, so that no curator's window, summary,
  card or search reads it (docs/work-state.md L4); a stop is no different. An event would have been
  read as a session's work: by the window that holds it (its labels, its last time, whether it is
  a subagent's alone), by the turn it falls into and by the manifest's facts. Session start shows
  no closed row, and My Work reads the list apart, as claude-mem's work view does.
- **SubagentStart.** `setup::CODEX_EVENTS` gains `("SubagentStart", None, 10, true)` (10 s, as
  Codex's SessionStart, whose work it does). `hook::injects` answers it as Codex's SessionStart is
  answered, the session start's context for the checkout, unless the rollout's first line (at most
  1 MiB) is a `session_meta` with a string `payload.forked_from_id`; an unreadable rollout counts as
  fresh. It takes no compaction flag and records nothing. A recording failure is shown there as at a
  session start. An assembled context that is empty prints nothing; a checkout with no memory still
  gets the work state's rule, as at a session start.
- **On the owner's machines.** A new event is wired only by `oboete setup claude` and
  `oboete setup codex` (or the settings page's agent setup), after the binary is installed, and
  reaches sessions started after that. Tried first in the `oboete-dogfood` user (CLAUDE.md).

### Slice 2: the subagents on My Work

- `/api/my-work` gives each conversation's Claude Code subagents: id, type, the time of its last
  recorded step (its newest record with its `agent_id`), its stop (the newest `agent-lifecycle`
  row of the conversation and id, the owner's Mark finished, or the parent's foreground Agent or
  Task call returning, whichever came at or after its last step), and the request it was launched
  with. Tool records keep the payload's `agent_type` beside `agent_id` from slice 2 on; one without
  it is named "Subagent".
- The agents view keeps one card a conversation (its main agent) and adds a card per subagent, as
  claude-mem's: its type, "Subagent · " and the conversation's title, its state (Working,
  No recent step, Finished with the time), its request, and Mark finished when it has no stop. Both
  languages.
- Mark finished writes the same `agent-lifecycle` row, its `stop_source` `owner-marked-finished`
  and its time the click's, as claude-mem's; the page posts it as its other actions are posted.
- Bounded as the page's lists are: the subagents of the conversations the view shows.

### Slice 3: a subagent's files

- An `agent-lifecycle` row from a SubagentStop makes the worker copy the files that subagent's records wrote (its
  `agent_id` in the session), as a turn's copies are made, so a subagent that finishes after its
  parent's reply has its files copied too. A file its records already had copied is not copied
  twice.

## Acceptance tests

Slice 1:

- A Claude Code SubagentStop with a session and an agent id writes one closed `agent-lifecycle` row
  with claude-mem's fields and the hook's time; one with no id, or an id over 128 characters,
  writes nothing; a type that is not a short non-empty string is left out. It records no event and
  prints nothing, and session start shows no closed row.
- My Work makes no request of an `agent-lifecycle` row and shows none; another list is read as
  before.
- `oboete setup claude` wires SubagentStop and `oboete setup codex` SubagentStart, a rerun keeps one
  of each, `--remove` takes them out, and Codex's trust rows match the new list; the settings page's
  agent setup shows the change before it is made.
- A fresh Codex subagent's SubagentStart prints the checkout's session start context under
  `hookEventName: "SubagentStart"` and nothing Codex's output refuses; a forked one (the rollout's
  `forked_from_id`) prints nothing; an unreadable or missing rollout counts as fresh; no compaction
  flag is taken, nothing is recorded, and the root's shown set stays the root's; a recording failure
  is shown as at a session start.

Slice 2:

- A subagent with a stop at or after its last step is Finished with the time; one whose step came
  after its stop is Working or No recent step again; Working within 15 minutes of its last step, No
  recent step after; a main agent shows no state.
- Mark finished is offered only to a subagent with no stop, writes the owner's row, and makes the
  card Finished; a forget of the conversation leaves no card for its subagents.
- A tool record without `agent_type` is shown as "Subagent"; a Codex subagent thread is shown as
  claude-mem's agent view shows it (its groups by platform, session and agent id: to be read when
  slice 2 is built).

Slice 3:

- A file a subagent wrote and its parent's turn never copied is copied at the subagent's stop, once;
  one already copied is not copied again; a private turn's subagent copies nothing.

## As built

### Slice 1: the two hooks (2026-11-06)

- **SubagentStop** (`setup::CLAUDE_EVENTS`, 5 s): `hook::agent_stop` writes the closed
  `agent-lifecycle` row through `capture::work_state` and `Raw::work_state_under`, as
  `work_state_write` does, after the place's capture exclusions; the list's name is
  `work_state::AGENT_LIFECYCLE`. Nothing is printed. My Work leaves the list out of the work it
  reads (`my_work::read`).
- **SubagentStart** (`setup::CODEX_EVENTS`, no matcher, 10 s, the context whole as
  `additionalContextLimit` 0): `hook::injects` answers it unless `hook::forked` reads the rollout's
  first line as a fork. The context is SessionStart's for the checkout; the subagent's `agent_id`
  makes its thread `compacted-…`, so the root's shown set stays the root's, as after a subagent's
  compaction (row N7). A recording failure is shown as at a SessionStart (`reads_start`). Nothing
  is recorded.
- **First tried as an event** (`subagent_stop`): a local Codex review found it read as a session's
  work by the window labels (`Raw::labels_in`), whether a window is a subagent's alone
  (`Raw::subagent_only`), the session's previous window (`SESSION_LATEST`), a turn's start
  (`Raw::turn_start`) and the manifest's facts, so it became the work state row claude-mem keeps.
- **Tests:** `hook::tests::a_claude_subagent_stop_is_kept_as_a_closed_lifecycle_row`,
  `hook::tests::a_fresh_codex_subagent_gets_the_session_start_context_and_a_fork_none`,
  `hook::tests::a_home_that_cannot_be_made_is_reported_at_session_start` (a Codex SubagentStart too),
  `my_work::tests::a_subagents_stop_is_no_work_of_a_request`,
  `setup::tests::the_subagent_hooks_are_wired`.
