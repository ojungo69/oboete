# Raw retention (milestone 5, slice 3e-3)

Plan of the slice that builds spec 6.2's retention expiry (owner decision 16; the setting in 1.5,
default forever): the text of a raw record past the period the user sets is removed, on this
device, once curation and the summaries are done with its session. Written before the code, as
the other slices' notes are, and checked against the code by three rounds of review before it
(Codex); the last two found mechanism only, so, as for a design doc here, the rest is left to the
implementing pull requests' reviews. Decisions
marked (Claude; overrulable) are technical ones the plan leaves to the PR
(docs/milestone-5-plan.md, 3e-3).

## What an expired record is

The row stays an event (`type = 'event'`) with its device, seq, time, source, kind, labels (agent,
session, repository, branch, head, git directory, working directory), ledger rows and import
identity (`import_origins`). Its body becomes empty, its original size is dropped, and a new
column, `expired`, holds the time it expired. `Raw::after` reads such a row as
`Item::Expired`, with the labels and no text. (Claude; overrulable)

- **A mark that is there**: a reader tells an expired record from a forgotten one by the
  `expired` column, never by a tombstone or a denial missing (a restore from an old segment can
  lose a tombstone, and a forgotten record read as expired would show its claims again). A
  whole-record tombstone wins over it: `hide` reads a tombstoned expired record as Removed, as it
  reads a tombstoned event, before and after the purge.
- **Still an event to every query that reads labels**: forget's selection and its live
  identities, the session and repository lookups of the exclusion list (`sessions_in`,
  `sessions_over`), a session's first record for the transcript cutoff (`earliest_by_session`),
  the session of a quote (`session_key`) and the label checks that read the ledger
  (`verified_session`) all select `type = 'event'` and keep finding it, labels and ledger as they
  were. A forget of an expired record removes it as any other: `purge_records` rewrites it to a
  Removed row.
- **Why the row stays**: a seq is a row's (`MAX(seq) + 1` is the next), a restore keeps a
  migration's ops only while every seq of it has a row, and the curation checkpoint stays.
- **Body readers**: every query that reads `body` is checked for the empty body; the compression
  consumer, the rescan and the masks pass an expired row over.

## Store format

An older binary reads an expired row as an event whose text is empty: it would curate it again
as nothing and drop the claims anchored on it in a rebuild. So the home's format moves to 3
(docs/store-format.md), but only with the first expiry batch: this binary reads up to 3, stamps 2
on open as before (`FORMAT` is split into the readable maximum and the open's stamp; a stamp is
never lowered), and raises raw.db to 3 in the transaction that commits the first expiry batch
(step 1 below), before any backup or copy holds an expired line; a restore that brings back an
expired line, and a copy rewritten with one, are stamped 3 too. A home where no batch ever began
(retention never set, or set with nothing eligible yet) still opens in the previous binary (the
rollback of spec 7.5); one where a batch began is refused by it with its newer format error, and
the rollback note says so. A
backup line of an expired record has its own type, `expired`, which an older binary's restore
refuses rather than reading as an event.

## What expires

Records expire a curation window at a time: the records of a window op's range (the windows of
the checkpoint tile the seq line; a recuration's window joins those it overlaps), so no window is
ever partly expired, as a forget or a mask that rewrites a window relies on curating it again.
A window expires as a whole once all of these hold. (Claude; overrulable)

1. Its newest record is older than the period: `MAX(ts) < now - days × 24 h`, each record's own
   time (an import's is the original's).
2. Every record of it is at or below the whole-record curation checkpoint
   (`curate::curated_through_in`), and no session with a record in it has a record above the
   checkpoint: a later window of a session reads its earlier records (the first prompt as the
   goal, an earlier reply's numbered options for a later "1"), so a session's windows stay while
   any of it waits for curation.
3. None of its records waits for curation: none is in a skipped or refused window, whatever the
   reason (an import's parked records, an excluded repository's window, a window every provider
   failed, one refused for its quotes), none is in a window's `excluded` list (an excluded
   session's records inside a curated window, never read), and none is in a window queued for
   recuration (knowledge.db `recurate`), unless a later recuration curated it (`curated_parts`).
   Spec 6.2 lets raw "skipped with a reason" expire; each of these has a command or an undo that
   curates it later, so none was curated, and RD/issue50.md:74 forbids discarding unprocessed
   data automatically: the spec's line and A55 are amended to say so.
4. For each session with live records (`hook`, `replay`) in it: the turn of each of its records
   there was asked for a summary (the session's next reply at or after the record is at or below
   `turns::last_asked`) and none of its turns waits kept back by the exclusion list (a `turns` row
   with `excluded = 1` and none asked after it for the same reply). Records after a session's last
   reply are no turn the summaries will read. A session of imported records only has no turns to
   wait for.
5. Every record consumer's checkpoint of the device (knowledge.db `checkpoints`, not the op
   consumers' `op_checkpoints`) is at or past its newest record.
6. A session's windows expire in order: a window with a record of a session whose earlier window
   stays (it waits, or is not past the period) stays too. So a session's expired records are
   always its first ones, which Cursor's recovery relies on (below).
7. No rescan mask is owed (`Raw::masks_owed`): see "Claims stay".
8. Every quote of one of its records reads (`Quote::Reads`), in every claim op in raw whose
   evidence names it, whichever window's batch wrote the op (a recuration's, or a later window's
   that carried a claim), the ops a forget or a mask rewrote aside. An expired quote stands on
   the quote its op keeps, with no text left to check it against, so an op whose derivation was
   refused because its quote stopped reading (a record's text rendered another way by a later
   binary, say) would make its claim in the next rebuild; a recuration of its window writes new
   ops and leaves that one in raw. Such a window keeps its text instead. This PC's store has none
   (2026-10-16: every skipped claim op is a forgotten one).

An `oboete pref add` directive (`kind = 'directive'`, the owner's own words and a preference's
only evidence) and a v1 `touch` row (labels only) never expire, and do not keep their window from
expiring: the rest of it expires around them. A session resumed after some of its windows expired
reads those records as gone in its later windows; a session that stays open keeps every window
it has a record in until all of it is curated; doctor counts the records past the period that wait
so. A window skipped, refused or queued that is curated again after the windows around it expired
reads them as gone too. Curation turned off stops the checkpoint, so the records it never read
stay; those it read before may still expire.

Measured on this PC's store (2026-10-16, counts only): a group of records sharing a session or a
window, the first form of this rule, joined 577,403 events into 3 groups, all of them waiting, so
nothing could ever expire; a window is at most 2,000 records (812 windows). Today 290 windows are
an import's parked records and 405 more wait on two sessions still open, so nothing would expire
yet at any period: the store's older records are imports no one has curated.

## How a pass runs, and what a stop leaves

The windows are read in one pass over this device's events in seq order, with no body (577,403
rows on this PC's store read in 1.8 s by a script), and the window ops. A batch is what one pass
takes. Its steps, in order, each finished before the next starts:

1. **The boundary**: in one raw.db write transaction (captures wait behind it), each window of the
   batch is checked again (no record of its sessions came since it was read; a window whose
   sessions changed waits for the next pass), the batch's records (device, seq, time) go into
   raw.db's `expiry_owed` table and raw.db is stamped format 3. That commit is the boundary and the
   authority: a record captured after it is in a later window. Then the same list is written to
   `state/expiry-owed` (synced, renamed into place, its directory synced), the copy a restore
   reads when raw.db is lost; a pass that finds the table and no file writes the file again before
   step 2. From here the batch is finished whatever the setting says later: a pass that finds owed
   rows resumes them first, a wrong or removed setting included. No backup holds an expired line
   before the home is format 3, so a previous binary never meets one in a home it may open.
2. **Backups and copies first**: the record segments that hold an owed seq (a segment's name
   carries its seq range), live and set aside, are rewritten as forget's step 4 rewrites them
   (`backup::walk` and its sealed replace), each line judged by device, seq and time (a line of
   another time is another record that reused the seq): an `event` line becomes an `expired`
   line, labels, ledger and identity kept. Every raw.db copy set aside is rewritten against every
   expired record raw.db holds and the owed ones (a restore makes copies after a sweep). So no
   backup or copy holds the text once raw.db does not.
3. **raw.db**: the owed rows become expired rows, in batches under dispatch as a purge's are,
   once no mask is owed (`forget --continue` runs the masks' purge first; while one is owed the
   batch waits).
4. **The WAL**: truncated as a purge's is (`truncate_wal`); readers that keep it from truncating
   leave the batch owed for the next look.
5. **knowledge.db**: built again from raw.db, as a forget's purge builds it (its step 3). A
   full-text index keeps a deleted row's trigrams and their places in its segments until they
   merge, so only a new file holds none of the text. The readers of part 1 index no expired text,
   mark its vector key `expired` and give its labels' facts; no carry takes an expired record's
   vector. The knowledge.db copies set aside are carried from and removed. Then the owed rows and
   `state/expiry-owed` go.

A restore (`backup::restore`) applies `state/expiry-owed` to the raw.db it rebuilds before it
swaps it in: the owed rows go into its `expiry_owed`, it is stamped format 3, and an owed record
restored as an event whose time matches becomes expired, so a batch cut by a damaged raw.db is
finished on what replaces it. The original it quarantines is a copy the next pass rewrites
(step 2). Step 5 removes the file after the table's rows.

As built (part 2):

- Before step 1 the pass seals the backups of the records and the ops (`backup::export_timed`)
  and begins nothing when that fails, so a restore that finishes a batch also brings back the
  claims and summaries made from its records (Codex's review of part 2).
- `state/expiry-owed` holds one `device seq time` line per record.
- A restore's rebuild runs with `secure_delete`: the text it clears for an owed batch, or for a
  forget it applies, leaves nothing in the file's free space.
- A copy whose WAL a reader keeps from truncating is removed, as a purge removes a copy it cannot
  rewrite.
- Step 5 is a rebuild of knowledge.db (the commit security review of part 2: rows deleted from a
  contentless trigram index leave the trigrams and their places in its segments, and FTS5's
  `secure-delete` cannot act on a contentless table). It costs what `oboete rebuild` costs, about
  5 minutes on this PC's store (slice 3c-1), once a batch; while it runs, search and the
  per-prompt context answer as during `oboete rebuild`. A seq another record took is that
  record's, and the rebuild indexes it.
- Rule 4 is read per session: a session with a turn the exclusion list keeps back keeps every
  window it has a record in. Rule 5 reads every record consumer, compression's checkpoint too.
- The pass takes the worker lock only when a period is set or a batch is owed.

## Claims stay

Spec 6.2: claims stay, and their evidence reads "raw expired". One function gives each quote's
state, for every reader: it reads (an event whose text at the quote's place is the quote), it
expired (an expired row no tombstone hides), or it is gone (forgotten, masked over, never here).

- **Derivation** (`consumer::claims::derive`, which a rebuild and a rewind run again) and
  **Anchors** (which checks every quote of a claim another record's tombstone touches): an
  expired quote stands, on the quote the claim op keeps; the claim's repository and time come
  from the expired row's labels. Only a gone quote drops the claim and queues its window; an
  expired quote whose offset or length cannot hold it (negative, or a length not the quote's
  bytes) is gone. Which quotes read before the text goes is rule 8's.
- **Masks come first**: whether a rescan's mask covers an expired record's quote cannot be told
  any more (the text it is read against is gone), so expiry never runs ahead of a mask. A mask's
  purge rewrites the claim ops of the masked record's window, the ones whose quote it covers among
  them, and its recuration writes them again from the masked text (slice 3e-1); until then the
  mask is owed. No batch begins, and an owed batch's raw rows are not rewritten (step 3 below),
  while a mask is owed; a rescan reads no expired text, so no mask comes after. A claim that
  stands on an expired quote therefore never stands on one a mask covered.
- **Status** (3d-3, #167): a status a record raised stands while that record is expired, as while
  it is there: expiry is not evidence against what the record showed, and a decision that drops
  to a proposal because time passed would change what agents are given. (Claude; overrulable)
- **Readers**: an expired quote reads `raw expired`, as spec 6.2 says, and the quote is not
  shown: a redaction rule added later may need the words around the quote, which are gone, to
  hide it (`redact::outbound_quote` reads the whole record), so a quote that can no longer be
  checked is withheld. `get` prints `- <device>:<seq>: raw expired`, the viewer the same, `cite`
  gives `live: false, expired: true` and no quote. The claim's body is shown as now, through the
  egress gate. The claim is labelled `quote-only`: its record can no longer be opened. `get` of an
  expired record's id says its raw expired.

## Recuration

- `oboete recurate` refuses a span that holds an expired record, whichever list it comes from:
  "records 120-180 hold raw that expired under [retention]: only their claims' kept quotes remain,
  so they cannot be curated again" (spec 6.2, RD/issue50.md:75).
- No window that holds an expired record is queued for recuration, by `queue` or by the Claims
  consumer's forgotten-window queue. The rows decide, at a forget's own rebuild and at any later
  one alike (a forget's purge rewrites the rows before its rebuild): a window whose expired
  records a forget took every one of holds none any more, only text that never expires
  (directives, v1 touches) and Removed rows, and is queued as any forgotten window is.
- A forget of a record in an expired window still rewrites what forget rewrites (the window's
  summary, the claims of its batch, slice 3d), and nothing can curate the window again to give the
  rest back: forget's guarantee comes first. Forget also finds the claims that copied a forgotten
  record's words through another quote by comparing their text with the record's
  (`settle`, `stored_texts`), which an expired record no longer has: for an expired forgotten
  record, every claim a later window was shown or carried from the rewritten window's batch (the
  window ops' `candidates` and `carried`) counts as copied, and its window is taken as a copy's
  is. (Claude; overrulable)
- A rescan reads no expired text, but a mask of a live record follows its claims into the windows
  that were shown or carried them (`Judge::window`), an expired window among them: that window's
  summary and claims go as a live one's do, and nothing curates it again. Privacy comes first, as
  for forget.

## Search and vectors

- The FTS consumer reads an expired row into `raw_docs` (labels) and `session_start` and indexes
  no text, so a rebuild and a rewind keep where each session started (#506's mapping). `reindex`
  (a tombstone's target, read again) tells an expired record from a removed one: an expired one
  keeps its `raw_docs` row and `session_start` and loses only its text row and vector; a removed
  one loses all, as now.
- The embedding phase marks a record it reads as expired `expired` and sends nothing; a carry
  (`carry`, `carry_set_aside`) takes no vector of an expired record, as it takes none of a
  forgotten one.
- The gaps consumer counts an expired record as recorded (`Raw::turns` reads the labels); an
  expired session end is not checked again by a rebuild, as its transcript's path went with its
  text. The manifest consumer keeps the facts an expired record's labels give (an event, a
  prompt, a reply, an end) and none its text gave.
- Cursor's SessionEnd recovery matches the transcript's turns to the records by their text,
  which an expired record no longer has. A session's expired prompts are its first ones (its
  windows expire in order, and each SessionEnd recovered its missing turns before), so recovery
  passes over as many of the transcript's first turns as the session has expired prompts and
  matches the rest as now: an unchanged transcript records nothing, and a resumed session's new
  turns are recovered. A transcript with fewer turns than that (cut short, or another file) cannot
  be lined up, and nothing is recovered from it. A limit: a turn that neither the hook nor a
  recovery recorded before its session's first records expired takes the place of one that was,
  which is then recorded again from the transcript (Cursor's own file still holds its text). Every
  other reader that compares a record's body to find what is missing is checked for the same.
- Forget's rewrite of the segments (`purged_record_line`) turns an `expired` line of a forgotten
  record into a removed line, as an `event` line.

## The setting and the schedule

- `[retention] days = N`, a whole number of days from 1 to 36,500; no key, or no table, is
  forever. A value out of range, or not a whole number of days, makes the table wrong: no new
  batch is taken (owed rows still finish) and doctor says so (`config::retention`). The hooks
  ignore the table (`CaptureConfig`), so a wrong one never stops capture.
- The worker looks at most once a day, as it looks for a forget left unfinished
  (`forget::continue_due`), and every 10 minutes while rows are owed: it starts
  `oboete forget --continue`, which runs the pass after forget's own work, under the same purge
  lock and the worker lock (the resident worker steps aside, as for a purge).

## What it does not remove

Expiry removes records' text, not memory: claims keep their quotes, cards and session summaries
keep what curation wrote, turn summaries keep theirs, and the ops that hold them are not
rewritten. Spec 6.2 and section 8's retention line say so: the "0 hits" for an expired span is for
the records' bodies outside what claims quote and summaries say, in raw.db, its WAL and copies,
the backups, and knowledge.db, which is built again. R2 holds nothing until milestone 6.

## Doctor and the page

- doctor: `retention: forever` or `retention: N days (a window expires once curated and past
  it; last look <time>, M records expired)`, a batch begun and not finished, and a wrong table
  as a failure.
- The settings page (its own PR): the key in the detailed view's privacy group, in English and
  Japanese.

## Pull requests

1. **Readers**: the `expired` column and `Item::Expired`, the format split, every reader above
   (claims, Anchors, search, recuration and the queues, FTS and `reindex`, embedding and the
   carries, gaps, manifest, `hide`, Cursor's recovery, forget's copy finding and its segment
   rewrite), the `expired` backup line and its restore. Nothing writes an expired row yet but the
   tests.
2. **The pass**: the setting, the windows and their eligibility (rule 8's scan of every claim
   op's quotes among them), `expiry_owed`, its file and the
   five steps, restore's application of the owed file, the copies' sweep, the worker's look,
   doctor, the spec's amendments (6.2, A55, section 8's line, 7.5's rollback note).
3. **The page**.

## Tests

- Readers: an expired claim's quote (withheld, `raw expired`), label, `cite` and `get`, also
  under a rule added after the expiry that needs the quote's context; a rebuild and a rewind keep
  the claim, its repository, time and status, and `session_start`, a historical range tombstone
  on the expired record included; a claim quoting an expired and a live record survives a mask
  of the live one outside its quote; a forget registered on an expired record hides it at once
  (`hide`, `cite`, the claim, support), its purge removes it, and its segments' line becomes a
  removed one, and a window whose every expired record forgets took is queued again; `recurate`
  refuses and no queue takes an expired window; the vector queue, the
  backlog and both carries pass it; Cursor's recovery records nothing again for a session with an
  expired record; restore of an `expired` line, and an older binary's refusal of it and of
  format 3.
- Windows: a recuration joining the windows it overlaps, an exempt row inside a window, a
  session's windows expiring in order; how many records wait and why, measured on this PC's
  store with the scan.
- A forget of an expired record whose words a claim of another window copied through another
  quote takes that claim and its window; a mask of a live record whose claim an expired window
  was shown takes that window's summary and claims, through a rebuild.
- The pass (A31, A55): a window with a record above the checkpoint, a parked v1 or transcript
  record, an excluded record, a failed or refused span, a queued span, an unasked or kept-back
  turn, a newest record inside the period, a record of a session still waiting, or a record a
  claim op quotes where its quote no longer reads (rule 8), stays whole; a
  directive stays; a finished window past the period expires whole; a window of imported records
  only, once recurated, expires; a record captured in a window's session just before the boundary
  keeps the window, and one just after does not.
- The expired text is in no byte of raw.db, its WAL, a copy or a segment (a canary in a record's
  body that no claim quotes); a restore from the segments brings back expired rows, not text.
- A stop after each step, finished by the next pass with the setting removed; a reader keeping
  the WAL from truncating; a restore during and after step 2, with the setting removed, its
  quarantined original rewritten by the next pass; the previous binary after a stop in step 2.
- The previous binary opens a home where no batch began and refuses one where one did, before
  any row expired.
- The settings: no table, `days = 0`, a negative, a fraction, a string; hooks record with each.
