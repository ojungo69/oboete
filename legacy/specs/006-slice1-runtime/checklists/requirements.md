# Specification Quality Checklist: Slice 1 Automatic Memory Runtime

**Purpose**: Validate specification completeness and quality before proceeding to planning
**Created**: 2026-08-30
**Feature**: [spec.md](../spec.md)

## Content Quality

- [x] No source-file/framework implementation plan leaks into the spec; named wire/security/state
  contracts are intentional user-supplied invariants
- [x] Focused on user value and business needs
- [x] User stories lead with user value; technical contract detail is confined to requirements and
  edge cases needed for deterministic implementation
- [x] All mandatory sections completed

## Requirement Completeness

- [x] No [NEEDS CLARIFICATION] markers remain
- [x] Requirements are testable and unambiguous
- [x] Success criteria are measurable
- [x] Success criteria are observable at contract/runtime boundaries and contain no source-path tasks
- [x] All acceptance scenarios are defined
- [x] Edge cases are identified
- [x] Scope is clearly bounded
- [x] Dependencies and assumptions identified

## Feature Readiness

- [x] All functional requirements have clear acceptance criteria
- [x] User scenarios cover primary flows
- [x] Feature meets measurable outcomes defined in Success Criteria
- [x] Implementation paths remain in plan/tasks; specification names only required observable
  contracts and persistence/lifecycle invariants

## Source-Hardening Review

- [x] Provider proposal is closed and buildable: two wire protocols, complete canonical endpoint,
  explicit CredentialRefV1, exact auth/request/response/no-auth behavior and resource limits,
  computed provider/manifest fingerprints, local-HTTP credential-none/eligible-only behavior,
  authenticated local-HTTPS restricted behavior, and compiler-derived policy
- [x] Contract/fixture correction is a separate first checkpoint; no runtime consumer precedes setup
  activation
- [x] Base/local/repaired manifests and resume/redirect/downgrade fingerprints are complete; v2/max17
  remains a runner-only fault successor and static PR 0 makes no runtime claim
- [x] Fixed resource profile includes periodic, idle, debounce, stuck-claim, retention, and 100-event
  job fields with enforcement ownership named by PR
- [x] Privacy covers provider, maintenance, all Store/reference/daemon/MCP/viewer/pack/trace/export/
  import/dedup consumers through one DestinationBoundary, including viewer artifacts and export-v2
  prompt/legacy-summary/safe-session projections
- [x] DestinationBoundary carries compiler/runtime-derived provider peer trust, so its pure function
  distinguishes unverified local HTTP from verified local HTTPS without caller or mutable-config input
- [x] Claude/Codex/MCP model egress is remote/unknown by default; no caller/local-process claim can
  select runner-only on-device destination classes
- [x] Repository authority uses transport-preserving verified Git remote with exact SSH user or a
  realpathed primary Git anchor, never basename/project label or cross-transport/user collapse;
  capture/boundary revalidation invalidates cached authority after origin A→B
- [x] Schema v21 durable jobs, retry-exhausted capacity, one-shot resume, attempt/admission provenance,
  attempt-count 0 plus automatic claims 1-3, atomic completion, retention safety, and legacy `gave_up`
  handling precede #130 closure
- [x] RawEventSweeper production nudge and stop-race, content-free diagnostics, semantic-disabled
  vector retention, and generated artifacts have named source/tests
- [x] EgressDiagnosticV1 requires a closed safe `nextAction`; no producer may omit it or emit free text
- [x] Setup pointer/editor activation publishes `current` last and has owner-only interruption-journal
  recovery with all-target zero mutation when any external hash is unknown, plus shared setup/daemon
  lifecycle-lock exclusion before provider startup
- [x] Activation receipt, persisted provider health edge, and user-confirmed doctor retry are the only
  crash-idempotent resume-signal producers; every signal binds exact job+producer receipt and global
  setup/health events fan out only to the bounded matching job set
- [x] Closed runner-evidence schema owns initial plus every retry/redirect recovery provider-egress
  observation (including zero-egress no-ops), executed plateau or canonical no-activity null plateau,
  and six ordered TLS receipts; result
  fingerprints/aggregates bind them; every observation binds the bundle invocation, owning case, and
  one fresh bundle-unique process-tree root; TLS network/receipt evidence binds the invocation, while
  plateau candidate/artifact/environment/invocation/root and one identical duplicate workload count
  are closed; plateau timestamps strictly prove workload→drain→checkpoint→
  sample order and non-overlap while pair-bound conflict reuse/distinctness and exact 16+1 evidence
  remain closed; sensitivity-byte totals cannot exceed observed payload bytes, and runner-owned
  restricted-payload/sentinel observations are zero
- [x] Provider authorization records explicit committed event IDs/count/fingerprint and accepts
  canonical non-prefix sets; no validator infers the authorized set from count alone
- [x] Tasks are sequential T001-T060 and every independent PR boundary/dependency/range is explicit

## Notes

- Validated against issues #130/#137, current Codemem source/test paths, and Product Reset intent.
- The pre-PR 0 `specs/005-product-reset/` fixture/schema/validators/bound evidence required the
  contract-first mechanical correction co-delivered with these planning artifacts in T001-T005.
- No clarification marker or unchecked checklist item remains; the artifacts are ready for the PR 0
  contract checkpoint.
