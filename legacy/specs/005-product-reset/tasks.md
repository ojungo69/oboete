# Tasks: Lightweight Automatic Memory Product Reset M0

**Input**: Design documents from `/specs/005-product-reset/`

**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/`

**Scope**: M0 resets product authority and GitHub work tracking. Runtime implementation is split
into three later focused specifications and pull requests.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel because the task writes a different file or performs read-only work
- **[Story]**: Product user story enabled by the authority or child issue

## Phase 1: Setup and Evidence Freeze

**Purpose**: Prove the branch starts green and freeze the foundation decision before changing
public product authority.

- [x] T001 Record the clean baseline commands and expected build/check results in `specs/005-product-reset/quickstart.md`
- [x] T002 [P] Validate all Product Reset design artifacts and the 16-item specification checklist under `specs/005-product-reset/`
- [x] T003 [P] Write the accepted foundation and scope decision in `evidence/adr-006-product-reset.md`
- [x] T004 Audit every open issue and pull request into keep, defer, close, or replace dispositions in `specs/005-product-reset/issue-routing.md`

**Checkpoint**: No public authority or GitHub state changes before T001-T004 are complete.

---

## Phase 2: Foundational Product Authority

**Purpose**: Make the approved Product Reset the single active repository entry point while
preserving historical safety evidence.

- [x] T005 [US1] Rewrite the product statement, status, supported scope, architecture direction, and roadmap in `README.md`
- [x] T006 [P] [US2] Mark v6 continuity, Rust-first, and broad-platform evidence historical and index the new ADR/spec in `evidence/README.md`
- [x] T007 [US1] Verify `README.md`, `evidence/README.md`, and `specs/005-product-reset/spec.md` agree on Claude Code and Codex, Linux/WSL, automatic memory, semantic retention, and deferred Cloud/Rust scope

**Checkpoint**: A new reader reaches only the Product Reset authority from the repository entry
points; historical files remain available but are not described as active work.

---

## Phase 3: User Story 1 and 2 - Automatic Memory That Fails Open (Priority: P1) 🎯 MVP

**Goal**: Establish the parent product issue and first runtime slice without implementing runtime
code in M0.

**Independent Test**: The parent and Slice 1 issue can be read without any old issue and fully
define the bidirectional capture-to-injection flow, runtime startup, flush timing, durable fallback,
and failure acceptance scenarios.

- [x] T008 [US1] Create the Product Reset parent issue from `specs/005-product-reset/spec.md` and record its URL in `specs/005-product-reset/issue-routing.md`
- [x] T009 [US1] Create the Slice 1 automatic runtime-path issue from User Story 1 and `contracts/alpha-comparison.md`, then record its URL in `specs/005-product-reset/issue-routing.md`
- [x] T010 [US2] Add daemon outage, provider failure, spool recovery, idempotency, prompt-triggered flush, and lexical fallback acceptance criteria to the Slice 1 issue and mirror the final scope in `specs/005-product-reset/issue-routing.md`

**Checkpoint**: Slice 1 is the only implementation-ready product issue.

---

## Phase 4: User Story 3 - Simple Flexible Models (Priority: P1)

**Goal**: Establish the second focused slice for resource profiles, independent providers, and
bounded explainable injection.

**Independent Test**: The Slice 2 issue defines at most three non-credential setup choices,
independent summary and embedding providers, atomic validation, and one effective configuration
consumed by runtime and doctor.

- [x] T011 [US3] Create the Slice 2 profiles-and-retrieval issue from `contracts/capability-manifest.md` and `contracts/injection-pack.md`, then record its URL in `specs/005-product-reset/issue-routing.md`
- [x] T012 [US3] Verify the Slice 2 issue preserves semantic retrieval, explicit egress/cost behavior, lexical fallback, and the no-silent-degradation contract in `specs/005-product-reset/issue-routing.md`

**Checkpoint**: Slice 2 is blocked on Slice 1 and is not marked implementation-ready.

---

## Phase 5: User Story 4 and 5 - Inspectable Technical Alpha (Priority: P2)

**Goal**: Establish the final Alpha slice for doctor, inspection/deletion, lifecycle, packaging,
backup/restore, resource soak, and external validation.

**Independent Test**: The Slice 3 issue contains every User Story 4/5 acceptance scenario and does
not add macOS, native Windows, Cloud, Rust, or additional Agents.

- [x] T013 [US4] Create the Slice 3 doctor-and-release issue from User Stories 4 and 5 and `quickstart.md`, then record its URL in `specs/005-product-reset/issue-routing.md`
- [x] T014 [US5] Add clean install, update, backup, restore, uninstall, packed artifact, resource soak, and five-user validation gates to the Slice 3 issue and mirror the final scope in `specs/005-product-reset/issue-routing.md`

**Checkpoint**: All three Alpha slices are sequenced and only Slice 1 is active.

---

## Phase 6: Legacy PR and Issue Routing

**Purpose**: Remove the old execution order from active GitHub work without losing historical
discussion or concrete current-runtime defects.

- [x] T015 Close PR #133 without merge using the disposition in `specs/005-product-reset/issue-routing.md`
- [x] T016 Close #134 and #135 as superseded and link the Product Reset parent recorded in `specs/005-product-reset/issue-routing.md`
- [x] T017 Apply the audited keep, defer, close, and replace dispositions to all remaining open issues from `specs/005-product-reset/issue-routing.md`
- [x] T018 Verify no more than five issues have an active status and only the Product Reset Slice 1 replacement issue is implementation-ready, while preserving audited statuses on kept legacy issues; record the final counts in `specs/005-product-reset/issue-routing.md`

---

## Phase 7: Validation and Handoff

**Purpose**: Prove M0 changed product authority and work routing without changing runtime behavior.

- [x] T019 Run every command in `specs/005-product-reset/quickstart.md` that is applicable before push and record any environment-specific deviation in that file
- [x] T020 Verify the branch changes no file under `vendor/codemem/` or `harness/` and passes `git diff --check`; record the result in `specs/005-product-reset/issue-routing.md`
- [x] T021 Re-run the Product Reset specification checklist and mark every completed task in `specs/005-product-reset/tasks.md` only after corresponding file or GitHub evidence exists
- [x] T022 Run Spec Kit verify-tasks against `specs/005-product-reset/tasks.md` and write `specs/005-product-reset/verify-tasks-report.md`

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1**: Starts immediately; T003 and the read-only portion of T004 may run in parallel.
- **Phase 2**: Depends on Phase 1 so public claims cite a completed decision and routing inventory.
- **Phase 3**: Depends on Phase 2; creates the parent and first user-visible slice.
- **Phase 4**: Depends on the parent issue from Phase 3 but may be drafted before Slice 1 starts.
- **Phase 5**: Depends on the parent issue from Phase 3 and the final Alpha contract.
- **Phase 6**: Depends on all replacement issues existing so every closure has a live destination.
- **Phase 7**: Depends on repository and GitHub routing completion.

### User Story Dependencies

- **US1 and US2** share Slice 1 and form the MVP implementation path.
- **US3** depends on Slice 1's working capture/store/inject path.
- **US4 and US5** depend on the runtime and profile contracts from Slices 1 and 2.

### Parallel Opportunities

- T003 and read-only issue audit work for T004 touch independent surfaces.
- T005 and T006 touch different files after T003 is complete.
- Slice 2 and Slice 3 issue drafts may be prepared in parallel after the parent exists, but public
  mutation is serialized and only Slice 1 receives an active status.

## Parallel Example

```text
Task A: Write `evidence/adr-006-product-reset.md` from the accepted research decisions.
Task B: Read and classify all open GitHub work into `specs/005-product-reset/issue-routing.md`.
```

## Implementation Strategy

### M0 First

1. Freeze the foundation and full issue-routing inventory.
2. Update repository authority.
3. Create replacement parent and child issues.
4. Only then close or defer old work.
5. Validate that M0 changed no runtime source.

### Later Product Slices

1. Slice 1: automatic bidirectional runtime path and fail-open durability.
2. Slice 2: profiles, independent providers, semantic lifecycle, and InjectionPack compiler.
3. Slice 3: doctor, inspection, lifecycle, package, soak, and external Alpha validation.
4. Open a new Spec Kit feature for each slice; do not append runtime implementation to M0.

## Notes

- GitHub closure is reversible, but every closed item requires a replacement or explicit
  historical disposition first.
- Do not use issue priority labels alone to decide whether work blocks Alpha; tie every active
  blocker to a Product Reset acceptance scenario.
- Do not merge, release, deploy, or physically delete legacy runtime/evidence in M0.
