# Cards: claude-mem's observations on oboete's engine

What the worker keeps of each stretch of work besides claims, and the table every reader of "what
was done" takes it from: session start, the page, search, `timeline`, the note on a file. Replaces
the "recent sessions" section drafted as unit P9 of the takeover plan.

## Why

The owner, 2026-10-03, after comparing what both tools show on the same sessions:
「もうここまできたらほぼ丸パクリぐらい寄せて理想に近付けた方がいいんじゃない？」, confirmed as: claude-mem's
behaviour, look and prompts are the specification; its code is not ported; oboete's engine and its
merits stay (no vector server, a chain of summarizers, masking, forget, several agents, the owner's
decisions first). Both projects are Apache-2.0: a `NOTICE` entry comes with the first text taken
(slice 2); this slice takes only the shape of an observation.

claude-mem's unit of memory is the observation: a type, a title, a subtitle, facts, a narrative,
concepts, files read and files modified. Its session start, its page, its search tools and its
note on a file all show observations. oboete keeps claims (one sentence with a status and a
quote), which serve the owner's decisions well and "what was done" badly: a list of claims is no
timeline, and the window summary that does say what was done is read by nothing.

Measured on one evaluation home (records 1 to 14,059, 13,358 tool uses) and claude-mem's store
for the same sessions:

| | claude-mem | oboete |
|---|---|---|
| AI calls | one per tool use at most | one per window: 2,879, one per 4.6 tool uses |
| what it keeps of the work | 1 observation per 5.5 tool uses | 1 window summary per 5.0 tool uses (231 characters on average), shown nowhere |
| a window | | 5 records on average (median 4); 99.3% hold one session and one repository |
| lost to a failed call | one tool use | one window (6.5% of windows, with the one provider of that run) |

So a window is already the size of an observation, and it is made by one call. A card is that
window's observation.

## A card

```
cards(device, op_seq, n,                 -- the window op it came in, and its place there
      from_seq, from_offset, to_seq, to_offset, at, ts,
      agent, session, repo,
      type, title, subtitle, narrative, facts, concepts, files_read, files_modified,
      replaced_by)                       -- primary key (device, op_seq, n)
```

`facts`, `concepts`, `files_read` and `files_modified` are JSON arrays of strings. `type` is one
of claude-mem's: bugfix, feature, refactor, change, discovery, decision, security_alert,
security_note, sensitive; or null (K1).

K1. **Where cards come from.** A window op with outcome `curated`. Its `observations` (slice 2)
are its cards, in their order. An op without that field gives one card from its `summary`, when it
is not empty: the summary is the narrative, its first sentence the title (at most 120 characters,
ending in `…` when it was cut), and the type is null. Every window op curated before slice 2 is
of this kind.

K2. **Whose it is.** `agent`, `session` and `repo` are set only when the event records of that
device from `from_seq` to `to_seq` have one agent, one session and one repository between them.
Otherwise they are null, and the card is shown for no session and no repository. `ts` is the time
of the last of those records.

K3. **Curated again.** A window op marked `recurate` replaces the cards of the earlier ops of its
device whose span overlaps what it curated: its own range, less the records of excluded sessions
it kept back. They get its op_seq as `replaced_by` and no reader shows them. Not what the op
`covers`: that holds the earlier windows of the same run, whose cards are new. A card the
recuration covers only in part goes too; the part outside has no card until it is curated again.
The curation phase itself never reads a record twice, so only a recuration replaces.

K4. **Removed records.** A card is not shown when a tombstone appended after its window was cut
targets a record of its span: its text may say what was removed. The window op says where the
device's records stood when it was cut (`at`, new in slice 2; an op without it uses its `to_seq`,
which hides more, never less). A recuration gives the span new cards (K3).
`ponytail:` a new redaction rule tombstones old records, so the cards over them are hidden until
`oboete recurate` reads them again, though K6 would have masked the value; a narrower rule comes
with the forget unit's fence for derived text.

K5. **Rebuild and rewind.** The table is derived: `oboete rebuild` makes it again from the op log,
and a rewind of a device's op log removes that device's cards above the point and clears the
`replaced_by` marks set from above it.

K6. **Text is gated when it is read**, with the rules as they are then, before it is cut: a rule
added after the card was written hides its value.

K7. **One way in.** Readers take cards through `cards::recent` and its siblings, which apply K3,
K4 and K6; none reads the table itself.

## Is a card safe to show?

MUST-M4 says that tool and file content never becomes decided, a preference or global. A card is
written by the curator from records that include tool output, so text an attacker controls (a web
page, a file) can colour it.

- A card has no status: it is never among the decisions, and nothing ranks a claim or acts on it.
  It is shown as a record of work, inside the fence that says the packet is data and not
  instructions.
- The manifest already shows records of the same kind in the same way: the last failing command's
  output and the last reply.
- claude-mem and oboete v1, which the owner runs today, show the same kind of text with no fence.

So MUST-M4 holds as written.

## Slices (test first, one at a time)

1. **The table** (this note's K1 to K7 for ops as they are today): the consumer, the reader,
   `at` in the window op.
2. **The curator writes cards**: `observations` in its answer (0 to 3 a window, claude-mem's
   fields and its guidance on what to record and what to skip, cut down to fit beside the claims
   within a provider's request ceiling), checked like the rest of the answer; file paths are kept
   only when the window's records name them.
3. **Session summaries** (claude-mem's request, investigated, learned, completed, next steps),
   then **session start** in claude-mem's order after the owner's decisions, the **page**, the
   **tools** and the **note on a file**, each its own unit.

## Tests of slice 1 (through `worker::run_once` and `cards::recent`)

1. A curated window op with a summary gives one card: the summary as its narrative, its first
   sentence as its title, the session and repository of its records.
2. A window whose records are of two sessions, or of two repositories, gives a card of no session
   and no repository.
3. A skipped or covered window, and one with an empty summary, give no card.
4. A recuration replaces the cards of the windows it overlaps, one it covers only in part too; a
   rewind below the recuration brings them back; a card over records the recuration kept back
   stays.
5. A tombstone for a record of the window, appended after the window was cut, hides the card; one
   appended before it was cut does not, by the op's `at` or, without one, its last record.
6. A redaction rule added after the card was written masks its value when it is read.
7. `oboete rebuild` gives the same cards.
8. The curation phase's window op says where the records stood when the window was cut, not when
   the answer came.
