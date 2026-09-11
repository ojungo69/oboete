# Device sync contract (US6, T033)

Status: contract only. No code, dependency, schema, network call or real transfer exists for it.
Owner decisions of 2026-09-11 fix the shape: encrypted bundle files in a directory the user
chooses (any file-sync tool moves them), Node `crypto` only, no new dependency, no cloud object
store in 009. Cloudflare R2 remains a later transport behind the same envelope; `research.md` R8
keeps the earlier R2 findings for that day.

Sync moves the user's own knowledge between the user's own installations. It is not sharing with
other people, not a backup of the whole database and not a migration: live quarantined text and
unprocessed captures are not exported, an approval never travels as authority over content it did
not approve, and the migration merger (`transfer-merge.ts`) is never used to apply sync input.

## Fence

- Nothing runs unless a sync space is configured, its consent tuple matches and a human runs
  `oboete sync push` or `oboete sync pull`. No hook, capture, injection, observer worker, doctor or
  MCP path imports the sync module or performs sync I/O. There is no background scheduler in 009.
- Sync performs no network operation. The only I/O is file I/O below one directory the user named.
- Text of a secret memory never leaves the machine (native export rule). Unprocessed captures and
  live quarantined rows are not exported; see "What a replica publishes".
- Encryption authenticates bundles; it never grants authority. Every pulled record passes the same
  field validation as a native import before any row changes, and no pulled record can approve a
  proposal, widen visibility, lower sensitivity or undelete a tombstone.
- Clock values in bundles are display metadata. Nothing is ordered or resolved by them.

## Sync space, replicas and keys

- **Sync space**: one directory the user owns, `<dir>/oboete-sync/v1/<space_id>/`. `space_id` is
  `hex32` random, created once by `oboete sync init <dir>`. A device joins an existing space with
  `oboete sync join <dir>`, which prompts for the key on a TTY with echo off. The key is never
  accepted as a command-line argument, environment variable or file path, so it never appears in
  shell history, `ps` output or logs.
- **Replica identity**: the installation's `replica_identity.origin_id` (0007). It names the
  replica's own bundle file and namespaces every origin ID it creates. Never derived from path,
  host, account or clock.
- **Space key**: 32 random bytes from `crypto.randomBytes`. Stored only in
  `$OBOETE_HOME/sync/<space_id>.key` (file mode `0600`, directory `0700`), never in SQLite,
  `config.toml`, logs, doctor output or bundles. `oboete sync key show` prints the one line
  `oboete-sync-key/1:<space_id>:<base64url key>` for the user to carry to the next device over a
  channel they trust; it is the only command that prints it and it requires a TTY.
- **Key id**: `HKDF-SHA256(ikm = space key, salt = "", info = "oboete-sync-key-id/1", 8 bytes)`,
  hex. A bundle whose key id differs from the local one is reported as `key_mismatch` before any
  decryption is attempted; that is a diagnostic ("another space's key, or a modified prefix"), not
  proof of either, and such a bundle is skipped like any rejected one. The id is safe to store in
  config and bundles.
- Key rotation is a new space. There is no re-encryption in place and no per-device recipient
  list; a lost or leaked key means the user creates a new space and re-pushes from a trusted device.
- `config.toml` stores: directory (as given and as `realpath` at consent time), `space_id`,
  `key_id`, selected sensitivity classes. Nothing else.

## Bundle envelope: `oboete-sync-bundle/1`

One bundle per replica per space: `<space dir>/<replica origin_id>.osb`. A replica writes only its
own file, always as a whole: write `<name>.osb.tmp-<random>`, `fsync`, rename over `<name>.osb`.
Because no two replicas ever write the same file, a file-sync tool has nothing to merge and never
produces a "conflicted copy" of a bundle that Oboete would read. Readers accept only names of the
exact form `<hex32>.osb`; any other file in the directory is ignored.

Binary layout, all big-endian:

| Offset | Bytes | Content |
| --- | --- | --- |
| 0 | 20 | Magic `oboete-sync-bundle/1` (ASCII, no terminator). |
| 20 | 8 | Key id (see above). |
| 28 | 16 | Bundle salt from `crypto.randomBytes`. |
| 44 | … | Chunks. |

Chunk keys: `HKDF-SHA256(ikm = space key, salt = bundle salt, info = "oboete-sync-bundle/1",
32 bytes)`. Chunks are AES-256-GCM (`crypto.createCipheriv('aes-256-gcm', …, { authTagLength: 16 })`),
in order, each followed by its 16-byte tag. Framing is fixed-width, the age payload rule: every
non-final chunk encrypts exactly 65,536 plaintext bytes (65,552 bytes on disk); the final chunk
encrypts 1 to 65,536 bytes, or 0 bytes only when the whole plaintext is empty. The reader needs no
length field: while more than 65,552 bytes remain after the prefix it consumes one non-final
chunk; the last 16 to 65,552 bytes are the final chunk (a stream reader confirms end-of-file by
lookahead before treating a full-size unit as final). The 12-byte nonce is an 11-byte big-endian
chunk counter starting at 0 followed by one byte that is `0x01` for the final chunk and `0x00`
otherwise. Every chunk sets the 44-byte prefix as AAD. A missing final chunk, a non-final chunk
shorter than 65,536 bytes, a counter gap, a remainder shorter than 16 bytes or a tag failure
rejects the whole bundle; nothing partial is ever applied.

Bounds follow from the framing. Total plaintext (header line plus every record line) is at most
256 MiB (268,435,456 bytes), so a file is at most `44 + plaintext + 16 × ceil(plaintext / 65,536)`
= 268,501,036 bytes; a larger file is rejected by `stat` before it is opened for reading. The
header line is at most 65,536 bytes.

## Plaintext: header and revision log

The plaintext is JSONL: one header line, then revision lines. There is no separate native body;
each revision carries its own payload, so multiple heads of one origin, control revisions without
content and history without content are all the same line shape. Field validation of a payload
reuses the native v2 record schemas (`transfer-format.ts`, one schema per kind); reference rules
are sync's own and are stated below.

Header (`oboete-sync-snapshot/1`), fixed-size fields only:

```json
{"format":"oboete-sync-snapshot/1","space_id":"…","replica_origin_id":"…","snapshot_id":"…","revision_lines":42,"heads":7,"revisions_sha256":"…","withheld":{"works":0,"memories":0,"sources":0,"contexts":0,"proposals":0},"produced_at":1760000000}
```

- `replica_origin_id` must equal the file name's `hex32`; `space_id` must equal the directory's.
- `revision_lines` bounds the rest of the plaintext before parsing (line count and total bytes
  are both checked); `revisions_sha256` is the SHA-256 of the exact bytes of all revision lines and
  must match the bytes actually read. Everything is inside the ciphertext, so all of it is
  authenticated.
- `snapshot_id` = SHA-256 over canonical JSON `["oboete-sync-snapshot/1", space_id,
  replica_origin_id, revisions_sha256]`: it identifies the exact delivery (which revisions, which
  of them are heads, which payloads are shipped), so a consent change that ships more payloads, a
  `--republish` after a mapping, or a newly held payload all produce a new snapshot, while an
  unchanged state produces the same ID because lines are canonical and heads are recomputed
  deterministically. Every bundle's ciphertext still differs. `heads` is the count of lines
  flagged `head` and must match.
- `withheld` counts what "What a replica publishes" left out.

Revision line (`oboete-sync-revision/1`):

```json
{"origin_id":"<hex32>:<kind-local id>","kind":"memory","revision_id":"<hex64>","author":"<hex32>","parents":["<hex64>"],"control":{"tombstone":false,"sensitivity_floor":"local_only"},"natural":{"repo":"<repo key>","content_hash":"<hex64>"},"payload_hash":"<hex64>|null","head":true,"payload":{…}|null}
```

Identity fields are `origin_id`, `kind`, `revision_id`, `author`, `parents`, `control`,
`natural`, `payload_hash`; they are immutable once created and are what a reader stores. `head`
and `payload` are delivery fields, recomputed by every publisher. `natural` is the record's
identity in the terms the store already uses to recognize the same record from a different
source, so a reader can alias a control-only revision onto a row it already holds under another
origin. It never contains a local id or a sender-side hash that embeds one: memory
`{domain: "ordinary", repo: <repo key>, material_hash}` (the reader recomputes
`contentHash(local repo id, material_hash)` after mapping, which is how migration recognizes the
same material) or `{domain: "personal_projection", projection_hash}` (repository-independent, as
`memories.content_hash` is globally unique and personal hashes carry no repository) or, for a
checkpoint, `{domain: "checkpoint", repo, work: <work origin>, parent: <memory origin or null>,
material_hash}`; source `{memory: <memory origin>, source_hash: <SHA-256 of the canonical source
fields>}`; visibility `{memory: <memory origin>, audience, repo, work: <work origin or null>}`
(the scope tuple `memory_visibility` enforces UNIQUE, so a migration-created `v_migration:…` grant
and a `v_<hash>` grant with the same scope are one grant); sharing_proposal `{candidate:
<candidate identity hash>, origin_memory: <origin>}`; context `{repo, local_key}`; work
`{work_id}`.

- `origin_id` is `<creating replica origin_id>:<local identifier>`; only the creating replica
  allocates one, but any replica may create later revisions (successors) of any origin it knows.
  Kinds: `memory`, `source`, `visibility`, `sharing_proposal`, `work`, `context`. For a `source`
  the local identifier is `source:<memory local id>:<SHA-256 of the canonical source fields>`
  (sources are immutable evidence; `memory_sources.id` is a reusable rowid and is never used); for
  the other kinds it is the row's own id. Repositories are not revisioned: a `repo` line has only
  `kind`, `origin_id` and its identity fields, and a bundle carries every repo line its payloads
  reference (see "Repository identity").
- `revision_id` = SHA-256 over canonical JSON `["oboete-record-revision/1", origin_id, kind,
  author, sorted parents, control, natural, payload_hash]`. Parent order and member order cannot
  change it.
  `payload_hash` is the SHA-256 of the canonical payload (with references already in origin form)
  or null for a control-only revision. Because the id covers the hash and not the bytes, a
  publisher may ship the line with `payload: null` when the current content filter withholds the
  text, and a reader still verifies the identity; a shipped payload must hash to `payload_hash`.
- Payload references come in two classes, exactly as the native v2 appendix and
  `referencesValid` distinguish them. Entity references are origin IDs or repo keys and must
  resolve, per kind: memory `repo_id`, `work_id`, `checkpoint_parent_id`, `superseded_by`; source
  `memory_id`, `source_memory_id`, `source_context_id`; visibility `memory_id`, `repo_id`,
  `work_id`, `proposal_id`; sharing_proposal `origin_memory_id`, `origin_repo_id`,
  `origin_work_id`, `projected_memory_id`; context `repo_id`; work `repo_id`, `origin_context_id`,
  `current_checkpoint_memory_id`. This one list drives origin conversion on push, the reference
  closure, and the ownership check on apply. Provenance identifiers (`raw_event_id`,
  `source_session_id`, `source_batch_id`, `purpose_source_event_id`, capture roots, source paths
  and the other foreign IDs the appendix lists as carrying no local authority) are kept verbatim
  as metadata and are never resolved to local rows. A reader maps entity references through
  `sync_origins` (0008: `origin_id` ↔ kind, local id) and allocates a local id for a new origin.
- `control` travels on every revision: `tombstone` (the origin is deleted; text empty) and
  `sensitivity_floor` (the lowest sensitivity a reader may hold the origin at). A control-only
  revision (`payload_hash: null`) references nothing, so it is valid and applicable whatever else
  the bundle withholds.
- Every `parents` entry must name a line in the same bundle or a revision the reader already
  stores, of the same `kind` (of the same origin, or of another origin the writer had aliased to
  the same row, see "Merge rules"); a reference to neither, a duplicate parent, a
  self-parent or a duplicate `revision_id` line rejects the bundle. A cycle cannot be built
  (ids are hashes of parents), so validation is one pass plus a bounded ancestor walk in a
  private scratch SQLite exactly as the native reader stages input.
- `head: true` marks a current head as the publisher computes it at push time; a reader recomputes
  heads from the graph it stores and never trusts the flag for anything but the `snapshot_id`
  check. An origin may have several heads (unresolved siblings), and every head is shipped with
  its payload when the filter allows, so a third replica reading only this bundle sees the same
  conflict.
- Received identity fields are stored verbatim in `sync_revisions` (0008) and re-published
  verbatim with the same `revision_id`. Payload bytes are stored when received and erased locally
  (set to null for every revision of that origin) as soon as a `secret` floor or a tombstone is
  stored for the origin; an erased payload is never re-published. Local effective state (selected
  head, quarantine, local approval record, conflict rows) lives in local tables and is never
  derived back into a foreign revision.

## Merge rules

- Identity: the origin ID, aliased onto local rows by `natural`. When a pulled revision's
  `natural` key (for a memory: repository and `content_hash`, which `memories.content_hash`
  already enforces UNIQUE per repository) equals an existing local row created under a different
  origin (two devices captured the same material independently), `sync_origins` maps the new
  origin to that row instead of inserting: one local row, several origins. A row has one head set
  (the union across its origins), one selected head, one conflict report and one
  `materialized_hash`, all kept on its canonical origin (the smallest origin ID mapped to it); a
  local change or a `resolve` creates one revision under the canonical origin, and its parents
  may be revisions of any origin aliased to the row. A reader stores a parent link to another
  origin as received; the link counts for the head set only once both origins map to one local
  row here (by `natural`, or by a repository mapping that makes two `common_dir` keys one
  repository), and until then each row keeps its own heads. Because `natural` travels on control-only revisions
  too, a deletion or secret floor for material this replica created under its own origin aliases
  and applies even when the
  other origin was never seen with a payload. Control from any aliased origin applies to the row
  (a tombstone or floor stored under either origin deletes or raises it); a live payload under one
  origin never undeletes a row tombstoned under another. A later alias onto a row that is already
  terminal inherits that terminal state.
- A revision already stored changes nothing in the graph; if it arrives with a payload the reader
  stored as null (withheld at the time) and the origin has no stored `secret` floor or tombstone,
  the validated payload is stored. Storing a payload never re-applies a revision: the effective
  row is always re-evaluated from the selected head, the stored control and the approval record,
  so a late payload for a superseded ancestor changes nothing visible.
  `oboete sync map-repo` re-evaluates stored revisions that were `withheld_on_apply` without any
  new pull.
- Heads: storing revision R removes from the row's head set every head that is an ancestor of
  R and adds R. If R removed nothing (it descends from no current head) it is a sibling: the row
  is reported in `sync_conflicts` (one row per local row, `id = 'sync:' || canonical origin ID`,
  `status = 'open'`, the head set with each head's origin in the state JSON, `content_hash` when
  it applies). Heads that R neither removed nor descends from stay heads. Rows that
  `observer/checkpoint.ts` wrote before or between syncs keep their own ids; `sync status` lists
  them with the sync rows, and a `resolve` of a work row also marks `resolved` every open
  observer row whose JSON names that work.
- Selected head: the effective row follows one head per local row. It moves to R only when R
  descends from the previously selected head (a local change or a remote descendant of the local
  line); a fresh row selects its first head; ties are never broken by time. A human moves it
  with `oboete sync resolve`.
- A local change to a row creates a revision authored by this replica whose single parent is
  the revision the row was last materialized from (`materialized_revision`, the selected head
  whose payload was applied or written here); when the selected head moved to a revision whose
  payload was withheld, the local change is a sibling of it, so a conflict reports what the
  device could not see rather than silently superseding it. Unresolved siblings stay
  unresolved. Only `oboete sync resolve` creates a revision with more than one parent, and it
  lists every current head as a parent.
- Resolution: a revision with more than one parent whose parents include every current head of
  the row is applied as the resolution wherever it arrives: it becomes the single head and the
  selected head, its payload (for a work, including `current_checkpoint_memory_id`) applies
  without the checkpoint-parent chain check below, and the conflict report closes. Its
  single-parent descendants follow the ordinary rules with it as their base. A multi-parent
  revision that misses a head this replica holds (a sibling arrived after the resolver's pull)
  is one more sibling, and the conflict stays open. A single-parent revision is never a
  resolution, whatever the head set.
- Local change capture: nothing outside sync is instrumented (no triggers, no marks; the hook and
  worker are untouched). `sync_origins` records, for every local row ever materialized here (on
  its canonical origin), the canonical state the replica last wrote or applied for it
  (`materialized_hash`: payload hash plus the control the row implies) and the revision it came
  from (`materialized_revision`). An apply writes it from
  the state the applied revision implies plus the corrections that are not changes of content or
  control (a pulled approval held as `pending` without a local approval record, id mapping); a
  deletion or raise the apply itself performs (lineage inheritance, the checkpoint sweep below,
  the 0005 trigger) is not folded in, so the closing pass sees the row differ and records the
  control revision. Push and pull both begin, inside
  the writer lock and before anything else, with one pass that walks `sync_origins` and the
  tracked tables together: a row whose current canonical state differs from `materialized_hash`
  becomes a new revision authored by this replica (whether a command, the worker, the 0005
  cascade or a pull's own apply caused it); an origin whose row no longer exists (sources and
  grants are physically deleted by `observer/provenance.ts` and `worker/imported.ts`) becomes a
  tombstone revision, distinct from an origin that was never materialized (`withheld_on_apply`);
  a row with no origin yet gets one. Comparing against the materialized state, not the wire
  payload, is what keeps an apply-time correction from being mistaken for a user change. The pass
  is a scan bounded by the store and runs only in these two manual commands. A completed
  work is thus a head before any remote descendant of the older head is compared against it; a
  push restart after a `data_version` mismatch repeats the pass, so a deletion or secret marking
  that landed during staging becomes its control revision before the bundle is rebuilt; a raise
  the 0005 trigger applied to a descendant during a pull is captured at the end of that same pull
  (the pass runs again before commit) and shipped by the next push. A change-free A→B→A round
  trip records nothing, because an applied revision leaves the row exactly at the state its
  payload hash and control describe.
- `control.tombstone` dominates: once any stored revision of an origin carries it, the effective
  row is deleted and no later live payload undeletes it (a live revision that descends from the
  tombstone is a conflict, not a resurrection). Absence of an origin from a bundle never means
  deletion. For checkpoints the deletion unit is what `contracts/checkpoint.md` deletes: a
  tombstone stored for any checkpoint of a mapped work with a given `material_hash` applies to
  every checkpoint of that work and material, already stored or arriving later, whatever its
  parent or origin. Storing such a tombstone deletes every stored checkpoint row of that work
  with that `material_hash` in the same transaction (the 0005 trigger only reaches rows that
  existed on the deleting device, so a peer sweeps the set itself; the closing pass then records
  a tombstone revision for each swept row), and the apply checks stored tombstones by
  `{work, material_hash}` before creating a checkpoint row, so the outcome does not depend on
  the order bundles are read.
- `control.sensitivity_floor` merges by rank upward only, exactly as migration does
  (`SENSITIVITY_RANK`; the raise cascades to local descendants through the 0005 trigger). The
  effective sensitivity of an origin is the maximum of its local value and every stored floor. A
  floor of `secret` also drops local text.
- Lineage inheritance on apply is the migration merge's guarantee restated for sync
  (`transfer-merge.ts` `raiseToParents`): a memory or source applied under a parent
  (`source_memory_id`, `checkpoint_parent_id`) whose effective sensitivity, stored floor or
  aliased row is stricter is raised to that rank in the same transaction, its text dropped when
  the rank is `secret`, whether the parent arrived in this bundle, was already stored, or was
  raised earlier by the trigger (the AFTER UPDATE trigger does not fire for a child that arrives
  after the parent's raise, so the apply walks the lineage itself). Every raise the apply or the
  trigger causes is recorded as a control revision of the raised origin before commit (see
  "Local change capture"), so the next push carries it.
- Work checkpoints keep their existing rule for arriving revisions: a pulled checkpoint whose
  parent is not the current local checkpoint of that work is a sibling head of the work origin,
  not a replacement, and it is reported through the work row's `sync:` conflict row (see
  "Heads"; the row names the checkpoint memory IDs, and the observer's own rows for that work
  are listed beside it). `oboete sync resolve` on a work origin sets, in one transaction, the
  selected head, the work's `current_checkpoint_memory_id` to the kept head's checkpoint, and the
  conflict row to resolved; the checkpoint-parent rule is not re-applied to a human resolution.
- Visibility grants and proposal decisions arrive as revisions of their own origin. An approval is
  bound to what was approved: the local approval record stores the exact candidate hash, projection
  identity and scope at approval time, and a pulled `approved` proposal creates or keeps a
  projection only when its candidate hash, projection and scope equal that record. Otherwise the
  local effective state is `pending` with the remote decision visible; the stored wire revision is
  unchanged and re-published as received.

## What a replica publishes

Every push writes the replica's complete knowledge, so a third replica (or a device joining after
several updates) can catch up from any one bundle: the identity fields of every revision this
replica stores, own history included (`sync_revisions` keeps every revision the replica created
or received), plus the payload of every current head that passes the filter below. Ancestors ship
as identity-only lines. A local change since the last push first creates the new revision, then
publishes it.

- Content filter, evaluated in a fixed order: (1) candidates are the payloads that pass their
  own class rule below (memory, work, sharing proposal; a source with its memory; a grant with
  its memory), (2) a context is a candidate when a candidate work or source references it or a
  candidate of its repository exists, (3) the reference closure below removes candidates until
  nothing changes, and the survivors ship. A memory's payload is shipped only when its effective sensitivity is in a
  class the consent selected (`eligible`, `local_only`, `private`; `secret` is never selectable),
  and a work's payload only when its `purpose_sensitivity` (raised by any stored floor) is in a
  selected class, whether or not the work has a checkpoint; a sharing proposal's payload only when
  its `candidate_sensitivity` (raised by any stored floor) is in a selected class, independently
  of its origin memory and work (`import promote` can attach a `private` pending candidate to an
  `eligible` memory);
  it is not live quarantined text (`review_state = 'imported'` and not tombstoned and not secret),
  and none of its sources still points at an unfinished raw event on this replica
  (`raw_events.classification_state <> 'done'` or `processing_state` not in `processed`,
  `excluded`; `legacy_unknown` counts as unfinished). A source ships exactly when its memory
  ships: sources are provenance rows, with or without retained evidence, and a purged raw event
  changes nothing. A context's payload ships only when at least one payload of its repository
  ships in the same bundle; otherwise the context ships identity-only (its `natural`
  `{repo, local_key}` carries only hashes), so `root` and `repo_secret_paths_json` stay on the
  device, and no `repo` line is emitted for a repository nothing was exported from. Migration
  receipts (`migration_records`) never ship. A foreign head's payload
  ships only when the replica still holds it (not erased) and the same filter allows it.
- Control always travels: for every origin the replica ever published, the current revision is
  shipped even when the content filter withholds the payload, as a control revision
  (`payload: null`, `tombstone`, `sensitivity_floor`). This is what carries "now secret", "now
  private" and "deleted" to replicas that already hold the text, and it is unaffected by
  `review_state` (the 0005 trigger and `worker/batches.ts` set `review_state = 'imported'` when a
  memory becomes secret).
- Reference closure: a payload whose entity reference points at a withheld origin is itself
  withheld (its identity line still goes). A work whose checkpoint chain, purpose or context
  references a withheld origin is withheld as a whole. So no bundle contains a dangling payload
  reference, and no withheld row is promoted by being referenced.
- An origin under an open `sync_conflicts` row is published with all its heads; the conflict is
  local bookkeeping.

### Repository identity

A repository key is fixed-size so it fits every `id512` reference field of the reused native
schemas while `normalized_identity` itself may be 16,384 characters: the `repo` line carries the
full identity, references carry the key. A `remote` repository with a canonical identity
(`isCanonicalRemoteIdentity`) is global: its key is `remote:<SHA-256 hex of normalized_identity>`
and every replica resolves it to the same local repository, creating it when absent exactly as
native import does. A `common_dir` identity is a path on one device, so its key is
`<replica origin_id>:common_dir:<SHA-256 hex of normalized_identity>` and it never resolves by
itself: the reader needs an explicit mapping (`oboete sync map-repo <key> <local repo id>`,
stored in `sync_repo_mappings`, 0008). Until a mapping exists, payloads referencing that repository
are stored but not applied (`withheld_on_apply`, visible in `sync status`), and they apply on the
next pull after the mapping appears. After mapping, repository ownership is verified as migration
verifies it: a memory, source, work, context or grant whose repository differs from its parent's
is rejected.

`withheld` in the header counts the payloads left out by these rules, and `oboete sync status`
shows them by reason.

## Push

0. Take the per-space lock: open `$OBOETE_HOME/sync/<space_id>.lock.db` (a one-table SQLite file)
   and hold `BEGIN IMMEDIATE` on it for the whole operation. A second push or pull for the same
   space gets `SQLITE_BUSY` and exits `busy`. The operating system releases the lock when the
   process dies, so there is no stale-lock state and nothing to reclaim.
1. Recompute the consent hash; on mismatch write nothing and exit 3 with the changed fields.
   Then, on the one connection the push uses throughout, `BEGIN IMMEDIATE`, read
   `PRAGMA data_version` first (the baseline, D0), run the change pass, `COMMIT`. SQLite bumps
   that counter when a connection starts a transaction and finds another connection's commit
   since its previous one, never for the connection's own commits and never inside a held
   transaction, so every later reading that equals D0 proves no foreign commit landed since the
   pass's snapshot.
2. `BEGIN` a read transaction, read `PRAGMA data_version`; if it differs from D0, end it and
   restart from step 1 (a commit landed between the pass and this snapshot). Build the revision
   lines as canonical JSONL into an owner-only temporary file under `$OBOETE_HOME/sync/staging/`
   (hashing and counting as it goes; reject when header plus lines exceed 256 MiB or any line
   exceeds the transfer line limit), compute `snapshot_id`, then end the read transaction. The
   push is a candidate for "unchanged" when `snapshot_id` equals `last_pushed_snapshot_id` for
   this space **and** the published file `<space dir>/<origin_id>.osb` exists with the recorded
   ciphertext SHA-256 and size, and `--republish` was not given; a missing, shorter or different
   published file is re-published from the same snapshot (the file-sync tool may have lost or
   rolled it back).
3. Unless the push is a candidate for "unchanged", encrypt the staging file to a ciphertext file
   in the same private staging directory and `fsync` it. Nothing has touched the space directory
   yet, so a file-sync tool cannot carry a stale ciphertext: the space directory only ever
   receives bytes that passed step 4.
4. `BEGIN IMMEDIATE` on the database (a new snapshot plus the writer lock, so no other connection
   can commit until this step ends) and read `PRAGMA data_version` again. If the value differs
   from D0, some commit happened after the pass (the worker may have made a staged row secret or
   deleted): roll back, delete the temporary files, restart from step 1; after three restarts
   exit `busy`. If it is equal and the push is a candidate for "unchanged", end the transaction
   and stop: "unchanged", no file write (the check runs before this exit, so a secret marking or
   deletion committed during staging is never hidden behind a matching snapshot). Otherwise the
   staged bytes are exactly the current state: copy the ciphertext into the space
   directory as `<origin_id>.osb.tmp-<random>` (a rename when the staging directory is on the same
   filesystem, otherwise a streamed copy plus `fsync`), rename it over `<origin_id>.osb`, record
   step 5 in the same transaction, `COMMIT`. The writer lock is held for the copy, the rename and
   the record, not for staging or encryption (the change pass holds it for its own short
   transaction). A tool that carries the temporary name mid-copy
   carries current-state bytes that fail authentication until complete; readers ignore the name.
5. `last_pushed_snapshot_id`, the ciphertext SHA-256 and size are recorded in the step 4
   transaction after the rename returns. Any failure before that leaves the previous bundle and
   the record untouched; the temporary file is removed. Temporary files left by an interrupted
   push are ignored by readers (name pattern) and deleted by the next push of the same replica.

Push is idempotent: re-running with no local change and an intact published file rewrites nothing.

## Pull

0. Hold the same per-space lock as push.
1. Recompute the consent hash; on mismatch read nothing and exit 3.
2. `readdir` the space directory; keep only `<hex32>.osb` names other than the local origin ID; at
   most 32 replicas per space directory, more is an error naming the count and the file names
   that have no stored cursor (the user deletes bundles of replicas that no longer exist; mtime
   never ranks them, see "Fence").
3. For each bundle: `stat` size bound, check magic and key id (`key_mismatch` skips), decrypt chunk
   by chunk into a bounded owner-only plaintext staging file, and only after the final chunk
   authenticates: parse the header, verify `replica_origin_id`, `space_id`, `revision_lines`,
   `heads` and `revisions_sha256`.
4. If `snapshot_id` equals the cursor stored for that replica, skip it.
5. Validate every line in a private scratch SQLite: schema per kind, origin ID shape, recomputed
   `revision_id` and `payload_hash`, parents resolvable and of the same kind, no
   duplicate line, repo lines present for every referenced repository, no entity reference to an
   origin that is neither in the bundle nor known locally, and the graph bounds below. A bundle
   with any invalid line is rejected whole.
6. Apply in one `BEGIN IMMEDIATE` transaction: first record revisions for local changes not yet
   recorded (below), then apply in two phases. Phase one resolves or allocates the local row for
   every origin in the bundle (alias by `natural`, else a new local id) and stores the revisions
   in `parents` order per origin. Phase two connects references and writes rows: source and
   checkpoint lineage edges (`source_memory_id`, `checkpoint_parent_id`) are applied parent-first
   and must be acyclic, supersession (`superseded_by`) is applied afterwards and checked
   separately, and ownership back-references (`work_id`, `current_checkpoint_memory_id`,
   `origin_context_id`) are plain lookups with no ordering role, exactly the three separate checks
   the native reader performs. A work advances (fast-forward) when the pulled head's
   `current_checkpoint_memory_id` reaches the local current checkpoint through
   `checkpoint_parent_id` links available in the bundle or already stored; a new work is created
   at the pulled head; anything else is the sibling case. Intermediate checkpoints are memories
   with their own origins and ship as heads with payloads, so a joining device rebuilds the chain
   from the current bundle alone. Then store the cursor
   `(space_id, replica_origin_id, snapshot_id)` in the same transaction. Rows created by pull keep
   the `review_state` (`unreviewed`/`reviewed`), sensitivity and origin IDs they carry (the creating
   replica already classified them) and are eligible for retrieval like local rows; a pulled row is
   never placed in quarantine and never gains local approval authority.
7. A bundle that fails at any step is reported by name and reason; the database, the cursor and the
   other bundles' outcomes are unaffected. A truncated or half-copied file simply fails
   authentication and is retried on the next pull.

Pull never writes into the space directory.

## Consent

A sync-specific consent record, separate from observer consent, binds:

```text
transport=file-bundle, directory (as given + realpath), space_id, key_id,
encryption=aes-256-gcm+hkdf-sha256 (oboete-sync-bundle/1), sensitivity classes exported,
"no network"
```

`oboete sync init|join` shows the tuple and stores its hash. Every push/pull recomputes and compares
it first; any difference (directory moved, key file replaced, class selection changed) performs no
I/O and reports the changed field. Removing the space (`oboete sync leave`) deletes the key file,
the local cursors and this replica's own `<origin_id>.osb`, and leaves the other bundles to the
user. Repository identities (hashed keys, and the plaintext `repo` line for every repository a
shipped payload references) and identity-only context and control lines leave regardless of the
class selection.

## Commands and health

| Command | Effect |
| --- | --- |
| `oboete sync init <dir>` | Create space, key, consent; write nothing to `<dir>` until first push. |
| `oboete sync join <dir>` | Join; key typed on a TTY with echo off; consent. |
| `oboete sync key show` | Print the key line (TTY only). |
| `oboete sync push` / `pull` | As above. `--json` prints counts, withheld, conflicts, skipped bundles. |
| `oboete sync status` | Local-only: space, replicas seen, cursors, open conflicts, withheld counts. |
| `oboete sync resolve <origin_id> --keep <revision_id \| checkpoint memory origin>` | Create a successor revision with every current head as parent whose content is the kept head; for a work, `--keep` may name a stored checkpoint memory of that work instead (an observer conflict candidate that no revision points at), and the successor is the work's current payload with `current_checkpoint_memory_id` set to it; closes the conflict row. |
| `oboete sync leave` | Remove key, cursors, consent and this replica's own bundle file. |

MCP exposes `sync status` read-only. No MCP or agent path can push, pull or resolve.
`oboete doctor` reports sync as configured/unconfigured and the consent state from local data only.

## Bounds

| Bound | Value |
| --- | --- |
| Plaintext per bundle (header + all lines) | 256 MiB (268,435,456 bytes) |
| Ciphertext per bundle | 268,501,036 bytes |
| Chunk | 65,536 bytes + 16-byte tag |
| Header line | 65,536 bytes |
| Replicas per space directory (pull refuses above this) | 32 |
| Revision lines per bundle | 1,000,000 (the native line cap) |
| Parents per revision | 64 (equal to heads per origin, so `resolve` can always cite every head) |
| Heads per origin, and per local row after aliasing (checked inside the apply transaction) | 64 |
| Revisions per origin | 4,096 |
| Staging | one plaintext file per bundle, removed after apply or failure |
| Process RSS | import/export CLI budget from `contracts/migration.md` |

## Schema (0008, additive; designed in T034 from this contract)

`sync_spaces` (space, directory, key id, consent hash, last pushed snapshot, published file
hash/size), `sync_cursors` (space, replica, snapshot), `sync_origins` (origin_id ↔ kind, local id;
several origins may map to one local id, and the row's selected head, `materialized_hash` and
`materialized_revision` live on its canonical origin), `sync_revisions` (revision_id,
origin_id, kind, author, parents JSON, control JSON, payload_hash, payload JSON or null,
received-from replica), `sync_repo_mappings` (repo key ↔ local repo id), and the local approval
record's candidate hash/projection/scope columns. No trigger and no column is added to the
tracked tables. Heads are derived (`revisions with no stored child`),
not stored. `sync_conflicts` (0003) is reused as the conflict report. 0001–0007 tables are
otherwise untouched.

## Review status (T033)

Nine fresh Codex read-only contract reviews on 2026-09-11 (`/var/tmp/oboete-009-20260909.jJ5grc/
scratch/syncrev1`…`syncrev9`, prompts and verdicts) shaped this text: rounds 1–7 each found
1–3 high findings that were folded in above (payload/identity separation, control revisions,
natural-key aliasing, push staging fence and `data_version` re-check, two-phase apply, lineage
inheritance, materialized-state change capture); round 8 added the proposal class gate, the
checkpoint deletion unit and tombstones for physically deleted rows. Round 9 ran twice: Codex
(high 2, medium 3: the `data_version` baseline now spans the change pass, the "unchanged" exit
moved behind the step 4 check, `materialized_hash` excludes apply-side raises, resolutions
bypass the checkpoint chain, observer conflict rows are listed and closed) and a five-lens
adversarially verified review (`syncrev9/workflow-synthesis.json`: 42 findings, 5 survived: the
checkpoint sweep of already stored rows, per-row heads for aliased origins, identity-only
contexts, the per-origin head bound in the test list, the 32-replica error naming files). All
are folded in above. Round 10 (Codex, `syncrev10`: high 1, medium 4) found the resolution rule
matching ordinary successors and a local edit building on an unseen head; v10 restricts
resolutions to multi-parent revisions, parents local edits on `materialized_revision`, binds
cross-origin parent links only once the reader aliases the origins, bounds heads per local
row, lets `resolve` name a checkpoint memory, and fixes the filter's evaluation order. T034
closes every finding with a test in the list below.

## Verification (T034–T036)

`test/unit/sync.test.ts` with three isolated homes and one shared temporary directory:

- canonical `revision_id`/`snapshot_id` (parent order, member order, control, null payload);
- push idempotence (unchanged snapshot with an intact file writes nothing; a lost or rolled-back
  published file is re-published; rewritten bundle has a new salt, same ID);
- pull of duplicate, descendant, tombstone-over-live, live-after-tombstone (conflict, no
  resurrection), sibling (both heads on both sides), resolve creating a successor; a local edit
  after a sibling arrived keeps the sibling;
- relay: A and B produce siblings, C pulls only B's bundle after B pulled A: C sees both heads and
  the same conflict; A→B→C equals A→C for every scenario here; a device joining after A pushed
  r0 then r1 (only r1's payload in the file) stores r0 as identity-only and r1 as head;
- partial descent: heads {a, b}, a1 with parent a arrives: heads become {a1, b}, the conflict
  stays open, the selected head moves only if a was selected;
- foreign edit: B edits, deletes and marks secret a memory A created; B's successors reach A and
  C; A's stored copy of the text is erased after the secret floor and never re-published by A, B
  or C;
- push race: a commit by a second connection during staging, and one between the end of staging
  and `BEGIN IMMEDIATE`, each make `data_version` differ and the push restart; the bundle never
  contains the row the concurrent commit made secret; a commit attempted during step 4 waits;
- delivery identity: widening the consent classes, `--republish` after `map-repo`, and a payload
  newly held all change `snapshot_id`; a pull that brings a payload for a known null-payload
  revision fills it in unless the origin has a secret floor or tombstone;
- checkpoint resolve: work forks C1/C2 from C0, C1 selected, `resolve --keep` C2: selected head,
  `current_checkpoint_memory_id` and the conflict row change together and no new conflict opens;
  a bundle listing C2 before C1 still advances C0 to C2 without a conflict; a resolution R that
  keeps C2 reaches a device on the C1 branch (and a third device that receives only R's
  single-parent successor with payload): head, `current_checkpoint_memory_id` and the conflict
  row change together; a resolution that misses a head the receiver holds stays a sibling;
- natural-key alias: two devices capture identical material under different origins; pull maps
  the second origin to the existing row (no UNIQUE failure); a tombstone under either origin
  deletes the row on both devices; A deletes or marks secret before B ever saw A's origin, and
  B's copy still aliases through `natural` and follows; a repository identity of 16,384
  characters round-trips through its hashed key; divergent edits across aliases (A pins and B
  retires the same aliased row): each device shows one conflict naming all heads with their
  origins, and a single `resolve` on either origin leaves both devices with the same payload,
  no open conflict and no stale `materialized_hash`;
- change-free round trip: A pushes, B pulls and pushes, A pulls: no new revision anywhere; a
  deletion committed while a push was staging reaches the other device on the retried push; a
  secret marking or deletion committed between the change pass and step 2, and one committed
  during staging of a push that would otherwise be "unchanged", each restart the push and ship
  their control revision; the late-child raise (below) reaches a third device as a control
  revision, not only the local row;
- checkpoint order: a bundle whose lines list C2, C1, C0 in that order fast-forwards a work at C0
  to C2; a joining device rebuilds the chain from heads alone;
- work purpose: a `private` purpose with `private` unselected withholds the whole work, checkpoint
  or not, and its control revision still ships;
- change pass: A completes a work and pulls before pushing; B's descendant of the older head
  arrives as a sibling of A's completion, not as its replacement; a pull whose apply raised a
  descendant through the 0005 trigger records that raise before commit and the next push ships
  it (three devices: B holds D under P, C captured the same D independently, A marks P secret);
- late child: after B applied P's secret floor, C's new D with `source_memory_id = P` arrives:
  D is stored secret without text on B;
- proposal class: a `private` pending candidate on an `eligible` memory and work is withheld
  when `private` is unselected; its control revision ships;
- checkpoint deletion: C0 (material X) of a work is deleted on A; C2 with the same material and
  a different parent arrives from B: it is applied deleted; reverse order: B already stores C2
  when A's tombstone for C0 arrives: C2 is deleted on B and B's next push ships its tombstone; a
  third device that pulls B before A ends with C2 deleted too;
- physical deletion: a source row deleted by the worker and a grant revoked by adoption ship as
  tombstones on the next push; a pulled approval held `pending` on a device without the approval
  record does not become a new revision on that device's next push;
- alias after mapping: A and B share a `common_dir` repository at different paths; after
  `map-repo`, A's tombstone or secret floor for material B captured independently aliases through
  `natural` (material hash and mapped repository) and applies; a personal projection aliases
  across repositories;
- staging fence: the space directory receives no file before the step 4 check passes, even with a
  concurrent secret-marking commit during encryption;
- 64 sibling heads are accepted and one `resolve` closes them; an origin with 65 heads is
  rejected; two aliased origins with 33 heads each are rejected as one row of 66;
- identity-only head: A creates the first `private` checkpoint C1 of a work while B has `private`
  unselected; B receives the work revision W1 identity-only, then completes the work: B's
  revision is a sibling of W1 (parent: the last materialized revision), both devices report the
  conflict, and C1 is still selected on A; the relay case with an identity-only resolution R
  followed by a payload-carrying successor S still converges;
- observer candidate: an observer checkpoint conflict (C1 current, C2 stored as a candidate)
  that predates sync is resolved with `resolve <work origin> --keep <C2 origin>`: the work's
  pointer moves to C2 and both conflict rows close;
- mapped alias: A and B hold the same repository under different `common_dir` keys; after
  `map-repo` on B, a `resolve` whose parents span both origins' heads validates and applies on
  B, and on a third device only after it maps the repository too (until then each origin keeps
  its own heads);
- work-only repository: a repository with an `eligible` work, its context and no memories ships
  both work and context payloads; a bundle from a store of 2,000 single-head origins is accepted; open observer
  conflict rows that predate the first sync are listed by `sync status` and closed by the
  `resolve` of their work row;
- context payloads: a repository whose memories are all withheld ships its contexts identity-only
  and no `repo` line; the first shipped memory of that repository brings the context payload and
  the `repo` line;
- pull crash: a process killed after rows were updated and before the cursor was recorded leaves
  the database at the previous state (single transaction) and the next pull re-applies the bundle;
- lock: two pushes for one space, one gets `busy`; a push killed mid-way leaves no lock behind;
- sources: a re-inserted identical source keeps its origin; a removed source ships a tombstone; a
  memory with a source whose raw event is `classification_state = 'done'` and
  `processing_state = 'waiting'` is withheld with its sources and the works that reference it; a
  summary whose raw event was purged ships with its evidence-less source rows;
- repository keys: two devices with the same `common_dir` path do not merge; a mapped key applies
  the withheld payloads on the next pull;
- graph bounds: a bundle over any bound in "Bounds" is rejected before apply; validation time and
  RSS are measured on a 1,000,000-line chain, a wide fan-out and a repeated merge DAG;
- secret after sync: A publishes an `eligible` memory, B pulls it, A marks it secret (which also
  sets `review_state = 'imported'`), A pushes, B pulls: B's copy loses its text and becomes secret;
  same for `eligible` → `private` with `private` unselected on A: B raises without new text;
- approval binding: A approves candidate C0, B pulls the approval, A's later revision of the same
  proposal carries candidate C1 with a matching projection: B stores it `pending`, no projection;
  B re-publishes A's revision verbatim; a local approval already present on B stays bound to C0;
- withheld closure: unprocessed sources, live quarantined memories, unselected classes, and the
  works that reference them are absent as payloads, present as control revisions where they were
  ever published, and counted; no dangling reference in any bundle;
- interrupted push (temporary file left behind is ignored), truncated and tampered bundles (every
  chunk position, tag, counter, final marker, a full-size final chunk), wrong key id, oversize
  file, foreign file names, a shipped payload whose hash differs from `payload_hash`, a line
  whose parent names a revision of another kind;
- bounds: a push at 256 MiB total plaintext round-trips; one byte more is rejected before writing;
  a bundle one byte over the ciphertext bound is rejected by `stat`;
- consent drift performs no I/O; no destination performs no I/O; `doctor` and MCP never open the
  space directory;
- schema: 0008 is additive and does not touch 0001–0007 tables except `sync_conflicts` rows.

Real multi-device use over a file-sync tool is a separate activation and is recorded as such.
