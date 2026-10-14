# Records a hook could not write (parity row 12, G03)

claude-mem's hooks send to its worker; what they cannot send waits in a spool file and is sent
again for seven days (src/shared/hook-spool.ts, 13.30.0). oboete's hooks write raw.db themselves,
so a stopped worker loses nothing, but a write that fails after its 2 s wait (a restore holding the
store, a long write elsewhere, an I/O error) drops that hook call's events today: MUST-M16's marker
says recording failed, and nothing brings them back. This keeps them and writes them later.

## Decisions

- **U1. What is kept.** The events the failed write would have appended, as they would have been
  stored: after the gate (spec 2.2), each with its ledger rows and the ruleset version. Never the
  payload as the agent sent it. One file per hook call, `<home>/unwritten/<n>.json`, `n` one past the
  newest file's there or in `bad/` (U4), chosen under the keep lock (U5) and padded to 20 digits, so
  the names sort as the files were kept, those of two processes in one millisecond too, and a file
  set aside never takes the name of one set aside before; mode 0600, written to a temporary name
  and renamed, so a file is whole or absent, and the directory and the home synced after the rename
  (the home every time: a keep killed after making the directory left its entry unsynced): the
  rename is the only copy's entry. A temporary file found
  under the keep lock is a keep's that was killed before its rename, and is removed. Not `spool/`: v1
  left a directory of that name, and `oboete migrate --finish` deletes it.
- **U2. When.** A hook whose store does not open within its 2 s, or whose append fails, keeps the
  events it built. Built without a store, they lack two things the store gives: an event whose
  agent names no session is named for its device when it is written back (`own_session`), and
  Cursor's SessionEnd recovery, which counts the turns raw already holds, is not run, so those
  turns are not recovered by that call. agy's step claims stay taken: the kept event is that step.
- **U3. Written back.** By the next hook call whose store opens, before its own events, so the
  seqs follow the events' times: a call visits at most the oldest 16 files and writes at most
  **256 KiB of serialized events**, including their labels and ledger rows, across all files.
  This leaves time for the hook's own keep and marker inside the shortest 3 s timeout after the
  store's 2 s wait (#443). Parts end at event boundaries. The first event is admitted even when
  larger than 256 KiB, so every call with a writable store can advance: the capture cap is per
  field, not per whole event. Each part commits with its ledger rows and consumed event offset
  in raw.db (`unwritten_progress`), under synchronous=FULL; no offset becomes durable before
  its rows. An event refused by forget or capture exclusion still advances the offset. The
  file stays whole, in its existing format, until its final part commits, then is removed.
  Reading and decoding the file still reads the whole file, up to U5's bound; the byte budget
  limits the rows and ledger written, rather than that decode. One call at a time (a lock beside
  the files): two at once would append a part twice. While older files remain after a call's
  write-back (a partial file, more than 16 queued, or another call writing them back), the call
  keeps its own events behind them instead of appending them, so the seqs keep the events' order;
  it appends them only when they
  cannot be kept, and an agy step they claimed stays claimed then (the append may still write it;
  only events whose last chance failed give their step back). The file's content hash is kept in
  raw.db (`unwritten_written`) in the transaction that writes its final part, so a file left behind
  by a call stopped between the commit and the removal is removed by the next write-back, not
  written again (#454; not its name, which starts again from 0 once the directory empties). Each
  keep puts a random id in its file, so the hash names that keep alone: two calls in one
  millisecond with one payload keep the same events, and both are written. A file kept before
  the id has none, and two of them can hold the same bytes: its hash is not kept, so it is
  written whole, in one transaction, as before #443, and if left behind, written again (#460).
  Only a file with an id is written in parts. Existing whole-file receipts are still read, and
  raw.db format 2 keeps an older binary from ignoring a partial offset (docs/store-format.md).
  A raw.db put back from an older copy lost those hashes and offsets too, and may write such a
  file again: a duplicate, never a loss. A write-back that finds raw.db damaged keeps the call's
  events behind the files and fails, so the marker is set and the worker is asked to restore the store; any
  other store error stops the write-back and the call goes on, the files written back before it
  counted as the call's write (the marker is cleared, the worker started). When the call's own
  append fails after the write-back, the marker is set and the worker is started all the same,
  for the rows written back.
- **U4. What cannot be written back** (an unknown version, unreadable JSON, an event raw refuses)
  moves to `unwritten/bad/` under the keep lock (U5), never deleted by oboete; doctor counts it.
- **U5. Bound.** Nothing more is kept while `unwritten/` holds 64 MiB, the new file and `bad/`
  counted (oboete never deletes what it set aside): the marker alone says the write failed, as
  before. One keep at a time checks the bound, names its file and renames it (a lock beside the
  files), so overlapping failed calls cannot each pass it, and a move to `bad/` takes the same lock,
  so the bound counts each file once; a keep waits half a second at most for the lock, so the hook
  still keeps its events and sets its marker before the agent's deadline (a SessionEnd hook has 3 s,
  and the store's wait takes 2 of them). A full disk usually refuses the file too; then likewise.
- **U6. What is said.** The marker and its line stay: the write did fail. Its time is when the store
  failed, taken before the keep, which may wait and sync: overlapping hooks order the marker by it. While files wait, the
  line says how many hook calls are kept and that they are written when a write succeeds; doctor
  says the same, and counts `bad/`. A `unwritten/` that cannot be listed may hide kept files:
  doctor says so and fails, and `oboete replay` refuses the home; a hook call appends its events
  then, since a keep names its file from the list (U1) and could not keep them either.
- **U7. Settings changed between the two.** A kept event keeps its gate's masks and ruleset
  version: a rule added since reaches it as it reaches every record stored before the rule
  (rescan). It was shaped by the capture settings of its time (`store_prompts`, `tool_output`).
- **U8. Forget and the deletion canary.** A kept event is no record yet: no forget can name it
  until it is written. Its file holds record text, so the canary's file list (spec, M4 deletion
  canary) covers `unwritten/`. Once written back it is a live record, which a forget names by
  where and when it was recorded (milestone 5 slice 3a), and U3's hash keeps a file left behind
  from writing it again.

## Tests

1. A hook whose store is held keeps its events; the next hook call writes them back before its
   own, in time order, and the file is gone. The kept file holds the masked text, never the
   secret, and is 0600 on Unix.
2. A file that cannot be written back moves to `bad/`; doctor names both counts.
3. Past the bound nothing is kept; `tests/disk_full.rs` still passes (nothing kept, the marker).
4. An agy prompt whose append fails is recorded once: kept and written back, or, when it cannot
   be kept, captured by the next hook through the step claim given back. One that cannot be kept
   behind older files and is appended is recorded once too.
5. A file several times the byte budget advances one part per call. Its seqs follow the events'
   order, and each call's own events wait behind the remaining parts.
6. A call stopped at a part's commit boundary neither duplicates nor loses that part on retry;
   its rows, ledger and consumed offset commit together.
