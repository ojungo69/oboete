# Validation guide

Use `/home/jura/projects/free-mem-wt/009-memory-core`, temporary stores and synthetic content.
Keep the daily installation, original claude-mem store and cloud destinations untouched.

## Local checks

```sh
npm ci
npm run typecheck
npm run lint
npm run build
node --test --enable-source-maps build/test/unit/memory-recovery.test.mjs
node --test --enable-source-maps build/test/migrations/memory-processing.test.mjs
```

The new recovery and migration tests are implemented and runnable. Run focused
worker/request/apply/purge tests while changing those paths, then `npm test` and
`npm run pack-check` once for the cohesive increment.

## Required scenarios

1. Capture known facts, fail the provider, advance past old expiry, run retention, restore the
   selected provider and retry. Sources remain; expected memories are retrievable; repeated
   recovery creates no duplicate effects.
2. Place unique facts throughout oversized material, crash between bounded requests and resume.
   Unsent portions remain pending; final source-range accounting covers accepted material.
3. Exercise empty output, explicit noop, detector rejection, foreign source ID and lost lease.
   Inspect fixed outcomes without storing model bodies or secret values.
4. Advance 30 days after successful processing. Ordinary full activity may expire; pending
   sources and important evidence remain.
5. Later: interleaved work, ambiguous selection, adoption without completion, personal proposals,
   repeated import, disconnected replica conflicts and deletion propagation.

## Runtime completion

Use selected isolated logins and selected reference models. A configured CLI or hook is not a
successful model run. No Mac target or installed Ollama model was verified on this host.
Do not force a live lease or infer readiness from a fixed sleep. Use persisted processing
barriers, and score capture/coverage/application/retrieval/delivery/answers separately.

Record diff/engine hash, versions, exits and stage counts in sanitized evidence. Full completion
requires all [success criteria](spec.md#success-criteria), including actual Mac, twelve ordered
pairs and seven real-use days. Unit tests do not substitute for these conditions.

## Increment A evidence — 2026-09-10

The uncommitted `009-memory-core` diff is based on `c9a9e585`; the daily installation and
`008-qd-d` checkout remain untouched. Logs are in
`/var/tmp/oboete-009-20260909.jJ5grc/` and contain synthetic inputs only.

- Recovery, request paging, thirty-day evidence retention, migration lease fencing and temporal
  update/delete guards were reproduced before their fixes. Migration 0001–0003 are unchanged.
- `a4-privacy-red.tap`: five failing cases for cached eligible/private reclassification, tool
  input/repository rules, detector failure and explicit processing of one active turn.
  `a4-privacy.tap`: the same five cases pass after repair.
- `a4-summary-red.tap`: the summary allocated 6,408,190 bytes in one body query; the new
  aggregation passes the bounded-allocation regression. A separate concurrent-capture test
  verifies write availability and retry without publishing a stale snapshot (`a4-race.tap`).
- `a4-final.tap`: 85 focused tests pass, including rotated nearby-memory credentials and a
  credential change after detection but before send. Typecheck and ESLint also pass.
- Full-suite, Node 22.16, packed CLI and formal review results will be recorded after completion.
  No real provider, Mac, cross-device or seven-day acceptance result is claimed here.
- `npm-test-a5-node24.log`: 910 first-stage tests and 202 serial E2E/fault tests pass before the
  final source-origin/path corrections. Those corrections have 167 focused passing tests
  (`a5-fence.tap`); final full-suite receipts will replace the intermediate numbers.
- `a5-origin-red.tap` reproduces wrong resolution of `./` paths outside the worker cwd, reused
  checkout identity, and retained secret citation values. `a5-path-red.tap` reproduces an
  unredacted original tool-call path. Shared path resolution/write-back and provenance fencing
  correct these cases. Earlier source-origin RED used a duplicate SessionEnd fixture and is
  not counted as a valid reproduction; the corrected case is included in the focused pass.
- `schema-compat/receipt.json` executes the actual `c9a9e585` database module against schema 4:
  its worker refuses, its old hook can open, and the new hook refuses a later schema. All binaries
  used for activation must therefore be upgraded together; no old-hook refusal is claimed.
- One intermediate Node 22 fault run overlapped a package rebuild and lost `dist/oboete.mjs`.
  It is not accepted as a product regression or final evidence. Build/package mutations and
  runtime tests are serialized for the final run.
- `a6-review.tap`: 92 focused tests pass after review fixes, including nullable capture times,
  retained historical decisions, unavailable source membership and explicit requeue lease loss.
  `a6-final-typecheck.log` and `a6-final-lint.log` pass.
- Cubic found two reproduced defects: a deleted memory could regain source evidence within the
  same response, and a reused old summary could remain retired. Their fixes pass four focused
  regressions (`a6-cubic-fixed.tap`). Two structural suggestions did not justify extra machinery.
  CodeRabbit completed with six suggestions: the lease-loss exit was corrected; five conflicted
  with verified contracts or were unreachable under the existing capture-order guard. Native
  standards/spec review findings were corrected; ponytail review removed two unused wrappers.
  Grok could not review because its usage balance was exhausted (HTTP 402); no result is claimed.
- The final privacy fixture now supplies retained origin for the eligible memory and explicitly
  excludes an otherwise eligible memory without origin (`a6-privacy-fixture.tap`). This corrects
  its pre-provenance expectation without relaxing the send-time policy.
- `a6-final-node22.log` and `a6-final-node24.log`: each passes 923 unit/migration/script tests and
  202 serial E2E/fault tests. Exact runtimes are Node 22.16.0 and 24.16.0. Tests and package builds
  ran sequentially. `a6-final-pack.log` passes installed-size (20.444 MB), install and CLI checks.
  These are isolated synthetic checks, not real-model or whole-product acceptance.
- Formal diff review: `security-us1/report.md` under the same artifact directory is generated and
  sealed for the frozen US1 snapshot `9b28fd67…`, accounting for 22/22 production files with no new
  diff-attributable candidates. Its source snapshot remains unchanged while US2 is implemented.
  The report explicitly leaves the pre-existing learned-title sensitivity behavior outside that
  diff conclusion; checkpoint work must preserve all contributing sensitivities. Live deployment
  and provider state were not inspected; terminal review token usage was unavailable.

## Increment B1 evidence — 2026-09-10

`contracts/work.md` fixes the capture and work-selection boundary before implementation.
Migration 0005 preserves existing source/session IDs, holds sources without work provenance and
adds repository-aware native-session lookup without rebuilding the old foreign-key graph.

- `b1-capture-red.tap`: missing work bindings; `b1-spool-red.tap`: missing late-source membership
  and a closed short purpose that never drained. The fixes preserve separate worktree/purpose
  bindings, native resume, bounded ambiguous choices and per-binding batches.
- `b1-operations-red.tap`: first-declaration orphan work and missing CLI choice operations.
  Current tests cover bootstrap purpose, choice persistence, stale/foreign refusal and explicit
  completion. `work choose-source` resolves one otherwise unbound historical source without
  changing the live selection.
- `b1-collision-red.tap`: two repositories sharing a native ID lost one source; a late spooled
  SessionEnd ended a resumed session. Repository-namespaced event IDs and scoped native lookup
  preserve both; a retained last-capture watermark fences late lifecycle/turn changes.
- Old-spool cross-repository collisions and late-root rollback have regression coverage in
  `work-context.test.ts`. An initial late-span RED used the wrong Bash response shape and is not
  counted as a valid reproduction. The corrected fixture covers the declaration and following
  tool result remaining in the same unresolved historical span.
- `b1-legacy-red.tap` proves an inherited unbound batch made an unwanted provider call. The
  repaired guard holds it with `work_selection_required`, and explicit source selection permits
  exactly the chosen source to process. Direct-SQL fixtures now create a resolved work binding.
- `b1-migration.tap`: 154 tests pass across migrations, work/capture/repository identity,
  batching, worker and recovery. `b1-final-typecheck.log` and `b1-final-lint.log` pass.
  Checkpoint production, injection/MCP integration and the full US2 acceptance remain in progress.
- `schema5-compat/receipt.json` executes the frozen schema-4 module and current schema-5 module
  under Node 22.16.0: both old worker and old hook refuse the new store. The version-4 migration
  regression also preserves every prior migration receipt and verifies foreign keys/integrity.
- `b2-mcp-new-choice-red.tap` reproduces duplicate work creation from a repeated `new` selection
  token. Its fix consumes that token once; `b2-mcp.tap` passes 23 MCP/work tests plus typecheck/lint.
- `b1-native-display-red.tap` reproduces a cross-repository native-ID collision borrowing the
  other session's successful compaction verdict. Actual native IDs are used in diagnostics;
  unresolved collisions are reported unknown. The related run (`b1-native-display.tap`) passes
  24 tests and typecheck. This does not complete the separate replay quality work in T021/T022.
- `b1-pending-source.tap` confirms an operator can assign a complete legacy source still awaiting
  classification; the ordinary detector and send-time admission remain required afterward.
  Read-only review confirms all four B1 findings closed; no additional finding was reported.

## B2-B5: Work checkpoints and all readers

Artifact root remains `/var/tmp/oboete-009-20260909.jJ5grc`. T018/T019 cover local implementation
and synthetic hook/worker verification; they do not complete T020 (closed later, E11) or the
real-agent/product gates.

- Checkpoint generation validates exact admitted sources, immutable parentage and the current
  pointer under the existing lease. Same-content confirmation, conflicts, historical outputs,
  tombstones and unchanged progress retain distinct receipts. Work selection reaches CLI, MCP,
  viewer and all four adapters without using repository-latest session recency as progress.
- Removed/moved worktrees retain original source paths and bounded saved repository rules.
  Filesystem generation distinguishes a recreated directory. Shared output filtering checks the
  current policy, complete unescaped fields and retained paths before returning bodies or labels.
  Deferred Grok receipts preserve the actual printed pack; Pi waits for its exact captured prompt.
- C10 privacy regressions showed generated updates, additions and checkpoints could lose earlier
  input restrictions. Actual admitted context now sets the privacy floor independently of cited
  evidence. Bounded flat proof and direct dependency IDs preserve later restrictions, including
  same-content descendants, cycles and tombstoned ancestors. Privacy-only rows do not become
  citations, source completion, timeline attribution or raw-retention exemptions.
- `b5-context-red.tap` and `b5-descendant-red.tap` preserve the original failures.
  `b5-descendant.tap` passes 66 focused tests, including supplied/withheld checkpoints, current
  sensitivity races, unknown/oversized proof, raw purge and exact prior-context preservation.
- C9's real CLI regression exposed an inline path-rule cost overrun. The existing detector worker
  now bounds expensive input to 200 ms per source and the remaining hook budget. The 1,300 ms test
  remains unchanged; `b5-path-cutoff.tap` and both complete suites pass. Accepted sources survive.
- `b5-all-node24-verified.log` and `b5-all-node22-verified.log` each pass 1,014 unit/migration/script
  tests and 202 serial E2E/fault tests on Node 24.16.0 and 22.16.0 respectively.
  `b5-typecheck-verified.log`, `b5-lint-verified.log` and `b5-pack.log` pass. Installed package size
  is 20.538 MB against the 30 MB limit; the packed CLI reports its expected version.
- The B5 Standards, Spec and Ponytail review files record the final reviewed source. Earlier
  Cubic/CodeRabbit findings were checked and repaired; Grok's quota failure still means no Grok
  result. `b5-generation-security.md` preserves C10's findings and independent repair checks.
- `b5-provenance-cost.json` measures a changed proof through 1,000/10,000/100,000 descendants.
  The largest case takes 7.27 seconds and peaks at 112.05 MiB; 1,000 descendants with 128 KiB proof
  take 1.41 seconds and peak at 96.7 MiB. All final-proof assertions pass within the current worker
  and RSS targets. Repeated confirmation, slower disks and seven-day behavior remain unqualified.
- The 47-file security review snapshot, candidate validations and current-source checks are retained
  under `security-b4/`. Its terminal finalizer rejected
  `coverage.surfaces[0].receiptRefs[3]: expected a file under artifacts/` after an evidence path was
  added outside that required directory. The draft is unsealed and has no generated final report.
  Follow up by placing receipts under `artifacts/`, validating the draft, and finalizing in a later
  response; the skill prohibits retrying completion in the same response. This packaging failure
  does not erase the code review or test evidence, and T043 remains open.

## Increment C2 — evaluation correctness

Receipts under `/var/tmp/oboete-009-20260909.jJ5grc/`:

- `c2-final-node22.tap` and `c2-final-node24.tap`: 25 focused replay/why tests pass on Node
  22.16.0 and 24.16.0. Typecheck/lint and `c2-code-review-standards.md`, `c2-spec-review.md`,
  `c2-ponytail-review.md` pass; the private-memory policy finding has a RED and recorded closure.
- `c2-full-replay.json`: the existing 1,051-event corpus finishes with 1,143 successful hook exits
  and all fork/resume/compact/clear checks passing. Explicit none-model configuration produces no
  provider calls. The 40 facts pass capture but remain pending at coverage/application; retention,
  retrieval and delivery fail, and receiving-agent answers are `not_run`. This is evaluation
  correctness evidence, not real-model recall qualification.
- The initial C2 run exceeded ordinary injection timing and worker RSS (188.1 MiB). Its one
  unclassified start was Claude's explicit fork, which is not an injectable native event; that
  wall time is now included in ordinary hook samples. The invalid first four-event trial used a
  malformed test configuration and produced no report; it is not qualification evidence.

## Increment C3 — measured runtime corrections

- `c3-hook-profile-5tvwcqed/before.json` and `after.json`: the same copied synthetic DB/input
  takes 483.5 → 272.7 ms inside the hook, with Git calls reduced from 36 to 9 (153.9 → 40.3 ms).
  Pack-local policy reuse preserves fresh final and deferred-emission guards; the regression and
  asynchronous policy-change cases are in `work-readers.test.ts`.
- `c2-rss-investigation.md` attributes the dominant retained heap to Secretlint's enabled library
  profiler. Disabling its public timing collector preserves all detection rules. The new direct
  profiler declaration uses the existing 13.0.5 package; no installed lockfile package entry changes,
  and npm audit reports zero vulnerabilities. `c3-rss-red.tap` records 2,240 retained entries before
  the fix; the repeated-detection test now retains none while still detecting the corpus secret.
- `c3-full-replay.json` uses the immutable `c3-replay-bundle` recorded by its SHA-256 file. Worker
  peak falls to 108,596 KiB (106.1 MiB), below 150 MiB. All 1,143 hooks and all lifecycle checks
  pass, with no unclassified eligible start. Ordinary injection remains failed: p99 344.1 ms and
  96.8% of samples within 300 ms. The two unprinted Grok start packs still make the pending-text
  check fail (46/48), and none-model recall remains 0/40. These are unresolved qualification results.
- `c3-two-packs-red.tap` and `c3-exception-red.tap` preserve the Codex combined-response races.
  Current code confirms only surviving output, cancels stale/exceptional first plans, and permits
  retries of caller-cancelled starts without retrying ordinary empty/legacy omissions. The source
  security review and closure are in `c3-security-review.md`; this is separate from the unsealed
  formal report described above. Cohesive C3 checks are recorded as they finish.
- `c3-all-node24.log` and `c3-all-node22.log`: 1,032 unit/migration/script checks plus 202 serial
  E2E/fault checks pass on each supported Node version. `c3-pack.log` installs the 20.559 MB package
  and runs its version command. After Ponytail's identical cancellation pairs were consolidated,
  `c3-ponytail.tap` and `c3-ponytail-node22.tap` pass all 40 affected injection/deferred tests.
  These checks do not replace the unresolved runtime/real-model qualifications above.

## Increment D1 — work, project and personal sharing

- Migration 0006 adds explicit visibility grants and proposal decisions. New facts have no implicit
  audience. Observer application derives work from the accepted batch, rechecks mutable nearby
  inputs inside the lease transaction, and grants only the selected work/project scope. Legacy
  degraded guidance remains work-only when its batch proves that scope; unknown origins stay held.
- Direct personal declarations require one fully processed direct-user source and the exact
  statement. Inferred proposals require human CLI/viewer approval. Personal projections retain only
  the approved title/body and public memory fields, with no source links or origin payload. Approval,
  rejection and project adoption leave work purpose, lifecycle and checkpoint pointers unchanged.
- The common query/read policy now covers work knowledge in both pack lanes and enforces personal
  projection shape through CLI, MCP, timeline and viewer. MCP has read-only sharing status; human
  mutation uses CLI/viewer. Explicit local history/status/why inspection keeps its documented stored
  policy exception. Versioned export/import and device sync remain the later US5/US6 work.
- `d1-inheritance-red.tap` and `d1-inheritance.tap` cover inherited source privacy through nearby
  updates and deterministic session summaries. `d1-adopted-late-red.tap` and `d1-adopted-late.tap`
  cover adopted project knowledge from removed origins with a closed unresolved historical binding.
  Summary source identity, learned-title dependencies and flat context bounds prevent historical
  summaries from becoming unproven project progress.
- Cubic's four findings were reproduced or verified and repaired: exact proposal identity prevents
  normalized-equivalent inferred text from acquiring direct approval (`d1-cubic1-red.tap`);
  source-free purpose labels are withheld and source changes cancel delivery (`d1-choice-red.tap`,
  `d1-choice-grok-red.tap`); pending proposals precede terminal history (`d1-cubic3-red.tap`); public
  types describe the actual personal projection. CodeRabbit's 22 findings were triaged in
  `d1-coderabbit-triage.md`; the two verified defects were the migration description and viewer
  sharing-fetch failure blocking primary data. The other suggestions were not adopted as defects.
- Choice labels keep opaque IDs while independently checking their raw source, both current/origin
  path rules, purpose/source mapping and the final aggregate guard. Five real temporary-worktree
  cases pass in `d1-choice-contexts.tap`: clean, current-only restriction, origin-only restriction,
  removal and path recreation. Placeholder masking stays inside the character limit; the source
  references remain in Grok's deferred guard and in the single prepared pack snapshot.
- `d1-reviewed-node24.log` and `d1-reviewed-node22.log` each pass 1,097 unit/migration/script checks
  plus 202 serial E2E/fault checks. `d1-reviewed-types.log`, `d1-reviewed-lint.log` and
  `d1-contract-lint.log` pass. `d1-pack.log` installs the 20.596 MB package and verifies its version.
- `d1-viewer-failure-red.log` reproduces the primary-data failure. `d1-viewer-qa.json` confirms the
  repair in installed Google Chrome through Playwright, at 1440x1000 and 390x844: exact candidate
  titles/bodies, keyboard approval, rejection, project adoption, personal search, unchanged work,
  no horizontal overflow and no unexpected console/page errors. The sharing-unavailable, desktop
  and mobile screenshots were inspected. All data, homes and server processes were isolated.
- `d1-hook-profile/metrics.json` repeats C3's immutable synthetic input after migration, producing
  the identical 1,080-byte output in 233.7 ms with nine Git calls (37.95 ms). This is one comparable
  profile, not a percentile or real-model qualification; the C3 full-replay failures remain open.
- `d1-security-closure.md`, `d1-code-review-standards-closure.md` and
  `d1-code-review-spec-closure.md` find no remaining required repair in D1. The subsequent
  `d1-ponytail-review.md` finds no unnecessary wrapper, configuration, dependency or duplication.
  This closes T025-T028; the earlier formal security report-finalizer limitation still applies.

## Increment E1 — native migration and local quarantine, in progress

- `contracts/migration.md` pins the public claude-mem query-export revision and the native V1/V2
  authority boundary. Native parsing now uses a private bounded SQLite plan, strict UTF-8, complete
  reference validation and shared preview/apply merge decisions. Preview does not create or migrate
  the destination. V2 export stages one read snapshot before publication; exact proposals and held
  provenance survive re-export without creating local work/session/raw-event authority.
- Migration 0007 adds a persistent public replica namespace and private origin receipts. An origin
  has one destination across files. Existing/tombstoned content retains its ownership, duplicate
  content within one file has the same preview/apply counts, and inherited secret state never
  rehydrates payload or becomes ordinary content. Foreign noncanonical remote metadata cannot
  auto-create a repository; explicit mapping preserves the existing destination metadata.
- `us5-classify-red.tap`, `us5-native-merge-red.tap`, `us5-classify-budget-red.tap`,
  `us5-path-lists-red.tap` and `us5-remote-red.tap` record the reproduced defects. The classifier now
  checks retained evidence and typed JSON fields, recomputes sanitized identity, preserves existing
  active/tombstoned targets, and rechecks full row/source/grant/origin plus live policy/context/lease
  state before release. Original paths govern checks; sanitized paths alone enter the flat proof.
- Classification is local-only and bounded to 100 units per pass, with a fenced cursor for progress
  past failed rows. The enclosing worker deadline also applies. A foreign path uses its source root
  only as a lexical anchor; an unanchored absolute path remains held. No real provider is used.
- `us5-classifier-final.tap` passes 68 focused checks on Node 24.16.0. After the additional remote
  metadata repair, `us5-remote-green.tap` passes 82 migration/transfer/privacy/repository checks.
  `us5-classifier-security-review.md` closes the scoped classifier/receipt/remote-metadata review.
- These initial E1 receipts do not claim full migration completion or installation readiness.
  The later handoff checkpoint below supersedes their implementation/check status.

## E1 handoff checkpoint — 2026-09-10

[Claude Code handoff](HANDOFF-claude-code.md) is the resume entrypoint. T029-T032 remain open.

- The pinned claude-mem query-export adapter now connects through `--from claude-mem`. Exact project
  and hash mappings are exclusive; preview displays counts, hashes, collision/held/excluded counts
  and source deletion-history limits. Native V2/external apply requires an existing current schema.
- The frozen synthetic fixture is in `test/fixtures/migration/`. Nine memory origins converge on
  eight local memories. Repetition, unchanged source bytes, private unknown metadata, unsupported
  records and native re-export/reimport pass. Related session/prompt payloads participate in each
  memory's classification through bounded private source receipts; no new runtime dependency was added.
- `us5-external-red.tap`, `us5-external-preview-red.tap` and `us5-external-support-red.tap` establish
  the integration, read-only preview and hidden-directive regressions. The subsequent focused
  `us5-external-support-green.tap` passes 73 checks, followed by the whole suites below.
- All nine `us5-native-integrity-red.tap` failures were repaired: DB/symlink/sidecar export targets,
  full staged graph validation, mixed dependency cycles, orphan repo origin, NULL candidate mismatch
  and terminal-parent proposal/origin redaction. `us5-native-integrity-green.tap` passes 16 combined
  native/external checks. Native publication still rechecks same-connection `data_version` afterward.
- `us5-handoff-unit-node24.tap` and `us5-handoff-unit-node22.tap` each pass 1,134 checks.
  `us5-handoff-typecheck.log`, `us5-handoff-lint.log` and `us5-handoff-build.log` pass.
  `us5-handoff-pack.log` verifies an isolated 20.698 MB installed package and its version;
  `us5-handoff-help.log` confirms the implemented CLI switches.
- The first Node 24 serial run overlapped unit/pack rebuilding and failed the large-input partial
  capture check (201/202). No source change was made; the prescribed isolated serial run passes
  202/202 in `us5-handoff-serial-node24-isolated.tap`. Preserve the original failed receipt and keep
  build/pack, parallel unit and serial E2E/fault phases sequential.
- `us5-handoff-serial-node22-isolated.tap` also passes 202/202 on Node 22.16.0. Each supported Node
  therefore passes 1,336 unit/migration/script/E2E/fault checks at this handoff snapshot.
- The latest independent native review produced the nine regression cases above; parent verification
  confirmed their RED/GREEN results. Additional independent review stopped at the agent usage limit.
  Parent Standards/Spec/Ponytail review and its exact scope are in `us5-handoff-review.md`.
  The earlier formal security finalizer limitation remains; this checkpoint does not close T043.
- Still open: final wire appendix/source research consolidation, explicit migration promotion,
  the remaining preview/terminal/personal/race matrix, near-limit/adversarial RSS and a complete US5
  review. US6, US7, resident/fallback implementation and actual product qualification continue
  under the existing task markers after handoff.

## E2 — explicit migration promotion

2026-09-10, `009-memory-core`, with the second review follow-up based on unchanged HEAD `590c0a2f`.
The current command is `oboete import promote <migration-record-id> --work <local-work-id> [--json]`
in `src/transfer-promote.ts`, with dispatch in `src/transfer.ts` and CLI help in `src/cli.ts`.
Use `oboete import promote --list [--json]` to discover receipts in the verified cwd repository.
It adds no dependency or schema. T029-T032 remained unchecked at E2; E7 closes them.

- Git identity and stored-context verification run before the immediate transaction. Inside it,
  the context ID/repository/local key, clean held candidate, import-created (`effect='inserted'`),
  locally classified non-expired ordinary
  origin other than a session summary, and existing local work are rechecked. Promotion adds only
  the explicit historical work grant, a pending inferred proposal with empty source IDs, and its
  receipt pointer. Candidate hashes use the sanitized exact strings. Repetition preserves pending,
  rejected and approved decisions. Imported approval and declarations confer no local approval.
- The initial `test/unit/migration-promote.test.ts` suite added 44 cases: native export/import/classify/status/approve,
  source immutability, unchanged current work/session/event state, repeated grants/proposals,
  terminal decisions, source `cli`/`automatic_direct` approval, 22 unavailable conditions with full
  table snapshots, nine argument errors, opaque work IDs, fixed identity/hash vectors, strictest
  sensitivity, transaction rollback, personal-domain retention and a missing destination store.
  Classification uses the local detector through `runObserve`; provider calls fail the fixture.
- Receipts are under `/tmp/oboete-009-20260909.jJ5grc/us5-promote-*`. The requested existing
  `/var/tmp/oboete-009-20260909.jJ5grc/` is outside this session's writable roots, so these receipts
  were not copied there. `us5-promote-commands.log` records commands; `us5-promote-tests.log` lists
  every new test name; `us5-promote-runtime-versions.log` records both installed executables.
- RED/GREEN slices: `red-01-detail.tap` plus `red-01-direct.tap` establish the missing command;
  `red-02.tap` establishes duplicate promotion failure; `red-03.tap` establishes the incorrect
  pending result after rejection; `red-04.tap` establishes rejection of an exact source ID containing
  `=`. The corresponding `green-01.tap` through `green-04.tap` pass. Each filename has the
  `us5-promote-` prefix. Additional existing-guard coverage was already green when added.

| Initial E2 verification | Result | Receipt, with `us5-promote-` prefix |
| --- | --- | --- |
| Final typecheck / lint / build, Node 24.16.0 | PASS | `final-typecheck.log`, `final-lint.log`, `final-build.log` |
| New tests after review, Node 24.16.0 | 44 PASS, 0 FAIL | `final-node24.tap` |
| New tests after review, Node 22.16.0 | 44 PASS, 0 FAIL | `final-node22.tap` |
| Focused migration/scope/transfer, each Node | 124 PASS, 1 FAIL | `focused-node24-inprocess.tap`, `focused-node22-inprocess.tap` |
| Node 24 full unit/migration/scripts, sequential per-file detail | 1,155 PASS, 27 FAIL | `unit-node24-detail.tap`, `.log`, `.json` |
| CLI help | PASS | `help.log` |

The required typecheck, lint, build, focused Node 24, focused Node 22 and full Node 24 phases ran
sequentially. The ordinary process-isolated runner returned only file-level summaries: focused
6/7 files pass on each Node; full 72/83 files pass (`focused-node24.tap`, `focused-node22.tap`,
`unit-node24.tap`). Supplementary runs used `--experimental-test-isolation=none` to obtain actual
case counts. The full diagnostic runs each file in a separate, sequential Node invocation, retaining
file isolation without the opaque test-child report. Build/pack never overlapped tests.

The focused failure is the unchanged FIFO CLI test's `spawnSync mkfifo EPERM`. Full failures are in
the existing CLI/trust/log/replay/transfer/viewer/work/script tests; observed errors include process
`EPERM`, loopback `listen EPERM` and tmux startup failure, with dependent empty-output assertions.
`spawn-diagnostic.log` records a credential-free reproduction. This environment has not passed the
whole gate. The full diagnostic run preceded the final equivalent test-arranger/comment cleanup;
typecheck/lint/build and all 44 changed tests were then repeated on the final files. Unchanged
blocked suites were not repeatedly rerun under the same restrictions.

Scoped correctness/security, Standards/Spec and Ponytail reviews are recorded in `review.log`.
Standards findings about the test setup cascade and unnecessary comments were corrected and
re-reviewed. No additional authorization or transaction defect was found. Online CLI/Sonar/Codacy
reviews were not run under the network ban; lizard is not installed. This is not a complete US5 review.

**Contract gap closed by the review follow-up:** as directed by the owner, `cleanCandidate` now
requires `type <> 'session_summary'` and `valid_to IS NULL`. The former scratch cases in
`us5-promote-contract-gap.tap` are retained as historical evidence; `expired-origin` and
`summary-origin` now belong to the ordinary unavailable-case table, asserting the fixed exit 1
result and unchanged full-table snapshots. `us5-promote-fix-red-scope.tap` records both erroneous
exit 0 results before the SQL change; `us5-promote-fix-green-scope.tap` passes both cases.

The same follow-up moves both `resolveRepoIdentity` and `verifiedRepoContext` outside the write
lock. The transaction still checks the context row's repository and unchanged `local_key` before
re-reading the candidate. A separate-connection write-lock probe during each Git call fails in
`us5-promote-fix-red-lock.tap` and passes in `us5-promote-fix-green-lock.tap`, together with the
existing `stale-context` case. No shared reader policy, contract or research file was changed.

Follow-up verification ran sequentially: `npm run typecheck`, `npm run lint`, `npm run build`,
then the promotion/migration/memory-scope suites on Node 24.16.0 and 22.16.0. All three static/build
checks pass (`us5-promote-fix-typecheck.log`, `us5-promote-fix-lint.log`, `us5-promote-fix-build.log`).
Both runtime suites pass **121/121**, including all **47** promotion cases and `stale-context`
(`us5-promote-fix-node24.tap`, `us5-promote-fix-node22.tap`). These runs use
`node --test --experimental-test-isolation=none --test-reporter=tap --enable-source-maps` with
`build/test/unit/migration-*.test.mjs` and `build/test/unit/memory-scope.test.mjs`; the glob includes
the promotion suite once. Exact commands and RED/GREEN results are in `us5-promote-fix-commands.log`.

**Whole gate outside the delegated sandbox (Claude Code, 2026-09-10 21:01-21:2x JST):** the Codex
sandbox blocked `mkfifo`, loopback `listen` and tmux, which explains every failure above. The same
files were then run by the parent session with no sandbox, strictly sequentially, using the exact
`package.json` phases: `npm run typecheck`, `npm run lint`, `npm run build`, the parallel
unit/migration/scripts glob, the `--test-concurrency=1` E2E/fault glob, and `npm run pack-check`.

| Verification | Result | Receipt, with `us5-e2-` prefix |
| --- | --- | --- |
| typecheck / lint / build, Node 24.16.0 | PASS | `typecheck.log`, `lint.log`, `build.log` |
| unit/migration/scripts, Node 24.16.0 | 1,181 PASS, 0 FAIL | `unit-node24.tap` |
| serial E2E/fault, Node 24.16.0 | 202 PASS, 0 FAIL | `serial-node24.tap` |
| unit/migration/scripts, Node 22.16.0 | 1,181 PASS, 0 FAIL | `unit-node22.tap` |
| serial E2E/fault, Node 22.16.0 | 202 PASS, 0 FAIL | `serial-node22.tap` |
| packed install / version | PASS, 20.703 MB | `pack.log` |

Each supported Node therefore passes 1,383 checks at this snapshot (1,336 at the handoff plus the
47 promotion cases). Phase exit codes are in `us5-e2-gate-summary.txt`; the driver is
`us5-e2-gate-driver.log`. The `/tmp/oboete-009-20260909.jJ5grc/us5-promote-*` receipts were copied
into `/var/tmp/oboete-009-20260909.jJ5grc/` unchanged.

**Whole gate for the follow-up (Claude Code, `us5-e2b-*`):** the same sequential `package.json`
phases pass again on both Nodes after the `--work`/`--list` change and the six review fixes:
1,203 unit/migration/scripts and 202 serial E2E/fault each, typecheck/lint/build and pack-check
(20.703 MB). Phase exit codes: `us5-e2b-gate-summary.txt`.

Still open: the remaining US5 terminal/preview/race matrix, packed CLI/RSS measurements and complete
US5 reviews. No commit, push, daily installation, real provider or external network operation ran.

**Second review follow-up (`590c0a2f`, no commit):** promotion now accepts exactly one nonblank
`--work <local-work-id>` of at most 512 characters; the held payload already pins the historical
origin work. The former promotion `--map-work` form is rejected. Native import mappings retain
their existing `--map-work` form. `--list` accepts no record ID or work argument and returns at most
100 sharing-proposal receipts, ordered by ID, plus an omitted count. JSON is `{ records, omitted }`;
each record has only `id`, `memory`, `state`, `effect`, `promotable`, and `proposal`. Human output
has one line per record and `N more records omitted.` when truncated. Candidate text, payload
fields, source IDs, project names and paths never leave this listing. The row SQL and payload
predicate are shared with promotion; listing omits only the work-argument check and uses a read-only
connection and read transaction. Empty lists succeed without a write lock. Missing, behind, ahead
and unverifiable contexts preserve the fixed unavailable result and never create/migrate a store.

All following receipts are under `/tmp/oboete-009-20260909.jJ5grc/`, with prefix `us5-promote2-`.
The command/list contract change is RED in `command-list-red.tap`; all 69 final promotion cases
are GREEN in each `node24-detail.tap` / `node22-detail.tap`. `help.log` confirms actual CLI usage.

| Closed finding | RED receipt | GREEN evidence |
| --- | --- | --- |
| Origin must be import-created | `origin-red-detail.tap`: native import converges on unchanged local content, then incorrectly promotes (exit 0). | `origin-green.tap`: refusal with all tables unchanged; final suites also cover `matched_existing`, `held_by_tombstone`, and `historical_held`. |
| Dead proposal guard | No new RED: the existing first test already exercises `share status` and successful `share approve`. | `origin-green.tap` and both final detail TAPs; no redundant `provenance_complete` condition was added. |
| Operational errors are not unavailable | `operational-red.tap`: swallowed busy/save errors; `operational-cli-red.tap`: actual CLI exits 1 instead of 3. | `operational-fd-green.tap`: busy reaches `isBusyError` and actual CLI first-line stderr/exit 3, with unchanged tables; final suites also verify checksum mismatch and database I/O propagation. |
| Phantom `deleted-origin` | `deleted-origin-red.tap`: removing only the deletion guard changes exit 1 to exit 0. | `deleted-origin-green.tap`: restoring the guard refuses promotion. The fixture restores and asserts the trigger-cleared clean payload first. |
| Phantom `secret-origin` | `secret-origin-isolated-red.tap`: removing only the sensitivity guard changes exit 1 to exit 0. | `secret-origin-isolated-green.tap`: restored guard refuses promotion. The fixture restores both 0007 receipt fields and 0005 origin review/provenance fields. |
| Phantom `wrong-payload-kind` | `wrong-payload-kind-red.tap`: removing only the kind guard changes exit 1 to exit 0. | `wrong-payload-kind-green.tap`: restored guard refuses a complete native memory payload that passes both `nativeMemorySchema` and `migrationPayloadShape`. |

`secret-origin-red.tap` retains the diagnostic showing that restoring the receipt alone still left
the 0005 `memories_provenance_privacy` review-state guard masking the sensitivity test. The isolated
receipt above is the effective RED. `mutation-commands.log` records each individual mutation,
sequential build/test and restoration. The first operational GREEN attempt and initial combined
command/CLI run retain the sandbox pipe failures; the FD and final receipts above supersede them.

Verification ran in the required order: typecheck, lint, build, the specified Node 24 test glob,
then that same Node 22 glob. All three static/build checks pass (`typecheck.log`, `lint.log`,
`build.log`). Supplemental detail runs then used `--experimental-test-isolation=none`, sequentially,
to obtain case-level results. No build overlapped tests.

| Latest follow-up verification | Node 24.16.0 | Node 22.16.0 | Receipt names |
| --- | --- | --- | --- |
| Requested process-isolated migration/scope/transfer/CLI glob | 6 PASS / 2 FAIL file suites | 6 PASS / 2 FAIL file suites | `node24.tap`, `node22.tap` |
| Same glob, case-level detail | 153 PASS / 4 FAIL | 153 PASS / 4 FAIL | `node24-detail.tap`, `node22-detail.tap` |
| Promotion cases within the detail runs | 69 PASS / 0 FAIL | 69 PASS / 0 FAIL | `verification-summary.log` plus the detail TAPs |

The four unchanged failing tests are CLI version, unknown-command usage, doctor JSON, and the
transfer CLI/FIFO test. `sandbox.log` reproduces `spawnSync` pipe `EPERM` with empty output on both
Nodes, while direct file descriptors receive the correct version/usage with the expected exit
codes; `mkfifo` also reports `EPERM`. The busy regression uses a temporary stderr FD to verify the
real CLI without weakening its expected message or exit code. The earlier parent whole-gate result
above applies to the baseline; the parent must run the full gate for this latest diff. This delegate
did not run E2E/fault or pack, access a network/provider, change Git state, or tick any checkbox.

## E3 — migration test matrix

2026-09-10, branch `009-memory-core`, baseline `943660a4ef1d1ecdcfe96545e7a1156e63772c5e`.
The ten requested T032 matrix cases live in `test/unit/migration-matrix.test.ts`. Existing migration,
transfer and scope tests were read first; no existing suite, contract, research file or checkbox changed.
Receipts below are in `/tmp/oboete-009-20260909.jJ5grc/`, with prefix `us5-matrix-`.

| Test name | Coverage and RED receipt |
| --- | --- |
| `matrix A1: preview requires an explicit verified context when zero or two candidates exist` | Real temporary Git repositories share one remote identity and have distinct verified generations. Zero/two candidates stay null; explicit choices resolve; unknown/stale choices reject. `a1-red.tap` removes the ambiguity guard and selects the first context. |
| `matrix A2: native preview bounds project and unresolved details with matching human counts` | 107 native projects produce 100 entries and 7 omissions in both outputs; unresolved hashes have the same independent bounds/counts. `a2-red.tap` shows the missing unresolved list. Expected hashes use a fixed vector or independent `node:crypto` computation. |
| `matrix A3: preview preserves missing, behind, ahead and writer-held WAL destinations` | Actually applies migrations 0001-0006 with checksums; also checks absent, version-8 and current WAL stores with an uncommitted second writer. Source bytes, all tables, `sqlite_master` and `user_version` stay unchanged; old/ahead apply refuses. `a3-red.tap` misreports behind as ready. |
| `matrix B4: changing mappings for the same file rejects without any table changes` | `import_mapping_changed` and full-table equality. `b4-red.tap` disables the file mapping guard. |
| `matrix B5: an overlapping origin mapped elsewhere rolls back even after earlier records` | `origin_mapping_changed` for first/later conflicting origins, including rollback of earlier rows and the import receipt. `b5-red.tap` disables the origin mapping guard. |
| `matrix B6: two source projects can share a destination while retaining distinct origins` | Collision count 1, successful apply, separate origins with one target key, and same-file/same-mapping JSON/human preview without writes. `b6-red.tap` removes collision accounting; `b6-duplicate-red.tap` reproduces the real duplicate-preview defect. |
| `matrix C7: all proposal decisions round trip as private history without nested origins or grants` | Pending/approved/rejected exact proposals survive export/import/local classification. Personal projection stays ungranted and source-free. Two transfer hops keep every original key/hash once without nested envelopes. `c7-red.tap` disables inherited-origin preservation. |
| `matrix C8: redacted source dependencies stay terminal after a forged plaintext replay` | Secret/deleted parent source fields export as null; source receipts are null and terminal. A forged same-origin payload with its parent link removed cannot rehydrate or downgrade the receipt. `c8-red.tap` replaces the insert-ignore guard. |
| `matrix C9: revoked personal grants preserve export identity and tombstones survive reimport` | Fresh local promotion/approval creates the grant before explicit local revocation. Export still identifies the personal projection; exact and overlapping reimports keep its local tombstone. `c9-red.tap` removes the migration-record domain lookup. |
| `matrix D10: packed migration preview and promotion print only bounded metadata` | Actual `dist/oboete.mjs` previews the frozen external fixture and native v2, lists no candidates, and refuses a bad promotion ID: exits 0/0/0/1. Both output streams exclude recognizable text, project names and paths. `d10-red.tap` exposes unhashed project identities. |

Two defects were fixed with the smallest source changes:

- `src/transfer.ts` now reports bounded unresolved hashes and their omitted count. Native human
  output includes the same project/omission counts as JSON. The original defect is in `a2-red.tap`.
- `src/transfer-merge.ts` resolves scratch mappings before the duplicate no-op return, so a repeated
  preview no longer reports mapped projects as unresolved or loses collisions. `b6-duplicate-red.tap`
  shows 0 collisions instead of the expected 1. Destination writes and duplicate no-op semantics are
  unchanged; the extended B6 asserts full-table equality.

The other nine cases initially passed existing code. Their RED receipts come from one temporary
mutation at a time, with sequential build/test and byte-for-byte source restoration recorded in
`mutations.log` and the runnable `mutations.py`. `restored-green.tap` and the post-review
`review-green.tap` each pass all 10 cases. `c9-initial.tap` and `c9-fixture-check.tap` are setup
diagnostics (a missing local binding and an incorrect expectation that local tombstoning clears
stored text), not product-defect RED evidence. The test now respects local tombstone storage and
checks that replay cannot revive it. All RED and initial receipts are retained.

`code-review` Standards/Spec reviews confirmed the duplicate-metadata fix and the independent hash
expectations; full-sentence copy and comment references were corrected. The subsequent Ponytail
review found nothing to remove. The scoped correctness/privacy review found no output payload leak,
destination write in preview, weakened guard or new dependency. These are local scoped reviews,
not a whole-US5 security finalizer or network CLI review.

Verification ran sequentially: `npm run typecheck`, `npm run lint`, `npm run build`, focused Node
24.16.0, focused Node 22.16.0, and full Node 24.16.0 unit/migration/scripts. `commands.log` contains
the exact commands; `runtime-versions.log` names both executables. The full process-isolated run
reports file suites; a subsequent run of its 11 failing files with `--experimental-test-isolation=none`
and explicit existing filenames provides case-level diagnostics. The first gate is retained as `first-*`;
the unprefixed phase names below are the final source after the duplicate-preview repair.

| Final verification | Result | Receipt |
| --- | --- | --- |
| typecheck / lint / build, Node 24.16.0 | PASS | `typecheck.log`, `lint.log`, `build.log` |
| Focused migration/scope/transfer, Node 24.16.0 | 159 PASS / 1 FAIL; new matrix 10 PASS | `focused-node24.tap` |
| Focused migration/scope/transfer, Node 22.16.0 | 159 PASS / 1 FAIL; new matrix 10 PASS | `focused-node22.tap` |
| Full unit/migration/scripts, Node 24.16.0, process isolated | 73 PASS / 11 FAIL file suites | `unit-node24.tap` |
| Diagnostics for the 11 failed files, Node 24.16.0 | 116 PASS / 27 FAIL | `unit-node24-failures-detail.tap` |

The focused failure on each Node is the unchanged transfer CLI/FIFO test: `spawnSync mkfifo EPERM`.
The packed matrix case uses temporary stdout/stderr file descriptors and all four real CLI commands
pass. The whole-suite failures occur in unchanged CLI/trust/logs/replay/transfer/viewer/work-context/
work-readers/DCO/tmux/pack-check tests: explicit subprocess/socket/FIFO `EPERM`, missing subprocess
output, and the work-readers subprocess's 2-second `ETIMEDOUT`. The latter output/timeout symptoms
are runtime limitations, not independently proven sandbox causes. The complete gate remains failed
and requires parent-environment verification; no assertion or gate was weakened.

Diagnostic command correction: the first supplementary same-process full run received literal glob
arguments. Existing `pack-check.mjs` treats an unresolved `argv[1]` realpath as direct invocation, so
module loading unexpectedly ran build, then `npm pack` failed with `EPERM`; it did not reach install.
This occurred before TAP cases started, but violated the intended explicit build/test phase boundary.
The run completed before it could be stopped. Its 1,190 PASS / 27 FAIL receipt is retained as
`unit-node24-detail-glob-invalid.tap` and is **not accepted gate evidence**. An explicit-file one-case
pilot (`diagnostic-argv-check.tap`, 1 PASS), then only the 11 failed files, produced no build/pack side
effect. The saved gate driver now expands same-process glob arguments before execution.

Contract mismatch left for the parent: `contracts/migration.md`, "Preview and mappings", requires
bounded context candidate IDs. The implementation returns the chosen `context` or null, without a
candidate list. A1 proves the requested ambiguity/explicit-mapping behavior; this job does not change
that contract or add candidate-list UI. `protected-files.log` confirms contracts/research match HEAD.
Scoped `speckit-verify-tasks` found T032 intentionally unchecked (`verify-tasks.log`); no completion
markers were changed. Near-limit RSS, publication races, E2E/fault, packed installation, whole-US5
qualification and external reviews remain outside this matrix job. No worktree commit, push, stash,
reset, checkout or branch change, global installation, external network or real provider was used.

### E3 review follow-up — four findings closed

2026-09-10, same branch and HEAD, preserving the uncommitted matrix changes above. Receipts are
in `/tmp/oboete-009-20260909.jJ5grc/` with prefix `us5-matrix2-`. The initial files are captured in
`start.log`; `src/transfer-merge.ts`, context verification, the migration contract and every existing
helper remain byte-identical to that snapshot.

| Closed finding | Change and evidence |
| --- | --- |
| Missing context candidate IDs | `previewMetadata` reads the destination database only for a mapped repository whose context is null. JSON lists up to 10 stored context IDs in ID order and the remaining count; a missing store produces an empty list. Human output prints one `Context candidate <repo-id>: <context-id>.` line per ID and one omission-count line when needed. A1 checks zero/two candidates, explicit selection without candidates, 12 stored candidates bounded to 10 plus 2 omitted, absence of roots/paths, and unchanged tables. This closes the contract mismatch left above. |
| Misleading human unresolved omissions | Human output is exactly `N unresolved projects.` for the JSON total `unresolved.length + unresolvedOmitted`. JSON fields are unchanged. A2 compares the entire summary line with that total; B6 checks the zero case. |
| Missing negative apply case | A3 now applies to a missing destination and checks `destination_schema_not_ready`, `applied: false`, and both the database file and destination directory still absent. Existing behavior already passed this assertion before the source repair. |
| Five duplicated output helpers | `test/helpers/output.ts` exports the original capture helper, imported by matrix/external/import/native-integrity/promote tests. `helper-review.log` confirms identical behavior, including the differently formatted native-integrity copy, and no other helper changes. |

RED was recorded before implementation. `red-node24.tap` reports the failing file only;
`red-node24-detail.tap`, using the same explicit file with `--experimental-test-isolation=none`,
shows A1 failing for missing candidate fields and A2 failing for the old human summary, while A3
passes (1 PASS / 2 FAIL). The initial union-property typecheck error is retained in
`first-typecheck.log`; an `in` guard fixes it without changing the JSON shape.

Final verification ran strictly in order: `npm run typecheck` → `npm run lint` → `npm run build`
→ the requested focused Node 24.16.0 tests → the same Node 22.16.0 tests. Only afterwards, the
same focused files ran sequentially with `--experimental-test-isolation=none` for case-level
diagnostics. All glob arguments were expanded to existing filenames before invocation.
`commands.log` records exact commands; `runtime-versions.log` records both executables.

| Verification | Result | Receipt suffix |
| --- | --- | --- |
| typecheck / lint / build | PASS | `typecheck.log`, `lint.log`, `build.log` |
| Requested focused run, Node 24.16.0 | 7 PASS / 1 FAIL file suites | `focused-node24.tap` |
| Requested focused run, Node 22.16.0 | 7 PASS / 1 FAIL file suites | `focused-node22.tap` |
| Focused case diagnostics, Node 24.16.0 | 159 PASS / 1 FAIL; matrix 10 PASS / 0 FAIL | `focused-node24-detail.tap` |
| Focused case diagnostics, Node 22.16.0 | 159 PASS / 1 FAIL; matrix 10 PASS / 0 FAIL | `focused-node22-detail.tap` |

Both case receipts provide GREEN evidence for A1/A2/A3 and the other seven matrix cases;
`red-green.log` summarizes those transitions. The sole failure on each Node is the unchanged
`test/unit/transfer.test.ts:370` FIFO setup, `spawnSync mkfifo EPERM`. The required focused gate
remains failed; no assertion, test selection or gate was weakened. The parent runs the whole gate.
Scoped correctness/privacy review, independent `code-review` Standards/Spec reviews (0 findings
each), and the subsequent Ponytail review (0 findings) are recorded in `review.log`.
`final-scope.log` checks preserved files, references, HEAD/branch and unchanged task markers.
No checkbox was ticked; the broader US5 qualification remains outside this follow-up.

**Whole gate for E3 (Claude Code, `us5-e3-*`):** the sequential `package.json` phases pass
typecheck/lint/build, 1,213 unit/migration/scripts checks on each Node, serial E2E/fault 202/202 on
Node 22.16.0 and pack-check. The Node 24.16.0 serial run was 201/202: `worker-kill-after-response`
failed in its seed precondition (`capture hit its 300 ms deadline under load`) while the resource
measurement job was saturating the host with a near-256 MiB preview. This is the documented
load-only seed miss, not a regression in the changed files; the phase is rerun in isolation once
the measurement finishes and its receipt is recorded below.

The isolated Node 24.16.0 serial rerun is recorded under E4 (`us5-sec-serial-v24.16.0.tap`,
202/202, and again in every later `us5-sec*` gate).

## E4 — US5 security review and fixes

2026-09-11, branch `009-memory-core`, the commit after `7f37376c`. Receipts in
`/var/tmp/oboete-009-20260909.jJ5grc/` with prefix `us5-sec`; review transcripts under
`us5-sec-reviews/` (`arch`, `g1`…`g3` from 2026-09-10, `secrev2` resumes, `secrev3`…`secrev13`
fresh follow-up rounds (`secrev11` was cut short by the provider's content filter and rerun as `secrev12`), `finder-reports.txt` from the `code-review` finders).

Per `rules/security.md` the fixes were written by Claude Code, not delegated. Every fix followed
RED → GREEN in `test/unit/migration-authority.test.ts` (16 cases) plus one case in
`migration-native-integrity.test.ts` and an updated matrix C8; the RED receipts are `us5-sec-red.tap`, `us5-sec2-red.tap`,
`us5-sec3-red.tap`, `us5-sec5-red.tap`, `us5-sec7-red.tap`, `us5-sec10-red.tap`.

| Finding (source, severity) | Fix |
| --- | --- |
| Redacted `personal_projection` wire `content_hash` selects any ordinary memory and tombstones/blocks it (arch+g2, high) | `findExisting`: an unverifiable hash (redacted personal) only matches a row already known as a personal projection; no match → `historical_held`, never a tombstone row. Verified (text-bearing) personal hashes resolve any row, including marker-less tombstones (secrev3 medium). |
| Cached sensitivity lets a later record lower a trigger-raised value (g2, medium) | Rank guard in the `UPDATE` write path; counts stay cache-based so preview == apply. |
| Dependency source may carry plaintext / non-secret child of secret parent (g3, medium) | Validator rules `dependency_source_has_text` (edge only) and `dependency_sensitivity_below_parent` (all rank pairs, source and checkpoint edges). |
| `promote --list` loads up to 100 payloads (g3, medium) | `CASE WHEN eligible THEN payload_json END` + `iterate()`. |
| Local stricter parent, coalesced origins, trigger-raised parent, held parent, order-dependent unverified records (code-review + secrev4/secrev5, medium) | `raiseToParents`: identity-keyed worklist over `transfer_lineage`, live parent rank, trigger-descendant resync after each live raise, unverifiable records merged last. Doubling probe linear (`us5-sec7-perf-probe.log`). |
| Receipts miss a later alias / trigger raise / cross-import or re-export terminal change (secrev5, secrev7, secrev8, secrev9, medium) | `saveOrigins` takes the stricter of the identity's final merged state and the live row; a record whose identity resolved to a row in another repository is hash-only (`identity_elsewhere`), as are records nested under it, so no held payload exists to clear later. Two intermediate designs (trigger by payload hash, then by an `identity_hash` column) were reverted after Codex rounds 5 and 6 showed each left a path. |
| Orphan retainable origin payload (secrev10, secrev12, medium) | Validator rule `orphan_origin_payload`, terminal label read per kind exactly as `migrationPayloadRedaction` does; matrix C8's forged replay is now refused outright instead of ignored. The provenance loss for `identity_elsewhere` records is fixed as specification in the contract. |
| Proposal receipt ignores the projected memory's terminal state (secrev12, medium, pre-existing) | `saveOrigins` folds the projected memory's final state into the proposal receipt (`finalState` helper shared with the parent memory). |
| UNIQUE violation reported as `scratch_storage_failed` (g1 rerun, non-security) | Primary SQLite code (`& 0xff`); `duplicate_source_origin` now reachable (test added). |

Reviews on the final tree: Codex `codex exec --sandbox read-only` security rounds ok:true with 0
critical/high (`secrev6` parsing layer, `secrev12` round 8b, `secrev13` round 9 final; the raw
`/tmp` outputs of rounds 6-9 were lost to a reboot on 2026-09-11 and are preserved as the
transcript copies under `us5-sec-reviews/transcript-verdicts/`); `code-review high` (finders
a/b/c/cleanup/altitude + lead) final verdict ok:true, 0 findings, repro set 168/168;
`ponytail-review` one shrink applied; semgrep 0 findings (`us5-sec2-semgrep.json`).

| Verification (`us5-sec12-*`, final tree, no concurrent build or review) | Result |
| --- | --- |
| typecheck / lint / build | PASS |
| Node 24.16.0 unit/migration/scripts | 1,177 PASS |
| Node 22.16.0 unit/migration/scripts | 1,177 PASS |
| Node 24.16.0 serial E2E/fault | 202 PASS |
| Node 22.16.0 serial E2E/fault | 202 PASS (`us5-sec12-serial-isolated-v22.16.0.tap`; the in-gate run lost four worker/lease cases to the 300 ms seed deadline while transcript recovery ran alongside) |
| pack-check | PASS |

Earlier gates on intermediate trees (`us5-sec`, `us5-sec2`, `us5-sec4`, `us5-sec5`, `us5-sec8`,
`us5-sec9`) are green too; the single failures in `us5-sec` (memory-recovery, e2e-hook), `us5-sec8`
(`remote-no-duplicate`) and `us5-sec11` (serial, both Nodes) are the documented load-only hook seed
misses and each passed in isolation (`us5-sec-recovery-isolated-v24.tap`,
`us5-sec-serial-isolated-v22.16.0.tap`, `us5-sec8-serial-isolated-v24.16.0.tap`,
`us5-sec11-serial-isolated-v24.16.0.tap`, `us5-sec11-serial-isolated-v22.16.0.tap`). `us5-sec3`,
`us5-sec6`, `us5-sec7`, `us5-sec10` and `us5-sec11` ran while the bundle was being rebuilt or
reviewed under load and are not evidence on their own.

Accepted residuals (documented in the contract): `updated`/`unchanged` counts are cache-based;
`historical_held` records count as `unchanged`; a proposal receipt follows its origin memory, so a
later terminal change of the projection row alone does not clear it (Known validator limits); wall
time is dominated by scratch autocommit (E5). Codex round 9 (`secrev13/fix9.out`, 0 findings, 60
in-memory cases) suggests widening cases #15 (proposal/visibility with memory deletion) and #16
(other repository, deletion, alias order) as permanent regression tests; left for a later test pass.

## E5 — import wall time and RSS (commit `97bbe882`)

2026-09-11. The E1 measurement at `590c0a2f` (`us5-rss/us5-rss-report.md`) showed the near-limit
import spending 98.9 % of sampled time in the per-memory scratch merge: the private SQLite plan
ran every `transfer_targets`/`transfer_rows` write as its own autocommit statement under
`journal_mode = DELETE`, so each paid a journal create, fsync and delete. Codex
(`task-mtvzonzs-xg4byp`, worktree `009-rss`) wrapped the scratch writes in one transaction and set
`synchronous = OFF` on the scratch; that cut wall time by 94–98 % but doubled peak RSS. The cause,
isolated with seven packed-CLI variants (`us5-rss2/experiments/README.md`), was not the
transaction: the merge prepared about ten statements per record inside loops, and the faster run
accumulated millions of native `StatementSync` objects before garbage collection released them.
`src/db/statements.ts` caches one prepared statement per (database, SQL); `insertSource` and
`grantVisibility` use it too (`code-review` finding), and the memory loop scans the scratch in
rowid order instead of sorting it.

Receipts: `/var/tmp/oboete-009-20260909.jJ5grc/us5-rss3/` (`prepare.log`, `pack-receipt.json`,
`results.json`, `us5-rss2-report.md` as written by the harness, `runs/`, `profiles/`), same
generators, inputs (byte-identical, SHA-256 checked) and methodology as E1: packed tarball
installed offline into a private prefix, `/usr/bin/time -v` around the CLI only, isolated home per
run, sequential runs, Node 24.16.0. Counts, effects and destination table counts match the E1
baseline for every pair.

| Run (Node 24.16.0, packed CLI) | Input | E1 RSS KiB | E5 RSS KiB | E1 wall s | E5 wall s |
| --- | --- | ---: | ---: | ---: | ---: |
| case1-near preview | 1,000,000 lines, 256 MiB − 1 | 202,532 | 192,640 | 2,987.31 | 28.16 |
| case1-near apply | same | 304,276 | 181,232 | 3,227.97 | 108.03 |
| case2-valid preview | 4 MiB high-cardinality lists | 232,712 | 224,872 | 0.69 | 0.95 |
| case2-valid apply | same | 226,276 | 239,744 | 0.70 | 1.41 |
| case2-mixed preview | 4 MiB mixed lists | 359,120 | 360,176 | 1.06 | 1.18 |
| case2-mixed apply | same | 373,068 | 374,544 | 1.26 | 1.46 |
| case4 external preview | claude-mem 5 MiB | 147,108 | 129,228 | 12.49 | 0.59 |
| case4 external apply | same | 201,588 | 131,832 | 16.29 | 0.99 |
| case3 nested origins preview | 255 MiB receipts | 143,044 | 144,192 | 8.22 | 6.70 |
| case3 nested origins apply | same | 145,640 | 146,420 | 9.53 | 9.11 |
| 100k-memory profile preview | 65 MiB | 166,292 | 141,484 | 736.53 | 6.58 |

Every run is below the 512 MiB import/export CLI budget the contract now states; the largest is
case2-mixed apply at 374,544 KiB (the 4 MiB high-cardinality input), unchanged from E1 and not
investigated by this work. Scratch peak grows with the transaction (case1-near 857 MB against
664 MB, the rollback journal now covering the whole merge) and is recorded next to RSS; it lives in
the private temporary directory, not in memory. The two `case2` walls are noise at the 1 s scale.

Unpatched 100k-memory apply on the old tree for the missing E1 apply number: 200,328 KiB in
13:19.60 (`us5-rss2/experiments/590base-apply.time`); the same input now applies in 0:25.85 at
145,640 KiB.

Reviews: `/code-review high` on the perf diff, 6 findings (3 confirmed, 3 plausible), all
applied: statement cache moved to `src/db`, `insertSource`/`grantVisibility` cached, `+kind` scan
instead of a re-sorted two-pass query, scratch ROLLBACK guarded so it cannot mask the merge error,
rollback test extended with a receipt-stage fault, scratch journal size documented. `ponytail-review`
two shrinks applied. Gate `us5-perf1-*`: typecheck/lint/build, both Nodes 1,180 unit/migration/
script checks, Node 24 serial 202/202, Node 22 serial 201/202 in-gate (`db-missing`, the 300 ms seed
deadline while the Codex contract review ran alongside) and 202/202 isolated
(`us5-perf1-serial-isolated-v22.16.0.tap`), pack-check 20.7 MB.


## E6 — device sync (US6, T034–T036)

Two devices share one directory the user names (a mounted drive, a synced folder); nothing else
is contacted. On the first device:

```
oboete sync init /mnt/shared --classes eligible,local_only   # prints the space id
oboete sync key show                                          # terminal only: the one key line
oboete sync push
```

On the next device `oboete sync join /mnt/shared` asks for the key line on the terminal (it is
never an argument), then `oboete sync pull` / `push`. `oboete sync status` (also the MCP
`sync_status` tool and the `sync` line of `oboete doctor`) reads local state only; `resolve
<origin> --keep <revision | checkpoint origin>` closes a conflict; `map-repo` binds a repository
known only by path on the other device; `leave` removes the key, cursors and this device's bundle.
Exit codes: 0 ok, 1 nothing to do or a rejected bundle, 2 usage, 3 consent mismatch, 4 busy.

What travels: a per-replica revision log (identity lines for every revision, payloads for the
selected classes, control revisions for tombstones and sensitivity floors) inside an AES-256-GCM
bundle keyed from the space key (HKDF per bundle). Secret memories, deleted rows and quarantined
imports never carry text; a pulled approval keeps a projection only when it matches the local
approval record. The contract is `contracts/sync.md` (v10 plus "Implementation notes").

Evidence: `test/unit/sync*.test.ts`, 75 cases (identity, envelope tamper matrix, capture, replica
round trips, checkpoint forks and resolutions, natural-key aliases and `map-repo`, publish classes
and the reference closure, relay and push races, bounds and rejections). `OBOETE_SYNC_HEAVY=1`
adds the 256 MiB push round trip (82 s) and the 180,000-line chain / fan-out / merge-DAG staging
(~50 s each, RSS flat): the contract's 1,000,000-line chain would take ~270 s at the measured
3,700 lines/s, so the heavy gate runs the largest size under 60 s. Writing the verification list
found seven apply/publish defects and one CLI input gap, all fixed before review (tasks.md,
T034–T036 note).

## E7 — US5 close (T029-T032, `35c1d9d4`)

2026-09-14, on `main` at `35c1d9d4`, no source change. E3 closed the migration matrix, E4 the
security review and E5 the import wall time and RSS, so this section only re-runs the increment's
gate and records why the four markers can be checked.

- `us5-close-unit-v24.16.0.tap` and `us5-close-unit-v22.16.0.tap`: 142 checks pass on each
  supported Node over the seven `build/test/unit/migration-*.test.mjs` files and
  `build/test/unit/transfer.test.mjs`, 0 fail, 0 skipped.
- `us5-close-typecheck.log`, `us5-close-lint.log` and `us5-close-pack.log` exit 0; the packed CLI
  installs at 20.879 MB (limit 30 MB) and reports `0.1.0-alpha.0`.
- Three levels of "packed" are distinguished here, because T032's packed-CLI requirement is easy to
  over-claim. Exactly one test spawns the **built bundle** `dist/oboete.mjs`: `matrix D10`, which
  covers an external preview, a native preview, `import promote --list` and a refused promotion
  (exits 0/0/0/1, bounded metadata only) — not an export and not an applied import. The
  export/import exit codes of the CLI contract are pinned in `transfer.test.ts`, and
  `migration-import.test.ts` calls `runExport`/`runImport` directly; both are in-process and neither
  spawns a binary. `pack-check` builds and installs the **tarball** but only calls `--version`. None
  of those is an import through an installed package, so one was run today as well and recorded in
  `us5-close-installed-import.log`: oboete
  0.1.0-alpha.0 installed from `npm pack` into a temporary prefix, invoked under `env -i` with `HOME`
  and `OBOETE_HOME` inside the temporary tree, previews the frozen claude-mem fixture with exit 2 and
  an empty stderr — source hash `cfd2203c…`, 8 observations / 3 sessions / 1 summary / 4 prompts, two
  unresolved project hashes, `applyPossible: false`, 31 records held — and `import promote --list`
  exits 1 with the bounded scope message. The fixture's SHA-256 is identical before and after.
- Applying an import through an installed package is still not claimed by this section, and neither
  is the near-limit measurement. That stays E5's, recorded at `97bbe882`: 108 s apply, 28 s preview,
  largest run 374,544 KiB under the contract's 512 MiB budget. Its evidence bundle survives at
  `/var/tmp/oboete-009-20260909.jJ5grc/us5-rss3/` (2.8 GB; `/var/tmp`, so the reboot that cleared the
  `/tmp` scratch did not touch it). The `us5-perf1-*` files are that increment's gate logs, not its
  RSS bundle; both are intact and nothing in T029-T032 needs re-measuring.
- Two earlier statements said T029-T032 stay unchecked pending the macOS probe and a final review
  pass. Both are amended in `tasks.md`: the platform probe is T040's product, whose macOS leg the
  owner deferred, and the cohesive product gate is T043's. Neither is named by T029-T032.
- Receipts under `/var/tmp/oboete-009-us5close/`. Running the installed package's `setup` rewrites
  the real agent configuration files whatever `OBOETE_HOME` says, so that check must run with `HOME`
  pointed inside the temporary tree.

## E8 — resident observation worker (T047)

2026-09-14, branch `009-t047-resident`. The binding spec is
`contracts/resident-worker.md`, created at `f0f2dda6` before any implementation and amended in the
thirteen later commits that the implementation and the reviews exposed, the last of them this round's. Three implementation rounds (Grok, then Codex twice)
with a review pass over each delta — correctness first, over-engineering second — and a final test
round for the inputs that had no reader.

- Gate: `npm run build`, `npm run typecheck` and `npm run lint` exit 0. The full `npm test` passes
  on both supported Node versions — 1,512 pass / 0 fail / 2 skipped in the parallel leg and 280
  pass / 0 fail in the serial one, no `not ok` lines in either
  (`t047-full-v24.16.0-r16.log`, `t047-full-v22.16.0-r16.log`; the same legs before the last two
  review rounds are `t047-full-v24-r5.log` and `t047-full-v22-r5.log`, and before the first
  `t047-full-v24.16.0.log` and `t047-full-v22.16.0.log`). 33 of those tests are the resident's own,
  in `test/unit/resident-worker.test.ts`. Two harness flakes were met and re-run along the way,
  both in tests this PR does not touch: `matrix A2` lost to `ENOTEMPTY` inside the temporary home's
  teardown (#206, `t047-full-v22.16.0-r14.log`), and CI lost `grok-other-handler-deny` and
  `migration-promote` on one twin of the duplicated run (#168, #214), each green on the re-run.
- Idle cost, contract item 12, measured on a replayed corpus rather than an empty process: the
  1,051-event fixture bundle replayed into a kept home (1,322 raw events, 100 batches, 48
  sessions), then quiesced, then a resident run with no injected clock and a raised idle timeout.
  Over 675 s the process used 1,200 ms of CPU: **0.178% of one core**, per-30-s-sample 0.125% to
  0.218%, against a 0.5% target, and the observe log records no epoch line for the window because
  a maintenance epoch that changes no counts writes none (`idle-cost-final.json`). RSS settled
  rather than grew: 72.1 MiB at start, 78.6 MiB by 162 s, and a 79.6 MiB peak first reached at
  546 s and flat to the 675 s end — 1.0 MiB of drift across the last 8.5 minutes. `SIGTERM` then ended it as `signal`, exit 0, lease released. The first
  measurement (`idle-cost.json`) kept the un-quiesced home, so its first five minutes are the
  worker doing real work — 1.0-1.8% of one core while it produced 100 fallback batches — and its
  last 5.8 minutes contain no epoch at all: 0.233-0.300% of one core, sampled every 30 s. Both
  windows are reported because the average across them (0.746%) is not an idle number and would be
  the wrong receipt.
- RSS over the first measurement: 72.1 MiB at start, 97.7 MiB at the end, 99.1 MiB peak; within
  the epoch-free window it moved 97.1 to 97.7 MiB; the quiesced run above settles 18 MiB lower
  because it never produced the 100 fallback batches. The long-run RSS claim is not made here; item 12 was split so that T042's sweep owns it, along with
  the three corpus sizes, concurrent captures, the held reader and the WAL recycle.
- This host's monotonic clock runs about 7.4% slower than its wall clock (`clocksource` is `tsc`
  under WSL2): a 30,000 ms timer returns after 32,200 ms, measured directly. Every rate above is
  therefore computed from the sampled interval rather than the requested one, and `idle_exit` fires
  at about 1.07 times its configured duration in wall terms — 129.0 s and 129.3 s for a 120,000 ms
  bound in two runs, 960 s for 900,000 ms. That is the contract behaving as written, since epoch and
  idle budgets read the monotonic clock while expiry and retry read the wall clock. The idle poll's
  inputs were watched from a second connection every 2 s for a whole run and never moved, so the
  activity mark resets once at startup and a maintenance epoch does not postpone `idle_exit`. Those
  were the capture and completion stamps the poll read at the time; the poll now reads
  `data_version` and an in-process count instead, for the reason in the next bullet, and the
  measurement stands as a receipt that nothing was captured or processed during the window.
- The idle activity marks read no clock, and two candidate mechanisms were measured against a
  capture they must not hide. The stamps the first implementation compared — a change in
  `MAX(last_captured_at)` and `MAX(completed_at)` rather than an increase — cannot carry the signal
  they were chosen for: `markSessionCaptured` writes `last_captured_at` clamped with `MAX`, so it
  never decreases, and a batch completing after a backward correction adds a row whose smaller
  stamp the maximum over rows hides. Both marks therefore freeze while work continues, which is the
  failure the change comparison was meant to fix. `MAX(rowid)` over `raw_events` replaced them and
  has a narrower hole of the same kind: a purge that deletes the newest row frees exactly the rowid
  the next insert takes, so retention plus a capture inside one poll window leaves the mark
  unchanged. The mark is therefore SQLite's `data_version`, which advances on another connection's
  commit and never on this process's own writes — so a capture, always another process, is always
  seen, and the resident's own purge can never be mistaken for one. Both halves of that are
  asserted, each RED against the mechanism it replaced:
  `a backward system clock does not read continuing captures as idleness` (RED against the stamp
  read: `idle_exit` at 900,000 ms instead of surviving to 1,350,000 ms) and
  `a purge that frees the newest rowid does not hide the capture that reuses it` (RED against the
  rowid read, same shape). The one capture that arrives on the resident's own connection — a hook
  that exhausted its database budget spools, and recovery stores it later — resets the mark at that
  insert, asserted by `a capture the resident stores from the spool resets the idle budget` (RED
  without the reset: the log shows `recovered=1` and then `reason=idle_exit`). That reset reads an
  exact count only because recovery no longer discards committed work: a busy database now ends
  `recoverSpool` the way a lost lease already did, returning what it stored and leaving the
  remaining files queued for the next pass, so the call site needs no busy retry around it. The pin
  is `spool-recovery.test.ts`'s `a busy database returns what was committed and leaves the spool for
  the next pass` (RED before the change: `Error: database is locked` out of `transactionImmediate`).
  The ordering half — an entry stored before the busy one stays counted — holds by construction,
  since the counter moves before the throw point, and no test can sequence two writers inside one
  synchronous loop from outside it. The same hole undercounted `recovered` in the epoch log and
  `last_run` for the one-shot worker, which becomes exact with it. Completed processing
  is the resident's own applied and fallback count; that half has no isolating test, because every
  stimulus that completes a batch also inserts raw events or leaves work queued, and a test that
  passed on the other half's reset would be the narrow kind.
- Two controls were confirmed in production rather than only in tests, both with the lease released
  and exit 0: `SIGTERM` ended a resident as `signal` (`idle-cost.json`), and rebuilding
  `dist/engine.mjs` under an idle resident ended it as `upgraded` within one poll
  (`idle2-upgraded-exit.log`). The second was an accident — a rebuild during a measurement — which
  is the strongest form of that evidence and the reason the measurement had to be re-run.
- Retention does not ride on the idle probe. The probe clause the first contract draft called for
  was measured at about 1.6 microseconds per retained row on every poll, over a range that never
  empties because cited rows are retained forever (27.9 ms at 20,000 rows). An epoch now also opens
  on a 60,000 ms maintenance interval, which costs nothing per poll and bounds the delay to one
  minute against a seven-day TTL.
- The migration fence is a staleness rule, not occupancy, and now has a test rather than a source
  reading: a fresh heartbeat defers the migration with `MigrationBusyError`, and a heartbeat older
  than 6,000 ms is cleared by the migration itself, so a killed resident cannot deadlock an upgrade.
- The cleanup ownership probe is inside the failure guard, on both the resident and the one-shot
  path. A storage fault leaves the handle open with its statements failing, so a probe outside the
  guard throws while the storage outcome is being recorded and the run ends with no `run end` line
  and no closed handle. `a cleanup ownership probe that cannot answer still records the run end`
  asserts exit 3 and the run-end record on both paths, driving the fault by dropping `worker_lease`
  from a second connection. Each leg is RED against its own site: the resident leg with
  `shutdownResident` unfixed (`Error: no such table: worker_lease` out of `shutdownResident`,
  reported as a rejected call rather than an exit code), and the one-shot leg with only
  `recordRunFailure` reverted (the same error out of `recordRunFailure`). The one-shot leg reaches
  the fault through `captureRunningBatch`: a running batch inside its reclaim window keeps the
  queue undrainable, so the pass waits between passes instead of releasing. The first draft of
  this bullet claimed that seam did not exist; the Codex gate's fifth round named the fixture that
  provides it.
- Two holes in this PR's own new code, found by the sixth review round and fixed with a pin each.
  A database at schema version zero — what an interrupted first migration leaves behind — has no
  `worker_lease` table, so the `spawnAfterSpool` probe threw instead of answering and every capture
  spooled against a file that nothing would ever migrate; the catch now reads the version, which is
  exactly the set of states with no lease table (`a version-zero database spools and still starts
  the worker that migrates it`, RED before the fix on `spawned 0 !== 1`). And a stop sentinel that
  cannot be removed exited `stopped` in silence, stopping every later resident on sight; the removal
  now reports its error code and the release logs it (`a stop sentinel that cannot be removed is
  logged and the lease is released anyway`, RED on the missing warn line while the run still exits 0
  `reason=stopped`). Propagating the unlink failure instead was declined: the removal runs inside
  the transaction that releases the lease, so a throw would roll the release back and leave the
  sentinel as well as a held lease.
- What T047 does not claim: the resource sweep and soak (T042), the macOS platform leg (T040,
  deferred by the owner), and the pre-existing pass-loop defect filed as issue #231, which the
  resident inherits unchanged from the one-shot worker.
- Receipts under `/var/tmp/oboete-009-t047/`; the round-3 RED/GREEN logs, one per mutation, under
  `/var/tmp/oboete-009-t047/round3/`.

The contract's sixteen verification items, each against the test that carries it. Unless another
file is named, the test is in `test/unit/resident-worker.test.ts`.

1. `a resident retries a due source in a later epoch of the same process` — asserts both halves,
   the probe seeing the row and the next epoch batching it.
2. `the idle probe sees a due retry, a spool file, a pending batch and a pending summary`, and
   `a running batch inside its reclaim window does not start an epoch per poll`.
3. `a second resident exits 0 as another_worker without writing`, and `lease.test.ts`'s
   `rotateLease propagates SQLITE_BUSY so its caller can retry`, which also asserts the retry that
   follows returns a new token.
4. `each cooperative control exits 0 with its own reason` (eight rows), with
   `a fallback epoch still exits 0 on a cooperative stop`,
   `fallback exits keep worker and storage error codes in resident mode`,
   `capture activity resets the idle budget while an unchanged session expires` for the idle row's
   inputs, and `a purge that frees the newest rowid does not hide the capture that reuses it` and
   `a capture the resident stores from the spool resets the idle budget` for the mark that carries
   them.
5. `worker-stop is removed before the lease is released and pause is not consumed`,
   `a stop sentinel survives a takeover that happens during shutdown` — the removal runs inside the
   releasing transaction, so ownership is tested at the write rather than before it, and a lease
   stolen in that seam leaves the sentinel for the new owner —
   `an idle exit preserves a stop sentinel written during that exit`,
   `signal handlers survive shutdown and a signalled worker preserves the stop sentinel`,
   `shutdown with queued work releases the lease so a later spawn can reach it`,
   `observe --stop writes the sentinel and exits 0 without claiming the lease`,
   `a cleanup ownership probe that cannot answer still records the run end` for the guard the
   sequence runs inside, and
   `a stop sentinel that cannot be removed is logged and the lease is released anyway`, whose
   fixture puts a directory at the sentinel path so `unlinkSync` fails with a code the log names.
6. `capture.test.ts`'s `a schema-behind capture spools and still starts a worker when the lease is
   free`, `a schema-behind capture does not start a worker while the lease is held` and
   `a version-zero database spools and still starts the worker that migrates it` for the file an
   interrupted first migration leaves behind, which has no lease table to read at all, the
   `upgraded` row of item 4's table, and `test/migrations/apply.test.ts`'s `a live worker defers the
   migration and a stale one is cleared by it` for the crash variant.
7. `a stop before the provider request leaves the batch pending for immediate adoption`,
   `a control after a usable response preserves the applied batch citations and log`,
   `a stop after a response prevents both output and language retries` and
   `shutdown with queued work releases the lease so a later spawn can reach it`.
8. `the heartbeat keeps ownership under the token rotated for the second epoch` and
   `the heartbeat fires during a delayed apply and the lease survives it`.
9. `lease.test.ts`'s `after 6001 ms without heartbeat the second claim steals and the first token is
   fenced out`, `batches.test.ts`'s `a stale running batch of a dead worker is reclaimed after 120
   seconds`, and `observe.test.ts`'s `a crash after response leaves running work that is reclaimed
   once with two calls and one apply` — the two latencies separately, as the item requires.
10. `a wall-clock jump does not end an epoch budget measured on elapsed time`, `a wall-clock jump
    during apply does not cut the active epoch short`, and `a backward system clock does not read
    continuing captures as idleness` — the last written against the mutation that requires the
    capture stamp to grow, which is what the code did before this PR's last round. The item's two
    mechanisms are stated in the contract; what changed to make the first of them true everywhere
    is that `reclassifyImported` now takes a stop predicate, so no pass derives a wall deadline
    from a monotonic budget. Suspend/resume stays a platform question for T042 and T040.
11. `one-shot observe still exits after a failed source and does not retry in-process`,
    `shouldSpawnResident follows [worker] resident and defaults true`, and the unchanged
    `observe`/e2e suites on both Node versions.
12. This section's measurement.
13. `signals interrupt an injected wait during an epoch and release the lease`, `SIGTERM cancels the
    native idle timer so the resident process exits promptly`, and `signal handlers survive shutdown
    and a signalled worker preserves the stop sentinel`.
14. `a maintenance epoch purges an expired secret with no batchable work`.
15. `a config malformed at startup exits as config_changed before loading the worker config`.
16. `a batch_error ends a run after one attempt, including at the deadline in either mode`.

## E9 — bounded consented provider fallback chain (T037, T038, T039, T048)

2026-09-15, branch `009-t048-fallback-chain`. The binding spec is
`contracts/provider-fallback.md`, written at `73568de6` before any implementation, after four
orientation reads whose findings it records: consent covered only the primary preset, the
destination label is an authorization that `reconcilePendingDestinations` re-validates per pass,
`outcomeForSource` already defers a failed batch's sources with a retry time, and `CONSTITUTION.md`
requires an explicit spending policy. The same commit retires "M1 enables exactly one observer
preset at a time" in `specs/007-oboete-m1-alpha/contracts/observer.md`. Security-scoped work
(consent, credentials, egress), so it was implemented in this session rather than delegated.

- Gate: `npm run build`, `npm run typecheck`, `npm run lint` and `semgrep scan --config auto`
  (over the nine changed source files) exit 0 with 0 findings; `markdownlint-cli2` reports 0 issues;
  `scripts/dco-check.mjs main HEAD` passes all six commits. `npm test` is green on Node 24.16.0 and
  22.16.0 at the final head: 1537 tests, 1535 pass, 0 fail, 2 skipped, `NPM_TEST_EXIT=0` on both
  (`/var/tmp/oboete-009-t048/t048-full-v{24.16.0,22.16.0}-r7.log`). One earlier run failed
  `viewer-server.test.ts`'s SC-011 bound on Node 24 with `took 3235 ms`; the file passes 8/8 twice
  when run alone and this branch touches no viewer code — the test starts its clock before the
  stream is open, filed as #237.
- Two keys, one default: `[observer] fallback` is at most three ordered `{preset, model}` targets
  and `[observer] cost_policy` defaults to `["free-tier", "local"]`. Every configuration that
  exists today parses to an empty admitted chain, so `consentHash` appends nothing and the literal
  digest already pinned in `test/unit/config.test.ts` (`WORKERS_AI_CONSENT`) still matches — no
  install is asked to re-consent on upgrade. The opposite direction is pinned beside it: one
  admitted target changes the digest, and a listed target the policy excludes does not.
- The chain is a loop around the existing call and settlement in `processBatch`, not a new send
  path. Everything before it still happens once — privacy revalidation, the destination reconcile,
  the request build, the final detector check, `markRequest` — so one batch is one payload and its
  sources settle once. `observation_batches.provider_attempts` counts the reservations the chain
  took, which nothing reads as a bound.
- Packed CLI, 2026-09-15, temp home with no Workers AI credentials, a `ollama` target and a
  policy-excluded `nim` target (`/var/tmp/oboete-009-t048/packed-receipt/`): `setup --accept-egress`
  displays "Fallback targets, tried in this order only after a target fails" with the ollama target
  and its sensitivity classes, and does not display the excluded one; `oboete doctor` then reports
  `fallback:1 healthy  Target 1 is ollama with model qwen2.5:7b, admitted as local and ready`,
  `fallback:2 warning  … which the cost policy does not admit`, and a `provider degraded` whose
  consequence reads "every batch is summarized by the fallback chain below" rather than the
  rule-based sentence — the uncredentialed primary is a failed target, not a run without a provider.
- Measured, not asserted: a failing target that already answered does not spend a second
  allowance. The `unusable_output` case takes two reservations on one target (llm.ts's own retry)
  and makes zero requests to the next host; the three-target success case takes exactly three, one
  per target.

### E9 verification

Numbered against the contract's list. All in `test/unit/provider-fallback.test.ts` unless named
otherwise.

1. `an empty chain leaves the consent hash exactly where it was, and one target moves it`
   (`config.test.ts`) — against the literal digest, with the one-target half beside it.
2. `resolveModel carries the admitted chain and refuses one it cannot use` (`providers.test.ts`)
   and `a fallback chain the resolver refuses is reported at its position and on the provider item`
   (`doctor.test.ts`)
   — a `local` primary with a `remote` entry is `chain_unusable` at the resolve, and the run has no
   provider rather than a crash.
3. `a local target is never given a batch a remote target could not have been given`.
4. `admission drops what the policy excludes and refuses what widens egress` (`config.test.ts`) —
   the same fixture one key apart: default policy admits nothing remote, `remote` in the policy
   admits it in written order.
5. `admission drops what the policy excludes and refuses what widens egress` covers the
   `model_required` position and the `chain_without_primary` case.
6. The same test's last block: the primary repeated is dropped, a second model on the same preset
   is its own target.
7. `an exhausted primary hands the same batch to the next admitted target` — `exhausted_at` is
   per-preset, so the exhausted host receives nothing at all.
8. `the daily cap advances past every capped target and stops at none of the local ones` —
   `workers-ai` and `nim` both refuse at their own reservation, `ollama` answers.
9. `a target with no credentials is attempted, answers without a request and the chain moves on`.
10. `a consent change between targets stops the chain before the next host`.
11. `an unusable answer stops the chain instead of spending a second allowance on it`.
12. `every target failing settles once, keeps the worst reason and leaves the source retryable` —
    `provider_exhausted` outranks `unreachable` in `DEGRADED_PRECEDENCE`, the source is `waiting`
    with a non-null `retry_after`, and `processing_attempts` rose by one for the whole chain.
13. `a target that answers after two failures applies its output like any other`.
14. `the fallback chain is reported per target without a second provider request`
    (`doctor.test.ts`) — one provider request with a three-target chain, `fallback:1` healthy and
    `fallback:2`/`fallback:3` warning, and the same test shows that admitting a paid class stops
    the stored consent from matching.
15. `a primary with absent credentials is a failed target, not a run without a provider` — the
    destination label comes from the primary's egress class, so the loop is reached and the local
    target applies. Names the ceiling the contract retired.
16. `the reason a stop ended the chain on outranks a more severe reason behind it` — `auth_failed`
    then `consent_changed`; the batch keeps the consent reason and both attempt lines are logged.
17. `a target whose answer is refused for its language is still named in the log` — the ollama
    target answers twice in the wrong language, and its own `language_mismatch` line is present.
18. `a chain the configuration cannot use blocks neither capture-only nor rewiring`
    (`setup.test.ts`), and the `--remove` leg of
    `a destination that would strip the chain of its admission is refused before anything is
    written`.
19. `the second of two identical fallback entries is reported as covered, not as ready`
    (`doctor.test.ts`), and the `preset = "none"` leg of `a fallback chain the resolver refuses is
    reported at its position and on the provider item`.

Bot round on PR #238 at head `011b1b2e`: all check-runs completed, `dco`, `secrets`, `check`,
`engine (22.16.0)`, `engine (24.x)`, `semgrep-cloud-platform/scan`, SonarCloud (gate passed),
GitGuardian and Socket green; Codex code review and security review both completed with no
findings. Fixed from the four that did report: CodeQL's two high `js/incomplete-url-substring-
sanitization` alerts on the test helper's `url.includes(<host>)` dispatch (now `new URL(...).host`
equality), Codacy's `Semgrep unsafe-dynamic-method` on the `CHAIN_MESSAGES[code]` lookup (now a
`switch`, and the table is gone), Codacy's `Lizard_nloc-medium` on `fallbackTargetItem` (53 → 43
NLOC, measured with `pipx run lizard -l typescript`), SonarCloud's `typescript:S7755`
(`attempts.at(-1)`), and two CodeRabbit findings: an `agent-cli` target was reported ready although
`readCredentials` calls an agent login present without checking it, and a capped target was reported
ready with the shared allowance spent — which `allowanceItem` only reports when the primary is
capped. Declined: CodeRabbit's "apply `cost_policy` before validating an excluded target", because
it would move a hard privacy refusal behind a policy flag (see "Admission" rule 5).

Findings from the review round, all fixed in the same branch: the setup gate refused
`--remove`/`--provider none`/a bare run (P2, both reviewers); a stop's reason was hidden behind a
more severe earlier reason (P2); a `language_mismatch` target had no attempt line; doctor numbered
`chain_without_primary` as "fallback target 0" and reported a duplicated entry as ready; admission
rule 4 let an `egress: 'none'` primary admit a remote target (latent); the consent screen displayed
a local target's full capability rather than what the remote batch carries; and the dead
`?? outcome.reason` / `?? outcome.detail` branches hid the reason/detail pairing `loggableDetail`
depends on. Rejected: moving the three-target bound out of the configuration schema, which would
make one key's arity behave unlike every other malformed-config error.

Setup's side of T037 is `adding a fallback target refuses --yes and is displayed before it is
accepted` (`setup.test.ts`): a target written in after consent was stored refuses `--yes` with exit
2, prints the target's host before it is accepted, and leaves the stored hash alone until
`--accept-egress` re-records it. `the display names every fallback target the consent hash binds`
(`consent.test.ts`) pins that a policy-excluded target is not displayed as a destination.

### E9 follow-up — the second bot round, and the shape it opened

Head `2129c357` drew three P2 findings from Codex's PR reviewer, all three confirmed against the
source and the contract before anything was changed, and all three fixed here. Reading them opened
one shape with five instances, so the fix is the shape, not the three lines:

1. **A day-wide exhaustion flag answered for one preset.** `usageEstimate` returned
   `exhausted: MAX(exhausted_at) over every capped preset`, and four single-preset callers read it:
   `fallbackAllowanceItem` (Codex's finding), plus `providerCapItem`, `doctorReserve` and
   `allowanceEstimateItem`. `doctorReserve`'s was not cosmetic — it refused the probe's own
   reservation, so `oboete doctor --probe-provider` silently never called a primary that had its
   full allowance. The field is gone; `presetExhaustedAt(db, preset, now)` is exported from
   `src/observer/reservation.ts` and is the one reader of the stamp, including inside
   `reserveAttempt`, which had its own copy of the query.
2. **Doctor reported a fallback target ready under a primary the resolver refuses.** `admittedChain`
   validates the entries only, so `preset = "ollama"` with no `[observer] model` (its catalog
   default is empty) reported `fallback:1 … admitted as local and ready` while the worker degraded
   every batch with `no_provider`. Both surfaces now ask `resolveModel`, the worker's own resolver:
   the chain report replaces its per-target items with one degraded item, and the `provider` item
   carries the same sentence — which is the half that matters when no chain is configured at all,
   because `fallbackItems` returns nothing then. The same guard closes a case the reviewer did not
   name: a chain entry that widens egress takes the primary down with it, so a probed
   `provider healthy` used to contradict a `fallback degraded` in the same report.
3. **An `agent-cli` target spawned the paid child process without a reservation.**
   `summarizeWithAgentCli` never called `ctx.reserve`, so the batch stayed `pending` through the
   call. `adoptPendingBatches` takes a `pending` batch over with no wait at all, while
   `reclaimStale` fences a `running` one for 120 s — so a worker that died with the CLI in flight
   had its subscription spent again at once. It now takes the same `prepareProviderReservation` the
   HTTP targets take, which also gives it their second consent boundary. Two unit tests pinned the
   defect as an invariant (`reserve: () => assert.fail('agent-cli must not reserve')`); the contract
   never exempted an uncapped target from step 3, so the pins were stale and are now the opposite
   assertion.
4. **The fence those three lean on was measured from the wrong moment.** `claimed_at` was stamped
   once, at batch creation, and `adoptPendingBatches` only `COALESCE`s it, so `reclaimStale`'s 120 s
   was already spent for any batch created more than two minutes before its attempt — for every
   preset, not just `agent-cli`. `reserveAttempt` now restamps it with the attempt. Without this the
   contract sentence added for item 3 would have been false.

Deliberately not changed: the `fallback:N` items say nothing about consent, because consent is one
hash over the primary and the whole chain. (**Retired two rounds later** — there is no `consent`
item in the report at all, so that silence left a stale record unnamed: the chain now collapses into
one `fallback` item and the `provider` item carries the same sentence. See "the consent check moves
to the seam that already collapses the chain" below.) Filed instead of fixed: doctor calls the shared cap spent at
`remaining === 0`, while `reserveAttempt` already refuses `ten_turns` and `retention` at
`DAILY_CAP - SESSION_END_RESERVE`, so between 140 and 150 calls doctor reports an allowance the
worker will not grant. That is pre-existing, it is a wording decision about a trigger doctor cannot
see, and it is issue #240.

- Gate at this head: `npm run typecheck`, `npm run lint`, `markdownlint-cli2` and
  `semgrep scan --config auto` over the three changed source files all exit 0 with 0 findings.
  `npm test` green on Node 24.16.0 and on Node 22.23.1 (the 22.x line installed here; CI's
  `engine (22.16.0)` job covers the engines floor): 1547 + 280 tests, 0 fail, 2 skipped,
  `NPM_TEST_EXIT=0` on both, rerun at the review round's head.
- Each of the five was written as a failing test first, and each failed for its own reason before
  the fix. The first seven were run red before the fixes landed; the last three (`a reservation
  restamps claimed_at…`, `the provider item names a primary the resolver refuses…`, and the extended
  chain case) were confirmed red afterwards by reverting the two product lines, rebuilding and
  rerunning them — `claimed_at` came back as the creation stamp, and the provider item came back
  `unverified` with "Not probed this run" while doctor exited 0. The tests:
  `one capped preset's exhaustion is neither another's nor the shared allowance`,
  `a primary the resolver refuses leaves no fallback target to call ready`,
  `the provider item names a primary the resolver refuses when no chain reports it`
  (`doctor.test.ts`); `agent-cli is uncapped, consented, reserves its attempt and validates the CLI
  text as observer JSON`, `a refused reservation stops agent-cli before the paid child process
  runs`, `a consent change after the agent-cli reservation stops the chain before the child
  process` (`llm.test.ts`); `an agent-cli target reserves its attempt before the paid child process
  runs` (`provider-fallback.test.ts`); `a reservation restamps claimed_at so the reclaim timer runs
  from the attempt` and `usageEstimate reports the shared capped calls and reset; exhaustion stays
  per preset` (`callpolicy.test.ts`).
- The worker-level test for item 3 was green on Node 24 and stalled on Node 22 with
  `Promise resolution is still pending but the event loop has already resolved`, deterministically
  and in isolation. Not a flake and not a Node difference in the product: the shared agent-CLI spawn
  stub answered *every* command, so the `git rev-parse` that `updateBatchCitations` runs after a
  batch applies was handed a scripted CLI reply, and the pass stopped inside `checkpointBatch`
  without reporting anything. Node 24 hid it by delivering the stream events in an order that let
  the stub's failure land inside the `catch`. The stub now fakes only `claude`, `codex` and `grok`
  and hands every other command to the real `spawn`, which is what the two unit tests using it
  always assumed.
- One existing fixture was relying on the second defect: `a fallback target is not called ready when
  its allowance is gone or its login is unchecked` configured `preset = "agent-cli"` with no model,
  which the resolver refuses, so its capped-target warning was only reachable while doctor ignored
  the primary. It now names a model, which is what makes the uncapped-primary case it was written
  for real.

### E9 follow-up — the correctness review of the fixes

Eleven findings on the three fix commits. Seven taken, two rejected on their premise, one already
done, one deliberately left as an issue.

Taken:

- Three more `usageEstimate().exhausted` readers than Codex named, which is the sweep result above
  and was already in the fix. Beyond it: `providerCapItem` and `doctorReserve` still kept the stamp
  behind `PRESET_CATALOG[preset].capped`, while `reserveAttempt` reads it before it looks at the
  cap. Both now read it for any preset, which is what the fix commit's own title claims. Measured
  rather than assumed: with only `providerCapItem` reverted the probe is *still* stopped, by
  `doctorReserve`, but reported as "Provider reservation refused" instead of as the exhaustion it
  is — so both halves earn their place. Reachable only through a 429 carrying body code 3036
  (`classifyApiError`), which in practice is Workers AI, so this is alignment rather than a live
  bug. Test: `an uncapped preset that reported exhaustion is not probed and is not called healthy`.
- `presetExhaustedAt` dropped a guard the doctor helper it replaced had: `numberValue` reads a
  non-numeric `exhausted_at` as the epoch, so a row that was never stamped would have been reported
  as "exhausted at 1970-01-01". It now returns null unless the value is a number or a bigint.
- The `reset_at > now` half of that function had no test that could tell it from `true`, because
  both existing pins cross the UTC day and are filtered out by `utc_day` first. `a same-day stamp
  whose reset has already passed is not exhaustion` seeds the row the clause exists for and asserts
  the reservation is granted.
- The stub answered an overflow spawn with `assert.fail` inside a stream handler — the same
  asynchronous-throw channel that stalled the Node 22 run. Overflow now comes back as a failed
  child through `runChild`'s own `process_failed`, and the count is what a test asserts.
- `resolverRefusal` borrowed only `resolveModel`'s throw and discarded its result, so
  `providerProbeReadiness` still re-derived `(config.observer.model ?? defaultModel).trim()` by
  hand — two copies of one rule. It is now `resolvedObserver`, returning the resolved model for
  `configuredProvider` to pass down, in the same `kind`-tagged shape the file already uses.
- Extracting that helper had left `fallbackItems`'s own paragraph attached to it, stacking two doc
  blocks and leaving the exported function with none. Moved back.
- The recovery line said to set `[observer] model` to "a model the preset lists", which `agent-cli`
  and `ollama` do not do. It now says "a model that preset accepts".

Rejected:

- "`providerCapItem`'s `estimate` parameter is only read for `resetAt`, a pure function of `now`" —
  it is also read for `estimate.remaining <= 0`, which is the daily-cap branch.
- "`presetExhaustedAt` should use `prepared(db, sql)`" — `src/db/statements.ts` is used across
  `src/sync/`, and no module in `src/observer/` or `src/worker/` imports it. Adopting it for one
  function would leave `reservation.ts` inconsistent with itself, and the reader runs once per
  attempt, not once per row.

Left as an issue rather than fixed: the reviewer's root-cause proposal was to refuse
`setup --provider ollama|agent-cli` when no model is set. `a chain the configuration cannot use
blocks neither capture-only nor rewiring` (`setup.test.ts`) asserts exit 0 for exactly that command,
so selecting a local preset and then naming the model is the specified flow, and doctor saying so is
the recovery path rather than a regression. What is genuinely odd is that `agent-cli` requires a
model nothing ever sends — `summarizeWithAgentCli` reads it only as a non-empty gate and
`runAgentCli` never receives it. Fixing that moves the consent hash, so it is issue #241.

### E9 follow-up — the security review of the fixes

Defensive pass over `f5f766f9~1..62a9e2b9`, scoped to the four places the fixes could have moved an
authorization: the consent boundary on the agent-CLI path, the restamped fence, the removal of the
day-wide exhaustion flag, and the new doctor strings. **CLEAR, no P0/P1.** What it grounded, rather
than what it concluded:

- The agent-CLI path's condition for spawning the child is now `consentOk()` → `reserve()` →
  `consentOk()`, a strict subset of the old single check, so no input reaches the child under
  consent the old code refused; a consent change *during* the reservation is newly refused.
  `LeaseLostError` has no new escape: the worker's throw site is inside the chain loop that
  `src/worker/observe.ts` already wraps for the HTTP targets, and `doctorReserve` holds no lease, so
  its only throw is the `SQLITE_BUSY` the item already catches.
- The restamp cannot produce a concurrent call, a live-lock or a changed pick order, and the
  deciding inequality is `REQUEST_TIMEOUT_MS` (60 s) < `RECLAIM_AFTER_MS` (120 s): an in-flight call
  always finishes inside its own new window. A dead worker restamps nothing, because the restamp is
  `WHERE owner_token = ?` after `assertLease` while `reclaimStale` takes `owner_token IS NOT ?`.
  `pendingBatches` reads only `pending`, and the one `running → pending` path writes `claimed_at`
  itself, so restamped values never enter that queue's order.
- Dropping the day-wide flag removed *over*-refusal, not a refusal: it let one preset's stamp refuse
  another's. `reserveAttempt` was per-preset before the change, so doctor moved to the worker's rule
  and not the reverse. The review's own P2 — `doctorReserve` and `providerCapItem` still reading the
  stamp below the `capped` gate — was the correctness round's finding too, and `62a9e2b9` closes it.
- Nothing from `resolveModel` can carry a credential into a report: it throws only
  `ProviderConfigError`, interpolating a zod-enum preset name or an integer position, and
  `admittedChain` is total so there is no third path. Doctor reasons reach stdout and `--json`
  only — `src/doctor.ts` logs `{ exit, degraded }` and never the reason text.

Two nits and two pre-existing observations, none blocking:

- `src/work.ts` and `src/why.ts` order checkpoint decisions by `claimed_at DESC`, which the restamp
  makes further from settle order than it already was (`reclaimStale` restamped too). Nothing in
  `test/` covers either query today, so moving a sort key blind is not the trade: issue #242 carries
  the fix and the fixture it needs.
- The new number/bigint guard flips an unreachable state from fail-closed to fail-open.
  `provider_usage` is a `STRICT` table with an `INTEGER` column and `recordExhausted` is its only
  writer, so the direction is moot; the comment now says so instead of the code branching on it.
- Pre-existing and already filed: doctor ignores `SESSION_END_RESERVE` (#240, which now also records
  that `--probe-provider` can spend from that reserve), and `fallbackTargetItem` echoes the user's
  own configured model into `--json`.

### E9 follow-up — the session-end reserve, found three times

`SESSION_END_RESERVE` is 10 of `DAILY_CAP`'s 150 calls, and `reserveAttempt` refuses a `ten_turns`
or `retention` reservation from 140 calls on so an end-of-session summary is still possible. No
doctor surface knew that: `providerCapItem`, `fallbackAllowanceItem` and `allowanceEstimateItem` all
waited for `remaining === 0`, and `doctorReserve` used the same threshold, so between 140 and 150
calls doctor reported "Estimated 8 of 150 calls remaining" and a capped target as ready while the
worker refused every batch that was not a session end — and `--probe-provider`, the one doctor
caller that takes a real reservation, could spend from the ten held calls.

It was filed rather than fixed at first (#240), on the grounds that the wording was a decision about
a trigger doctor cannot see. Three independent finders changed that: this session's own sibling
sweep, the defensive security review, and CodeRabbit on the pushed head. It is also the same shape
`2129c357` closed for a fallback target — a surface calling something ready that cannot be
attempted — which is this branch's subject. So it is fixed here, and doctor does not need the
trigger to be accurate: below the reserve, only an end-of-session batch is served, and that is what
the items now say. `sharedAllowance` is the one reader of the band and `allowanceClause` the one
sentence the two allowance surfaces share; `doctorReserve` refuses in the band for the same reason
`reserveAttempt` does.

- `a capped target is warned while the last calls are held for end-of-session batches` pins both
  sides of the boundary — one call below the reserve both surfaces are still healthy, and at the
  reserve both report it. That is the clean red: `healthy` → `degraded` with the threshold reverted.
- `the session-end reserve stops a doctor probe without consuming another call` is a third row on
  the existing table beside `provider exhaustion` and `the daily cap`, pinning the exact sentence
  and that `provider_usage.calls` does not move. Its red is indirect — with the threshold reverted
  the fixture falls through to the probe and fails on its missing consent hash rather than on the
  band — so the boundary test above is the behavioural pin and this row is the string and no-spend
  pin.
- Gate at this head: `npm test` green on Node 24.16.0 and 22.23.1, 1549 + 280 tests, 0 fail, 2
  skipped; typecheck, lint, markdownlint, `semgrep scan --config auto` and
  `pipx run lizard -l typescript -T nloc=50 src/doctor/provider.ts` all clean.

The adversarial half of the security gate ran four attack lenses over the same range with two
refuters each, and **none of its ten findings survived refutation** — including two that named this
same reserve band, both refuted as pre-existing rather than introduced, which is what the record
above says too. #240 stays open only for the wording of the primary `allowance` item's healthy line,
which still quotes the raw remainder.

### E9 follow-up — the third bot round

Two more P2s from Codex on the pushed head, both taken.

- **The reserved band reused the spent state's consequence.** Folding `reserved` into the branch
  that already existed meant `oboete doctor` said "Source processing waits for the allowance to
  reset" while end-of-session summaries were still running — false for exactly the batches the
  reserve exists to protect. That was a judgement call made in the previous commit (the reason line
  carries the nuance, so let the consequence stand) and the reviewer was right that it does not:
  each of an item's three lines has to be true on its own. `allowanceClause` now returns all three,
  and the reserved state says that end-of-session summaries still run and that the reset is what the
  other batches wait for.
- **A duplicate entry was told to widen its cost policy.** `fallbackTargetItem` reported one
  combined "the cost policy does not admit or a nearer target already covers" for both of the
  un-admitted cases, and recommended adding the cost class — which cannot make a duplicate runnable,
  and which the entry usually already has. The contract's Diagnostics had asked for the two verdicts
  apart since it was written. `admittedChain` now returns a `ChainVerdict` per written entry
  (`admitted` / `covered` / `excluded`), because it is the function that knows which branch dropped
  the entry; doctor reads it instead of matching admitted targets back to entries, which deletes the
  `unclaimed`/`findIndex`/`splice` dance the item used to do. A duplicate is told to remove the entry
  or point it elsewhere, and its recovery is pinned not to mention `cost_policy` at all.

Red before the fix, by reverting the two branches: the duplicate's recovery still named
`cost_policy`, and the reserved band still claimed all processing waits. Gate at this head:
`npm test` green on Node 24.16.0 and 22.23.1 (1549 + 280, 0 fail, 2 skipped), typecheck, lint,
markdownlint, semgrep and lizard clean.

One process note worth keeping: restoring the two reverted branches by hand swapped the `spent` and
`reserved` texts, which the suite caught as three failures — including a row that had been green
before the revert. A revert-to-verify-red is only safe with the suite rerun after the restore, not
just after the fix.

### E9 follow-up — the fourth bot round

Two more P2s, both taken, and both the same defect family one axis further out.

- **Two `agent-cli` entries with different models were two targets.** Nothing sends the model —
  `summarizeWithAgentCli` reads it only as a non-empty gate and `runAgentCli` never receives it — so
  both entries launch the identical paid call, and an advancing failure such as `timeout` on the
  first pays the subscription twice for one payload. That is the shape US7 scenario 2 forbids, and
  it is the same paid-double-spend the first round's third finding was about, reached through
  admission instead of through the reservation. `identityOf` now identifies an `agent-cli` target by
  the command line tool, so the second entry is `covered`; the contract's Admission section says so.
  The other way to close it, sending the model to the CLI, widens what oboete asks of the
  subscription and stays issue #241.
- **A refused primary still claimed processing waits.** `daily_cap` and `provider_exhausted` both
  *advance* the chain, so `FALLBACK_CONSEQUENCE` contradicted the worker and the healthy target
  reported below it in the same report. `afeca975` had already made exactly this conditional for the
  uncredentialed primary; `providerCapItem` never got it. `refusedPrimaryConsequence` is now the one
  place that decides, and the credentials branch reads it too, so the two cannot disagree.

Red before the fix: `admission drops what the policy excludes and refuses what widens egress`
reported `verdicts: ['admitted', 'admitted']` with two `agent-cli` targets, and `a refused primary
says the chain is offered the batch, not that processing waits` got the "source processing waits"
consequence. Restoring after that check was done by copying the files back rather than by hand,
after the previous round's hand-restore swapped two branches.

Gate at this head: `npm test` green on Node 24.16.0 and 22.23.1 (1550 + 280, 0 fail, 2 skipped);
typecheck, lint, markdownlint, semgrep and lizard clean.

Declined in the same round: a stop landing between one target's own internal retries drops that
target's attempt line, which the finding called the stopped pass's only provider record. It is not —
the target reached that state by taking a reservation, and `reserveAttempt` writes the
`provider_usage` row and increments `provider_attempts` in the same committed transaction, which is
the accounting the contract names. The contract's sentence is about a pass that stops *between*
targets, and the omission is the same decision the settle path takes explicitly one branch below
(a line only for `state === 'fallback'` with a reason). `ProviderAttempt.reason` is a
`DegradedReason` read by `CHAIN_STOPS` and `mostSevereReason`; a cooperative stop is not one, so
recording it would widen that union with a value neither consumer can rank.

### E9 follow-up — the fifth bot round

One taken, three declined, and the three declines all resolve against this feature's own contract
rather than against the code.

- **Taken: the agent CLI stub refused a spawn call that omitted `options`.** `cliSpawn` hands its
  stub to the product through slots typed `typeof spawn` (`deps.spawn` in `src/worker/observe.ts`,
  `src/doctor.ts` and `src/setup/probe.ts`), and that type permits `spawn(command, args)`. The stub
  read `options.signal` unconditionally, so such a call would have failed with a `TypeError` in the
  tests while the same call worked in production — a test-only landmine of exactly the kind the
  stub's scoping fix was already about. `options` now defaults to `{}`, and a test calls the stub
  with two arguments. Red before the fix: `TypeError: Cannot read properties of undefined (reading
  'signal')` at `test/helpers/agent-cli.ts:50`.
- **Declined: continue the chain after `language_mismatch`.** The finding read `CHAIN_STOPS`, which
  lists only `consent_changed` and `unusable_output`, and concluded that `language_mismatch` was
  meant to advance. The set is not the authority — `contracts/provider-fallback.md` "Advance and
  stop" puts `language_mismatch` in the **stop** row beside `unusable_output`, for the reason the
  row gives: the request reached a provider, was answered, and spent that target's allowance, and
  both reasons already own their retries. `language_mismatch` is absent from `CHAIN_STOPS` because
  it never reaches that branch: `retryOnLanguageMismatch` owns its retry and its own fallback and
  returns `done`, which the contract states in the same section.
- **Declined: record the successful target in `attempts`.** Verification 13 pins the opposite —
  "the observe log carries one line per **failed** target" — and a batch that reaches `applied` is
  itself the record that a target succeeded. `ProviderAttempt.reason` is a `DegradedReason` read by
  `CHAIN_STOPS` and `mostSevereReason`; widening it to represent success would hand both consumers a
  value neither can rank, which is the same objection that declined the fourth round's finding.
- **Declined as already filed: expose the session-end reserve in `usageEstimate`.** Correct, and
  already issue #240, which names the same 140-of-150 threshold, the same three doctor surfaces and
  the same both-sides-of-140 test. It is a wording decision about one number describing two limits,
  not a threshold change, so it stays a follow-up rather than growing this PR.

**And one the bots did not report.** Running the length oracle over the whole tree and differencing
the warning set against the PR's own base (`85d48437`) — rather than reading the total, which stayed
116 in both — showed `runSetup` (`src/setup/setup.ts`) had crossed the bound this feature's own work
pushed it over: 43 NLOC at the base, 52 at this head, against `-T nloc=50`. It was invisible in the
total because `writeConfig` in `test/helpers/observe.ts` fell from 52 to 9 in the same PR and
cancelled it out. The `--provider`/chain-admission decision is now `selectedDestination`, which is
what the block already was — one decision with its own paragraph of comment — and `runSetup` is back
to 45. The earlier rounds in this section said "lizard clean" meaning the functions that round
touched; the differenced set is the claim that actually holds, and it is what the remaining rounds
state.

Gate at this head: `npm test` green on Node 24.16.0 (1551 + 280, 0 fail, 2 skipped) and 22.23.1
(same totals); typecheck, lint, markdownlint and `semgrep scan --config auto` clean; lizard warning
set differenced against `85d48437` adds nothing and drops `writeConfig`.

### E9 follow-up — the sixth bot round

One finding, on the extraction the fifth round produced, and it is real: `oboete setup --provider
<preset>` over a `[[observer.fallback]]` entry with no model wrote the destination and said nothing.
Reproduced before changing anything (a `none` primary, an `ollama` entry with no model, then
`--provider workers-ai`): exit 0, `preset = "workers-ai"` written, and no mention of the entry
anywhere in the report — while `resolveModel` refuses a chain it cannot resolve, so the primary the
run had just selected would not run either and every batch would be rule-based.

The reviewer's own fix — refuse on any chain error — is the one thing the contract rules out.
Verification 18: "`oboete setup --remove`, a bare `oboete setup` and `--provider none` all succeed
while a chain the configuration cannot use sits in the file; **only** a `--provider` that narrows
egress under an admitted chain is refused." Refusing `model_required` would block a destination
selection over a defect the flag did not cause, and it would be the opposite of what the same
command already does for a missing credential, which `contracts/cli.md` says setup reports "instead
of failing".

So the destination is still written and the entry is now named, with the sentence that the selected
preset does not run until it is corrected. Verification 18 gained that case, because the contract
previously said only what setup refuses and left what it does with the rest to be inferred — which
is the gap the finding read.

One edge named rather than closed: `admittedChain` returns at the first entry it refuses, so a
modelless entry ahead of a widening one hides the widening from this check, and such a file can be
written. Nothing is sent — the resolver refuses the same chain for the same reason, so no target is
ever built — and the report now says entries after the named one were not examined. Re-deriving the
admission rules in `setup.ts` to close it would put the same policy in two places, which is the
shape that drifts.

Red before the fix: the new setup test's `/Fallback target 1 requires an observer model/` did not
match, with the report printing the consent tuple and the credential steps and nothing else.

Gate at this head: `npm test` green on Node 24.16.0 and 22.23.1; typecheck, lint, markdownlint and
`semgrep scan --config auto` clean; lizard warning set differenced against `85d48437` unchanged.

### E9 follow-up — the seventh bot round

One P2, taken: the same defect the fourth round fixed, one branch further along. `providerItem`'s
**post-probe** failure returned `FALLBACK_CONSEQUENCE` unconditionally, so a probe that failed with
`auth_failed`, `unreachable`, `timeout`, `no_provider`, `provider_paid` or `model_alias` while an
admitted target sat below it in the same report said "source processing waits for the provider" —
while the worker hands that batch straight to the target the report calls healthy. The fourth round
gave the *pre-probe* refusals (credentials, cap, exhaustion) a shared `refusedPrimaryConsequence`;
the branch that reads `summarizeWithProvider`'s own answer never got it.

`CHAIN_STOPS` decides which it is, and it moved to `src/observer/classify.ts` beside
`DEGRADED_PRECEDENCE` rather than being copied: the worker's target loop and the doctor item are two
readers of one rule, and a second copy is the one that drifts. A stop reason keeps the waiting text,
because a stop really does leave the queue waiting.

Both directions are pinned in one test, and both were measured: with the fix reverted the chained
assertion fails (`'Temporary guidance is available while source processing waits for the provider.'`),
and with the branch forced to always chain the `consent_changed` half fails instead.

Also in this round, from the oracle rather than a bot: the new test's
`replace(/hash = "[^"]*"/u, …)` put a double quote inside a regular expression, and lizard's
TypeScript reader lost the function boundary there and swallowed 700 lines of `doctor.test.ts` into
one 588-NLOC span. The differenced warning set is what showed it — the file had no findings at the
base and two after. Replacing the value instead of matching the line removes the quote and the
warnings with it. Recorded because the oracle's *silence* over those 700 lines would have been read
as "under the bound".

Gate at this head: `npm test` 1553 + 280 green on Node 24.16.0 and 22.23.1; typecheck, lint,
markdownlint and `semgrep scan --config auto` clean; lizard warning set differenced against
`85d48437` adds nothing.

### E9 follow-up — the eighth bot round

Two P2s, one taken and one declined, and both are about a boundary this PR drew rather than about
new code.

**Taken: a bare `oboete setup` said nothing about an unusable chain.** The seventh round's reporting
only ran when `--provider` named a destination, which was my scope line and it was drawn in the
wrong place: the entry that stops `resolveModel` takes the *stored* primary down with it, and
nothing else in the report names it — the consent display lists the targets of an admitted chain,
and an unusable chain has none. `selectedDestination` now takes the whole `Options` and looks on
every run except `--remove`, which is the recovery path. Refusal is unchanged and still only for a
`--provider` that widens egress. Red before the fix: a bare `--accept-egress` over a modelless
`ollama` entry printed the consent tuple, the credential steps and the agent table, and never the
entry; the same test pins that `--remove` stays silent.

**Declined: stop the chain when a target's retry fails after it already answered.** The mechanism is
real — `agentCliResultOutcome` and `providerTextOutcome` both return `null` for an unusable first
answer so the loop retries, and a second attempt that dies in transit makes the settled reason
`timeout` or `unreachable`, which advances. But that is the right row. "Advance and stop" is read
from the reason the target settles with, and "no answer from this host" is what happened: the host
gave one unusable answer and then nothing. A later target can plainly improve on a dropped
connection, and stopping instead would strand the batch on a transport error while an admitted local
target sat unused — which is what US7 scenario 5 asks the chain to prevent. The evidence the stop
rule is about is *two* unusable answers, which is exactly the case `summarizeWithProvider` reports as
`unusable_output`. The proposed fix also needs cross-attempt state the contract does not define
("preserve that an answer was received"), and `CallOutcome` carries one reason by design — the same
objection that declined the fourth round's stopped-attempt line and the fifth round's success line.

Both decisions are now in the contract rather than only here: "Advance and stop" states that the
column is read from the settled reason, and Verification 18 states that a bare setup names the chain
error too. The previous rounds' findings came back because the contract said only what setup
refuses and only which reasons stop, leaving the rest to be inferred.

Gate at this head: `npm test` 1554 + 280 green on Node 24.16.0 and 22.23.1; typecheck, lint,
markdownlint and `semgrep scan --config auto` clean; lizard warning set differenced against
`85d48437` adds nothing.

### E9 follow-up — the ninth bot round

Two P2s, both taken, and both are the same shape as earlier rounds one surface further out: a report
that displays the chain and then describes the primary as the whole provider configuration.

- **The reserved band kept the refused-primary consequence.** `allowanceClause` was added in the
  fifth round precisely so 140–149 calls could say something true — end-of-session summaries still
  run — and `providerCapItem` then used only its `reason`, passing the spent state's consequence and
  re-implementing its `recovery` inline in different words. So at 145 calls the `provider` item said
  processing waits (or that the chain takes the batch) while `reserveAttempt` still grants a
  `session_end` batch that very preset. `refusedPrimaryConsequence` gained a default-argument so the
  band can pass the clause's own sentence as the unchained case, and the inline recovery is gone;
  both cap states now quote the clause the `allowance` item quotes.
- **`credentialGuidance` said "written by rule alone" over a usable chain.** With a `workers-ai`
  primary and no `OBOETE_CF_API_TOKEN`, the same report displayed an admitted `ollama` target and
  then told the user memories come from the rules — while Verification 15 pins that the worker
  advances past the primary's `no_provider` and applies that target's output. The doctor item was
  corrected for this in the fourth round; the setup report never was.

Red before each fix: the allowance table test's reserved row returned the waiting consequence and
the second copy of the recovery, and the new setup test's
`/fallback targets shown above are attempted instead/` did not match.

`npm test` on Node 22.23.1 failed once here with `a busy database spools inside the capture budget`
— "the hook took 493.6 ms" against the 300 ms budget — when it ran immediately after the Node 24.16.0
suite on the same machine. Alone it is green (1555 + 280, 0 fail, 2 skipped). Same load-only family
as issues #203 and #168, and as the CI flake filed as #243 in this PR.

Gate at this head: `npm test` green on Node 24.16.0 and 22.23.1; typecheck, lint, markdownlint and
`semgrep scan --config auto` clean; lizard warning set differenced against `85d48437` adds nothing.

### E9 follow-up — closing the shape instead of waiting for the tenth round

Rounds four, seven and nine each fixed one instance of the same defect: a report that predicts what
happens to a batch the primary cannot serve, without asking whether a target is admitted. Each round
found the next instance rather than the shape, so the shape was enumerated directly this time —
every user-facing sentence in `src/doctor/` and `src/setup/` that says where a batch goes:

| surface | state it predicts | closed in |
|---|---|---|
| `configuredProvider` | primary has no credentials | round 4 |
| `providerItem` post-probe | the probe call failed | round 7 |
| `providerCapItem` exhaustion / cap | the reservation is refused | rounds 4 and 9 |
| `credentialGuidance` (setup) | primary has no credentials | round 9 |
| `allowanceItem` | the shared daily allowance | **this sweep** |

`allowanceItem` was the one left, and structurally so: `allowanceEstimateItem` never received the
configuration, so it could not have consulted `admittedChain` even if it wanted to. It said
processing waits for the reset while an admitted `ollama` target summarizes those batches. The
chain-awareness moved into `allowanceClause` itself, which both readers already share, so
`providerCapItem`'s reserved branch lost the wrapper the ninth round put around it.

The chained sentence says the batch is **offered** to the chain rather than summarized by it,
because `DAILY_CAP` is shared across capped presets: a capped target refuses at its own reservation
with its own `daily_cap`, and only an uncapped target answers. Which targets those are is
`fallback:N`'s to report, and it already does.

Both directions are pinned per band, in one test each, against the same database: with an admitted
target and with none.

Everything else in that table was re-read rather than assumed; `fallbackAllowanceItem` speaks about
one target rather than about the batch, so it is not in the family.

Gate at this head: `npm test` 1557 + 280 green on Node 24.16.0 and 22.23.1; typecheck, lint,
markdownlint and `semgrep scan --config auto` clean; lizard warning set differenced against
`85d48437` adds nothing.

### E9 follow-up — the tenth bot round, including two the sweep missed

Three P2s, all taken. Two of them are instances of the shape the sweep above claimed to have closed,
which is the sweep's own lesson: enumerating the *surfaces* was not enough, because one surface had
a branch the enumeration did not open and another had its sentence in a constant.

- **`CHAINED_CONSEQUENCE` promised the chain would summarize.** Admission is not runnability: a
  `workers-ai` primary with no credentials followed by a `nim` target with no credentials admits a
  target that also answers `no_provider`, after which the batch is rule-based. The sentence the
  sweep wrote for the allowance says the batch is *offered* to the chain for exactly this reason;
  the constant written three rounds earlier still said "summarized by". It says "offered" now, and
  every target's own verdict stays `fallback:N`'s to report.
- **The exhaustion branch of `allowanceEstimateItem` still hard-coded `ALLOWANCE_CONSEQUENCE`.** The
  sweep made the *cap* branch of that function chain-aware and left the branch immediately above it
  alone. `exhausted_at` is per preset, so the chain's next target is unaffected and the worker
  advances past `provider_exhausted` — the allowance item was the only surface still saying the
  queue waits.
- **A target whose answer `applyObservations` refuses had no attempt line.** A required progress
  decision the detector rejects mints `unusable_output` *after* the provider call, so — unlike the
  stopped-pass case declined in the fourth round — no reservation row ties that reason to a target.
  The log therefore named every target that failed to answer and then a batch reason nothing
  accounted for. `processBatch` remembers which target answered and appends its line when the apply
  refuses it. Red before the fix, with the ollama line absent:

  ```text
  provider attempt … position=0 preset=workers-ai … reason=provider_exhausted
  batch … state=fallback reason=unusable_output
  ```

Gate at this head: `npm test` 1559 + 280 green on Node 24.16.0 and 22.23.1; typecheck, lint,
markdownlint and `semgrep scan --config auto` clean; lizard warning set differenced against
`85d48437` adds nothing.

### E9 follow-up — the same sweep on the axis the tenth round exposed

The tenth round found two instances of a shape the sweep before it had declared closed, and the
reason was in how the sweep looked: it enumerated **functions**, and the two it missed were a
*branch* inside one of them and a *constant* read by another. Re-running the sweep along those two
axes — every `degraded`/`warning`/`unverified` construction in `src/doctor/provider.ts` and every
string in `src/doctor/` and `src/setup/` that claims a batch becomes rule-based — found three more,
none of them in a function the first sweep had listed:

- **`fallback:N` for a target the cost policy excludes** said "a failure ahead of it falls through to
  rule-based records". With `[nim (excluded), ollama (admitted)]` a failure ahead of it passes to
  `ollama`; the sentence is only true when the policy admits nothing at all.
- **The `catalog` item for a model the catalog does not list** said summaries fall back to
  rule-based. That call fails with `model_alias`, which advances.
- **The `catalog` warning for a paid-only model** said the same about `provider_paid`, which also
  advances.

All three were verified against the "Advance and stop" table rather than against the code, and all
three keep their original sentence for the case that makes it true — no admitted target.

Three axes have now produced instances of this one shape: the surface, the branch within a surface,
and the constant a surface reads. The reusable form is [[defect-shape-closure-needs-second-axis]]:
enumerate the *claims*, not the code that makes them.

The `ponytail-review` pass over the same range then found the reason two of the three had survived:
both were **open-coded copies of `refusedPrimaryConsequence`** — the same
`admittedChain(config).targets.length > 0` test written out at the call site — and one of them read
its chained sentence from a module constant far from the call. That is the drift vector, not just
duplication: the constant held the wrong wording for three rounds while the function beside it was
"fixed" twice. Both call sites use the helper now, the constant is gone, and one occurrence of the
test remains in the file.

Gate at this head: `npm test` 1562 + 280 green on Node 24.16.0 and 22.23.1; typecheck, lint,
markdownlint and `semgrep scan --config auto` clean; lizard warning set differenced against
`85d48437` adds nothing. CodeRabbit reviewed `b2fda4c1` with no findings.

### E9 follow-up — the eleventh bot round, on the helper itself

One P2, taken, and it is the shape turned inward: `refusedPrimaryConsequence` tested chain
*admission* and nothing else, so a primary the **resolver** refuses — `workers-ai` with a
whitespace-only `[observer] model` and a valid `ollama` entry — still got the chained sentence, while
`resolveObserveModel` turns that throw into a run with no model *and no targets*. `providerItem` and
`fallbackItems` notice it first through `resolvedObserver`; `allowanceItem` and the catalog items
have no such step, which is exactly the two the previous two commits had just wired to the helper.

The test went into the helper rather than into those two callers
([[guard-belongs-in-the-shared-function-not-one-caller]]): one guard in the function all of them
share is smaller than two, and a third caller added later inherits it. `resolveModel` throws only
for a chain admission already emptied and for a primary with no model of its own — **absent
credentials are not a resolve error** — so the fourth round's uncredentialed primary still says the
chain is offered the batch, which its own test re-confirms.

Red before the fix: `allowanceItem` returned "Batches are offered to the fallback chain below…" for
a configuration whose observer cannot start at all. One test pins both surfaces at once, because
both were wrong for the same reason.

Gate at this head: `npm test` 1563 + 280 green on Node 24.16.0 and 22.23.1; typecheck, lint,
markdownlint and `semgrep scan --config auto` clean; lizard warning set differenced against
`85d48437` adds nothing.

### E9 follow-up — the twelfth bot round

Two P2s, both taken, and the first is the previous round's own fix not reaching its sibling.

- **`credentialGuidance` kept the admission-only test** the doctor helper had just stopped using, so
  `oboete setup` still promised the chain for a primary the resolver refuses. The fix is not a second
  copy of the guard: `chainIsReachable(config)` now lives in `src/observer/providers.ts` beside
  `resolveModel`, which applies both tests in one expression (`resolveModel(config).chain.length > 0`),
  and the doctor helper and the setup guidance are its two readers. That is what the eleventh round's
  fix should have been — putting the guard inside `refusedPrimaryConsequence` closed doctor and left
  setup, because the shared thing was the *question*, not the doctor's sentence.
- **A stop was missed when the next target had no credentials.** `summarizeWithProvider` answers
  `no_provider` for an uncredentialed target *before* it asks whether consent still holds, so a chain
  that ended on such a target kept an earlier target's reason by precedence — an `auth_failed`
  primary followed by an uncredentialed `nim` degraded with `auth_failed`, sending the user to fix a
  credential when consent was what they had to act on. The contract lists `currentConsent()` as step
  2 of "The attempt sequence", before the reservation and the call, and it is now the loop's own
  check rather than something delegated to `providerCall`.

Red before the second fix: `degraded_reason: 'auth_failed'` where the contract's "the reason a stop
ended the chain on outranks the precedence order" requires `consent_changed`. The existing test for
that rule passes with a *credentialed* final target, which is why the rule looked pinned.

Gate at this head: `npm test` 1564 + 280 green on Node 24.16.0 and 22.23.1; typecheck, lint,
markdownlint and `semgrep scan --config auto` clean; lizard warning set differenced against
`85d48437` adds nothing.

### E9 follow-up — the thirteenth bot round

One P2, taken, with its stated cause corrected: consent was missing from the reachability question.
With a stored record that no longer matches the tuple, `consent_changed` stops the chain before any
target is reached — one hash covers the primary and the whole chain — while the credential, cap,
allowance and catalog items all said the batch is offered to the fallback chain.

The finding attributed this to the twelfth round's new `currentConsent()` check in the loop. It is
not: `providerCall` already passed `consentOk` into `summarizeWithProvider`, which calls it inside
`prepareProviderReservation`, so a credentialed primary was refused on consent before that change
too. The contradiction is pre-existing; the finding is right and only its mechanism was misread.
Recorded because a fix committed under a wrong cause is the failure mode
[[revert-rationale-must-name-a-verified-mechanism]] names.

`chainIsReachable(config, env)` takes consent now, which is where the three tests it has to satisfy
belong together: the resolver refuses a primary with no model, admission empties a chain the policy
cannot use, and consent authorizes all of it or none. Threading `env` reached `allowanceItem`, which
had never needed it — the parameter is the price of the item being able to answer a question about
authorization at all.

**Four fixtures asserted the chained sentence for configurations the worker would have refused**,
because `configSchema.parse({ observer: { preset: 'workers-ai', … } })` stores no consent record and
`consentMatches` refuses a remote preset without one (R8). They now build their consent with
`consented()`, and a new test pins the other direction: the same chain with `hash: 'not-the-tuple'`
gets the waiting sentence, and with a matching hash gets the handoff.

`npm test` on Node 22.23.1 failed once here with `ENOTEMPTY: directory not empty, rmdir
'…/work/.git'` in `staleness.test.ts` teardown — issue #206, the documented teardown race. Green on
the rerun.

Gate at this head: `npm test` 1565 + 280 green on Node 24.16.0 and 22.23.1; typecheck, lint,
markdownlint and `semgrep scan --config auto` clean; lizard warning set differenced against
`85d48437` adds nothing.

### E9 follow-up: the whole-pull-request correctness review, and what the bot rounds did not reach

Thirteen bot rounds had cleared `d78ea756` and the `ponytail-review` pass over `main...HEAD` had
returned "lean", so the pre-merge gate's remaining item was a correctness review with `ok: true` on
the final head. A `/code-review` pass over the **whole** pull request — not the round's delta —
returned fourteen findings. Ten were adopted, four declined with a mechanism. The receipts:

| # | Finding | Disposition |
|---|---|---|
| 1 | `catalog` was pushed after the chain items, so three consequences that say "the fallback chain below" printed above it | adopted: `doctor.ts` order, plus an assertion that reads the report's own item names |
| 2 | Nothing names the target that answered | declined: Verification 13 asks for one line per **failed** target |
| 3 | A fallback entry on a preset with no `defaultModel` refuses the whole chain | declined: the refusal is the contract's; the arbitrary half is `agent-cli`'s and now named in #241 |
| 4 | `oboete setup --provider none` over a chain reports nothing and leaves a configuration `resolveModel` always refuses | adopted |
| 5 | A throw inside `processBatch` discards every attempt line already recorded | adopted: the caller owns the array |
| 6 | An `ollama` target was reported "ready" with nothing checked behind it | adopted: `credential.kind === 'none'` answers `unverified`, as `agent-login` beside it already did |
| 7 | `chainTargets`' egress test restated as "not wider than the label" | declined: that admits an `egress: 'none'` target the consent tuple authorizes for no classes at all |
| 8 | `presetExhaustedAt` prepares its statement per call | adopted: the repository's own `prepared(db, sql)` cache |
| 9 | The loop's consent check as amplification | declined: one extra evaluation on the path where the primary answers; it only repeats after two targets have failed |
| 10 | `providerCapItem` builds a consent hash on the path that discards it | adopted |
| 11 | Four places each derived "its own model, else the preset's default, trimmed" | adopted: `targetModel` is the one reader |
| 12 | The consent screen prints the primary's classes for a target while the hash binds the target's own | **adopted, then reverted**: the contract's "Consent coverage" decides it, and states why the two differ. The defect was the `consent.ts` header, written before the chain existed, which claimed the hash binds the tuple shown. The header now names the one field that differs, and `config.test.ts` pins the asymmetry in both directions |
| 13 | `?? 'excluded'` under `verdicts[index]` | adopted: it could only mislabel a target as excluded by a policy that would not help it |
| 14 | `CHAIN_STOPS` holds two of the contract's three stop rows | adopted: the doc block now says why `language_mismatch` is not one |

Three of the ten — 10, 11 and 13 — are `ponytail-review`-shaped, and the ponytail pass over the same
range had missed them: it enumerated the abstractions the diff **adds** and checked each for a second
caller, which is blind to work done on a path that discards it, to an expression duplicated across
two files, and to a defaulting operator that cannot fire. The lesson is the same shape as
`enumerate-the-claims-not-the-code-that-makes-them`, one axis further out.

Finding 12 is the one to read twice. The reviewer's reasoning was sound and the fix passed its RED
test; it was still wrong, because `contracts/provider-fallback.md` "Consent coverage" had already
decided the question in the other direction with a reason ("they describe the destination, which is
what consent binds"). The test that "failed" was the contract's own pin. Adopting it would have
narrowed what a stored consent record covers for every chained install.

**Scope of the `ok: true` that follows.** A second whole-pull-request pass is not the gate: 3,500
lines at high effort returns twelve to thirteen findings every time
(`whole-pr-rereview-does-not-converge`), and this pass is the record of the whole diff having been
read. The `ok: true` is taken on the fix delta.

### E9 follow-up — the fix delta's own review, and the shape it kept finding

The `ok: true` for the merge gate was taken on `de99b2ed..1e57b3bc`, the delta of the round above
rather than the pull request again. It returned nine findings, and the two most severe were both the
same shape: a fix that closed the path it was pointed at and left a sibling open.

- The attempt array moved to the caller so a throw could not drop it — and `checkpointBatch`
  rethrows a storage error *after* the `try`, so `logBatch()` never ran and the lines were dropped
  anyway. `logBatch()` is now in a `finally`.
- "Do not call a target ready" returned `unverified` before `fallbackAllowanceItem` ran, so an
  `ollama` target that reported exhaustion today — which `reserveAttempt` refuses on the stamp
  *before* it looks at `capped` — was reported as merely unchecked, and the `db === null` branch was
  skipped with it. The allowance verdict is read first now and only its `healthy` answer is
  downgraded, by one `unverifiableTarget` test rather than a branch per preset. That also closes the
  same swallow for `agent-cli`, which had it before this pull request.

The rest: `allowanceClause` took the spent band's consequence as the caller's thunk, so finding 10's
discarded work is gone rather than moved; `ChainResult` became a union whose `outcome` exists only
on the variant that has an answer, with `answered` null on the other, so the pairing a comment
carried is the type's; four comments and
a docstring that justified a sentence by the target being reported `healthy` were swept, since local
targets no longer are; and the `notDeepEqual` pin added the round before came out — it was implied by
the `deepEqual` above it and false in general, because a local primary admits only local targets and
then the two agree.

Declined: that reporting an orphaned chain under `--provider none` is a symptom fix and setup should
strip the entries instead. Verification 18 has that run succeed, the note names the two commands that
resolve it, and setup does not delete configuration the user wrote anywhere else. The test now pins
that the entries survive the run.

`fallbackTargetItem` crossed the length bound at 52 NLOC while that restructure happened, which the
warning **set** differenced against `85d48437` caught and the count would not have: the same run
dropped `fallbackReason` and `writeConfig`. `unadmittedEntryItem` took the two verdicts that need no
storage read.

Gate at this head: `npm test` 1566 + 280 green on Node 24.16.0 and 22.23.1; typecheck, lint and
markdownlint clean; `semgrep scan --config auto` unchanged against the round-12 baseline; the lizard
warning set differenced against `85d48437` adds nothing and drops two.

### E9 follow-up — the second delta review, and where it stopped

`1e57b3bc..3239ec08` returned nine findings, eight adopted. Two were regressions the round before
introduced, both in the same eleven lines:

- `logBatch()` in a `finally` can itself throw — `appendLog` writes to a file in the same directory
  as the database, so the ENOSPC or EROFS that made the checkpoint fail makes the log write fail
  too, and a throw from a `finally` replaces the error being reported. `recordRunFailure` would have
  recorded the log write's code instead of the `SQLITE_*` one.
- That same `finally` wrote `level=info state=applied` for a pass that did not finish. The *absence*
  of the batch line is what said so.

Both are closed by a `catch` that writes only the attempt lines, inside its own `try`, and rethrows.

The rest were the shape of the fixes rather than their effect: the `db === null` branch had moved
ahead of the local-target check and told an uncapped `ollama` target to wait for an allowance it
never spends; `fallbackAllowanceItem` returns `DoctorItem | null` now, so nothing builds a
`… and ready.` sentence to throw away and nothing reads a status string back out of an item to
decide; an uncapped target no longer reads the shared allowance at all, and `cappedCalls` goes
through the `prepared` cache; `unadmittedEntryItem` takes the catalog entry its caller already had;
and the `ChainResult` doc block named the wrong exclusive member — it is `outcome`, since `answered`
is on both non-`done` variants and the caller discriminates on `answered === null`.

Declined: replacing the `spentConsequence` thunk with an object of the two literals. Equivalent, and
the churn buys nothing.

**Not pinned, and filed as #245.** The attempt lines' survival of a storage throw has no test.
`src/testing/faults.ts` forbids a seam for this class outright — "a missing, corrupt or read-only
database ... is induced for real by the test" — and inducing it for real from the answering target's
fetch handler does not work: `chmod 0o400` does not revoke the write descriptor the worker already
holds, and the run finishes `exit=0 applied=1` (measured). The issue names the one approach that
would work, a competing `BEGIN IMMEDIATE` held past `retryBusy`'s budget.

This is where the delta reviews stop. The first returned fourteen findings over the whole pull
request, the second nine over its fixes, this one nine over those — but the two that mattered here
were both in eleven lines written the round before, and the remaining seven were shape rather than
behaviour. The next pass would review eleven more lines of error handling, which is the regress
`whole-pr-rereview-does-not-converge` describes from the other end.

Gate at this head: `npm test` 1566 + 280 green on Node 24.16.0 and 22.23.1; typecheck, lint and
markdownlint clean; `semgrep scan --config auto` unchanged against the round-12 baseline; the lizard
warning set differenced against `85d48437` adds nothing and drops two.

### E9 follow-up — the two bot findings on the fix rounds

CodeRabbit and the Codex connector each found one thing in the four fix commits, and both were
real.

The chain is ordered, so an excluded entry's consequence — "a failure ahead of it passes to the
targets the policy does admit" — is true only when an admitted target comes **after** it.
`chainIsReachable` answers for the whole chain, so with an admitted `ollama` at position 1 and an
excluded `nim` at position 2 the report promised a handoff that cannot happen: by the time the chain
is at 2, the target at 1 has already had its turn and failed. `fallbackItems` passes
`admittedAfter` now, and the test that exercises that exact configuration asserts the rule-based
sentence for both excluded entries.

The language-mismatch attempt line was written after `applyFallback` rather than before it, so a
target that had just spent two allowances lost its line whenever that call came back `lease_lost` or
threw. It is written where the reason is decided now, and the delayed append in `attemptTargets` is
gone — it only ever fired for this one reason.

That second one is **not pinned**, and #245 carries the measurement: staging a lease theft from the
answering target's second `fetch` lands *before* `recordProviderResult`, so the run returns
`lease_lost` at that check and the language comparison never runs. No line is owed on that path
either — the target answered and the pass lost its lease, so no `DegradedReason` was decided. The
gap that needs a seam is narrower: a failure inside `applyFallback` itself, where the harness has no
`fetch` to hang a fault on.

One load-only failure on Node 22.23.1 in this round's full run: `ENOTEMPTY ... rmdir
'…/workspace/.git'` in `memory-scope.test.ts`, the teardown race of issue #206. Green on the
isolated rerun, 39 of 39.

Gate at this head: `npm test` 1566 + 280 green on Node 24.16.0 and 22.23.1 (the latter after the
isolated rerun of the flake above); typecheck, lint and markdownlint clean; `semgrep scan
--config auto` unchanged against the round-12 baseline; the lizard warning set differenced against
`85d48437` adds nothing and drops two.

### E9 follow-up — the round that corrected the round before it

`3239ec08..85cb4669` returned nine findings, seven adopted, and two of them were corrections of what
the *previous* review round had asked for. That is the useful part of this receipt.

- The `catch` that replaced the `finally` omitted the batch line unconditionally. Its reason —
  a `state=applied` line would claim a pass that did not finish — is true only when the pass had no
  error of its own. When `processBatch` had already thrown a non-storage error and
  `checkpointBatch` then threw a storage one, the first error's code was written nowhere:
  `recordRunFailure` reports the second. The catch now writes the batch line when `batchError` is
  set and the attempt lines otherwise.
- Putting the "nothing here checks this" item ahead of `dbUnread` — adopted one round earlier so an
  uncapped `ollama` target would not be told to wait for an allowance — suppressed the
  integrity-check message and told a user with a corrupt database to start a local model server.
  `dbUnread` is the only path that carries that message. Storage is the blocker when there is none,
  so it is reported; the sentence names the **provider usage record** rather than an "allowance",
  which is what makes it true for an uncapped target: `reserveAttempt` reads `presetExhaustedAt`
  above `capped`. The branch now has a test, which it did not before — `fallbackItems` was only ever
  called with an open database in the suite.

The rest: `logAttempts` uses the repository's own `appendLogQuietly` per line instead of a
hand-rolled swallow around the whole loop, so one append that cannot land no longer takes the
remaining targets' lines; the excluded entry's sentence was rewritten, because "a failure ahead of
it falls through to rule-based records" reads as *any* earlier failure and the primary's failure
does reach an admitted target at position 1; that consequence is now the one boolean it always was,
`admittedAfter && chainIsReachable(...)`, rather than a ternary wrapped around a helper whose
subject is a refused primary; the closure left with one call site was inlined; and a stray blank
line went.

Declined: rewriting `chain.verdicts.slice(index + 1).includes('admitted')` as a single reverse pass.
`fallback` is bounded at three entries by the schema, and the slice reads as the invariant it
checks.

Acknowledged, already filed: the attempt lines surviving a storage throw is still unpinned (#245),
so deleting the `catch` and restoring `finally { logBatch(); }` leaves the suite green.

**What the three fix rounds actually show.** Fourteen findings over the pull request, then nine over
the fixes, then nine over those — but the second and third rounds each found two real defects *in
the eleven lines the round before wrote*, and both times in the same error-handling block. The
pattern is not that the reviews are not converging; it is that this block has three exit shapes
(a stop, a normal end, a throw) and two log kinds, and each round fixed the pair it was pointed at.
It is now written as one statement of all six cases rather than a patch on the last patch.

Gate at this head: `npm test` 1567 + 280 green on Node 24.16.0 and 22.23.1; typecheck, lint and
markdownlint clean; `semgrep scan --config auto` unchanged against the round-12 baseline; the lizard
warning set differenced against `85d48437` adds nothing and drops two.

### E9 follow-up — restating the block instead of patching it again

`85cb4669..daa878d0` returned twelve findings, and the last one named what the previous three rounds
had been doing: each fixed the case it was pointed at and added a branch to the same eleven lines.
Two of the twelve were regressions from the round immediately before, which is the third time in a
row. `review-rounds-narrowing-means-wrong-design` is the memory for that, and it says to stop
patching and restate.

What the block actually has to say is two independent facts, and every round so far had been
encoding them as one:

- **what the batch reached** — `state` and `reason`, which are the batch's and are true even when
  the pass then fails, because `applyObservations` has already committed the row;
- **how the pass ended** — `error` for a failure inside `processBatch`, `pass` for one out of
  `checkpointBatch`.

So the batch line is now written for every batch, always, with those as separate fields, and the
"a missing line means the pass did not finish" convention is gone. It was never writable and it was
already wrong: a committed `applied` batch whose checkpoint then failed left no line at all, and a
batch whose destination is `fallback` — which returns before any target is attempted, so `attempts`
is empty — wrote nothing whatsoever for a batch the database says it applied.

Three regressions this fixes, all introduced by the two rounds before it:

- `appendLogQuietly` in `logAttempts` removed the escalation of a log-write failure to exit 3. That
  is `FR-002`'s rule for the hook, and the opposite of the worker's: on the stop path `logAttempts`
  is the *only* log call, so a full disk became an exit 0. It is `appendLog` again, and the one
  place a log failure must not win — while a storage error is already in flight — is stated where
  that is true rather than by making every write quiet.
- `reason: batchResult?.reason ?? errorCode(batchError)` let a batch that settled on a reason of its
  own hide the code of whatever failed after it. `error` is its own field now.
- Routing every `db === null` chain item through `dbUnread` lost the target's name: the integrity
  substitution replaces the whole sentence, so `fallback:1` and `fallback:2` printed the same line
  and named no position, preset or model — against "Diagnostics", which asks for all three.
  `dbUnread` takes a `subject` now, the test pins it, and the recovery follows the failure: only a
  corrupt database is a repair, while a missing, unwritable or schema-behind one is the `storage`
  item's own business.

Also: the excluded entry's consequence has three cases rather than two, because "no admitted target
after it" and "the chain is not runnable at all" are different, and the second must not imply that
adding a cost class would help — with a stale consent record the `consent` item is the one to act
on. And `refusedPrimaryConsequence`'s doc block no longer claims `fallbackItems` reaches it.

**Declined, filed as #246:** the `catch` around `processBatch` has no `isStorageError` rethrow, while
`checkpointBatch` and `summarizeSession` both do — so the same error class exits 0 from one and 3
from the other. It predates this pull request, and the same `catch` also receives busy errors that
have already exhausted `retryBusy`, for which yielding may be right; splitting those is its own
change.

**Measured, and worth writing down:** `openObserveDatabase` left the lizard warning set this round
without anyone touching it. It is a parser artefact, not a fix — lizard reports its span as
`@374-798` (425 lines) where the function is 23 NLOC, and the round before reported `@374-1023`
(650 lines). The differenced set is still the right gate, and it still adds nothing; but a warning
that *disappears* has to be counted by hand before it is called fixed
(`silent-oracle-is-not-under-the-bound`).

One load-only failure on the Node 22.23.1 full run: `ENOTEMPTY ... rmdir
'…/work/.git/ai/working_logs'` in `staleness.test.ts`, issue #206's teardown race. Green on the
isolated rerun, 6 of 6.

Gate at this head: `npm test` 1567 + 280 green on Node 24.16.0 and 22.23.1 (the latter after that
rerun); typecheck, lint and markdownlint clean; `semgrep scan --config auto` unchanged against the
round-12 baseline; the lizard warning set differenced against `85d48437` adds nothing.

### E9 follow-up — the restatement's own review, and the contract it had to retire

`daa878d0..37533dc2` returned eleven findings, nine adopted, two filed. The restatement was right
about the shape and wrong in three details, two of which a reviewer confirmed by running the built
`fallbackItems` against a stale-consent configuration rather than by reading it.

- The new third branch pointed the user at a **`consent` doctor item that does not exist**. The
  report's items are config, storage, fts, migration, worker, generation, spool, sync, provider,
  allowance, catalog, `fallback:N`, `agent:*`, unrecognized-agents, pi and paused — there is no
  consent item, and without `--probe-provider` the provider item says only "not probed this run".
  Worse, the branch's *recovery* was never changed at all, so the item still told the user to add a
  cost class that would change nothing. The branch fires if and only if `consentMatches` is false —
  `resolvedObserver` has already returned on a resolver refusal, and `admittedAfter` implies the
  chain is non-empty — so it names that, and recovers with `oboete setup --accept-egress`.
- Writing the attempt lines with the **throwing** writer put the worker-stop sentinel at risk. A
  pass that stops calls `logAttempts` and returns; a throw there reaches `recordRunFailure`, which
  ends the run as `storage_error` rather than `stopped`, and `releaseForExit` clears the sentinel
  only for `stopped`. A full disk during a stop would have left the sentinel behind: the stop the
  user asked for is reported as a storage failure, and the next resident spends its whole run
  reading the marker at startup, exiting `stopped` and clearing it — one run lost, not a worker that
  never runs again. The attempt lines are written quietly again; the
  **batch** line keeps the throwing writer, and it is the one that escalates — `EACCES` and `ENOSPC`
  are `isStorageError`. That is also what makes one failed attempt append no longer take the batch
  line with it.
- `state=error reason=none` at level `info` for a batch lost to `LeaseLostError`, because
  `processBatch` throws instead of returning the `lease_lost` state it has. The fallback pair reads
  `leaseLost` now. And `errorCode(batchError)` was printed twice, as `reason` and again as `error` —
  `reason` belongs to the batch and is `none` when the batch never settled on one.

The doc block's "one line per batch, always" was also false: a pass that stops writes its attempt
lines and no batch line, which the contract states at "Diagnostics". The comment names that
exception now.

**The contract was the thing to retire.** `contracts/provider-fallback.md` still described the log
as "one `provider attempt` line per target … then the existing degraded line for the batch" and
still asserted that "the `consent` item is where a mismatch is reported". The quickstart's narrative
does not retire normative text — `new-decision-doc-must-retire-old-statements` — so both sections
were rewritten: the batch line's `error`/`pass` split and which writer each line uses, and the fact
that no `consent` item exists together with where a mismatch *is* named.

**Now pinned:** `test/unit/provider-fallback.test.ts` makes the observe log unwritable from inside
the answering target's fetch handler and asserts the run exits 3. That is the escalation this round
restored, and it was the review's own finding that it had no test.

**Filed rather than fixed:** #247 — `dbUnread` branches its reason on `integrityFailed` but not its
recovery, so eight other doctor items still tell the user to repair a database that is merely
missing, unwritable or behind the schema. Fixing it there is right and belongs in its own change.
Also declined here and already filed: #246.

Gate at this head: `npm test` 1568 + 280 green on Node 24.16.0 and 22.23.1; typecheck, lint and
markdownlint clean; `semgrep scan --config auto` unchanged against the round-12 baseline; the lizard
warning set differenced against `85d48437` adds nothing.

### E9 follow-up — the consent check moves to the seam that already collapses the chain

`37533dc2..663d1be0` returned fourteen findings; thirteen adopted, one declined and filed. The top
one is the round before's own fix placed at the wrong altitude, which is the shape
`patched-handler-needs-restating-not-another-branch` describes.

**The consent check was in one entry's verdict, and belongs at the chain's seam.** The previous
round added it to the *excluded* branch of `unadmittedEntryItem`, so a stale consent hash still let
an admitted target report `healthy … admitted as remote and ready` — the report's largest untruth,
because no target is attempted at all. One hash covers the primary and the whole chain, so the test
belongs where `fallbackItems` already collapses the chain into a single item on a resolver refusal.
The guard there replaces the third branch, its second spelling of `chainIsReachable`, the
three-case consequence and the two-case recovery, and takes `config` and `env` off
`unadmittedEntryItem` entirely. The file is +29/−27: the branch is gone and the reasoning it had
spread across two verdicts is now stated once, next to the guard.

**Now pinned** (`test/unit/doctor.test.ts`): a stale record with two configured entries yields
exactly `['fallback']` — RED without the guard on `actual: ['fallback:1', 'fallback:2']` — and a
matching record still yields `['fallback:1', 'fallback:2']`, so the guard cannot pass by refusing
everything.

**A behaviour change worth naming:** the collapsed item is `degraded`, and only degraded items move
the exit, so `oboete doctor` now exits 1 for a stale record with a chain configured. Measured both
ways on the same fixture: before the guard, exit 0 with no degraded item at all — a report that
passed while no summary would ever be written. No `--probe-provider` is involved either way; the new
CLI test pins both directions.

**The unwritable-log test asserted the wrong thing.** `exit === 3` does not discriminate: `logEnd`
returns 3 when its own append fails, whatever the run reached. The same scenario with a writable log
was measured — exit 0, `last_run` reason `empty`, attempt lines present — so the test now asserts
what actually differs: the failing run records no `last_run` at all, because it throws out of the
batch line instead of draining the session-end summary behind it, and the attempt lines are absent,
which is the quiet writer doing its job. Two more from the same finding list: the Workers AI primary
is stubbed with a throwing handler rather than left to `chainFetch`'s `assert.fail`, which the
worker's own catch would swallow into an ordinary provider failure; and the test skips under root
like the other permission fixtures (`test/fault-storage.test.ts`).

**Two overstatements corrected.** The contract called `error` "a failure inside `processBatch`" and
`pass` "one out of the checkpoint after it". `pass` takes both conditions at once — a storage
failure *out of the checkpoint*, the one that ends the run — while `error` is anything out of
`processBatch`, storage or not, plus a non-storage checkpoint failure `checkpointBatch` carries on
past. And "a full disk
during a stop would leave every later resident refusing to run" was too strong — the next resident
reads the sentinel in `controlReason()` at startup, exits `stopped` without doing any work and
clears it there. The cost is one lost run, not a dead worker; corrected in the code comment, this
document and the contract. The contract's stop-path exception now sits beside the sentence it
qualifies rather than at the end of the bullet.

**Filed rather than fixed:** #248 — a stop that ends as `storage_error` leaves its sentinel behind,
because `releaseForExit` clears it only for `stopped`. Clearing it whenever `isWorkerStopped` holds
was the finding's own suggestion and was declined: it would swallow a stop request written *during*
an unrelated exit, which three existing tests pin.

**Caught by the oracle, not by the eye:** `fallbackItems` crossed 50 NLOC (51) once the guard was
added, and lizard's TypeScript reader made it worse by swallowing the `FallbackTarget` type alias
after it — the nested template literal in the new sentence is the same parser hazard as
`lizard-ts-parse-swallows-after-angle-compare`. The count is named in a local now, and the
chain-error branch moved out to `chainErrorItem`, which takes the function to 42.

### E9 follow-up — consent belongs where the worker reads it, not only where the chain is reported

The delta's own review returned seven findings; four adopted, one split into a fix and an issue,
two declined.

**The guard was still in one caller.** `[[observer.fallback]]` is empty by default, and
`fallbackItems` returns `[]` before reaching any guard when it is — so the case the previous round
called "a report that passed while no summary would ever be written" was still live for the majority
of configurations. The check moved to `configuredProvider`, which both the probe and the non-probe
path go through, at the position the worker uses: `initialProviderFailure` reads the preset, the
model and the credentials first, then consent. Same shape as
`guard-belongs-in-the-shared-function-not-one-caller`, one round later.

Three consequences worth naming. A stale record now answers **before** the probe, so `oboete doctor
--probe-provider` no longer spends a reservation on a call that could only come back
`consent_changed`. Three cap tests were passing configurations with no stored consent at all, which
the worker would refuse for consent rather than for the cap; they take a matching record now, so
they exercise the state they name. And the collapsed `fallback` item and the `provider` item now
carry the same consequence sentence, rather than one saying the batch is rule-based while the other
says processing waits.

**The chain item claimed an egress cause it cannot know.** An all-local chain whose hash drifted —
adding an entry changes the tuple — read "the configuration has not been accepted for egress" while
nothing in it leaves the machine. It names the record instead: "the stored consent record no longer
matches this configuration".

**The `error`/`pass` rule was rewritten in the wrong direction.** Last round called the split
"severity, not location". It is both: `pass` is a storage failure *out of the checkpoint*, and
everything out of `processBatch` lands in `error`, storage or not — the catch there has no
`isStorageError` test. Which surfaced a real inconsistency, filed as **#249**: a storage error
raised inside `processBatch` exits 0 while the same error out of the checkpoint exits 3.

**Declined.** (1) That the collapse hides the per-entry `covered`/`excluded` warnings and costs the
user a second `doctor` run — it does, and that is the point: no target is attempted, so no
per-target verdict is true, and the deleted combined recovery is exactly what the previous round's
top finding said to delete. (2) That `consentMatches` recomputes `admittedChain` and a SHA-256 the
function already has in hand — one hash in a one-shot CLI, against a branch that would have to be
threaded through `resolvedObserver`; `allowanceClause`'s thunk exists because it can skip the work
entirely, which this cannot.

**Also pinned:** the unwritable-log test asserts its own preconditions now (the primary was called
and failed, the batch is `applied` with two attempts), so its negative assertions cannot pass on a
run that stopped earlier.

Gate at this head: `npm test` 1571 + 280 green on Node 24.16.0 and 22.23.1; typecheck, lint and
markdownlint clean; `semgrep scan --config auto` 21 findings, unchanged; the lizard warning set
differenced against `85d48437` adds nothing — `configuredProvider` crossed at 57 NLOC when the
consent branch landed, and `uncredentialedPrimary` came out of it (44).

### E9 follow-up — two Codex threads the earlier polling had not seen

Both were `chatgpt-codex-connector` P2 inline comments, and both were missed the same way: the
polling read `pulls/238/comments` without `--paginate`, so a finding on page two looked like
silence. The connector's legend is explicit — it comments when it has suggestions and reacts 👍 only
when every review finishes with none — and the missing 👍 was the signal, not the reaction that
never came.

**Consent belongs ahead of the missing credential, not behind it.** When a remote primary has
neither a credential nor a matching consent record, the report named the credential and recommended
exporting the variable — and the worker does the opposite: `attemptTargets` asks `consentOk()`
before `providerCall` for exactly this reason, and its comment says so ("send the user to fix a
credential when consent is what they must act on", `src/worker/observe-batch.ts`). Adding the
credential would have left every batch stopping on `consent_changed`. Consent is tested first now;
one existing fixture had both problems and was pinning the wrong half, so it takes a matching
record, and a new test pins the order.

**A Workers AI chain target was called ready without consulting the catalog.** `catalogItems`
validates the *primary's* model and returns nothing at all when another preset is primary, so
`fallback:1 … admitted as free-tier and ready` was printed for a model the account does not serve.
`catalogTargetItem` now reads the same cached list under the same freshness rule, and says nothing
unless that list could refuse the model — see the round below, which corrected both the failure code
this item names and the states it was willing to speak in.

Gate at this head: `npm test` 1573 + 280 green on Node 24.16.0 and 22.23.1; typecheck, lint and
markdownlint clean; `semgrep scan --config auto` 21 findings, unchanged; the lizard warning set
differenced against `85d48437` adds nothing (`fallbackTargetItem` 39, `catalogTargetItem` 27,
`configuredProvider` 44).

### E9 follow-up — the catalog verdict was written for data that is never there

Twelve findings on the round above; eleven adopted, one declined, and the worker-side gap behind the
first one filed as #250. The two that matter are both about the check written to close a Codex
finding, which is the shape `fix-can-be-worse-in-another-dimension` warns about.

**The recovery it printed could never come true.** `refreshCatalog` fetches the Workers AI model
list only when Workers AI is the **primary** (`src/worker/observe.ts`), so the configuration the
check was written for — another preset primary, a `workers-ai` chain entry — never caches a list at
all. Every run would have printed `no fresh catalog is cached` with "run `oboete observe`, then
`oboete doctor` again", and the second run would print it again. The item is silent in every cache
state that cannot refuse a model now, and speaks only when a current list for this account omits it.
The worker-side gap is **#250**; when it closes, this item starts answering with no change of its
own.

**It named the wrong failure code.** An unserved Workers AI model returns a status
`classifyApiError` has no row for, so the attempt records `unreachable`; `model_alias` is a
*successful* call that answered with a different model id (`src/observer/llm.ts`). Corrected in the
item, the contract and this document.

**"Observer consent changed" for a record that never existed.** `consentMatches` also answers false
when `[consent] hash` is absent, which is every install that has not run `setup --accept-egress`
yet, so the first thing a half-configured install read was that a stored record no longer matched.
Two sentences now, chosen by whether a hash exists, shared by the `provider` item and the collapsed
`fallback` one. And the same item names the missing credential when both are missing — Codex's
original finding asked for the order *or* both recoveries, and taking only the order left
`initialProviderFailure` stamping `no_provider` on the batch while the report talked about consent.

**Also adopted:** the catalog check runs ahead of the allowance one, because an unlisted model is
wrong until the entry is edited while a spent allowance resets at midnight; `catalogIsStale` is one
function shared with the primary's item instead of a second copy of the same two clauses; the
credentials the caller already read are passed in rather than read again; `models(n)` makes it
"1 model"; the modified fixture uses the file's own `consented()` helper instead of inlining it; and
the healthy-case assertion binds its item once instead of building the same one twice.

**Declined.** Mirroring the primary's paid-plan warning onto every chain entry: `hasPaidOnlyModels`
is an account-level flag, not a property of the entry's model, so it would mark targets that are
entirely free — and it can only be true in configurations where the primary is Workers AI, where the
`catalog` item already says it once.

Gate at this head: `npm test` 1573 + 280 green on Node 24.16.0 and 22.23.1; typecheck, lint and
markdownlint clean; `semgrep scan --config auto` 21 findings, unchanged; the lizard warning set
differenced against `85d48437` adds nothing.

### E9 follow-up — the merge gate's own two blockers

`pr-merge-gatekeeper` returned **NO-GO** on the 7-item checklist with two blockers, both real and
both mine.

**SonarCloud had one new OPEN issue nobody had triaged.** `typescript:S6582` on
`src/doctor/provider.ts`, from the commit two before: `cache === null || cache.accountId !== …` is
the shape the rule asks to write as `cache?.accountId !== …`, which would stop narrowing `cache` for
the three reads below it. The null test is its own statement instead — not the shape the rule looks
for, and the narrowing survives. Reproduced locally with `@typescript-eslint/prefer-optional-chain`
(the rule behind S6582): it fires on the disjunction and is silent on the split.

**The recorded `ponytail-review` was six and a half hours stale.** It covered `main...de99b2ed`
(3582/204) while the head was 4808/242 — 14 commits and +1398/-210 unreviewed by that lens,
including this file's +408. Run over `de99b2ed..HEAD`, it found the delta lean apart from
`CHAIN_TAKES_THE_FAILURE` and `EXCLUDED_FALLS_THROUGH`, one caller each, now inlined in the ternary
that chose between them; `unverifiableTarget` moved to where its answer is read.

`/code-review` on the resulting delta returned **no correctness findings**, having reproduced each
claim rather than reading them: the inlined sentences are byte-identical to the constants, the moved
call is pure, and the rule dodge was verified against the plugin. Its six quality findings, three
adopted:

- the primary's catalog comment still said a model missing from the list fails with `model_alias`;
  it fails with `unreachable` for the same reason the chain's does, and `model_alias` needs a
  *successful* call that named another model;
- `catalogTargetItem`'s docstring justified its silence with "the worker replaces that cache on its
  next batch", which is false for the chain-only configuration the function exists for — nothing
  refreshes it there at all (#250);
- the two inlined sentences were asserted by substring regexes that skipped the clause the inlining
  had retyped by hand; both tests assert the whole sentence now.

Declined: sharing one `usableCatalog(db, accountId, now)` between the chain and primary paths — the
primary words a *different* item for each of the three states (absent, foreign account, stale) and a
helper that answers `cache | null` would take that distinction away, so the shared piece is the age
rule, which `catalogIsStale` already is. Also declined: enabling
`@typescript-eslint/prefer-optional-chain` repo-wide (28 other sites, its own cleanup), and reading
the catalog row once per report instead of once per entry (a one-shot CLI diagnostic).

### E9 follow-up — a refused configuration is not a consent problem

One P2 from the Codex connector on that head, adopted. The consent guard inside `attemptTargets`
ran ahead of every target, including the one `resolveObserveModel` builds from a configuration the
resolver refused — an empty model and an empty chain. With a stored record that no longer matched,
such a batch recorded `consent_changed` and the recovery said `setup --accept-egress`, which would
have changed nothing: accepting leaves the resolver error, and the next run fails `no_provider`.
`oboete doctor` names the resolver refusal first, so the report and the pack disagreed as well.

The first fix put the exception inside the target loop; the review of that commit put it where its
sibling already lives. `processBatch` answers `preset = "none"` with `no_provider` before any request
is built, and an empty model is the same state — so both are one condition now. That removes more
than the guard: the whole pipeline (`revalidateNearby`, the request build, a detector pass over the
serialized request, `markRequest`) no longer runs for a batch that cannot reach a provider, and no
`provider attempt` line names a target with an empty model. The catalog walk is skipped too — a
`workers-ai` primary under a refused configuration was still paginating `/models/search` with the
account token for a list nothing in the run could use, and the test's host assertion had excluded the
one counter that would have shown it.

RED before the fix on `['consent_changed'] !== ['no_provider']`, and RED again on
`catalog: 2 !== 0` once the test counted every host
(`a configuration the resolver refuses says no_provider, not consent_changed`).

Gate at this head: `npm test` 1574 + 280 green on Node 24.16.0 and 22.23.1; typecheck, lint and
markdownlint clean; `semgrep scan --config auto` 21 findings, unchanged; the lizard warning set
differenced against `85d48437` adds nothing; SonarCloud PR 238 back to **0** OPEN issues.

### E9 follow-up — checkpoint decisions in settlement order (#242)

One P2 from the Codex connector on `9cc50a2f`, the same defect the defensive review had filed
as issue #242 and left out of this pull request for want of a fixture. Adopted here, with the fixture.
`oboete work` and `oboete why` chose the latest checkpoint decision by `claimed_at`, which this pull
request made the reclaim fence: `reserveAttempt` restamps it with each attempt. A batch refused at
its own reservation (`daily_cap`, `provider_exhausted`) is never restamped, keeps its creation
stamp, and still settles after the batch ahead of it — with the `pending` decision `markRequest`
wrote — so `work` showed the earlier provider decision as the latest.

The connector's own mechanism does not occur as written: a `destination = 'fallback'` batch never
carries a checkpoint decision (`markRequest` runs only on the provider path, and `prepareCheckpoint`
returns null for any `fallbackReason`). The shape does, through the refused reservation above.

Both queries now order by `completed_at DESC, claimed_at DESC, id DESC`, the order #242 named.
`completed_at` is written in the same apply transaction as the settled decision
(`src/observer/apply.ts`), and SQLite sorts NULL last under DESC, so an in-flight decision falls
behind the settled ones. `reserveAttempt`'s docstring now says the column is the fence, not
settlement order. RED on both consumers before the change, from one seeded fixture with the two
stamps in the adverse order: `work` returned `'replace'` where `'pending'` settled last, and `why`
listed `['settled-first', 'settled-last']`
(`why and work order checkpoint decisions by settlement, not by the reclaim fence`).

Gate at this head: `npm test` 1575 + 280 green on Node 24.16.0 and 22.23.1, first run each, no
rerun; typecheck, lint and markdownlint clean; `semgrep scan --config auto` 0 findings in the four
touched files; the lizard warning set over the three touched source files is unchanged against
`9cc50a2f`. `github-advanced-security` still fails outside the required set, on its own model
(`CAPIError: 400 The requested model is not supported`), as on the previous heads.

Gate at the previous head: `npm test` 1570 + 280 green on Node 24.16.0 and 22.23.1, each after one
load-only rerun — `grok-no-tool` saw `pending` where `omitted` was expected (issue #243, the same
pair of states in the other direction; 6 of 6 on the isolated rerun), and `staleness.test.ts` hit
`ENOTEMPTY … rmdir '…/work/.git'` (issue #206; 6 of 6 isolated). Typecheck, lint and markdownlint
clean; `semgrep scan --config auto` 21 findings, unchanged against the round-12 baseline and none in
a touched file; the lizard warning set differenced against `85d48437` adds nothing (113 of the
base's 116, the three absences all counted in earlier rounds).

## E10 — platform checks on actual Linux, WSL and macOS (T040)

2026-09-17. The M1 iMac runbook (`docs/evidence/memory-core-2026-09/macos-runbook.md`) is replaced,
at the owner's direction, by a GitHub-hosted `macos-15` runner (macOS 15.7.9, arm64, 3 vCPU) through
`.github/workflows/platform.yml`: typecheck, lint, build, the unit suite, the serial end-to-end and
fault suites, pack-check, `scripts/measure-cold-start.mjs` and a no-model packed engine in an
isolated home, on Node 22.16.0 and 24.x (24.20.0). Every step after a successful build runs even
when an earlier one fails, so one red suite does not hide the rest. The workflow runs on demand and
on pull requests that change it, not on every push (#221). Later the same day the owner made the
M1 iMac available at any time over SSH; its runs of the same steps are the primary macOS receipt,
and the runner's are kept as a second, virtualised one.

| Platform | Where | Engine gate | Agent probes |
| --- | --- | --- | --- |
| Linux | `ci.yml` on `main` `092807c9`, ubuntu-24.04, run 35185205588 | `engine` pass on 22.16.0 and 24.x; the `check` coverage job failed once on `matrix A1` (#213) | not run: no agent login on a hosted runner |
| WSL | developer host, Node 24.16.0 and 22.23.1 | pass at `568155b7` (#254): 1579 + 280 on Node 24.16.0; the same on 22.23.1 one `src/paths.ts` edit earlier | the dogfood user runs all 12 ordered pairs daily (#244, 2026-09-17: 0 failing pairs) |
| macOS | M1 iMac (macOS 26.6.2) on `main` `84ba32ff`, and `platform.yml` runs below | M1 iMac: pass on 22.16.0 and 24.21.0 (typecheck, lint, build, 1585 + 280, pack-check, cold start, packed engine). Runner: unit and serial pass on both after #262; its hook cold start fails on the virtualised timer spread | **unverified**: no agent CLI on either machine (#269) |

### macOS runs

| Run | Head | Result on 22.16.0 and 24.x |
| --- | --- | --- |
| 35185263530 | `08704b4f` | 11 failures, identical on both: 8 from comparing a path through a symbolic link with its physical form (2 fail-open privacy sites, 6 test expectations in the linked spelling), 2 macOS portability defects in tests, 1 the busy wait below |
| 35188141848 | `905e5447` (with #254) | unit suite 1577 of 1581, 1 failure on both: the busy wait |
| 35188559922 | `3f7788ec` | unit 1577 and 1576 of 1581, serial 278 of 280; the busy wait on both, plus `busy`, `db-missing`, `worker-kill`, a partial-row e2e and a slow-git capture test once each |
| 35188968971 | `ad0d8b80` | unit 1577 and 1576 of 1581, serial 279 of 280; the busy wait and fault `busy` on both, plus the slow-git repository-identity test once |
| 35206343042 | `2e7c4e55` | unit 1576 and 1577 of 1581, serial 279 of 280; the busy wait and fault `busy` on both, plus the slow-git detector test once on 22.16.0; hook cold start pass |
| 35210161686 | `193f607a` | unit 1573 and 1576 of 1581, serial 278 of 280; the busy wait and fault `busy`, plus a harness test broken by that commit's `package.json` split (reverted in `cf3862f6`) and five one-off timing failures; hook cold start measured straight after the build at load 9.25 fails `--version` on 22.16.0 (max 137.0 ms against 100) |
| 35211675707 | `cf3862f6` | unit 1577 and 1576 of 1581, serial 279 and 280 of 280; the busy wait on both, fault `busy` on 22.16.0, the slow-git timeout test once on 24.x; hook cold start fails on both (below) |
| 35213351032 | `11f65c09` | unit 1577 and 1576 of 1581, serial 280 of 280; the busy wait on both and the slow-git detector test once on 24.x; hook cold start passes on 22.16.0 (max 286.1 ms) and fails on 24.x (`--version` max 113.4 ms, secret-dense max 340.8 ms) |
| 35217962090 | `298c2a48` (#251's head) | unit 1576 and 1577 of 1581, serial 280 and 279 of 280; the busy wait on both, fault `busy` on 24.x, the slow-git detector test once on 22.16.0; the packed engine with all eleven engine items passes on both; hook cold start fails on both (`--version` max 104.1 and 148.4 ms, clean 200 KB max 396.4 ms, spool max 303.2 ms) |
| 35224724418 | `9753829e`: #262's head `5780b4b2` merged onto `main` `b324f8b8`, on a temporary branch deleted afterwards; its tree is the tree of #262's merge commit `84ba32ff` | unit 1585 of 1588 (3 skipped) and serial 280 of 280 on both, including `a busy database spools inside the capture budget` (236 ms on 24.20.0) and fault `busy`; pack-check and the packed engine pass; hook cold start fails on both (`--version` max 118.3 and 105.6 ms; clean 200 KB max 309.8 and 312.1 ms; small, DB present max 350.9 ms with one of 33 events spooled on 24.20.0; one secret-dense sample of 675.9 ms on 22.16.0 at a 5-minute load of 15.02) |

The first run found a fail-open privacy defect, not a macOS quirk: on macOS the temporary directory
is a symbolic link, and a `secret_paths` rule or a worktree root compared in one spelling missed a
path written in the other, so a secret path rule was not applied. It reproduced on Linux with
`TMPDIR` pointing at a link and was fixed in #254 (`physicalPath`, `withPhysicalRules`), with the two
test-only portability defects (`/proc/<pid>/fd`, a realpath'd isolated home) in the same change.
`scripts/measure-cold-start.mjs` read `/proc/loadavg`, which macOS does not have; it falls back to
`os.loadavg()` since `ad0d8b80`.

Passing on macOS in run 35188968971: pack-check; the packed engine in an isolated home reports
`storage`, `fts`, `migration`, `worker` and `spool` healthy on both Node versions; and the hook
cold start stays inside its budget with the suite's load still on the runner (load average 6.36 and
8.02 at the start of the table):

| Node | `--version` p50 / max | hook small, DB present p50 / max | secret-dense 200 KB p50 / max | DB absent (spool) p50 / max | Budget |
| --- | --- | --- | --- | --- | --- |
| 22.16.0 | 42.8 / 45.2 ms | 128.9 / 153.3 ms | 137.7 / 154.6 ms | 128.4 / 175.5 ms | 100 / 300 ms |
| 24.20.0 | 49.0 / 58.6 ms | 151.2 / 190.0 ms | 160.1 / 201.4 ms | 151.2 / 220.1 ms | 100 / 300 ms |

From `cf3862f6` the cold-start step exits 1 when a row fails. On that commit the runner failed it and
the M1 iMac, measuring the same commit over SSH, did not:

| Machine | Node | 1-minute load | `--version` p50 / max | hook small, DB present p50 / max | secret-dense 200 KB p50 / max | DB absent (spool) p50 / max | Status |
| --- | --- | --- | --- | --- | --- | --- | --- |
| runner, run 35211675707 | 22.16.0 | 3.99 | 55.4 / 73.3 ms | 191.0 / 297.1 ms | 220.5 / 304.3 ms | 185.4 / 243.1 ms | fail: secret-dense max |
| runner, run 35211675707 | 24.20.0 | 5.27 | 62.3 / 80.6 ms | 216.0 / 266.0 ms | 223.6 / 300.2 ms | 229.0 / 288.6 ms | fail: secret-dense max; 32 of 33 events in the database |
| M1 iMac, macOS 26.6.2 | 22.16.0 | 2.52 | 38.4 / 39.1 ms | 143.4 / 147.7 ms | 151.0 / 160.4 ms | 138.6 / 140.6 ms | pass |
| M1 iMac, macOS 26.6.2 | 24.21.0 | 2.52 | 41.7 / 44.2 ms | 148.5 / 153.2 ms | 153.3 / 160.2 ms | 144.1 / 145.9 ms | pass |

The runner's hook medians were 128–160 ms in run 35188968971 and 185–229 ms here, while the iMac
measured 138–153 ms at the same commit, so the spread is the runner's; on the iMac every scenario
keeps all 33 events and no maximum passes 177.2 ms (clean 200 KB on 22.16.0). The 24.20.0 row's missing
event is not lost: past the 40 ms spool reserve the hook appends the event to the spool and returns,
as spec 007 FR-002 allows, and since the commit after `cf3862f6` the landed check counts both. The runner's
cold-start failure is filed with the busy wait as virtualised-timer inflation, not as a product
regression; the gate is not loosened for it.

### The deterministic macOS failure: the busy wait is not a wall-clock bound (#255)

`a busy database spools inside the capture budget` failed in all four runs on both Node versions
(335–772 ms against 300 ms), and fault `busy` wherever the serial step ran (317–974 ms). A probe on
the same runner (run 35189622707, from a temporary branch deleted afterwards) held `BEGIN IMMEDIATE`
on a WAL database and timed a second connection 20 times:

| Runner | `timeout: 150` then `BEGIN IMMEDIATE` | `timeout: 0`, retry until `performance.now()` passes 150 ms | `Atomics.wait` 1 ms |
| --- | --- | --- | --- |
| macOS, Node 22.16.0 | p50 419–452 ms, max 605 ms | p50 158–172 ms, max 189 ms | p50 5.0 ms |
| macOS, Node 24.x | p50 634–674 ms, max 1011 ms | p50 172–179 ms, max 229 ms | p50 9.0 ms |
| WSL host, Node 24.16.0 | p50 150.6–152.2 ms, max 165.6 ms | p50 150.1 ms, max 153.5 ms | p50 1.1 ms |
| M1 iMac (macOS 26.6.2), Node 22.16.0, 22.23.2, 24.21.0 | p50 177–182 ms, max 187.7 ms | p50 150.5–151.9 ms, max 152.8 ms | p50 1.3 ms |

The numbers do not change when CPU-bound processes are added, so the cause is how long a short
sleep lasts, not CPU contention. The M1 iMac row (three runs per Node version, idle and loaded,
measured over SSH the same afternoon) shows that the 3–6 times overrun belongs to the virtualised
runner's timer granularity; real Apple Silicon overshoots by about 30 ms, which still takes a busy
capture (hook startup plus the wait) past 300 ms. SQLite's default busy handler stops when the
*requested* sleeps add up to the timeout and never reads a clock, so the contract's "busy timeout is
min(150 ms, remaining budget minus the spool reserve)" (spec 007 `contracts/agents.md`) is a wall-clock
bound on Linux and not on macOS. The event still spools and nothing is lost; the hook overruns its
budget 3–6 times. #262 (merged as `84ba32ff`) fixes it: a hook-budget connection fixes a wall-clock
deadline when it is opened and retries `BEGIN IMMEDIATE`, and the open step, in short sleeps that
re-read `performance.now()`, each wait capped at 150 ms. Run 35224724418 above passes both busy
tests on the runner, and the M1 iMac passes the full suite on #262's head and on `main` (below).
Timing details of the last retry are #270.

### One-off timing failures (#256)

Each of these failed once in four runs and passed in the other three: the two slow-git tests simulate
git by blocking the thread for most of the real budget and then read what the clock says is left, so
a late wakeup leaves nothing (fixable in the tests); `db-missing`, `worker-kill` and the partial-row
end-to-end test have the shape of #203 and #213, a seed that runs out of its deadline under load.

### The M1 iMac: the gate on `main` after #262

`main` `84ba32ff`, cloned on the iMac with `HOME` and `TMPDIR` redirected to a scratch directory,
portable Node from the official tarballs, the `platform.yml` steps in order:

| Node | typecheck, lint, build | Unit, migrations, scripts | Serial end-to-end and fault | pack-check | Packed engine, isolated home |
| --- | --- | --- | --- | --- | --- |
| 22.16.0 (arm64) | pass | 1585 of 1588, 3 skipped, 0 fail | 280 of 280 | pass, 20.917 MB installed | `setup --provider none` exit 0; doctor exit 1 with every engine item healthy and `provider` degraded ("No observer provider is configured.") |
| 24.21.0 (arm64) | pass | 1585 of 1588, 3 skipped, 0 fail | 280 of 280 | pass, 20.917 MB installed | same |

| Node | 1-minute load, kept attempt | `--version` p50 / max | hook small, DB present p50 / max | clean 200 KB p50 / max | secret-dense 200 KB p50 / max | DB absent (spool) p50 / max | Status |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 22.16.0 | 6.85 | 38.0 / 38.5 ms | 143.3 / 152.2 ms | 144.8 / 146.4 ms | 149.9 / 152.8 ms | 140.0 / 142.0 ms | pass, all 33 events kept |
| 24.21.0 | 10.70 | 42.2 / 44.5 ms | 147.3 / 149.1 ms | 148.8 / 151.2 ms | 154.1 / 156.4 ms | 145.4 / 148.9 ms | pass, all 33 events kept |

The script measures twice and keeps the attempt with the lower 1-minute load. The load comes from
the suites that ran just before on the same machine (8.48 and 12.37 before the first attempt), and
the cold start stays within 156.4 ms under it.

### Verdict

T040 is complete (2026-09-17). Linux and WSL pass the engine gate. On macOS the whole gate ran on real Apple
Silicon and on a virtualised runner: it found and fixed a fail-open privacy defect (#254) and a busy
wait that was not a wall-clock bound (#255, #262), and `main` `84ba32ff` passes every step on the M1
iMac on both Node versions. The runner still fails the hook cold start on its timer spread and
sometimes a one-off timing test (#256); these are recorded, not counted as passes. Agent probes on
macOS stay unverified, not passing: neither machine has an agent CLI (#269). Remaining macOS
privacy gaps are #252 and #253.

## E11 — synthetic ordered agent pairs and a removed worktree (T020)

Commit `2339d2c1` adds `test/unit/work-pairs.test.ts`. Everything in it is synthetic: the observer
runs through `runObserveForFixture` with a mocked provider response, capture goes through the
product's hook path with a stubbed `git` that replays each worktree's identity, and the receivers
are the in-process readers of each agent (Claude prompt stdout, Codex `hookSpecificOutput`, Grok's
pending runtime state attached on `PreToolUse`, Pi `runInject`). No agent CLI, model or network is
used.

### What the thirteen tests assert

| Test | Assertion |
| --- | --- |
| `synthetic <seed> -> <receiver>` for the twelve ordered pairs of claude, codex, grok and pi | One corpus per pair: a repository with linked worktrees A and B, works X and Y in A, work Z in B, prompts interleaved across the three works, a checkpoint generated for each by the observer, and a later prompt per work left unprocessed. The continuation prompt shares words with all three works. The observer is asked once per work and never sees a sibling's checkpoint or nearby memory. The receiver's ambiguous start in A lists X and Y, not Z, and delivers no checkpoint. After an explicit choice (X for even pairs, Y for odd) the pack holds the selected work's outstanding checkpoint, and the siblings' checkpoints, outstanding text, work-scoped observations and pending prompts are withheld; the injection ledger joined with memory visibility names no sibling work memory. |
| `removed worktree: search and explicit continuation retain Z checkpoint and provenance without recreating the directory` | After `git worktree remove` of B, a new session in A that explicitly chooses Z receives only Z's checkpoint; CLI history search finds it, `get` on that hit shows only Z's agent as its source, the stored sources keep B's original path and context, and B is still absent afterwards. |

### Runs

| Node | `work-pairs.test.ts` | with `work-readers.test.ts` and `work-context.test.ts` |
| --- | --- | --- |
| 24.16.0 | 13 of 13 | 88 of 88 |
| 22.23.1 | 13 of 13 | 88 of 88 |

On WSL the branch's full gate exits 0: typecheck, lint, markdownlint, and `npm test` on both versions
(unit 1599 of 1601 with 0 failing, serial 280 of 280).

### Mutations

Each mutation edits the engine's SQL in the built test bundle, runs the thirteen tests, and restores
the bundle from a copy. Each one fails all thirteen, on the assertion named:

| Mutation | Engine change | Failing assertion |
| --- | --- | --- |
| no checkpoint | the selected work's checkpoint query (`src/db/queries.ts`) returns nothing | `selected work X/Y/Z includes its generated checkpoint` |
| other checkpoint | the same query returns another active work's checkpoint | `selected work X/Y/Z includes its generated checkpoint` |
| scope leak | the memory, checkpoint and visibility work filters admit any work of the repository, and the pack stops excluding session summaries | `work X/Y checkpoint is withheld` |
| raw activity | the pack reads pending prompts of any work (`src/injection/pack.ts`) | `work X/Y pending activity is withheld` |
| knowledge lane | the memory and visibility work filters admit any work of the repository | `work X/Y observation is withheld` |

### Limits

- `contracts/work.md` B3 keeps real agents and platforms separate acceptance gates. The native runs
  through `scripts/e2e/isolated-user.mjs` and `isolated-lifecycle*.mjs` are #265.
- The daily dogfood's twelve pairs (#244) ran the schema 3 bundle until 2026-09-17 (E13) and check
  fact recall only; they are not SC-002 evidence.
- The older twelve-pair loop in `work-readers.test.ts` (one work per corpus) stays; it covers
  checkpoint delivery per receiver, not sibling isolation.

## E12 — retrieval misses investigated on the fixture corpus (T023)

### The no-model replay

`node dist/oboete.mjs fixture replay test/fixtures/events-1000.jsonl --json` on `main` `6b683213`,
Node 24.16.0, under `env -i` with a temporary `HOME` and `OBOETE_HOME` and `[observer] preset =
"none"`, at a 1-minute load of 0.24.

| Stage (40 tagged facts, 20 ja and 20 en) | Outcome |
| --- | --- |
| capture | pass, 40 |
| coverage | pending, `no_range`, 40 (the first failing stage for every fact) |
| application | pending, `deferred`, 40 |
| retention | fail: `no_linked_fact` 32, `temporary_only` 8 |
| retrieval | fail, `no_candidate`, 40 |
| delivery | fail, `missing`, 40 |
| answer | not run, 40 |

Its other bounds: capture p99 164.1 ms with 100% of 717 samples within 300 ms (pass); injection p99
263.6 ms with 100% of 378 within 300 ms (pass); worker VmHWM 107.6 MiB over 38 observe runs (pass);
session start fails because 2 of its 48 pending packs lack `summary_pending` (46 carry it); SC-009 recall 0/40
fails. With no model nothing is generated (research.md R6), so the replay cannot show a ranking
miss: no fact reaches retrieval.

### The same facts stored verbatim

Each tagged fact's sentence (the payload line containing its expected value) stored as a memory,
all 40 in one repository, and each fact's own query run through `searchMemories`: all 40 within
the first five, 39 first. The one that is not first, `f-ja-19` (「鍵ローテは月のいつ？」), is third
behind `f-ja-04`, which also mentions the key rotation, and `f-ja-01`: MMR moved a row similar to the
top one down, and it is still returned. These are lexical questions that share words with their
facts; true paraphrase is T024's (#266).

### Pins in `test/unit/retrieval.test.ts` (PR #273)

| Test | Pins |
| --- | --- |
| `searchMemories returns each events-1000 fact among the first five through the search surface` | 40 facts derived from the fixture's `tags.fact`, 20 ja and 20 en; each within the first five; at least 39 first |
| `rankCandidates ignores created_at when trigram and cjk scores are equal` | equal scores order by id, and swapping `created_at` changes nothing |
| `searchMemories returns a relevant older fact among newer unrelated memories` | a fact created at time 1 ranks first for its query |
| `searchMemories hides a superseded fact unless history is requested` | default search omits the superseded row; `--history` returns both; `get --history --json` shows `valid_to` and `superseded_by` naming the current row |
| `searchMemories returns two distinct facts that share a title when they are the only candidates` | both returned |
| `searchMemories returns the fact-bearing memory of a five-row corpus` (skipped, #275) | the five rows of the `claude-to-codex` pair at pack time and that pair's recall prompt; un-skipped it fails with `returned m_confirm`, the receipt below |
| `the pinned pair prompts are still the ones the probe library sends` | the artifact's three facts and its copied recall and seeding prompts, compared exactly against what `scripts/e2e/probe-lib/isolated-agent.mjs` returns, plus one shell-quoted fact the pair's own facts cannot show; runs whether or not the artifact is skipped. The `factSet` comparison is what kills mutation 9: the fact sentences are written out here and only the stem comes from `factStem`, so that line pins the text but not the stem. The stem is pinned by the seeding prompt's `printf` line, which spells it out |

All 36 runnable tests in the file pass on Node 24.16.0 and 22.23.1, re-measured on both after the
2026-09-18 edits to this file; the 37th is the #275 artifact, which is skipped until that fix. Each of the first six mutations edits the built test bundle, runs
the named test, and restores the bundle (sha256 compared). The last four edit
`scripts/e2e/probe-lib/isolated-agent.mjs`, which the prompt pin imports. That import is static and
esbuild inlines it, so editing the source alone changes nothing: each of the four was re-run on
2026-09-18 as `npm run build && node --test build/test/unit/retrieval.test.mjs`, and each left 35
passing and one failing — `the pinned pair prompts are still the ones the probe library sends` in
all four cases. The source was restored from a copy afterwards (`git status` clean, and the file
compared byte for byte against that copy), and the bundle rebuilt from it — a restored source alone
leaves the last mutation inside `build/`, which is why the six above compare the bundle's sha256
instead.

That import is a plain one. It became possible in this PR: `trusthash.mjs` guarded its command
block with `realpathSync(process.argv[1]) === self`, and esbuild collapses `import.meta.url` to the
bundle, so the guard fired on the test runner's own entry and read an argument it does not set. The
guard now checks this module's own name first — `basename(self)`, which is the bundle's name when it
is inlined and `trusthash.mjs` when it is not, so the block runs only in the second case. Reading
`self` rather than `process.argv[1]` keeps the symlink tolerance the realpath comparison exists for:
a symlinked entry still runs the block, a renamed copy of the file does not.
`scripts/e2e` is outside the TypeScript program, so
`isolated-agent.d.mts` declares the four functions a `.ts` file imports.

| Mutation | Failing assertion |
| --- | --- |
| `injection.threshold` config default raised to 0.99 (the live path; `DEFAULT_THRESHOLD` in `rank.ts` is only the fallback and mutating it changes nothing) | corpus: `fact f-ja-19 query 鍵ローテは月のいつ？ position absent above []` |
| equal scores prefer the newer `created_at` | age: `[z_new, a_old]` instead of `[a_old, z_new]` |
| normalized score × 0.6 for rows older than a day | age: the same |
| `m.valid_to IS NULL` removed from `memoryScope` | supersession: default search returned the old row |
| `superseded_by` dropped from history output | supersession: `get --history` `superseded_by` undefined |
| MMR rejects everything after the first pick | shared title: `[m_hooks]` instead of both |
| a space added before the `\|` in the seeding prompt's last line | prompts: `buildFactSeedingPrompt` differs from the pinned text |
| `fact line` reworded to `fact-line` in the recall prompt | prompts: `recallPrompt('codex', false)` differs from the pinned text |
| `cedar` capitalised in `factSet` | prompts: `factSet` differs from the pinned three facts |
| `shellQuote(fact)` replaced with `` `'${fact}'` `` in `buildFactSeedingPrompt` | prompts: the shell-quoted fact's `printf` line differs |

### Limits

- The pins go through the search surface. The injection pack uses the same ranking with a character
  budget and filters already-delivered and retired rows (`src/injection/pack.ts`); the pack path is
  measured by the replay above, which needs a model to say anything about recall.
- The fixture corpus is too large to show a small-corpus miss that the first 009 dogfood run did
  (E13, #274). In pair `claude-to-codex` at pack time (five memories, session summaries excluded),
  FTS5 clamps the IDF of trigrams in more than half the documents to 1e-6, so one row matching a
  rare trigram scores -0.436 and the other two -0.0000064 and -0.0000047. Normalized by the ratio
  to the best score, both fall to about 0.00001, below the 0.3 threshold, and the memory holding
  the three exact facts is omitted. The same prompt against the same rows one memory later includes
  all three. That is #275, and T023 stays open for it. Those five rows and that recall prompt are carried
  verbatim in `test/unit/retrieval.test.ts` as a skipped test, so the fix un-skips a failing artifact
  rather than writing a new one. The artifact names the rows
  `m_confirm`, `m_decision` and `m_fact` for `m_c2bfcff0`, `m_363fe065` and `m_9da36e8d`, plus
  `m_checkpoint` and `m_request` for the pair's two session summaries. Keep all five. Measured on
  2026-09-18 by inserting each corpus and calling `searchMemories` with the pair's recall prompt: the
  five rows return `m_confirm` alone, and the three searchable rows alone return `m_confirm` and
  `m_decision`. So the miss is not an artefact of the summaries — `m_fact` is absent either way — but
  the summaries are in the FTS index even though the scope hides them, and removing them takes the
  corpus to three documents, which lifts `m_decision` above the threshold. Fixing against three rows
  would be measuring against a corpus the run never had. The counter-pin the fix has to land with is
  recorded on #275. The receipt for the copied rows is that pair's database from the run,
  `/var/tmp/oboete-dogfood-upgrade/all0917/claude-to-codex/memory.db`, verified row for row on
  2026-09-17. That copy is the dogfood account's and the daily cron keeps writing to it (it holds six
  memories now, not five), so the test file is the frozen one.
  The two prompts the artifact carries are not taken on trust: `the pinned pair prompts are still the ones
  the probe library sends` compares them exactly against what `scripts/e2e/probe-lib/isolated-agent.mjs`
  returns. It imports that module statically, so esbuild inlines it into the test bundle and an edit to
  the source only reaches the pin through a rebuild — `npm test` rebuilds, a bare
  `node --test build/...` does not.
- A memory injected once and then unused for 90 days is omitted from packs as `retired` (data model);
  it is still returned by search, which has no `last_injected_at` filter, so User Story 3's first
  acceptance scenario (age alone does not make a fact unavailable when asked about) holds.
- The MMR rule rejects a candidate once its similarity to a selected row reaches its normalized
  relevance, 61 / (60 + rank). A probe drops "The busy timeout for the CLI is 2000 ms." once 15
  candidates rank above it and above its sibling about hooks. With no failing corpus case, lambda is
  unchanged and this is #272.

## E13 — the daily dogfood install moves to the 009 bundle

On 2026-09-17, with the owner's approval, the isolated dogfood account's install moved from the M1
bundle (schema 3) to the 009 bundle packed from `main` `6b683213`, so the daily run from 2026-09-18
exercises 009 (#244 was schema 3).

- Before: no worker running; `wal_checkpoint(TRUNCATE)`; the whole `~/.oboete` and the previous
  global package copied to a dated backup in that account's home; the backup's `quick_check` ok with
  125 memories and the same sha256 as the live database.
- Install: `npm run build && npm pack` on a clean `main`, `npm install -g` of that tarball; the
  installed engine contains the #264 change.
- Migration: hooks never migrate (they spool and start the worker) and `doctor` opens the database
  read-only, so the migration ran on an explicit read-write open (`oboete work status --json`):
  `user_version` 3 → 8, migrations 1–8 recorded, `quick_check` ok, 125 memories.
- `oboete doctor` afterwards: storage, fts, migration, worker, spool, sync, allowance and the
  claude, codex and pi agent items healthy; provider and catalog unverified (not probed);
  `agent:grok` degraded because Grok dropped the managed
  markers (#178, already reported by the run before the upgrade); `generation` warning with 4 parked
  and 27 legacy sources. Migration 0005 parks unbatched prompts from before work continuity for an
  explicit choice, and 0004 keeps rows of old applied batches as legacy instead of starting a provider
  backlog on upgrade.
- Rollback, if needed: restore both the backed-up `~/.oboete` and the previous package together; the
  schema 3 bundle cannot open a schema 8 database.
- First daily run on this bundle (`2026-09-17T15-05-08-894Z`, issue #274): 1 of 12 pairs pass,
  against 12 of 12 on the M1 bundle the day before with the same preset and model. In every pair
  the explicit `oboete observe` applied its batch, so this is not a provider or consent failure.
  Six pairs never found the facts in `oboete search` because the observer answered `no_memory` for
  the fact prompt, and the one row that still holds the verbatim request (the free session summary)
  is outside ordinary retrieval by contract (`contracts/sharing.md`). Five pairs had a fact-bearing
  memory, but the receiving start pack carried only the work checkpoint and the prompt pack left
  that memory below threshold. The M1 start pack carried the free summary instead. Separately,
  the dogfood home's `consent_changed` outcomes are held doctor-probe sources whose temporary
  root is gone. The cause, the fixes and the gate's 009 form are tracked in #274.
