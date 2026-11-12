# A hook's time: one deadline for what it injects, and a record of the slow ones

Finding O1 of the 2026-11-10 audit. MUST-M16 says a write failure never holds the agent up; this
extends that to time: how a hook keeps to the time its agent gives it, and how a hook that did
not is found afterwards.

## What happened

Claude Code cancels a hook at the timeout its settings give it, and `oboete setup claude` writes 5 s
for `UserPromptSubmit`. On this PC Claude Code's transcripts hold seven cancellations of oboete's
hooks in three days (attachments of type `hook_cancelled`): six prompts (2026-11-07 04:49,
2026-11-08 03:15, 03:21 and 03:23, 2026-11-10 00:01 and 01:26 UTC) and one `PostToolUse`.

- The prompt was recorded each time: capture comes first in the hook. What was lost came after
  it. A cancelled hook gives the prompt no context, records no shown set, counts nothing for
  doctor, and leaves the marker and the worker's start to the next hook.
- The waits inside a prompt's hook each had a clock of their own: raw.db for 2 s (`STORE_WAIT`),
  the read fence for 1 s (`READ_WAIT`), the resident viewer for `[inject] per_prompt_wait_ms`
  (2,000 ms, counted from the fence). 2 + 1 + 2 is the 5 s of the timeout, before the process's own
  start and its local reads are counted.
- Two shapes were seen in the three whose records were set against the transcript's time. The two
  of 2026-11-10 were recorded 0.06 and 0.03 s after the prompt and cancelled all the same: the
  time went after capture. The one of 2026-11-08 03:23 was recorded 4.456 s after the prompt: the
  time went before capture or in it.
- Nothing recorded where the time went: a cancelled hook writes nothing at its end.

## The change

### One deadline for what a hook injects

- A hook's time is its agent's timeout, as `setup` writes it for that agent's event
  (`setup::hook_timeout`), less a margin of 1 s (`hook_time::MARGIN`) for the hook's own end (the
  marker, the worker's start, its answer on stdout) and for the time before `main`. It is counted
  from the first line of `main` (`hook_time::init`).
- Recording is never cut short by it. The store's wait, the record and what is kept when the
  store cannot be had (docs/unwritten.md) are as they were.
- What a hook injects takes what remains:
  - the read fence is waited for `READ_WAIT` or what remains, whichever is less. With nothing left
    the hook injects nothing, by the path of a fence that was not had: the record is written, and
    an injection point the session claimed is given back;
  - the prompt point's wait for the viewer ends at `per_prompt_wait_ms` or with the hook's time,
    whichever comes first. With no time left the viewer is not waited for: the prompt takes the
    words side, and its side is counted `late`;
  - a read's hook (the note on a file, a memory lookup) gives nothing once its time is over, and
    its lookup's wait, its store's wait and its fence's wait take what remains.
- An agent whose hook oboete writes no timeout for (OpenCode's and Pi's plugins, an agent with no
  adapter) has no such deadline: its waits are as they were. The plugins kill the hooks they
  start themselves: OpenCode's each after 3 s; Pi's after 3 s where nothing waits for the hook,
  and with its own 2 s wait where it waits for one (docs/unwritten.md U2).
- A hook run inside another process (a test) has a clock that starts when it is called and no
  deadline unless the test sets one.

| Agent | Event | Timeout | Time for what it injects |
|---|---|---|---|
| claude | `SessionStart` | 10 s | 9 s |
| claude | `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `Stop`, `PostCompact`, `SubagentStop` | 5 s | 4 s |
| codex | `SessionStart`, `SubagentStart` | 10 s | 9 s |
| codex | `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Stop`, `PostCompact` | 5 s | 4 s |
| grok | `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `Stop`, `PostCompact` | 5 s | 4 s |
| claude, codex, grok | `SessionEnd` | 3 s | 2 s |
| agy | `SessionStart`, `PreInvocation`, `PostToolUse`, `Stop` | 10 s | 9 s |
| cursor | each of the seven hooks `setup` writes | 10 s | 9 s |

An event that injects nothing (a `Stop`, a `SessionEnd`) has the time all the same: it only
places the `late` line below. An event `setup` writes no hook for has none.

### A record of slow hooks

`state/hook-time.jsonl` holds one line for each look at a slow hook:

- A hook marks each phase as it begins (`Clock::begin`): `parse`, `lookup`, `note`, `settings`,
  `open`, `record`, `fence`, `manifest`, `prompt`, `after`, `output`. The marks are atomics: a
  hook inside its time does no I/O for them.
- A thread beside the hook (`Clock::watch`) writes a line when the hook is still running 2 s
  after its start (`"at": "slow"`), and one more half a second past the hook's time for what it
  injects (`"at": "late"`), the last look before the agent's timeout. A hook with no such time
  (an agent oboete writes no timeout for) gets no `late` line. The thread ends with the process:
  a hook that ends inside 2 s never hears it.
- A hook that took 2 s or more writes a line at its end (`"at": "end"`).
- A hook with `slow` or `late` lines and no `end` never ended: the agent cancelled it, or it was
  killed. Its last line names the phase it was in.

A line holds names and numbers only, never text, a path or a session:

```json
{"ts": 1794300000000, "pid": 4242, "agent": "claude", "event": "UserPromptSubmit",
 "at": "slow", "ms": 2001, "before_ms": 30, "phase": "prompt",
 "began": {"parse": 0, "settings": 2, "open": 9, "record": 11, "fence": 40, "prompt": 41}}
```

- `ts` is the wall clock at the first line of `main`, which a transcript's time for the prompt can
  be set against: a prompt whose `ts` is seconds after the transcript's time started late.
- `before_ms` is the time between the process's creation and that first line (Linux:
  `/proc/self/stat`'s start time; null elsewhere): the loader, and a binary read back from disk.
- `began` is when each phase began, in ms after `ts`. A phase lasts until the next one begins.
- The agent's and the event's names are the command line's, kept to letters, digits, `-` and `_`,
  32 at most.
- The file starts again at 256 KiB, the one before it kept as `hook-time.jsonl.1`. It is 0600.
  No home is made for it. Two hooks that turn it over at once lose nothing but what the first
  kept: the second's line begins the new file.
- A line begins on a line of its own, an empty one before it: a line cut short (a full disk, a
  hook killed as it wrote) takes only itself with it.

`oboete doctor` reads both files and says, after the injection's lines and whatever `[inject]`
holds, how many hooks on record in the last 7 days were still running at 2 s, how many of them
never ended, and the phase most of them were in (`slow hooks: …`; no line when there is none).
The two files hold some hundreds of slow hooks: where more than that were slow inside 7 days, the
newest are counted.

## Where it does not reach

- A start later than the margin is not fixed by a deadline: an agent that starts the hook late,
  or a binary that takes seconds to page in. `before_ms` and a late `ts` show the second; the
  first shows only against the transcript.
- One local read that takes seconds is not cut: SQLite reads are not interrupted, and a hook past
  its deadline that finishes them still injects. Throwing the result away then would save no
  time and lose what may still arrive inside the agent's timeout. The record says whether that
  is where the time goes, in `manifest`, `prompt` or `note`; cutting those reads is #897's.
- O1 is closed on what the record shows after the install, not on the deadline alone (#897).

## Tests

- `hook_time::tests`: what remains of a timeout and when it is over; a hook inside its time writes
  nothing; a slow hook's `slow`, `late` and `end` lines and what they hold; an end with no thread;
  names only, and no home made; the time before `main` on Linux, its arithmetic by hand; the
  file's turn-over at its cap, a turn-over another hook made first, and one that fails; a line
  cut short; doctor's line; an end's longest phase.
- `tests/doctor_hook_time.rs`: doctor prints the line, with `[inject]` right or wrong.
- `hook::tests::a_hook_with_no_time_left_records_and_injects_nothing`,
  `the_read_fence_is_waited_for_within_the_hooks_time`,
  `the_prompt_points_wait_ends_with_the_hooks_time`,
  `a_read_whose_hook_has_no_time_left_gets_no_note`,
  `a_memory_lookup_waits_within_the_hooks_time`.
- `setup::tests::a_hooks_timeout_is_the_one_setup_writes`.
