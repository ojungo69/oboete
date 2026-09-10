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
It adds no dependency or schema. T029-T032 remain unchecked.

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
