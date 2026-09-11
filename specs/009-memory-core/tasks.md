# Tasks: Reliable memory and work continuity

**Input**: [plan.md](plan.md), [spec.md](spec.md), [data-model.md](data-model.md),
[contracts](contracts/memory-core.md), [research.md](research.md).

Current resume entrypoint: [Claude Code handoff](HANDOFF-claude-code.md), 2026-09-10.

Tests are required by the feature's acceptance scenarios. A checked item needs an implementation
and verification receipt. Full product completion is distinct from the first safe increment.

## Phase 1: Setup

- [X] T001 Record the confirmed direction and 16-item requirements review in `specs/009-memory-core/spec.md` and `checklists/requirements.md`.
- [X] T002 Apply the approved 4.0.0 amendment in `CONSTITUTION.md` and synchronize the local Spec Kit copy.
- [X] T003 Prepare isolated `009-memory-core`, install the pinned `package-lock.json`, and inspect impact in `src/observer/` and `src/worker/`.

## Phase 2: Foundation

- [X] T004 Review `specs/009-memory-core/plan.md`, `data-model.md` and contracts for migration, privacy, retry and completion consistency; resolve material findings before source edits.
- [X] T005 Define the concrete increment-A processing/retry columns and compatibility rules in `specs/009-memory-core/data-model.md` and `contracts/memory-core.md`.

## Phase 3: US1 — Recover accepted information (P1)

Independent test: fail generation, outlive old retention, restore the selected provider and
recover without loss/duplicate effects; oversized or rejected source portions stay accounted for.

- [X] T006 [US1] Reproduce failed-generation retention/recovery and post-processing retention in `test/unit/memory-recovery.test.ts` using `test/helpers/observe.ts`.
- [X] T007 [US1] Test version-3 upgrade, unchanged historical checksums and old-engine refusal in `test/migrations/memory-processing.test.ts`.
- [X] T008 [US1] Add processing/retry metadata in `src/db/migrations/0004_memory_processing.sql` and register it in `src/db/open.ts`.
- [X] T009 [US1] Remove forced fallback on source age; preserve pending sources and implement processed 30-day retention in `src/worker/batches.ts`, `src/worker/purge.ts`, `src/observer/apply.ts` and `src/capture.ts`.
- [X] T010 [US1] Requeue due generation safely under the current destination/consent and release the worker with deferred work in `src/worker/batches.ts`, `src/worker/observe.ts` and `src/worker/observe-batch.ts`.
- [X] T011 [US1] Test source-cap omission, partial success and crash/resume coverage in `test/unit/memory-recovery.test.ts` and `test/unit/request.test.ts`.
- [X] T012 [US1] Process bounded source portions with explicit outcomes in `src/observer/request.ts`, `src/observer/contract.ts`, `src/observer/llm.ts` and `src/worker/observe-batch.ts`.
- [X] T013 [US1] Persist source receipts/evidence and retire replaced temporary guidance atomically in `src/observer/apply.ts`, `src/worker/purge.ts` and the processing migration.
- [X] T014 [US1] Expose pending/partial/recovered generation separately from provider connectivity in `src/doctor/storage.ts`, `src/doctor/provider.ts` and `src/why.ts`.
- [X] T015 [US1] Verify privacy, lease/crash and storage-failure cases through existing `test/fault-worker.test.ts`, `test/unit/apply.test.ts`, `test/unit/purge.test.ts` and focused recovery tests; record evidence in `specs/009-memory-core/quickstart.md`.

## Phase 4: US2 — Continue the intended work (P1)

Independent test: interleave worktrees and two purposes within one worktree; another agent
resumes the selected work with zero unrelated active checkpoints.

- [X] T016 [US2] Specify and test context/work binding, collisions, native resume and ambiguous choice in `specs/009-memory-core/contracts/work.md` and `test/unit/work-context.test.ts`.
- [X] T017 [US2] Extend Git identity and add context/work/session bindings in `src/repo-identity.ts`, `src/capture.ts` and a numbered `src/db/migrations/` file.
- [X] T018 [US2] Implement automatic/explicit work selection and current checkpoints in `src/db/queries.ts`, `src/injection/pack.ts`, `src/mcp.ts` and work CLI operations.
- [X] T019 [US2] Preserve related investigation, compaction/fork lineage and outstanding steps after merge in `src/events.ts`, `src/observer/classify.ts` and work operations.
- [ ] T020 [US2] Verify removed worktrees and ordered cross-agent continuation through `scripts/e2e/isolated-user.mjs` and `scripts/e2e/probe-lib/isolated-lifecycle*.mjs`; record actual runs in `specs/009-memory-core/quickstart.md`.

## Phase 5: US3 — Useful current knowledge (P1)

Independent test: Japanese/English paraphrases, superseded facts and already-delivered facts
produce separately scored retention, retrieval, delivery and answer outcomes.

- [X] T021 [US3] Correct readiness/lease barriers and prior-delivery accounting in `src/fixture/replay.ts` and `src/fixture/replay-evaluate.ts`, with a focused `test/unit/replay-evaluate.test.ts` regression.
- [X] T022 [US3] Add source-stage accounting and inspectable omission reasons in `src/fixture/replay-evaluate.ts`, `src/why.ts` and `src/fixture/replay-report.ts`.
- [ ] T023 [US3] Reproduce and correct demonstrated lexical/MMR/supersession misses in `test/unit/rank.test.ts`, `src/retrieval/rank.ts` and `src/db/queries.ts`.
- [ ] T024 [US3] Qualify selected local/external profiles on the paraphrase corpus; add semantic retrieval in `src/retrieval/` only if the measured target requires it, documenting primary API/dependency evidence in `specs/009-memory-core/research.md`.

## Phase 6: US4 — Share at the correct scope (P1)

Independent test: work, project and personal knowledge include only intended material across
tasks/projects; inferred sharing and imported/tool instructions cannot self-approve.

- [X] T025 [US4] Specify and test scope/approval/adoption boundaries in `specs/009-memory-core/contracts/sharing.md` and `test/unit/memory-scope.test.ts`.
- [X] T026 [US4] Add stored visibility and proposal state with observer provenance checks in `src/db/migrations/`, `src/observer/contract.ts` and `src/observer/apply.ts`.
- [X] T027 [US4] Apply common visibility selection to `src/db/queries.ts`, `src/injection/pack.ts`, `src/mcp.ts` and `src/viewer/server.ts`.
- [X] T028 [US4] Implement proposal approval and knowledge adoption without task completion in `src/memories-cli.ts`, `src/mcp.ts` and existing viewer controls; verify cross-project exclusions in `test/unit/memory-scope.test.ts`.

## Phase 7: US5 — Preserve existing memories (P1)

Independent test: preview and import a supported frozen corpus twice without changing the
source store, duplicating effects, reviving tombstones or activating historical tasks.

- [ ] T029 [US5] Pin supported claude-mem/CMEM export schemas and migration mappings in `specs/009-memory-core/contracts/migration.md` using primary sources and synthetic fixtures.
- [ ] T030 [US5] Extend versioned Oboete transfer with scope/provenance and the old reader in `src/transfer.ts` and `test/unit/transfer.test.ts`.
- [ ] T031 [US5] Add a read-only migration adapter with dry-run/explicit mapping and classification quarantine in migration operations and `src/worker/imported.ts`.
- [ ] T032 [US5] Verify source immutability, identity collisions, tombstones, repetition and historical work in `test/unit/migration-import.test.ts` and packed CLI checks.

## Phase 8: US6 — Carry memory across devices (P1)

Independent test: disconnected replicas converge idempotently, propagate deletion and expose
incompatible progress conflicts without clock-only overwrites.

- [x] T033 [US6] Pin encryption/transport APIs and record envelope/revision contracts in `specs/009-memory-core/research.md` and `contracts/sync.md` before adding dependencies.
- [x] T034 [US6] Add stable replica/revision identity and merge/conflict behavior using `src/transfer.ts`, `src/db/migrations/` and `test/unit/sync.test.ts`.
- [x] T035 [US6] Implement opted-in encrypted push/pull and destination consent in sync operations, `src/config.ts` and `src/setup/consent.ts`.
- [x] T036 [US6] Expose conflict choices through CLI/MCP and verify interruption, tampering, deletion and repeated transfers in `test/unit/sync.test.ts`.

## Phase 9: US7 — Choose model and cost (P2)

Independent test: selected free/local/paid/agent modes obey consent and configured limits;
free/local failure causes zero attempts at an unselected paid destination.

- [ ] T037 [US7] Test mode choice, limit exhaustion and consent changes in `test/unit/providers.test.ts` and `test/unit/setup.test.ts`.
- [ ] T038 [US7] Complete explicit cost-policy setup and reservation handling in `src/config.ts`, `src/setup/`, `src/observer/reservation.ts` and `src/doctor/provider.ts`.
- [ ] T039 [US7] Verify no-model capture-only behavior and real chosen profiles through `src/doctor.ts`, packed CLI and `specs/009-memory-core/quickstart.md` evidence.

Owner amendment, 2026-09-10:

- [X] T046 Record the approved resident-worker option and configured model/provider failover in `CONSTITUTION.md`, the local Spec Kit constitution, `specs/009-memory-core/spec.md` and `plan.md`.
- [ ] T047 [US1] Implement and verify resident waiting for new/due work, one owner through idle/active epochs, pause/stop/config changes and upgrade/crash recovery in `src/worker/`, capture startup and operator controls; retain bounded one-shot observe and prove idle/long-run resources.
- [ ] T048 [US7] Implement a bounded, consented model/provider fallback chain after free-tier/API failures in provider selection, reservations, setup/config and worker processing; verify free-only admission, shared quota versus target failure, per-attempt source eligibility and all-targets-failed retention.

## Phase 10: Completed-product verification

- [ ] T040 Align accepted native capabilities and run actual Linux/WSL/macOS checks in `.github/workflows/` and `scripts/e2e/probes/`; preserve unsupported/unavailable verdicts.
- [ ] T041 Run all twelve ordered agent pairs and selected real-model Japanese/English evaluations; record sanitized evidence under `docs/evidence/memory-core-2026-09/`.
- [ ] T042 Measure 1,000/10,000/100,000-event resources and seven days of real use with `src/fixture/replay.ts` and `scripts/measure-cold-start.mjs`; include local-model consumption.
- [ ] T043 Run cohesive typecheck/lint/build/tests/pack and correctness/security, code-review and ponytail-review; record results in `specs/009-memory-core/quickstart.md`.
- [ ] T044 Update `README.md` and user-facing help with only verified capabilities and remaining limitations.
- [ ] T045 Run fresh-context verify-tasks for `specs/009-memory-core/tasks.md`, validating every completed marker against source and receipts.

## Dependencies and implementation strategy

T001-T005 precede source edits. US1 first: T006/T007 reproduce before T008-T010; T011 precedes
T012/T013; T014/T015 validate the cohesive lifecycle. T009/T010 may form an intermediate checkpoint,
but US1 is incomplete until T011-T015 pass. Work binding precedes scoped sharing; both precede
scope-preserving migration/sync. US7 policy verification may move earlier when retry uses it.
T046-T048 implement the subsequent owner amendment. Concrete resident lifecycle and fallback
contracts/reviews precede their source changes; both are required before the completed-product gates.
US3 evaluation correction can run alongside read-only work-scope design, but source writes are
serialized within this worktree. Each later contract task resolves its concrete technical details
before that increment's implementation. No unresolved product question requires re-approval.

Parallel work is read-only review/research beside the single implementation writer: US1 privacy
review, US2 native-lineage review, US3 corpus audit, US4 trust review, US5 schema verification,
US6 crypto/transport documentation and US7 provider-policy review. Separate implementation
writers require separate worktrees. No deployment follows merely from an increment passing.

## Evidence so far

- T001/T002: owner-confirmed answers reflected in spec/constitution; review corrected processing
  retention anchor and explicit WSL platform acceptance. Requirements checklist 16/16.
- T003: clean baseline c9a9e585 in isolated worktree, pinned `npm ci` exit 0, zero reported
  vulnerabilities; current GitNexus index built with `--skip-skills --skip-agents-md`. Direct
  callers of `applyObservations` include provider/fallback application and apply/CLI-memory tests.
  Index omits some whole execution flows; source searches remain authoritative.
- T006-T015: US1 source lifecycle is implemented and verified by the increment-A receipts in
  `quickstart.md`: each Node 22.16.0/24.16.0 suite passes 923 + 202 tests, and the packed CLI passes.
  Native and available CLI review findings were triaged and fixed; Grok quota remains unavailable.
  This marker covers the isolated source-lifecycle increment, not the later real-model/product gates.
- T004/T005: independent spec/plan review returned no material blockers after fixes. Source
  review selected row reattachment with retained attempt membership, per-source destination
  rechecking, delayed retries, legacy-unknown holding and evidence-safe purge. Public seams are
  capture/worker CLI behavior and the documented SQLite migration contract; no new seam approval
  is required by the already-approved specification.
- T016/T017: B1 source/contract review closed all four confirmed defects after repair. See
  `quickstart.md` for 154 focused tests, schema-4-to-5 preservation/fencing, source selection,
  old-spool collision and late-recovery evidence. CLI/MCP work selection is also tested, but
  T018-T020 remain open until work checkpoint production and all read/injection paths are done.
- T018/T019: B2-B5 implement and verify checkpoint production, all read/injection paths, explicit
  work selection, removed/moved/recreated worktrees, native lineage and inherited generation
  privacy. Both supported Node versions pass 1,014 + 202 tests; see the B5 receipts in `quickstart.md`.
  T020 still requires actual native-agent runs. Security review and regression repairs are recorded,
  but the terminal report finalizer rejected an evidence path; report packaging remains a T043
  follow-up and no finalized security report is claimed.
- T021/T022: C2 passes 25 focused tests on Node 22.16.0 and 24.16.0, plus typecheck/lint and
  Standards/Spec/Ponytail review. The 1,051-event isolated no-model replay completed with all
  1,143 hook calls exiting 0 and every lifecycle check passing; all 40 facts are accounted for as
  captured but awaiting generation, with answer evaluation explicitly not run. C3's repeat removes
  the ineligible native-fork timing sample and passes worker RSS at 106.1 MiB. Ordinary hook timing,
  two unprinted Grok starts and real-model recall remain failed/unqualified, not reclassified as
  successful evaluation. See `quickstart.md`; T023/T024/T040-T043 stay open.
- T025-T028: D1 implements explicit work/project grants, exact personal proposals/projections,
  common source/visibility checks, and CLI/viewer approval plus work-preserving adoption. Both Node
  versions pass 1,097 + 202 checks; installed-browser actions, package validation, normal security,
  Standards/Spec and Ponytail review pass. See the D1 receipts in `quickstart.md`. This does not claim
  native transfer/sync, real-agent/model qualification, or the unfinished resident/failover amendment.
- T031/T032 (partial E2): `import promote` now creates/reuses a pending inferred proposal from a
  clean import-created candidate and explicit `--work <local-work-id>`, preserving local terminal
  decisions; `--list` discovers bounded cwd-repository receipt metadata through the same predicate.
  At HEAD `590c0a2f` plus this follow-up, all 69 promotion tests pass on Node 24.16.0 and 22.16.0;
  typecheck/lint/build pass. The requested migration/scope/transfer/CLI glob reports 6/8 passing
  file suites per Node; detail runs show 153 PASS / 4 FAIL, confined to unchanged CLI pipe/FIFO
  sandbox failures. Six review findings are closed with RED/GREEN receipts in `quickstart.md` E2
  (`us5-promote2-*`); the existing status/approve test covers the dead-proposal concern without a
  redundant provenance guard. The parent previously passed the baseline whole gate (`us5-e2-*`:
  1,181 unit/migration/scripts + 202 serial E2E/fault per Node, plus pack-check) and then passed the
  whole gate again on this follow-up (`us5-e2b-*`: 1,203 + 202 per Node, pack-check 20.703 MB).
  Earlier `us5-promote-*` receipts remain historical evidence.
  T029-T032 stay unchecked: the remaining matrix, RSS/packed checks and full reviews remain open.
- T043/T031 (E4 security, this commit): the US5 security review converged after the initial
  architecture/g1-g3 Codex pass, eight fresh Codex follow-up rounds and the `code-review` finder
  set; eight security fixes plus one error-code fix in `transfer-merge.ts`/`transfer-plan.ts`/
  `transfer-promote.ts` (0007 unchanged) with 16 authority cases, one integrity case and an updated
  matrix case RED→GREEN; final tree gate `us5-sec12-*` green on both Nodes with nothing else
  running. See `quickstart.md` E4.
- T032/T043 (E5 wall time and RSS, `97bbe882`): the scratch merge runs in one transaction with a
  per-database prepared-statement cache; near-limit import 3,228 s → 108 s apply, 2,987 s → 28 s
  preview, every packed-CLI run below the 512 MiB import/export budget the contract now states
  (largest 374,544 KiB), counts and effects identical to the E1 baseline. Gate `us5-perf1-*` green.
  See `quickstart.md` E5. T029-T032/T043 stay unchecked pending the macOS probe and the final
  review pass.
- T034–T036 (US6, `4317d3ea`…`4b426a1c`): the owner chose "implement per the contract" on
  2026-09-11. Schema 0008 (`sync_spaces`, `sync_cursors`, `sync_origins`, `sync_revisions` +
  parents, `sync_repo_mappings`, `sync_approvals`), `src/sync/` (identity, envelope, format,
  store, capture, publish, stage, apply, space, status), `oboete sync` (init/join/key show/
  push/pull/status/resolve/map-repo/leave, exit codes 0–4), MCP `sync_status` (read-only),
  `oboete doctor` `sync` item, and the local approval record written by every approval. Tests:
  `test/unit/sync*.test.ts` (seven files, 124 cases; the bounds file gates its 256 MiB push and
  180,000-line graphs behind `OBOETE_SYNC_HEAVY=1`; `sync-review.test.ts` pins every review
  finding). Writing the verification list found and fixed seven
  apply/publish defects (phantom work successor, resolution successor chain check, checkpoint
  tombstone on arrival, alias resolve payload id, context promoted past the closure, late-child
  raise, approval bound to candidate only) and one CLI input gap (unknown class names). Three
  contract sentences were amended and recorded in the contract's "Implementation notes"
  (`revisions_sha256` scope, `sync.key_id` allow-list, `status.ts` as the one module doctor and
  MCP import) plus the `--republish` delivery-identity bullet. Review rounds (`/code-review high`
  three times plus one finder angle, Codex correctness passes, receipts under
  `/var/tmp/oboete-009-20260909.jJ5grc/us6/`) found 10 + 6 + 9 + 9 + 14 + 11 defects, each fixed in
  security-owned code by Claude Code and pinned RED→GREEN in `sync-review.test.ts`; round four
  replaced the content-derived source identity with a stored key (`memory_sources.sync_key`) and
  made write-time aliases re-enter the pass; round five closed the holes that redesign opened
  (bound-row lookups, tuple matching, claim-before-write); round six settled the source key as
  a device-local name with the memory back in the natural key, and made tuple collisions wait
  instead of deleting other origins' rows. The contract's "Implementation notes" record the
  rules per round. 0008 was edited in place: a
  database that applied an earlier 0008 of this branch must be recreated (nothing released).
  The full gate is recorded below when it completes.
- T033 (US6 contract, this commit): `contracts/sync.md` and research R8 record the owner's
  2026-09-11 transport decision (encrypted bundle files, Node `crypto` only, no dependency) as a
  revision-log contract: envelope, identity/delivery split, control revisions, natural-key
  aliasing, push staging fence, two-phase apply, change capture against materialized state.
  Eight Codex contract reviews were folded in; round 8's four findings are folded in unconfirmed
  (see the contract's "Review status"). Unchecked: the owner confirms scope at this checkpoint
  before T034 starts, and T034 re-reviews the contract first.
- T032 (E3 matrix, `943660a4` plus worktree): added ten focused `migration-matrix.test.ts` cases
  for preview/context/schema/WAL, mapping rollback/collisions, proposal/terminal/personal round trips
  and actual packed CLI metadata. All ten pass on Node 24.16.0 and 22.16.0. Two preview-accounting
  defects were repaired with RED evidence (`us5-matrix-a2-red.tap`, `us5-matrix-b6-duplicate-red.tap`);
  nine existing-guard mutations also have preserved RED/restoration receipts. Typecheck/lint/build
  pass; focused suites are 159 PASS / 1 unchanged FIFO `EPERM` failure per Node. Full Node 24
  unit/migration/scripts is 73 PASS / 11 FAIL file suites; explicit-file diagnostics are 116 PASS /
  27 FAIL, including subprocess/socket limitations and a 2-second timeout. `quickstart.md` E3 records
  commands, the discarded glob-based diagnostic, review closures and the existing context-candidate
  listing contract mismatch. No checkbox is ticked; RSS/races/full-US5 qualification remain open.
  Follow-up (`us5-matrix2-*`) closes all four review findings: bounded context candidate IDs and
  omission counts (closing that mismatch), the single human unresolved total, missing-destination
  apply refusal coverage, and the shared output helper in all five tests. A1/A2 have RED evidence;
  all ten matrix cases are GREEN on both Nodes. Typecheck/lint/build pass; requested focused runs
  report 7 PASS / 1 FAIL file suites per Node, with case diagnostics 159 PASS / 1 unchanged FIFO
  `EPERM` failure per Node. E3 records the receipts and reviews. The parent's whole gate (`us5-e3-*`)
  passes 1,213 unit/migration/scripts on each Node, serial E2E/fault 202/202 on Node 22.16.0 and
  pack-check; the Node 24.16.0 serial run hit the documented load-only seed miss (201/202) while the
  resource measurement ran concurrently and is rerun in isolation afterwards.
