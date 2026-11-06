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

## What oboete does now

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

- **SubagentStop.** `setup::CLAUDE_EVENTS` gains `("SubagentStop", 5)`. `capture::shaped` turns it
  into a record of kind `subagent_stop`, its body `{agent_id, agent_type}` as claude-mem keeps them
  (each a non-empty string of at most 128 characters; the type left out when it is not one); with no
  agent id, nothing. Its labels (session, repository, checkout, time) are the hook's, as every
  record's, so forget, retention, exclusion and the backup take it as they take the session's other
  records.
- **A mark, not text.** The record has no words: the full-text consumer gives it no `raw_docs` row,
  as the owner's directive has none (so no search hit, embedding or snippet), and a curator's
  window passes over it unsent, as over a record its reading set aside. My Work's activity counts
  prompts, summaries, cards and work items, not records of other kinds, so it moves no request.
- **SubagentStart.** `setup::CODEX_EVENTS` gains `("SubagentStart", None, 10, true)` (10 s, as
  Codex's SessionStart, whose work it does). `hook::injects` answers it as Codex's SessionStart is
  answered, the session start's context for the checkout, unless the rollout's first line (at most
  1 MiB) is a `session_meta` with a string `payload.forked_from_id`; an unreadable rollout counts as
  fresh. It takes no compaction flag and records nothing. An empty context prints nothing.
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
- The agents view keeps one card a conversation (its main agent) and adds a card per subagent, as
  claude-mem's: its type, "Subagent · " and the conversation's title, its state (Working,
  No recent step, Finished with the time), its request, and Mark finished when it has no stop. Both
  languages.
- Mark finished posts the owner's action as a My Work op, as the other actions on the page; the
  my_work consumer keeps it, and a forget of the conversation takes it.
- Bounded as the page's lists are: the subagents of the conversations the view shows.

### Slice 3: a subagent's files

- A `subagent_stop` record makes the worker copy the files that subagent's records wrote (its
  `agent_id` in the session), as a turn's copies are made, so a subagent that finishes after its
  parent's reply has its files copied too. A file its records already had copied is not copied
  twice.

## Acceptance tests

Slice 1:

- A Claude Code SubagentStop with an agent id records one `subagent_stop` with the id and type, and
  the session's labels; one with an id over 128 characters or none records nothing; a type that is
  not a short non-empty string is left out.
- The record is in no search (full text, vectors, the raw arm), no snippet, no curator's window,
  and moves no My Work request's activity; a forget of its session takes it.
- `oboete setup claude` wires SubagentStop and `oboete setup codex` SubagentStart, a rerun keeps one
  of each, `--remove` takes them out, and Codex's trust rows match the new list; the settings page's
  agent setup shows the change before it is made.
- A fresh Codex subagent's SubagentStart prints the checkout's session start context under
  `hookEventName: "SubagentStart"`; a forked one (the rollout's `forked_from_id`) prints nothing; an
  unreadable or missing rollout counts as fresh; a checkout with nothing to show prints nothing; no
  compaction flag is taken and nothing is recorded.

Slice 2:

- A subagent with a stop at or after its last step is Finished with the time; one whose step came
  after its stop is Working or No recent step again; Working within 15 minutes of its last step, No
  recent step after; a main agent shows no state.
- Mark finished is offered only to a subagent with no stop, records the owner's action, and makes
  the card Finished; a forget of the conversation takes the action.
- A tool record without `agent_type` is shown as "Subagent"; a Codex subagent thread is shown as
  claude-mem's agent view shows it (its groups by platform, session and agent id: to be read when
  slice 2 is built).

Slice 3:

- A file a subagent wrote and its parent's turn never copied is copied at the subagent's stop, once;
  one already copied is not copied again; a private turn's subagent copies nothing.

## As built

### Slice 1: the two hooks (2026-11-06)

- **SubagentStop** (`setup::CLAUDE_EVENTS`, 5 s): `capture::shaped` keeps `{agent_id, agent_type}`
  as a `subagent_stop` record, each a non-empty string of at most 128 characters through the gate
  every captured string goes through; no id, nothing. Nothing is printed.
- **A mark.** `consumer::fts::Fts` gives it no `raw_docs` row, beside the owner's directive. A
  curator's window gives it no line already: a kind the window does not render has no head and no
  text (`Prepared::new`), and a test now holds that.
- **SubagentStart** (`setup::CODEX_EVENTS`, no matcher, 10 s, the context whole as
  `additionalContextLimit` 0): `hook::injects` answers it unless `hook::forked` reads the rollout's
  first line as a fork. The context is SessionStart's for the checkout; the subagent's `agent_id`
  makes its thread `compacted-…`, so the root's shown set stays the root's, as after a subagent's
  compaction (row N7). Nothing is recorded; an empty context prints nothing.
- **Tests:** `hook::tests::a_claude_subagent_stop_is_kept_as_its_id_and_type`,
  `hook::tests::a_fresh_codex_subagent_gets_the_session_start_context_and_a_fork_none`,
  `consumer::fts::tests::a_subagent_stop_is_no_row_of_the_index`,
  `curate::tests::a_subagent_stop_gives_a_window_no_line` (a guard: it passed before the change),
  `setup::tests::the_subagent_hooks_are_wired`.
