# A home written by a newer oboete is refused

Plan of the slice that adds a format stamp to the home and refuses it in an older binary, written
before the code, as the other slices' notes are. It comes from a comparison with claude-mem 13.37.0
(2026-10-13): claude-mem 13.36 stamps the newest schema that wrote its database and refuses
persistent changes from an older binary. oboete has no such guard: spec 7.3 plans a check only
inside the updater (its step 2), and nothing stops a binary run directly.

## Why before the switch

After the cut-over the owner's binary is replaced by `cargo install` (and later `oboete update`).
An older binary run against a home a newer one wrote (an install from an older checkout, the
dogfood user's binary on a copied home, a rollback by hand) would write with the older meaning of
everything: ops it does not know, the forget, exclusion and redaction state it does not apply. A
guard only protects from the first binary that has it, so it has to be in the binary the owner
switches to.

## The stamp

- **One number for the home**: `raw::FORMAT`, kept as raw.db's
  `PRAGMA user_version` (raw.db had never set it, so every store before reads 0). It was 1 in this
  slice; #443 raised it to 2, as an older binary would ignore a kept file's partial write-back
  offset (docs/unwritten.md U3) and append that part again. raw.db is the
  store of record, and nearly every process opens it before it reads knowledge.db or
  providers.db (forget's deny rows and the tombstones are there); the opens of knowledge.db and
  providers.db read raw.db's stamp first too, as doctor's provider lines and `oboete resume`
  open providers.db alone. So one stamp covers the home. knowledge.db's
  `user_version` stays its own migration marker (1: `turns_fts` holds the summaries' notes, #403).
- **When it moves**: a release raises `FORMAT` when an older binary would mishandle anything it
  writes: raw.db's schema or the meaning of its rows and ops, knowledge.db's or providers.db's, a
  backup segment's lines. A change an older binary reads correctly (a column it ignores, an index)
  does not raise it. The release notes say when it moved.

## The refusal

- Every open of raw.db reads `user_version` right after its connection, before anything writes
  the file (its journal mode included), and refuses a value above `FORMAT` with a typed error,
  `raw::NewerFormat { found }`: "raw.db was written by a newer oboete (format 3; this one reads up
  to 2): update oboete". The writable open (`open_within_report`, under every `raw::open`) and
  `raw::read_only` both check, and so do the opens of knowledge.db and providers.db
  (`raw::refuse_newer_home`, before they create or change anything). A refused open changes no
  byte of the file.
- The writable open raises a lower value to `FORMAT` before its schema and migrations, in an
  IMMEDIATE transaction that reads it again, so that of two binaries opening together the newer
  one's stamp stays and the older one refuses it before it migrates anything. A raw.db already at
  `FORMAT` is not written: a hook's open stays read-only when the schema is complete, as now.
- The open reads it once more before it returns its handle (a newer binary may have stamped the
  file while this one migrated it), and each append of records or ops (captured events, kept
  events, imports, tombstones, ops) reads it in its own transaction: a handle opened before a
  newer binary stamped the file appends nothing after. Other writes of a handle opened before
  the stamp (a purge's rewrites) go on until its next open; a worker checks at every batch
  (below).

## Who says what

- **A hook** records nothing and exits 0 (MUST-M16), with a new failure class `newer`, so doctor
  and the next SessionStart say "recording has failed since T (a newer oboete wrote this home:
  update oboete)". Its kept events (docs/unwritten.md) wait as for any failure, and the newer
  binary writes them back.
- **doctor** says it in its store sections ("cannot read raw.db: raw.db was written by a newer
  oboete …") and turns red. **The worker** exits with the error; a refused open is never taken
  for damage (`backup::damaged`), which would restore an older home over the newer one. **The
  MCP server and the viewer** answer each request with it.
- **A worker** that opened raw.db before a newer binary stamped it would keep writing with its
  own meaning. Between two batches of its consumers and at each wait tick and phase round, it
  reads `user_version` too, opens the store again on a newer one, and stops on the refusal. The MCP server's tools, search and the viewer's routes open raw.db
  for each request, so each meets the check.
- **The updater** (spec 7.3, step 2) runs the same check with the staged binary.

## Limits

- A binary from before this slice cannot refuse: MUST-M17's update path stops the worker first,
  and the release that moves `FORMAT` past 1 says so in its notes.
- Two binaries opening one home at the same moment: a newer one may stamp between this open's
  first check and its last. In that window this open writes only its setup: the journal mode,
  WAL, which every oboete store already has (so nothing is written), and its migrations, which
  create what is missing and add a column only when absent (`CREATE ... IF NOT EXISTS`), so they
  write nothing a newer schema has unless a later format removed something. The open's last
  check refuses the handle before anything else is written.
- Backup segments carry no stamp: a restore by an older binary reads the lines it knows. A
  release that changes a segment's lines raises `FORMAT`, and the newer restore stamps the
  rebuilt raw.db; an older binary's restore from those segments is not refused in this slice.
- The copies of raw.db set aside are not checked: no reader opens them (forget's purge only
  rewrites the tables and columns it knows).

## Tests

- A raw.db at `FORMAT + 1`, without a column this binary's migration adds back: the writable open
  and `raw::read_only` refuse with `NewerFormat`, and the file's bytes are as they were
  (`raw::tests::a_raw_db_a_newer_oboete_wrote_is_refused_and_left_as_it_was`).
- A raw.db at 0 (every store so far): the first writable open stamps `FORMAT`, a second writes
  nothing, and a handle sees a stamp made after it opened
  (`raw::tests::a_raw_db_from_before_the_stamp_gets_it_once`).
- A hook against a newer raw.db records nothing, keeps its event and marks `newer`, which the next
  SessionStart's line names (`hook::tests::a_home_a_newer_oboete_wrote_is_not_written_and_the_next_session_start_says_so`);
  the class and its line (`failure::tests`).
- A worker whose raw.db is stamped newer meanwhile stops on the refusal at its next round
  (`worker::tests::a_worker_stops_when_a_newer_oboete_stamps_its_raw_db`); a refused open is never
  damage (`backup::tests::a_newer_format_is_never_damage`); `oboete restore` refuses such a raw.db
  and leaves it in place (`backup::tests::a_restore_refuses_a_raw_db_a_newer_oboete_wrote`).
- Mutations, each failing one of these: the open's check, the read-only open's, the stamp, the
  worker's check, the damage guard, the restore's refusal, the failure class.

## Docs

spec 2.5 (the failure class), 7.3 (step 2 uses this check; `FORMAT` and when it moves), the
parity table (claude-mem 13.36's writer stamp), this note.
