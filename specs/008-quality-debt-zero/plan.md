# Implementation Plan: Quality Debt to Zero

**Branch**: `008-quality-debt-zero` (planning branch; work lands through per-batch branches `008-qd-<batch>` merged to `main`) | **Date**: 2026-09-07 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/008-quality-debt-zero/spec.md`

## Summary

Bring SonarCloud (310 open code smells) and Codacy (397 current issues) to zero on `main` without weakening any gate or changing behaviour. Four populations, four remedies, in this order: (1) one configuration pull request that stops rules that cannot apply to the file class (15 Sonar + 287 Codacy findings), judged complete only by the post-merge analysis; (2) security-flavoured findings read one by one in this session with written verdicts and any real fix merged first (≈ 30, of which ≈ 16 in `src/`); (3) mechanical rewrites delegated to Codex in rule-family batches (≈ 200 Sonar); (4) complexity and function-length refactors by extraction, delegated except for the security-owned modules and the viewer front end (Sonar S3776 58, Codacy function length 73 incl. 6 in harness tests, file length 31 with a decision for every file). Every finding ends as fixed / resolved-with-reason / excluded-by-file-class, recorded in `docs/evidence/quality-debt-2026-09.md`, and the end state is verified as 0 / 0 on the services after the last analysis.

## Technical Context

**Language/Version**: TypeScript 5 (`src/`, `test/`), ES modules JavaScript (`scripts/**/*.mjs`), SQLite migrations (`src/db/migrations/*.sql`), Bash (`scripts/e2e/dogfood.sh`), Node 22.16 / 24.x as in CI.

**Primary Dependencies**: No new runtime dependency. Analysis services: SonarCloud (project `ojungo69_free-mem`, Sonar way profile, API token in `~/SONAR_TOKEN.md`), Codacy (repository `ojungo69/oboete`, tools Opengrep, Lizard, TSQLLint, SQLint, markdownlint, Stylelint, ShellCheck). Optional tooling: `@codacy/codacy-cloud-cli` 1.6.0 (needs an account API token, see checkpoint C1).

**Storage**: N/A for the feature itself. The SQLite migration files are touched only by analysis configuration, never edited (their bytes feed the schema hash).

**Testing**: Existing suites unchanged: `node --test` over `build/test/unit/**`, `build/test/migrations/**`, `scripts/e2e/**/*.test.mjs`; E2E bundle tests in CI; `scripts/measure-cold-start.mjs` and `oboete fixture replay test/fixtures/events-1000.jsonl --json` for hook-path timing (base SHA vs candidate SHA, research R7); candidate-bundle isolated dogfood (`isolated-user.mjs --daily --pairs all`, plus `--lifecycle` when lifecycle code moves) for harness batches.

**Target Platform**: Linux (WSL2) development host; CI on ubuntu-latest.

**Project Type**: CLI + hooks + local MCP server + viewer (single package); this feature is a cross-cutting cleanup.

**Performance Goals**: Unchanged budgets: capture hook 300 ms, injection 1,300 ms, viewer response as measured in T068. Hook-path refactors must not move the resource-fixture medians by more than the run-to-run noise already recorded (T067 evidence).

**Constraints**: No behaviour change (FR-007, FR-008); no gate weakened (FR-003); security-owned modules, the viewer front end, and real security findings written in this session (FR-006, CLAUDE.md frontend rule); Grok delegation paused until 2026-09-12 so Codex is the only external implementer; the daily dogfood evidence cron commits in the shared checkout hourly at :20 and requires it on `main` and clean, so the shared checkout is **pinned to `main` for the whole feature** and all editing (session and Codex) happens in linked worktrees (research R8).

**Scale/Scope**: 734 findings (321 Sonar, 413 Codacy; allocation: A 302, E 41, B1 128, B2 57, B3 41, C1 27, C2 39, C3 17, C4 62, D 20) across ≈ 120 files — 707 in the 2026-09-07 export plus the 13 the live search reported on 2026-09-08 (T030a) the Claude probe file-length finding added on 2026-09-09 (T036), and thirteen PR #185 findings at `ebe687dc` (T043e); 12 pull requests; ≈ 30 service-side resolutions.

**Post-push inventory continuation** (2026-09-09): PR #185 at `ebe687dc` reports eleven new Sonar IDs on moved code and two native Codacy lifecycle file-length IDs. Read each source and its baseline before assigning a disposition. Append these service IDs without dropping existing rows, regenerate allocation and the record, and update the current totals. The newly observed Sonar S4036/S8707 security rules are deliberately unallocated by the old fixed rule list; extend the existing shared security classifier so both allocation to E and mandatory verdict validation cover these two rules. Keep unknown rules unallocated. This affects only the record CLI, never the candidate engine, and adds no flag or service mutation. Verify the allocation and missing-verdict failures before the change, then the existing CLI tests on Node 22/24, lint, generated-record consistency and the unchanged packed engine hash; review the final delta before committing.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | Note |
|---|---|---|
| I. Automatic, agent-neutral memory | Complies | No change to capture, storage, retrieval, or injection semantics; refactors are extraction-only (FR-007). |
| II. One file, no daemon | Complies | No process model change. |
| III. Local-first, fail-closed classification | Complies | `src/privacy/*` and the injection modules are refactored only in this session; any S8786 regex change in `privacy/detect.ts` gets a targeted test for the input class it guards (research R5, R6). |
| IV. Honest degradation, bounded resources | Complies | Budgets untouched; hook-path refactors compared against the resource fixture (research R7). |
| V. Parity target and milestones | Complies | No milestone content; this is M1 hygiene before M2. |
| VI. Portable and minimal | Complies | No new dependency; the Codacy CLI is run ad hoc with `npx`, not added to the package. |
| Workflow: isolated branch and worktree; shared checkout never rewritten | Complies | The hourly evidence cron is a second concurrent editor of the shared checkout, which is the case in which CLAUDE.md (2026-09-06) allows worktrees. The shared checkout stays on `main`; the session edits in `~/projects/free-mem-wt/008`, each Codex job in its own worktree (research R8). |
| Workflow: security changes not delegated | Complies | Security-owned modules and real findings are written here (research R8). |
| Workflow: smallest failing test for new behaviour | Complies | No new behaviour. A real security finding gets a failing-then-passing test (FR-006). |
| Workflow: Spec Kit sequence | Complies | specify → clarify → plan (this) → tasks → implement → verify-tasks. |

No violation. Complexity Tracking is empty.

## Project Structure

### Documentation (this feature)

```text
specs/008-quality-debt-zero/
├── plan.md              # This file
├── research.md          # Phase 0: populations, service mechanics, seam table, verdict procedure
├── data-model.md        # Phase 1: Finding / Disposition / Batch / Exclusion and the record format
├── quickstart.md        # Phase 1: how to verify each batch and the end state
├── checklists/requirements.md
└── tasks.md             # Phase 2 (/speckit-tasks)
```

No `contracts/` directory: the feature exposes no interface. The disposition record format is in `data-model.md`.

### Source Code (repository root)

```text
sonar-project.properties          # + sonar.plsql.file.suffixes=pks,pkb
.codacy.yml                       # rewritten: exclude_paths + per-engine exclude_paths (research R4)
.markdownlint.json                # new: default false + the standard's possible rules + MD024 siblings_only (Codacy reads it on its own)
docs/evidence/quality-debt-2026-09.md          # new: disposition record (data-model.md format)
docs/evidence/quality-debt-2026-09/*.json      # new: the two inventories (they grow when a service reports a finding no row covers) + batch allocation
scripts/quality-debt-record.mjs   # new: CLI, allocation, record generation, --check
scripts/quality-debt-ledger.mjs   # new: ledger read/write and validation shared by the CLI and the service module
scripts/quality-debt-services.mjs # new: --apply-sonar / --apply-codacy / --confirm (the only module that talks to a service)

src/                              # mechanical rewrites and extraction refactors; no file added except by a seam split:
├── capture.ts                    #   split candidate (research R7)
├── worker/observe.ts             #   split candidate
├── fixture/replay.ts             #   split candidate
├── injection/, mcp.ts, viewer/server.ts, viewer/app/, transfer.ts, db/queries.ts, privacy/   # session-owned
└── (all other src/ files)        #   Codex batches

scripts/
├── e2e/isolated-user.mjs         #   split candidate (TUI module)
├── fixtures/generate-1000-events.mjs   # split candidate (data tables → JSON)
├── e2e/dogfood.sh                #   SC2024 verdict
└── (other scripts/*.mjs, scripts/e2e/**)   # Codex batches

test/                             # matching tests follow extracted modules; assertions stay unchanged
```

**Structure Decision**: Existing single-package layout. New files are limited to analysis configuration, the disposition record, and the source/test seams listed in research R7. Added tests cover previously untested rendering/frame/fixture-validation boundaries.

## Batches and order

**What one pull request may contain** (FR-009): one *rule family* × one *tree*, or one *module area*. The two rule families are *mechanical* (the value-preserving rewrites of research R5, reviewed with one checklist) and *structural* (extraction refactors for complexity and length). Trees are `src/` (split further into session-owned and Codex-owned module areas) and `scripts/`. The allocation of every inventory id to exactly one batch is generated into `docs/evidence/quality-debt-2026-09/allocation.json` by `scripts/quality-debt-record.mjs` from the rules below and checked (`--check`) so that no id is unassigned or assigned twice.

Each batch is one pull request; each passes the PR #155 gate before merge; `main` returns to green before the next merge. Batches marked *Codex* run as background jobs in their own worktree; batches marked *session* are written here.

| # | Batch (branch `008-qd-<#>`) | Owner | Scope rule for the allocation | Findings closed (Sonar / Codacy) | Depends on |
|---|---|---|---|---|---|
| A | Analysis configuration (research R4) + inventories + record generator | session | Sonar `plsql:*`; Codacy ids under the excluded file classes, the 8 MD024 rows (`.markdownlint.json`), the Stylelint row (pattern disable) | 302 (allocation: 15 + 287) | — |
| E | Security-flavoured verdicts and fixes: Sonar S8786 ×13; Codacy timing 6, RegExp 7 (3 in `src/`, 2 test constants in `scripts/dco-check.test.mjs`, and the 2 the 2026-09-08 refresh re-issued for the same two `src/agents/index.ts` constructors), SSRF 1, prototype pollution 2, dynamic method 2, tainted SQL 2, SC2024 2, ci.yml 1 | session | Sonar S8786/S4036/S8707; Codacy Opengrep + ShellCheck patterns not excluded by A | 41 (34 original, 2 later Codacy findings and 5 PR #185 security findings; later rows are reviewed individually) | A analysed |
| B1 | Mechanical rewrites, `scripts/**` | Codex | Sonar mechanical rules, file under `scripts/` | 128 (allocation; includes 3 PR #185 IDs) | E |
| B2 | Mechanical rewrites, Codex-owned `src/**` | Codex | Sonar mechanical rules, file under `src/` and not session-owned | 57 (allocation; includes 2 PR #185 IDs) | E |
| B3 | Mechanical rewrites, session-owned `src/**` (`injection/`, `mcp.ts`, `viewer/`, `transfer.ts`, `db/queries.ts`, `privacy/`, `fixture/replay.ts`) | session | Sonar mechanical rules, session-owned file | 41 (allocation; includes 1 PR #185 ID) | E |
| C1 | Structural, `src/worker/**` + `src/observer/**` (incl. S107 / parameter count on `providerCall`, `processBatch`) | Codex | Sonar S3776 + S107, Codacy function length + parameter count, file in the area | 27 (allocation) | B2 |
| C2 | Structural, `src/capture.ts` (incl. S107 on `write`), `src/agents/**`, `src/setup/**`, `src/doctor*`, `src/repo-identity.ts`, `src/events.ts`, `src/db/open.ts`, `src/retrieval/**` | Codex | as above | 39 (allocation) | B2 |
| C3 | Structural, session-owned `src/**` incl. `src/viewer/app/main.tsx` (`App`, 73 NLOC) and `src/fixture/replay.ts` | session | as above | 17 (allocation) | B3 |
| C4 | Structural + S107 (`launchAgent`) in `scripts/**` incl. `scripts/e2e/*.test.mjs` (6 function-length rows); candidate-bundle dogfood before merge | Codex, dogfood by session | Sonar S3776/S107, Codacy function length + parameter count, file under `scripts/` | 62 (allocation) | B1 |
| D | File-length seams in all 18 measured files (R7): 13 original findings fixed, 5 cohesive residuals resolved; two new over-limit lifecycle modules assessed and their native IDs recorded | Session integrates `008-qd-d`; capture/worker and harness work stays in scoped linked worktrees, injection/replay owned by the session | Codacy file length not excluded by A | 20 IDs (18 original and 2 new lifecycle findings) | C1–C4 |
| F | Service-side calls and confirmations: Sonar transitions + comments, Codacy ignores with reason + comment (API, hex ids), confirmation of every planned `fixed` / `excluded` row against the analysis that dropped it, polish edits, then 0 / 0 against the final `main` SHA | session | owns no ids (research R9) | 0 (records confirmations) | all, token in hand |

Parallelism: E alone first (session). Then B1 ∥ B2 (Codex) while B3 is written here; then C1 ∥ C2 ∥ C4 (Codex) while C3 is written here; then D; then F. At most three Codex jobs at once, each in `~/projects/free-mem-wt/<batch>` on its own branch `008-qd-<batch>` (a Codex batch that shares a pull request with session work, as in D, uses `008-qd-<batch>-codex` and is folded into the session branch with `git -C ~/projects/free-mem-wt/008 merge --ff-only`); the session's worktree is `~/projects/free-mem-wt/008`; the shared checkout stays on `main` and is only advanced with `git pull --ff-only` after a merge (research R8; memory `fold-in-runs-in-the-worktree-when-chained-after-cd`).

## Checkpoints for the maintainer

### Batch D continuation, 2026-09-09

PR #185 retains FR-016 as written. Whether a first extraction closes the 500-NLOC finding is
not a seam criterion. Re-read the twelve retained files, move each independently testable
concern, then measure and justify any remaining cohesive module. The six completed extractions
remain unchanged. The handoff's current checkout layout supersedes the original cron layout
above: the integration checkout stays on `008-qd-d`; each concurrent writer uses its own linked
worktree. PR #186's CLI prerequisites are already merged.

| Area | Extraction scope | Verification |
|---|---|---|
| Capture | Process/CLI adapter to `src/capture-command.ts` and pure epoch state transitions to `src/capture-compaction.ts`; the capture transaction, deadline and hook protocol stay in `capture.ts`. Update direct imports and move the adapter tests without changing assertions. | Capture, hook and storage-fault tests; compare all moved bodies and the bundled runtime. |
| Worker | Batch application to `src/worker/observe-batch.ts`; imported-memory maintenance to `src/worker/imported.ts`; preserve lease fencing, consent callbacks, retry order and exact dependency types. Keep citation maintenance separate if its independently testable concern remains in an over-limit module. | Observe, privacy, lease, staleness and worker/provider fault tests. |
| Injection | Pi command to `src/injection/pi.ts`; pure pack framing/items to `src/injection/pack-format.ts`. Keep common delivery/validation in `inject.ts`, and selection/ledger in `pack.ts`; update every importer and move direct tests. | Injection, pack, deferred, degradation, why and agent-fault tests; privacy review. |
| Replay | Separate completed-run measurement and evidence rendering from the replay driver, retaining exact output, measurements and exit verdicts. | Replay tests, identical report output from the same recorded inputs, full fixture replay. |
| Isolated harness | Separate lifecycle state assertions, lifecycle execution, and the process operations shared by pair/lifecycle modes. Move the corresponding existing tests; keep the CLI entrypoint, event barriers and cleanup ordering. | All harness tests, import-cycle check, all-pair and lifecycle candidate dogfood. |
| Fixture generator | Move the post-generation coverage validator, retaining emission order and all validation. | Generator output byte equality and coverage validation. |
| Probe helpers and suites | Separate process runtime from frame/evidence decoding; MCP frame assertions and reports from its driver; Codex/Grok lifecycle and MCP probes from payload probes. Preserve every probe ID and discovery contract; helpers stay in `probe-lib/`. | All harness tests, probe discovery and affected real probe IDs. |

Review the resulting source boundaries before editing each area. No new dependency, timing
budget, public command, credential policy or gate definition belongs to these extractions.
Export names and signatures are preserved; internal import paths follow the moved definitions
as in T037, with no compatibility wrappers. Type-only references do not create runtime cycles.
Preserve moved function bodies and test assertions; use the compiler and existing suites to
verify the new imports. Measure all resulting files with Lizard and re-read any over-limit
residual before recording its disposition.

Runtime validation remains a separate gate. The ordinary replay strips provider credentials;
an isolated diagnostic may test the existing configured `--home` path with an operator-owned
preload restricted to the measured bundle's `observe` command. Such a diagnostic does not
change the product or prove acceptance: only full base/candidate results against the unchanged
bounds can satisfy T032/T039. The daily installation and credentials remain outside the edits.

- **C1 (before A's analysis and before F)**: A Codacy **account API token** is needed for (a) nothing for markdownlint any more (Codacy reads `.markdownlint.json` on its own, research R4), (b) disabling the one SCSS Stylelint pattern (`PATCH …/tools/{uuid}` with a `patterns` array; if refused because the repository follows the organisation standard, a per-issue ignore instead, never an edit of the shared standard), (c) ≈ 30 per-issue ignores (`PATCH …/issues/{hexId}` with reason and comment). Ask once at the start of A. Provided on 2026-09-07 as `~/CODACY_TOKEN.md` line 2 (verified against `GET /api/v3/user` before use). Without it the maintainer performs the same actions by hand from the list in the record. Either way the feature is **not complete** until the actions have run and the final `main` analysis reports 0 on both services (FR-002); a handed-over list is an intermediate artefact.
- **C2 (during E)**: If any security-flavoured finding is real, report it before fixing, with the controller, validation, sink, and effect, so the maintainer knows a real weakness existed in M1 alpha.
- **C3 (before F, found and decided 2026-09-07)**: Codacy's ESLint step has crashed on every analysed commit (research R10), so the 397 findings contain no ESLint finding. Decision: disable Codacy's ESLint tool for the repository (the repository's own ESLint 9 gate is the lint standard; Codacy's ESLint 8 cannot read it); the alternative, triaging the ≈ 1,100 findings the tool would surface, was declined. The feature's 0 / 0 is not written while the step still appears in the analysis log.

## Phase 0: Research

Complete: [research.md](./research.md) (R1–R9). No NEEDS CLARIFICATION remained.

## Phase 1: Design

- [data-model.md](./data-model.md): Finding, Disposition, Batch, Exclusion; the disposition record table format; state rules.
- [quickstart.md](./quickstart.md): per-batch verification commands and the end-state check.
- `contracts/`: not applicable (no external interface).

## Constitution Check (post-design)

Unchanged from the pre-research check: all principles comply; the worktree layout of research R8 keeps the shared checkout on `main` for the cron and puts every editor in its own worktree.

## Complexity Tracking

No violations to justify.
