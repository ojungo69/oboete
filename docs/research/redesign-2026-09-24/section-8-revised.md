# Design B, section 8: evaluation and build order (revised after the audit, 2026-09-25)

Bullets that changed carry their finding ids in brackets. Letters A to J mark gaps I found while revising that were not on the audit list. Short names follow sections 5-7:
- "RD/" is docs/research/redesign-2026-09-24/.
- "proposal" is docs/research/search-sync-proposal-2026-09-23.md.
- "MUST-Mn" and "Sn" are items in RD/improvements-synthesis.md.
- "eval/" is ~/.oboete/eval/, which exists only on the owner's machine (docs/pr-b.md:13).

A copy of this text is at /tmp/claude-1000/-home-jura-projects-oboete/035bf06c-56a7-4208-83ad-48d576b8d187/scratchpad/section-8-revised.md.

Inputs:
- RD/options-draft.md §9 (the measurement plan);
- RD/improvements-synthesis.md (MUST M1-M23, SHOULD S1-S9);
- RD/issue50.md §8 (build order) and §9 (acceptance);
- sections 1-7 and their spikes;
- the owner decisions: finish every feature before public release, and the lighter project process (milestone notes in docs/, one Codex review lane per PR).

## 1. Revised section 8

### 1. Evaluation rules

- **What this section replaces** [requirements-4, requirements-6, C]:
  - Where this section differs from RD/options-draft.md §9, this section wins. Three parts of §9 are not carried: its Hindsight corpus sizing and system list (:320-322), A's decision rule (:348-350), and its 300 ms hook lines (:339-340, :344).
  - Section 7's release gate cites §9 (RD/section-7-revised.md:240) and says it will use this section once this section is settled (:244). So the release gate reads this section.
  - Every MUST item's own "Measure" lines are acceptance tests of the milestone that builds it (MUST-M1 to M23; M20 is postponed with the folder transport, decision 12). §4 names them per milestone.
  - The table in §2 lists only the lines that need a split, a judge, a scale run or a release decision.
- **Three kinds of line** [validity-2; refuted inherited-2's one-line fix]:
  - **Judged quality lines** (M1, Raw, M3, M5, M6, M21, Inject, Judge, Window, Rerank). They are fixed before the run, tuned on the dev split and decided once on the test split. There is no fix-and-remeasure loop on the test split.
  - **Invariant fixtures** (M2, M4 and the MUST fixtures). The test is written first and must fail against today's code, then pass. They are deterministic, so going from fail to pass is not a remeasure.
  - **Engineering lines.** There are two:
    - the write-hook line (M14), set at milestone 2;
    - the read-hook line (SessionStart and per-prompt injection), set at milestone 4 from the warm path (packet and shortlist).

    The method is fixed now. Each number is set once, from its first measurement on the slowest machine (RD/sections-1-4.md:20, :41), and stays fixed for every later run.
- **No 300 ms hook budget anywhere** [A]:
  - The inputs still carry the old 300 ms line in five places:
    - M5's worker rule (RD/options-draft.md:339-340);
    - MUST-M9 (RD/improvements-synthesis.md:149);
    - MUST-M22 (:381);
    - S2 (:434);
    - S4's trigger (:445).
  - Each now means the read-hook line. Section 4 set hook latency by measurement, not from the old budget, which included a Workers AI call (RD/sections-1-4.md:41; RD/constraints-synthesis.md:314).
  - M14's write number does not apply to the read paths. It times an fsync and a redaction scan, not a read (RD/sections-1-4.md:20).
- **Test-split budget** [validity-2, merges refuted inherited-3]:
  - The 112-question test split has already been used once, for D2's hybrid default (docs/pr-d.md:175).
  - It is used once more, at milestone 4, for every retrieval candidate of the redesign in a single run: the M1 changes, the three Raw variants and the S3 reranker. p-values are Holm-corrected across the candidates in that run.
  - A candidate that fails goes back to dev and is never tried on test again.
  - After milestone 4 the test split decides nothing. The unseen final set decides the release.
  - Section 7's cut-over check reuses the 112 questions (RD/section-7-revised.md:166). It chooses nothing, so it does not count as a use of the split.
  - End-to-end lines follow the same rule on the replay set: dev transcripts tune, and the held-out transcripts decide once.
- **Unseen final set** [validity-8]: issue50 requires a final set kept apart from every set whose results have been seen (RD/issue50.md:194).
  - Source: questions built by docs/eval/build_queries.py from a fresh read-only claude-mem copy (docs/pr-b.md:13).
    - Only sessions that started after the 2026-09-24 copy (the one the 424 questions came from) are used. So the set shares no session with the 424 or with the replay set.
    - Add at least 10 new held-out transcripts from the same period, for the end-to-end lines.
  - Size and strata: at least as many questions as the test split (112), stratified the same way (Japanese/English, developer prompts/agent searches).
  - Sealing:
    - Milestone 1's note records the selection rule and the seed.
    - The set is drawn and judged only at milestone 8, by a judge that passed calibration.
    - Nobody tunes on it.
  - Use: M1, M3, M5 and M6 run on it once. A failed final run uses up the set: the fix is decided on dev, and a new final set is drawn from later sessions.
- **Judge trust** [validity-1, validity-6]:
  - The PR-B trust condition (proposal:240):
    - about 50 human-labelled pairs;
    - binary relevance κ ≥ 0.4, until 10 or more system configurations exist;
    - after that, Kendall τ ≥ 0.85 on the system order.

    Until one of these passes, the judge decides nothing.
  - B3 (docs/pr-b.md:9) is milestone 1's first gate.
  - D2's test result (hybrid nDCG@10 0.545) was judged by claude-sonnet-5 before the trust condition was measured (eval/runs-test/report-test.txt:1; docs/pr-d.md:173). So that result, and the default it set, are provisional. If B3 fails, D2's test pool is judged again, by a judge that passes or by the owner's labels, before M1 uses it as the baseline.
  - Each run records the judge's model and version. Calibration is rerun after a judge model change (RD/improvements-synthesis.md:539).
  - One owner does all the labelling (RD/constraints-synthesis.md:101). At least a week after labelling, the owner labels a blind random 20 of the 50 calibration pairs again. The owner's agreement with their own earlier labels is reported next to the judge's agreement.
- **Where labels come from** [validity-5]:
  - Human labels are split by session hash, like the questions (docs/pr-b.md:40).
  - Calibration pairs and tuning labels come from dev pools and dev transcripts only.
  - Test-side labels are opened only in the run that decides.
- **Baselines**, all on the same inputs:
  - no memory;
  - the current oboete at comparison time;
  - claude-mem, both default and with its 90-day window removed.

  Hindsight is not a baseline. It failed Phase 0 by the rule stated in advance (RD/phase0.md:13, :27, :31), and M1's Hindsight arm depended on Phase 0.
- Search-only results and end-to-end results are reported separately. An imported memory found by search does not count toward oboete's curation.
- Every feature is measured alone and in combination. A gain in search that costs resume or latency is not adopted automatically.

### 2. What is measured, and the lines

| Id | What | Pass line | Source |
|---|---|---|---|
| M1 [inherited-1, validity-2, H] | Retrieval: the 424-question set (dev 312, test 112; docs/pr-b.md:64) on the 178,502-document evaluation store (docs/pr-d.md:14), the same store and questions D2 used. No sub-corpus: B has no per-document LLM ingest, and embedding the whole store cost about USD 0.80 (docs/pr-d.md:14). Systems: FTS only (e0), the current hybrid, claude-mem default, claude-mem without its window | A change replaces the hybrid only at nDCG@10 ≥ hybrid + 0.03, p < 0.05 (Holm across the run's candidates). Both are scored in the same judged pool: 0.545 is D2's pool value, and the size of the pool moves the number (docs/pr-d.md:25, :121). No slice's recall@10 drops > 0.02. Nothing ships below claude-mem on any slice | options-draft §9, without its Hindsight corpus rule and systems (RD/options-draft.md:320-321) |
| Raw [inherited-4] | Raw chunks in search: excluded, included with an RRF penalty, or raw only | The individual-improvement line: nDCG@10 ≥ default + 0.02, no slice drop > 0.02. This decides section 4's raw-chunk handling, which options-draft left to M1 (RD/options-draft.md:126). Raw chunks come only from the agent transcripts that still exist: Claude Code's from 2026-06-19 and Codex's from 2026-08 (read-only listing of ~/.claude/projects and ~/.codex/sessions, 2026-09-25). The store's prompts go back to 2025-12-14 (eval/claude-mem-2026-09-24.db). So Raw is scored only on the test questions typed inside the transcript window (about the 29 typed within 90 days, eval/runs-test/report-test.txt), with N reported. If +0.02 cannot reach p < 0.05 at that N, section 4's stated default stays (raw ranked below curated rows, RD/options-draft.md:126) | RD/options-draft.md:327 |
| M2 | Coverage and crash | Every seq is curated, elided with a marker, or skipped with a reason (100%). A crash at 20 points gives identical rows (hard). The test fails against today's code first | options-draft §9 |
| M3 [validity-4] | Decisions and gates | Overturned decisions shown as current = 0% (hard), on ≥ 50 human-confirmed test pairs, at least 20 of them across sessions (MUST-M3), plus ≥ 20 dev pairs for tuning. Control pairs dropped ≤ 2%. `decided` precision ≥ 0.95 on ≥ 100 claims the curator marked `decided`. `decided` recall ≥ 0.80 on ≥ 100 decisions the owner confirmed in the held-out transcripts. At N = 100, an observed 0.95 has a one-sided 95% Wilson lower bound of 0.90, and an observed 0.80 one of 0.73. Injection and taint canaries 0% (MUST-M4). Precision and recall per kind (MUST-M21) | options-draft §9, MUST-M3, M4, M21 |
| M4 [requirements-5, G] | Deletion | 0 hits after forget in every file, on every device, in the hub, R2 and Vectorize (Vectorize only if the hub spike keeps it). Covers the re-index, re-sync, re-import (claude-mem and transcripts), restore, crash, in-flight, rescan and retention cases (hard). The grep leaves out the limits that sections 5-7 list and doctor names: the DO's 30-day history (RD/section-5-revised.md:20), migration snapshots and evaluation copies (RD/section-7-revised.md:140-141) | sections 5, 6, 7 |
| M5 [build-1, A] | Resume across sessions and devices | Open-item recall ≥ 0.80. Closed items shown as open ≤ 10%. Knowledge on device 2 within 5 min (p95). None tier: open-item recall ≥ 0.50 from the manifest. Worker rule: the worker is kept only if running without it misses the read-hook line at SessionStart, propagation p95 ≤ 5 min, or MCP p95 ≤ 1.5 s on the slowest device with local embeddings. The always-on service is used only if the hook-started worker misses the same lines. M5 also decides the WebSocket wake (RD/section-5-revised.md:12). §4 places the runs at milestones 4 and 6 | RD/options-draft.md:338-340, with its 300 ms read as the read-hook line |
| M6 | Lookup ("how did we fix X") | ≥ 0.70 and ≥ current + 0.10. Cited-span validity ≥ 0.95 | options-draft §9 |
| M14 | Hook write and redaction cost | Sets the write-hook line on the slowest machine (provisional: 20 ms p95) | section 2 |
| Read hook [A] | SessionStart and per-prompt injection latency | The read-hook line: set from the first measurement of the warm path on the slowest machine at milestone 4, then fixed. M5's worker rule, M22, MUST-M9 and S2 use it | section 4 (RD/sections-1-4.md:41) |
| M21 [validity-3] | Slices | English test slice: the 7 English test questions (docs/pr-d.md:125) plus new questions from real English sessions, drawn through the same session-hash split (docs/pr-b.md:40) until the test side has at least 53 more, so N ≥ 60. The 40 English dev questions (docs/pr-d.md:31) are for tuning. Pass: English within 0.05 of Japanese, and M1's lines hold on the English slice. Error rates: per-question nDCG@10 spread is 0.16-0.17 on D2's test runs and 0.18-0.20 on dev (my computation from eval/runs-test/hybrid-d2.trec and eval/judgments.jsonl). With N = 60 English against 103 Japanese, a system that is truly equal in both languages fails the 0.05 line 4-6% of the time, and one that is truly 0.10 worse in English passes 4-6% of the time. The English-minus-Japanese gap is reported with its 95% interval (about ±0.06). If the store holds fewer than 53 more English questions, the rest come from sessions after the freeze, as for the final set. Also: a cross-lingual subset, and owner corrections survive rebuild, re-derivation and resync (hard) | MUST-M21 |
| M22 [requirements-1, build-6, B, E] | Operations at scale | The corpus is counted in milestone 1, not assumed: the evaluation store, the live store, the other machines' claude-mem databases if imported, and one year of raw chunks at the measured rate. Under concurrent worker writes: MCP p95 ≤ 1.5 s; slowdown under writes ≤ 20%; forget time. Injection hook p95 stays within the read-hook line on both paths: the warm path (the shortlist), and the cold path (full text before the worker is up, RD/sections-1-4.md:40; trigram OR measured p95 767 ms at the 178k store, docs/pr-e0.md:95). If the cold path misses, S4's pruning is tried first (RD/improvements-synthesis.md:445). If that also misses, the cold path injects the packet's ranked claims, the other option section 4 already gives (my decision). First sync of a blank device, over the hub only (decision 12), on Free and on Paid (RD/section-5-revised.md:17). Manifest truncation per agent | MUST-M22 |
| Isolation [requirements-3] | Curator, judge and digest CLIs | A per-CLI capability test with three canaries passes when no file is created, the local listener sees no request, and the canary never appears in the output. Where a CLI reports its tool list, it is checked on every call. A CLI without a proven no-tool mode is skipped for these roles, with a doctor line. Results go in a per-CLI table like M10. Reviewed under rules/security.md | RD/section-6-revised.md:96-99, :114-119 |
| Transcript [requirements-5] | Transcript import | A canary forgotten before the import is absent after it (M4, hard). 100% of imported records carry `source = transcript`. At most one turn per session appears twice, and both copies are labelled by source. A session the old `DELETE` trimmed gets its head back. One fixture per parser | RD/section-7-revised.md:145-157 |
| Inject | Per-prompt injection threshold | On by default only if the one-sided 95% upper bound of irrelevant injections is ≤ 10% | proposal decision 12 (proposal:36), section 4 |
| Judge | Judge-model roles | Selection: curator input −30% with recall drop ≤ 0.02. Veto: fewer wrong `decided`, with recall kept | section 3 |
| Window | Curation window size | The smallest size that passes M2, M3 and M6, swept on dev transcripts only (RD/constraints-synthesis.md:266) | section 3 |
| Rerank [validity-2] | S3 reranker | The M1 line, inside milestone 4's single test run. Also CPU p95 and RSS on the M1 iMac and the slowest WSL machine (RD/improvements-synthesis.md:441) | S3 |
| Public [F] | JQaRA and JaCWIR sanity check | On each set, the default is at most 0.02 nDCG@10 below the better of its two legs run alone (FTS only, bge-m3 only). A sanity line, not a tuning set (RD/constraints-synthesis.md:377) | section 4 |
| Cost | Per heavy day | Curator calls ≤ 20% of each daily cap. Paid ≤ USD 5/month | options-draft §9 |
| Install | Fresh machines | Install, set up two agents, and recall a memory in a new session within 10 min, with a clean doctor, on all 5 targets (2 in CI only) | MUST-M23, section 7 |
| Final [validity-8] | The release run | M1, M3, M5 and M6 run once on the unseen final set (§1) | RD/issue50.md:194 |

The 1.5 s MCP line is kept as written. It does not come from the old hook budget: it is proposal:256's per-path line for MCP search. The measured hybrid already meets it: p95 1,029 ms on this PC at the 178k store, and about 1.1 s estimated on the M1 iMac (docs/pr-d.md:112).

### 3. Spikes (throwaway, with docs/ notes)

- **Curator spike** (the biggest risk, RD/options-draft.md:210) [requirements-2, build-3]. Two parts:
  - **Isolation.** Needs no labels, so it starts at once. It runs the Isolation row's capability test on each CLI:
    - claude keeps its current flags (RD/section-6-revised.md:100).
    - grok adds `--disable-web-search` and `--no-subagents`; if the canary still fails, `--sandbox` is next (:102-103).
    - codex needs either a mode with no shell tool or a sandbox that hides the home directory. The canary also checks that its read-only sandbox blocks network access (:105-106).
    - agy: the spike tests a custom `--agent` with no tools (:109). Copying agy's login token into a private config directory touches a credential, so it is tried only with a rules/security.md review (:94).
  - **Gate quality.** Waits for milestone 1's dev labels. It measures M3 gate precision and recall on Japanese dev sessions, and sweeps the window size on dev transcripts. It shows direction only: M3's pass line is decided at milestone 3, on the held-out transcripts.
- **Donor self-host spike** [build-4]:
  - This is the precondition for the hub build units (MUST-M19, RD/improvements-synthesis.md:333; RD/critique.md:31, :71).
  - The question: can we fork claude-mem's sync code, add purge, and run it without cmem.ai (RD/section-5-revised.md:94)?
  - Pass: a fork with no built-in cmem.ai URL completes a push, a paged pull and the purge of a canary op in the owner's account, and makes no request to cmem.ai.
  - It must pass before milestone 6.
- **Hub platform spike** (RD/hub-platform.md §4) [build-2]:
  - The platform is settled (decision 18: Cloudflare). So the two conflicting statements of which items "decide the platform" (RD/hub-platform.md:106 against :148) no longer matter.
  - Every item is a pass/fail line inside Cloudflare. Three of them change the build:
    - Item 1 (deploy without Node): if it fails, public users get another hub host; the owner's hub stays on Cloudflare (RD/section-5-revised.md:96).
    - Item 3 (trigram full-text in the DO, including 2-character queries): picks the fallback for short queries.
    - Item 4 (semantic search in the DO): if it passes, Vectorize is dropped. Remote semantic search then works on the Free plan, and M4 has one store fewer to grep (RD/hub-platform.md:45, :133).
  - Items 2 and 5-8 are checks with their own pass lines.
- **Hook spike** (M14): raw.db writes under `synchronous=FULL`, with 64 KB and 256 KB outputs and a full redaction scan, on WSL, Windows native and the M1 iMac. It sets the write-hook line (M14); the read-hook line is set later, at milestone 4 [A].

### 4. Build order (milestones; each has a docs/ note and PRs)

Milestone 1 comes first. The spikes that need no labels run alongside it: isolation, hook, hub platform and donor self-host. The curator spike's gate-quality part waits for milestone 1's dev labels [build-3].

1. **Freeze and label** (issue50 §8 row 1, RD/issue50.md:159) [build-3, validity-1, validity-5, validity-8, B, D]:
   - B3 first: the judge calibration gate in §1.
   - Freeze:
     - the 424 questions and their splits;
     - the replay set: events-1000.jsonl, 30 held-out transcripts stratified by length, language and agent, and the 24-hour session if its transcript exists (RD/options-draft.md:329);
     - a separate set of dev transcripts for tuning curation (RD/constraints-synthesis.md:266);
     - the final set's selection rule and seed.
   - Transcript parsers move here from milestone 7 [D]:
     - The replay set and the Raw variants need raw events from agent transcripts. oboete deleted its own raw events after summarising, until PR #52 (RD/options-draft.md:309; decision 20).
     - Claude Code's JSONL needs a new parser. The hooks already read agy, Codex and Cursor transcript tails (RD/section-7-revised.md:150).
   - Labels: dev labels first, because the curator spike needs them; test labels before milestone 3's deciding run (owner question 2).
   - Failure fixtures: the 24-hour session, a decision that appears only in the middle, overturned pairs, deletion canaries.
   - Baseline runs of the current oboete and claude-mem.
   - Count M22's corpus [B].
2. **Record** (sections 1-2):
   - Build: raw.db keyed by (device, seq); full redaction; `synchronous=FULL`; write-failure reporting; backups; tombstones in raw.db from day one (deletion and sync are designed in from the start, issue50 §8 row 7); raw FTS and the deterministic manifest.
   - The none tier works end to end: record, search, and resume from the manifest.
   - Lines: the write-hook line (M14).
   - MUST fixtures: MUST-M14 (FULL, checkpoint rewind), MUST-M15, MUST-M16, MUST-M5 (none-tier negation in the manifest).
3. **Curate** (section 3):
   - Build: windows and checkpoints; claims with speaker, status, evidence and supersedes; code gates; digests; the provider chain with curator isolation; `rebuild` and `recurate`.
   - Lines: M2, M3, Isolation, Window, Judge [requirements-3].
   - MUST fixtures: MUST-M1, M2, M3, M4, M6, M7 (tie order), M18, M21 (per kind; corrections survive rebuild and re-derivation).
4. **Deliver** (section 4) [build-1, build-6, validity-2]:
   - Build:
     - packets and shortlists; SessionStart and per-prompt injection; compaction; current-first search on MCP, CLI and viewer;
     - the S3 reranker (RD/sections-1-4.md:48);
     - B's claude-mem import into an evaluation home, so M1 runs on B's code [I]. This is B1's `--eval-store` path (docs/pr-b.md:21), ported to B's schema. The import into the owner's store stays at milestone 7;
     - the sync client, docs/hub-protocol.md and its fake hub (RD/section-5-revised.md:95; the client tests need the fake hub anyway, RD/hub-platform.md:56). This settles the worker question before milestones 5-7 build on it.
   - The one test-split run: M1, Raw and Rerank together.
   - M5's one-device lines, on the held-out transcripts.
   - The worker/no-worker comparison, on dev transcripts over the fake hub:
     - Its latency and propagation lines need no labels.
     - The fake hub has no network hop, but it still measures polling and curation timing, which make up most of the 5 minutes.
     - The consumers are the same code in either process model. So if milestone 6 reverses the verdict, only the lifecycle wrapper changes, not the pipeline.
   - Then: the read-hook line, M6, Inject, M21 English, M22 search and injection at scale, M22 manifest truncation.
   - MUST fixtures: MUST-M8, M9, M11, M12, M13.
5. **Forget and safety** (section 6):
   - Build: the four levels, the forget pipeline and `forget_jobs`, the env allow-list, viewer writes.
   - Lines: M4 (local), M22 forget time at scale.
   - MUST fixtures: MUST-M14 (purge).
6. **Hub** (section 5) [requirements-6, build-4]. Starts after the donor self-host spike passes and the hub platform spike has run (a failed item 1 changes what milestone 7 ships for public users, not the owner's hub).
   - Build: the hub from the donor fork with purge, auth, and remote MCP with OAuth and grants.
   - Lines:
     - M4 (devices, hub, R2, and Vectorize if kept);
     - M5's device lines over the real hub. This is the deciding run for the worker rule and the WebSocket wake;
     - M21: corrections survive resync;
     - M22: first sync.
   - MUST fixtures:
     - MUST-M7: clock skew across devices;
     - MUST-M17: ops from a newer version are parked and applied after the upgrade;
     - MUST-M19: sync refuses to start with no hub URL; a hand-made supersedes cycle is flagged; in the delete/supersede race, X is never current and Y renders; dropped wake messages change nothing; the daily probe agrees ≥ 0.95 on top-3 results and status.
7. **Ship** (section 7) [requirements-4, requirements-5]:
   - Build: installer; `oboete update` (MUST-M17: after a failed self-check the old binary still runs, and nothing is lost compared with the backup); v1 and claude-mem import; transcript import (the Transcript row); doctor with the M10 table; docs; NOTICE.
   - Lines: Install on 5 targets, Transcript.
   - MUST fixtures: MUST-M17, MUST-M23.
8. **Finish**: M10 live checks for all 7 agents, Public, the final run on the unseen set, and the release gate (section 7, which reads this section).

The owner's machines switch from the current oboete after milestone 4, when recording, curation and delivery work on one device:
- The dogfood user switches first, then the owner's machines in section 7's order (RD/section-7-revised.md §5, :159).
- The owner's hub joins after milestone 6.
- Nothing is lost before that: milestone 4's sync client talks only to the fake hub, and the current oboete has no sync either (owner question 1).

Who builds [build-7]:
- **Claude Code** writes:
  - the core: raw store, curation, gates, delivery, forget;
  - every security-scope part: redaction, the egress gate, curator isolation, hub tokens and device enrollment, OAuth and approval codes, grants and purge (RD/section-6-revised.md:133; rules/security.md).
- **Codex or Grok** take independent pieces in parallel (project CLAUDE.md):
  - agent adapter ports;
  - transcript parsers;
  - the donor modules that have no auth in them (op envelope, push and ack, paged pull, caps), written against docs/hub-protocol.md and tested on the fake hub.
- Each PR gets one Codex review lane.

Size [requirements-7, build-5]:
- The MUST list took B to 34-41 PR-sized units (RD/improvements-synthesis.md:12).
- Postponing the folder transport (decision 12) removes:
  - MUST-M20 (about 1 unit, :14);
  - the folder half of "hub port + folder transport: 3" (RD/options-draft.md:204; about 0.5-1, my estimate).
- The draft added five items on top. Two of them were already priced:
  - the update steps are MUST-M17 (about 1.5, RD/improvements-synthesis.md:281-297);
  - most of the forget jobs widen the "deletion, deny-list and compaction" unit (RD/options-draft.md:203).
- These are new:
  - curator isolation, a new MUST-level item (RD/section-6-revised.md:99): about 1;
  - transcript import, new scope (RD/section-7-revised.md:145): about 1-1.5;
  - the protocol doc and fake hub (RD/section-5-revised.md:95): about 0.5;
  - the rest of the forget jobs: about 0.5.
- Total: about 35-43 units (estimates are mine). This section adds labels, which cost owner time, not build units.

## 2. What changed and why

- **inherited-1 + H:**
  - M1 now runs on the whole 178,502-document evaluation store that D2 used. B has no per-document LLM ingest to budget for: embedding the whole store cost about USD 0.80 (docs/pr-d.md:14).
  - Hindsight's cost-bound sub-corpus and the Hindsight and Cognee arms are removed (RD/options-draft.md:320-321).
  - The baseline is re-scored in the same pool, because 0.545 depends on the pool (docs/pr-d.md:25).
- **inherited-4:** a new Raw row restores options-draft's +0.02 "individual improvement" line for the three raw-chunk variants (RD/options-draft.md:327). That line is how B's raw-chunk handling gets decided (:126).
- **inherited-5 + validity-5 + build-3 (owner-facing):**
  - Owner question 2 now gives an estimate per phase and a fallback. It includes the labels the improvement sweep added: MUST-M3's cross-session pairs and M21's per-kind labels.
  - §1 now says labels are split by session hash, and tuning and calibration labels come from dev only.
- **requirements-1 (adopted in modified form):** M22 measures injection latency at scale again. The target is the read-hook line, not the 300 ms in RD/improvements-synthesis.md:381. 300 ms is the old budget, which the settled section 4 dropped (RD/sections-1-4.md:41, decision 17; RD/constraints-synthesis.md:314).
- **requirements-2:** the curator spike tests agy with a custom `--agent` that has no tools (RD/section-6-revised.md:109). Copying agy's login token into a private config is a separate credential path, tried only with a security review (:94).
- **requirements-3:** new Isolation row, with section 6's canary pass line and per-CLI table, gated at milestone 3.
- **requirements-4:** MUST-M17's two lines are named at milestone 6 (version parking) and milestone 7 (rollback). §1 also states the general rule: every MUST item's own measures are acceptance tests of the milestone that builds it.
- **requirements-5:** new Transcript row, with section 7's three rules (deny-list, source label, the bound on duplicates). M4 gains the transcript re-import case.
- **requirements-6:** milestone 6 names MUST-M19's lines: URL refusal, the cycle check, the delete/supersede race, dropped wake messages and the daily probe.
- **requirements-7 + build-5:** the size is recounted to about 35-43 units.
  - Out: M20 and the folder half of the donor unit (decision 12).
  - Already priced: the update steps (M17) and most of the forget jobs.
  - New: isolation, transcript import, the protocol doc with its fake hub, and the rest of the forget jobs.
- **validity-1:** B3 calibration is milestone 1's first gate, and the trust condition is written out (proposal:240). D2's test result and the default it set are provisional, because the judge scored them before the condition was measured (docs/pr-d.md:173).
- **validity-2 (merges refuted inherited-3):** a budget for the test split.
  - It was used once for D2. It is used once more at milestone 4, with every retrieval candidate in one Holm-corrected run.
  - After that it decides nothing. Section 7's cut-over check does not count as a use.
- **validity-3:**
  - M21's English test slice has N ≥ 60: the 7 existing questions plus at least 53 new ones. The 40 dev questions are kept for tuning.
  - The error rates are computed from the per-question spread measured on D2's runs.
- **validity-4:** M3's decided slice needs N ≥ 100 for precision and ≥ 100 for recall. At that size, passing at 0.95 bounds true precision at ≥ 0.90 (one-sided 95%, Wilson).
- **validity-6 (cheap form):** the owner labels a blind 20 of the 50 calibration pairs a second time. There is no second labeller, because the tool has one owner (RD/constraints-synthesis.md:101).
- **validity-8:** the unseen final set is now defined:
  - drawn from sessions after the freeze, so it shares no time or session with earlier sets;
  - the same size and strata as the test split;
  - sealed until milestone 8;
  - used up if the final run fails.
- **build-1:**
  - The sync client, the protocol doc and the fake hub move to milestone 4, so the worker comparison runs there, on dev transcripts.
  - Milestone 6's run over the real hub decides the worker rule. It can still reverse the verdict, which would change only the lifecycle wrapper.
- **build-2:** the platform is settled (decision 18). So the spike items are listed by what each one changes in the build (items 1, 3 and 4), not by the two conflicting statements in RD/hub-platform.md:106 and :148.
- **build-4:** the donor self-host spike is now a named gate with a pass line, before milestone 6 (MUST-M19 precondition; RD/section-5-revised.md:94).
- **build-6:** every part of M22 now has a milestone:
  - the corpus count at 1;
  - search and injection at scale, and manifest truncation, at 4;
  - forget time at 5;
  - first sync at 6.
- **build-7:** Claude Code keeps purge and the auth parts of the donor port: tokens, enrollment, OAuth, approval codes, grants. Only the donor modules with no auth in them are delegated.
- **decision 12:** M22's first sync is measured over the hub only. The folder half of RD/improvements-synthesis.md:383 is dropped.
- **A (the inherited 300 ms):**
  - Five places still carried the old 300 ms hook budget: M5's worker rule (RD/options-draft.md:340), MUST-M9 (:149), MUST-M22 (:381), S2 (:434) and S4's trigger (:445).
  - §1 now reads all five as a new read-hook line. It is set by measurement at milestone 4, like M14's write line, because M14 times writes, not reads (new Read hook row).
- **B (the inherited corpus size):**
  - The "~330k documents" figure comes from RD/options-draft.md:100. It is reused by MUST-M22 and by the forget-time lines (RD/improvements-synthesis.md:247, :379).
  - It adds "the existing ~180k documents" to "~150k claude-mem observations". But the ~180k store is itself the claude-mem import: 178,370 rows, of which 152,030 are observations (docs/pr-b.md:58).
  - The owner's live store holds a few thousand documents at most (docs/pr-e0.md:97). A read-only count of ~/.oboete/oboete.db on 2026-09-25 gave 141 observations, 17 summaries and 15 prompts.
  - So the figure appears to count claude-mem twice. M22 now counts the corpus instead, including a year of B's own raw chunks, which may be larger than either figure.
- **C:** section 7's release gate cited options-draft §9 in full (RD/section-7-revised.md:240). §9 still holds Hindsight's corpus rule, A's decision rule and the 300 ms lines. §1 now states what this section replaces.
- **D:** transcript parsers move to milestone 1. The replay set and the Raw variants need raw events from transcripts, and oboete deleted its own raw events until PR #52 (RD/options-draft.md:309, :329).
- **E:**
  - M22 also times the cold injection path: full text before the worker is up (RD/sections-1-4.md:40).
  - Full-text search alone measured p95 767 ms at the 178k store (docs/pr-e0.md:95). So the cold path, not the shortlist, is where the read-hook line is at risk.
  - If S4's pruning does not bring the cold path inside the line, it injects the packet's ranked claims instead. Section 4 offers both options; I made this choice.
- **F:** the Public row had no number ("does not fall apart"). It now has a line fixed before the run: at most 0.02 below the better of its own two legs.
- **G:**
  - M4's hard 0-hit line now leaves out the limits that sections 5-7 list and doctor names: the DO's 30-day history, migration snapshots and evaluation copies. Without this, the line would fail by design.
  - M4 also gains section 6's in-flight, rescan and retention cases.
- **I:** B's claude-mem import into an evaluation home is built at milestone 4, before the test run. M1 has to run B's code on B's schema. The only evaluation store today was built by the current import, into today's schema (docs/pr-b.md:15, :21).
- **J:**
  - The Raw row's corpus may not exist in full. Claude Code transcripts survive only from 2026-06-19, and Codex transcripts from 2026-08; the store's prompts start on 2025-12-14.
  - So Raw is scored only on the questions inside that window, with N reported. If that N cannot show +0.02, section 4's default stays.
- **MCP 1.5 s kept:** it is proposal:256's MCP line, not the hook budget, and the measured hybrid already meets it (docs/pr-d.md:112).

Refuted findings, with a note where useful:
- **inherited-2:** there is no contradiction. M14's measured line is settled (RD/sections-1-4.md:20), and M2's fail-first test is TDD, not a remeasure. §1's "three kinds of line" states the scope in one bullet anyway.
- **inherited-3:** the same point as validity-2, which survived. The test-split budget handles it.
- **inherited-6:** the Public row stays Japanese-only on purpose (RD/constraints-synthesis.md:374-377, settled in sections 1-4). English is M21's job, and M21 now has a stated N and error rates.
- **validity-7:** no change. M2 and M4 are deterministic, not scored by the judge, and M6 already carries a +0.10 margin.

I decided these myself, without asking:
- the test-split budget and the Holm correction;
- the final set's definition;
- every N above, and where the English questions come from;
- the Public line;
- counting M22's corpus instead of assuming it;
- the number of dev pairs for M3;
- the milestone moves: parsers, the evaluation-store import, the sync client, the fake hub, the reranker;
- the read-hook line and the cold-path fallback;
- the Raw window rule;
- the split of the donor port between Claude Code and Codex/Grok.

## 3. Questions only the owner can decide

1. **Switch the owner's machines after milestone 4, or only when everything is finished?**
   - Recommendation: after milestone 4. By then recording, curation and delivery have passed M1, M2, M3, M5 (one device) and M6. The current oboete has no sync to lose. Section 7's order (dogfood user first) and its rollback (the old binary is kept) limit the risk (RD/section-7-revised.md §5, :159).
2. **How much time can the owner give to labelling?** Claude drafts every candidate, so the owner only confirms, rejects or grades. My estimate:
   - Before the curator spike, about 4-6 h:
     - B3's 50 calibration grades plus 20 blind repeats (2-3 min each);
     - 20 dev overturned pairs with 20 control pairs;
     - about 50 dev decisions.
   - Before milestone 3's deciding run, about 5-7 h:
     - 50 test overturned pairs with 50 control pairs (at least 20 across sessions);
     - 100 real decisions in the held-out transcripts;
     - yes/no on about 100 no-answer and 100 false-premise questions (RD/sections-1-4.md:44).
   - During milestones 3-4, about 4-6 h:
     - 100 claims the curator marked `decided`;
     - about 100 per-kind labels (MUST-M21);
     - M5's 20% spot-check;
     - answer keys for M6's 40 questions.
   - Total: about 13-19 h, in sittings of an hour or less.
   - Recommendation: give the full set. If less time is available:
     - Keep human labels for B3 and the hard M3 lines.
     - Let Claude label the precision and per-kind sets, with a 20% owner spot-check, as M5 already does. This saves about 3 h.
     - Any line without its labels stays unmet, and the release waits for it.