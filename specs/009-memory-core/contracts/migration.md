# Migration contract

This contract implements T029-T032. It extends [memory-core.md](memory-core.md),
[work.md](work.md) and [sharing.md](sharing.md). Import is an operator operation on an untrusted
file; it cannot supply accepted raw events, local work selection, or local personal approval.
The current source schema is migration 0006. New persistent import state starts at migration 0007.

## Supported inputs

| Input | Boundary |
| --- | --- |
| `oboete-export/1` | Frozen existing JSONL wire format; the new reader remains compatible. |
| `oboete-export/2` | Tagged JSONL retaining memory, source, visibility and proposal provenance. |
| `claude-mem-query-export@8bc631a` | Explicit adapter for the documented local query export below. |

The claude-mem adapter is pinned to v13.24.5, commit
`8bc631a71a487424b866756e43a6efa4574cc66b`. Its header contains `exportedAt`,
`exportedAtEpoch`, `query`, optional `project`, the four `total*` counts, and arrays
`observations`, `sessions`, `summaries`, `prompts`. Totals must equal array lengths. Times and
counts are safe nonnegative integers; corresponding ISO strings must describe the same time.
There is no upstream schema-version field. Unknown extensions carry no local authority.

The public exporter and actual SQL define this shape, rather than the private CMEM service:
<a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/scripts/export-memories.ts">exporter</a>,
<a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/src/services/sqlite/SessionStore.ts">storage queries and importer</a>,
<a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/src/services/sqlite/types.ts">SQLite types</a>,
and <a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/docs/public/usage/export-import.mdx">public export documentation</a>.

The six observation types `bugfix`, `feature`, `refactor`, `change`, `discovery`, `decision`
map directly. Custom types are counted as unsupported, never coerced. `facts`, `concepts` and
file lists are SQLite JSON text or null; typed lists are validated without coercion.
Metadata extensions are retained as JSON values without a per-field schema.
Sessions retain their platform/content-session identity. Observations and summaries retain their
memory-session identity. Prompts are joined only through an unambiguous exported relationship.

This query export has no deletion history or stable installation/project registry. Record absence
never means deletion. The CMEM canonical cloud replication envelope is a different protocol and is
not accepted by this adapter. Prompt-only exports may lack a session. Preview states these limits.

## Input and resource boundary

Read a path, FIFO or stdin exactly once, hash its original bytes incrementally, and decode strict
UTF-8. Reject malformed input before opening the destination for mutation. Source files are never
rewritten, renamed, chmodded or opened with SQLite. Their bytes remain unchanged.

Native JSONL is limited to 256 MiB. V1 retains its 64 KiB physical-line limit. V2 allows one complete
tagged record per line, up to 4 MiB, so retained evidence needs no new chunk protocol. A memory's
privacy provenance still obeys the existing 50-context/2 MiB bound. Oversize records fail with a
fixed reason; no reader or writer clips them. Required in-file references are checked after EOF,
independently of input order. Duplicate `(kind,id)` origins reject the entire import, even when
their payloads match.

Native parsing stores validated records in a private temporary SQLite plan and processes bounded
rows. It does not retain the file, all lines, all parsed records or all preview details in memory.
The scratch directory is owner-only and removed in `finally`; scratch SQLite uses bounded cache
and file-backed temporary storage. It carries no provider credentials. Export iterates rows and
writes a private temporary file before replacing a named target; a failed export preserves the
previous target. Standard output receives only a fully validated export.
Enumeration uses one read transaction. After staging, a fresh same-connection SQLite data-version
check must still match; otherwise export refuses before publishing any bytes. This conservatively
rejects concurrent benign commits as well as changed privacy/deletion state.

Limit native input to 1,000,000 physical lines and 1,000,000 staged records.
Before `JSON.parse`, reject each value with more than 100,000 unquoted `[`, `{`, `,` and `:` delimiters.
Typed arrays have explicit count bounds. The private
SQLite plan has a 512 MiB page ceiling and a bounded cache; storage exhaustion rejects cleanly and
cleans the scratch directory. Stop parsing at the first fatal error. An incomplete input has no
successful receipt, so it need not be drained only to finish its hash. Resource tests include many
tiny records, high-cardinality JSON and database amplification, not only long prose.

The first claude-mem adapter accepts at most 5 MiB before its bounded `JSON.parse`. This is an
Oboete operational limit, not an upstream validity rule. The upstream exporter can produce larger
files. Official query/project shards are supported and overlapping records deduplicate; a selected
shard cannot prove whole-store coverage. Oversize input reports unsupported size with no partial
apply. Native near-limit and external 5 MiB peak RSS must be measured against 150 MiB before claiming
the advertised limits; lower a limit if the implementation cannot demonstrate it.

## Preview and mappings

Select the external adapter explicitly with `--from claude-mem`; the default reader accepts native
Oboete JSONL only. Native v2 and claude-mem imports default to preview and require `--apply` to write. `--dry-run`
is explicit preview. Frozen v1 invocation retains its established implicit apply. Conflicting
`--apply --dry-run`, unknown options, duplicate mapping keys and invalid IDs are rejected.
The format header therefore selects legacy v1 implicit apply when no mode flag is given. This is
an intentional compatibility boundary: use `--dry-run` for an unknown file. Human output names
legacy implicit apply explicitly; format must never be advertised as irrelevant to default mode.

External projects require `--map-project <exact-source-name>=<existing-repo-id>` for every exact
nonempty project string. Split at the last `=`; do not guess by basename, cwd, branch, session,
`merged_into_project` or most recent repository. Native inputs use `--map-repo`; a verified native
remote identity retains the v1 mapping rule. Auto-creation also requires the source identity to
round-trip through the existing remote URL normalizer without userinfo, query, fragment or controls;
a self-consistent hash alone cannot admit raw URL metadata. Explicit mapping ignores source display
metadata and preserves the existing destination. Project collisions are always visible in preview.
An unsafe project name is addressed with
`--map-project-hash <sha256-of-exact-UTF8-source-name>=<existing-repo-id>`. Hash and exact-name
arguments may not both select the same source project. Unknown or ambiguous hashes are rejected.
Privacy context uses `--map-context <local-repo-id>=<local-context-id>`, or exactly one currently
verified stored context for that repository. Zero or multiple candidates remain unresolved; neither
the newest/first context nor an imported root may select policy. Preview lists bounded candidate IDs.
The chosen context's identity/generation is recorded and freshly verified before classification
release. A replaced/moved-to-unrelated root cannot inherit this choice.

Preview opens an existing destination through a dedicated read-only, no-migration connection and
a read transaction. It never creates the destination directory/database, changes tables, runs
migrations, takes a writer lease or writes receipts. A missing/older/newer schema is reported and
blocks apply until the normal local schema setup can run. SQLite WAL readers may create or update
WAL/SHM coordination files; byte equality for those destination runtime files is not promised.
The source-file byte invariant is separate and unconditional.

Preview includes exact format/revision/raw SHA-256, byte size, source counts, proposed effects,
quarantine/exclusion/held counts, bounded unresolved identity hashes, mapping readiness, destination
schema readiness, source-tombstone availability and whether apply is possible. It contains no
memory/prompt/evidence text, source paths, model output or arbitrary parser messages. The current
CLI displays source project hashes usable for mapping; it keeps every exact source name private.
Bounded detail lists include an omission count. Validation emits fixed reason/field codes and row
indices only. `--json` has the same content boundary as human output.

Before apply, mapping targets and current destination effects are recomputed inside one immediate
transaction. A preview is advisory and cannot authorize an outdated identity or revive a deletion.
Parse/references/errors roll the whole apply back. No asynchronous detector or network runs while
holding that write transaction.

## Stable receipts and merge behavior

`migration_imports` records format/revision/raw file hash, a hash of sorted exact mappings, counts
and import time. Its ID hashes `['migration-import-v1', format, revision, rawHash]`. The same file
and same mappings have no duplicate effects; a changed mapping for that file is refused.

`migration_records` records source kind, original identity, exact payload hash, destination mapping,
effect and classification state. Origin identity hashes format/revision, kind, source project,
natural key and payload hash. External natural keys are observation
`[memory_session_id,title,created_at_epoch]`, summary `[memory_session_id]`, session
`[platform_source,content_session_id]`, and prompt `[content_session_id,prompt_number]` with its
resolved platform/project context. Changed payloads remain separate historical sources.
Identical natural-key/payload records from different upstream installations cannot be distinguished.
Native origin tuples retain the SHA-256 of the exact source record ID, rather than its raw string.
The full allowed original ID remains in the private payload; terminal hash-only records therefore
cannot leak a credential supplied as an opaque source ID. One origin has one destination mapping
across all files: a conflicting target rejects the whole import. Imported terminal `secret` state
is preserved even when the local destination has never seen that origin.

Destination ordinary identity uses the existing `materialHash`, `contentHash(mappedRepo,material)`
and `memoryIdFor`. Personal projections use the exact D1 identity
`sha256Json(['personal-projection-v1', exactTitle, exactBody])`, never normalized repo identity.
Origin metadata never chooses an arbitrary local memory/raw-event/session/work ID.

| Destination state | Effect |
| --- | --- |
| Existing tombstone | Keep the tombstone; attach no text, sources, visibility or approval. |
| Identical active row | Preserve local text, scope and validity; only raise sensitivity and retain origin receipt. |
| New active | Insert unpinned, at least `local_only`, `review_state='imported'`, with bounded quarantined provenance. |
| Native tombstone | Resolve the correct local identity, retain hashes, apply deletion and existing evidence clearing. |
| Overlapping export | Deduplicate origin/target receipts and content without repeating effects. |

Never attach unclassified evidence to a visible row.

Private migration holds retain exact bounded provenance that cannot safely be attached to an
existing visible memory, and exact foreign proposal payloads. A hash alone is not preservation.
Holds have no FTS entry and are excluded from retrieval, packs, MCP, viewer, ordinary history,
provider input and preview/log text. The migration operation owns them. Secret or deleted source
content travels as hashes only; local secret classification clears held payloads too.

## Native v2 wire records

The exact header format is `oboete-export/2`, with an export time and capability revision. Repository
records carry source repo ID, identity kind and normalized identity. Separating repository records
keeps the header bounded for many projects.
The header also contains the source store's persistent random 128-bit `origin_id` from
`replica_identity`. Include it in native origin identity. It is public provenance, not authentication
or authorization. V1 and the pinned external format lack an installation ID and disclose that limit.

Tagged records preserve these fields; the implementation's strict validators define scalar bounds:

- `memory`: source repo/memory ID, all v1 semantic fields, source session/batch/work/checkpoint and
  supersession origins, `provenance_complete`, source-capture time, and identity domain
  (`ordinary` or `personal_projection`). Source lineage is metadata, not a local foreign key.
- `source`: its source record ID, parent memory origin, raw-event/dependency/context origins,
  citation kind/value/agent, exact evidence, range/total/hash, capture/processing times, origin root
  and source paths, and `context_only`. Native original source IDs never populate local raw IDs.
- `visibility`: source grant ID, memory origin, audience, repo/work/proposal origins, creation time,
  and source grant kind restricted to `migration`, `observer`, `explicit_adoption`,
  `proposal_approval`. Source grant kind is provenance; imported grants use `migration`.
- `sharing_proposal`: proposal and origin memory/repo/work IDs, exact candidate title/body/material
  hash/sensitivity, unique source-event origins, basis, source state, source decision channel,
  projected-memory origin, creation and decision times. The reader checks decision-channel,
  projection and decision-time nullability against proposal state. Approved source projections
  must be source-free. Exact candidate equality is required only for unredacted proposals.
- `migration_origin`: retained source receipt/hold provenance for re-export, with no local approval
  or scope authority. A retained origin is copied once; repeated transfer never nests envelopes.
- `work` and `context`: referenced source work/context metadata, retained in private migration
  provenance. Purpose, state, root, parent context and current-checkpoint IDs describe the source
  only. They do not create local work/context rows or select a task. Session/binding/raw-event
  origin IDs are retained as provenance; this file is not a full accepted-event database backup.

The exporter redacts secret/tombstone text, concepts, evidence, paths and candidate payloads. A
source-free personal projection cannot gain project sources through export/import. Referenced
deleted origins retain hash identity; references do not require old raw events to survive.
Dependencies between memories must resolve within the file; raw/session/work origins are historical
identifiers and never assert that corresponding local state exists.
Validate required references in the source namespace before mappings can collapse multiple source projects.
Check repository ownership for memory/source dependencies, work/context relationships, grants and proposal origins.
The reader does not require an approved projection to share its origin memory's repository.
Source-memory, supersession and checkpoint-parent relations reject
self/cross cycles with bounded iterative graph validation. Source-memory and checkpoint-parent
edges share one dependency graph; supersession is checked separately because its direction is
opposite to the checkpoint-parent relation. A personal projection is distinct from
its origin and has no source children or work/session/batch lineage.

Separate source/proposal/origin records inherit known secret/deletion state from their required
parent memory, origin and projected memory. They then retain only hashes and relationship/state
metadata. Pending/private/quarantined payload is preserved by this explicit local native backup
boundary; it is not automatic model/provider egress and must not silently erase held provenance.
Once a stable migration origin is secret/deleted/rejected and cleared, repeats, overlapping files
and re-exports never rehydrate its payload or downgrade the terminal classification.

V1 export remains available through `--format 1` with its frozen field shape. New v1 imports stop
copying foreign session/batch IDs into live lineage and retain them as origin metadata. V1 lacks
work/personal scope, so its ordinary records retain the legacy mapped project meaning and quarantine.

## Historical scope and explicit recovery

Import never creates, activates, selects, completes or reopens work; never creates native sessions,
raw events, batches or injection records; and never updates current checkpoint or session-summary
pointers. External summaries are historical, unbound `session_summary` rows. External prompts and
sessions are supporting provenance, not accepted events or standalone active progress.

Native project grants become mapped project grants behind quarantine. Foreign work records are
retained without grants by default. An explicit `--map-work <source-work>=<existing-local-work>`
may attach a historical work grant only when the source repo mapping agrees with the local work.
It still changes no work state, binding or current pointer. Checkpoints remain historical; their
lineage must never become the mapped work's live checkpoint chain.

| Source | Imported visibility after clean release |
| --- | --- |
| External observation / ordinary v1 | Mapped project grant, stored behind quarantine with `grant_kind='migration'`. |
| External/v1 summary | Mapped project grant for explicit history; ordinary readers still exclude unbound summaries. |
| Native project knowledge | Its mapped project grant. |
| Native work knowledge/checkpoint | Explicitly mapped historical work grant only. |
| Native personal projection | No grant until fresh local approval. |

Move an import-owned grant to a new sanitized identity only when the merge permits it. A convergent
existing active row preserves its existing visibility; classification does not widen it.

Personal projections are imported ungranted and source-free. Source proposal decisions are retained
as private history. After local classification, explicit migration promotion of a held candidate
requires its origin work to be mapped; it creates a new local **pending, inferred** proposal with
no authoritative imported raw-event IDs. Existing `share approve` supplies the fresh human decision
and privacy check. Imported `approved`, `automatic_direct`, declaration text or grant kind cannot
create local approval. Repeated promotion is idempotent and never reverses a local rejection.

The operation is `oboete import promote <migration-record-id> --work <local-work-id>
[--json]`. Cwd supplies a verified local repository/context; the record and existing mapped work
must belong to it. Only a locally classified origin and candidate qualify. The operation grants
that explicit historical work association, computes the sanitized candidate hash and a local
proposal identity from `['migration-proposal-v1', localMemory, localWork, exactTitle, exactBody]`,
and creates/reuses a pending inferred proposal with empty source-event IDs and null decision,
projection and decision time. An existing exact local rejection remains terminal. Missing,
unclassified and wrong-scope IDs share a fixed unavailable result. Output is only the local
proposal ID/state; `share status` provides the reviewable candidate before human approval.
`oboete import promote --list [--json]` lists at most 100 sharing-proposal migration records in ID
order for the same verified cwd repository, with an omitted count and only record ID, destination
memory ID or null, classification state, effect, promotability without a work argument, and promoted
proposal ID or null; candidate text, payload fields, source IDs, project names and paths are never
output.

## External content projection

Observation title uses title, subtitle or a fixed imported-type label within 120 characters. Body
uses labelled Narrative, Text and Facts sections within 2,000 characters and an explicit omission
marker. Full bounded source fields, concepts, files and uniquely related prompts/sessions remain in
quarantined provenance. Unsupported records and unresolved support relationships retain bounded
origin receipts and exact allowed payload holds, with fixed exclusion reasons.

Each supported memory also retains its related session and unambiguous prompt payloads in private
source receipts, so those fields participate in that memory's classification and fresh snapshot
check. The existing 50-source/2-MiB per-memory provenance limit bounds this duplication. Standalone
session/prompt receipts and `excluded` receipts preserve input history without creating live events
or knowledge. Native `migration_origin` records retain these private payload kinds on re-export.

Summary projection labels Request, Investigated, Learned, Completed, Next steps and Notes. Full
evidence is retained, but old next steps never become current work. No source path is used as a
trusted local root. The exact mapped repository supplies a currently verified local context; source
paths remain subject to the destination's current path rules.

## Local quarantine release

All new active imports start quarantined. For newly imported records, the worker establishes a
verified local repo context and a bounded flat privacy proof. Unresolved context remains pending.
Imported source/root/context IDs never manufacture that proof. Every text-bearing field, evidence,
concept, citation/path and held candidate passes the current local detector and directive check.
No provider is used for import classification.

One classification unit is at most 500 rows and 2 MiB of compared memory/source/hold state, with at
most 4,096 distinct detector fields and 50,000 field references. Typed JSON text lists are decoded,
sanitized element by element and re-encoded. Structural IDs, hashes and enums are never rewritten;
if they cannot be retained, the associated private payload is cleared. Flat proof paths contain
only detector-returned text. An imported absolute path may use its source root solely as a lexical
anchor for checking the corresponding relative path at the verified destination. An absolute path
without a usable source anchor or verified local containment stays quarantined.

A pass examines at most 100 units and respects the enclosing worker deadline. A fenced private
cursor resumes after failed/unresolved rows on the next pass. Only one sanitized detector result is
retained while checking additional contexts, so the 50-context limit does not multiply text memory.

Clean release stores only detector-returned sanitized text. A detector failure, incomplete/oversize
proof, changed credentials/rules/context or changed row/evidence leaves quarantine intact. A secret
or directive tombstones and clears all text/evidence/held payloads while retaining hashes. Within
the fenced write transaction, re-read the entire classified snapshot and fresh policy stamp before
releasing it; the earlier asynchronous result is insufficient. If sanitation changes material,
recompute the memory's material/content/ID using its identity domain and re-run tombstone/existing
content rules for that resulting identity. Move source/grant/receipt references atomically only
when allowed; a matching tombstone wins and an existing active row receives no unclassified source.
The old identity becomes a text-free tombstone; its sources are removed when sanitation converges
on an existing row. Never release text under the pre-sanitized identity or leave a second live duplicate. Existing old
imported rows without new receipts retain their conservative privacy checks, but do not bypass
this deletion and identity rule.

Classification owns only newly inserted quarantined rows and their migration payloads. When import
matched a pre-existing active local memory, a clean result only sanitizes the private origin hold;
a secret/directive rejects and clears that hold. Neither path edits the existing memory's text,
hash, review state, sources, visibility, validity or deletion. The initial native merge's explicit
tombstone/stricter-sensitivity rules remain separate from this hold-classification boundary.

## Verification and order

Implement native format/streaming preview first, then receipts/merge, external adapter, quarantine
and explicit proposal recovery. Reuse the current SQLite opener, identity functions, source privacy
guard, sharing approval path, Zod and `node:test`; add no runtime dependency.

Focused tests must prove frozen v1 compatibility; exact v2 pending/approved/rejected candidate and
source provenance; separate personal identity; malformed/dangling/oversize/invalid UTF-8 rejection;
source-byte preservation; preview on missing/current/old/WAL targets; mapping collisions; repeated
and overlapping imports; tombstone precedence; unchanged current work/lineage; detector-only evidence
secrets; stale asynchronous classification; no approval from imported text; explicit promotion and
human approval; sanitation converging on existing live/deleted content; and near-limit packed CLI
RSS with bounded output. Run typecheck/lint/build and
relevant migration/worker/transfer suites on Node 22.16 and 24.16, then the cohesive gate and ordinary
security, Standards/Spec and Ponytail reviews. External accounts/providers and daily installs remain
outside this local implementation.

## Appendix: pinned claude-mem query export

This appendix describes [transfer-claude-mem.ts](../../../src/transfer-claude-mem.ts) as read on 2026-09-10.
Its SHA-256 is `b1b62c688b81cbc3ef006162e750fcc9bb9da48005e6c88d082f7ca4c0d46972`.
Select this single JSON-object reader with `--from claude-mem`.
The internal format is `claude-mem-query-export` at revision `8bc631a71a487424b866756e43a6efa4574cc66b`.
The input has no required format or version marker.

Here `uint` means a nonnegative JavaScript-safe integer.
`id` means a nonempty string of at most 16,384 UTF-16 code units.
`text` means a string of at most 2,097,152 UTF-8 bytes, including an empty string.
Every listed field is required unless marked optional.
Nullable fields still require their keys.
Unknown object members are retained as JSON values within the whole-input bound.
They do not receive the known fields' per-field validation.

### External header and array bounds

| Field | Accepted value |
| --- | --- |
| `exportedAt` | `id`; timestamp paired with `exportedAtEpoch`. |
| `exportedAtEpoch` | `uint`; epoch milliseconds. |
| `query` | `text`. |
| `project` | Optional `id`; every collected project must equal it when present. |
| `totalObservations` | `uint`; equals `observations.length`. |
| `totalSessions` | `uint`; equals `sessions.length`. |
| `totalSummaries` | `uint`; equals `summaries.length`. |
| `totalPrompts` | `uint`; equals `prompts.length`. |
| `observations`, `sessions`, `summaries`, `prompts` | Arrays of objects with the fields below. |

The caller rejects input larger than 5 MiB before parsing, and the adapter checks the byte count again.
The caller also applies strict UTF-8 decoding and the 100,000-delimiter bound described in the native appendix.
There is no physical-line bound for this single JSON object.
Each array and their combined total allow at most 20,000 input rows.
The adapter allows at most 50,000 normalized records before the caller adds repository records.
Each memory allows at most 50 source records and 2 MiB of their serialized UTF-8 JSON combined.
Exceeding these bounds rejects the input without accepting a partial plan.

### External row fields

| Observation field | Accepted value |
| --- | --- |
| `id`, `discovery_tokens`, `created_at_epoch` | `uint`. |
| `memory_session_id`, `project`, `created_at` | `id`. |
| `type` | Nonempty string of at most 128 UTF-16 code units. |
| `text`, `title`, `subtitle`, `narrative` | `text` or null. |
| `facts`, `concepts`, `files_read`, `files_modified` | `text` or null; typed lists. |
| `prompt_number` | `uint` or null. |

| Session field | Accepted value |
| --- | --- |
| `id`, `started_at_epoch` | `uint`. |
| `content_session_id`, `project`, `platform_source`, `started_at` | `id`. |
| `memory_session_id`, `completed_at` | `id` or null. |
| `completed_at_epoch` | `uint` or null. |
| `user_prompt` | `text` or null. |
| `status` | `active`, `completed` or `failed`. |
| `custom_title`, `model`, `billing` | Optional `text` or null. |

| Summary field | Accepted value |
| --- | --- |
| `id`, `discovery_tokens`, `created_at_epoch` | `uint`. |
| `memory_session_id`, `project`, `created_at` | `id`. |
| `request`, `investigated`, `learned`, `completed`, `next_steps`, `notes` | `text` or null. |
| `files_read`, `files_edited` | `text` or null; typed lists. |
| `prompt_number` | `uint` or null. |

| Prompt field | Accepted value |
| --- | --- |
| `id`, `prompt_number`, `created_at_epoch` | `uint`. |
| `content_session_id`, `created_at` | `id`. |
| `prompt_text` | `text`. |
| `session_db_id` | Optional `uint` or null. |
| `memory_session_id` | Optional `id` or null. |
| `project`, `platform_source` | Optional `id`. |

A typed list is null or JSON text encoding at most 10,000 strings.
Each list item is at most 2 MiB in UTF-8.
The adapter validates these lists even for an unsupported observation type.
Every populated timestamp pair must match this string pattern and satisfy `Date.parse(string) === epoch`:

```text
^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}(?:Z|[+-]\d{2}:\d{2})$
```

The parsed date must be finite.
Session completion string and epoch are either both null or both populated.
No chronological ordering or session-status/completion-time relationship is checked.

### External natural keys and support resolution

| Record | Natural key used by `recordId` |
| --- | --- |
| Observation | `[memory_session_id, title, created_at_epoch]`. |
| Summary | `[memory_session_id]`. |
| Session | `[platform_source, content_session_id]`, using the input platform string unchanged. |
| Prompt | `[content_session_id, prompt_number, contextPlatform, contextProject]`. |

`contextPlatform` and `contextProject` use the uniquely resolved session, then the prompt's own value, then null.
The record ID is `sha256Json(['claude-mem-query-record-v1', kind, naturalKey, payloadHash])`.
Here `payloadHash` is `sha256Hex(JSON.stringify(originalRow))`, including unknown members.
The adapter does not sort the original object's members before hashing.
Project IDs are `project:` followed by the SHA-256 of the exact source project string.

Numeric `id` values must be unique within each array.
Session `(platform_source, content_session_id)` pairs must be unique.
Non-null session `memory_session_id` values must also be unique.
Every observation and summary must resolve its memory-session ID to an exported session with the same project.
The adapter does not independently reject repeated observation, summary or prompt natural keys with different payloads.
Duplicate normalized `(kind,id)` pairs are rejected.

Prompt candidates start with sessions sharing `content_session_id`.
A populated `session_db_id` selects its exact exported session when present.
Supplied platform, project and non-null memory-session fields must agree with that selection.
They also filter the candidates when no numeric session selection is supplied.
A conflicting numeric selection or elimination of all existing content-session candidates rejects the input.
One candidate resolves the prompt; zero or multiple candidates retain unresolved or ambiguous support.
Only a resolved session's memory-session ID and matching prompt number attach the prompt to projected memories.
A prompt without an exported session is therefore accepted when it does not conflict with exported session data.

| Outcome or rejection code | Meaning |
| --- | --- |
| `unsupported_type` | Observation type is outside the six supported types; retain an `excluded` record. |
| `support_only` | Session receipt or resolved prompt receipt; no standalone memory is created. |
| `support_only_unresolved` | Unresolved or ambiguous prompt; retain support without a memory attachment. |
| `invalid_record`, `invalid_field` | Invalid object, missing required field, wrong type or field bound. |
| `count_mismatch`, `selected_project_mismatch` | Counts or the header's selected project disagree with rows. |
| `timestamp_mismatch`, `timestamp_pair_mismatch` | Timestamp syntax, equality or completion nullability fails. |
| `invalid_typed_list` | A list is malformed, has a non-string item or exceeds its bounds. |
| `duplicate_source_id`, `duplicate_session_identity` | Repeated numeric row ID or session platform/content pair. |
| `duplicate_memory_session`, `duplicate_normalized_origin` | Repeated session memory ID or normalized record pair. |
| `memory_session_project_mismatch` | Observation/summary session is missing or belongs to another project. |
| `prompt_session_conflict` | Exported prompt/session selectors contradict one another. |
| `source_file_too_large`, `too_many_source_records` | Input bytes or combined row count exceed the bound. |
| `too_many_normalized_records`, `memory_provenance_too_large` | Normalization or per-memory support is too large. |

The six supported observation types are `bugfix`, `feature`, `refactor`, `change`, `discovery` and `decision`.
Summaries become historical `session_summary` memories.
Generated memories have null work, checkpoint, session and batch lineage.
Their sensitivity is `local_only` and review state is `imported`.
The normalized header reports `query_scoped=true`, `origin_id=null` and `source_tombstones='unavailable'`.

## Appendix: exact native v2 wire format

Source snapshot read on 2026-09-10, pinned with `sha256sum`:

```text
0ca1e0553117c593028b7f7b3a82413437ba4d74adb8c87710f884ff3310098c  src/transfer-format.ts
41fbd9eedec5528f537916077248ef55cf1995d1fdfb5b2b3884aa48a5bb2303  src/transfer-plan.ts
```

The authorities are [transfer-format.ts](../../../src/transfer-format.ts) and
[transfer-plan.ts](../../../src/transfer-plan.ts).
This appendix describes their accepted input, including checks they leave to later stages.
Every table field is required.
`T or null` permits a null value, not a missing key.
No scalar coercion is applied.

### Framing, bounds and object handling

The reader decodes strict UTF-8 and hashes the original bytes, including ignored whitespace.
The UTF-8 decoder consumes an initial byte-order mark if present.
The first nonblank physical line is the header; subsequent nonblank lines are tagged records.
Blank lines still count toward the physical-line and byte limits.
LF terminates a line, and one trailing CR is removed before the complete-line check and parsing.
A final nonempty line without LF is accepted.
A final LF does not add another empty physical line.

The original native input is at most 256 MiB.
Each complete native line is at most 4 MiB after removing its trailing CR and excluding LF.
An unfinished pending line is also capped at 4 MiB, before any trailing CR is removed.
Consequently, a CR split across chunks can reach the pending-line bound before the complete-line check.
At most 1,000,000 physical lines and 1,000,000 staged records are allowed.
The header consumes a physical line without becoming a staged record.
A native file with no blank lines can therefore contain at most 999,999 records after its header.

Before parsing each outer JSON value, `boundedJson` counts unquoted `[`, `{`, `,` and `:` characters.
More than 100,000 such delimiters produces `source_structure_too_large`.
Escaped punctuation inside a quoted string does not count.
The same check applies when parsing `migration_origin.origin_json`.
Typed JSON-text fields have their separate parsers and limits below.

The header and every native record use `z.object`, including `memory`.
Unknown members are accepted but stripped from those parsed objects before staging.
An old inline `memory.sources` member therefore does not create source records.
`migration_origin.payload` retains unknown members because it is an arbitrary JSON object.
Payload shape validation does not replace that retained object with Zod's parsed output.

All rows are staged before source-reference and cycle validation.
Record order after the header does not affect those checks.
The staging table makes `(kind,id)` unique.
A repeated pair yields `duplicate_source_origin`, even for an identical payload.
Different kinds may reuse an ID.
The scratch plan sets SQLite `cache_size=-2048` and `max_page_count=131072`.
Temporary SQLite storage is file-backed.
Fatal parsing, schema, reference, cycle or storage errors discard the plan.

### Scalar notation and header

String lengths in the native tables are UTF-16 code units, unless a byte limit is stated.

| Name | Accepted value |
| --- | --- |
| `hex32` | String matching `^[0-9a-f]{32}$`. |
| `hex64` | String matching `^[0-9a-f]{64}$`. |
| `id512` | String of length 1 through 512. |
| `uint` | Nonnegative JavaScript-safe integer. |
| `int` | Signed JavaScript-safe integer, including negative values. |
| `text64k` | String of length 0 through 65,536. |
| `text2m` | String of length 0 through 2,097,152. |
| `string` | String with no further scalar length bound. |
| `sensitivity` | `eligible`, `local_only`, `private` or `secret`. |

| Header field | Accepted value |
| --- | --- |
| `format` | Exactly `oboete-export/2`. |
| `revision` | Exactly `memory-provenance-visibility-sharing/1`. |
| `exported_at` | `uint`. |
| `origin_id` | `hex32`. |

Any header schema failure, including an unrecognized format or revision, a non-`uint` `exported_at`
or an invalid `origin_id`, yields `unsupported_source_format`.
An unknown record kind or invalid native field shape yields `invalid_native_record`.
Timestamp fields use numeric `uint` checks; the native reader does not parse ISO strings.

### Repository fields

| Field | Accepted value |
| --- | --- |
| `kind` | `repo`. |
| `id` | `id512`. |
| `identity_kind` | `remote` or `common_dir`. |
| `normalized_identity` | String of length 1 through 16,384. |

### Memory fields and identity

| Field | Accepted value |
| --- | --- |
| `kind` | `memory`. |
| `id`, `repo_id` | `id512`. |
| `type` | One of the nine memory types listed below. |
| `title`, `body`, `concepts` | `text64k` or null. |
| `material_hash`, `content_hash` | `hex64`. |
| `sensitivity` | `sensitivity`. |
| `review_state` | `unreviewed`, `reviewed` or `imported`. |
| `degraded_reason` | `string` or null. |
| `source_session_id`, `source_batch_id`, `source_agent` | `string` or null. |
| `valid_from`, `valid_to`, `pinned_at` | `uint` or null. |
| `superseded_by` | `string` or null; populated values must resolve to a memory. |
| `pin_order` | `int` or null. |
| `deleted_at`, `created_at` | `uint` or null. |
| `identity_domain` | `ordinary` or `personal_projection`. |
| `work_id`, `checkpoint_parent_id` | `id512` or null. |
| `provenance_complete` | Numeric `0`, numeric `1` or null. |
| `source_captured_at` | `uint` or null. |

The nine types are `bugfix`, `feature`, `refactor`, `change`, `discovery`, `decision`,
`security_alert`, `security_note` and `session_summary`.
Non-null `concepts` must parse as a JSON array of at most 10,000 strings.
There is no additional per-item length bound beyond the enclosing field.

A memory is redacted when `deleted_at` is non-null or `sensitivity` is `secret`.
Its title and body must each be null or the empty string.
Its concepts must be null or exactly the string `[]`.
For every other memory, `material_hash` must equal `materialHash(title ?? '', body ?? '')`.
An ordinary memory must satisfy the following content-hash rule even when it is redacted:

| Condition | Required `content_hash` |
| --- | --- |
| `work_id` is null | `contentHash(repo_id, material_hash)`. |
| `work_id` is populated | `checkpointHash(repo_id, work_id, checkpoint_parent_id, material_hash)`. |

A personal projection requires null `work_id`, `checkpoint_parent_id`, `source_session_id` and `source_batch_id`.
When it is not redacted, its content hash is `sha256Json(['personal-projection-v1', title ?? '', body ?? ''])`.
No source child may refer to a personal projection.

### Source fields and redaction

| Field | Accepted value |
| --- | --- |
| `kind` | `source`. |
| `id`, `memory_id` | `id512`. |
| `raw_event_id`, `source_memory_id`, `source_context_id` | `id512` or null. |
| `citation_kind` | `file_read`, `file_modified`, `commit` or null. |
| `citation_value`, `source_agent` | `text2m` or null. |
| `portion_start`, `portion_end`, `source_total` | `uint` or null. |
| `source_hash` | `hex64` or null. |
| `evidence`, `capture_root`, `source_paths_json` | `text2m` or null. |
| `captured_at`, `source_processed_at` | `uint` or null. |
| `context_only` | Numeric `0` or numeric `1`. |

The three portion values are either all null or all populated with `portion_start <= portion_end <= source_total`.
Non-null `source_paths_json` must parse as an array of at most 10,000 strings.
There is no additional per-path length bound beyond the enclosing field.
A personal-projection parent rejects the source row regardless of its fields, as stated above.
If the parent selected by `memory_id` is secret or deleted, five fields must be null:
`evidence`, `citation_value`, `capture_root`, `source_paths_json` and `source_agent`.
The remaining IDs, hashes, range values, times, citation kind and context flag may remain.

### Visibility fields and audience shape

| Field | Accepted value |
| --- | --- |
| `kind` | `visibility`. |
| `id`, `memory_id` | `id512`. |
| `audience` | `work`, `project` or `personal`. |
| `repo_id`, `work_id`, `proposal_id` | `id512` or null. |
| `grant_kind` | `migration`, `observer`, `explicit_adoption` or `proposal_approval`. |
| `created_at` | `uint`. |

| Audience | `repo_id` | `work_id` | `proposal_id` |
| --- | --- | --- | --- |
| `work` | Populated | Populated | Null |
| `project` | Populated | Null | Null |
| `personal` | Null | Null | Populated |

### Sharing-proposal fields and state shape

| Field | Accepted value |
| --- | --- |
| `kind` | `sharing_proposal`. |
| `id`, `origin_memory_id`, `origin_repo_id`, `origin_work_id` | `id512`. |
| `candidate_title` | String of length 0 through 120. |
| `candidate_body` | String of length 0 through 2,000. |
| `candidate_material_hash` | `hex64`. |
| `candidate_sensitivity` | `sensitivity`. |
| `source_event_ids_json` | String of length 0 through 32,768; JSON list constrained below. |
| `basis` | `direct_declaration` or `inferred`. |
| `state` | `pending`, `approved` or `rejected`. |
| `decision_channel` | `automatic_direct`, `cli`, `viewer` or null. |
| `projected_memory_id` | `id512` or null. |
| `created_at` | `uint`. |
| `decided_at` | `uint` or null. |
| `redacted` | Boolean. |

| State | `decision_channel` | `projected_memory_id` | `decided_at` |
| --- | --- | --- | --- |
| `pending` | Null | Null | Null |
| `approved` | Populated | Populated | Populated |
| `rejected` | Populated | Null | Populated |

`source_event_ids_json` must parse as an array of at most 50 unique strings of at most 512 code units each.
Empty strings are accepted, and sorting is not required.
With `redacted=false`, `candidate_material_hash` must equal `materialHash(candidate_title, candidate_body)`.
Secret candidate sensitivity requires `redacted=true`.
A secret or deleted origin memory or projected memory also requires `redacted=true`.
Redaction requires empty candidate title and body and exactly `source_event_ids_json='[]'`.
An approved proposal must resolve to a personal projection distinct from its origin memory.
For an unredacted proposal, the projection's title, body and material hash must exactly equal its candidate fields.
These comparisons use SQL `IS NOT`, so null differs from an empty candidate string.

### Context fields and rules

| Field | Accepted value |
| --- | --- |
| `kind` | `context`. |
| `id`, `repo_id`, `local_key` | `id512`. |
| `root` | String of length 0 through 16,384 or null. |
| `repo_secret_paths_json` | String of length 0 through 32,768 or null. |
| `created_at`, `last_seen_at` | `uint`. |
| `redacted` | Boolean. |

`redacted=true` requires null `root` and `repo_secret_paths_json`.
A non-null rules field must parse as JSON and return a non-null result from `repoSecretPaths`.
That delegated [repository-rule validator](../../../src/config.ts) allows at most 64 rules.
Each rule is a string of length 1 through 256 and must pass its usable-glob check.
Invalid JSON, list shape or rule content is rejected as `invalid_context_rules` within the schema.

### Work fields and purpose redaction

| Field | Accepted value |
| --- | --- |
| `kind` | `work`. |
| `id`, `repo_id`, `origin_context_id` | `id512`. |
| `purpose` | String of length 0 through 300 or null. |
| `purpose_source_event_id` | `id512` or null. |
| `purpose_sensitivity` | `sensitivity`. |
| `state` | `active`, `dormant` or `completed`. |
| `created_at`, `updated_at` | `uint`. |
| `completed_at` | `uint` or null. |
| `current_checkpoint_memory_id` | `id512` or null. |
| `redacted` | Boolean. |

`purpose` must be null when `redacted=true` or `purpose_sensitivity='secret'`.
A non-secret, unredacted work record may also have a null purpose.

### Retained migration-origin fields and payload checks

| Field | Accepted value |
| --- | --- |
| `kind` | `migration_origin`. |
| `id` | `hex64`; equals `sha256Json(JSON.parse(origin_json))`. |
| `repo_id`, `memory_id` | `id512` or null. |
| `origin_json` | String of length 0 through 4,096; seven-element JSON tuple below. |
| `payload_hash` | `hex64`; equals tuple slot 6. |
| `stored_payload_hash` | `hex64` or null. |
| `record_kind` | One of the nine retained kinds listed below. |
| `payload` | JSON object or null; arrays and scalar top-level values are rejected. |
| `classification_state` | `pending`, `clean`, `secret` or `not_applicable`. |

Retained kinds are `memory`, `source`, `visibility`, `sharing_proposal`, `work`, `context`,
`session`, `prompt` and `excluded`.
The exact origin tuple is:

```text
["migration-origin-v1", sourceFormat, sourceRevision, sourceOriginId,
 recordKind, sourceRecordIdHash, payloadHash]
```

| Slot | Required value |
| --- | --- |
| 0 | Exactly `migration-origin-v1`. |
| 1, 2, 3 | One of the supported format/revision/origin triples below. |
| 4 | Exactly the enclosing `record_kind`. |
| 5 | `hex64`; the source-record ID hash. |
| 6 | Exactly the enclosing `payload_hash`. |

| Source format | Source revision | Source origin ID |
| --- | --- | --- |
| `oboete-export/1` | `1` | Null |
| `oboete-export/2` | `memory-provenance-visibility-sharing/1` | `hex32` |
| `claude-mem-query-export` | `8bc631a71a487424b866756e43a6efa4574cc66b` | Null |

The tuple has exactly seven elements, and its parsed-array hash must equal the enclosing `id`.
Payload and stored-payload hash must either both be null or both be populated.
Classification `secret` or `not_applicable` requires a null payload.
A populated payload must have `kind` equal to `record_kind`.
Its `sha256Hex(JSON.stringify(payload))` must equal `stored_payload_hash`.
Any source memory selected by the enclosing `memory_id` being secret or deleted requires a null payload.

`migrationPayloadRedaction` classifies embedded memory sensitivity `secret` or proposal candidate sensitivity `secret`
as secret before considering deletion.
It classifies an embedded memory's non-null, non-undefined `deleted_at`, or proposal `redacted=true`, as deleted.
For such terminal-labeled embedded payloads, the reader skips `migrationPayloadShape`.
It does not skip the enclosing origin tuple, kind, stored hash or parent-redaction checks.

Other embedded payloads must pass `migrationPayloadShape`.
For `session`, `prompt` and `excluded`, this requires only a `hex64` ID and an object-valued `external_payload`.
That external object must be non-null and must not be an array.
For other kinds, the helper requires a successful native-record schema parse and disallows `migration_origin` nesting.
It does not run `validateMemory` or the staged graph checks on the embedded record.

### Cross-record references and cycles

The following references resolve in the staged source namespace, before any destination mapping:

- Every memory, work and context repository ID resolves to a `repo` record.
- Every source and visibility parent memory ID resolves to a `memory` record.
- Every proposal origin memory ID resolves to a memory in `origin_repo_id`.
- Every work's origin context resolves to a context in that work's repository.
- A populated memory work ID resolves to a work in that memory's repository.
- A populated source context ID resolves to a context in its parent memory's repository.
- A populated source dependency ID resolves to a memory in its parent memory's repository.
- A populated supersession ID resolves to a memory.
- A populated checkpoint parent resolves to a memory in the same repository with the same work ID.
  The work comparison treats two nulls as equal and null versus a populated ID as different.
- A populated work current-checkpoint ID resolves to a memory in that work's repository whose work ID equals its ID.
- Non-personal visibility has the same repository ID as its memory.
- Work visibility resolves its work ID to a work in the grant's repository.
- Personal visibility resolves its proposal ID to an approved proposal projecting that exact visibility memory.
- Every proposal origin work resolves to a work in `origin_repo_id`.
- An approved proposal resolves its projection as described in the sharing-proposal section.
- A populated migration-origin memory ID resolves to a memory whose repository equals the enclosing repository ID.
  A null enclosing repository cannot match the memory's required repository ID.
- Every populated migration-origin repository ID resolves to a repository, even without a memory target.

Terminal-parent redaction checks apply to staged source, proposal and migration-origin records as described above.
The graph validator uses two separate Kahn traversals over scratch tables.
The first graph combines `memory -> source_memory_id` and `memory -> checkpoint_parent_id` edges.
The second graph contains `memory -> superseded_by` edges.
Duplicate edges are collapsed, and ready nodes are processed in pages of at most 1,000.
Self-edges and cycles in either graph yield `source_lineage_cycle`.
A cycle mixing source-dependency and checkpoint-parent edges is therefore rejected.

### Known validator limits

- Supersession is not combined with the dependency graph for cycle detection.
  Its target is not required to share the source memory's repository or work.
- A work grant or proposal origin work need not equal the memory's own work ID.
  An approved projection need not share its origin memory's repository.
  A personal projection is not forbidden from carrying a non-personal visibility record.
- Proposal event IDs need not be sorted or resolve to raw events, and empty event IDs are accepted.
  Basis is not coupled to decision channel, and candidate sensitivity is not compared with projection sensitivity.
- Historical raw-event, session, batch and purpose-source IDs are not resolved to records.
  Source hashes and range lengths are not compared with evidence, and citation kind/value pairing is not checked.
  Source text redaction is checked against `memory_id`, not against `source_memory_id`.
- Timestamp ordering, work state/completion-time consistency and checkpoint memory type are not checked.
  Repository identity normalization and source ID derivation from hashes are not checked in this reader.
- Redacted memory material hashes and redacted personal content hashes are not recomputed from absent text.
  Memory redaction does not clear or reject other nullable metadata strings such as `degraded_reason` or `source_agent`.
- Embedded origin payloads do not receive independent graph or reference validation.
  Terminal-labeled embedded memory/proposal payloads bypass field-shape validation.
  They need not clear their own text here.
  The embedded terminal label is not required to agree with `classification_state`.
  A `pending` or `clean` origin can retain such a payload unless its enclosing parent requires redaction.
  Embedded memory material/content hashes and personal lineage do not receive the top-level checks.
  Embedded proposal candidate hashes also bypass their top-level check.
  Embedded external support validates only ID and object shape, not the pinned external schema.
  The original `payload_hash` is checked against the tuple, not recomputed from the retained payload.
