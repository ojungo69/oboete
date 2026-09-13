# Data Model: Quality Debt to Zero

**Date**: 2026-09-07 | **Plan**: [plan.md](./plan.md)

The feature has no runtime data. Its only artefact is the disposition record, which maps every finding in the frozen 745-id inventory to an end state — the 2026-09-07 export and 38 additions (see [Inventory provenance](#inventory-provenance)).

## Entities

### Finding

One row reported by a service on `main` — as of 5e03d67f for the rows exported 2026-09-07, and of the commit each later addition was read at (see [Inventory provenance](#inventory-provenance)).

| Field | Source | Notes |
|---|---|---|
| `service` | fixed | `sonar` or `codacy` |
| `id` | service | SonarCloud issue key (`AZ...`) or Codacy `issueId` (non-empty lowercase hex preserved exactly, with no inferred width restriction). Preserved as the service's native value; Sonar can re-key IDs and Codacy content-hash IDs can change when a line moves, so identity across analyses is not guaranteed. |
| `rule` | service | Sonar rule key (`typescript:S3776`) or Codacy pattern id (`Lizard_nloc-medium`) |
| `severity` | service | Sonar `BLOCKER…INFO`; Codacy `Error/Warning/Info` |
| `file`, `line` | service | Repository-relative path; line as of the commit the row was exported from, named per addition in [Inventory provenance](#inventory-provenance) |
| `population` | derived | `inapplicable` / `mechanical` / `complexity` / `security` (research R1) |

Identity: `(service, id)`. The two inventories (`sonar-main-issues.json`, `codacy-main-issues.json`) hold the frozen **745 IDs (332 Sonar, 413 Codacy)** that must be dispositioned. They began with the 707-row export of `5e03d67f` on 2026-09-07; 38 rows were appended before batch F froze the scope; [Inventory provenance](#inventory-provenance) names the commit for each. A row is never removed: a finding that stops being reported keeps its row and its disposition. Acceptance follows [FR-002](./spec.md#functional-requirements). *(Amended 2026-09-14; the original text described "310 + 410 rows" that "grow only when a service reports a finding no row covers" and said "the acceptance is the service's own count, not the size of this file".)*

#### Inventory provenance

The counts are the diff of each commit against its parent.

| Commit | Date | Sonar | Codacy | Appended |
|---|---|---|---|---|
| `c0ed2500` | 2026-09-07 | 310 | 397 | The frozen export of `5e03d67f`: the 707 baseline. |
| `d8760a55` | 2026-09-08 | — | +13 | Thirteen Codacy findings the live issue search at `9e52c3c2` reported and no row covered, from the reader-boundary and file-growth causes the T030a note records. |
| `7c1622b6` | 2026-09-09 | — | +1 | `16267458dd41ebb61fa3aec7549291cc`: read at `67242108`, where C4's own extractions had grown `scripts/e2e/probes/claude.mjs` from 478 to 539 NLOC. Found in the review of the C4 ledger rows and allocated to batch D, which became 18 ids. No task owns this row. |
| `c8fb5d7c` | 2026-09-09 | +11 | +2 | PR #185's analysis of `ebe687dc`: eleven Sonar findings on moved code and two native Codacy lifecycle file-length findings. |
| `c9a9e585` | 2026-09-09 | +11 | — | Sonar's separately gated push-branch analysis of `ebe687dc` reported the same eleven source findings under distinct ids, matched by rule, file, line, source hash and message before recording. |

332 Sonar (310 plus 22 additions) + 413 Codacy (397 plus 16 additions) = the frozen 745. This table
is the only statement of the per-commit split: state an addition's provenance here and point at it.

### Disposition

The end state of one Finding. Exactly one per Finding (FR-001).

| Field | Values | Rule |
|---|---|---|
| `state` | `fixed` / `resolved` / `excluded` | `fixed`: the code at that line changed in a merged PR and the service no longer reports the id at the named confirming analysis. `resolved`: the id is closed on the service by a transition (Sonar `wontfix` / `falsepositive`) or an ignore (Codacy) that carries the reason. `excluded`: a configuration change stops the rule for the file class; the id closes at the next analysis, which is recorded as its confirmation. |
| `where` | PR number / comment text / config line | `fixed` → `#NNN`; `resolved` → the one-sentence reason as posted on the service; `excluded` → the file and line of the configuration change (for example `.codacy.yml engines.lizard.exclude_paths`) |
| `verdict` | free text, security population only | `controller — validation — sink — effect — real / not applicable` (research R6). Required for every `fixed` or `resolved` Finding whose rule is security-flavoured (Codacy `Semgrep_*` and `shellcheck_SC2024`, Sonar S8786/S4036/S8707), whatever the file; an `excluded` Finding is covered by the file-class review in research R4 instead. |
| `confirmed` | analysis id / commit SHA / HTTP response, or absent | The service's agreement with `state`: for `fixed` and `excluded`, the SonarCloud analysis id or the Codacy commit SHA whose analysis no longer lists the id; for `resolved`, the HTTP status and timestamp of the Sonar comment after resolution or the Codacy ignore `PATCH`. For later regrowth covered by the scope amendment, a `fixed` row may instead retain a before/after measurement tied to the earlier closing SHA and the follow-up owner (research R11; issue #207); it must not claim a service absence that was never observed. Codacy's issue search reports only issues of the current analyzed commit: an id rewritten by an earlier batch can be absent while its record still exists and accepts PATCH, so a Codacy `resolved` row always needs the PATCH receipt. Sonar completion and `--confirm` remove `transitioned`. Absent until the applicable analysis or service receipt is recorded; each batch records available confirmations, and batch F records the remaining/final confirmations. `--check` treats an unconfirmed row as `open`. The field itself accepts any non-blank string, so the CLI cannot tell a service receipt from a hand-recorded measurement; a distinct shape for measured closures is filed in issue #224. |
| `reason` | Codacy only, one of `AcceptedUse`, `FalsePositive`, `NotExploitable`, `TestCode`, `ExternalCode` | The enumerated reason the Codacy API stores alongside the free-text `where` comment. |
| `transitioned` | Sonar `resolved` rows only, ISO time, or absent | Crash-safety record written by `--apply-sonar` after the transition succeeds and before the comment is posted. No decision reads this marker: each run reads live status and resolution, and an `OPEN` or `REOPENED` issue still needs a transition even when the marker exists. A successful comment writes `confirmed` and removes `transitioned`, so a reopened completed issue is redone by clearing `confirmed` alone. |
| `history` | array of `{from, to, when, why}`, or absent | Every re-disposition MUST append an entry, including a planned `resolved` row changed to confirmed `fixed`: previous state, new state, re-disposition date, and the evidence and reason for the change. Keep exactly one current row per `(service, id)`; the generator renders these entries in `## History` as `service / id / from / to / when / why`. This is a recording requirement that `--check` does not yet enforce: it checks neither missing history nor malformed entries, and missing entry fields render as blank cells. The automated reader is filed in issue #223. |

`--apply-sonar` reads pending IDs in chunks of 100 without an open-only filter. The ledger transition
maps to the expected service resolution: `wontfix` (the default) → `WONTFIX`, `falsepositive` →
`FALSE-POSITIVE`. Each row follows this decision table:

| Live state | Decision |
|---|---|
| ID absent from the search | Refuse that row and ask for re-disposition. |
| `CLOSED`, any resolution (including `FIXED` and `REMOVED`) | Refuse that row and ask for re-disposition; name only a known resolution, otherwise point to the quickstart. |
| `RESOLVED`, resolution matches the ledger transition | Send `add_comment` only. |
| `RESOLVED`, any other resolution | Refuse that row and name the expected resolution; name the actual resolution only if known, otherwise point to the quickstart. |
| Anything else (`OPEN`, `CONFIRMED`, `REOPENED`) | Transition, then comment. |

Known resolutions are `FIXED`, `REMOVED`, `WONTFIX` and `FALSE-POSITIVE`; other response values are
never printed. Refused rows remain unchanged and make the run fail after the other rows are processed, with one
error naming their count and IDs. Successful calls persist immediately. Invalid search responses,
including a page whose reported total differs from the number returned, fail before any write.

`--apply-codacy` sends the ignore `PATCH` for every pending `resolved` row, including an id absent
from the current-commit search. Only the HTTP response confirms it; the search describes findings on
that commit, not whether an older issue record still exists. HTTP 404 refuses that row without
confirmation and continues with later rows; one final error names the refused count and IDs.
Every other request failure aborts immediately.

State transitions: a Finding is `open` until its batch merges; the ledger row written at merge time is a **planned** end state; it becomes confirmed `fixed` when the next analysis no longer lists the id and the batch changed that line; confirmed `resolved` when the service action succeeds; confirmed `excluded` when the configuration PR merges **and** the following analysis no longer lists the id. Each batch records its available confirmations; batch F records the remaining/final confirmations and owns no Finding. A Finding reopened within this feature's scope goes back to `open` and must be dispositioned again. Later regrowth assigned to the follow-up round by the scope amendment instead keeps its earlier named confirmation, with the before/after measurement and owner recorded (research R11; issue #207). The final tables always hold **exactly one row per `(service, id)`**; every re-disposition appends to that row's `history`, rendered in `## History` as `service / id / from / to / when / why`. *(Amended 2026-09-14; the original unconditional rule was "A Finding that reappears (for example because a rewrite was reverted) goes back to `open` and must be dispositioned again".)*

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

Security-population rows carry the verdict in the last column before the action, separated by ` — `. A summary line above each table states the frozen inventory's `open / fixed / resolved / excluded` counts. The closing evidence records the service analysis labels, timestamps and live counts, with ownership of findings outside the inventory.

The record is generated by `scripts/quality-debt-record.mjs` (with `quality-debt-ledger.mjs` for the ledger and `quality-debt-services.mjs` for the service calls, each under the 500-NLOC Codacy file limit) from the two inventory files at `docs/evidence/quality-debt-2026-09/{sonar,codacy}-main-issues.json` (the only copies the script reads) plus a ledger file `docs/evidence/quality-debt-2026-09/ledger.json` (`service, id, state, where, verdict?`) appended per batch. `--check` prints a count for every failure class it knows — `missing`, `duplicate`, `open`, `unconfirmed`, `resolved-without-reason`, `unknown`, `without-where`, `without-verdict` — and exits non-zero when any of them is not zero; `--check --planned` is the exception, tolerating an unconfirmed row because a planned disposition has no receipt yet. `checkAllocation` reports its own problems separately. Closing follows FR-002; final-analysis checks use SonarCloud `api/ce/activity` and Codacy commit status for that SHA with the required tools.

`--check-live` validates ledger IDs against the inventory and rejects duplicates before reading
both public current-issue searches. It reports `uncovered` IDs
outside the inventory, `contradicted` findings matching either a confirmed `fixed`/`excluded`
`(service, id)`, a confirmed `resolved` **Sonar** `(service, id)`, or an inventory
`(service, rule, file)` triple whose rows are all confirmed `fixed` or `excluded`, and `invalid`
findings whose comparison fields cannot be read. A confirmed ID is
contradicted even if its code moved; a planned disposition claims nothing, and a triple with an
`open`, `resolved`, missing or unconfirmed disposition claims nothing. The `resolved` case is Sonar
only and ID-level only: `openSonarIssues` passes `resolved=false`, so the ID returning means the
resolution was reopened. All 29 `resolved` Codacy rows have HTTP 204 receipts, 27 of them from this
batch's calls, but Codacy's current-commit search takes no ignore filter and exposes no ignore field:
presence does not distinguish an ignored issue, and absence does not prove resolution because four
omitted ids still had PATCHable records. When an absence was observed at a named analysis, a `fixed`
or `excluded` row claims that the analysis of that commit did not report the finding. Codacy's search
is scoped to the current analysed commit, so silence must be observed at that analysis, not assumed.
`verifyCodacyCommit` proves the analysis ran and ended before `--confirm` stamps a Codacy row. A
hand-recorded R11 measurement does not pass through that verification: `isConfirmed` accepts any
non-blank string, and `confirmLedger` filters already-confirmed rows out before verification.
The four confirmed `fixed` Codacy rows in `contradicted` therefore split into two cases:
`188840d78c29d17901a37c07a2879cbe` (#165) and `77635429d1f6f5ac9a4b30db32223ad9` (#166) retain their
observed-absence receipt at `67242108a7bfcfeaecda9a92f8ad3c8b0f3cd43f`; later regrowth does not erase
what that analysis observed. For `6d0b78f8fc02642841c96cb14a549f20` and
`7d9b63ea2aafe102e6476a3d93c9c9db` (both #185), absence was never observed: the current-commit search
already reported regrowth. These two rows use the permitted R11 before/after measurement instead
(`batches.ts` 570 → 411 NLOC, regrown to 551; `pack.ts` 602 → 470 NLOC, regrown to 558), with the live
findings handed to #207. This permission does not replace the first pair's observed-absence receipts.
A `resolved` row claims a service-side ignore, and the search
exposes no ignore field, so its silence measures the wrong thing whichever commit it was taken at,
and only the `PATCH` response settles it. A `resolved` Codacy row therefore stays out of
`contradicted`, and no `resolved` row joins the triple map — it says nothing about the rest of that
`(service, rule, file)`, so a sibling finding there is a new finding. A triple match for confirmed
`fixed`/`excluded` rows cannot distinguish regrowth from a new instance of that rule in that file;
either means their claim no longer holds. An exact-ID return identifies the specific finding.
Claims use the frozen inventory's rule and file, never ledger copies. Comparison validates Sonar
`rule` and `component` (including a non-empty path after `ojungo69_free-mem:`), and Codacy
`patternInfo.id` and `filePath` where it reads them. ID validation remains shared by all searches;
`--confirm` and apply modes do not require comparison fields they never read. An invalid comparison
field records `<id>: <sanitized reason>` without suppressing that ID's `uncovered` or ID-based
`contradicted` verdict; other findings are still classified. Any group
makes the run exit 1; empty groups are omitted, and no findings means exit 0 without output. Failed
requests or incomplete pages also exit 1. The check reads no credentials and writes no files.

## Validation rules

- Every id in the two inventories appears exactly once with a non-`open`, **confirmed** state before batch F closes (SC-003); `--check` enforces missing, duplicate, `open`, unconfirmed, and reason-less rows as failures (`--check --planned` relaxes only the confirmation, for use between batches).
- `excluded` rows only reference file classes listed in research R4, and none of them is under `src/` except `src/db/migrations/**` for the SQL rule sets (FR-004).
- `resolved` rows have a reason of at most one sentence that names the code fact, not the rule name (FR-011).
- Security-population rows have a verdict naming a value origin (FR-005).
