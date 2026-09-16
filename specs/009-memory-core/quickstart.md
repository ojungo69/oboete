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
and synthetic hook/worker verification; they do not complete T020 or the real-agent/product gates.

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
hash over the primary and the whole chain and the `consent` item is the surface that reports a
mismatch (contract "Diagnostics"). Filed instead of fixed: doctor calls the shared cap spent at
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
