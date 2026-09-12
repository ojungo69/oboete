# Implementation Plan: Reliable memory and work continuity

**Branch**: `009-memory-core` | **Date**: 2026-09-09 | **Spec**: [spec.md](spec.md)

**Baseline**: `c9a9e585ca89e1b2191baa14fe309bdd58f98fb6`

## Summary

Keep the TypeScript/SQLite engine, privacy boundary, fenced worker and four agent adapters.
First make accepted sources recoverable after generation failure. Then separate work progress,
project knowledge and personal knowledge, preserving those identities through migration and sync.
The specification defines the completed product; each implementation increment proves only its
own completed tasks. The shared `free-mem` checkout and PR185 remain intact.

## Technical Context

- Language: TypeScript 5.9, Node >=22.16; verify Node 22.16 and 24.16.
- Dependencies: existing SQLite, Zod, AI SDK/providers, Secretlint, Hono/Preact. Recovery and work
  identity need no new dependency. Encryption/semantic integration gets its own dependency review.
  C3 directly declares the already-installed `@secretlint/profiler@13.0.5` so detector initialization
  can disable its unused global timing collector. This repairs measured worker memory growth without
  weakening rules; installed package versions/integrities are unchanged. See
  [the measurements and library contract](contracts/injection-performance.md#worker-rss-disable-unused-library-profiling).
- Storage: existing WAL SQLite, additive SQL migrations and sanitized spool.
- Tests: existing `node:test`, migrations, fault/E2E suites, packed CLI and isolated agent homes.
- Platforms: Linux, WSL and macOS initially; native Windows later.
- Product: local CLI/hooks/stdio MCP plus foreground viewer, one developer on multiple devices.
- Budgets: capture/ready start <=300 ms, pending start <=1,300 ms, engine RSS <=150 MiB;
  measure optional local models separately and together, through 100,000 retained events.
- Constraints: one bounded worker, no model/network on capture, fixed-code diagnostics,
  explicit provider/cost choice and non-destructive migration.

## Constitution Check

Version 5.0.0 records the owner's approved product direction and 2026-09-10 resident/failover
amendment. No exception is requested.

| Principle | Design check |
| --- | --- |
| I Automatic memory | Work identity separate from native session/delivery lineage |
| II SQLite/CLI seam | Existing transactions and one lease; optional idle residency across bounded passes |
| III Privacy | Retries, sharing and imports retain detector and destination checks |
| IV Recovery/resources | Pending sources retained; bounded attempts; processed raw 30 days |
| V Completion | Recall, all agents/platforms, migration, sync and seven-day evidence remain gates |
| VI Minimality | Existing modules and profiler API; flat work items; no TaskGraph, CRDT or runtime rewrite |

Repeat this check after each increment's concrete contract/diff review.

## Increments and scope

### A. Source retention and recovery — US1, supporting US3/US7

1. Add processing/retry metadata without changing migration 0001-0003 checksums. Preserve
   legacy sources still present; report already-purged material as unavailable.
2. Remove age-forced fallback/purge. Full processed activity expires 30 days after successful
   processing. Pending material and retained supporting evidence do not follow that expiry.
3. Retry recoverable work during later bounded worker invocations. Due dates/policy prevent
   same-run retry storms. A provider change rechecks every source; a mixed fallback batch cannot
   be sent wholesale to a newly selected remote destination.
4. Replace destructive request clipping with bounded source portions and persisted coverage.
   Unsent, rejected and unaccounted portions remain pending. Explicit irrelevant/noop decisions
   are recorded; empty output alone cannot acknowledge the original batch.
5. Apply source outcomes, memory effects and temporary-guidance retirement in one fenced
   transaction. Distinguish temporary summaries from ordinary generation health.

Files: `src/db/`, `src/capture.ts`, `src/spool.ts`, `src/observer/`, `src/worker/`, related
`src/doctor/` and tests. Retention plus due retry is the first safe checkpoint; coverage/quality
remain open until separately proven.

### B. Work selection and continuation — US2

Extend existing Git identity with a local worktree key. Keep `repo_id` as project identity and
`conversation_id`/epoch for native lineage/deduplication. Flat work items own checkpoints;
sessions bind to the selected item. Branch/path names are hints, not stable global identity.
Git integration checks run in the worker, outside capture's budget. Select unambiguous work;
otherwise supply short choices without their active progress. Clear separate purposes split;
related investigation does not. Integration adopts knowledge without completing work.

Files: `src/repo-identity.ts`, `src/events.ts`, `src/capture.ts`, `src/db/queries.ts`,
`src/injection/`, work CLI/MCP controls and lifecycle fixtures.

### C. Useful recall and scoped sharing — US3/US4

Store work/project/personal visibility separately from sensitivity. All consumers apply the
same visibility rule before ranking/formatting. Integration adds project visibility to reusable
knowledge while preserving provenance. Direct personal declarations may create a sanitized
shared statement; inferred knowledge needs approval. Imported/quoted/tool text cannot approve
itself. Shared projections omit project source payloads.

Fix evaluation barriers and stage accounting before comparing model/ranking changes. Reuse
FTS/CJK, supersession and delivery tracking; correct demonstrated ranking errors. Add semantic
retrieval only if the measured paraphrase corpus requires it, with selected local/remote
embeddings and separately measured cost/resources.

Files: `src/observer/`, `src/db/queries.ts`, `src/retrieval/`, `src/injection/`, `src/why.ts`,
`src/memories-cli.ts`, `src/mcp.ts`, existing viewer controls and replay fixtures.

### D. Migration and encrypted sync — US5/US6

Extend validated transfer with stable origin IDs, scope and provenance, retaining the old
Oboete reader. A read-only claude-mem/export adapter previews mappings, keeps historical work
historical, preserves tombstones and quarantines imported content for local classification.
Do not assume a private CMEM server schema. Encrypted snapshots reuse the transfer serializer and
reader for validation only; origin/parent revision IDs make repeats idempotent and sibling
progress conflicts visible.
Clock order alone cannot resolve them. Test two local replicas before an opted-in destination.
Owner decision 2026-09-11 (research R8, `contracts/sync.md`): the 009 transport is encrypted
bundle files in a user-chosen directory, one file per replica, built from Node `crypto` only; the
maintained age implementation and the S3 client for R2 move to a later transport behind the same
envelope. The migration merger is not the sync applier: sync needs its own revision-aware apply.

Files: `src/transfer.ts`, `src/worker/imported.ts`, transfer/sync modules, config, CLI/MCP conflict
controls and isolated replica tests. Read current official APIs before adding dependencies;
cloud provisioning/real transfers remain separate activation.

### E. Cost choice and completion evidence — US7, all stories

Reuse consent tuples, provider reservations and setup. Expose free/paid/local/selected agent-CLI
modes with configured policies; label local estimates separately from provider hard caps.
No model means generation pending. Add configured model/provider fallback after free-tier failures;
each target rechecks consent and source eligibility before a bounded, reserved attempt. Free/local
failure cannot select an unselected paid destination. Free-eligible pricing is not a substitute for
provider-side free-only admission on a billing-enabled account.
Add resident waiting to the existing worker so due retries wake without another capture, with one
owner across idle/active epochs, low idle CPU, bounded processing passes and explicit pause/stop.
Keep one-shot observe for manual processing, migration safety and reproducible tests. Concrete
lifecycle and fallback contracts must be reviewed before these source changes.
Align probes with accepted native behavior; run twelve ordered agent pairs with real model
output, actual Linux/WSL/macOS, migration/sync faults, Japanese/English recall and seven-day use.
The current host has no confirmed Mac target or installed Ollama model; those are verification
prerequisites. Daily install, real spending, account changes, publication and new PR/merge work
are not activated by this local implementation.

## Project Structure

Feature artifacts live in `specs/009-memory-core/`, including `contracts/memory-core.md`.
Immediate migration: `src/db/migrations/0004_memory_processing.sql`; later migration numbers
are assigned with their implemented consumers. Reuse `test/unit/`, `test/migrations/`,
`test/e2e-*.test.ts`, `test/fault-*.test.ts` and `scripts/e2e/`. No empty future modules.

## Verification and rollback

Reproduce source-retention/recovery failure using synthetic stores and controlled transport.
Test public worker behavior, due retry, clipping, privacy rejection, lease loss, restart and
same-content replay. Run focused tests, typecheck/lint/build; broaden once for the cohesive
increment, then correctness/security, Standards/Spec and Ponytail reviews. Check new CLI flags
against actual `--help` and parsing. Record evidence before checking tasks; run final verify-tasks
in a fresh review context.

Migrations use the existing transaction and checksum verification. Rehearse a version-3 copy
and old-engine/new-schema refusal. Rollback restores the isolated pre-migration copy, never
deletes schema-version rows from an active store. Real-model evidence uses synthetic facts and
does not publish provider bodies or credentials. Missing runtime evidence stays explicitly open.
