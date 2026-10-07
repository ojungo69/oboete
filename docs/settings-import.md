# History import in the viewer

The history import section uses the same transcript and v1 migration backends as
the CLI. Claude Code and Codex use their native saved transcript folders. A v1
source is an explicit local path; leaving it empty chooses the older `oboete.db`
in the viewer's memory home. The destination is always that memory home.

Preview is an explicit action and reads existing source copies without creating
destination stores, configuration, identity or lock files. It reports candidates,
selected bytes, waiting/refused files and settings copy/preserve/default effects.
These totals precede native checkpoint, duplicate and forgotten-record checks.
Transcript preview also includes any conditional v1 pass and its settings effects;
transcript-only counts never authorize an undisclosed migration.

Confirmation binds the current operation, source identity/content/file set,
effective settings and effects. A changed source or setting requires a new
preview. Native import/config locks, no-replace settings copy, checkpoint and
forget checks remain. Earlier committed batches stay committed if a later file
or batch fails; an explicit rerun uses the existing checkpoint/deny rules.

The authenticated status read uses bounded in-memory metadata and opens no
store or source. The viewer keeps one active operation and one last receipt.
Start uses the original synchronous connection admission for its full lifetime,
so a disconnect does not cancel the native import or let the resident viewer
exit during it. The page shows unknown completion and inspection guidance rather
than automatically retrying a lost request. An exact active/last operation id
returns its existing receipt; a different request reusing that id is refused.
Restarting the viewer may lose the receipt, so inspect stored history first.

Progress advances at native committed boundaries. A zero-payload checkpoint is
still committed progress and makes a later failure partial. Migration event
counts describe source rows read; records/repository touches/documents describe
committed payloads. Transcript byte counts describe selected captured bodies
before deny filtering and are not exact saved bytes. No percent or ETA is
invented. Session/identifier lists and backend error chains are not exposed.

Import actions make no curation or embedding request. Later resident processing
follows saved configuration, including an explicitly confirmed copied config.
Import does not submit unrelated settings or preference drafts; copied settings
are inspected through an explicit settings reload.

## Rebuild and restore

Rebuild and restore use the same complete operations as the CLI. Their preview
reads existing private database/WAL copies and backup metadata without creating
stores, locks or identities. It shows current record/change counts, selected
backup work and invalid segments, kept files and deletion requests. Missing or
unreadable counts remain unknown. Candidate rows are not promised restored rows.
A damaged Raw store requires a separately previewed restore; a completed stopped
restore is reported as recovery required and is not finished by a preview.

Consent binds the current home identity, saved configuration, live store/WAL and
backup files, logs and preserved/staged state. Native worker admission comes
before data work. The operation rechecks consent after admission and under the
exclusive Raw fence before moving files. The saved configuration writer lock is
kept through the complete operation. Rebuild and restore run the normal consumers
with no embedding or curation phase; copied vector cache is not a completed
semantic-search generation. Later background work follows saved settings.

Confirmed restore uses the parsed deletion-log snapshot whose contents produced
the confirmation key. Log contents are rechecked before taking the Raw fence;
their file identities, sizes and modification times are checked again under it.
No deletion-log content is read while the exclusive fence is held.

Restore drains the same native consumers even when restoration fails, preserving
the CLI guarantee. A stale unadmitted confirmation performs no data work. Receipts
keep each phase's facts: rebuild file effects, explicit restore effects and any
later automatic recovery. Prepared replay counts become live only when that
restore's swap is known complete. A drain that finishes the same interrupted
staged file records that completion; it does not infer it from candidate counts.
Rollback checks the admitted home before deleting or renaming files, and keeps
known old-file facts if the home was replaced. The last complete stopped Raw and
its live deletion requests remain authority during recovery.

Resident maintenance retains the identity of the viewer's held lock from startup,
including before its first binary restart. A changed or non-regular lock is
refused without replacing it or waiting on a FIFO. Inspection/open failures keep
a pending restore request only while the original home proof remains valid.
The [resident R3 path-check limits](resident.md#limits-to-state-in-the-docs-the-owners-words-are-in-the-review-result-verdictlimits) still apply.

Progress reports actual native effect/consumer boundaries. Its records/ops
checkpoint is an absolute position, not a number of new records or an ETA. Known
progress survives later failure, even at zero payload. Derived/FTS success waits
for the native drain; cleanup/log/backup warnings and retained old files remain
visible. Existing one-active/one-last, exact-ID replay, guarded routes and unknown
completion handling apply to these operations too.

A consumer transaction counts as committed progress when it changes data,
schema or a checkpoint. Store initialization, recovery, committed rewind steps
and deletion-log repairs report their own effects even before a later failure.
A consumer's separately committed Raw writes remain reported effects when its
later step or knowledge checkpoint fails.
Another connection's writes and SQLite storage housekeeping do not count as this
operation's progress. Receipt reporting does not hash whole stores. An empty
transaction does not itself make a failed
restore partial. Quarantined segment counts count each logical segment once;
kept-file counts also include its checksum file.

Claude-mem everyday import waits for repository mapping. Recuration/finalization
retain their next W5 slice. The updater remains unavailable until its native
backend exists. Owner data/runtime, held evaluations and cutover are not changed
by implementation or synthetic acceptance tests.
