# Cards: claude-mem's observations on oboete's engine

What the worker keeps of each stretch of work besides claims, and the table every reader of "what
was done" takes it from: session start, the page, search, `timeline`, the note on a file. Replaces
the "recent sessions" section drafted as unit P9 of the takeover plan.

## Why

The owner, 2026-10-03, after comparing what both tools show on the same sessions:
「もうここまできたらほぼ丸パクリぐらい寄せて理想に近付けた方がいいんじゃない？」, confirmed as: claude-mem's
behaviour, look and prompts are the specification; its code is not ported; oboete's engine and its
merits stay (no vector server, a chain of summarizers, masking, forget, several agents, the owner's
decisions first). Both projects are Apache-2.0: `NOTICE` names the text taken (slice 2); the table
takes only the shape of an observation.

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
      from_seq, from_offset, to_seq, to_offset, removed, ts,
      agent, session, repo,
      type, title, subtitle, narrative, facts, concepts, files_read, files_modified,
      replaced_by)                       -- primary key (device, op_seq, n)
```

`facts`, `concepts`, `files_read` and `files_modified` are JSON arrays of strings. `type` is one
of claude-mem's: bugfix, feature, refactor, change, discovery, decision, security_alert,
security_note, sensitive; or null (K1).

K1. **Where cards come from.** A window op with outcome `curated`. Its `observations` (slice 2)
are its cards, in their order. An op without that field gives one card from its `summary`, when it
is not empty: the summary is the narrative, and the type is null. Such a card is kept without a
title: its title is the narrative's first sentence as the card is read (K6), at most 120
characters, ending in `…` when it was cut. A summary as long as the op keeps one (2,000
characters) may have been cut there, before any gate read it, and gives no card. Every window op
curated before slice 2 is of this kind.

K2. **Whose it is.** `agent`, `session` and `repo` are set only when the event records of that
device from `from_seq` to `to_seq` have one agent, one session and one repository between them.
Otherwise they are null, and the card is shown for no session and no repository. `ts` is the time
of the last of those records by seq, which need not be the latest time (a replay, a late hook).

K3. **Curated again.** A window op marked `recurate` replaces the cards of the earlier ops of its
device whose span overlaps what it curated: its own range, less the records of excluded sessions
it kept back. They get its op_seq as `replaced_by` and no reader shows them. Not what the op
`covers`: that holds the earlier windows of the same run, whose cards are new. A card the
recuration covers only in part goes too; the part outside has no card until it is curated again.
The curation phase itself never reads a record twice, so only a recuration replaces.

K4. **Removed records.** A card is not shown when a tombstone removes, from a record its curator
was shown, something its window op does not list: the curator read it, and the card's text may
say it. Those records are the window's own and each session's goal, its first prompt, which a
window carries in (`goals` names the ones outside the window that the prompt kept; one cut to fit
the prompt was not shown). The op lists what was already
removed from them when the window was cut (`removed`: each a record and, for a part of one, its
offset and length), by this device's own tombstones up to the record that was its last before the
window's first read; a removal that lands during the cut, or comes from another device, is not
listed and hides the card. A removal is known by what it removes, not by when: the same one made
again is still listed, and a restore that lost records and uses their seqs again changes nothing.
An op that lists none (every op before this slice, and one with more than 500 removals) has no
card shown once anything is removed from its records. A recuration gives the span new cards
(K3), with the removals it found.
Not covered, and left to the forget unit's fence for derived text (#366): what a card restates
of a claim its curator was shown, and a removal whose tombstone a restore lost with a damaged
segment.
`ponytail:` a new redaction rule tombstones old records, so the cards over them are hidden until
`oboete recurate` reads them again, though K6 would have masked the value; a narrower rule comes
with the forget unit.

K5. **Rebuild and rewind.** The table is derived: `oboete rebuild` makes it again from the op log,
and a rewind of a device's op log removes that device's cards above the point and clears the
`replaced_by` marks set from above it.

K6. **Text is gated when it is read**, with the rules as they are then, before it is cut: a rule
added after the card was written hides its value. So a card is kept whole, and a reader cuts it:
a title made of a narrative is cut from the gated narrative, where a value the rules know by what
follows its sentence is hidden already and one the cut would split is still whole. The agent,
session and repository a card is shown under are gated too.

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

## The curator's cards (slice 2)

The curator's one call per window returns `observations` beside `claims` and `summary`: the cards
of what the stretch built, fixed, changed, decided or found out, usually 1 to 3 and at most 5.

C1. **What it is asked.** claude-mem's guidance for an observation (`plugin/modes/code.json` at
039c6160): the nine types and what each means, the seven concepts, what a title, a subtitle, a
narrative and a fact are, what to record ("what the system now does", not "what was looked at")
and what to skip (routine steps, a result only confirmed again). Cut down to about 390 tokens
beside the claims' 840, by `budget::estimate`. Cards are written in the answer's language; a type
and a concept stay as the keywords. `NOTICE` names what was taken. Not taken: the observer's
framing (it watches a live session and must not contact it; the curator is one stateless call
over a record fenced as data), its XML (the chain's JSON schema asks for the fields) and its skip
sentinel (an empty list).

C1a. **The window's own lines only.** claude-mem's observer is shown only the work it records.
The curator is also shown what the window carries in (each session's goal, its proposals,
decisions and open items) and the kept claims the lines may change, under headings that start
`Already known:`. In probes on the evaluation home, free models wrote carried open items as the
window's cards and as claims quoting them with a uid for a line, and turned a command whose output
the window elided into its result. The prompt now says that the summary, the cards and the claims
are about the numbered lines, that what is already known is never a card's subject or a new claim,
that a claim's line is an `L` id and its quote never comes from the claims below, and that a
command whose output is not shown says nothing about its result: about 120 tokens more. On the
window those probes failed on, four runs after the change wrote none of it.

C2. **One call, as before.** A second call per window would double the calls, which is what
oboete does not copy. The request grows by those 390 tokens and the answer by its cards. On the
2,691 curated windows of the evaluation home above, 99.6% were within the one hard ceiling among
the default entries (Groq free: 8,000 tokens a request, 1,250 of them kept for the answer); with
the cards' instructions and C1a's rule about 64% are. `window_tokens` stays at 5,000: a window over an entry's
ceiling goes to the next entry without a call, and a Groq entry is bound first by its 40,000
tokens a day, which is about six windows.

C3. **Each card is checked alone and never fails its window** (`curate::cards_of`). A claim that
does not parse fails the answer, and a provider with it; a card that cannot be kept is dropped,
and the op says how many were (`cards_dropped`).

- It is kept when its title is not empty: claude-mem's one rule for storing an observation.
- A type outside the nine is no type. claude-mem keeps an unknown type as written and takes a
  missing one for `bugfix`; a wrong label on the timeline is worse than none.
- Concepts outside the seven go. A list holds text only, each item once.
- A file is kept when the window's lines (their text, not their ids or the session headings) name
  it whole, not as a part of a longer path
  (`/etc/passwd` is not named by `/tmp/etc/passwd`, nor `bar.rs` by `foo+bar.rs` or `foo:bar.rs`:
  a mark a file name may hold is part of it, and a `:` ends a path only before a line number or
  where no more of a name follows); a relative path may end a longer one. Marks text puts around a
  path (quotes, brackets, `,`, `;`, `=`, `|`, `!`, `?`, `#`) end it, so a file name holding one
  reads as two names: a limit. A card
  is found by its files (the note on a file), so it names none the curator was not shown.
- A title over 300 characters, a subtitle over 500 or a narrative over 4,000 drops the card:
  nothing is cut before the gate (K6). A fact or a file over 500 characters is dropped, as is one
  past the tenth fact or the twentieth file of a list.
- The first five cards are kept. Over the op cap, the last card goes first.

C4. **An answer without the list is an answer.** A provider that does not hold to the schema
may leave `observations` out: the op then has none and its summary is the card (K1). An empty
list says there was nothing worth a card, and the window has none.

C5. **A refused answer gives no cards.** An answer whose claims are all unanchored is still a
provider's failure (D11), its cards with it, and the window goes to the next entry. Before C1a,
the answers so refused in the probes were the ones that had written carried items as cards. A rule
that kept their cards and dropped their claims would also take the window as curated without the
claims the next entry may anchor. With Codex's small model as the only entry, 2 of the 14 windows
of a span were refused so after C1a, each over one claim that quoted a tool's input or an output
JSON-escaped twice (issue #368); a window every entry refuses has cards after
`oboete recurate --skipped`.

`ponytail:` the curator sees one window, not the cards it wrote before, so a result confirmed
again two windows later can get a second card; claude-mem's observer sees its own earlier
answers. If the timeline shows such repeats, the session's last card titles go into what the
window carries in, within its existing budget.

## Slices (test first, one at a time)

1. **The table** (this note's K1 to K7 for ops as they are today): the consumer, the reader,
   `removed` in the window op.
2. **The curator writes cards**: `observations` in its answer (C1 to C5).
3. **Session start shows the cards** (S1 to S6 below), then **session summaries** (claude-mem's
   request, investigated, learned, completed, next steps, with their rows and fields in the same
   block), the **page**, the **tools** and the **note on a file**, each its own unit.

## Session start shows the cards (slice 3)

claude-mem's session start is one block: a header, a legend, a format line, a line on how to
fetch more, then the recent observations and session summaries by day (its
`src/services/context/` at 039c6160). oboete's packet keeps its own sections and gains that
block.

S1. **Where.** After the owner's decisions and open items (oboete's packet puts them first), before
the digest of the last session, which the latest summary's fields replace in the next unit, and
before the checkout's state lines (todo list, last exchange, files touched) and the live lines.

S2. **What.** claude-mem's model text, as its format gives it, without what oboete has no data
for yet:

```text
# [<repository name>] recent context, 2026-10-03 7:37am GMT+9

Legend: ●bugfix ◆feature ↻refactor ✓change ○discovery ⚖decision ⚠security_alert ⚷security_note ⊘sensitive
Format: ID TIME TYPE TITLE
Fetch details: get(ID) | Search: search(query)

### Oct 2, 2026
412.0 9:41p ○ The worker leaves a lock it no longer holds
413.0 " ✓ The lock file is removed on exit
### Oct 3, 2026
420.1 6:05a ● Two workers no longer race for one lock
```

- No `Mode:` line: oboete has one set of types. No `🎯session` in the legend and no session rows
  until session summaries exist (next unit). No `Stats:` line and no footer: claude-mem counts
  the tokens each observation's work took, and oboete does not count a window's.
- The times are local, as claude-mem's are: the header's date and `h:mmam` with the offset as
  `GMT+9`, each day `Mon D, YYYY`, each row `H:MMa` or `H:MMp`, and `"` for a row in the same
  minute as the row before it that day.

S3. **Rows.** `<ID> <TIME> <ICON> <title>`, claude-mem's compact row, the oldest first. The icon
is the type's (the legend's); a card without a type (a window's summary, K1) has `📝`, claude-mem's
icon for an unknown type. The ID is the card's window op and place, `<op seq>.<n>`, for a card of
the device that reads it, and `<device>.<op seq>.<n>` for another device's: a copied home keeps
the ops of the device it was copied from, whose op seqs start again on the copy (Codex on slice 3),
and sync brings others'. A dot, so it is never read as a claim's uid, whose prefix is hexadecimal,
nor as a record's `device:seq`. The title is gated as it was written and again on the one line
its row shows it on: a rule may match only the flattened title, and the row's ID and time in
front of it would keep an anchored rule from matching at the packet's gate (Codex on slice 3).

S4. **Which cards.** The repository's newest current cards, at most 50 (claude-mem's default),
read through `cards::recent` (K3, K4, K6), of every agent: claude-mem shows only the calling
agent's by default, and one memory across agents is what oboete is for.

S5. **Size.** The packet's cap rises from 6,000 to 9,000 characters: claude-mem's block alone
may take 10,000, and Cursor drops a context over 10,000 (oboete keeps it under its 9,500 units).
The block gets the room the rest of the packet leaves and is fitted as claude-mem fits its own:
the number of cards halves until it fits, down to one; with no room for one there is no block.
It is measured as the packet leaves: gated (a mask can make the rest longer than it was read),
with each closing tag escaped as the fence escapes it, in UTF-16 units, the measure Cursor cuts by
(Codex on slice 3). The fence's own text is outside the cap, as it always was.
The rest of the packet is never cut for it, and the stored manifest keeps its own 6,000, so its
state lines never take all of the block's room.

S6. **`get` shows a card.** The fetch line names `get`, so `get <op seq>.<n>` (CLI and MCP)
prints the card in full: its type, title, subtitle, narrative, facts, concepts, files read and
modified, date, agent and session, through the same reader (K4, K6). An ID of no current card
says so.

## Tests of slice 1 (through `worker::run_once` and `cards::recent`)

1. A curated window op with a summary gives one card: the summary as its narrative, its first
   sentence as its title, the session and repository of its records.
2. A window whose records are of two sessions, or of two repositories, gives a card of no session
   and no repository.
3. A skipped or covered window, and one with an empty summary, give no card.
4. A recuration replaces the cards of the windows it overlaps, one it covers only in part too; a
   rewind below the recuration brings them back; a card over records the recuration kept back
   stays.
5. A removal the window op does not list hides the card; one it lists does not, nor the same
   one made again; after a restore that lost records, a removal at a seq they had hides it too;
   a removal from the goal a window carried in hides it as one from its own records does.
6. A redaction rule added after the card was written masks its value when it is read: in its
   text, in a title whose sentence the rule knows only by what follows it or whose cut would split
   the value, and in the session and repository it is shown under.
7. `oboete rebuild` gives the same cards.
8. The curation phase's window op lists what was removed from its records before the window was
   cut, by this device up to the cut, and names the goal it carried in from outside its span.
9. A summary as long as the op keeps one is no card.

## Tests of slice 2 (through `curate::run_phase` with a stub curator, and `cards::recent`)

1. An answer's cards are kept in its window op, in their order; an empty list is kept as one.
2. An answer without the list, or with something else in its place, gives an op without it.
3. A card without a title, one that is no object, one with a text over its cap and those past
   the fifth are dropped and counted; the window is covered.
4. A type outside the nine becomes none, concepts outside the seven go, a list holds text only,
   and a file the window does not name whole goes.
5. Three cards within their own caps that pass the op cap together: the last goes first.
6. The answer's shape names every field of a card as required, and the prompt asks for cards.
7. A window op's observations are its cards in the table, in their order, and its summary is
   then no card; an op with an empty list has none.
8. Every field of a curator's card is gated when it is read.
9. The prompt keeps the summary, the cards and the claims to the numbered lines (C1a).


## Tests of slice 3 (through `cards::block`, `consumer::manifest::text` and `search::b::get`)

1. The block is claude-mem's recent context: its header with the repository's name and the local
   time and offset, the legend, the format and fetch lines, the days oldest first, a row per card
   with its ID, local time (`"` in the minute of the row before it), icon and title, and `📝` for a
   card without a type.
2. The block halves its cards, the newest kept, until it fits its room; with no room for one, or
   no card, there is none.
3. Session start shows the repository's cards after the decisions and before the state lines, and
   not another repository's.
4. The cards take only the room the rest of the packet leaves, and the rest is not cut for them;
   the stored manifest keeps its own 6,000 characters under the packet's 9,000.
5. A card is read by its ID through the reader's rules (K4); an ID of no current card gives none;
   another device's card is named and read with its device.
6. `get` shows a card in full: its ID, time, type and repository, title, subtitle, narrative,
   facts, concepts and files.
7. A title is gated as its row shows it, on one line, so a rule anchored to the flattened title
   hides its value.
8. The cards leave room for what the gate adds to the rest, and are fitted in UTF-16 units inside
   the fence, a closing tag in a title escaped: the rest is not cut for them.
