# Design decisions and evidence

## R1. Repair source processing before replacing the foundation

**Decision**: Keep TypeScript, SQLite, privacy checks, agent adapters and the fenced worker.
Change source completion, recovery and retention before adding retrieval infrastructure.

At c9a9e585, `src/observer/apply.ts` makes every output/fallback terminal,
`src/worker/batches.ts` selects only unbatched rows, and `src/worker/purge.ts` deletes raw
material of applied/fallback batches at capture-time expiry. `src/observer/contract.ts` can
remove/shorten source events without persisting which material was omitted.

The [provider diagnostic](../../docs/evidence/quality-debt-2026-09/batch-d-provider-diagnostic.md)
found 22 expected facts absent from memories. Read-only follow-up confirmed their source rows
were present and unexcerpted; model omission, noop, rejection and application were not separately
recorded. Harness timing/lease defects also prevent using that run as a provider-quality ranking.
Vectors or a language rewrite alone cannot recover already-consumed sources.

## R2. Worktree provenance does not replace task identity

**Decision**: Adopt integrated project knowledge with separate active-work boundaries.
The reviewed [claude-mem v13.24.1](https://github.com/thedotmack/claude-mem/releases/tag/v13.24.1)
[project context](https://github.com/thedotmack/claude-mem/blob/v13.24.1/src/utils/project-name.ts)
combines parent/current worktree labels. Its
[context query](https://github.com/thedotmack/claude-mem/blob/v13.24.1/src/services/context/ObservationCompiler.ts)
selects recent observations/summaries within those labels. It does not establish distinct
purposes inside one worktree. This stronger boundary is an approved Oboete requirement.

[CMEM Pro integration](https://docs.claude-mem.ai/cmem-pro-headless) documents a remote observer
and [cloud sync](https://docs.claude-mem.ai/cloud-sync); public evidence does not establish
automatic task isolation. No private cloud account was tested.

## R3. Temporary availability and completed generation are different

**Decision**: Keep useful fallback output while its accepted sources remain recoverable.
Persist considered portions and explicit outcomes with memory effects. An irrelevant declaration
is distinguishable from omission and remains subject to quality evaluation. Reuse content hashes,
tombstones, validity and lease fencing. Retry on bounded worker invocations, not a new scheduler.
A changed provider must reapply destination rules to every source in an old fallback group.

## R4. Retention starts at processing

**Decision**: Default full activity retention is 30 days after successful processing. Pending
accepted material is not age-purged; important supporting portions stay with knowledge.
Secret and failed-classification exclusions retain the existing fail-closed behavior.

The owner explicitly approved this after comparison. Upstream
<a href="https://github.com/thedotmack/claude-mem/blob/
v13.24.1/src/services/worker/SessionMessageBuffer.ts">transcript recovery</a>
depends on agent-owned transcripts. Oboete already accepts sanitized raw rows and must preserve
its own accepted material across agent/platform differences. No arbitrary disk quota or deletion
of pending work is introduced.

## R5. Work scope and native lineage remain separate

**Decision**: Flat work items own current checkpoints; native session/conversation/epoch identity
retains resume/deduplication semantics. Visibility membership is independent of sensitivity.
Merge adopts reusable knowledge and never closes work by itself. A general TaskGraph, branch-name
identity and clock-only sync were rejected because they do not satisfy these boundaries.

## R6. Measure actual quality and preserve activation boundaries

**Decision**: Controlled transports prove deterministic correctness; selected real local/external
profiles prove quality. No model is generation-pending, not healthy. Models are qualified by
measured outcomes rather than assumed interchangeable.

Agent-CLI and Ollama adapters exist, but the inspected host has no installed Ollama model.
The Xcode tool returned `spawn xcrun ENOENT`; no Mac target was demonstrated. Native capability
negatives already accepted by M1 amendments A8/A16/A21 must not be labeled new regressions.
Actual Mac, full agent-pair, real-model and seven-day evidence remain open until run.

Semantic-index and remote-encryption integration details are refined in their own increments
against current primary APIs before dependency additions. No unresolved product decision blocks
the source-recovery increment.

## R7. Migration input formats

**Decision**: Support one explicitly selected external adapter in the first migration increment.
Name it `claude-mem-query-export@8bc631a` and pin it to claude-mem v13.24.5 commit
`8bc631a71a487424b866756e43a6efa4574cc66b`. The input has no format discriminator, so the CLI
must require the declared source format instead of guessing from arbitrary JSON.

The exporter runs the supplied query through hybrid search and collects matching observations,
summaries and prompts. It then fetches sessions only for memory-session IDs found in observations
and summaries. It writes one plain JSON object, so the result is a query-selected subset rather
than a full database backup.

Sources: <a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/scripts/export-memories.ts#L45-L109">exporter</a> and
<a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/docs/public/usage/export-import.mdx#L15-L25">public docs</a>.

The top-level object has timestamps, the query, optional project, four totals and four record
arrays. It has no schema version, source repository identity, authoritative source installation
or device ID, sensitivity marker, deletion record or tombstone. Wildcard result rows can still
carry per-record sync-origin fields; those fields do not identify the exporting installation.
Cloud sync writes a higher-revision tombstone outside the live row and then deletes that row.
The classic exporter cannot carry that tombstone, so absence from another query export never
proves deletion.

Sources: <a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/scripts/export-memories.ts#L94-L109">export object</a>,
<a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/src/services/sqlite/SessionStore.ts#L2296-L2365">rows</a>,
and <a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/src/services/sync/CloudSync.ts#L647-L699">deletion</a>.

The adapter requires the pinned observation, summary, session and prompt fields. Observation and
summary queries return current SQLite rows with `o.*` and `ss.*`. Prompt queries return `up.*`
plus joined project, memory-session and normalized platform fields. Content hashes, model and
relevance metadata, merged-project fields, agent IDs and sync-origin fields are optional
extensions. The session batch has an explicit field list and includes `custom_title`; it does not
export observed model or billing fields at this pin. Unknown bounded extension fields may be
retained as provenance but cannot acquire local authority.

Sources: <a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/src/services/sqlite/types.ts#L1-L43">row types</a>,
<a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/src/services/sqlite/SessionStore.ts#L2296-L2365">observations</a>,
<a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/src/services/sqlite/SessionStore.ts#L2804-L2895">summaries and prompts</a>,
and <a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/src/services/sqlite/SessionStore.ts#L2431-L2458">sessions</a>.

The actual session repeat key is normalized `(platform_source, content_session_id)`. Summary rows
use `memory_session_id`. Observation rows use `(memory_session_id, title, created_at_epoch)`.
Prompts use `(session_db_id, prompt_number)` when a session resolves and fall back to
`(content_session_id, prompt_number)` otherwise. Because observation title is nullable, the SQL
predicate `title = ?` does not match a null title and can miss that duplicate. These upstream skip
keys are not strong enough to serve as Oboete origin or content identities.

Source: <a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/src/services/sqlite/SessionStore.ts#L3115-L3348">import methods</a>.

The upstream JSON body parser accepts at most 5 MB. The route checks only that the four members
are arrays, then inserts sessions, summaries, observations and prompts sequentially without one
surrounding transaction. This contradicts the public documentation's transactional and rollback
claims. Oboete must enforce its own byte and record bounds and use one all-or-nothing apply
transaction.

Sources: <a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/src/services/worker/http/middleware.ts#L13-L17">body limit</a>,
<a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/src/services/worker/http/routes/DataRoutes.ts#L89-L93">shape check</a>,
<a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/src/services/worker/http/routes/DataRoutes.ts#L458-L629">handler</a>,
and <a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/docs/public/usage/export-import.mdx#L196-L265">documented claim</a>.

A prompt-only search match can have no exported session. Prompts are collected independently, but
the exporter builds its session request only from observation and summary memory-session IDs.
The adapter must retain this as unresolved provenance instead of rejecting an otherwise complete
file.

Source: <a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/scripts/export-memories.ts#L63-L101">session collection</a>.

CMEM's public page describes remote observer generation, opt-in cloud sync of the local
observations database and private MCP access. The pinned client syncs local observation, summary
and prompt operations, including tombstones, through a separate protocol. These sources do not
publish a second portable private-cloud export contract. Oboete therefore claims support only for
the pinned local JSON export, not for a private CMEM export, API or cloud database.

Sources: [CMEM pricing](https://cmem.ai/pricing) and
<a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/src/services/sync/CloudSync.ts#L115-L245">sync rows</a>.

Direct reads of the four raw GitHub URLs failed because `raw.githubusercontent.com` could not be
resolved in the research sandbox. The GitHub file-content reader then returned base64 bytes at
the exact pinned ref. Decoding those bytes directly into `sha256sum`
without temporary files produced these values, all equal to the supplied research memo.

| Source | Bytes | Observed SHA-256 |
| --- | ---: | --- |
| `scripts/export-memories.ts` | 5,476 | `0c820510cf462491165f08e475eb71bbed04bcd4f7dfeb52b785f69d954fe556` |
| `scripts/import-memories.ts` | 3,085 | `3372f25074cc4816f39fb1e70fd04d9a726b05505b1437e50552dbe982867e29` |
| `docs/public/usage/export-import.mdx` | 8,092 | `dfc5fed8176ddb2c623f1dc708889f7a14b51034a0ab8e6c558835acc0146b27` |
| `src/types/database.ts` | 1,559 | `57c1e9286d3e9033aebe0c6d3433cc8f984e165b87296060812bf93686d20c4d` |

Hash inputs: <a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/scripts/export-memories.ts">exporter</a>,
<a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/scripts/import-memories.ts">importer</a>,
<a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/docs/public/usage/export-import.mdx">documentation</a>,
and <a href="https://github.com/thedotmack/claude-mem/blob/
8bc631a71a487424b866756e43a6efa4574cc66b/src/types/database.ts">database types</a>.

The fetched Git blob IDs were `4f39f727ab391f433108e90c89510200f5f0a0b0`,
`70d8727a839e08debed994fc78c98f719e3ae70e`, `197bb37f51fb0e7457b531f5be7f748a850a2589`
and `4531b3bfbeb010a73d58cfe1b35ff675480e5b67` in table order.

This research did not inspect a user store, CMEM account or provider. It establishes only
the public portable export contract at the pinned revision. It does not establish direct SQLite
migration, private CMEM state or deletion transport through this format.

## R8. Device sync: file bundles with Node `crypto` only

**Decision** (owner, 2026-09-11): US6 in 009 syncs through encrypted bundle files in a directory the
user chooses, so any file-sync tool (iCloud Drive, Dropbox, Syncthing, a USB stick) moves them. No
dependency is added; the envelope is built from Node `crypto` primitives. Cloudflare R2 remains a
later transport behind the same envelope. This supersedes the plan D sentence that named a
maintained age implementation and an S3 client; that earlier research
(`/var/tmp/oboete-009-20260909.jJ5grc/sync-api-research.md`, 2026-09-10, age-encryption 0.3.1 and
aws4fetch 1.0.20) stays valid for the R2 day and its revision/snapshot identity rules are reused as
written in `contracts/sync.md`.

Why file bundles first: the developer already runs a file-sync tool between the machines in
question; a directory needs no account, credential, endpoint, jurisdiction or cost class in the
consent tuple, and "no network" becomes a property the tests can enforce rather than a promise. Why
one file per replica: file-sync tools have no compare-and-swap, so a single shared object would need
the conditional-PUT protocol the R2 research designed; when each replica writes only its own bundle,
concurrent writers never touch the same file and readers merge revisions locally.

Why Node `crypto` and not `age-encryption`: the only primitives needed are a CSPRNG, HKDF-SHA-256,
AES-256-GCM with AAD, SHA-256 and constant-time comparison, all present in Node 22.16/24.16. The
chunked construction copies age's payload layer (64 KiB chunks, 11-byte counter plus final-byte
nonce, HKDF-derived per-file key) with AES-GCM instead of ChaCha20-Poly1305 because Node ships the
former with hardware acceleration and the latter with the same API; there is no recipient or
passphrase layer to reimplement, because one symmetric space key carried by the user replaces
X25519 recipients in a one-person product. The age passphrase path would have cost ~256 MiB of
scrypt working memory, above the engine budget.

Primary sources (Node 22 API):
[`crypto.randomBytes`](https://nodejs.org/docs/latest-v22.x/api/crypto.html#cryptorandombytessize-callback),
[`crypto.hkdfSync`](https://nodejs.org/docs/latest-v22.x/api/crypto.html#cryptohkdfsyncdigest-ikm-salt-info-keylen),
[`crypto.createCipheriv` with `authTagLength`](https://nodejs.org/docs/latest-v22.x/api/crypto.html#cryptocreatecipherivalgorithm-key-iv-options),
[`cipher.setAAD`](https://nodejs.org/docs/latest-v22.x/api/crypto.html#ciphersetaadbuffer-options),
[`cipher.getAuthTag`](https://nodejs.org/docs/latest-v22.x/api/crypto.html#ciphergetauthtag),
[`decipher.setAuthTag`](https://nodejs.org/docs/latest-v22.x/api/crypto.html#deciphersetauthtagbuffer-encoding),
[`crypto.timingSafeEqual`](https://nodejs.org/docs/latest-v22.x/api/crypto.html#cryptotimingsafeequala-b),
[`crypto.hash`](https://nodejs.org/docs/latest-v22.x/api/crypto.html#cryptohashalgorithm-data-outputencoding).
Chunk construction: [age specification, "Payload"](https://c2sp.org/age#payload) (64 KiB chunks;
nonce = 11-byte big-endian counter + `0x01` final byte; streaming decryption must fail on a
missing final chunk). Not used: [`crypto.scryptSync`](https://nodejs.org/docs/latest-v22.x/api/crypto.html#cryptoscryptsyncpassword-salt-keylen-options)
(a passphrase-wrapped key export would need it with `maxmem` bounded; deferred) and
[`crypto.generateKeyPairSync('x25519')`](https://nodejs.org/docs/latest-v22.x/api/crypto.html#cryptogeneratekeypairsynctype-options)
/ [`crypto.diffieHellman`](https://nodejs.org/docs/latest-v22.x/api/crypto.html#cryptodiffiehellmanoptions)
(per-device recipients; deferred with R2).

What this research does not establish: that any specific file-sync tool preserves rename atomicity
or delivers whole files (the envelope's final-chunk marker and per-bundle authentication make a
half-delivered file a rejected file, so nothing depends on it); real two-device operation, which
stays a separate activation; and the 0008 schema, which T034 designs from the contract.
