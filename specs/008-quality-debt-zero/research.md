# Research: Quality Debt to Zero

**Date**: 2026-09-07 | **Feature**: [spec.md](./spec.md)

Every item below was checked against the live services or the vendor documentation on 2026-09-07. Counts are for `main` at 5e03d67f.

## R1. What the two backlogs actually contain

**Decision**: Treat the backlogs as four populations with different remedies: (a) rules that cannot apply to the file class, (b) mechanical rewrites, (c) complexity and length refactors, (d) security-flavoured findings that need reading.

**Evidence** (SonarCloud API `issues/search`, Codacy API v3 `issues/search`, both exported to the session scratchpad as `sonar-main-issues.json` and `codacy-main-issues.json`):

| Population | SonarCloud (310) | Codacy (397) |
|---|---|---|
| (a) inapplicable rule / file class | 15 PL/SQL rules on `src/db/migrations/*.sql` (S1192 ×9, CreateOrReplaceCheck ×6) | 26 TSQLLint + SQLint on the same two `.sql` files; 9 markdownlint MD024 (8 `docs/`, 1 `legacy/`); 1 Stylelint SCSS rule on `app.css`; 2 prototype-pollution in `legacy/`; 1 file-length on `package-lock.json`; lizard function/file length on `test/**` (27 + 17); Opengrep on `test/**` (13 timing, 4 RegExp, 1 SSRF, 1 fs-path); 175 fs-path + 7 RegExp + 1 SSRF on `scripts/e2e/**` |
| (b) mechanical rewrites | ≈ 200: S7778 38, S6582 28, S3358 26, S4624 22, S7780 18, S7781 11, S7744 10, S7755 8, S6551 7, S7776 4, S6653 3, S1854 3, S3516 3, and ≈ 25 singletons/doubletons | none (Codacy has no equivalent patterns enabled) |
| (c) complexity / length | S3776 58 (src 37, scripts 21), S107 4 | lizard function length ≥ 50 NLOC: src 42, scripts 31; file length ≥ 500 NLOC: src 6, scripts 7; parameter count 3 |
| (d) security-flavoured, must be read | S8786 13 (regex backtracking) | in `src/`: timing attack 6, non-literal RegExp 3, SSRF 1, prototype pollution 2, unsafe dynamic method 2, tainted SQL 2; in `scripts/`: ShellCheck SC2024 2 (`sudo cat > file` in `dogfood.sh`); in `.github/`: 1 "SonarQube API key" pattern on the pinned `sonarqube-scan-action` line |

Key observations that change the plan:

- Codacy's fs-path finding (176) has **zero hits in `src/`** (175 in `scripts/`, 1 in `test/`). The shipped code already passes the rule; the noise is the operator-run harness.
- Codacy analyses `test/`, `docs/`, `legacy/`, `.github/` and `package-lock.json` although `.codacy.yml` lists `include_paths: [src/**, scripts/**]`. Codacy's documentation states `include_paths` only defines exceptions to `exclude_paths`; on its own it excludes nothing. The current file is inert.
- All 6 "timing attack" findings in `src/` are on comparisons of a local worker-lease token, a trust hash, a null check, or an empty-string check, not on a network credential (see R6 for the verdict procedure; the verdicts themselves are produced in the implementation phase, not assumed here).

## R2. SonarCloud: how to close findings without code changes

**Decision**: Use the issue transition API with a comment for per-issue resolutions, and stop PL/SQL analysis of `.sql` files with `sonar.plsql.file.suffixes` in `sonar-project.properties`.

**Rationale**: The token in `~/SONAR_TOKEN.md` returns `transitions: [accept, confirm, resolve, falsepositive, wontfix]` on `issues/search?additionalFields=transitions`, so per-issue resolution is possible from this session. `sonar.plsql.file.suffixes` currently inherits `sql,tab,pkb`; setting it to `pks,pkb` in the project properties leaves `.sql` files with no language, which removes the 15 PL/SQL findings as "closed" on the next analysis. The T-SQL analyser is on `.tsql` only, so no other analyser claims `.sql`.

**Alternatives considered**: `sonar.exclusions=**/*.sql` (also works, but it hides the files from every future analyser, including a SQLite-aware one if Sonar ever ships it); `-- NOSONAR` per line (15 comments in migration files that must stay byte-stable for the schema hash, rejected).

**API shape** (verified against the SonarCloud Web API on 2026-09-07):

```
POST /api/issues/do_transition  issue=<key>&transition=wontfix|falsepositive
POST /api/issues/add_comment    issue=<key>&text=<reason>
```

## R3. Codacy: what repository configuration can do, and what needs an account token

**Decision**: Do everything possible through files in the repository first; use the Codacy Cloud CLI with an account API token for the remaining per-issue ignores and pattern changes.

**Rationale** (Codacy docs "Codacy configuration file" and "Configuring code patterns", read 2026-09-07):

- `.codacy.yml` supports top-level `exclude_paths` and per-engine `engines.<name>.exclude_paths` with Java glob syntax (`test/**` = everything under `test`). Engine names: `opengrep`, `lizard`, `tsqllint`, `SQLint`, `markdownlint`, `stylelint`, `shellcheck`.
- Tool configuration files in the repository supersede the UI pattern settings for that tool: `.markdownlint.json`, `.stylelintrc.json`, `.shellcheckrc`, `.tsqllintrc`, `.semgrep.yaml`. Lizard and SQLint have no repository configuration file.
- Per-issue ignore, tool enable/disable, and pattern parameters require an **account API token** (API v3 authenticated endpoints; the anonymous read that this session used works only because the repository is public). Endpoints, read from the generated client inside the published `@codacy/codacy-cloud-cli@1.6.0` bundle (`dist/api/client/services/AnalysisService.js`) on 2026-09-07:
  - `PATCH /api/v3/analysis/organizations/gh/ojungo69/repositories/oboete/issues/{issueId}` with body `{"ignored": true, "reason": "AcceptedUse" | "FalsePositive" | "NotExploitable" | "TestCode" | "ExternalCode", "comment": "<one sentence>"}`; `issueId` is the non-empty lowercase hexadecimal id from the inventory (observed at 30–32 characters, not fixed-width) and is sent unchanged. Codacy **does store a reason and a comment**. The CLI's `issue` command is not used because it parses its argument with `parseInt`, which cannot carry the hex id; the API is called directly.
  - `PATCH …/tools/{toolUuid}` with `{"useConfigurationFile": true}` (markdownlint uuid `547ff16b-d57c-4d75-8067-5ccf3ea88a00`), the "Use configuration file" switch.
  - `PATCH …/tools/{toolUuid}` with `{"patterns": [{"id": "Stylelint_scss_function-disallowed-list", "enabled": false}]}` (Stylelint uuid `1f03328a-086e-459e-bfa3-73e56f01020f`): the same endpoint as the configuration-file switch, with a `patterns` array, is how the CLI's `pattern` command (`src/commands/pattern.ts`, `configureTool`) changes one pattern. `PATCH …/tools/{toolUuid}/patterns` is a different call, a **bulk** update whose body is only `{"enabled": …}` applied to every pattern that matches the query filters (`categories`, `severityLevels`, `search`, …); it is not used, because one wrong filter changes the whole tool. After the call, `GET …/tools/{toolUuid}/patterns` is read and every pattern other than the target must still report the `enabled` value it had before. The repository follows the organisation's "Default coding standard" (`followsStandard: true` on every tool); if the repository-level call is refused for that reason, the standard is **not** edited (a coding standard applies to every repository that follows it, and this feature has no mandate outside `oboete`); the one finding is closed by a per-issue ignore with reason `FalsePositive` and the record says so. A pattern disable is a configuration change, so its row is `excluded` (confirmed by the next analysis), not `resolved`.
- Inline `nosemgrep` suppression is not documented as honoured by Codacy's Opengrep; not relied on.

**Issue ID width** (verified 2026-09-09): the [published API schema](https://api.codacy.com/api/api-docs/swagger.yaml) defines `CommitIssue.issueId`, `IgnoredIssue.issueId` and the PATCH `issueParam` as strings with no length restriction. The inventory's 30–32-character widths are observations, not a supported range; the CLI validates the known lowercase-hex form and preserves the service value without deriving a width limit from that sample.

**Configuration-file toggle**: Codacy only reads a repository configuration file for a tool after "Use configuration file" is switched on for that tool on the repository's Code patterns page (docs "Configuring code patterns", 2026-09-07). The switch needs a repository admin in the UI or the account token (`PATCH …/tools/{uuid}` with `useConfigurationFile: true`). Batch A therefore has a maintainer step before its analysis can be judged, and A is complete only when the post-merge analysis shows the expected drop, not when the files merge.

**Consequence**: The repository-file batch (R4) removes about 300 of the 397 Codacy findings. About 30 per-issue ignores remain (file-length "won't fix" on cohesive modules, security-flavoured findings judged not applicable, the `ci.yml` pattern hit). Those need a Codacy account API token (or the maintainer clicking each ignore). The token is a **prerequisite of batch F and of the feature's completion** (FR-002 requires 0 on the service); a list handed to the maintainer is an intermediate artefact, and the feature stays open until the actions have run and the final `main` analysis reports 0.

## R4. Repository-side configuration changes (the first pull request)

**Decision**: One pull request that only touches analysis configuration, with the count it removes stated per line.

| Change | File class | Findings removed | Why it cannot hide a real finding in `src/` |
|---|---|---|---|
| `sonar.plsql.file.suffixes=pks,pkb` | `.sql` | Sonar 15 | PL/SQL rules judge Oracle syntax; the files are SQLite. No other Sonar analyser reads `.sql`. |
| `.codacy.yml`: `exclude_paths: [legacy/**, package-lock.json, build/**, dist/**, coverage/**]` | historical tree, lockfile, build output | Codacy 4 (+ future) | `legacy/` is frozen evidence (constitution: historical, not product); lockfile is generated. Sonar already excludes the same paths. |
| `.codacy.yml`: `engines.tsqllint.exclude_paths` and `engines.SQLint.exclude_paths` = `src/db/migrations/**` | `.sql` | Codacy 30 | T-SQL and generic-SQL linters judge SQL Server / PostgreSQL dialect; the files are SQLite. |
| `.codacy.yml`: `engines.lizard.exclude_paths: [test/**]` | repository unit/fault tests under `test/` | Codacy 44 (27 function + 17 file) | Test bodies under `test/` are linear given/when/then scripts; splitting them scatters one scenario. `src/` and everything under `scripts/` (including `scripts/e2e/*.test.mjs`, 6 function-length rows fixed in batch C4 per FR-015/FR-017) stay measured. |
| `.codacy.yml`: `engines.opengrep.exclude_paths: [test/**, scripts/e2e/**]` | tests and the operator-run harness | Codacy 200 (176 fs-path, 9 RegExp, 2 SSRF, 13 timing) | The rules' premise is "untrusted input reaches this call". Tests and the harness run under the operator's own account with operator-owned paths and are not in the npm package. `src/` and the other `scripts/*.mjs` stay covered. |
| `.markdownlint.json`: `default: false`, the 33 rules the coding standard may run switched on by name, and `MD024` with `siblings_only` | markdown | Codacy 8 (the 9th is in `legacy/`) | The repeated headings are per-section structure (`### Metrics` under each day, `### 根拠` under each contract). `siblings_only` keeps the rule for real duplicates. Codacy applies a repository `.markdownlint.json` as soon as it exists (PR #159's first analysis, 2026-09-07, with `{"MD024": …}` alone: 317 new findings, 275 of them MD013, because markdownlint enables every rule a configuration file does not mention). The rule list was derived, not guessed: `codacy/codacy-markdownlint` run locally on the same tree with all 43 of its default-enabled rules fired 10 rules (MD034, MD033, MD038, MD037, MD012, MD056, MD032, MD022, MD025, MD028) on files Codacy analyses, none of which Codacy ever reported on `main`, so the standard demonstrably does not run them; the 33 that fired nothing locally are kept on so no rule the standard may run is switched off. With this file the local run reports 0 outside `legacy/`. |
| Codacy pattern `Stylelint_scss_function-disallowed-list` disabled for the repository (`PATCH …/tools/{stylelint uuid}` with a one-element `patterns` array, R3; `…/patterns` is only read back afterwards; needs the token) | `.css` | Codacy 1 | Only the one SCSS-specific rule is switched off; every other Stylelint CSS rule keeps running on `src/viewer/app/app.css`. No repository `.stylelintrc` is added, so the tool's CSS rule set is not replaced. |

Codacy counts after this batch: 397 − 287 = 110 (the 287: 4 in the excluded trees, 30 SQL, 44 Lizard on tests, 200 Opengrep on tests and harness, 8 MD024, 1 Stylelint), all in `src/` and non-harness `scripts/`, all handled by code or by per-issue verdict. The `.markdownlint.json` row needs no account action: Codacy read the file on the pull request's first analysis without the "Use configuration file" switch (R3's reading of the docs was wrong for markdownlint; the switch stays relevant only for tools whose file Codacy does not pick up on its own).

**All 31 file-length findings** of the 2026-09-07 inventory (FR-016; NLOC from the Codacy inventory; decisions confirmed by reading at the post-C state). The 2026-09-08 refresh added four more — `scripts/e2e/probes/pi.mjs`, `src/observer/classify.ts`, `src/observer/llm.ts`, `src/setup/setup.ts`, the last three grown past 500 NLOC by C1 and C2 — and the 2026-09-09 read of the live search added a fifth, `scripts/e2e/probes/claude.mjs`, grown from 478 to 539 by C4. T036 read all of them and the R7 table carries the decision for each:

| File | NLOC | Decision | Reason |
|---|---|---|---|
| `package-lock.json` | 2556 | excluded (`.codacy.yml exclude_paths`) | generated lockfile |
| `test/unit/agents.test.ts` | 1246 | excluded (`engines.lizard.exclude_paths: test/**`) | unit test file |
| `test/unit/capture.test.ts` | 1214 | excluded | unit test file |
| `test/unit/privacy.test.ts` | 1000 | excluded | unit test file |
| `test/unit/doctor.test.ts` | 950 | excluded | unit test file |
| `test/unit/classify.test.ts` | 937 | excluded | unit test file |
| `test/unit/pack.test.ts` | 847 | excluded | unit test file |
| `test/unit/inject.test.ts` | 781 | excluded | unit test file |
| `test/fault-provider.test.ts` | 781 | excluded | fault test file |
| `test/fault-worker.test.ts` | 753 | excluded | fault test file |
| `test/unit/deferred.test.ts` | 695 | excluded | unit test file |
| `test/unit/batches.test.ts` | 573 | excluded | unit test file |
| `test/unit/observe.test.ts` | 540 | excluded | unit test file |
| `test/unit/degraded.test.ts` | 525 | excluded | unit test file |
| `test/unit/retrieval.test.ts` | 524 | excluded | unit test file |
| `test/unit/llm.test.ts` | 523 | excluded | unit test file |
| `test/unit/queries.test.ts` | 515 | excluded | unit test file |
| `test/unit/cli-memories.test.ts` | 502 | excluded | unit test file |
| `scripts/e2e/isolated-user.test.mjs` | 1556 | resolved — won't fix | harness test file: one scenario list; stays measured by lizard (not under `test/`) and its 6 long functions are fixed in C4 |
| the 13 source and harness files, plus the 4 added on 2026-09-08 and 1 on 2026-09-09 | — | see R7 table | 6 split (`fixed`), 12 `resolved` |

## R5. Mechanical rewrites: batching and safety

**Decision**: Delegate to Codex in rule-family batches, one worktree per batch, with the rewrite rules stated and a "revert and record" instruction for any rewrite that changes a runtime value.

**Rationale**: The rules are local transformations (optional chaining, `String.raw`, `replaceAll`, `.at()`, `structuredClone`, `Object.hasOwn`, combining consecutive `push` calls, nested ternary to `if`). Each has one known semantic trap, listed for the implementer:

- `String#replace` → `replaceAll` (S7781): only when the pattern is a **global** regex (`/g`), where the two calls are identical. With a string pattern `replace` changes the first occurrence and `replaceAll` every occurrence (`'a-a'.replace('a','x')` is `'x-a'`, `replaceAll` gives `'x-x'`), so a string-pattern site is rewritten only if a test or the surrounding code proves the input holds at most one occurrence; otherwise the finding is `resolved` with that reason.
- `String.raw` (S7780): only when the literal contains no escape the author meant to interpret (`\n`, `\t`); a `\\` that becomes `\` is the intended change.
- `.at(-1)` (S7755): identical for negative indexes on arrays and strings; not for objects.
- Optional chaining (S6582): `a && a.b` → `a?.b` is equivalent only when **all three** hold, and the implementer writes which fact proves each. (1) **Value set**: `a` can never be a falsy non-nullish value (`0`, `''`, `false`, `NaN`, `0n`); `Boolean(0 && (0).toFixed)` is `false` but `Boolean(0?.toFixed)` is `true`. In TypeScript a static type of `object | null | undefined` (or a narrower object type) proves it; in `.mjs` only a preceding `typeof x === 'object'` / `instanceof` check or a construction site that shows the value is an object or nullish. `Map#get`, `Array#find`, `RegExp#exec`, and `String#match` prove nothing by themselves: a map or array can hold `0` or `false`, so the element type or the code that fills the container must be read. (2) **Consumption**: when `a` is `null`, `a && a.b` yields `null` and `a?.b` yields `undefined`, so the rewrite is allowed only where the two are indistinguishable (`if`, `!`, `Boolean()`, `??`, `== null`, `!= null`, or a left operand whose type excludes `null`); a result that is returned, stored, compared with `===`, or serialised keeps the original form. (3) **Evaluation**: `a` is evaluated once in both forms only when it is an identifier or a property read without a getter; a call or a getter is evaluated twice in `a && a.b` and once in `a?.b`, so such a site stays. Everything else stays as written and is recorded `resolved` with the counter-example.
- Nested ternary (S3358): rewrite to `if`/`else` or a lookup; keep evaluation order.
- Spread fallback (S7744): `{...(x ?? {})}` → `{...x}` is safe (spreading `undefined`/`null` is a no-op).
- `structuredClone` (S7784): drops functions and class instances; only when the source is plain JSON.
- Any rewrite whose equivalence cannot be shown from the code and its tests is skipped and recorded, never applied on the assumption that the rule's author meant it to be safe.
- S8786 (super-linear regex) is **not** mechanical: each pattern needs (1) the existing accept / reject / secret-detection cases kept green, (2) a new case with a long non-matching input (100 kB of the character that feeds the backtracking) bounded in wall time, and it belongs with the security-flavoured batch. A pattern may be left as written and `resolved` **only** when its input is bounded by existing code (a path limited by the OS, a constant, a line of a file the operator wrote) so the quadratic cost cannot exceed the budget; a pattern that a repository, agent payload, model output, or network response can feed is a real finding and is fixed with its reproduction test, or the feature stays open (FR-006). Measured on 2026-09-07: `stripPrivate`'s `/<\s*(\/?)\s*private\s*>/` takes 1.4 s on `<` followed by 50,000 spaces and the captured text cap is 1 MiB, so it is real; the linear form `/<\s*(?:(\/)\s*)?private\s*>/` accepts the same language (10 cases compared) and takes 5 ms on 1 MiB.

**Alternatives considered**: ESLint autofix with `eslint-plugin-unicorn` / `sonarjs` rules (would fix a subset in one run, but adds a dependency and its own rule set to the repository; rejected for a one-time cleanup).

## R6. Security-flavoured findings: the reading procedure

**Decision**: For each finding, write a one-line verdict of the form `<file>:<line> — <value origin> — real | not applicable — <action>` into the disposition record before any code changes; fix the real ones in this session with a failing-then-passing test; ignore the rest on Codacy with the same sentence.

**Value origins recognised**: operator-owned path from `OBOETE_HOME` or `~`; configuration the operator wrote under their home; **repository-supplied configuration** (a committed `.oboete.toml`, which `src/config.ts` treats as untrusted and restricts; a clone of a foreign repository can carry one); constant in the bundle; local worker-lease token stored in the operator's own database; validated repository path from `repo-identity`; string supplied by an agent hook payload or by the model. A finding is real when a value controlled by someone other than the operator (repository, agent payload, model output, network response) reaches the flagged sink without a check that removes the attack, and the effect is one the constitution forbids (secret egress, path escape, prototype change, unbounded CPU). Each verdict therefore records four things: **controller**, **validation on the way**, **sink**, **effect if unchecked**.

**Specific checks** (from reading the flagged lines on 2026-09-07; verdicts are produced in implementation):

- `src/worker/lease.ts:84` `row?.owner_token !== token` and `src/worker/observe.ts:220/804`: the lease token is a fencing token between two processes of the same user; any process that can read the database can read the token, so a timing channel gives an attacker nothing they do not already have. Candidate: not applicable. A constant-time compare is still cheap; the implementer may use `timingSafeEqual` if the lengths are fixed, but the verdict does not depend on it.
- `src/setup/detect.ts:187` `trusted_hash === hash`: comparison of Codex trust hashes that are written to the operator's config file; not a secret. Candidate: not applicable.
- `src/config.ts:321` `token !== ''`, `src/injection/recognize.ts:71` `hash !== null`: presence checks. Candidate: not applicable.
- `src/agents/index.ts:81,322` (RegExp built from `PATH_KEYS` constants and a `key` from the same constant list) and `src/privacy/detect.ts:140` (`new RegExp(\`^[${body}]$\`)`): trace where `body` comes from (a rule pattern the operator or the repository configuration wrote?) and what the character class is used for. A blanket escape would change glob semantics such as `[a-z]`; the fix, if one is needed, is validation of the class syntax or bounding of its length, verified by tests that keep the existing accept, reject, and secret-detection cases.
- `src/viewer/app/api.ts:78` `fetch(path)`: browser code fetching its own origin; not applicable unless `path` can carry a full URL from user input.
- `src/setup/managed-block.ts:168,201` (`value = value[key]`): keys come from the TOML table path constants; check that no user-controlled key reaches them.
- `src/agents/pi.ts:143` `PI_TOOLS[native]` and `src/cli.ts:94` `commands[name]`: table lookups; check `Object.hasOwn` guards so `__proto__`/`constructor` cannot select a prototype member.
- `src/injection/pack.ts:342`, `src/db/queries.ts:254`: `IN (?, ?, ...)` placeholder lists built from array length, values bound as parameters. Candidate: not applicable (parameterised).
- `scripts/e2e/dogfood.sh:27,32` `sudo cat file > dest`: the redirection runs unprivileged, which is what the script intends (the destination is the maintainer's file). Candidate: not applicable; add a `# shellcheck disable=SC2024` with the reason, or keep and ignore on Codacy.
- `.github/workflows/ci.yml:58`: the pattern matched the pinned commit SHA of `sonarqube-scan-action`; not a key. Not applicable.

## R7. Complexity and length refactors: safety rails

**Decision**: Refactor by extraction (pull a block into a named function with the same inputs and outputs), never by reordering; keep every public export; run the module's tests unmodified; compare the resource fixture numbers for hook-path modules; run one isolated dogfood for harness batches.

**Rationale**: Sonar S3776 and lizard NLOC both fall when a function is split; extraction is the transformation that cannot change control flow if the extracted block has a single exit.

**Resource check for hook-path batches** (C1, C2, C3, D): the T067 numbers come from `node scripts/measure-cold-start.mjs` (prints a Markdown table of version and capture timings) and the T068 numbers from `node dist/oboete.mjs fixture replay test/fixtures/events-1000.jsonl --json` (the committed 1,000-event fixture: ready/pending injection, worker RSS, database growth, with its own pass/fail against the 300 / 1,300 ms budgets; without the fixture path the command prints usage and exits 2). Both are run twice on the same quiet machine within the same hour: once on the base `main` SHA the batch branched from and once on the candidate SHA, each built with `npm ci && npm run build` in its own checkout, each command's exit code checked before the next runs. Acceptance: both replays exit 0 and the candidate's own pass/fail is green; every median is within 15 % of the base run; the maximum sample of each series does not exceed the base maximum by more than 15 % and never crosses the budget. The two Markdown tables and the two JSON results go into the PR body. A miss blocks the merge until explained.

**Harness verification for C4 and D** (FR-017): the harness executes the `oboete` found on `PATH`, and the agents' hook and MCP entries carry the **absolute path of the bundle that `oboete setup` wrote**, so installing a candidate into another prefix and prepending it to `PATH` proves nothing on its own (the hooks would still run the daily install's bundle), and `oboete --version` prints the same package version for every candidate. The candidate run therefore: (1) packs the candidate SHA (`npm run build && npm pack`, memory `npm-pack-ships-stale-dist`) and records `sha256sum` of the tarball and of both `dist/oboete.mjs` and `dist/engine.mjs` inside it (since issue #210 `dist/` is two files, and `oboete.mjs` is a four-kilobyte launcher every build emits identically from `src/launcher.mjs`, so hashing it alone would match across builds whose engines differ entirely); (2) installs it into `~oboete-dogfood/candidate` (a prefix the daily cron never uses); (3) runs `oboete setup --agents claude,codex,grok,pi --provider workers-ai --yes` from that prefix **with the real account `HOME` kept** (`sudo -u oboete-dogfood -H`; `isolated-user.mjs` refuses to run when `HOME` is not the account's home, and the agents' logins and oboete's consent record live in the real configuration directories) and the five configuration variables the harness and `oboete setup` both honour (`src/setup/detect.ts:216`, `isolated-user.mjs:758`) pointed at candidate-only copies: `OBOETE_HOME`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `GROK_HOME`, `PI_CODING_AGENT_DIR`, each a `cp -a` of the daily directory (`~/.oboete`, `~/.claude`, `~/.codex`, `~/.grok`, `~/.pi/agent`) taken before the run, so the copies carry the logins and the consent tuple that `--yes` needs and the hook and MCP entries `oboete setup` writes into them reference `~oboete-dogfood/candidate/.../dist/oboete.mjs`; (4) checks, by reading each agent's configuration in the copies, that every `oboete` hook command and MCP entry points at the candidate bundle, that none references the daily install, and that both bundle files' `sha256sum` equal the ones recorded in step 1; (5) runs `isolated-user.mjs --daily --pairs all` with the same five variables and `PATH`, and `--lifecycle --agents claude,codex` when lifecycle or TUI code changed; (6) records the candidate SHA, all three hashes, the run id, the `12 of 12` line, and the doctor table in the PR body. The daily cron's install, home, and agent configurations are not modified: the copies are deleted after the run and the real directories are never written under a candidate variable. Trust: the candidate is this project's own branch, reviewed (codex-review, ponytail-review, bot triage) before the run, exactly like the `main` the daily cron executes after the merge; the `oboete-dogfood` account holds only the dogfood provider keys (`OBOETE_<PRESET>_API_KEY`), its own agent logins, and dogfood data, none of which is shared with the developer account, so a candidate run exposes nothing the daily run does not already expose. The repository accepts no external contributions.

**Source and harness file-length decisions** (FR-016). The R4 table records the eighteen excluded files; the table below records the other eighteen frozen findings. The left-hand NLOC is the post-C measurement used by T036, and the right-hand value is the integrated continuation measured with Lizard 1.24.0 on 2026-09-09.

FR-016 is unchanged: independently testable concerns move when they can do so without a runtime cycle. Whether the finding closes is measured afterwards. The continuation removes the earlier requirement that one extraction must bring a file under 500; it also removes file size and an earlier timing outlier as reasons to retain an independent concern. The engine is one esbuild ESM bundle, so these source modules do not add runtime filesystem module loads. Internal callers follow moved definitions, with type-only reverse references where necessary.

All eighteen files now have their identified independent concerns separated. Thirteen original findings are planned `fixed #185`; five residual files remain `resolved` / `AcceptedUse` because the code left behind is one cohesive driver. Their reasons describe that remaining code, not the cost of further splits. The harness helpers remain in `probe-lib/`: every module discovered under `probes/` must still export `probes[]`.

| File | NLOC before → after | Decision | Extracted concern / residual reason |
|---|---|---|---|
| `src/worker/batches.ts` | 570 → 411 | split → `fixed` | Spool recovery moved to `worker/spool-recovery.ts`; classification and batch creation stay. |
| `src/setup/setup.ts` | 508 → 472 | split → `fixed` | Shared setup/doctor display helpers moved to `setup/report.ts`. |
| `src/observer/classify.ts` | 648 → 374 | split → `fixed` | Observation apply moved to `observer/apply.ts`; the batch worker and direct tests import it there. Shared SQL constants and directive checks stay in `classify.ts`, without a runtime reverse import. |
| `src/observer/llm.ts` | 595 → 483 | split → `fixed` | API error classification moved to `observer/llm-errors.ts`; prompt builders and provider calls stay. |
| `scripts/e2e/probes/pi.mjs` | 566 → 467 | split → `fixed` | Error-log evidence moved to `probe-lib/pi-errors.mjs`; imports now follow the shared process/frame modules. |
| `scripts/e2e/probes/claude.mjs` | 539 → 454 | split → `fixed` | TUI compaction driver moved to `probe-lib/claude-tui.mjs`; imports now follow the shared process/frame modules. |
| `src/capture.ts` | 1182 → 999 | split → `resolved` | Process/CLI adapter moved to `capture-command.ts` and the pure compaction epoch state machine to `capture-compaction.ts`. The residual is the single capture transaction: one adapted/redacted draft, absolute deadline, turn/epoch decision and store-or-spool outcome; transaction tests enter through `captureEvent`; the three direct epoch-state tests follow their module. 99.8% over. |
| `src/worker/observe.ts` | 1105 → 568 | split → `resolved` | Provider/fallback application, citation maintenance and imported-memory maintenance moved to `worker/observe-batch.ts`, `worker/citations.ts`, and `worker/imported.ts`. The residual owns one bounded leased run: queue passes, heartbeat, retry, summary completion and release. 13.6% over. |
| `src/fixture/replay.ts` | 2106 → 1143 | split → `resolved` | Completed-run evaluation moved to `fixture/replay-evaluate.ts`, serialization to `fixture/replay-report.ts`. The residual is one stateful execution driver: fixture parsing, hook/worker startup, lease/pending windows, sample accumulation and cleanup share `ReplayRun`. 128.6% over. |
| `scripts/e2e/isolated-user.mjs` | 1952 → 423 | split → `fixed` | Agent preparation, lifecycle state evaluation, lifecycle execution and lifecycle reporting moved to `probe-lib/isolated-agent.mjs`, `isolated-lifecycle-state.mjs`, `isolated-lifecycle.mjs`, and `isolated-lifecycle-report.mjs`. The entrypoint retains pair orchestration. |
| `scripts/e2e/isolated-user.test.mjs` | 1636 → 214 | split → `fixed` | Agent, lifecycle-state, lifecycle-execution and lifecycle-report tests now follow their subjects; shared test setup moved once to `isolated-user.test-support.mjs`. Existing assertions remain. |
| `scripts/fixtures/generate-1000-events.mjs` | 1817 → 1344 | split → `resolved` | Coverage validation and shared fixed corpus inputs moved to `fixtures/fixture-coverage.mjs`. The residual is the ordered emitter: its seeded clock/random state, session sequence and payload builders generate one deterministic stream. 168.8% over. |
| `scripts/e2e/probe-lib/agents.mjs` | 899 → 334 | split → `fixed` | Process execution/environment handling moved to `process.mjs`; frame/evidence decoding moved to `agent-events.mjs`. The original owns agent home setup, launchers and their shared completion prompt. |
| `scripts/e2e/mcp-clients.mjs` | 840 → 546 | split → `resolved` | Wire assertions moved to `probe-lib/mcp-assertions.mjs`, reporting to `probe-lib/mcp-report.mjs`. The residual is the registration/run/cleanup transaction for the client probe, with restoration coupled to the commands it starts. 9.2% over. |
| `scripts/e2e/probes/grok.mjs` | 800 → 285 | split → `fixed` | Lifecycle and MCP probe descriptors moved to `probe-lib/grok-lifecycle.mjs` and `grok-mcp.mjs`; IDs and descriptor order remain. |
| `scripts/e2e/probes/codex.mjs` | 746 → 183 | split → `fixed` | Lifecycle and MCP probe descriptors moved to `probe-lib/codex-lifecycle.mjs` and `codex-mcp.mjs`; IDs and descriptor order remain. |
| `src/injection/inject.ts` | 657 → 419 | split → `fixed` | Pi command parsing, storage opening and CLI execution moved to `injection/pi.ts`; the shared hook delivery/validation path stays. |
| `src/injection/pack.ts` | 602 → 470 | split → `fixed` | Pure framing and item rendering moved to `injection/pack-format.ts`; selection, privacy checks, budget and ledger remain together. |

Two new files remain over 500 and have also been read under FR-016. They are not excluded. PR #185 analysis of `ebe687dc` reported native IDs `f24df26860d01b16c092756092dba6ce` (source) and `48b2a00cb7eb3b5e6344415870d01661` (test); T043e adds both to the inventory and ledger before final confirmation.

| New file | NLOC | Decision / concern |
|---|---:|---|
| `scripts/e2e/probe-lib/isolated-lifecycle.mjs` | 724 | `AcceptedUse`: one lifecycle execution state machine; its parent-seed, before/after snapshots, event barriers and teardown order are coupled across resume/compact/fork/clear. Reporting, state evaluation and process preparation are already separate. 44.8% over. |
| `scripts/e2e/isolated-lifecycle.test.mjs` | 638 | `AcceptedUse`: one test suite and simulator for that lifecycle execution state machine; tests of independent state evaluation, agent preparation and reporting have moved out. 27.6% over. |

All other extracted source modules are below 500, including `fixture-coverage.mjs` at 491, `replay-evaluate.ts` at 489 and `replay-report.ts` at 490. The resource/dogfood requirements above still apply; structural completion does not waive a runtime miss.

## R8. Delegation and gating

**Decision**: Codex for mechanical and complexity batches outside the security-owned modules and outside the viewer front end; this session for `src/injection/*`, `src/mcp.ts`, `src/viewer/server.ts`, `src/viewer/app/*` (front end), `src/transfer.ts`, `src/db/queries.ts`, `src/privacy/*`, and every real security finding; one worktree per concurrent Codex job; every pull request through the PR #155 gate (CI, Sonar gate, `codex-review` `ok: true`, `ponytail-review`, bot triage, CodeRabbit once per push where the quota allows).

**Order**: the security batch E runs **before** every code batch, because its files (`scripts/e2e/isolated-user.mjs`, `src/setup/managed-block.ts`, `src/privacy/detect.ts`, `src/agents/index.ts`, …) overlap B and C, and a security fix must land in its own reviewed PR before mechanical or structural edits touch the same lines. B3 and E are both session-owned and run one after the other, never in parallel.

**Checkouts**: the shared checkout `~/projects/free-mem` stays on `main` and clean for the whole feature, because the hourly evidence cron commits there. The session works in its own linked worktree `~/projects/free-mem-wt/008` (memory `speckit-in-linked-worktree`: `.specify` symlink and `feature.json` pointing at the spec directory), and each Codex job in `~/projects/free-mem-wt/<batch>`. This is the "two or more concurrent editors" case in which CLAUDE.md allows worktrees. The shared checkout is never written to by the feature: every commit is made with `git -C <worktree>` in the worktree that holds the branch, a Codex batch is folded into the session's branch with `git -C ~/projects/free-mem-wt/008 merge --ff-only <codex-branch>` (each parallel writer has its own branch, `008-qd-<batch>-codex`, because one branch cannot be checked out in two worktrees), pushes go from that worktree, and `main` in the shared checkout only advances by `git pull --ff-only` after a merge. Codex cannot commit in a linked worktree, so this session commits.

**Rationale**: `rules/coding.md` routing (Grok paused until 2026-09-12); constitution: security-related changes are not delegated; memory `codex-cannot-commit-in-linked-worktree` (this session commits).

## R10. Codacy's ESLint step has failed on every analysed commit (found 2026-09-07)

**Fact**: `GET /api/v3/analysis/organizations/gh/ojungo69/repositories/oboete/commits/{sha}/logs` lists the analysis steps; on `main` (5e03d67f) and on every commit sampled back to 2026-08-16, the `ESLint` step is `error`: `codacy/codacy-eslint:9.18.10` (ESLint 8.57, legacy `.eslintrc*` only; the repository's flat `eslint.config.js` is not read, `hasConfigurationFile: false`) crashes inside `eslint-plugin-security-node` (`detect-unhandled-async-errors` on a `try … finally` without `catch`; with that rule removed, `detect-unhandled-event-errors` on an optional-chained call). The 397 Codacy findings therefore contain **no ESLint finding at all**, and the "0 new issues" gate has been judging commits without ESLint. Every other step is `success`.

**Measured** (the same image run locally on `main` with the tool's default pattern set minus the 17 `security-node_*` patterns, `docker run codacy/codacy-eslint:9.18.10` with a `/.codacyrc`): 2,493 findings, of which 1,102 are inside `.codacy.yml`'s scope (`src` 645, `scripts` 457, `test` 0): `security_detect-non-literal-fs-filename` 386, `security_detect-object-injection` 222, `@typescript-eslint_consistent-type-definitions` 149, `no-unused-vars` 100, `xss_no-mixed-html` 56, `@typescript-eslint_no-unnecessary-condition` 44, and a tail of 25 rules under 25 each. The repository's own ESLint 9 gate (`npm run lint`, CI) is green on the same tree.

**Decision** (maintainer, checkpoint C3, 2026-09-07): **(b)**, disable Codacy's ESLint tool for the repository. The repository's ESLint 9 flat configuration is the maintained lint standard and a CI gate; Codacy's ESLint 8 cannot read it, and a second `.eslintrc` would drift. The action (`PATCH …/tools/f8b29663-2cb2-498d-b923-a10c6a8c05cd` `{"enabled": false}`, needs the token) is recorded in the disposition record as a tool-health row with this reason and the measured 1,102 estimate, and the final confirmation requires the ESLint step to be **absent** from the analysis log rather than `success`. The two outcomes that were weighed:

- **(a) Disable the 17 `security-node_*` patterns** for the repository (`PATCH …/tools/f8b29663-2cb2-498d-b923-a10c6a8c05cd` with a `patterns` array; the plugin is unmaintained and crashes on current TypeScript), let ESLint run, freeze the surfaced findings as a third inventory, and triage them as a batch **G** with the same three states: rules that cannot fit this code base (`detect-object-injection` fires on every `obj[key]`; `detect-non-literal-fs-filename` fires on a CLI whose purpose is reading operator-configured paths) become repository pattern disables with the reason, and the rest are fixed. Adds roughly 1,100 findings and several pull requests.
- **(b) Disable the ESLint tool on Codacy** for the repository (`PATCH …/tools/{uuid}` `{"enabled": false}`), recorded with the reason that the repository's ESLint 9 flat configuration is the maintained lint standard and CI gate, that Codacy's ESLint 8 cannot read it, and that a second `.eslintrc` would drift. Codacy keeps its other 25 steps. Zero new findings; the gap becomes an explicit, recorded decision instead of a crash.

Either way needs the account token (C1). Until decided, the counts in R1, plan.md, and the record are the counts **without ESLint**, and the final 0 / 0 in batch F is not written while the ESLint step reports `error`.

## R9. Disposition record

**Decision**: `docs/evidence/quality-debt-2026-09.md`: one table per service with columns `id | rule/pattern | file:line | state | where` where `state` ∈ {fixed, resolved, excluded} and `where` is the pull request number, the service comment text, or the configuration line. Generated from the two inventory JSON files by a small script kept under `scripts/` only if it is reused; otherwise produced once and committed as data.

**Rationale**: FR-012 requires the record to be reachable from the repository; `docs/evidence/` is where M1 keeps its evidence.

**Planned versus confirmed**: a ledger row carries the end state the batch intends (`state`) and, separately, the evidence that the service agrees (`confirmed`: the SonarCloud analysis id / the Codacy commit SHA whose analysis no longer lists the id, or the HTTP response of the transition / ignore call). A row without `confirmed` is still `open` for `--check`; the final tables show `fixed ✓` / `fixed (planned)` accordingly, so a merge that has not been analysed yet or an API call that has not run cannot pass the final check. Batch F owns no ids: it is the step that runs the service calls and records confirmations for rows whose batch already merged.

**Uniqueness and checks**: the final tables hold exactly one row per `(service, id)`; state history, when a finding reopened, lives in a separate "History" section. The generator (`scripts/quality-debt-record.mjs --check`) exits non-zero when any inventory id is missing from the ledger, appears twice, is still `open`, or has a `resolved` state without a reason; the inventories live at `docs/evidence/quality-debt-2026-09/*.json` and the generator reads only that path. The 0 / 0 line is written only after both services show a **successful analysis of the final `main` SHA**: SonarCloud `api/project_analyses/search?project=ojungo69_free-mem&branch=main` lists an analysis whose `revision` equals that SHA, and Codacy's commit endpoint (`GET …/repositories/oboete/commits/{sha}`) reports the analysis complete with the tools that ran; the count queries are run only after both, and the polish edits of the feature go into the last pull request so that no repository change follows the confirmed SHA.
