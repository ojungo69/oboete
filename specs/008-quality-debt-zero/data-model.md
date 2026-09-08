# Data Model: Quality Debt to Zero

**Date**: 2026-09-07 | **Plan**: [plan.md](./plan.md)

The feature has no runtime data. Its only artefact is the disposition record, which maps every finding in the two inventories to an end state — the 2026-09-07 export and the rows added while the feature ran (T030a).

## Entities

### Finding

One row reported by a service on `main` — as of 5e03d67f for the rows exported 2026-09-07, and of the later commit an added row was read at (T030a).

| Field | Source | Notes |
|---|---|---|
| `service` | fixed | `sonar` or `codacy` |
| `id` | service | SonarCloud issue key (`AZ...`) or Codacy `issueId` (32 hex). Stable across analyses while the code line survives. |
| `rule` | service | Sonar rule key (`typescript:S3776`) or Codacy pattern id (`Lizard_nloc-medium`) |
| `severity` | service | Sonar `BLOCKER…INFO`; Codacy `Error/Warning/Info` |
| `file`, `line` | service | Repository-relative path; line as of the commit the row was exported from — `5e03d67f` for the 2026-09-07 rows, `9e52c3c2` for the 13 added on 2026-09-08 |
| `population` | derived | `inapplicable` / `mechanical` / `complexity` / `security` (research R1) |

Identity: `(service, id)`. The two inventories (`sonar-main-issues.json`, `codacy-main-issues.json`) hold every finding that must be dispositioned; 310 + 410 rows. They start as the 2026-09-07 export of `5e03d67f` and grow only when a service reports a finding no row covers — 13 such rows were added on 2026-09-08 from the live search at `9e52c3c2` (T030a). A row is never removed: a finding that stops being reported keeps its row and its disposition, because the acceptance is the service's own count, not the size of this file.

### Disposition

The end state of one Finding. Exactly one per Finding (FR-001).

| Field | Values | Rule |
|---|---|---|
| `state` | `fixed` / `resolved` / `excluded` | `fixed`: the code at that line changed in a merged PR and the service no longer reports the id. `resolved`: the id is closed on the service by a transition (Sonar `wontfix` / `falsepositive`) or an ignore (Codacy) that carries the reason. `excluded`: a configuration change stops the rule for the file class; the id closes at the next analysis. |
| `where` | PR number / comment text / config line | `fixed` → `#NNN`; `resolved` → the one-sentence reason as posted on the service; `excluded` → the file and line of the configuration change (for example `.codacy.yml engines.lizard.exclude_paths`) |
| `verdict` | free text, security population only | `controller — validation — sink — effect — real / not applicable` (research R6). Required for every `fixed` or `resolved` Finding whose rule is security-flavoured (Codacy `Semgrep_*` and `shellcheck_SC2024`, Sonar S8786), whatever the file; an `excluded` Finding is covered by the file-class review in research R4 instead. |
| `confirmed` | analysis id / commit SHA / HTTP response, or absent | The service's agreement with `state`: for `fixed` and `excluded`, the SonarCloud analysis id or the Codacy commit SHA whose analysis no longer lists the id; for `resolved`, the HTTP status and timestamp of the transition / ignore call. Absent until batch F records it; `--check` treats an unconfirmed row as `open`. |
| `reason` | Codacy only, one of `AcceptedUse`, `FalsePositive`, `NotExploitable`, `TestCode`, `ExternalCode` | The enumerated reason the Codacy API stores alongside the free-text `where` comment. |
| `transitioned` | Sonar `resolved` rows only, ISO time, or absent | Written by `--apply-sonar` after the transition succeeds and before the comment is posted, so a rerun after a failed comment skips the transition (SonarCloud refuses a second resolution of a RESOLVED issue) and posts only the comment; `confirmed` is written after the comment and `transitioned` is removed then, so a reopened issue is redone by clearing `confirmed` alone. |

State transitions: a Finding is `open` until its batch merges; the ledger row written at merge time is a **planned** end state; it becomes confirmed `fixed` when the next analysis no longer lists the id and the batch changed that line; confirmed `resolved` when the service action succeeds; confirmed `excluded` when the configuration PR merges **and** the following analysis no longer lists the id. Batch F records the confirmations; it owns no Finding. A Finding that reappears (for example because a rewrite was reverted) goes back to `open` and must be dispositioned again; the final tables always hold **exactly one row per `(service, id)`**, and the earlier state is moved to a separate `## History` section (`id | from | to | when | why`).

### Batch

One pull request. Keyed by the batch letter in the plan (A, B1, B2, B3, E, C1–C4, D, F).

| Field | Notes |
|---|---|
| `pr` | number after opening |
| `scope` | rule family or module area (FR-009) |
| `findings` | the ids it claims, generated into `docs/evidence/quality-debt-2026-09/allocation.json` from the scope rules in plan.md; a Finding belongs to exactly one batch and `--check` fails on an unassigned or doubly assigned id |
| `gate` | CI ✓, Sonar gate ✓, `codex-review` `ok: true` session id, `ponytail-review` run, bot threads resolved, dogfood run id (C4 only) |

### Exclusion

One configuration change (batch A). Carries `file class` (glob), `rule set` (engine, rule, or pattern), `count removed`, and `why it cannot hide a real finding in src/` (FR-004). The seven rows are in research R4.

## Disposition record format

File: `docs/evidence/quality-debt-2026-09.md`. One section per service, one table each, sorted by file then line. Columns:

```text
| id | rule | file:line | state | where / reason |
```

Security-population rows carry the verdict in the last column before the action, separated by ` — `. A summary line above each table states `open / fixed / resolved / excluded` counts and the analysis timestamp at which the service reported 0.

The record is generated by `scripts/quality-debt-record.mjs` (with `quality-debt-ledger.mjs` for the ledger and `quality-debt-services.mjs` for the service calls, each under the 500-NLOC Codacy file limit) from the two inventory files at `docs/evidence/quality-debt-2026-09/{sonar,codacy}-main-issues.json` (the only copies the script reads) plus a ledger file `docs/evidence/quality-debt-2026-09/ledger.json` (`service, id, state, where, verdict?`) appended per batch. `--check` exits non-zero when an inventory id is missing from the ledger, appears twice, is `open`, or is `resolved` without a reason. The 0 / 0 summary line is written only after both services report a successful analysis of the final `main` SHA with the relevant tools run (SonarCloud `api/ce/activity`; Codacy commit status for that SHA) and the count queries return 0.

## Validation rules

- Every id in the two inventories appears exactly once with a non-`open`, **confirmed** state before batch F closes (SC-003); `--check` enforces missing, duplicate, `open`, unconfirmed, and reason-less rows as failures (`--check --planned` relaxes only the confirmation, for use between batches).
- `excluded` rows only reference file classes listed in research R4, and none of them is under `src/` except `src/db/migrations/**` for the SQL rule sets (FR-004).
- `resolved` rows have a reason of at most one sentence that names the code fact, not the rule name (FR-011).
- Security-population rows have a verdict naming a value origin (FR-005).
