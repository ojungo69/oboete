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

Claude-mem everyday import waits for repository mapping. Rebuild/restore and
recuration/finalization retain their later W5 slices. The updater remains unavailable
until its native backend exists. Owner data/runtime, held evaluations and cutover
are not changed by implementation or synthetic acceptance tests.
