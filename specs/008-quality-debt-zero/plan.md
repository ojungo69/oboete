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

**Scale/Scope**: 707 findings (allocation: A 302, E 34, B1 125, B2 55, B3 40, C1 27, C2 39, C3 17, C4 55, D 13) across ≈ 120 files; 12 pull requests; ≈ 30 service-side resolutions.

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
docs/evidence/quality-debt-2026-09/*.json      # new: the two frozen inventories + batch allocation
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

test/                             # untouched except: a new test added next to a real security fix
```

**Structure Decision**: Existing single-package layout. New files are limited to analysis configuration, the disposition record, and modules created by the three-plus-two seam splits listed in research R7.

## Batches and order

**What one pull request may contain** (FR-009): one *rule family* × one *tree*, or one *module area*. The two rule families are *mechanical* (the value-preserving rewrites of research R5, reviewed with one checklist) and *structural* (extraction refactors for complexity and length). Trees are `src/` (split further into session-owned and Codex-owned module areas) and `scripts/`. The allocation of every inventory id to exactly one batch is generated into `docs/evidence/quality-debt-2026-09/allocation.json` by `scripts/quality-debt-record.mjs` from the rules below and checked (`--check`) so that no id is unassigned or assigned twice.

Each batch is one pull request; each passes the PR #155 gate before merge; `main` returns to green before the next merge. Batches marked *Codex* run as background jobs in their own worktree; batches marked *session* are written here.

| # | Batch (branch `008-qd-<#>`) | Owner | Scope rule for the allocation | Findings closed (Sonar / Codacy) | Depends on |
|---|---|---|---|---|---|
| A | Analysis configuration (research R4) + inventories + record generator | session | Sonar `plsql:*`; Codacy ids under the excluded file classes, the 8 MD024 rows (`.markdownlint.json`), the Stylelint row (pattern disable) | 302 (allocation: 15 + 287) | — |
| E | Security-flavoured verdicts and fixes: Sonar S8786 ×13; Codacy timing 6, RegExp 5 (3 in `src/`, 2 test constants in `scripts/dco-check.test.mjs`), SSRF 1, prototype pollution 2, dynamic method 2, tainted SQL 2, SC2024 2, ci.yml 1 | session | Sonar S8786; Codacy Opengrep + ShellCheck patterns not excluded by A | 34 (allocation) | A analysed |
| B1 | Mechanical rewrites, `scripts/**` | Codex | Sonar mechanical rules, file under `scripts/` | 125 (allocation) | E |
| B2 | Mechanical rewrites, Codex-owned `src/**` | Codex | Sonar mechanical rules, file under `src/` and not session-owned | 55 (allocation) | E |
| B3 | Mechanical rewrites, session-owned `src/**` (`injection/`, `mcp.ts`, `viewer/`, `transfer.ts`, `db/queries.ts`, `privacy/`, `fixture/replay.ts`) | session | Sonar mechanical rules, session-owned file | 40 (allocation) | E |
| C1 | Structural, `src/worker/**` + `src/observer/**` (incl. S107 / parameter count on `providerCall`, `processBatch`) | Codex | Sonar S3776 + S107, Codacy function length + parameter count, file in the area | 27 (allocation) | B2 |
| C2 | Structural, `src/capture.ts` (incl. S107 on `write`), `src/agents/**`, `src/setup/**`, `src/doctor*`, `src/repo-identity.ts`, `src/events.ts`, `src/db/open.ts`, `src/retrieval/**` | Codex | as above | 39 (allocation) | B2 |
| C3 | Structural, session-owned `src/**` incl. `src/viewer/app/main.tsx` (`App`, 73 NLOC) and `src/fixture/replay.ts` | session | as above | 17 (allocation) | B3 |
| C4 | Structural + S107 (`launchAgent`) in `scripts/**` incl. `scripts/e2e/*.test.mjs` (6 function-length rows); candidate-bundle dogfood before merge | Codex, dogfood by session | Sonar S3776/S107, Codacy function length + parameter count, file under `scripts/` | 55 (allocation) | B1 |
| D | File-length seams: split `worker/observe.ts`, `capture.ts`, `fixture/replay.ts`, `isolated-user.mjs`, `generate-1000-events.mjs`; won't-fix rows for the other 8 (research R4/R7 tables) | Codex on branch `008-qd-d-codex` (`observe.ts`, `capture.ts`, two scripts); session on `008-qd-d` (`replay.ts`), folded with `--ff-only` | Codacy file length not excluded by A | 13 (allocation: 5 fixed + 8 resolved) | C1–C4 |
| F | Service-side calls and confirmations: Sonar transitions + comments, Codacy ignores with reason + comment (API, hex ids), confirmation of every planned `fixed` / `excluded` row against the analysis that dropped it, polish edits, then 0 / 0 against the final `main` SHA | session | owns no ids (research R9) | 0 (records confirmations) | all, token in hand |

Parallelism: E alone first (session). Then B1 ∥ B2 (Codex) while B3 is written here; then C1 ∥ C2 ∥ C4 (Codex) while C3 is written here; then D; then F. At most three Codex jobs at once, each in `~/projects/free-mem-wt/<batch>` on its own branch `008-qd-<batch>` (a Codex batch that shares a pull request with session work, as in D, uses `008-qd-<batch>-codex` and is folded into the session branch with `git -C ~/projects/free-mem-wt/008 merge --ff-only`); the session's worktree is `~/projects/free-mem-wt/008`; the shared checkout stays on `main` and is only advanced with `git pull --ff-only` after a merge (research R8; memory `fold-in-runs-in-the-worktree-when-chained-after-cd`).

## Checkpoints for the maintainer

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
