# What an op was made from: one reading for every kind

Design of the change that comes before decision 57's slices 4 to 8 (docs/my-work-model.md), written
before its code, as the other units' notes are. Slices 4 to 8 add My Work jobs whose ops are built
on other ops (reviews on groups, links and the story on reviews, rules and goals on prompts). Every
rule that follows what an op was made from is written today once per op kind, and the viewer's
answer gate exempts the values oboete generates once per route and per field. Seven findings of
2026-10-25 and 2026-10-26 came from that shape:

- #664, Codex twice: a restore that reuses an op seq left a backup copy's and a segment's other
  body at that seq unjudged.
- #666 (CodeRabbit on #664): dependencies are resolved across stores; a segment's body must stay.
- #669: a grouping op did not name the grouping ops it read, so a forget missed it; then exclusion
  did not follow `read.ops` (S6).
- #670: a group's keyed id was masked by a rule that matched 12 hex digits.
- #673: the hidden list's cursor was not exempt from the gate.

The map behind this note (five readers over `main` 935fd220, each fact checked in the source) is in
the working notes, not in the repository; its findings that matter are below.

## What is per kind today

1. **Shapes.** A window op names its records as `from_seq`/`from_offset`/`to_seq`/`to_offset`,
   `goals`, `removed` (left out when empty or over 500) and the claims it was shown
   (`candidates`, `carried`); a turn op as `from`/`through`, `read` pairs, `goals` and `removed`; a
   My Work op as `read.spans`, `read.ops` and `removed`; a claim by its `evidence` and `support`,
   and its window by the shared `batch`; a correction by its `anchor`; a hide by its targets.
2. **Readers of provenance.** Forget's judge has a function per kind (window, turn, My Work) and
   per-kind arms in `purged` and `widen`; the kinds that hold derived text are listed by hand in
   `forgotten_op_bodies`' SQL, backup's `judged_line` and `rewrite_copy`; a restore cuts the ops
   past its records by window and migration ops alone (`Rebuild::finish`); backup's dry count
   (`ops_to_purge`) judges with an empty judge. A new kind must be added to each.
3. **Identity.** Records and ops are numbered `MAX + 1` per device, so a restore that lost raw.db's
   tail numbers new ones with the lost seqs, while set-aside segments and copies keep the old ones.
   Every reference is by (device, seq) alone: forget's `taken`, a backup line given raw.db's
   rewritten body (`Judge::purged`), My Work's `read.ops`, hide targets, the consumers' rewind.
4. **Exclusion.** Each egress path checks it its own way (by repository, by session key, by
   quoted sessions); the shortlist matches sessions by id without their agent; S6 holds for the
   grouping fold alone.
5. **The answer gate.** `view::answered` takes generated values out of the redaction walk by the
   route's name and the field's name: `next` for four routes, a My Work request's id, its
   conversations' ids, its work's streams and its version, a session's key, and a list of selector
   keys for four other routes. Each was added after a finding.

## Possible faults the map found (each to be confirmed by a failing test before it is fixed)

- F1. A restore keeps a turn op whose `through`, or a My Work op whose `read` spans, pass the
  restored records: only window and migration ops are cut.
- F2. A set-aside segment's old op at a seq raw.db reused is given the new op's rewritten body, of
  another kind even (`Judge::purged` for Window, Turn and MyWork).
- F3. A hide whose target seq a restore reused hides the new card.
- F4. Backup's export trusts an old segment at seqs that new ops covered before it looked (#83's
  stated limit).
- F5. A claim's uid names its record by (device, seq): a new claim of the same sentence on a reused
  seq has an old claim's uid.
- F6. The shortlist's exclusion check by session id without the agent.

## Rules

P1. **One reading of provenance.** One function reads what an op was made from, for every kind, as
each is stored today: its records (spans by device, with byte ranges where there are), the ops it
read, the uids it was shown, its goals and what was removed from it. Forget's judge, the backup's
purge and count, a restore's cut and S6 read provenance only through it, so a kind added later
adds one case there. No stored op is rewritten to a new shape. The judge's rules stay as they are
(a record forgotten as it was shown then, a uid forgotten, an op it read taken, and which job's
mark a body gets), applied to that one reading.

P2. **A seq of a device names one record or one op for good.** Today a restore that lost raw.db's
tail gives the next records and ops the lost seqs (`MAX + 1`), while set-aside segments, copies and
knowledge.db's checkpoints still name the old ones. The rule: a restore starts each device's next
record and op past the highest seq any segment (live or set aside), any copy, any consumer checkpoint
or any restored op names, leaving a gap, as a segment a restore skipped does now. Every point read
of a record by seq already checks that the record it gets is that seq (the `after(seq - 1, 1)`
readers compare `r.seq`), and the range readers test membership, so a gap reads as nothing there.
The consumers rewind today only when a checkpoint is above raw.db's highest seq (`Consumer::top`,
MUST-M14), which a gap hides once a new record passes it; so the restore also says which range it
did not bring back, in seq order, where every consumer meets it as an ordinary step and rewinds that
range (one way to weigh before the code: a marker record and op at the first seq past the gap, whose
body is the range, backed up and restored as any other, with no provenance of its own). This closes
F1 to F5 where they start, and #83's stated blind spot ("a loss that new records have already
covered again goes unseen"). The other way, checking each reference against its target's time,
leaves F5 open (a claim's uid is a hash of its record's seq), needs a check at each of five places,
and trusts a clock that steps on this PC (docs/resident.md R14). The test of record: restore with a lost
tail, append, and every new seq is above every seq a segment, a copy, a checkpoint or a restored op
names; a set-aside segment judged afterwards takes no line of raw.db's body; the consumers rewound
the lost range. Before it merges, the same damage and restore is rehearsed on a copy of this PC's
store, its config made safe first.

P3. **One closure per store.** What forget takes, and what S6 leaves out, is followed to its end
within one store at a time: raw.db, the backup's segments, each copy set aside. An op is taken
when what it read was taken there; a reference is never resolved in another store (#666).

P4. **Exclusion reads provenance.** S6 holds for every My Work op through P1 and P3: one that read a
record of an excluded session or repository, or an op built on one, is shown nowhere and sent to
no job while the exclusion stands. The egress paths that check an exclusion keep their checks;
the shortlist's matches the agent too (F6).

P5. **Generated values at the route.** A cursor, an id, a keyed print or a version is oboete's own,
never the owner's text, and no redaction rule changes it. The code that builds an answer names
each such value it puts in, with its place; the gate takes out exactly those, walks everything
else and puts them back. Nothing is exempt by its route's name or its key's name, and a value the
builder did not name is walked.

## Slices

1. P1 alone, behaviour unchanged: today's forget, backup, restore and My Work S5 tests pass as they
   are, as the golden set, and F1 and F2 are written as failing tests to keep for slice 2. The
   per-kind cases in the judge, the backup purge and the restore cut are counted before and after,
   to show the change pays.
2. P2 and P3, and F3 to F5 checked under them.
3. P4 with S6 for every My Work job (slice 4 needs it), and F6.
4. P5, its tests the findings of #373, #570, #576, #597, #635, #670 and #673, one per generated
   value.

This note's own PR stops after two review rounds whose findings are mechanism only (no change of a
rule): what is left goes into an issue as acceptance tests for slice 1, and the PR merges without a
further commit.

## As built

### Slice 2a: a restore's cut for every kind (2026-10-27)

- **F1**: a restore cuts each device's ops at the first op made from records it did not bring
  back, whatever its kind, every later op of the device with it (`Rebuild::cut`). An op is past
  the restored records when the highest record of its device it names (`Provenance::top`: its
  span's end, the spans it read, its goals, its quotes of that device) passes the device's last
  restored record. A migration whose span misses a record is cut too, as before. A turn op and a
  My Work op were kept before.
- **#690 item 3**: a span's end is read alone (`Provenance::end`), so a window op with `to_seq` and
  no `from_seq` is judged by it, as the SQL before #687 did.
- **The order after it**: slice 3 (P4) comes before the rest of slice 2. My Work's next slice
  (goals) needs S6 for every job, and P4 closes within raw.db, which the rest of slice 2 does not
  change. That rest, slice 2b (P2, P3 across stores, F2 to F5), follows as disaster-recovery work.
  Its plan was reviewed before the code: P2's marker record and op (the way this note offers)
  rather than a floor kept in raw.db's meta, which no backup holds; and the backup's segment check
  and set-aside rule must accept the gap a marker names.
