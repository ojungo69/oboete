# Direct competitor positioning snapshot

- Date: **2026-08-18 (Asia/Tokyo)**
- Scope: coding-agent memory / continuity products that compete directly with `free-mem`
- Purpose: separate architectural novelty from product-market differentiation and define honest comparison targets.

## Evidence rule

This document records public repository metadata, README claims, and inspectable public behavior only. It does not treat a README claim as benchmark proof. Performance, reliability, memory quality, and continuity correctness must be reproduced through #8 before free-mem makes comparative claims.

Pinned snapshots:

| Project | Pinned commit | Primary inspected source |
|---|---|---|
| free-mem | [`6a9ae0832a9ef97ad5a8a3d48efbfc056b0f538c`](https://github.com/ojungo69/free-mem/commit/6a9ae0832a9ef97ad5a8a3d48efbfc056b0f538c) | [README](https://github.com/ojungo69/free-mem/blob/6a9ae0832a9ef97ad5a8a3d48efbfc056b0f538c/README.md), specs, issues #1/#8/#13 |
| claude-mem | [`fae697a45d107aae567d605916391ab64d8ecae1`](https://github.com/thedotmack/claude-mem/commit/fae697a45d107aae567d605916391ab64d8ecae1) | [README](https://github.com/thedotmack/claude-mem/blob/fae697a45d107aae567d605916391ab64d8ecae1/README.md) |
| Engram | [`47f281cb9dcf20f2c93cd3d9849d10191722d4d3`](https://github.com/Gentleman-Programming/engram/commit/47f281cb9dcf20f2c93cd3d9849d10191722d4d3) | [README](https://github.com/Gentleman-Programming/engram/blob/47f281cb9dcf20f2c93cd3d9849d10191722d4d3/README.md) |
| ai-memory | [`7f052990991aa541022a4bd015b58d1f5a9e8bf5`](https://github.com/akitaonrails/ai-memory/commit/7f052990991aa541022a4bd015b58d1f5a9e8bf5) | [README](https://github.com/akitaonrails/ai-memory/blob/7f052990991aa541022a4bd015b58d1f5a9e8bf5/README.md) |
| remem | [`81a4e2013efbf82ceda2c63db43ecc308611d9a1`](https://github.com/majiayu000/remem/commit/81a4e2013efbf82ceda2c63db43ecc308611d9a1) | [README](https://github.com/majiayu000/remem/blob/81a4e2013efbf82ceda2c63db43ecc308611d9a1/README.md) |
| PAXM | [`1d9a460693e9a51291555409dbac743bf55b347f`](https://github.com/pax-beehive/paxm/commit/1d9a460693e9a51291555409dbac743bf55b347f) | [README](https://github.com/pax-beehive/paxm/blob/1d9a460693e9a51291555409dbac743bf55b347f/README.md) |

A future comparison must record the exact release/tag actually installed, artifact hash, platform, configuration, model/provider, and benchmark fixture version. Branch-head commit pinning here is discovery evidence, not a release certification.

## Market reality

The following are useful requirements but are **not sufficient differentiation** in 2026:

- local-first operation
- SQLite / FTS5
- Rust or Go implementation
- one native binary
- MCP server
- automatic capture through hooks
- multiple coding-agent integrations
- cross-session search
- compact/session handoff
- optional embeddings
- dashboard or viewer
- cloud or cross-machine sync
- provider selection

At least one direct competitor's pinned public materials claim each of these, and several claim many
of them together. Per the evidence rule above, that is a positioning claim, not a verified capability —
install and behaviour are measured through #8.

## Competitor strengths to treat as baselines

### claude-mem — automatic memory UX and adoption baseline

Public positioning includes automatic lifecycle capture, AI compression, progressive disclosure, automatic context injection, citations, a web viewer, multiple client integrations, and cloud sync.

Use as the baseline for:

- installation and first-value UX
- automatic capture and context delivery
- progressive disclosure and token economy
- viewer usability
- memory extraction / summary quality
- ecosystem maturity and documentation

Do not claim superiority merely because free-mem uses Rust or avoids Chroma.

### Engram — simplicity and distribution baseline

Public positioning includes a Go binary, SQLite + FTS5, broad MCP-agent support, setup commands, CLI/HTTP/MCP surfaces, TUI, Git sync, and opt-in cloud replication.

Use as the baseline for:

- one-command setup
- single-binary distribution
- low operational complexity
- agent integration breadth
- CLI / TUI diagnostics
- install, update, and uninstall clarity

A free-mem architecture that is safer but materially harder to install is not product-competitive.

### ai-memory — Rust cross-agent continuity baseline

Public positioning includes a Rust runtime, cross-agent handoffs, lifecycle capture, a durable wiki, FTS and optional richer retrieval, broad client support, local or remote operation, and managed workstreams that resume across harnesses.

Use as the baseline for:

- Rust-native runtime and packaging
- cross-agent handoff coverage
- workstream continuation UX
- multi-client integration breadth
- migration / server deployment flexibility
- transparent operation across different harnesses

This is the closest direct competitor to the proposed free-mem product position. Cross-agent support alone is not a unique claim.

### remem — auditable and privacy-aware recall baseline

Public positioning includes a single Rust binary, SQLite/SQLCipher, Claude/Codex hooks and MCP, source-attributed context bundles, selection/drop audit, current-truth and staleness handling, poisoning gates, and operational diagnostics.

Use as the baseline for:

- encryption-at-rest option
- provenance and injection audit
- current/stale truth handling
- hostile-memory filtering
- bounded context construction
- operational `doctor` quality

Source attribution, temporal validity, and poisoning protection alone are not unique claims.

### PAXM — provider-neutral routing baseline

Public positioning includes local SQLite, multiple remote or self-hosted memory providers, custom JSON-RPC providers, active and passive paths, durable write queues, fail-open provider handling, multiple agent integrations, and published evaluation methodology.

Use as the baseline for:

- provider abstraction and switching
- durable queue behavior
- multi-provider partial failure handling
- setup/configuration ergonomics
- evaluation transparency
- storage-provider portability

Provider flexibility alone is not unique differentiation.

## Honest status of free-mem

At this snapshot, free-mem is pre-release. Phase 1 safety boundaries and runtime-neutral continuity contracts exist, but the repository does not yet provide a supported release artifact, compatibility promise, complete Rust Core, or demonstrated end-user advantage over the products above.

Therefore, current comparative statements must use the following language:

- `designed to` for frozen but unimplemented behavior
- `candidate-validated` for Phase 1 evidence already produced
- `implemented` only when code and tests exist on main
- `benchmarked` only when reproducible reports exist
- `better than` only when the same pinned scenario and measurement contract support the claim

## Differentiation hypothesis

free-mem should not position itself as a generic persistent-memory database. Its testable product hypothesis is:

> **A crash-safe coding-agent continuity runtime that determines whether a previous task can be resumed safely in the current workspace, rather than merely retrieving related history.**

The combined differentiators are:

### 1. Task-lineage-scoped canonical work state

The canonical unit is the logical task lineage, not merely a chat session or repository-wide recent history. A heuristic may propose a boundary but cannot silently supersede the previous task.

### 2. Workspace reconciliation before full resume

Before automatic full delivery, free-mem compares repository/workspace identity, branch/worktree, checkpoint HEAD ancestry, dirty state, file drift, and unresolved operations. Related memory is not enough to authorize automatic continuation.

### 3. Typed in-flight operations with safe replay policy

Commands, tests, tools, and file mutations can remain `unknown`. Unknown results are rendered as `verify_first` or `never_auto`; they are not guessed to be successful, failed, or unexecuted.

### 4. Exact-version capability evidence

Support tiers are based on real client version + capability hash + fixture evidence. Unsupported or unobserved prompt/compact delivery paths downgrade to hint, candidate selection, or manual recovery.

### 5. Fenced checkpoint delivery lifecycle

Checkpoint content, initial claim, delivery attempt, engagement, and acceptance are separate. Delivery or one successful model turn does not automatically consume the checkpoint.

```text
claimed -> delivered -> engaged -> accepted
                   \-> dismissed
         \-> abandoned / lease_expired
```

### 6. Public continuity conformance suite

The same language-neutral fixtures should apply to TypeScript reference, Rust Core, Claude, Codex, and future adapters. Critical failures are release-blocking rather than documentation-only expectations.

These elements are individually comparable to ideas in other systems. The differentiation claim depends on their **combined behavior and measured result**, not on terminology.

## Required benchmark expansion

Issue #8 currently centers claude-mem. The benchmark must use role-specific direct baselines instead of one universal ranking.

**This document does not move the release gate.** The canonical Core 1.0 criterion is SC-8 in
`specs/001-agent-memory-core/spec.md`, restated in `resume-continuity-addendum-v6.2.md` §14: major public
resume scenarios must meet #8's frozen claude-mem non-inferiority gate, or carry a reviewed exception ADR.
That remains the only blocking baseline today. The table below is the target shape of the expanded
benchmark; each row becomes blocking only when #8's frozen gate is extended to it, which is tracked in
[#79](https://github.com/ojungo69/free-mem/issues/79). Treating these as blocking before that extension
would leave two disagreeing authorities, and the canonical one would win.

| Product quality | Required public baseline |
|---|---|
| automatic memory UX / progressive disclosure | claude-mem |
| install simplicity / single binary / doctor | Engram |
| Rust cross-agent workstream continuity | ai-memory |
| provenance / staleness / context audit / privacy | remem |
| provider routing / storage portability / queue degradation | PAXM |
| frozen legacy behavior / differential oracle | codemem + free-mem TS |

### Absolute continuity gates

The following are free-mem release properties even when a competitor does not support the scenario:

- wrong-project or wrong-workspace automatic resume: **0**
- automatic replay of an unknown unsafe operation: **0**
- concurrent active delivery attempts for one checkpoint on a single device: **0**
- checkpoint accepted before engagement evidence: **0**
- fabricated canonical work-state field: **0**
- data loss / duplicate commit / split brain under required fault cases: **0**
- Agent blockage caused by optional memory/provider failure: **0**

The delivery gate is scoped deliberately, matching `agent-memory-final-spec-v6.md` §22.4 / §27.7. Two
things that look like duplicate delivery are permitted by the contract and are not failures:

- **A later attempt on the same checkpoint after the previous one ended.** `abandon` and lease expiry
  clear the claim so the checkpoint becomes eligible again; re-claiming it is a required fixture
  (`resume-continuity-addendum-v6.2.md` §6.1). What must never happen is two attempts holding the claim
  at once — the CAS on `checkpoint id + revision + claimFence + destination session` is what enforces it.
- **Cross-device duplication in local-first mode.** Delivery state is not synced, so two partitioned
  devices can claim the same checkpoint; the spec calls this permitted behaviour and keeps both results
  as fork lineage. Requiring zero there is a Personal Cloud decision with a server-authoritative claim
  authority, not a Core 1.0 property.

### Comparative metrics

- installation success rate and steps to first verified recall
- re-explanation turns and tokens
- time to first useful action after restart
- task completion success after resume
- critical-state recall
- fabricated / stale state rate
- injection precision and injected tokens
- cold/warm start and idle RSS
- crash recovery latency
- memory/query latency at fixed scales
- migration, rollback, and uninstall success

Unsupported features must be reported as `unsupported`, not silently scored as zero or excluded after results are seen.

## Product message

Primary positioning:

> **free-mem — crash-safe continuity for coding agents.**

Supporting explanation:

> Other memory tools help an agent recall what happened. free-mem is designed to verify what can safely continue now: the task, workspace, pending operations, and exact client capability.

The README and demos must avoid claiming that every designed property is already implemented.

## Killer demonstration

The first public demonstration should be a fixed, reproducible scenario rather than a feature tour.

1. Start an implementation in Claude Code.
2. Leave a test or command with no trustworthy terminal result and terminate abruptly.
3. Change branch, worktree state, or a relevant file for one negative run.
4. Start Codex.
5. Show that free-mem identifies the task lineage, reconciles the workspace, marks the operation `verify_first`, and does not auto-replay it.
6. Show that the incompatible run is downgraded or rejected instead of receiving full automatic resume.
7. Restore a compatible workspace and continue to the first useful action without re-explaining the task.
8. Emit the machine-readable report and compare the same scenario where possible with pinned baselines.

The demo is complete only when another user can reproduce it from a clean machine using released artifacts.

## Roadmap implications

1. Adopt Rust Core as the strategic default runtime target through ADR-005.
2. Keep TypeScript/codemem as a reference and migration source until verified cutover.
3. Expand #8 through a dedicated direct-competitor benchmark work package.
4. Build a 90-second continuity demo and clean-install path before broad Agent or Cloud expansion.
5. Treat setup, doctor, importer, rollback, and uninstall as product features, not release packaging afterthoughts.
6. Delay broad feature-count competition until Claude Code <-> Codex crash/compact continuity is demonstrably better.

## Re-baseline policy

Direct competitors evolve quickly. Re-baseline when any of the following occurs:

- Core 1.0 release candidate is cut
- a pinned competitor changes a relevant major capability or architecture
- a comparison adapter no longer installs or reproduces the recorded behavior
- a public benchmark claim is added to the README
- six months pass since the latest frozen competitor manifest

A re-baseline updates source pins and reports; it does not silently change previously frozen thresholds after seeing results.
