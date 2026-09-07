# Tasks: Quality Debt to Zero

**Input**: Design documents from `/specs/008-quality-debt-zero/` (plan.md, spec.md, research.md, data-model.md, quickstart.md)

**Tests**: No new test suites. Existing suites must pass unmodified on every batch; a real security finding gets one failing-then-passing test (FR-006); each S8786 regex change gets one long-input timing case (research R6).

**Organization**: Phases follow the pull-request batches of plan.md (A → E → B → C → D → F) because each batch reduces the noise the next one works in and the security batch must land before any code batch touches the same files. Every task carries its story label; US5 (small gated steps) is realised by the gate task that closes every batch. Owner per task: **Codex** = delegated background job in its own worktree, reviewed here; **session** = written in this session (security-owned modules, viewer front end, verdicts, configuration, service actions).

## Format: `[ID] [P?] [Story] Description`

- **[P]**: can run in parallel with the other [P] tasks of the same phase (different files, separate worktrees)
- **[Story]**: US1 disposition record, US2 security verdicts, US3 complexity/length, US4 configuration exclusions, US5 gated delivery

## Path Conventions

Single package at repository root: `src/`, `scripts/`, `test/`, `docs/evidence/`, analysis configuration files at the root. The shared checkout `~/projects/free-mem` stays on `main`; the session edits in the linked worktree `~/projects/free-mem-wt/008`; Codex jobs in `~/projects/free-mem-wt/<batch>`.

---

## Phase 1: Setup

**Purpose**: Establish the worktree layout, freeze the inventories, and build the record tooling.

- [x] T001 Create the session worktree `~/projects/free-mem-wt/008` on branch `008-qd-a` from `origin/main`, symlink `.specify` and write `.specify/feature.json` there per memory `speckit-in-linked-worktree`, and return the shared checkout to a clean `main` (session, 2026-09-07)
- [x] T002 Copy the two inventory exports to `docs/evidence/quality-debt-2026-09/sonar-main-issues.json` and `docs/evidence/quality-debt-2026-09/codacy-main-issues.json` (310 and 397 rows; fields per data-model.md "Finding"; the Sonar field is named `id`, not `key`, because gitleaks reads `"key": "AaB3…"` as a generic API key) and create `docs/evidence/quality-debt-2026-09/ledger.json` as an empty array (session)
- [x] T003 Write `scripts/quality-debt-record.mjs` with: `--allocate` (assigns every inventory id to exactly one batch from the plan.md scope rules and writes `allocation.json`), default mode (regenerates the two tables and the History section of `docs/evidence/quality-debt-2026-09.md` from inventories + ledger, one row per `(service, id)`), `--check` (non-zero exit on missing, duplicate, `open`, or reason-less `resolved` rows, and on unassigned or doubly assigned ids), `--apply-sonar --dry-run` / `--apply-sonar` (transition + comment via the SonarCloud API using `~/SONAR_TOKEN.md`); one `node --test` file `scripts/quality-debt-record.test.mjs` with a fixture of five rows covering each failure mode (Codex job task-mtq5g71o, reviewed; 19 subtests)
- [x] T003b Extend `scripts/quality-debt-record.mjs`: ledger fields `confirmed` (analysis id / commit SHA / HTTP response) and `reason` (Codacy enum); `--check` fails on unconfirmed rows and `--check --planned` relaxes only that; the tables print `state ✓` or `state (planned)`; `--apply-codacy [--dry-run]` sends `PATCH …/issues/{hexId}` with `{ignored, reason, comment}` using `~/CODACY_TOKEN.md` line 2 and writes the HTTP status into `confirmed`; `--apply-sonar` writes `confirmed` the same way; `--confirm --sonar-analysis <id> --codacy-commit <sha>` marks planned `fixed` / `excluded` rows confirmed when the given analysis no longer lists the id (queries both services); tests for each new failure mode (Codex, same worktree)
- [x] T004 Run `--allocate` and `--check`, paste the per-batch counts into the plan.md batch table, and write the record header (export timestamp, `main` SHA 5e03d67f, branch/worktree notes) at the top of `docs/evidence/quality-debt-2026-09.md` (session)
- [x] T005 Checkpoint C1 (maintainer): request a Codacy account API token; record in the plan whether the token was provided or the maintainer will perform the Codacy UI actions from the record's lists (session; maintainer chose to save it as `~/CODACY_TOKEN.md` line 2 on 2026-09-07; verify with `GET /api/v3/user` before the first write)

---

## Phase 2: User Story 4 — Configuration exclusions that cannot hide a real finding (Priority: P2, executed first) — Batch A

**Goal**: Stop the rules that cannot apply to their file class, with the removed count and the file class stated per line; judged by the post-merge analysis.

**Independent Test**: After the analysis of the merge commit, SonarCloud drops the 15 PL/SQL findings and Codacy drops 287 findings; `git diff` of the configuration files shows only the file classes in research R4; `src/` (except `src/db/migrations/**` for SQL rule sets) is under no new exclusion.

- [x] T006 [US4] Add `sonar.plsql.file.suffixes=pks,pkb` with a one-line comment (SQLite migrations are not PL/SQL) to `sonar-project.properties` (session)
- [x] T007 [P] [US4] Rewrite `.codacy.yml`: top-level `exclude_paths` (`legacy/**`, `package-lock.json`, `build/**`, `dist/**`, `coverage/**`); `engines.tsqllint.exclude_paths` and `engines.SQLint.exclude_paths` = `src/db/migrations/**`; `engines.lizard.exclude_paths` = `test/**` only; `engines.opengrep.exclude_paths` = `test/**`, `scripts/e2e/**`; delete the inert `include_paths` block; comment each line with the file class and why it cannot hide a `src/` finding (session)
- [x] T008 [P] [US4] Add `.markdownlint.json` with `default: false`, the 33 rules the standard may run switched on by name (derivation in research R4), and `MD024` with `siblings_only` (a file that names only MD024 switches every other rule on: 317 new findings on the first analysis); PR body states that the repeated headings in `docs/evidence/m1-dogfood.md` and `docs/research/oboete-contracts-2026-09-02.md` are per-section structure (session)
- [ ] T009 [US4] Disable the single pattern `Stylelint_scss_function-disallowed-list` (`PATCH …/tools/1f03328a-086e-459e-bfa3-73e56f01020f` with `{"patterns": [{"id": …, "enabled": false}]}`, research R3; then `GET …/tools/…/patterns` and check that no other pattern's `enabled` changed; if refused because the repository follows the organisation coding standard, ignore the one issue with reason `FalsePositive` instead and never edit the standard); no `.stylelintrc` is added, so the other CSS rules keep running; record both HTTP responses in the ledger (session)
- [x] T010 [US4] Verify `git diff 5e03d67f..HEAD -- .github/workflows sonar-project.properties` shows only T006, and that no gate definition references `.sql`, `legacy/`, `test/`, or `scripts/e2e/` in a way the exclusions change (session) — verified 2026-09-07 against 5e03d67f..1816a503: besides T006 the range carries the T003b coverage step in ci.yml (5 lines, a `node --test` run of the record tests) and `sonar.test.inclusions` gaining `**/*.test-support.mjs` (a test helper classified as test code); no gate condition changed
- [x] T011 [US4] Append batch A rows to `ledger.json` as **planned** states: every Sonar `plsql:*` id and every Codacy id under the excluded file classes as `excluded` with the configuration line; the Stylelint id as `excluded` with the pattern-disable setting (a configuration change, confirmed by the next analysis); regenerate the record (session; 302 rows, `--check --planned` reports 405 open)
- [x] T012 [US4] [US5] PR #159 `008-qd-a` (config + inventories + generator + record): run the PR #155 gate (CI, Sonar gate, `codex-review` `ok: true`, `ponytail-review`, bot triage, one CodeRabbit trigger), merge with a merge commit, `git -C ~/projects/free-mem pull --ff-only`, wait for both services' analysis of the merge commit, then confirm the 302 planned rows against that analysis (`--confirm`) and write the post-A counts into the record; A is done only when the analysis shows Sonar 295 and Codacy ≈ 110 and every A row is confirmed (session) — confirmed 2026-09-07 against the analyses of 1816a503 (A and E together): Sonar 310 → 295 after A (analysis 46205843 of 2d2dac10) → 279 after E; Codacy 108 after E (the post-A Codacy count was not captured separately); 301 of the 302 A rows confirmed, the Stylelint row waits for the token (T009)

---

## Phase 3: User Story 2 — Security-flavoured findings are read, not pattern-dismissed (Priority: P1) — Batch E

**Goal**: A written verdict (controller, validation, sink, effect) for every security-flavoured finding in `src/`, the two harness `sudo` lines, and the `ci.yml` line; real ones fixed here with a test, before any code batch touches the same files.

**Independent Test**: The record has a verdict row for each of: Codacy timing 6, non-literal RegExp 5, SSRF 1, prototype pollution 2, unsafe dynamic method 2, tainted SQL 2, ShellCheck SC2024 2, `ci.yml` 1, and Sonar S8786 13; each names a controller from research R6 (including repository-supplied configuration); any `real` row points at a merged fix with its test.

- [x] T013 [US2] Create branch `008-qd-e` in the session worktree from `main` after A merged; read and write verdicts for the 6 timing-attack lines `src/worker/lease.ts:84`, `src/worker/observe.ts:220`, `src/worker/observe.ts:804`, `src/injection/recognize.ts:71`, `src/config.ts:321`, `src/setup/detect.ts:187`, each with controller / validation / sink / effect (session)
- [x] T014 [P] [US2] Verdicts for the RegExp constructions `src/agents/index.ts:81`, `src/agents/index.ts:322`, `src/privacy/detect.ts:140`, and `scripts/dco-check.test.mjs:105`, `:109` (the last two are test constants: the job id literal and an indent width; verdict only): trace `PATH_KEYS`, `key`, and `body` back to their controller (bundle constant, operator config, repository-supplied `.oboete.toml` per `src/config.ts:155`, agent payload); if a non-operator controller reaches the constructor, add syntax validation or length bounding (not blanket escaping, which changes `[a-z]` semantics) with a failing-then-passing test in `test/unit/agents.test.ts` or `test/unit/privacy.test.ts` that keeps the existing accept / reject / secret-detection cases (session)
- [x] T015 [P] [US2] Verdicts for `src/viewer/app/api.ts:78` (fetch path), `src/setup/managed-block.ts:168` and `:201` (bracket lookups by TOML path keys), `src/agents/pi.ts:143` and `src/cli.ts:94` (table lookups): confirm `Object.hasOwn` or an allow-list guards each lookup against `__proto__` / `constructor`; add the guard with a test if missing (session)
- [x] T016 [P] [US2] Verdicts for `src/injection/pack.ts:342` and `src/db/queries.ts:254` (placeholder lists with bound parameters), `scripts/e2e/dogfood.sh:27,32` (`sudo cat > file`: unprivileged redirection is intended; add `# shellcheck disable=SC2024` with the reason on the line above), `.github/workflows/ci.yml:58` (pinned action SHA matched a key pattern) (session)
- [x] T017 [US2] For each of the 13 Sonar S8786 super-linear regexes (`src/privacy/detect.ts:72` stripPrivate — measured real, 1.4 s on `<` + 50k spaces against a 1 MiB text cap, linear form `/<\s*(?:(\/)\s*)?private\s*>/gi` verified equivalent; `src/events.ts:328-330`; `src/repo-identity.ts:63,65`; `src/setup/managed-block.ts:83`; six in `scripts/`): write controller / validation / sink / effect; add to the module's existing test file one case with a 100 kB (1 MiB for stripPrivate) non-matching input bounded in wall time and keep the existing accept / reject / secret-detection cases; rewrite to a linear pattern or a trim helper. A pattern is left as written and `resolved` **only** when existing code bounds its input (OS path length, a constant, an operator-written line) so the cost cannot reach the budget; a pattern fed by repository, agent, model, or network input is real and must be fixed with its test or the feature stays open (FR-006) (session)
- [x] T018 [US2] Checkpoint C2: report every `real` verdict to the maintainer with controller, validation, sink, and effect before the fix merges (session; first one: stripPrivate, reported 2026-09-07)
- [x] T019 [US2] Append batch E rows to the ledger as planned states (`fixed #PR` or `resolved — <verdict>` with the Codacy `reason` enum), regenerate the record (session)
- [x] T020 [US2] [US5] Open PR `008-qd-e`; run the security review path of `rules/security.md` (semgrep CLI, `codex-review mode=security`, `codex:adversarial-review` on the changed lines) in addition to the standard gate; merge; write post-E counts (session) — post-E counts written to the record header 2026-09-07 (Sonar 279, Codacy 108 at 1816a503)

---

## Phase 4: User Story 1 — Every finding has a recorded disposition: mechanical rewrites (Priority: P1) — Batches B1, B2, B3

**Goal**: Close the ≈ 200 mechanical Sonar findings in code with value-preserving rewrites; any rewrite whose equivalence cannot be shown is skipped and recorded as `resolved` with the counter-example.

**Independent Test**: Sonar count on `main` falls to ≈ 95 after the three merges; `git diff main...HEAD -- test/` is empty for every batch; each Codex prompt quotes the research R5 rules verbatim (including the `replace`/`replaceAll` string-pattern and `null?.b` counter-examples) and each PR body lists the ids closed and the ids skipped with reasons.

- [x] T021 [P] [US1] Batch B1 (Codex, worktree `~/projects/free-mem-wt/b1` on `008-qd-b1` from `main` after E): rewrite the mechanical Sonar findings allocated to B1 (`allocation.json`, tree `scripts/`, rules S7778, S6582, S3358, S4624, S7780, S7781, S7755, S7744, S6551, S6653, S1854, S3516, S6397, S7784, S6353, S7688, S7785, S6594, S7741, S7726, S7765, S1994, S1066, S7786); prompt includes R5 verbatim, "skip and list any rewrite whose equivalence you cannot show from the code or a test", and `node --test 'scripts/e2e/**/*.test.mjs'` as the check
- [x] T022 [P] [US1] Batch B2 (Codex, worktree `~/projects/free-mem-wt/b2` on `008-qd-b2`): same for the ids allocated to B2 (tree `src/`, files not session-owned; S8786 and S107 are not in this batch); check = unit + migration suites
- [x] T023 [US1] Batch B3 (session, worktree `~/projects/free-mem-wt/008` on `008-qd-b3`): the ids allocated to B3 (`src/injection/*.ts`, `src/mcp.ts`, `src/viewer/**`, `src/transfer.ts`, `src/db/queries.ts`, `src/privacy/*.ts`), same rules, unit suites unmodified
- [x] T024 [US1] Review each Codex batch (`/code-review` on the worktree path, then `ponytail-review`), then commit with sign-off using `git -C ~/projects/free-mem-wt/b1` / `git -C ~/projects/free-mem-wt/b2` on `008-qd-b1` / `008-qd-b2` and push from there; the shared checkout is not touched; never `cd` into a worktree and chain further git commands in the same call (session) — B1 (#161), B2 (#162), B3: codex diff review ok:true and `/code-review` with adversarial verification on each, ponytail pass, committed with sign-off
- [ ] T025 [US1] Append B1–B3 rows to the ledger as planned states: closed ids `fixed #PR`, skipped ids `resolved` with the one-sentence counter-example; regenerate the record (session)
- [ ] T026 [US1] [US5] Open and gate PRs `008-qd-b1`, `008-qd-b2`, `008-qd-b3` one at a time (gate as T012), merge each with a merge commit, rebase the next on `main`, keep `main` green between merges; after each merge's analysis run `--confirm` for that batch's rows and write post-B counts into the record — #161 (B1) merged f874ae83, its 118 Sonar rows confirmed against analysis 996c72b5 (Sonar 160 / Codacy 110 on main); #162 (B2) open

---

## Phase 5: User Story 3 — Complexity reduced without behaviour change (Priority: P2) — Batches C1–C4

**Goal**: Sonar S3776 (58) and S107 (4), Codacy function length (73 incl. 6 in `scripts/e2e/*.test.mjs`) and parameter count (3) closed by extraction refactors; existing tests untouched; budgets unchanged and measured.

**Independent Test**: Sonar S3776 count 0 on `main` (or `resolved` rows with reasons for table-driven switches), Codacy `Lizard_nloc-medium` and `Lizard_parameter-count-medium` count 0 outside `test/`; base-vs-candidate resource runs within the research R7 tolerances in every hook-path PR body; the candidate-bundle dogfood line in the C4 PR body; the daily dogfood stays green after each merge.

- [X] T027 [P] [US3] Batch C1 (Codex, worktree `~/projects/free-mem-wt/c1` on `008-qd-c1` from `main` after B): extraction refactors for the ids allocated to C1 (S3776, S107 on `providerCall` and `processBatch`, Codacy function length and parameter count) in `src/worker/observe.ts`, `src/worker/batches.ts`, `src/worker/purge.ts`, `src/observer/llm.ts`, `src/observer/classify.ts`, `src/observer/catalog.ts`, `src/observer/fallback.ts`, `src/observer/request.ts`, `src/observer/contract.ts`; prompt: extract single-exit blocks into named functions, an options object for a >7-parameter function with every call site updated, keep every export and signature otherwise, no reordering, unit suites unmodified, list any function left over 15 / 50 with the reason
- [X] T028 [P] [US3] Batch C2 (Codex, worktree `~/projects/free-mem-wt/c2` on `008-qd-c2`): same for the ids allocated to C2 (incl. S107 on `write` in `src/capture.ts`) in `src/capture.ts`, `src/agents/claude.ts`, `src/agents/grok.ts`, `src/agents/pi.ts`, `src/agents/codex.ts`, `src/agents/index.ts`, `src/setup/setup.ts`, `src/setup/write-codex.ts`, `src/setup/managed-block.ts`, `src/setup/probe.ts`, `src/setup/detect.ts`, `src/doctor.ts`, `src/doctor/agents.ts`, `src/doctor/provider.ts`, `src/doctor/storage.ts`, `src/repo-identity.ts`, `src/events.ts`, `src/db/open.ts`, `src/retrieval/fts.ts`, `src/retrieval/rank.ts`
- [X] T029 [US3] Batch C3 (session, `008-qd-c3`, PR #164): same for the ids allocated to C3 in `src/injection/pack.ts`, `src/injection/inject.ts`, `src/injection/deferred.ts`, `src/transfer.ts`, `src/privacy/detect.ts`, `src/fixture/replay.ts`, and `src/viewer/app/main.tsx` (`App`, 73 NLOC; front end stays in this session). Done: 9 Sonar S3776 and 8 Codacy `Lizard_nloc-medium` ids, extraction-only; `tsc`/`eslint` clean, 867 + 171 tests pass; every function in the changed files under the Codacy bound of 50 NLOC by `pipx run lizard -l typescript -T nloc=50 -w`, the tool Codacy runs (largest 46; the same command reproduces six of the eight Codacy rows on `main` exactly); no cognitive-complexity warning `main` does not already have; the T068 fixture replay on this bundle reproduces the recorded structural numbers (hooks 1143/1143, lifecycle 3/4/4/4, SC-005 0, SC-010 0, SC-009 7/40, rows 1322/87/292/5464)
- [ ] T030 [P] [US3] Batch C4 (Codex, worktree `~/projects/free-mem-wt/c4` on `008-qd-c4`): S3776, S107 (`launchAgent` 9 parameters → one options object with the same call sites), function-length rows incl. the 6 in `scripts/e2e/isolated-user.test.mjs`, and parameter-count rows in `scripts/e2e/**`, `scripts/fixtures/generate-1000-events.mjs`, `scripts/measure-cold-start.mjs`; check = `node --test 'scripts/e2e/**/*.test.mjs'`
- [ ] T031 [US3] Review C1, C2, C4 (`/code-review` + `ponytail-review`), commit and push with `git -C` in each worktree as in T024 (session)
- [X] T032 [US3] Resource check per quickstart.md for `008-qd-c1`, `008-qd-c2`, `008-qd-c3`: base SHA and candidate SHA built in their own checkouts, `node scripts/measure-cold-start.mjs` (Markdown) and `node dist/oboete.mjs fixture replay test/fixtures/events-1000.jsonl --json` on each, exit codes checked, same hour, medians within 15 %, maxima within 15 % and under budget, every replay structural verdict green and identical on both sides (SC-009 recall is the M2 embedding target and is red on `main`, so it is compared, not required green); both result sets in the PR body (session)
- [ ] T033 [US3] Candidate-bundle dogfood for `008-qd-c4` per quickstart.md and research R7: pack the candidate and record the tarball and `dist/oboete.mjs` hashes; install into `~oboete-dogfood/candidate`; keep the real account `HOME` (`sudo -u oboete-dogfood -H`) and run `oboete setup` from that prefix with `OBOETE_HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `GROK_HOME`, `PI_CODING_AGENT_DIR` pointed at `cp -a` copies of the daily directories so the copies carry the logins and consent and their agent configurations reference the candidate bundle; verify by grep that every hook/MCP entry in the copies points at the candidate bundle and none at the daily install, and by `sha256sum` that the installed bundle equals the packed one; then `isolated-user.mjs --daily --pairs all` under the same variables and `PATH` (and `--lifecycle --agents claude,codex` if lifecycle code changed); candidate SHA, hashes, run id, `12 of 12` line, doctor table in the PR body; the daily cron's install, home, and agent configurations untouched (session)
- [ ] T034 [US3] Append C1–C4 rows to the ledger as planned states: `fixed #PR` per id; functions deliberately left complex as `resolved` with the reason (table-driven switch, parser); regenerate the record (session)
- [ ] T035 [US3] [US5] Open and gate PRs `008-qd-c1`, `008-qd-c2`, `008-qd-c3`, `008-qd-c4` one at a time (gate as T012), merge each, keep `main` green; after each merge's analysis run `--confirm` for that batch's rows and write post-C counts

---

## Phase 6: User Story 3 (continued) — File-length seams (Priority: P2) — Batch D

**Goal**: The 13 measured file-length findings decided per file: 5 split at a real seam, 8 resolved as "single cohesive module" (the other 18 of the 31 were excluded by A per the research R4 table).

**Independent Test**: Each split creates a module with its own imports and moves its tests, no circular import (`npm run typecheck` and the existing suites pass); the 8 `resolved` rows name the file's single concern and its percentage over 500 NLOC; Codacy `Lizard_file-nloc-medium` count 0 outside `test/` after batch F.

- [ ] T036 [US3] Confirm the research R7 seam table by reading each of the 12 files at the post-C state; update the table in `specs/008-quality-debt-zero/research.md` if a decision changes, with the reason (session)
- [ ] T037 [P] [US3] Batch D (Codex, worktree `~/projects/free-mem-wt/d` on its own branch `008-qd-d-codex` from `main` after C): split `src/worker/observe.ts` (provider call / retry → `src/worker/provider-call.ts`), `src/capture.ts` (store-or-spool write path → the seam found in T036), `scripts/e2e/isolated-user.mjs` (`startLifecycleTui` → `scripts/e2e/lifecycle-tui.mjs`), `scripts/fixtures/generate-1000-events.mjs` (data tables → `scripts/fixtures/data/*.json`); move the matching tests; no export renamed
- [ ] T038 [P] [US3] Split `src/fixture/replay.ts` at the seam found in T036 (timing table and report rendering vs. lifecycle driver) (session, `008-qd-d` in `~/projects/free-mem-wt/008`, disjoint files from T037)
- [ ] T039 [US3] Review the Codex part of D, then `git -C ~/projects/free-mem-wt/008 merge --ff-only 008-qd-d-codex` (rebase the Codex branch onto `008-qd-d` first if the session commit landed earlier) so one branch `008-qd-d` holds both parts; resource check as in T032 for the `capture.ts` and `observe.ts` splits; candidate-bundle dogfood as in T033 with `--lifecycle --agents claude,codex` because the lifecycle TUI moved (session)
- [ ] T040 [US3] Append batch D rows as planned states: 5 ids `fixed #PR`, 8 ids `resolved — single cohesive module (N % over the 500-line threshold)` with Codacy reason `AcceptedUse`; regenerate the record (session)
- [ ] T041 [US3] [US5] Open and gate PR `008-qd-d`, merge, `--confirm` its rows after the analysis, write post-D counts

---

## Phase 7: User Story 1 — Service-side calls, confirmations, and the 0 / 0 check (Priority: P1) — Batch F

**Goal**: Every planned `resolved` row applied on its service with reason and comment; every planned `fixed` / `excluded` row confirmed against the analysis that dropped it; the record complete and checked; both services at 0 against the final `main` SHA; no repository edit after that SHA.

**Independent Test**: `node scripts/quality-debt-record.mjs --check` exits 0 with `707 ids: 0 missing, 0 duplicate, 0 open, 0 unconfirmed, 0 resolved-without-reason`; the final-analysis confirmation in quickstart.md succeeds for both services with `revision` / commit equal to the final SHA; the two count commands print `sonar open 0` and `codacy current 0`; a random sample of 20 rows traces to a PR, a service comment, or a configuration line.

- [ ] T042 [US1] `node scripts/quality-debt-record.mjs --apply-sonar --dry-run`, review the list, then `--apply-sonar`: every Sonar `resolved` row transitioned (`wontfix` / `falsepositive`) and commented with the ledger reason; the HTTP status lands in `confirmed` (session)
- [ ] T043 [US1] `--apply-codacy --dry-run`, review, then `--apply-codacy`: every Codacy `resolved` row ignored through `PATCH …/issues/{hexId}` with its enumerated `reason` and the ledger sentence as `comment`, using `~/CODACY_TOKEN.md` line 2 (verified with `GET /api/v3/user` first); the HTTP status lands in `confirmed`; a refused call stops the run and is reported with the id (session)
- [ ] T043a [US4] Disable Codacy's ESLint tool for the repository (`PATCH …/tools/f8b29663-2cb2-498d-b923-a10c6a8c05cd` `{"enabled": false}`; decision C3, research R10), verify with `GET …/tools` that only ESLint changed, and append a tool-health row to the record (reason, the 1,102 local estimate, the HTTP response); the next `main` analysis log must not list an ESLint step (session)
- [ ] T043b [US1] [US5] Before opening `008-qd-f`: every merged batch's rows confirmed (`--confirm --sonar-analysis <key> --codacy-commit <sha>` after each merge's analysis, per T012/T020/T026/T035/T039) and `--check --planned` reporting 0 unconfirmed among ledger rows, so the ledger F carries is the confirmed one; the tool-disable row of T043a in the record (session)
- [ ] T044 [US1] Polish inside this batch (no repository edit may follow the final SHA): update `specs/008-quality-debt-zero/plan.md` and `research.md` with the final counts and any seam decision that changed, mark SC-001…SC-008 with their evidence line in `docs/evidence/quality-debt-2026-09.md`, regenerate the record (session)
- [ ] T045 [US1] [US5] Open and gate PR `008-qd-f` (record + ledger + apply modes + polish), merge, `git -C ~/projects/free-mem pull --ff-only`, wait for both services' analysis of the merge commit, then the final-analysis confirmation of quickstart.md (Sonar `revision` equal to that SHA; Codacy `commit.sha` equal to it with `endedAnalysis` set and every required step `success` and no ESLint step in `commits/{sha}/logs` (decision C3)), then the count commands; run `--confirm` for F's own rows **read-only** (the confirmed ledger for those rows, if any changed, goes into a small follow-up PR that is itself confirmed the same way, so the repository never holds an unconfirmed ledger); if either count is not 0, disposition the new ids in the same three states in a further PR and repeat; when both are 0, run `--check` (exit 0) and post the 0 / 0 evidence (analysis ids, SHA, timestamps, counts) as a comment on PR `008-qd-f` and in memory, not as a repository edit (session)

---

## Phase 8: Polish (outside the repository)

- [ ] T046 Run `speckit-verify-tasks` for this feature in a fresh session (after_implement hook) and link the report from the PR `008-qd-f` comment
- [ ] T047 Update memory `oboete-rebuild-state` with the end state (both services 0, PR numbers, final SHA); post the `speckit-verify-tasks` report (T046) as a PR comment before any cleanup; only then remove the session worktree `~/projects/free-mem-wt/008` and every batch worktree; delete the merged branches

---

## Dependencies

- Phase 1 → Phase 2 (A, judged by analysis) → Phase 3 (E, alone) → Phase 4 (B1 ∥ B2 Codex while B3 here) → Phase 5 (C1 ∥ C2 ∥ C4 Codex while C3 here) → Phase 6 (D, Codex on `008-qd-d-codex` folded into `008-qd-d`) → Phase 7 (F, needs the Codacy token; owns no ids) → Phase 8 (outside the repository).
- US4 (batch A) runs first although it is P2, because it removes ≈ 300 findings that every later batch would otherwise re-read; US2 (E) runs before any code batch because its files overlap B and C; US1 and US2 remain P1 and are the acceptance bar.
- At most three Codex jobs at once; commits and fold-ins use `git -C <worktree>` and never a `cd`-chained command; the shared checkout stays on a clean `main` for the hourly evidence cron throughout and only advances with `git pull --ff-only` after a merge.

## Parallel execution examples

- Phase 4: `T021 (Codex b1)`, `T022 (Codex b2)` in worktrees while `T023` (B3) is written in `~/projects/free-mem-wt/008`.
- Phase 5: `T027 (c1)`, `T028 (c2)`, `T030 (c4)` as three Codex jobs while `T029` (C3) is written here.
- Phase 6: `T037 (d)` Codex while `T038` (replay.ts) is written here.

## Implementation strategy

1. **MVP = Phase 2 (batch A) + its analysis**: after A alone, both dashboards drop by ≈ 300 and the record shows every excluded id with its file class, which already delivers US4 and the record mechanism of US1.
2. Then E for the security bar (US2), B for the bulk of Sonar (US1), C and D for the refactors (US3), F to close (US1).
3. Stop points: after any batch, `main` is green and `--check --planned` reports the remaining `open` count; the feature can pause with that count stated, but it is complete only when `--check` (confirmed states) passes and both services report 0 against the final SHA.
