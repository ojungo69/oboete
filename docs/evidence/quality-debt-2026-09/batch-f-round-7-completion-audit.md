# Batch F round 7: completion semantics audit

**Date**: 2026-09-14. **Review baseline**: `008-qd-f` at `9e6ce79b9463cec77dd57b82eeacd2b6c949034d`.

This audit reads the documents as they stood at that baseline and its verdicts describe the round-7
edits; later commits amended them further. [FR-002](../../../specs/008-quality-debt-zero/spec.md#functional-requirements) carries the
completion criterion as it now stands.

All 1,247 lines of `spec.md`, `plan.md`, `research.md`, `quickstart.md`, `tasks.md`, and
`data-model.md` under `specs/008-quality-debt-zero/` were read section by section, with an independent
read-only audit alongside the edits. The tables document the listed completion statements, including
statements retained as correct; they are not an exhaustive inventory of every field definition. Line numbers refer to that immutable baseline; headings and task IDs identify the same
statements after editing. Outcomes below describe the round-7 changes and retained requirements.

## Governing meaning

At that baseline, feature completion means all **745 frozen inventory IDs** (332 SonarCloud, 413 Codacy) have exactly one non-open disposition confirmed by the applicable service evidence, checked after successful analyses of the final `main` SHA. The record also states both live counts and classifies/attributes remaining live findings; findings outside the frozen inventory and findings regrown by later work belong to issue #207. A nonzero live count is therefore compatible with completion. A frozen Codacy ID may remain visible because current-commit search does not expose ignore status, and a previously fixed ID may be reported again after later code growth; neither fact erases the earlier named confirmation.

`0 missing/open/unconfirmed` for the **745-row ledger** is valid. `0` for a service-wide live count is valid only as an explicitly dated historical amendment quotation or as a narrowly scoped, dated batch measurement. Ordinary test/gate/structural completion remains a separate requirement and should not be rewritten into inventory language.

## Amended completion statements

| Doc | Heading / line | Statement meaning | Verdict |
|---|---|---|---|
| `spec.md` | Input, 9 | Original quoted goal says service-wide 0 / 0, but it is outside an explicit dated amendment parenthetical. | **amended** — Original request retained with an explicit dated cross-reference to the current completion criterion. |
| `spec.md` | US1 opening, 35 | A maintainer sees zero open findings. | **amended** — Frozen-745 confirmed dispositions and live counts/owners replace active service-wide zero; original opening remains in a dated note. |
| `spec.md` | US1 Independent Test, 39 | Samples only the 2026-09-07 707 rows and requires every remaining live finding to be outside that inventory. | **amended** — Samples the frozen 745 and checks named confirmations/live ownership, allowing confirmed ignores and regrowth; both previous clauses remain in dated notes. |
| `spec.md` | US1 Acceptance 1–2, 43–44 | Uses 310 / 397 as the completed inventory and says any still-live Sonar/Codacy finding is outside it. | **amended** — 332/413 retained IDs, confirmed dispositions and live ownership; baseline counts and former zero/outside-only clauses remain documented. |
| `spec.md` | Edge Cases, 122 | If permission is absent, a repository record and maintainer action list substitute for service marking. | **amended** — The maintainer list is explicitly intermediate; service action and confirmation remain necessary under FR-002 and C1. |
| `spec.md` | FR-001 / FR-002, 128–134 | FR-001 and the active FR-002 noun phrase freeze only the initial 310 + 397 rows; the old zero wording is already dated. | **amended** — Explicitly frozen 745; the Scope amendment records 707 initial + 38 added IDs. Original zero requirement remains in its dated quotation. |
| `spec.md` | SC-001 / SC-002 / SC-003, 162–169 | Success criteria cover only 310, 397, and the 2026-09-07 inventory. | **amended** — 332 Sonar, 413 Codacy and 100 percent of 745 retained IDs; historical baseline/zero clauses stay in amendment notes. |
| `plan.md` | Summary, 9 | Active opening still commands service-wide zero before a later amendment explains the new scope. | **amended** — Active opening uses frozen-745 confirmation and live attribution; original zero opening moved inside a dated amendment parenthetical. |
| `research.md` | R3 Consequence, 59 | Correctly requires the token and rejects a handoff list as final, but twice says completion requires service-wide zero. | **amended** — Both zero clauses replaced; token prerequisite and intermediate-artefact point retained; original clauses in a dated note. |
| `quickstart.md` | Prerequisites, 22 | Describes the current inventory files only as the 2026-09-07 310 / 397 exports. | **amended** — Initial 310/397 exports plus 38 additions, frozen at 332/413; old export-only description remains in a dated note. |
| `tasks.md` | Phase 5 Independent Test, 85 | Requires Sonar S3776 and Codacy function/parameter live counts to be zero. | **amended** — Frozen rule-family confirmations with later/regrown findings measured and attributed; original live-count-zero clauses in a dated note. |
| `tasks.md` | Phase 6 Independent Test, 111 | Requires Codacy file-length live count zero outside `test/` after F. | **amended** — Frozen file-length confirmations with later/regrown findings measured and attributed; original service-zero clause in a dated note. |
| `tasks.md` | T044 completion note, 155 | Says spec/plan/research were swept, but active zero/baseline-only clauses remain. | **amended** — Links this six-document inspection; prior implementation account and task checkbox retained. |
| `data-model.md` | Opening, 5 | Says the inventory is the original export plus “rows added … (T030a),” which omits later additions and never states the frozen 745 aggregate. | **amended** — Explicitly frozen 745 with additions from T030a, T036 and T043e. |
| `data-model.md` | Finding Identity, 22 | Gives 310 + 410 = 720 and only the 13 T030a additions; ends by saying acceptance is the service’s own count. | **amended** — Frozen 745 = 332/413 from 707 + 38; acceptance is confirmed coverage plus live counts/owners. Original count/growth/acceptance language remains in a dated note. |
| `data-model.md` | Disposition state, 30 | Defines `fixed` / `excluded` as permanent current absence, which conflicts with retaining an honest confirmation when later work regrows the finding. | **amended** — Absence refers to the named confirming analysis; later regrowth is handled separately in the state-transition/confirmation descriptions. |
| `data-model.md` | State transitions, 60 | Unconditionally sends every reappearing finding back to `open` for re-disposition. | **amended** — In-scope reopenings require re-disposition/history; later regrowth assigned to #207 retains its earlier confirmation and measurement. Old unconditional rule remains in a dated note. Each batch records available confirmations; F records remaining/final confirmations. |

Two further definitions were amended after the independent comparison:

| Document / baseline location | Statement | Outcome |
|---|---|---|
| `plan.md:75`, inventory comment | Inventories grow whenever a service reports an uncovered finding. | Now frozen 745 IDs, with the original growth clause in a dated note; later findings belong to issue #207. |
| `data-model.md:33`, confirmed field | Confirmation only arrives in F and every fixed confirmation is an observed service absence. | Each batch records available confirmations, F records the remaining/final ones. The R11 exception explicitly permits a measured earlier closure tied to the prior SHA and follow-up owner without claiming an unobserved service absence. |

## Statements already compliant or historical

The rows below cover the remaining completion, success, end-state, and final-count language. Grouped ranges contain statements with one semantic scope; exceptions are listed individually above. A retained requirement does not establish that its runtime or batch gate passed.

### `spec.md`

| Heading / line | Statement meaning | Verdict |
|---|---|---|
| Title, 1 | “Quality Debt to Zero” is the feature’s proper name, not a present service-count assertion. | **already compliant** |
| Starting Point, 13–22 | Dated baseline counts and rule breakdown at 5e03d67f. | **already compliant** — historical measurement, not completion. |
| US1 priority, 37 | Dispositions must explain the backlog without hiding real findings. | **already compliant** — transparency requirement; scope is the frozen inventory. |
| US1 Independent Test parenthetical, 39 | Original “open count 0 on each.” | **historical quotation** — explicitly dated 2026-09-13. |
| US1 Acceptance parentheticals, 43–44 | Original per-service zero requirements. | **historical quotation** — explicitly dated 2026-09-13. |
| US2, 55–61 | Security-verdict completeness and test/review outcome for real findings. | **already compliant** — a required security sub-goal, not service-wide completion. |
| US3, 67–79 | Tests, signatures, behavior, dogfood, and the allocated finding’s result after each refactor. | **already compliant** — per-refactor/runtime acceptance. “Finding is gone” is scoped to the confirming refactor analysis. |
| US4, 85–95 | Exclusion review completeness and count removed per exclusion. | **already compliant** — configuration sub-goal. |
| US5, 99–110 | Each PR passes its gate and `main` stays green; each batch names IDs closed. | **already compliant** — delivery/gate sub-goal. |
| Edge Cases, 116–121 | Reanalysis is awaited, value-changing rewrites revert, timings compare, and failures remain candidates until traced. | **already compliant** — prevents premature completion. |
| FR-002 parenthetical, 132–134 | Original service-wide 0 / 0 requirement. | **historical quotation** — explicitly dated 2026-09-13. |
| FR-003–FR-005, 135–137 | Gates stay intact, exclusions stay narrow, and security findings are individually read. | **already compliant** — independent safety obligations. |
| FR-006–FR-017, 138–149 | Failing/passing security tests, unchanged behavior, green main, record existence, three per-row end states, and harness dogfood. | **already compliant** — independent required sub-goals/end-state definitions. |
| Key Entities, 153–156 | Exactly one end state per inventory finding and separate batch/exclusion concepts. | **already compliant** once “inventory” is bound to the amended 745 definition. |
| SC-001 / SC-002 parentheticals, 164–168 | Original final-analysis 0 / 0 outcomes. | **historical quotation** — explicitly dated 2026-09-13. |
| SC-004–SC-008, 170–178 | Security verdicts, tests/coverage, gate integrity, dogfood, and PR scope/gate success. | **already compliant** — ordinary acceptance obligations. |
| Assumptions, 182–183, 197–200 | Permission handoff, analysis configuration, harness/delegation boundaries, dogfood continuity and unchanged M1 contracts. | **already compliant** — operational assumptions; the maintainer list remains intermediate under C1 and FR-002. |
| Scope amendment, 184–196 | Explains nonzero live counts, frozen-scope closure, outside ownership, and six findings regrown by #190. | **already compliant** — this explains the scope rationale for FR-002. |

### `plan.md`

| Heading / line | Statement meaning | Verdict |
|---|---|---|
| Summary amendment, 9 | Old 0 / 0 end state and its 2026-09-13 replacement. | **historical quotation** for the old clause; the replacement is **already compliant**. |
| Technical Context, 19, 25, 27 | Existing suites, unchanged budgets, behavior/gate constraints, and dogfood continuity. | **already compliant** — runtime/gate obligations. |
| Scale/Scope, 29–31 | Final 745 allocation and post-push inventory continuation. | **already compliant** — authoritative aggregate and provenance. |
| Constitution Check, 35–50 | Research/design gate passed and all listed principles comply. | **already compliant** — planning sub-goal. |
| Structure Decision, 96 | Source/test boundaries and added coverage tests. | **already compliant** — design sub-goal. |
| Batches/order, 100–118 | One allocation per ID, one gated PR per batch, green `main`, and F owns no IDs. “Findings closed” and F’s “inventory final count” are scoped batch-table shorthand, not a service-wide-zero acceptance rule. | **already compliant** — workflow/sub-goal claims. |
| Batch D continuation, 124–153 | Six completed extractions, remaining seam work, checks, and warning that diagnostics do not prove acceptance. | **already compliant** — structural/runtime scopes are explicit. |
| C1, 155 current clause | Feature remains incomplete until service actions run, final analyses complete, every frozen row is confirmed, and live counts/owners are recorded; handoff list is intermediate. | **already compliant** |
| C1 parenthetical, 155 | Original final-analysis 0 / 0 requirement. | **historical quotation** — explicitly dated 2026-09-13. |
| C3, 157 | Final count waits for an analysis log without the disabled ESLint step. | **already compliant** — required-tool health prerequisite. |
| Phase 0/1, 159–167 | Initial R1–R9 research complete, no clarification remained, and design artefacts exist. | **already compliant** — scoped planning milestones; R10/R11 are later implementation findings. |
| Post-design check, 169–175 | Principles still comply; no complexity violation to justify. | **already compliant** — design gate. |

### `research.md`

| Heading / line | Statement meaning | Verdict |
|---|---|---|
| Header/R1, 5, 13–24 | Dated 5e03d67f baseline counts; zero fs-path hits only in `src/`. | **already compliant** — historical/rule-scoped measurements. |
| R2, 28–32 | 15 PL/SQL findings close on the next analysis. | **already compliant** — configuration-batch outcome. |
| R3, 50–57 | Per-issue success evidence, post-analysis confirmation, and batch A’s expected drop. | **already compliant** — API/batch sub-goal. |
| R4, 65–100 | Per-exclusion counts, post-A 110 count, and file-specific fixed/resolved/excluded outcomes. | **already compliant** — dated batch calculations/decisions. |
| R5, 104–118 | Rewrites keep tests green; a real unresolved regex keeps the feature open. | **already compliant** — behavior/security obligation independent of FR-002 counts. |
| R6, 120–137 | Every security finding gets a verdict and real findings get passing tests. | **already compliant** — security sub-goal. |
| R7 acceptance, 141–147 | Base/candidate replay and dogfood success criteria. | **already compliant** — runtime acceptance remains required. |
| R7 structural completion, 149–183 | All 18 concerns separated, 13 fixed/5 resolved, two new residual decisions, extracted modules under 500; runtime miss is not waived. | **already compliant** — explicitly structural, not feature completion. |
| R8, 185–193 | Every PR passes the gate and concurrent work stays isolated. | **already compliant** — workflow sub-goal. |
| R11, 195–233 | Dated measurements show #190 regrowth; prior confirmations remain honest; live work belongs to the follow-up. | **already compliant** — directly supports the amended model. |
| R10 Fact/Measured, 235–240 | Initial inventory had no ESLint rows while the step failed; repository lint passed. | **already compliant** — dated baseline evidence, later refined at 261–266. |
| R10 alternatives, 241–245 | Tool disable yields zero *new ESLint findings*, not zero service findings. | **already compliant** — rule/tool-scoped result. |
| R10, 246 current clause | Final inventory dispositions/live counts wait for an analysis with no ESLint step. | **already compliant** |
| R10 parenthetical, 246 | Original final 0 / 0 wording. | **historical quotation** — explicitly dated 2026-09-13. |
| R10 Applied, 248–266 | Tool state is applied; next final confirmation still requires ESLint absent. | **already compliant** — applied sub-goal plus unresolved final proof. |
| R9, 269–277 current clauses | Unconfirmed rows remain open to `--check`; closing evidence waits for both final-SHA analyses and records frozen dispositions plus live counts/owners. | **already compliant** |
| R9 parenthetical, 277 | Original “0 / 0 line” wording. | **historical quotation** — explicitly dated 2026-09-13. |

### `quickstart.md`

| Heading / line | Statement meaning | Verdict |
|---|---|---|
| Prerequisites, 7–21 | Read-only endpoint checks succeeded at a dated SHA; authenticated modes still need tokens. | **already compliant** — operational evidence/prerequisite. |
| Per-batch expected results, 24–46 | Tests pass, assertions stay, replay/timing meets bounds. | **already compliant** — per-PR runtime gate. |
| Candidate runbook, 48–170 | Installation/reference/execution proofs, expected 12/12 and lifecycle/doctor outcomes, and explicit limits of those proofs. | **already compliant** — candidate-bundle sub-goal, not feature completion. |
| Service counts, 172–179 | Commands report live counts; every frozen ID is dispositioned/confirmed, while outside-inventory findings are attributed to #207. It does not assert that all live IDs are outside the inventory. | **already compliant** — nonzero live counts and confirmed own-ID visibility remain possible. |
| Disposition check, 181–188 | Planned and final checks; `745 ids: 0 ...` is the frozen-ledger result. | **already compliant** — these zeroes are not service live counts. |
| Live coverage, 190–214 | Empty `uncovered`/`contradicted`/`invalid` groups make this check pass, while known open service issues may still exist. | **already compliant** — explicitly separates coverage from live count. |
| Live coverage parenthetical, 215 | Original need for final 0 / 0 service counts. | **historical quotation** — explicitly dated 2026-09-13. |
| Apply modes, 217–259 | Successful transitions/PATCHes confirm rows; refused/incomplete responses fail. | **already compliant** — per-row confirmation semantics. |
| Record regression checks, 261–271 | Source tests and Lizard bounds must pass. | **already compliant** — CLI sub-goal. |
| Final heading parenthetical, 273–275 | Original heading “before writing 0 / 0.” | **historical quotation** — explicitly dated 2026-09-13. |
| Final analysis, 277–286 current clauses | Both services must complete the same final SHA with required tools, then frozen dispositions and live counts/owners are recorded. | **already compliant** |
| Final analysis parenthetical, 286 | Original “record 0 / 0” instruction. | **historical quotation** — explicitly dated 2026-09-13. |
| Gate definitions, 288–294 | Exact allowed gate-file diff and success condition. | **already compliant** — gate-integrity sub-goal. |

### `tasks.md`

| Heading / line | Statement meaning | Verdict |
|---|---|---|
| Header, 5–7 | Existing suites must pass; each gate task closes a batch. | **already compliant** — tests/batch delivery only. |
| Phase 1, 22–29 | Setup/record/token tasks are checked and their concrete outputs are recorded. | **already compliant** — setup sub-goals. |
| Batch A Goal/Test, 35–37 | Rules drop by a dated expected amount only after post-merge analysis. | **already compliant** — batch-specific count. |
| T009–T011, 42–44 | Config action done/planned rows generated; Stylelint remains unconfirmed until analysis. | **already compliant** — explicit distinction between applied/planned/confirmed. |
| T012, 45 | The checked historical batch task retains its batch-specific “done only” condition and explicitly records the incomplete 301/302 receipt. Overall feature confirmation is still deferred to F. | **already compliant** — do not change the checkbox or weaken the gate. |
| Batch E, 51–62 | Security verdict/fix completeness, passing tests, and post-E counts. | **already compliant** — security batch sub-goal and dated count. |
| B batches, 68–77 | Mechanical inventory IDs close or resolve, PRs pass gates, and dated post-B counts/confirmations are recorded. | **already compliant** — batch scope. |
| Phase 5 Goal, 83 | Named allocated rule-family populations are closed by the C extraction work, with tests/budgets kept separate. | **already compliant** — this is the phase goal; line 85 is the invalid current-live zero oracle. |
| T029–T031, 89–95 | C3/C4 implementation, test/gate, and named-analysis results. | **already compliant** — SHA-scoped completion evidence. |
| T032, 96–97 | Work/evidence was executed, but replay/budget conditions remain unmet and the box stays open. | **already compliant** — the prose prevents `[X]` from being read as runtime acceptance; preserve checkbox state. |
| T033–T035, 98–103 | Candidate run and C-batch merge/confirmation results at named runs/analyses. | **already compliant** — historical sub-goals/counts. |
| Batch D Goal, 109 | 13 original fixed / 5 cohesive residuals and new rows assessed. | **already compliant** — frozen D allocation goal; line 111’s service-zero oracle is the defect. |
| T036–T038, 113–118 | Six initial extractions and their structural/test measurements completed. | **already compliant** — scoped structural completion. |
| T039, 119–121 | Evidence run completed but recall/full dogfood acceptance remains open. | **already compliant** — explicit incomplete outcome; preserve checkbox state. |
| T037a–T038a, 122–131 | Later extraction, equivalence, test, lint, build, Lizard, and review results; runtime acceptance still remains T039. | **already compliant** — scoped implementation/verification. |
| T040–T041, 132–134 | D’s 20 dispositions, 745-ID planned coverage, merge confirmation, and dated post-D count; planned is explicitly not live/confirmed. | **already compliant** |
| Phase 7 heading, 138–140 | Current frozen-inventory check and original 0 / 0 heading. | Current heading **already compliant**; old heading **historical quotation** dated 2026-09-13. |
| Phase 7 Goal, 142 current clause | All rows applied/confirmed, record checked, all live findings measured/attributed at final SHA. | **already compliant** |
| Phase 7 Goal parenthetical, 142 | Original both-services-zero goal. | **historical quotation** — explicitly dated 2026-09-13. |
| Phase 7 Test, 144 current clause | 745 planned-state result with one circularly unconfirmed row, final-SHA analyses, live counts/owners, and sampling. | **already compliant** — ledger zeros are scoped. |
| Phase 7 Test parenthetical, 144 | Original count commands print service 0 / 0. | **historical quotation** — explicitly dated 2026-09-13. |
| T042–T043b, 146–154 | Service calls, tool disable, live-check prerequisites, 745-ID expansion, and current confirmation counts at named stages. | **already compliant** — concrete intermediate/final-preflight results, including nonzero live counts and unconfirmed rows. |
| T043b parenthetical, 154 | Original “0 unconfirmed” pre-F rule. | **historical quotation** — explicitly dated 2026-09-13. |
| T045, 156 current clause | Pending final merge, final-SHA analyses, final `--check`, and live-count ownership evidence. | **already compliant** |
| T045 amendment quotation, 156 | Original loop until both service counts are zero. | **historical quotation** — explicitly dated 2026-09-13. |
| T046, 162 | Verify report exists with 56 implementation tasks complete, but this task remains open until its link is posted. | **already compliant** — ordinary sub-goal, not feature close. |
| T047, 163 current clause | Pending memory/comment/cleanup end state uses confirmed frozen dispositions plus live counts/owners. | **already compliant** |
| T047 parenthetical, 163 | Original memory end state “both services 0.” | **historical quotation** — explicitly dated 2026-09-13. |
| Dependencies/strategy, 169–183 | F and outside-repo work remain; MVP/batch pause points are separate from final completion; final completion uses `--check` plus live attribution. | **already compliant** |

### `data-model.md`

| Heading / line | Statement meaning | Verdict |
|---|---|---|
| Disposition, 26 | Exactly one end state per frozen Finding. | **already compliant** once Finding is corrected to the 745 aggregate. |
| Confirmed field, 33 | Named evidence proves the planned state; unconfirmed rows are open to `--check`. | Core rule **retained**; confirmation timing and the R11 measurement exception were **amended**, as listed above. |
| Sonar/Codacy decisions, 37–58 | Success/refusal/error rules for service actions and receipts. | **already compliant** — operational row confirmation. |
| Batch, 62–71 | One PR, one allocation, and its gate evidence. | **already compliant** — batch sub-goal. |
| Exclusion, 73–75 | Batch A’s seven configuration scopes and count removed. | **already compliant** — exclusion sub-goal. |
| Record format, 79–85 | Frozen inventory counts are separate from closing live counts/ownership. | **already compliant** |
| Closing, 87 | Every inventory row confirmed after successful final-SHA analyses, plus live findings measured/attributed to issue #207. | **already compliant** |
| Live check, 89–111 | `uncovered`, `contradicted`, and `invalid` classify current findings; known current findings can coexist with confirmed rows; requests/incomplete pages fail. | **already compliant** |
| Validation, 115–118 | All inventory IDs have non-open confirmed states before F closes; exclusion/resolution/security rules remain mandatory. | **already compliant** — inventory-scoped zero, not service-wide zero. |

## Retained interpretations checked explicitly

| Document / baseline location | Interpretation checked | Final decision |
|---|---|---|
| `plan.md:104`, Findings closed header | Whether allocated batch IDs promise current service-wide absence. | Retained: finite allocation counts and named batch outcomes, not current live counts. |
| `plan.md:116`, F final count | Whether this means a service-wide zero. | Retained: explicitly the inventory count; Summary/C1 separately require live counts and owners. |
| `quickstart.md:179`, Service counts | Whether visible ignored/regrown IDs force zero. | Retained: all frozen IDs require confirmation, live counts may be nonzero; live-check/R11 explain the visible IDs. |
| `tasks.md:45`, T012 | All 302 A rows must be confirmed, while its historical receipt records 301/302. | Gate and checkbox retained. The receipt leaves Stylelint confirmation outstanding for F; delivery is not promoted to full confirmation. |
| `tasks.md:83`, Phase 5 Goal | Whether numbered batch findings require all future live findings to disappear. | Retained: finite batch population; the service-zero independent test at line 85 was the contradiction and is amended. |

## Additional evidence and history statements

| Location | Statement inspected or added | Outcome |
|---|---|---|
| `docs/evidence/quality-debt-2026-09.md`, Inventories bullet | Inventory grows whenever a service reports an uncovered finding. | **amended** — growth ends at the frozen 745; original wording remains in a dated note. |
| Same record, Counts on main bullet | The final service count is acceptance. | **amended** — confirmed inventory dispositions plus live counts/owners; original wording remains in a dated note. |
| Same record, Batch F five-row account and History | Five rows were re-dispositioned from planned resolved to confirmed fixed. | **restored** — each has one `resolved` → `fixed` entry dated 2026-09-13, naming open-search absence, unfiltered CLOSED/FIXED, the e27bb029 analysis and retained verdict. Existing row fields are unchanged. |
| Same record and quickstart live-check section | Triple matches report contradicted findings. | **clarified** — regrowth and a new same-rule instance in the same file are indistinguishable; both invalidate a confirmed fixed/excluded claim. Exact ID identifies the specific finding. |
| `data-model.md`, Finding ID and live-check descriptions | Cross-analysis ID stability and triple meaning. | **clarified** — re-keyed/content-hash IDs do not guarantee stable identity; fixed/excluded triple claims cover regrowth and new siblings. |
| `data-model.md`, history field | Re-disposition must retain its earlier state. | **added** — every re-disposition appends `{from, to, when, why}` to the current row; rendering order is service, ID, from, to, when, why. |
| `scripts/quality-debt-record.mjs`, checkLive comment | A sibling finding is new, not a contradiction. | **clarified** — applies to resolved rows outside the triple map. Confirmed fixed/excluded rows intentionally claim no instance of that rule in that file. Executable code is unchanged. |

All active feature-wide completion clauses inspected now use frozen-inventory confirmation and live
attribution. Old global-zero requirements retain dated historical amendment context. Ledger zeroes,
dated batch measurements and mandatory security/runtime/gate checks retain their original scopes.
This review does not declare T045 or the later publication/cleanup tasks complete.
