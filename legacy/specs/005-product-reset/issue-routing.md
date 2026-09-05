# GitHub Work Routing: Product Reset M0

**Snapshot**: 2026-08-25

**Repository**: `ojungo69/free-mem`

**Input state**: 69 open issues and two open pull requests

This file is the mutation ledger for T004 and T015-T018. Every issue body, current label set,
comments, related pull request, and relevant source path was reviewed before classification.
Closing an issue below does not claim its old implementation is fixed; it means its valid scope was
copied into a focused replacement or its product premise was superseded.

## Classification Rules

- **Keep active**: concrete current-runtime privacy, data-loss, or correctness defect that directly
  blocks the next automatic-memory slice.
- **Keep deferred**: concrete current-runtime or release defect that remains valid but is not one
  of the five active blockers.
- **Replace**: copy the still-valid acceptance requirements into one focused Product Reset slice,
  then close the old umbrella or harness issue.
- **Supersede**: close work whose only product outcome is Verified Continuity, Rust-first cutover,
  old capability-rig implementation, Cloud, additional Agents, broad MCP, or shared-memory scope.
- **Resolved**: close only when the new authority or merged source directly resolves the reported
  problem.

## Replacement Issues

| Role | Issue | Status after creation |
|---|---|---|
| Product Reset parent | [#136](https://github.com/ojungo69/free-mem/issues/136) | `status: in progress`, `target: technical alpha` |
| Slice 1 — automatic runtime and fail-open durability | [#137](https://github.com/ojungo69/free-mem/issues/137) | `status: ready for implementation`, `target: technical alpha` |
| Slice 2 — profiles, providers, semantic retrieval, InjectionPack | [#138](https://github.com/ojungo69/free-mem/issues/138) | `status: blocked`, `target: technical alpha` |
| Slice 3 — doctor, lifecycle, package, Technical Alpha release | [#139](https://github.com/ojungo69/free-mem/issues/139) | `status: blocked`, `target: technical alpha` |

Required new labels:

- `target: technical alpha`
- `status: deferred`
- `area: product`

## Keep Open

| Issue | Disposition | Post-reset status | Evidence and next boundary |
|---|---|---|---|
| #81 License files missing from 11 dependencies | Keep deferred | `status: decision needed` | PR #77 did not settle how to represent absent upstream license text; resolve before Slice 3 publish without inventing rightsholder data. |
| #123 sibling duration calculation in memory-artifact-report | Keep deferred | `status: deferred` | Current source still uses imprecise `julianday()` arithmetic; fix with exact boundary tests in Slice 1 maintenance work. |
| #124 `private_content_omitted` false positive | Keep deferred | `status: decision needed` | Current code conflates reserved markup seen with content removed; fail-closed behavior prevents egress but the reporting contract needs an owner decision. |
| #126 duplicated `</private>` keeps private content | **Keep active** | `status: ready for implementation` | Concrete current privacy bug in `ingest-sanitize.ts`; preserve existing stray-close behavior and add the duplicated-close regression. |
| #127 duration buckets missing from text output | Keep deferred | `status: deferred` | Computation is fixed, but human report/compare output still omits it; route to Slice 3 inspection work. |
| #128 orphan-session cleanup has no caller | Keep deferred | `status: deferred` | The existing cleanup function is unwired; add it to an existing bounded daemon-maintenance path without a new scheduler. |
| #129 redaction cold-start consumes the scan budget | **Keep active** | `status: ready for implementation` | Current retained kernel can discard content or persist an empty memory after worker startup consumes the deadline; separate readiness and scan budgets. |
| #130 local-only content reaches remote observer | **Keep active** | `status: ready for implementation` | Current raw-event flush ignores the egress flag; remote providers must skip it while explicit local providers may process it. |

Active-count policy after replacement creation: parent + Slice 1 + #126 + #129 + #130 = five.
The other kept issues remain open but do not carry `status: in progress` or
`status: ready for implementation`.

\#126, #129, and #130 are sub-issues and entry criteria of Slice 1 #137. They are counted as work
inside the one active product slice, not three parallel product tracks. #137 cannot be declared
complete while any remains open, and its focused spec must include their privacy/data-loss
regressions.

## Replace with Slice 1

The replacement issue consolidates bidirectional real-hook E2E, daemon startup and prompt-triggered
flush, bounded capture, fail-open spool, idempotent recovery, the Slice 1 comparison subset, and
lexical fallback. Only the runtime and minimal-E2E rows below are Slice 1 exit criteria; broader
comparison publication and long-resource-run safeguards are owned by Slice 3.

| Issue | Still-valid scope copied to Slice 1 |
|---|---|
| #19 | Prove actual supported-version capture-to-injection paths instead of composing self-reported capability cells. |
| #40 | Derive session/project/source identity from supported host context; reject caller self-asserted authority. |
| #45 | Bound admission fairly so an event flood cannot selectively evict accepted live work. |
| #46 | Duplicate and semantic no-op delivery must not create new memory, revision, or summary work. |
| #49 | Fix canonical stored bytes once and prevent later summary/embedding/injection work from mutating them. |
| #61 | Expose monotonic accepted, queued, dropped, retried, and oldest-boundary diagnostics without silent overflow. |
| #62 | Keep raw identifiers, prompt fragments, secrets, and absolute paths out of spool/state/logs; preserve sensitivity at capture. |
| #68 | When a primary destination is full, retain event identity in bounded spool or an explicit loss record before consuming its key. |
| #69 | Enforce common UTF-8 byte limits at daemon intake; never truncate identifiers into a different identity. |
| #80 | Replace crash-safe checkpoint demo with Claude-to-Codex and Codex-to-Claude capture-summary-store-inject E2E. |
| #91 | Prove prompt-before-model injection using a boolean marker and a required negative fixture. |
| #93 | Run comparison in disposable state without exposing the real user home or credentials. |
| #96 | Give independent cases fresh workspace, home, and Agent configuration; inherit state only in declared continuity cases. |
| #100 | Run both Claude Code and Codex adapters as real processes under one parameterized acceptance contract. |
| #101 | Mark interrupted runs incomplete; replay without success fabrication or duplicate storage. |
| #102 | Generate hook configuration with structured writers and test paths containing spaces and shell characters. |
| #108 | Require positive completion evidence; a missing error record never implies success. |

## Replace with Slice 2

| Issue | Still-valid scope copied to Slice 2 |
|---|---|
| #8 | Use pinned claude-mem automatic-memory behavior as a test donor for summary quality, stale/duplicate handling, retrieval, and injection efficiency. |
| #11 | Make summary and embedding egress independent, explicit, redacted, bounded, revocable, and provider-failure tolerant. |
| #32 | Generalize Slice 1's fixed final byte/token gate into exact time, byte, token, item, candidate, and lane budgets across profiles, including an explicit zero-delivery compilation refusal with its own truthful lifecycle. |
| #67 | Preserve Japanese, English, mixed, short-CJK, and old-important-memory recall in lexical and semantic paths with visible omission reasons. |

## Replace with Slice 3

| Issue | Still-valid scope copied to Slice 3 |
|---|---|
| #9 | Detect current installation ownership and provide transactional backup, migration, verification, rollback, and split-writer prevention. |
| #10 | Generate and verify artifact-specific SBOM, checksums, license boundary, and Codemem attribution. |
| #22 | Keep shipped Product Runtime source in required analysis without prematurely fixing a two-project Sonar topology. |
| #56 | Semantically validate restored scope identity, quarantine invalid artifacts, and expose safe manual recovery. |
| #57 | Calendar-validate every persisted timestamp before restore; never silently normalize corrupt data. |
| #66 | Report and bound local store/spool/quarantine/backup growth; use explicit dry-run/apply deletion and verified backup. |
| #82 | Provide one Linux/WSL path for install, first value, doctor, inspection, update, rollback, backup, and uninstall. |
| #83 | Restrict publish authority to protected Trusted Publishing/OIDC workflow with notice, checksum, SBOM, and packed-artifact gates. |
| #72 | Apply common bounds during restore, converge over-limit state, and report every repair. |
| #90 | Keep evidence-strength levels and forbid candidate-written evidence from claiming certified status. |
| #94 | Bound comparison input bytes, lines, line length, and JSON depth before parsing. |
| #95 | Prevent escaped child processes from observing the next release-certification case's state or credentials. |
| #97 | Remove home, repository, temporary, and credential paths from public comparison artifacts and failures. |
| #98 | Verify ephemeral-secret cleanup and stop later certification runs when cleanup fails. |
| #103 | Record actual Node and Agent paths/versions; unverified PATH selection cannot claim certified evidence. |
| #105 | Start the release evaluator from a clean environment and downgrade evidence when binary provenance is unproven. |
| #106 | Publish comparison artifacts atomically and preserve the previous complete result on failure. |
| #107 | Require runner-generated run identity/descriptor before importing evidence. |

## Close as Superseded or Resolved

| Issue | Disposition | Rationale |
|---|---|---|
| #1 | Superseded | Rust is now evidence-triggered, not the standard Alpha runtime; selected safety invariants are already in the new contracts. |
| #12 | Superseded/deferred | Personal Cloud is a post-Alpha candidate and no longer needs an open implementation umbrella before local validation. |
| #13 | Superseded/deferred | Task-lineage, workspace reconciliation, checkpoint claim, and delivery acceptance are the deferred Verified Continuity program. |
| #24 | Superseded | The old capability matrix is not reused; its field-level provenance design is unnecessary. |
| #31 | Superseded | The retired continuity oracle's hand-written type/schema dual authority is not carried into focused contracts. |
| #53 | Superseded/deferred | Global lineage ordinal, fork, head selection, and checkpoint CAS are deferred continuity/cloud concerns. |
| #54 | Superseded/deferred | TS/Rust revision hash parity and its migration profile are inactive while Rust cutover is deferred. |
| #58 | Superseded | Terminal partial-conflict vocabulary belongs to the retired reference model; new diagnostics use bounded product reasons. |
| #64 | Superseded | Codacy advisory harness topology and node:test false positives do not block the shipped Alpha path. |
| #65 | Superseded/deferred | DeepSeek adapter certification is post-Alpha; create a fresh exact-version issue only when additional Agents reopen. |
| #70 | Superseded | The reported orphan-fingerprint map is in the retired continuity model; Slice 1 independently validates digest conflicts. |
| #71 | Superseded | Terminal-order authority is specific to the deferred continuity model. |
| #73 | Superseded | Abandon/terminal correlation is deferred; actual spool/ingest paths share the new idempotency requirement. |
| #74 | **Resolved** | Constitution 2.0.0 removes the obsolete local-only development prohibition and permits requested public PR/Issue work. |
| #76 | Superseded/deferred | Broad remote/client MCP support is post-Alpha; reopen from a focused new spec after local validation. |
| #79 | Superseded | Universal multi-competitor continuity ranking is replaced by a small fixed Product Alpha comparison and claude-mem UX donor tests. |
| #84 | Superseded | ADR-005 Rust-first canonical-chain update conflicts with the accepted Product Reset. |
| #99 | Superseded | The lock-layout bug belongs to the retired rig implementation. |
| #104 | Superseded | Refactoring duplicated functions in the retired rig has no independent product outcome. |
| #132 | Superseded | Source-aware continuity state/checkpoint/sharing program is not the automatic-memory Alpha. |
| #134 | Superseded | The foundation audit is complete; Codemem stays and claude-mem is a UX/test donor. |
| #135 | Superseded | Verified Continuity Engine is no longer the product definition or execution order. |

## Pull Requests

| PR | Disposition | Evidence |
|---|---|---|
| #131 Dependabot GitHub Actions bump | Keep open | Independent two-line maintenance update; all gates except DCO pass. Do not merge until a signed-off replacement commit or compliant PR exists. |
| #133 source-aware continuity S0 | Close unmerged | 22 files and 15,209 additions implement the superseded contract. Current checks are green but nine current-head review threads remain, including correctness and privacy findings. No file is cherry-picked. |

## Standard Disposition Comments

### Replaced

> Product Reset (2026-08-25): this issue is being closed because its still-valid acceptance scope
> is consolidated into the focused replacement linked below. Closing does not claim the old
> implementation bug or contract was fixed. The copied scope and final mapping are recorded in
> `specs/005-product-reset/issue-routing.md`.

### Superseded

> Product Reset (2026-08-25): this work belonged to the former Verified Continuity, Rust-first,
> broad-Agent/Cloud, or capability-rig direction and is no longer active Product Alpha scope.
> Historical discussion remains here. Reopen through a new focused specification only after the
> automatic-memory Alpha provides evidence that this capability is needed.

### PR #133

> Product Reset (2026-08-25): closing without merge. This PR implements the superseded
> source-aware continuity contract and would add a competing product authority. Its discussion
> remains historical evidence; no contract/schema/harness file is being cherry-picked.

## Final Mutation Evidence

Recorded after T015-T018:

- Parent issue: #136
- Slice 1: #137
- Slice 2: #138
- Slice 3: #139
- Issues kept open from the original set: 8
- Issues closed from the original set: 61
- New Product Reset issues: 4
- Total open issues after reset: 12
- Active-status issues: 5 (`#136`, `#137`, `#126`, `#129`, `#130`)
- PR #131: open; DCO is the only failed check
- PR #133: closed and unmerged; branch retained
- Final GitHub verification timestamp: 2026-08-25T17:32:54+09:00
- base-to-HEAD, worktree, and index `git diff --check`: pass
- Runtime/harness diff: none
