# Contract: Sensitivity, Repository, and Destination Egress v1

## Canonical sensitivity

Every accepted event and durable memory has exactly one first-class sensitivity:

```text
secret > private > local_only > eligible
```

The trusted daemon boundary computes it. Payload/provider claims cannot weaken it. Missing, invalid,
ambiguous, or legacy-unknown values become `secret`. Derived, deduplicated, and superseding state
keeps the strongest contributing value.

A same-identity/same-payload replay joins trusted sensitivity before ACK. The canonical event and
every existing derived record that cites it are strengthened atomically. Quarantine is absorbing,
forces `secret`, makes those records unavailable to every consumer, and returns a non-success
receipt. Different-payload conflicts never let the incoming delivery weaken or rewrite the
canonical security disposition.

If redaction cannot produce safe content, capture stores only safe ordering metadata,
`sensitivity=secret`, `capture_state=quarantined`, and a bounded error code. Failed raw content is
absent and no provider/read path can select the row.

## RepositoryIdentityV1

Restricted authority is
`repo-v1:sha256:<digest of verified canonical Git remote or realpathed primary Git anchor>`. Project
basename, caller labels, filters, cwd spelling, and workspace names are never authority. Unknown
identity denies private/local-only disclosure; linked worktrees resolve through the same verified
remote or primary anchor.

Remote canonicalization retains HTTPS/SSH transport class and the exact bounded SSH username.
HTTPS and SSH spellings, or two SSH usernames on one host/path, do not share restricted authority.
The current canonical remote is revalidated at capture and before every restricted boundary. A
change from origin A to B yields B's identity; A's restricted rows cannot match B, and a failed
revalidation falls back only to a currently verified anchor or unknown.

## DestinationBoundaryV1

One closed internal boundary carries consumer kind, concrete Agent/model when applicable,
local/remote/unknown location, verified destination repository identity or NULL, frozen manifest
fingerprint, provider fingerprint when applicable, and compiler/runtime-derived
`providerPeerTrust=verified|unverified|not_applicable`. User filters cannot construct or override it.

Claude Code, Codex, and MCP resolve to remote/unknown in Slice 1 production. A locally running CLI,
Agent/model label, project, or RPC claim is not an on-device model attestation. Local destination
classes are selected only by runner-owned fixtures after a verified loopback consumer observation;
production has no local-attestation input.

One eligibility function and its SQL predicate implement:

| Destination | eligible | local_only | private | secret |
|---|---:|---:|---:|---:|
| remote, unknown, or local provider with `providerPeerTrust=unverified` | allow | deny | deny | deny |
| verified local provider or runner-attested local consumer + exact repository match | allow | allow | allow | deny |
| trusted local + cross/unknown repository | allow | deny | deny | deny |

Local HTTP accepts only credential `none`; port ownership, PID/UID lookup, and bearer headers do not
authenticate its server. Restricted provider projection requires the exact HTTPS peer to have
passed chain and hostname/IP verification at activation and daemon start under the frozen manifest,
which sets `providerPeerTrust=verified`; HTTP sets `unverified`. Non-provider boundaries use
`not_applicable`, and local execution remains constructible only from the named trusted internal or
runner-owned path.

Local summary/maintenance groups use each source's verified repository as the boundary repository.
Viewer has eligible-only behavior unless the daemon supplies a verified repository context.

Before any local provider context or prompt is built, candidate events are stably partitioned by
exact verified repository identity. A group with mixed or unknown repository identity is rejected
content-free before projection; it is never sent as one prompt and cannot rely on post-response
citation validation to recover isolation.

## Reachable consumers

Eligibility runs before content selection or materialization for all of these paths:

- raw-event flush and structured AI maintenance before session context/prompt construction, plus
  maintenance memory-role pack/report reads;
- search, recent, timeline, explain, `findByFile`, and `findByConcept`;
- daemon get/search/pack and MCP full-body, index, recent, timeline, explain, and pack tools;
- viewer raw-event/status/usage plus memory, observation, summary, prompt, artifact, and safe-session
  projections;
- lexical/semantic pack candidates, final rendering, ledger exposure, and trace;
- export serialization and import normalization;
- dedup/supersession matching and updates.

The unused extraction-replay and distill production barrel exports are removed. Internal benchmark
code remains test-only; any future public/runtime exposure requires DestinationBoundary before it
reads raw/memory rows or builds a provider prompt.

## Provider projection

Projection occurs before session context, transcript, prompt, request body, byte measurement, or
diagnostic text exists. Mixed input retains eligible source order and provenance. An all-restricted
job makes zero provider calls and terminates through the processing-job atomic privacy-skip
transaction.

The provider choice itself must be valid and local or remote; the unknown-as-remote rule applies only
to unresolved request-time content destinations, never to manifest activation.

## Derived memory

Provider output cites only event IDs/spans from the job's exact projected set. A newly admitted v21
job projects at most 100 events; a migrated legacy recovery job may project its wider immutable
actual range. Each output inherits the strongest cited sensitivity and one exact repository identity
plus manifest/provider/attempt provenance. Unknown/out-of-set citation, mixed repository identity,
output count above the active attempt manifest's
`maxMemoryItemsPerDerivation`, or partial parse rejects the whole derivation and commits no item.

New PR3 provider XML binds each `<observation>` and `<summary>` through one direct `<citations>` child.
Every `<cite>` uses only the zero-based ordinal of the ordered projected source and may include one
optional half-open UTF-8 byte span into canonical `redactedPayload`; an omitted span normalizes to the full
payload. The Store privately binds `ProjectedSourceSetV1` to the live claim, resolves ordinals to
exact raw-event IDs and the trusted repository, and revalidates boundary/source drift before commit;
no provider-supplied ID, repository, or digest is authority. Missing, malformed, duplicate,
noncanonical, out-of-range, or drifted citations reject the complete provider output. Provider-backed
persistence without that durable claim is fail-closed; legacy NULL provenance stays secret/unknown.

Dedup/supersession is same-repository only, retains the stronger sensitivity, and never reactivates a
tombstone. Unknown identity never merges into a known repository item.

## Retrieval, trace, export, and import

Eligibility is checked in SQL when possible and always rechecked on trusted database rows before
title/body/preview formatting, pack byte/token measurement, MCP/viewer response creation, or export
serialization. Semantic hits are rehydrated from first-class database state before the same check.

Restricted omissions emit only aggregate sensitivity/lane/reason counts and
`omitted_ineligible`; no item ID, title, body, preview, query, path, or source excerpt appears. Eligible
injected items retain visible source and selection reasons.
Fixture `expectedOmissions` content is runner-only test evidence, not runtime trace output.

Export payload v2 applies the boundary to memory items, user prompts, legacy session summaries, and
safe session shells. Restricted data requires a verified same-repository local boundary; unknown/
all-project export emits eligible rows only. Session shells omit cwd, Git remote/branch, user, and
free-form metadata. Import v2 preserves valid fields without downgrade; missing/malformed becomes
secret/unknown. Legacy v1 content always imports secret/unknown, and project/remap labels never grant
authority.

## Content-free evidence

Durable diagnostics, logs, status, doctor, maintenance progress, job failure, and pack omission trace
contain only closed reason/action codes, counts, state, safe fingerprints, and next action. They never
contain content, titles, paths, prompts, queries, provider response excerpts, sentinels, restricted
previews, or credential values. Wire request/byte claims come only from the runner-owned transport
evidence.

## Semantic-disabled behavior

`semantic_disabled` means the semantic lane contributes no candidates. Lexical retrieval remains
healthy and existing vector rows remain stored. Disabling semantic use is never a deletion signal.
