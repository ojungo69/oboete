# Going back to an earlier point (parity row X24, 2026-11-08)

claude-mem 15.3 can keep daily snapshots of its database (off by default; a copy 5 minutes after
start, then every 24 hours, the newest 7 kept) and restore a chosen one at its worker's next start,
keeping the live file as a pre-restore copy. `backup run`, `status` and `list` go with it, and so do
a Backups section on its page and a doctor warning when there is no snapshot or the newest is older
than two intervals.

oboete always backs up: raw.db's records and ops go into sealed, checked segments every 30 minutes,
at idle and before a command takes the store ([milestone-2.md](milestone-2.md), Backups). A restore
replays every sound segment, so it brings back the latest state only. This note adds points to go
back to, made from the same segments: no second copy of the store, and nothing a forget took comes
back, because forget and retention already rewrite the segments.

## Rules

- B1. **A point is an export.** Each export that writes a segment, once the device has a record
  backed up, then appends one line to `points.jsonl` in the backup directory: `{"at", "device", "records", "ops"}`. The fields are the
  time the export ended and the seq and op seq where that device's segments then end. A point names
  what the backups held at that moment. It does not use the records' own times: an import's records
  keep their source's times on new seqs (the owner's store has 575,009 imported records, the oldest
  from 2025), so a cut by record time would not be a prefix of the store. The line is appended and
  synced after the segments are sealed. The append, and each rewrite that drops points (B6), holds
  `points.jsonl.lock` in the backup directory, so a point another home sharing the directory notes
  meanwhile is kept. A torn or unreadable line is skipped. A lost `points.jsonl`
  loses the points and no record: `oboete restore` with no point replays every segment, as before.
- B2. **`oboete backup list`** prints this device's points, newest first: each one's time (UTC, in
  the form `--to` takes), the seq and op seq it keeps through, and how many records and ops raw.db
  holds past it. It prints every point of the last 24 hours and the newest point of each day before
  that; `--all` prints them all. It reads only `points.jsonl` and raw.db, and changes nothing.
- B3. **`oboete backup run`** exports now, under the worker's lock: it asks a running worker to step
  aside, as `restore` and `rebuild` do. Before the lock goes it indexes what hooks wrote meanwhile,
  as a restore's drain does, with no model call, whether the export went well or not; a failed
  indexing is said and does not hide what was written. It says what it wrote, or that the backups
  were up to date.
- B4. **`oboete restore --to <time>`** goes back to the newest point of this device at or before
  `<time>`. The time is RFC 3339 (`2026-11-07T05:30:12.345Z` as the list prints it, or with an offset),
  or `YYYY-MM-DD HH:MM` taken as UTC. With no point of this device at or before the time, it refuses
  and names `oboete backup list`. Without `--yes` it changes nothing and prints a preview: the
  point's time, seq and op seq, the records and ops that stay and those that go (rows counted in
  raw.db), and the command that goes back to it. `--yes` takes the point at exactly the time given,
  as the preview prints it, so a backup made in between cannot change which point it goes back to.
  The restore checks under its locks that the point is still listed (B6), and refuses if it is
  not. With `--yes` it restores as `oboete restore` does: the current raw.db and knowledge.db are
  set aside, never deleted; knowledge.db is rebuilt; forget requests are applied again. It differs in
  two ways:
  - Records past the point's seq and ops past its op seq are not replayed. Forget and exclusion
    ops past it are appended again, in their order (B5).
  - Every segment that ends past the point, of records or ops, is set aside at the swap under the
    name a damaged segment gets (`.quarantined-<ms>`). So an automatic restore after it cannot
    replay what was rolled back, and a later forget's purge still reaches those files (it walks set
    aside segments too). Nothing is deleted. A segment that the point cuts is replayed up to the
    point and set aside; the next export writes its first part again from raw.db (the cursor's
    rule for a range no segment covers).
  - No point is noted before the first record is backed up: a store then has nothing to go back to
    that a restore does not keep anyway (its forgets and exclusions survive every restore), so no
    point restores a store with no record, and a restore still needs a sound record segment.
  - Each sound ops segment that holds an op past the ops the rebuild kept from the backups (a cut
    by the point or by lost records) is set aside at the swap too: the ops appended again after
    the cut take those op seqs with other bodies, and the next export writes that range again.
- B5. **Going back keeps the two gates.** A forget op past any cut is appended again. That was
  already done for a restore that lost records (D5): forgotten text stays gone. An exclusion op
  past any cut is appended again too, the undone ones with it, the gates' ops in their op order.
  So is an exclusion op that raw.db holds and the backups do not yet (one made after the last
  export), read from raw.db before the swap; the same op is never appended twice. raw.db that is
  missing or damaged gives none (the backups' own are kept); one that cannot be read for another
  reason stops the restore. So going back never sends again what the owner excluded from sending. This also covers a restore of a damaged store,
  which used to drop every exclusion op after its first lost record. Like any new op, one appended
  again is in the backups from the next export, which the restore's own drain runs before the
  command returns. Mutes, corrections, hides,
  work states, imports, curation and My Work's jobs go back with the point. A mute is a reversible
  choice about noise, never a gate (spec 6.1, Mute).
- B6. **No point outlives what it names.** The seqs past a cut are used again by new records, so a
  point past them would name other records. A restore keeps in `points.jsonl` only the points whose
  seq and op seq it brought back: those through the highest seq it replayed, as the lines give it
  (a segment's name can overstate it, when the segment spans a range raw.db had lost), and through
  the op prefix it kept. A gap below that seq (a damaged segment skipped) is never used again, so
  a point past the gap still names the records it named, less those that segment lost. Every
  export drops the points past raw's end (raw lost commits, MUST-M14) before it sets aside the
  segments past it, so a stop or a failure between the two is retried by the next export.
- B7. **doctor** prints the newest point's time and how many records and ops came after it. When
  that point is two days old or more and raw.db holds records or ops past it, the line is a
  warning naming `oboete backup run`, and doctor turns red. The age is measured from the export's
  own time, never from a record's.
- B8. **The settings page** (History and recovery) gets the list and the go-back, with its preview,
  in a later slice. It goes through the same restore, and its consent binds the point and
  `points.jsonl` as well as the files a restore binds now.

## Acceptance tests (this slice)

1. An export that writes a segment appends one point with both cursors. An export that writes
   nothing appends none.
2. A restore to a point leaves raw.db with the records through its seq and the ops through its op
   seq. A forget op past it is kept (the uid stays forgotten), and so is an exclusion op past it
   (the repository stays excluded). A mute past it goes.
3. After a restore to a point, raw.db damaged and restored by the worker still ends at the point's
   seq: the segments past it were set aside at the swap.
4. Records whose own times are older than the point but whose seqs are past it (an import after
   the point) go. Records before the point stay, whatever their times.
5. After a restore, `points.jsonl` holds no point past it. A new export adds one, and restoring to
   the older point again still works.
6. A preview changes no file (raw.db, knowledge.db, segments, `points.jsonl`) and names the point
   that `--yes` then restores.
7. A segment that the point cuts is replayed up to the point and set aside. The next export writes
   the cut part again.
8. A restore of a damaged store that loses its newest record segment keeps the exclusion ops after
   its first lost record.
9. doctor warns when the newest point is two days old or more and records came after it.
10. `oboete backup run` exports under the worker's lock and adds a point.
11. An exclusion that raw.db holds and no export backed up yet survives a restore to a point and a
    restore of the whole backup, and the same op is not appended twice.
12. No point is noted before the first record is backed up.
13. A point that is no longer listed when the restore takes its locks is refused, and raw.db is
    left as it was.
14. When the segment that ends at the point is damaged, the restore keeps no point past the
    records that came back.
15. raw.db that cannot be read for a reason other than damage stops the restore, and stays.
16. A backup run indexes the records that came while it held the lock, even when its export
    fails.
17. When a restore cuts the ops and appends exclusions again in the op seqs it freed, the backups
    afterwards hold those exclusions in place of the old ops, and a later restore brings them back.
18. A backup whose record segments were lost is not restored from its ops alone.
19. A restore keeps no point past the highest seq it replayed, though a segment's name goes further.
20. An export drops the points past raw's end even when the segments past it were set aside
    before.
