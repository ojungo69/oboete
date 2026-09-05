# Research: Lightweight Automatic Memory Product Reset

The full foundation audit, source evidence, rejected bases, and revisit triggers live in
[`../../evidence/adr-006-product-reset.md`](../../evidence/adr-006-product-reset.md). This file keeps
only the decisions needed to derive the Product Reset plan.

## Decision 1: Keep the Codemem safety kernel conditionally

**Decision**: Preserve the pinned daemon writer, bounded spool, redaction, local retrieval,
Claude/Codex hooks, observer, backup, CLI/MCP, and viewer assets.

**Rationale**: The branch baseline passes build, typecheck, lint, and 1,895 tests; replacing it
would discard 103 vendor-focused safety commits before a user-visible Alpha exists.

**Alternatives considered**: claude-mem runtime fork, greenfield runtime, and normal Codemem
upstream tracking were rejected for the deltas detailed in ADR-006.

## Decision 2: Use claude-mem as UX/test donor only

**Decision**: Selectively adapt automatic-context behavior and characterization tests with
Apache-2.0 attribution; do not share its runtime lifecycle.

**Rationale**: Its proven UX is valuable, while Chroma, worker/settings coupling, and unbounded
runtime state are the liabilities free-mem is intended to remove.

**Alternatives considered**: direct hook bridge and deletion-until-minimal fork both retain
unstable upstream coupling and fail to prove unobserved paths.

## Decision 3: Measure before changing runtime language

**Decision**: Use one pinned fixture/result contract for capture correctness, injection facts,
latency, process/RSS slope, queue, storage, and tokens before changing the foundation.

**Rationale**: The observed problem is unbounded sidecar growth, not proof that Node itself fails
the resource envelope.

**Alternatives considered**: Rust-first rewrite and arbitrary minimum-RSS gates are deferred.

## Decision 4: Compile one effective capability manifest

**Decision**: A pure compiler produces the versioned, secret-free manifest consumed by setup,
runtime, and doctor.

**Rationale**: A small profile remains simple while summary and embedding execution stay
independent and privacy/cost/degradation cannot drift between behavior and diagnostics.

**Delivery boundary**: Slice 1 ships the permanent minimal manifest—one resource profile, explicit
summary provider, embedding disabled, lexical fallback. Slice 2 extends the same compiler to
multiple profiles, embedding providers, and semantic lifecycle.

**Alternatives considered**: exposing legacy settings, coupling both provider roles, and direct
environment reads by each subsystem were rejected.

## Decision 5: Stabilize the InjectionPack output

**Decision**: Normalize candidates, enforce manifest budgets, apply deterministic eligibility and
ordering, and record provenance plus inclusion/omission/degradation reasons.

**Rationale**: Storage and providers may change; what the Agent receives is the durable product
behavior.

**Alternatives considered**: protocol-first standardization, semantic-only retrieval, and a
lexical-only product were rejected.

## Decision 6: Linux/WSL before macOS

**Decision**: Alpha supports Linux/WSL on local Linux filesystems. macOS is the first post-Alpha
platform work package.

**Rationale**: The current durability boundary explicitly rejects non-Linux platforms; validating
equivalent macOS semantics would delay the first useful release.

**Alternatives considered**: including macOS now or changing foundations for platform coverage.

## Decision 7: Reset authority before deleting history

**Decision**: Update README, evidence index, spec, ADR, and GitHub routing now; keep continuity
assets historical until replacement runtime tests pass.

**Rationale**: Moving or deleting them in M0 creates link/test churn without user value. Git
history remains the final archive.

**Alternatives considered**: archive-directory renames and immediate physical deletion.

## Decision 8: Keep Pro behind the local core

**Decision**: Encrypted sync, multi-device backup, and hosted inspection may follow external Alpha
validation but never become local capture/retrieval dependencies.

**Rationale**: This preserves the long-term lane without making remote identity/conflict work block
the local product again.

**Alternatives considered**: building Cloud alongside Alpha.
