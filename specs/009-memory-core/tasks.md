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
- [X] T020 [US2] Verify removed worktrees and ordered cross-agent continuation: all twelve ordered agent pairs over an interleaved parallel-work corpus, and search and explicit continuation after `git worktree remove`, in `test/unit/work-pairs.test.ts`; record runs in `specs/009-memory-core/quickstart.md`. Native runs through `scripts/e2e/isolated-user.mjs` and `scripts/e2e/probe-lib/isolated-lifecycle*.mjs` are a separate acceptance gate (`contracts/work.md` B3) tracked in #265.

## Phase 5: US3 — Useful current knowledge (P1)

Independent test: Japanese/English paraphrases, superseded facts and already-delivered facts
produce separately scored retention, retrieval, delivery and answer outcomes.

- [X] T021 [US3] Correct readiness/lease barriers and prior-delivery accounting in `src/fixture/replay.ts` and `src/fixture/replay-evaluate.ts`, with a focused `test/unit/replay-evaluate.test.ts` regression.
- [X] T022 [US3] Add source-stage accounting and inspectable omission reasons in `src/fixture/replay-evaluate.ts`, `src/why.ts` and `src/fixture/replay-report.ts`.
- [ ] T023 [US3] Reproduce and correct demonstrated lexical/MMR/supersession misses in `src/retrieval/rank.ts`,
  `src/db/queries.ts` and `test/unit/retrieval.test.ts`. Status: none reproduce on the `events-1000` corpus
  (the no-model replay stops every fact before ranking; stored verbatim, all 40 fixture facts rank within the
  first five), pinned in `test/unit/retrieval.test.ts`; the MMR depth observation is #272. The open miss is the
  small-corpus threshold drop #275, reproduced from the `2026-09-17T15-05-08-894Z` dogfood run (JST
  2026-09-18) and carried as a skipped
  five-row artifact in the same file. Acceptance: #275 fixed; that artifact un-skipped and passing; the
  fixture pins and the threshold mutation still green; plus three pins the artifact alone does not give:
  a small-corpus false-positive case (an unrelated memory in a five-row corpus stays omitted), the same
  five rows through `buildPromptPack` (the pack path is where the dogfood run dropped the row, and it
  adds delivery filtering, retirement and the budget cut on top of the shared ranker), and evidence that
  the rescued row arrives through the trigram index rather than the LIKE fallback, which `applyThreshold`
  admits without comparing it to the threshold.
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

- [X] T029 [US5] Pin supported claude-mem/CMEM export schemas and migration mappings in `specs/009-memory-core/contracts/migration.md` using primary sources and synthetic fixtures.
- [X] T030 [US5] Extend versioned Oboete transfer with scope/provenance and the old reader in `src/transfer.ts` and `test/unit/transfer.test.ts`.
- [X] T031 [US5] Add a read-only migration adapter with dry-run/explicit mapping and classification quarantine in migration operations and `src/worker/imported.ts`.
- [X] T032 [US5] Verify source immutability, identity collisions, tombstones, repetition and historical work in `test/unit/migration-import.test.ts` and packed CLI checks.

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

- [X] T037 [US7] Test mode choice, limit exhaustion and consent changes in `test/unit/providers.test.ts` and `test/unit/setup.test.ts`.
- [X] T038 [US7] Complete explicit cost-policy setup and reservation handling in `src/config.ts`, `src/setup/`, `src/observer/reservation.ts` and `src/doctor/provider.ts`.
- [X] T039 [US7] Verify no-model capture-only behavior and real chosen profiles through `src/doctor.ts`, packed CLI and `specs/009-memory-core/quickstart.md` evidence.

T037-T039 and T048 share one contract, `contracts/provider-fallback.md`, and one branch.

Evidence for all four is `quickstart.md` section E9 and its fifteen numbered items, against the
contract's verification list. T037 is `provider-fallback.test.ts` plus the two setup tests named
there; T039 is the two doctor tests plus the packed-CLI gates in the same section. T038 needed **no
new accounting**: `provider_usage.exhausted_at` is already per-preset and `DAILY_CAP` is already
summed over the capped presets, so each target refuses at its own reservation with no write. It did
need two corrections in `src/observer/reservation.ts`, both recorded in E9's follow-up: the
per-preset stamp is now one exported reader (`presetExhaustedAt`) instead of a day-wide flag every
single-preset caller could misread, and a reservation restamps `claimed_at` so the reclaim timer
runs from the attempt rather than from batch creation. The cost policy is `observer.cost_policy` in
`src/config.ts` with `admittedChain` as its one reader, and `fallbackItems` in
`src/doctor/provider.ts` reports each target's verdict.

Owner amendment, 2026-09-10:

- [X] T046 Record the approved resident-worker option and configured model/provider failover in `CONSTITUTION.md`, the local Spec Kit constitution, `specs/009-memory-core/spec.md` and `plan.md`.
- [X] T047 [US1] Implement and verify resident waiting for new/due work, one owner through idle/active epochs, pause/stop/config changes and upgrade/crash recovery in `src/worker/`, capture startup and operator controls; retain bounded one-shot observe and prove idle/long-run resources.
- [X] T048 [US7] Implement a bounded, consented model/provider fallback chain after free-tier/API failures in provider selection, reservations, setup/config and worker processing; verify free-only admission, shared quota versus target failure, per-attempt source eligibility and all-targets-failed retention.

## Phase 10: Completed-product verification

- [X] T040 Align accepted native capabilities and run actual Linux/WSL/macOS checks in `.github/workflows/` and `scripts/e2e/probes/`; preserve unsupported/unavailable verdicts (macOS agent probes stay unverified: #269).
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
  T020 then still required actual native-agent runs (closed 2026-09-17 on synthetic coverage; native
  runs are #265). Security review and regression repairs are recorded,
  but the terminal report finalizer rejected an evidence path; report packaging remains a T043
  follow-up and no finalized security report is claimed.
- T021/T022: C2 passes 25 focused tests on Node 22.16.0 and 24.16.0, plus typecheck/lint and
  Standards/Spec/Ponytail review. The 1,051-event isolated no-model replay completed with all
  1,143 hook calls exiting 0 and every lifecycle check passing; all 40 facts are accounted for as
  captured but awaiting generation, with answer evaluation explicitly not run. C3's repeat removes
  the ineligible native-fork timing sample and passes worker RSS at 106.1 MiB. Ordinary hook timing,
  two unprinted Grok starts and real-model recall remain failed/unqualified, not reclassified as
  successful evaluation. See `quickstart.md`. As of C3 (2026-09-10) T023/T024/T040-T043 were all open. T040 has since
  closed on the macOS engine evidence (E10). T023 closed on the no-model `events-1000` replay
  (E12) and reopened on 2026-09-18 for #275, which the first 009 dogfood run,
  `2026-09-17T15-05-08-894Z` (E13), produced.
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
  T029-T032 stayed unchecked here: the remaining matrix, RSS/packed checks and full reviews were open.
  Amended 2026-09-14: E3 closed the matrix, E4 the security review and E5 the RSS/packed checks, so the
  four markers are now checked with the E7 receipts below.
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
  See `quickstart.md` E5. Amended 2026-09-14: the macOS probe is T040's product and the cohesive
  gate is T043's, so neither belongs to T029-T032; those four are checked with the E7 receipts and
  T043 stays open.
- T034–T036 (US6, `4317d3ea`…`4b426a1c`): the owner chose "implement per the contract" on
  2026-09-11. Schema 0008 (`sync_spaces`, `sync_cursors`, `sync_origins`, `sync_revisions` +
  parents, `sync_repo_mappings`, `sync_approvals`), `src/sync/` (identity, envelope, format,
  store, capture, publish, stage, apply, space, status), `oboete sync` (init/join/key show/
  push/pull/status/resolve/map-repo/leave, exit codes 0–4), MCP `sync_status` (read-only),
  `oboete doctor` `sync` item, and the local approval record written by every approval. Tests:
  `test/unit/sync*.test.ts` (seven files, 180 cases; the bounds file gates its 256 MiB push and
  180,000-line graphs behind `OBOETE_SYNC_HEAVY=1`; `sync-review.test.ts` pins every review
  finding). Writing the verification list found and fixed seven
  apply/publish defects (phantom work successor, resolution successor chain check, checkpoint
  tombstone on arrival, alias resolve payload id, context promoted past the closure, late-child
  raise, approval bound to candidate only) and one CLI input gap (unknown class names). Three
  contract sentences were amended and recorded in the contract's "Implementation notes"
  (`revisions_sha256` scope, `sync.key_id` allow-list, `status.ts` as the one module doctor and
  MCP import) plus the `--republish` delivery-identity bullet. Review rounds (`/code-review high`
  three times plus one finder angle, Codex correctness passes, receipts under
  `/var/tmp/oboete-009-20260909.jJ5grc/us6/`) found 10 + 6 + 9 + 9 + 14 + 11 + 17 + 16 + 20 + 12 + 4 + 3 + 18 + 7 defects, fixed in
  security-owned code by Claude Code and pinned RED→GREEN in `sync-review.test.ts` (round eleven
  fixed and pinned three of its four; its fourth, a mutual cross-memory context cycle held on one
  device, is not resolved — the cycle-break is order-dependent and two devices can diverge, tracked
  as a follow-up design change, issue #196; round twelve fixed and pinned one of three — a
  `storePayload` tuple-backfill drift — and tracked the two narrower source-identity edges as
  issue #197); round thirteen triaged the PR bot findings — eight adversarial-hardening defects in
  `src/sync/` plus ten found while closing them — a repo-line bound that regressed the legal
  boundary, the consent tuple the CLI never showed, unbounded `status` listings, the repo bound
  missing on the publishing side, `leave` wedging on a failed config write, the `init` singleton
  race, a peer-plantable temporary name turning a published push into a reported failure, the
  apply pass materializing every staged origin, and a descriptor leaked by each of the seven
  header rejections — all fixed and pinned, with the three re-surfaced convergence findings left
  on #196/#197 and the non-sync findings filed as #199/#200/#201/#202); round fourteen triaged the
  bot findings on that head — five, of which two were regressions round thirteen introduced in the
  same compensating-write design (`recordSpace`'s rollback deleted the config the winner of an
  `init` race had just written; `leave`'s single transaction left the config deleted when the
  COMMIT failed), plus a `remote:` key forged over a local `common_dir` path this device already
  holds, a repo line whose declared kind disagreed with its key prefix, and `--classes` accepted
  and silently ignored outside `init`/`join` — all four fixed and pinned; the `/code-review`
  pass on that fix then found the forged-key hole was only half closed — where the device does not
  yet hold the identity, the peer's line creates the row under the very id the device will compute
  when the developer opens that path, so `applyRepoLines` now requires a canonical remote identity,
  checks every key's hash whichever replica minted it, and `registerLocalRepos` records the kind its
  key was built from. That third pass also settled what the
  three passes could not: a machine-local repository key is not a boundary at all — a peer knows
  this replica's id and this device's paths because `publish` emits both, and the mapping binds on
  the developer's next open — so the contract now states the space key as the boundary and the
  consent gate is issue #205, with the publish-side RSS measurement as #204); round four
  replaced the content-derived source identity with a stored key (`memory_sources.sync_key`) and
  made write-time aliases re-enter the pass; round five closed the holes that redesign opened
  (bound-row lookups, tuple matching, claim-before-write); round six settled the source key as
  a device-local name with the memory back in the natural key, and made tuple collisions wait
  instead of deleting other origins' rows; round seven made the key random (sameness across
  devices only through the UNIQUE tuples) and replaced the wait with parking the row before the
  pass; round eight returned to a content-derived key with a revivable deletion, carried the
  tuple and lineage as sameness, and closed the parking holes; round nine closed that closure
  (per-memory key in the writer, retirement frontier, tuple per revision, held rows outside the
  pass, lineage join after store, deterministic source selection, rows set aside instead of
  merged into a taker); round ten closed round nine (tuple filled with a withheld payload,
  lineage join only within one local memory, terminal deletions reaching every holder through a
  post-pass sweep, resolutions carrying the kept head's tuple, retirement rebinding before union,
  and the transient park held only within a pass). The contract's "Implementation notes" record the
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
- T042 (hook cold start, issue #210): the capture hook's median moved 187.3 ms → 222.2 ms when US6
  merged. `dist/` is now `src/launcher.mjs` copied to `dist/oboete.mjs` plus the bundle at
  `dist/engine.mjs`, so the V8 compile cache is enabled before the bundle is compiled; interleaved
  measurement puts the launcher build back at the pre-US6 baseline (184.5/186.9/192.6 ms against
  218.9/217.1/214.8 ms). The `/code-review` pass on the first version found the split had also split
  what "the bundle" means and that the installer had followed the wrong half — `oboete setup` wrote
  `dist/engine.mjs` into every hook command, so the fix reached no install — plus a cache directory
  trusted rather than checked after `mkdirSync`; both are fixed and pinned, and the six call sites
  that name one of the two files are now deliberate. Two further review rounds found that the
  launcher had no eslint rules at all (a `.mjs` under `src/` matched neither `files` list) and that
  checking the versioned directories V8 writes inside the cache disabled the cache from the second
  run onward wherever the umask is 002 -- silently, with every test still green; the check is now on
  the cache directory's own traverse bits and the pin corrupts an entry to tell a live cache from a
  dead one. Two rounds after that found a symlink planted at the `oboete` directory: recursive
  `mkdir` follows one, so the launcher would have created `compile` inside somebody else's tree and
  written half a megabyte of bytecode there while every check on `compile` itself passed -- the
  parent is checked before `compile` is created now -- and a named import of `enableCompileCache`,
  which is resolved when the module is linked, before any statement runs and outside the reach of
  the surrounding `try`, so on a Node older than 22.1 it was a `SyntaxError` and a non-zero exit for
  every command including the hook contracted to exit 0 whatever happens; it is a namespace import
  and an optional call now. A last round moved the cache itself: `$XDG_CACHE_HOME/oboete/compile`
  writes outside the tree `OBOETE_HOME` bounds, which `CONSTITUTION.md` Principle VI does not allow
  and explicitly defers, so it is `$OBOETE_HOME/cache/compile` and the launcher resolves the home the
  way `src/paths.ts` does. That left every test spawning on a cold cache -- 6 % on the fault suites
  locally, and on CI a jump from a 215 ms median to 246 ms over 48 hook invocations that failed
  `fault-storage` and `e2e-hook` on both runs -- so both `node --test` runs load
  `test/helpers/compile-cache.ts` with `--import` and every test and every CLI it spawns shares one
  `NODE_COMPILE_CACHE`, which puts the suites back at 41.8 s. Wiring three spawn sites instead of
  the runner was not enough: the unit batch is what leaves the cache warm for the timed suites, and
  without it the first serial spawn still spent its whole budget compiling. The medians were re-measured interleaved and still show the same ~36 ms. The same round wrapped the launcher's own `import` of the engine: the split put a
  failure ahead of the handler in `src/cli.ts` that gives `hook`, `capture` and `inject` their
  contracted exit 0, so a missing engine printed a Node stack over an agent's session; the launcher
  now repeats that handler for those three commands and rethrows for every other. Cache directory, the rejected alternatives, the
  accepted `NODE_COMPILE_CACHE` and cache-growth costs and what the numbers do not claim are in
  `contracts/injection-performance.md`; the pin is `test/unit/launcher.test.ts`. T042 stays
  unchecked: the 1,000/10,000/100,000-event measurement it names is still open.
- T029-T032 (E7 close, `35c1d9d4`): the US5 increment is complete and the four markers are checked.
  `contracts/migration.md` pins `oboete-export/1`, `oboete-export/2` and
  `claude-mem-query-export@8bc631a` against the upstream exporter, store queries, types and public
  documentation at v13.24.5 (T029). `src/transfer.ts` and its `transfer-*.ts` modules carry scope
  and provenance and still read v1 (T030). The read-only adapter previews by default for native v2 and
  claude-mem input — `oboete-export/1` keeps its documented implicit apply, and says so in its own
  output — takes the mapping flags of its own format (`--map-repo`/`--map-work` for native input, `--map-project`/
  `--map-project-hash` for `--from claude-mem`, each rejected for the other), never opens a source
  with SQLite, and quarantines every memory it newly inserts as `imported`/`local_only` until the
  worker classifies it in `src/worker/imported.ts`; a row that matches an existing memory keeps
  that memory's local review state and scope and is recorded as `matched_existing`, and a tombstone
  or held historical record is never rewritten to `imported` (T031). The matrix is the seven
  `test/unit/migration-*.test.ts` files plus `test/unit/transfer.test.ts`: 142 checks pass on both
  Node 24.16.0 and 22.16.0, with typecheck, lint and pack-check (20.879 MB installed) exit 0. The
  packed-CLI half of T032 ran with them at three levels: `matrix D10` alone spawns the built
  `dist/oboete.mjs` (previews and promotion, not export or an applied import), `pack-check` installs
  the tarball and calls `--version`, and a preview through the installed package is recorded in
  `us5-close-installed-import.log` (exit 2 on unresolved project
  mappings, `applyPossible: false`, fixture bytes unchanged). Applying through an installed package
  and the near-limit measurement stay E5's recorded result at `97bbe882`, whose evidence bundle
  survives at `/var/tmp/oboete-009-20260909.jJ5grc/us5-rss3/` (T032). Receipts `us5-close-*` under `/var/tmp/oboete-009-us5close/`; see `quickstart.md` E7. The
  macOS platform probe remains T040's (its macOS leg deferred by the owner) and the cohesive
  product gate remains T043's.
- T048: the bounded, consented fallback chain is implemented and verified against
  `contracts/provider-fallback.md`, whose nineteen verification items are mapped to tests in the E9
  receipts in `quickstart.md`. Both supported Node versions pass the full `npm test`. This marker
  covers the five admission rules and their `ProviderConfigError`s, the `chain` field in
  `consentTuple` and its effect on `consentHash` (unchanged for an install with no admitted chain),
  the attempt sequence — stop sentinel, consent, a reservation for **every** target whether capped
  or not, then the call — the advance-and-stop table, the single `applyFallback` that keeps every
  source retryable when all targets fail, and the diagnostics: one observe-log line per target that
  failed, and a static per-target report in `oboete doctor` that spends no allowance.
  Two corrections it carries beyond the chain itself, because the chain depends on them: `claimed_at`
  is restamped at the reservation rather than only at batch creation, without which the 120 s
  `reclaimStale` fence was already spent for every preset; and `presetExhaustedAt` is the single
  reader of a per-preset stamp that four callers had been reading as a day-wide flag.
  It does not cover the real agent pairs and real-model evaluations (T041), the resource sweep and
  the seven-day soak (T042), the macOS platform leg (T040) or the cohesive product gate (T043).
  Issues recorded against paragraphs that place them outside this task: #240 (doctor reports
  allowance the worker will not grant between 140 and 150 calls), #241 (`agent-cli` requires an
  `[observer] model` that nothing sends), #242 (`claimed_at DESC` is no longer settle order for two
  display queries, which the restamp above is what changed) and #243 (an inject test that flaked
  once on a duplicate CI run).
- T047: the resident observation worker is implemented and verified against
  `contracts/resident-worker.md`, whose sixteen verification items are mapped to tests in the E8
  receipts in `quickstart.md`. Both supported Node versions pass the full `npm test`. Two controls
  were also confirmed on a real process against a replayed corpus (`signal` and `upgraded`, each
  exit 0 with the lease released), and the idle poll cost was measured in a window containing no
  epoch. This marker covers the resident lifecycle, its controls, capture startup and the retained
  one-shot behaviour — including the two corrections the one-shot run shares with the resident: an
  exact `recovered` count when a spool recovery meets a busy database, a lease-ownership probe
  that stays inside the guard that records a storage failure, and a capture startup that spawns the
  migrating worker for a version-zero database instead of spooling against it forever. It does not cover the resource sweep and the soak, which item 12 of the
  contract assigns to T042, nor the macOS leg, which is T040's and stays deferred; issues #231,
  #233 and #234 are recorded against the contract paragraphs that place them outside this task —
  a pre-existing pass-loop defect the resident inherits, two clock-and-retention findings from the
  delta reviews, and the post-release spawn hand-off the unconditional release leaves open.
- T040 (open, 2026-09-17): the macOS leg now runs on a GitHub-hosted `macos-15` runner through
  `.github/workflows/platform.yml` instead of the M1 iMac. Four runs are recorded in E10 of
  `quickstart.md`: they found a fail-open `secret_paths` defect through symbolic links (fixed in
  #254), measure the hook inside its budget on macOS, and leave one deterministic defect — the
  database busy wait is not a wall-clock bound (#255) — plus five one-off timing failures (#256).
  The marker stays open until the #255 fix passes on the runner. Agent probes on macOS are recorded
  as unverified: a hosted runner has no agent login.
- T040 (closed 2026-09-17): #262 fixed #255 (merged as `84ba32ff`). Run 35224724418, on #262's head
  merged onto `main`, passes unit and serial on both runner legs; the M1 iMac, available at any time
  by the owner's decision of 2026-09-17, passes every `platform.yml` step on `main` `84ba32ff` on
  Node 22.16.0 and 24.21.0 (E10). The runner's hook cold start still fails on its timer spread and is
  recorded as such; macOS agent probes stay unverified (#269).
- T020 (closed 2026-09-17): `test/unit/work-pairs.test.ts` runs all twelve ordered pairs of claude,
  codex, grok and pi over one parallel-work corpus each (two linked worktrees, two purposes in one
  of them, a third work in the other, checkpoints generated through the observer with a mocked
  provider). The receiver's ambiguous start lists only the two candidates of its worktree and
  delivers no checkpoint; after an explicit choice its pack and delivery ledger carry only the
  selected work's checkpoint, and the siblings' checkpoints, work-scoped observations and pending
  prompts are withheld. A removed worktree's work stays searchable with its provenance and is
  delivered on explicit continuation without recreating the directory. Five product mutations each
  fail all thirteen tests; see E11 in `quickstart.md`. `contracts/work.md` B3 keeps real agents a
  separate acceptance gate, so the native harness runs (`isolated-user.mjs`,
  `isolated-lifecycle*.mjs`) are #265; the harness's own tests run in `npm test`. The daily
  dogfood's twelve pairs (#244) ran the M1 bundle (schema 3) until the 2026-09-17 move to the 009
  bundle (quickstart E13) and check fact recall only, so they are not SC-002 evidence.
- T023 (open, 2026-09-18): an isolated no-model replay of `events-1000.jsonl` on `main` `6b683213`
  stops all 40 tagged facts at coverage (`pending`, `no_range`) with application deferred, so none
  reaches retention or retrieval; that is the no-model design of research.md R6, not a ranking miss.
  Stored verbatim as memories, all 40 rank within the first five for their own queries through
  `searchMemories`, 39 of them first. Nothing reproduced on that corpus.
  `test/unit/retrieval.test.ts` pins the corpus, age-neutral ranking, supersession (hidden by
  default, marked historical in `get --history`) and a shared-title pair, and pins the artifact's
  prompts against the probe library; each of nine mutations fails its test (E12). The MMR rule that drops distinct but similar facts deep in a candidate list is
  recorded as an observation without a failing corpus case in #272, and lambda is unchanged. The
  first daily run on the 009 bundle then reproduced a miss the fixture cannot: in a five-memory
  corpus FTS5 clamps the IDF of common trigrams, the ratio-to-best normalization drops every other
  candidate below 0.3, and a fact-bearing memory is omitted from the prompt pack (#275, E12
  Limits). T023 stays open for that fix.
- T024 (open, 2026-09-18): no local or external profile was qualified in 009, because activating a
  real model is not authorised (handoff of 2026-09-10). The no-model replay gives no generated facts
  to measure, and verbatim facts are all found lexically, so the measurement that would justify
  semantic retrieval does not exist yet. Qualification and that decision are #266; search stays
  lexical, with semantic search in M2 (`LEXICAL_NOTE` in `src/memories-cli.ts`).
- T041 (open, 2026-09-18): the twelve native ordered pairs on the 009 bundle are #265 and real-model
  Japanese and English evaluation is #266. The daily dogfood install moved from the schema 3 bundle
  to the 009 bundle on 2026-09-17 (E13), so its runs from 2026-09-18 exercise the 009 bundle; they
  still check fact recall only.
