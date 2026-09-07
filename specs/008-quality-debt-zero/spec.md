# Feature Specification: Quality Debt to Zero

**Feature Branch**: `008-quality-debt-zero`

**Created**: 2026-09-07

**Status**: Draft

**Input**: User description: "Quality debt to zero on SonarCloud and Codacy for the oboete repo (ojungo69/oboete, main at 5e03d67f). Goal: SonarCloud main branch shows 0 open issues and Codacy shows 0 current issues, without weakening any gate that currently binds and without changing observable behaviour of the oboete CLI, hooks, MCP server, viewer, or worker. [...] Each finding must end in exactly one of three states, recorded where reviewers can see it: fixed in code; marked as won't fix / false positive on the tool with a one-line reason; or excluded by configuration when the rule cannot apply to the file class. Security-flavoured findings must be examined individually by reading the code. Cognitive-complexity refactors must keep the existing unit tests green and must not change the hook budgets, deadlines, or wire formats. Work lands as a series of small PRs on main, each gated the same way as PR #155, and the daily dogfood cron keeps running against main throughout. Out of scope: new features, M2 work, changes to test coverage thresholds, and any edit to the gate definitions themselves."

## Starting Point

M1 alpha merged to `main` at 5e03d67f (PR #155, then PR #158). Both analysis services still list findings against `main`:

| Service | Open findings | Composition |
|---------|---------------|-------------|
| SonarCloud (`ojungo69_free-mem`, branch `main`) | 310 | All code smells. 0 bugs, 0 vulnerabilities, 0 hotspots. Maintainability rating A, debt ratio 0.4 %. `src/` 157, `scripts/` 153. |
| Codacy (`ojungo69/oboete`, `main`) | 397 current, 0 ignored | 176 "non-literal file system path", 100 "function length", 31 "file length", 19 "timing attack in string comparison", 14 "non-literal RegExp", 26 SQL-server-style rules on migration files, 9 "duplicate markdown headings", 4 "prototype pollution via loop", 3 "parameter count", 3 SSRF, 2 dynamic method call, 2 tainted SQL, 2 `sudo` with redirection, 6 others. |

Neither backlog blocks a merge today: the SonarCloud Quality Gate measures ratings on new code, and Codacy is not a required check. The backlog is nevertheless visible to every reviewer and to anyone evaluating the project, and it hides any genuine finding that lands among the noise.

The SonarCloud rule breakdown by count: cognitive complexity 58 (rated critical), consecutive multi-argument calls 38, optional chaining 28, nested ternary 26, nested template literal 22, `String.raw` 18, S8786 13, S7781 11, spread fallback 10, PL/SQL duplicated string 9, PL/SQL `CREATE OR REPLACE` 6, S7755 8, S6551 7, parameter count 4, S7776 4, invariant return 3, and a long tail of one to three each. The full list (issue key, rule, file, line) was exported from the SonarCloud API at the start of this feature and is the working inventory for planning.

## Clarifications

### Session 2026-09-07

- Q: How are Codacy's 100 "function length" and 31 "file length" findings handled? → A: Function length is fixed in code everywhere, alongside the cognitive-complexity refactors. File length is decided per file: a file is split only where it holds two or more independently testable concerns (a seam with its own imports and tests that can move without a circular dependency); a file that is one cohesive concern is resolved as "won't fix" with that reason. (User delegated the decision with "decide what is good in the long term"; this is that decision.)
- Q: Are complexity and function-length findings in the `scripts/e2e` harness fixed to the same standard as `src/`? → A: Yes. The harness is the regression instrument for M2 to M5 and its tests run in CI, so it is kept to the same readability standard. Safeguard: a pull request that refactors the harness runs the harness's own test files and one isolated dogfood run before merge, because the harness has no unit tests of its own. (User delegated the decision; long-term maintainability chosen over the cheaper "won't fix".)

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Every finding has a recorded disposition (Priority: P1)

A maintainer or reviewer opens SonarCloud or Codacy for `main` and sees zero open findings. For any finding that existed on 2026-09-07 they can find out what happened to it: it was fixed in a named pull request, it was marked "won't fix" or "false positive" on the service with a one-line reason, or it was excluded by a configuration change whose scope is narrow enough to be reviewed.

**Why this priority**: This is the whole feature. A backlog of unexplained findings is what the user asked to remove; a backlog of findings silently suppressed would be worse than the current state, because it would look clean while hiding real problems.

**Independent Test**: Query both services for `main`: open count 0 on each. Pick ten findings at random from the 2026-09-07 inventory and trace each to a merged pull request, a service-side resolution with a reason, or a configuration exclusion with a stated file class.

**Acceptance Scenarios**:

1. **Given** the 2026-09-07 inventory of 310 SonarCloud findings, **When** the feature is complete, **Then** SonarCloud lists 0 open issues on `main` and every inventory entry is either absent because the code changed, or resolved on the service with a reason.
2. **Given** the 2026-09-07 inventory of 397 Codacy findings, **When** the feature is complete, **Then** Codacy lists 0 current issues on `main` and each inventory entry is fixed, ignored with a reason, or covered by a configuration exclusion that names the file class it applies to.
3. **Given** a finding that was resolved on the service rather than in code, **When** a reviewer reads the resolution, **Then** the reason is one sentence that a person unfamiliar with the session can check against the code.

---

### User Story 2 - Security-flavoured findings are read, not pattern-dismissed (Priority: P1)

Some Codacy patterns carry security names: timing attack in string comparison, prototype pollution, SSRF, tainted SQL, non-literal RegExp, and non-literal file paths inside `src/`. The maintainer wants each of these examined by reading the code at that line, because a single real one (for example a token compared with ordinary string equality in the viewer) matters more than the other 700 findings combined.

**Why this priority**: The project's constitution puts secrets handling and the fail-closed boundary first. Dismissing a security pattern by rule name would violate the spirit of that principle even when the rule is usually noise.

**Independent Test**: For each security-flavoured finding in `src/`, a written one-line verdict exists (real and fixed / not applicable because ...), and the verdict cites the input source (operator-owned path, constant, validated value) rather than the rule name.

**Acceptance Scenarios**:

1. **Given** a "timing attack" finding on a comparison, **When** it is examined, **Then** a comparison of a secret (token, key, hash of a secret) is changed to a constant-time comparison, and a comparison of a non-secret (event name, agent identifier, file extension) is recorded as not applicable with that reason.
2. **Given** a "non-literal file path" finding in `src/`, **When** it is examined, **Then** the path's origin is named (the operator's home directory, a configuration value the operator wrote, a value derived from a validated repository path) and the finding is dismissed only if no agent-supplied or model-supplied string can reach it.
3. **Given** a security-flavoured finding whose examination shows a real weakness, **When** it is fixed, **Then** the fix is reviewed as a security change under the repository's security review process, not delegated to an external coding tool.

---

### User Story 3 - Complexity is reduced without changing behaviour (Priority: P2)

The 58 cognitive-complexity findings and the 100 function-length findings point at functions that are hard to review. The maintainer wants them split or simplified so that each piece can be read on one screen, while every existing test keeps passing and the hook budgets, deadlines, and on-the-wire formats stay exactly as specified in the M1 contracts.

**Why this priority**: These are the findings that improve future work rather than just the dashboard, but they also carry the highest risk of accidental behaviour change, so they come after the disposition rules are in place and after the security findings are settled.

**Independent Test**: The unit, migration, and E2E harness suites pass before and after each refactor with no test edited except to add a case; the recorded budgets and contract documents under the M1 specification are unchanged; the daily dogfood run stays green.

**Acceptance Scenarios**:

1. **Given** a function flagged for cognitive complexity, **When** it is refactored, **Then** the finding is gone, the function's public signature and observable behaviour are unchanged, and the existing tests for that module pass unmodified.
2. **Given** a refactor that touches a hook handler, **When** the daily dogfood run executes after it lands, **Then** all agent pairs still pass and the doctor report shows no new degraded item.
3. **Given** a complexity finding where splitting the function would make it harder to read (a single large table-driven `switch`, a parser), **When** the maintainer decides not to refactor, **Then** the finding is resolved on the service as "won't fix" with the reason, not left open.
4. **Given** a file flagged for length, **When** it is examined, **Then** it is split if it holds two or more independently testable concerns, and otherwise resolved as "won't fix" with the reason "single cohesive module"; either way the decision is listed in the plan.
5. **Given** a refactor inside the `scripts/e2e` harness, **When** its pull request is opened, **Then** the harness's own tests pass and one isolated dogfood run has passed on that branch before merge.

---

### User Story 4 - Configuration exclusions are narrow and reviewed (Priority: P2)

Some rules cannot apply to the files they flag: SQL Server rules on SQLite migration files, a markdown heading rule on files whose repeated headings are structural, and analysis of a manual E2E harness as if it were a published library. The maintainer wants these silenced by configuration rather than one at a time, but only where the exclusion cannot hide a real finding in the shipped code.

**Why this priority**: Configuration exclusions remove many findings with one change, but a broad exclusion is the classic way a dashboard gets clean while the code does not.

**Independent Test**: Every exclusion added names a file class (a directory or an extension) and a rule set, and the shipped `src/` tree is not covered by any exclusion added in this feature except where a rule is inapplicable to a whole file type.

**Acceptance Scenarios**:

1. **Given** a rule that applies to another database engine, **When** it fires on SQLite migration files, **Then** the rule is disabled for that file type and the change is reviewed in a pull request.
2. **Given** a rule that is valid for shipped code but noisy for the E2E harness, **When** an exclusion is considered, **Then** it is scoped to the harness directory only, and the same rule keeps running on `src/`.
3. **Given** any exclusion added by this feature, **When** a reviewer reads the pull request, **Then** they can see the count of findings it removes and the file class it covers.

---

### User Story 5 - The work lands in small, gated steps (Priority: P3)

The maintainer wants the cleanup delivered as a series of pull requests, each small enough to review in one sitting and each passing the same merge gate as M1, so that the daily dogfood cron on `main` is never blocked and a mistake can be reverted in isolation.

**Why this priority**: Ordering and packaging, not outcome. Necessary for safety but it does not change the end state.

**Independent Test**: Each pull request touches one rule family or one module area, passes every check that PR #155 passed, and `main` stays green between merges.

**Acceptance Scenarios**:

1. **Given** a batch of mechanical rewrites for one rule, **When** the pull request is opened, **Then** it names the rule, the number of findings it closes, and passes the full gate before merge.
2. **Given** two pull requests in flight, **When** one is merged, **Then** the other rebases cleanly or is re-reviewed, and neither leaves `main` with a failing check.

---

### Edge Cases

- A finding is fixed in code but the service does not re-analyse `main` promptly: the disposition record still points at the merged pull request, and the count is re-checked after the next analysis rather than assumed.
- A mechanical rewrite changes a runtime value (for example `String.raw` on a string that contained an escape the author meant to interpret): the change is reverted and the finding is resolved as "won't fix" with the reason.
- A refactor to reduce complexity would require touching a hook handler within the 300 ms capture budget: the refactor keeps the same code path length and the measured hook timing from the resource fixture is compared before and after.
- Codacy and SonarCloud flag the same line for different reasons: one code change is preferred over two service-side resolutions.
- A security-flavoured finding turns out to be real: the fix is a security change and is reviewed as one, with its own test, before the mechanical batches continue.
- The daily dogfood run fails during the feature: the failure is treated as a possible regression from the most recent merge before the soak continues, and the evidence cron's automatic issue is used as the record.
- Marking findings on the service requires a permission the session does not have: the disposition is recorded in the pull request or a tracking issue instead, and the service-side marking is listed for the maintainer.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Every finding in the 2026-09-07 inventories (310 on SonarCloud, 397 on Codacy) MUST end in exactly one of three states: fixed in code, resolved on the service with a one-line reason, or excluded by a configuration change that names a file class and rule set.
- **FR-002**: SonarCloud MUST report 0 open issues on `main`, and Codacy MUST report 0 current issues on `main`, at the end of the feature and after the next analysis of the final merge.
- **FR-003**: No gate that binds today MUST be weakened: the SonarCloud Quality Gate conditions, the coverage measurement and its threshold, CodeQL, semgrep, secret scanning, DCO, and the CI test jobs stay as they are.
- **FR-004**: Configuration exclusions MUST be scoped to a file class (directory or extension) and MUST NOT cover the shipped `src/` tree, except for rules that are inapplicable to a whole file type (for example rules for a different database engine on SQLite migration files).
- **FR-005**: Every security-flavoured finding (timing attack, prototype pollution, SSRF, tainted SQL, non-literal RegExp, and non-literal file path inside `src/`) MUST be examined individually with a written verdict that names the origin of the value in question.
- **FR-006**: A security-flavoured finding that is real MUST be fixed as a security change with its own failing-then-passing test and reviewed under the repository's security review process; it MUST NOT be delegated to an external coding tool.
- **FR-007**: Refactors that reduce complexity or function length MUST NOT change the public signature or observable behaviour of the function, MUST keep the existing tests passing without modifying their assertions, and MUST NOT alter hook budgets, deadlines, wire formats, or the M1 contract documents.
- **FR-008**: Mechanical rewrites (optional chaining, nested ternary, nested template literal, `String.raw`, spread fallback, consecutive calls, and similar) MUST preserve runtime values; a rewrite that would change a value is reverted and the finding is resolved as "won't fix" with the reason.
- **FR-009**: Work MUST land as a series of pull requests, each covering one rule family or one module area, each passing the same merge gate as PR #155.
- **FR-010**: `main` MUST stay green between merges, and the daily dogfood cron MUST keep running against `main` throughout; a dogfood failure after a merge is treated as a candidate regression before the next merge.
- **FR-011**: Service-side resolutions (won't fix, false positive, ignore) MUST carry a one-line reason that can be checked against the code by someone who did not take part in the work.
- **FR-012**: A disposition record MUST exist that maps every inventory entry to its end state, and it MUST be reachable from the repository (a document under the evidence directory or a tracking issue).
- **FR-013**: Complexity findings where a split would harm readability MAY be resolved as "won't fix" on the service, with the reason stated; they MUST NOT be left open.
- **FR-014**: The feature MUST NOT add features, change M2-scoped behaviour, or edit the gate definitions.
- **FR-015**: Function-length findings MUST be fixed in code in both `src/` and `scripts/`, in the same pull requests as the cognitive-complexity refactors for the same module, under the same behaviour-preservation rules as FR-007.
- **FR-016**: File-length findings MUST be decided per file: split when the file holds two or more independently testable concerns that can move without a circular dependency; otherwise resolved on the service as "won't fix" with the reason "single cohesive module". The plan MUST list the decision for each of the 31 files.
- **FR-017**: Complexity and function-length findings in the `scripts/e2e` harness MUST be fixed to the same standard as `src/`; a pull request that refactors the harness MUST pass the harness's own test files and one isolated dogfood run before merge.

### Key Entities

- **Finding**: One row on a service (SonarCloud issue key or Codacy issue id) with rule, file, line, severity, and message. Has exactly one end state.
- **Disposition**: The end state of a finding: `fixed` (pull request), `resolved` (service transition + reason), or `excluded` (configuration change + file class + rule). Recorded in the disposition document.
- **Batch**: A group of findings handled in one pull request, keyed by rule family or module area, with its own gate run.
- **Exclusion**: A configuration change on a service that stops a rule set for a file class. Carries the number of findings it removes.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: SonarCloud shows 0 open issues on `main` after the final merge's analysis, down from 310.
- **SC-002**: Codacy shows 0 current issues on `main` after the final merge's analysis, down from 397.
- **SC-003**: 100 % of the 2026-09-07 inventory entries have a disposition in the record, and a random sample of 20 can be traced to a pull request, a service resolution with a reason, or a named exclusion.
- **SC-004**: Every security-flavoured finding that is not excluded by file class has a written verdict (in `src/`: 6 timing-attack, 2 prototype-pollution, 1 SSRF, 2 tainted-SQL, 2 unsafe dynamic method, 3 non-literal-RegExp; plus the 2 harness `sudo` lines, the 1 `ci.yml` line, and the 13 super-linear regexes); any real one is fixed with a test.
- **SC-005**: All existing unit, migration, and E2E harness tests pass on every merged pull request with no assertion changed; the coverage figure on `main` does not fall below the value at 5e03d67f (93.3 %).
- **SC-006**: The SonarCloud Quality Gate conditions, CodeQL, semgrep, secret scanning, DCO, and the CI jobs are unchanged (their definitions have the same content before and after the feature).
- **SC-007**: The daily dogfood run passes on every day the feature is in progress, or each failure is traced to a cause and recorded.
- **SC-008**: No pull request in the series closes more than one rule family or module area, and every one passed the full merge gate before merge.

## Assumptions

- Service-side resolutions on SonarCloud and Codacy can be made with the credentials already available to the maintainer's tooling; if a resolution needs a permission the session lacks, the disposition is recorded in the repository and the marking is listed for the maintainer to apply.
- The Codacy pattern set and the SonarCloud "Sonar way" profile stay as they are; the feature does not switch quality profiles, and it does not raise or lower the Codacy "new issues" threshold.
- The SonarCloud and Codacy counts quoted above are the 2026-09-07 baseline; findings that appear during the feature because of new merges are handled in the same three states, and the end criterion is 0 on both services, not "baseline minus 707".
- The E2E harness under `scripts/e2e` is an operator-run tool that is not part of the published package. Readability rules (complexity, function length, mechanical rewrites) apply to it as to `src/`. Only rules whose premise is "untrusted input reaches this call" (for example non-literal file path on an operator-owned harness) may be scoped away from it by configuration, and the same rules keep running on `src/`.
- Mechanical rewrites and readability-preserving refactors outside the security-owned modules may be delegated to an external coding tool and are reviewed before merge; security-owned modules (injection, MCP server, viewer server, transfer, database queries) and any real security finding are handled by the maintainer's own session.
- The daily dogfood cron on the isolated account continues against `main` unchanged and is the behavioural regression signal for hook-path refactors, alongside the existing unit and E2E suites.
- The M1 contracts (`specs/007-oboete-m1-alpha/contracts/`) are the authority on budgets, deadlines, and wire formats; this feature does not amend them.
