# Session summaries: claude-mem's summary of a turn on oboete's engine

Unit C2 of the claude-mem parity plan (docs/cards.md, "Why"): what a turn of work asked, looked
into, learned, finished and left next, as claude-mem writes it at every Stop, kept beside the cards
and shown with them at session start. It replaces the digest (spec 3.4, 4.4).

## Why

claude-mem 13.28.0 (`src/sdk/parser.ts`, `src/services/worker/`, `plugin/modes/code.json` at
039c6160) asks its observer for a summary at each Stop of the main agent: `request`,
`investigated`, `learned`, `completed`, `next_steps` and `notes`. A summary with none of the first
five is invalid (13.34.2 takes notes alone: T4); `<skip_summary />` stores none; a session gets one per turn, appended. Session
start lists the ten newest as `S<id> <request> (<date time>)` rows among the observations and ends
with the newest one's investigated, learned, completed and next steps; the page shows them as
session cards. 13.34.2 shows notes after them, there and on the page, and takes a summary of
notes alone (#403).

oboete's digest is another thing: lines that cite claims, one per session and repository after the
idle time, shown only while every claim it cites is current. It says what was decided, which the
packet already shows above it, and not what was done.

Measured on the evaluation home of docs/cards.md (records 1 to 14,059): 318 turn ends (reply
records) beside 2,691 curated windows. One call per turn end adds about 12% calls; oboete stays at
about a fifth of claude-mem's.

## A summary

T1. **When.** One per turn end: a `reply` record, which only an agent's Stop writes (a
subagent's stop writes none, `capture::shaped`), of a session the exclusion list does not name
(D13), once the curation checkpoint has passed it, so the windows that hold the turn have their
cards. A turn is its session's live records from its first event after the session's previous
reply, through this one: an import of the same session is no boundary and no part of it, so neither
a card of a window of imports alone nor the removal of an import touches the turn (Codex on #371).
The worker asks for it where it asked for a digest: when the window phase sent nothing, one call a
run, the oldest turn end after the last one a summary was kept of (every turn end is reached,
however many records follow it), with the digest's holds (a summary every entry fails waits as a
window does, and the turns after it wait with it; one given up after `ATTEMPTS` is kept as a
skip, as a window given up is, and not asked again, though its request changes: that is a
recuration's business); a turn's waits are rows of their own table, `turn_pending`, so an upgraded
home's digest rows never stand for one, and a row a restore leaves past the restored records stands
for nothing but its own request. A turn end of a session the list names is kept back as a window's
records are: an op of its labels alone, read from no record's text, a skip marked `excluded`, so the
scan goes past it and no run reads it again. A run after an undo asks for it, the oldest of each
session the list no longer names first, one a run, though a later turn's summary passed it; each
run judges those sessions once, not their every turn (Codex on #371). Unlike the digest it does
not wait for the session to go idle: a turn is whole at its reply, and claude-mem asks at Stop. The
window phase's own idle wait still holds back the windows of a session's last turn (Codex on C2).

T2. **What it is shown**, between two fence lines, as recorded data, each text gated as a window's
is: the turn's prompts (at most 2,000 characters in all), the cards of its session's windows that
hold any of its records (type, title, subtitle, narrative, facts; the newest 20, by K4 and K6), and
the reply (at most 4,000 characters). Not the tool records: the cards say what they did, and a turn
of hundreds of tool uses still fits. A card of a window that holds two sessions has no session (K2)
and is not shown. Each prompt is gated alone, as a window gates each record, and a card's texts are
shown as they were written and gated (K6), never joined into one line, which would make a text
the gate did not read. Within `window_tokens`, the oldest cards go first; the
prompts and the reply are never cut for them, so with a `window_tokens` below what those take no
card is shown.

T3. **What it is asked.** claude-mem's summary request (`summary_instruction` and the six fields'
placeholders in `code.json`), as the chain's JSON fields, in the configured language; `NOTICE`
names it. `request` is the model's: a short title of what was asked and what was done. claude-mem
asks for a summary of every request, so a skip (`"skip": true`) is for a turn with nothing in it.
As C1a for cards: the summary is about the turn shown, never what was known before it.

T4. **What is kept.** A skip is an op with no fields. Otherwise each field is trimmed, and one over
its cap (`request` 300 characters, the others 2,000) is dropped, never cut: nothing is cut before
the gate (K6). An answer left with none of its fields is refused, as claude-mem's parser
refuses it, and the next entry is asked. Notes alone are a summary (#403; claude-mem 13.34.2's
parser takes them, and 13.28.0's refused them).

T5. **The op.** Kind `turn`: `agent`, `session`, `repo` (K2: the one repository of the turn's
records, or none), `through` (the reply's seq), `from` (the turn's first record), `read` (the
spans of the windows whose cards it was shown) with their `goals`, `removed` (as a window op lists
it, over all of those records), and the fields or `skipped`. An answer paid for is never lost to
the op log's cap: an op over it lists no removals, as a window op past its bound does, and is then
hidden by any (K4); one still over it is kept as a skip; and a turn whose op would pass the cap
even as a skip (its labels alone) is not asked for.

T6. **The table.** `consumer::turns` keeps each op in knowledge.db's `turns`; `oboete rebuild`
makes it again and a rewind removes a device's rows above the point (K5).

T7. **Hidden after a removal.** A summary is shown only while nothing is removed, beyond what its
op lists, from the turn's records (its session's: not another session's between them), the
records of the windows whose cards it read, or their goals:
K4's rule over all it was shown or built on. A removal that hides a card hides every summary that
read it.

T8. **Gated when read** (K6), through one reader, `turns::recent` (K7).

## Session start (slice 2)

S7. **Rows.** The repository's ten newest summaries as `S<op seq> <request> (<Mon D, H:MM AM>)`,
or `Session started` with no request, among the cards by their own time; another device's as
`S<device>.<op seq>`. claude-mem dates every row after the first by the next summary's time, which
moves it; oboete does not.

S8. **The newest one's fields** after the timeline, `**Investigated**: …`, `**Learned**: …`,
`**Completed**: …`, `**Next Steps**: …`, `**Notes**: …`, when it is not older than the newest card
shown (claude-mem's rule; Notes since #403, as claude-mem 13.34.2 shows them: a qualification the
summary role put in notes, such as "the agent says it was not verified", reaches the session). The legend gains `🎯session`.

S9. **Fitting.** claude-mem's order: the fields go first, then the rows are halved, then the cards.

S10. **`get S<op seq>`** shows one summary whole, as `get <op seq>.<n>` shows a card.

S11. **The digest retires.** Its phase stops, session start drops `## Digest of the last session`,
and its ops stay in the log, unread; the spec's sections that named it (1.1, 1.4, 1.7, 3.4, 4.1,
4.4, 5.4, 6.2, 6.5) name the cards and session summaries (owner direction 2026-10-03).

## Tests of slice 2 (through `cards::block`, `consumer::manifest::text` and `search::b::get`)

1. The block shows the newest summaries among the cards by their own time, after a card of the
   same time, `S<op seq>` (another device's with its device), `Session started` without a
   request, and the legend starts with `🎯session`.
2. The newest summary's fields, notes last, follow the timeline when it is not older than the
   newest card shown, and not otherwise.
3. Session start shows the repository's summaries and the newest one's fields in the block.
4. When the block does not fit: the fields go first, then the summary rows halve, the newest
   kept, then the cards.
5. `get S<op seq>` shows a summary in full; an ID of no summary gives none.
6. The digest is gone from session start, the worker and `oboete rebuild`.

## Differences from claude-mem

| | claude-mem | oboete | Why |
|---|---|---|---|
| When it is written | at Stop | once the turn's windows are curated (the window phase waits a session's idle time before it cuts its last window) | one call per window (oboete's merit); the newest turn may have no summary at the very next session start |
| Whose | the calling agent's sessions | every agent's sessions in the repository | one memory across agents |
| Files | attached from the session's tool evidence | on the cards | the cards carry them |
| After a forget | stays | hidden (T7) | forget is oboete's |
| Row dates | shifted to the next summary's time | its own | a claude-mem quirk |

## Slices (test first)

1. T1 to T8: the phase in the digest's place, the op, the consumer, the hide rule.
2. S7 to S11: session start, `get`, the digest retired, the spec amended.

## Tests of slice 1

1. A reply past the checkpoint gets one summary; one not yet past it gets none; a second run makes
   no second call.
2. An excluded session's reply gets no call.
3. The prompt holds the turn's prompts, its session's cards of the windows that hold the turn, and
   the reply, gated; not another session's cards, nor a card of a window that holds none of the
   turn's records.
4. An answer with none of the six fields is refused and the next entry asked, and one with notes
   alone is kept (#403); a skip is an op with no fields.
5. A field over its cap is dropped, never cut.
6. T7: a removal from the turn's prompt, from a record of a window whose card it read outside the
   turn, or from that window's goal hides the summary; one its op lists does not.
7. `oboete rebuild` gives the same rows; a rewind removes them.
8. `turns::recent` gates every field with the rules as they are when it reads.
9. A card's title, subtitle, narrative and facts are shown as they were written and gated, never
   joined into one line; each prompt is gated alone.
10. The cards fit `window_tokens` beside the prompts and the reply, the oldest going first, and the
    op lists only the windows of the cards shown.
11. A turn's repository is its own records' one, or none when they are of two.
12. A removal from another session's record inside the turn's span does not hide it.
13. An op over the op log's cap is kept without its removal list, or as a skip, and the turn is
    not asked again; a turn whose op would pass the cap even as a skip is not asked for.
14. A turn end followed by more than 2,000 records still gets its summary.
15. A turn starts at its session's first event after the previous reply: a window of the earlier
    turn that reaches past that reply only over other records shows none of its cards.
16. A turn the exclusion list kept back is summarized after an undo, though a later turn of
    another repository's session was summarized while it waited; it is kept back as an op of its
    labels alone, and a run after the one that kept it back writes nothing.
17. A turn every provider fails `ATTEMPTS` times is kept as a skip and not asked again, though the
    request changes.
18. A waiting row past what the store holds, as a restore from backups that lack the newest records
    leaves, does not hide the turns before it.
