# Design B, section 8: evaluation and build order (draft, 2026-09-25)

Short names as in sections 5-7. Inputs: RD/options-draft.md §9 (measurement plan, pass lines fixed before measuring), RD/improvements-synthesis.md (MUST M1-M23 with costs, SHOULD S1-S9), RD/issue50.md §8 (connection to the current code and order) and §9 (acceptance), sections 1-7 and their spikes, owner decisions (finish every feature before public release; the project's lighter process: milestone notes in docs/, one Codex review lane per PR).

## 1. Evaluation rules (carried from options-draft §9 and issue50 §9)

- Pass lines are fixed before any run. Tune on the dev split (70%), decide once on the untouched test split (30%); no fix-and-remeasure loop on the test split. A separate unseen final set is kept for the release gate (issue50 §9).
- The LLM judge decides pass/fail only after it meets the PR-B trust condition, calibrated on human labels; its model and version are recorded with each run.
- Baselines on the same inputs: no memory, the current oboete at comparison time, claude-mem (default and with its 90-day window removed). Hindsight is not a baseline: it failed Phase 0 by the pre-stated rule (RD/phase0.md), and M1 was conditional on Phase 0.
- Search-only and end-to-end results are reported separately; an imported memory found by search is not credited to oboete's curation.
- Every feature is measured alone and in combination; a gain in search that costs resume or latency is not adopted automatically.

## 2. What is measured, and the lines

| Id | What | Pass line | Source |
|---|---|---|---|
| M1 | retrieval, 112 questions | a change replaces the hybrid (nDCG@10 0.545 today) only at +0.03, p < 0.05; no slice's recall@10 drops > 0.02; nothing ships below claude-mem on any slice | options-draft §9 |
| M2 | coverage and crash | 100% of seqs curated, elided with marker, or skipped with reason; crash at 20 points gives identical rows (hard); red against today's code first | options-draft §9 |
| M3 | decisions and gates | overturned decisions shown as current = 0% (hard), control pairs dropped ≤ 2%; `decided` precision ≥ 0.95, recall ≥ 0.80; injection and taint canaries 0% (MUST-M4); per-kind precision and recall (M21) | options-draft §9, MUST-M4, M21 |
| M4 | deletion | 0 hits after forget across every file, device, hub, R2, Vectorize, with the re-index, re-sync, re-import, restore, crash and rescan cases (hard) | sections 5, 6 |
| M5 | resume across sessions and devices | open-item recall ≥ 0.80; closed items shown as open ≤ 10%; knowledge on device 2 ≤ 5 min p95; none tier open-item recall ≥ 0.50 from the manifest; also decides the worker lifecycle | options-draft §9, section 5 |
| M6 | lookup ("how did we fix X") | ≥ 0.70 and ≥ current + 0.10; cited-span validity ≥ 0.95 | options-draft §9 |
| M14 | hook write and redaction cost | sets the hook latency line on the slowest machine (provisional 20 ms p95) | section 2 |
| M21 | slices | English within 0.05 of Japanese; cross-lingual subset; owner corrections survive rebuild, re-derivation, resync (hard) | MUST-M21 |
| M22 | operations at ~330k documents | MCP p95 ≤ 1.5 s; slowdown under writes ≤ 20%; first sync of a blank device; manifest truncation per agent | MUST-M22 |
| Inject | per-prompt injection threshold | on by default only if the one-sided 95% upper bound of irrelevant injections is ≤ 10% | decision 12, section 4 |
| Judge | judge-model roles | selection: curator input −30% with recall drop ≤ 0.02; veto: fewer wrong `decided` with recall kept | section 3 |
| Window | curation window size | the smallest size that passes M2, M3 and M6 | section 3 |
| Rerank | S3 reranker | the M1 line | S3 |
| Public | JQaRA, JaCWIR sanity check | defaults do not fall apart outside the owner's data | section 4 |
| Cost | per heavy day | curator calls ≤ 20% of each daily cap; paid ≤ USD 5/month | options-draft §9 |
| Install | fresh machines | install → setup two agents → a memory recalled in a new session ≤ 10 min, clean doctor, on all 5 targets (2 in CI only) | MUST-M23, section 7 |

The hook latency lines of the old design (300 ms for injection) are not carried: section 4 set hook lines by measurement (M14), because hooks only read what the worker prepared.

## 3. Spikes before building (throwaway, docs/ notes)

- **Curator spike** (biggest risk, options-draft §4 "Biggest risk"): M3 gate precision and recall on Japanese sessions with the dev split; window size; per-CLI no-tool mode and the isolation check (section 6); agy with a private config that denies every tool.
- **Hub spike** (section 5): items 1-3 decide (Node-free deploy, latency from a US host, trigram in the DO); items 4-8 harden.
- **Hook spike** (M14): raw.db writes under `synchronous=FULL` with 64 KB and 256 KB outputs and a full redaction scan, on WSL, Windows native and the M1 iMac.

## 4. Build order (milestones; each has a docs/ note and PRs)

1. **Baselines and fixtures first** (issue50 §8 row 1): freeze the eval sets and the sealed test and final splits; failure fixtures (the 24-hour session, a decision only in the middle, overturned pairs, deletion canaries); baseline runs of the current oboete and claude-mem.
2. **Record** (sections 1-2): raw.db with (device, seq), full redaction, `synchronous=FULL`, write-failure reporting, backups, tombstones in raw.db from day one (deletion and sync are designed in, issue50 §8 row 7); raw FTS and the deterministic manifest. The none tier works end to end: record, search, resume from the manifest.
3. **Curate** (section 3): windows and checkpoints, claims with speaker, status, evidence and supersedes, code gates, digests, provider chain with curator isolation, `rebuild` and `recurate`. M2 and M3.
4. **Deliver** (section 4): packets and shortlists, SessionStart and per-prompt injection, compaction, current-first search on MCP, CLI and viewer. M1, M5 (one device), M6.
5. **Forget and safety** (section 6): the four levels, the forget pipeline and `forget_jobs`, env allow-list, viewer writes. M4 (local).
6. **Sync** (section 5): hub from the donor fork with purge, docs/hub-protocol.md and its fake hub, remote MCP with OAuth and grants. M4 (devices), M5 (devices), M22 first sync.
7. **Ship** (section 7): installer, update with rollback, v1 and claude-mem import, transcript import, doctor, docs, NOTICE. Install line on 5 targets.
8. **Finish**: M10 live checks for all 7 agents, the final run on the unseen set, the release gate (section 7).

The owner's machines switch from the current oboete after milestone 4 (single-device record, curate and deliver), through the dogfood user first; sync joins after milestone 6. The current oboete has no sync either, so nothing is lost before that (owner question 1).

Who builds: Claude Code writes the core (raw store, curation, gates, delivery, forget, hub purge); independent pieces (agent adapter ports, the donor port, transcript parsers) go to Codex or Grok in parallel (project CLAUDE.md). Security-scope pieces are never delegated.

Size: MUST roughly doubled B to 34-41 PR-sized units (RD/improvements-synthesis.md); sections 5-7 added the protocol doc, the curator isolation gate, forget jobs, the update steps and transcript import, about 4-6 more.

## Owner questions

1. Switch the owner's machines after milestone 4 (recommended), or only when everything is finished?
2. Owner time for labels: M3 needs ≥ 50 human-confirmed overturned pairs, M5 spot-checks 20% of labels, and the judge is calibrated on human labels (PR-B). Estimate and whether the owner can give it.
