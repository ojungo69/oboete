# Milestone 4 (Deliver) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Depth, as in milestone 3's plan:** every decision and every task's files and interfaces are here. Step bodies are written for Tasks 0-4, which settle the delivered set, where imported history lives and the search core that the rest builds on. Tasks 5-12 get their step bodies in a follow-up PR once Tasks 0-4 have merged, so that they are written against code that exists.

**Goal:** What milestone 3's curator makes reaches the agent the way claude-mem's memory does (owner decision 31): at SessionStart, after compaction, and, once its line passes, at each prompt; and the past can be searched on MCP, CLI and the viewer, over claims, imported history and raw records. The evaluation store moves to Design B's schema, so that the single test-split run measures B's code. The tools that the owner's cut-over runs are built here (spec 7.5).

**Architecture:** Hooks only read (spec 4.1). The worker keeps the manifest per checkout, a shortlist per (session, checkout), the full-text indexes and the vectors. The hook reads the manifest row and the delivered claims, and it never waits on the network. "Delivered" (spec 3.4) is one query beside the chain tips, which curation keeps reading unchanged, and one function picks from it for every surface. Imported history (claude-mem, v1's documents) is a new op type in raw.db, so it is backed up now and syncs later like other content ops (spec 5.4). A new consumer indexes it in knowledge.db. Search is one entry point that CLI, MCP and the viewer call. Vectors are made by a consumer that calls the embedder outside every database transaction.

**Tech Stack:** Rust (edition 2024), rusqlite (bundled SQLite, FTS5 trigram), sqlite-vec 0.1.9 (already a dependency), ureq for Workers AI, no async runtime.

**Spec:** `docs/spec.md` 3.4, section 4 (all), 5.4 (imported documents), 5.5 (a query that leaves the machine), 6.5, 6.6, 7.1 (local embeddings), 7.4, 7.5, 8.1, 8.2 (M1, Raw, Rerank, M5, M6, Inject, M21, M22, Read hook, Transcript), 8.4 row 4, Appendix B rows 30-1, 30-2, 30-14, 30-16, 30-18, 30-22, 46-1 to 46-3, 50c-a and 50c-b, 53-1 to 53-6, 54-6, 55-1, 55-5 to 55-8; MUST-M8, M9, M11, M12, M13 (docs/research/redesign-2026-09-24/improvements-synthesis.md:126-220); issue #295 rows 1-3; owner decisions 31 and 32. What milestone 3 left: `docs/milestone-3.md`.

## Global Constraints

- Hooks never wait on the network and never embed a query. No reranker or judge runs in a hook (spec 4.1, 4.3).
- Only delivered claims (spec 3.4) are injected, never prompt text. Every injection is fenced as data (spec 4.4, 4.6, 6.5; row 30-18).
- Imported memories are for search and timeline only, never injected and never current (spec 4.5).
- The chain tips (`claims::TIPS`) stay the store's current state. Curation, digests and `anchored_through` keep reading them unchanged (spec 3.4).
- The new version never writes the old store: `oboete.db` is opened read-only, never through `db::open` (spec 7.4).
- Every reader opens raw.db before knowledge.db, so that a restore or rebuild swap waits for it (as `search::raw` does today).
- Text sent to a remote embedder passes the egress gate and the exclusion check first (spec 5.5; rows 30-1, 30-2).
- The test split is used once, for every retrieval candidate in one run (spec 8.1). Held-out transcripts decide once. The deciding runs that read curated claims wait for milestone 3's deciding run (spec 8.4, A88).
- Per-prompt injection ships off by default until the Inject line passes (spec 4.6). The user may turn it on earlier (spec 1.5).
- Embedding spend has its own cap, outside the paid APIs' USD 5 a month (owner decision of 2026-09-27).
- The owner's machines keep running v1 until the cut-over after milestone 5. Design B runs only in the `oboete-dogfood` user and in temporary and evaluation homes (CLAUDE.md).
- Claude Code writes the core and every security-scope part: the delivered rule, the search core, the viewer's routes, the embedder's egress path, `migrate` (spec 8.4 "Who builds").
- CI: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` on Linux, macOS and Windows.

## Review Focus

1. **A pair cut by the limit.** SessionStart shows about 10 claims with bodies, ranked by relation to the manifest and by recency. An earlier decision ranks 10th, and the later decision whose curator link ended it ranks 11th. Expected: the later one moves up to sit directly before it, both dated, or both go to the one-line index. The earlier decision never stands alone (#295 row 2). Test in Task 1.
2. **178,502 imported documents.** `oboete import claude-mem` fills an evaluation home. Expected: the worker indexes them in bounded batches; curation never sends one to a provider; SessionStart never shows one; search finds them labelled `imported`. A second run appends nothing, and a run killed halfway resumes without duplicates. Test in Task 3.
3. **v1 migrated while hooks keep writing.** Expected: `oboete migrate` imports only v1 events it has not imported before. Imported events are parked, not curated, until `oboete recurate --source oboete-v1`. The manifest's last prompt and failing command stay the live session's. Tests in Tasks 2 and 9.
4. **A slow or refusing embedder.** Workers AI answers 429 or times out for an hour. Expected: hooks keep writing, curation keeps curating, search keeps answering from full text, doctor shows the wait, and no vector is published for a text that changed meanwhile (rows 55-1, 55-5, 55-6, 55-7). Nothing leaves the machine for an excluded repository (row 30-1). Test in Task 5.
5. **A query that names another repository.** An MCP `search` call passes `repo` for a repository other than the caller's. Expected: the query is checked against the exclusions of the caller's repository and of every repository searched before it can reach a remote embedder (row 30-2). A superseded decision is never ranked as current; with `history=true` it is shown with `superseded by <uid>` (MUST-M11). Test in Task 4.
6. **A read during a restore.** The viewer and MCP read while `oboete rebuild` swaps knowledge.db. Expected: each read opens raw.db first and waits, and none reads a half-swapped file. Test in Task 7.

---

## Decisions

Each decision is Claude's unless marked otherwise, and the owner can overrule it. They settle what the spec leaves to milestone 4 or leaves open.

**D1. "Delivered" is one query beside the chain tips.** A new SQL constant `DELIVERED` in `src/claims.rs` returns the tips plus each claim `a` of kind decision or preference, active and decided, whose every inbound edge meets three conditions. The edge is a curator `supersedes` link. Its linker is active, not retracted, decided or done, and backed by the owner the way `anchored_through` already defines it (the user's own words, a proposal the user accepted, or a claim the owner corrected). The linker's `valid_from` is later than `a`'s. `TIPS` does not change. A retraction, an accepted proposal, a closed open item and an owner correction still end the older claim (spec 3.4).

The restatement links that recuration adds (`curate::restate`, #261) are not curator links. `restate` writes them into a new op field `restates`, and the claims consumer stores them as edges of type `restates`. They end a claim, as today, and never make one delivered. Ops written before this change only have `supersedes`. They exist only in dev and evaluation homes, which are curated again for every arm.

**D2. One function picks for every surface.** `claims::deliver(k, repo, kinds, limit)` walks the delivered claims in their ranked order. When it picks an earlier claim, it also picks the claim that ended it and puts that one directly before it. The pair takes two places of the limit. If only one place is left, both go to the one-line index. Each claim is shown with its date. A constant `LINKS_END_DECISIONS` (false) is the switch that spec 3.4 waits for: once curator links pass M3's control line, the PR that records that result sets it, and the earlier claim then leaves delivery as it does from the tips (#295 row 1). It is a constant, not a setting, because it follows a measurement, not a preference. Every surface uses `DELIVERED`: SessionStart, the manifest, the shortlist, per-prompt injection, re-injection after compaction and search's first rank. Every surface that shows a limited number of claims picks them with `deliver` (#295 rows 2-3).

**D3. The SessionStart packet's manifest part is stored; its delivered part is read (amends spec 4.1).** The worker keeps the manifest row per checkout, as it does today. The hook reads it, and it reads the delivered claims, the global preferences and the fresh digest from knowledge.db's indexes at SessionStart. So a claim retracted since the last worker run is never shown, and no packet rebuild is needed after each op (`consumer/manifest.rs` explains why decisions are read at SessionStart today). A stored packet replaces the live read only if M22's scale check shows that the live read pushes SessionStart past the read-hook line. This PR changes spec 4.1's first bullet to say so, with Appendix A row A89. The per-prompt shortlist stays a worker product (D9), because its first stage needs vectors, which a hook never computes.

**D4. SessionStart's content, in spec 4.4's order.** It shows the explicit global preferences (`claims::global`); the manifest; delivered decisions, preferences, open items and lessons (kinds of spec 4.4; fixes, changes and repo facts are for search), about 10 with bodies, ranked by overlap with the manifest's files and last prompt and then by recency, the rest as one line each (`uid`, kind, date, first words); a line naming the `get`, `search` and `timeline` tools; the digest, only if fresh. A checkout with claims but no manifest row (a new branch, a dirty manifest, a tombstone not yet applied) still gets its preferences and delivered claims. Today it gets nothing.

**D5. Imported documents are `import` ops in raw.db.** Spec 5.4 says imported documents travel like other content ops, and raw.db's ops are kept, backed up and restored. A knowledge.db table alone would be lost on `rebuild`. So each document (a claude-mem observation, summary or prompt, or a v1 observation, summary or prompt) becomes one op of type `import`. Its body is `{uid, source, source_id, kind, repo, session, ts, title, body}`, at most 64 KB (`MAX_OP_BYTES`), with a longer body clipped with a marker. The source name is derived as `import.rs` derives it today, so that a document keeps its uid (`claude-mem:<hash>:o5`) and the 112-question judgments still map to it. The importer appends in bounded batches, each its own `batch`, and it reads the keys already imported before it appends, so a rerun appends nothing twice. A new op-log consumer, `Imported`, fills knowledge.db's `imported` table and its trigram `imported_fts`. Each row is keyed by the op that brought it, (device, op_seq), with the uid indexed, as claims are: a restore's rewind then removes exactly the rows of the lost ops, and a document that two devices imported stays once per op, shown once per uid. Curation reads records, not import ops, so it never sees them, and neither does injection.

`OpKind` gains `Import`. `Raw::ops_after` refuses a type it does not know (`src/raw.rs`), so an older binary stops at the first import op. That is MUST-M17's version skew, parked at milestone 6. On one device, every binary that writes import ops also reads them.

**D6. Imported events are records, parked from curation.** v1's events (`source = oboete-v1`) and transcript events (`source = transcript`) become records on the local device with new seq numbers, as spec 7.4 says. Curation reads records by seq, and today it has no source filter. So when the curation phase reaches records of those sources, it writes a window op skipped with reason `imported:<source>` and sends nothing. The spans stay counted, which M2 needs (every seq is curated, elided or skipped with a reason). No window mixes live and imported records. `oboete recurate --source <source>` curates the parked spans, with a cost estimate first. For `oboete-v1`, the estimate lists first the sessions whose v1 render passed 16,000 characters (spec 7.4). The manifest's facts (last prompt and reply, failing command, files touched, other active sessions) come only from live sources, `hook` and `replay`: `replay` stands for live in the dev and evaluation homes. The not-yet-curated count leaves parked records out (spec 7.4, MUST-M9), and doctor counts them apart. The v1 import's checkpoint (the highest v1 event id imported, per v1 device) is an op appended in the same raw.db transaction as each batch of records, so that a backup carries it with them.

**D7. One search entry point.** `search::query(home, &Query)` serves the CLI, MCP and the viewer. Delivered claims come first (D1, #295 row 3). Then imported documents and raw records are fused by RRF over the top 100 of each side (row 46-1). Raw records rank below curated rows, the default that spec 8.2's Raw row keeps if its measurement cannot decide. Superseded claims are shown only with `history=true`, labelled `superseded by <uid>` (MUST-M11). `since` and `until` filter each leg before fusion, each document by its own time (MUST-M12). Every hit carries one label, `citable`, `quote-only` or `imported`, and a repository label (MUST-M13). MCP replies are fenced as data. A query that could reach a remote embedder is first checked against the exclusions of the caller's repository and of every repository searched (row 30-2; the list is D13's).

v1's readers leave main as their Design B replacements land: MCP's and the CLI's v1 halves in Task 4; the viewer's v1 routes and its two DELETE routes, `inject.rs`, v1 search and v1 embedding in Task 7. The owner runs the v1 branch's binary until the cut-over, and the cut-over's old-code search check runs that binary (spec 7.5 step 3), so main needs none of them.

**D8. The embedding consumer.** It is an op-log consumer over claims and imported documents that writes vectors to knowledge.db, keyed by (claim uid or imported uid, embedder id, generation), with the SHA-256 of the text it embedded. A step reads its batch, calls the embedder with no transaction open, and then writes the vectors in one transaction, skipping any key whose text hash changed meanwhile (row 55-6). A failed call stops only this consumer until its retry time: the other consumers and curation go on (rows 55-1, 55-5, 55-7). Changing the embedder builds a new generation in the background, and search switches to it once it covers every current document (row 30-16; the remote index's drain comes with it at milestone 6). The first embedder is Workers AI's bge-m3, the model v1 uses, moved to knowledge.db keys. Its calls are recorded in providers.db and counted against the embedding cap (What needs the owner, item 1).

A local runner of bge-m3 (spec 7.1) is built after a spike on its runtime (Task 10). It is accepted as the same model when its vectors agree with Workers AI's on a sample (cosine agreement, the threshold stated before the sample is run), so it is a deployment, not a retrieval candidate. The single test-split run uses Workers AI (D10).

**D9. Per-prompt injection and the shortlist.** A worker consumer keeps about 50 delivered claims per (session, checkout), from the hybrid search over the session's last prompts and the manifest's files. It rebuilds them at each Stop, every N events, and when the session moves to another checkout (spec 4.1). N is 20 until the dev harness for Inject shows a better value; the rebuild on sync comes with milestone 6. At each prompt the hook matches the prompt's text against those claims with full text, keeps the hits over a threshold, and checks that each one is still delivered before it injects. The threshold is tuned on dev questions and decided by the Inject line, whose run waits for milestone 3's deciding run (A88). Until then `[inject] per_prompt` is off by default. `[inject]` also gains per-prompt and correction sizes and switches (spec 1.5), and the settings page shows them. A correction is delivered at the next prompt (spec 4.8; Grok by a flag and its next tool use). Grok, agy and OpenCode re-inject after compaction, as Claude Code, Codex, Pi and Cursor already do, and a static per-agent table records each point as live-verified, implemented or unverified (spec 4.7).

**D10. The single test-split run.** Every candidate is run before any is judged: B's hybrid over the imported evaluation store, the three Raw variants, the reranked hybrid, and the English slice (the 53 frozen questions and the 7 in the split, 8.2 M21), at full-text depth 100, pooled at 50 (row 46-3) and judged by the judge that passed calibration. p-values are Holm-corrected across the candidates (spec 8.1). B's hits are mapped to the judged document ids through the v1 evaluation store's `imports` table, read-only. The run reads no curated claim, so it does not wait for milestone 3 (A88).

- **Raw.** The raw records come from replaying the transcripts of the test questions' sessions into the evaluation home (docs/eval/m3.py's pattern), not from the cut-over's transcript import. Raw is scored on the test questions typed inside the transcript window, about 29 (spec 8.2 Raw row). If +0.02 cannot reach p < 0.05 at that N, the default stays: raw below curated rows.
- **Rerank.** A script reranks the hybrid's top 50 into a run of its own. The reranker is built into the binary only if that run clears the M1 line (Task 11). The same script times it for the CPU and RSS lines.

**D11. The viewer reads Design B.** Its routes read raw.db and knowledge.db through the same guards as today (127.0.0.1, per-run token, Host check, bounds; rows 53-1 to 53-4). It serves search (D7), a claim with its evidence quotes and its links, the earlier and the later claim of a pair, a claim's history, a timeline of claims and imported documents by time, the SessionStart text of a checkout as its Context page, and stats over raw.db, knowledge.db and providers.db. Every text it serves passes the outbound gate and is shown as text only. The settings page (#94) stays as built. Viewer writes (forget) come with milestone 5.

**D12. The M6 harness is milestone 3's debt.** Spec 8.2's Window row runs M6 on dev at milestone 3, and no M6 harness exists (no questions, no answer keys, no scorer). Task 12 builds it on dev: 40 lookup questions, the panel's answer keys, and the cited-span check, which reads the evidence offsets already stored. Its deciding run on held-out transcripts waits for milestone 3's deciding run (A88). docs/milestone-3.md's "What the next M3 run needs" names it.

**D13. The local exclusion list comes before any remote embedding.** Row 30-1 puts the exclusion on curation calls at milestone 3 and on embeddings at milestone 4, and no exclusion can be set on Design B yet: milestone 3 built the egress gate's redaction, not its list. With no hub, a device's local list is the whole list (spec 5.5). So Task 5 adds that list before its first call to Workers AI. `oboete exclude <repo>` and `oboete exclude --undo <repo>` append an exclusion op to raw.db (spec 5.5: an exclusion is an op, which milestone 6 syncs), and the egress gate reads the list from raw.db before each outbound call, with no consumer in between. Content of a session that touched an excluded repository goes to no curator and no embedder. Curation leaves that session's records out of what it sends (a window cuts across sessions, spec 1.6), and a window left with nothing is skipped with reason `excluded`. The embedding consumer skips the claims and imported documents of an excluded repository. A search query is checked against the caller's repository and every repository searched; when any is excluded, it gets full text only (row 30-2). Task 4's search has no remote leg, so nothing leaves the machine before the list exists. Capture exclusion, which keeps a repository from being recorded at all, stays milestone 5's (spec 7.5). The touch sets from tool paths and the rule for paths that cannot be classified (row 30-20) stay milestone 6's.

## Tasks

| # | Task | Executor | Depends on | Produces |
|---|---|---|---|---|
| 0 | Read-path timing in `oboete replay`: SessionStart and UserPromptSubmit spawns, warm and cold; today's numbers as a baseline that decides nothing | Claude | — | `replay --read-sample N`, `docs/spike/read-hook.md` |
| 1 | The delivered set (D1, D2) and SessionStart's content (D4); the spec's milestone 4 fixture wording (#295 row 1) | Claude | — | `claims::{DELIVERED, deliver, LINKS_END_DECISIONS}`, `restates` edges, SessionStart sections |
| 2 | Imported records parked from curation; `recurate --source`; the manifest's live sources; doctor (D6) | Claude | — | `imported:<source>` window skips, `oboete recurate --source` |
| 3 | `import` ops, the `Imported` consumer, claude-mem's import on B (D5) | Claude (op type, consumer); Codex (`import.rs` port to the op sink) | 2 | `OpKind::Import`, knowledge.db `imported` and `imported_fts`, `oboete import claude-mem` on B |
| 4 | The search core (D7); CLI and MCP on it; MCP `get` and `timeline` on B; the fence; v1's MCP and CLI halves removed | Claude | 1, 3 | `search::query`, `Query`, `Hit` |
| 5 | The local exclusion list and the egress gate's check (D13); then the embedding consumer and the hybrid's vector leg (D8); the embedding cap in providers.db | Claude | 3, 4 | `oboete exclude`, knowledge.db vectors, generations, doctor lines |
| 6 | `oboete eval` on B and the single test-split run (D10): Raw data, the rerank script, Holm, the English gap | Claude (eval entry, run); Codex (report.py's Holm, English gap and cross-lingual subset; the rerank script) | 3, 4, 5 | the run, its report in `docs/milestone-4.md` |
| 7 | The viewer on B (D11); v1's routes, `inject.rs`, v1 search and v1 embedding removed | Claude (routes, guards); Codex (the page's JavaScript for the new routes) | 4 | B routes, rows 53-1 to 53-6 on them |
| 8 | Shortlist and per-prompt injection (D9); corrections; compaction re-injection; `[inject]` settings; MUST-M8's one-device line, MUST-M9's lag form | Claude | 1, 4, 5 | shortlist consumer, UserPromptSubmit path (off by default) |
| 9 | Cut-over tools: `oboete migrate [--from]`, `--finish`, `oboete import transcripts`, `session_repos` as label records, doctor lines; the Transcript line's fixtures | Claude (`migrate`); Codex (`import transcripts`) | 2, 3 | commands; rows of spec 7.4 |
| 10 | Local bge-m3: a spike on the runtime (size, RSS, speed on the iMac and the slowest WSL machine), then `setup --embeddings local` (spec 7.1, MUST-M23) | Claude | 5 | `docs/spike/local-embeddings.md`, the local embedder |
| 11 | The reranker in the binary, loaded by the worker: only if Task 6's Rerank run clears the M1 line | Claude | 6 | or nothing, with the run's numbers |
| 12 | Lines and the milestone note: the read-hook line, M22, the worker half, the Transcript line; dev harnesses for M5, M6 (D12) and Inject | Claude; Codex (harness scripts) | 0-11 | `docs/milestone-4.md` |

---

## Task 0: Read-path timing

**Files:**
- Modify: `src/replay.rs` (a read sample beside `sample_spawns`)
- Modify: `src/main.rs` (`--read-sample N` on `Replay`)
- Create: `docs/spike/read-hook.md`

**Interfaces:**
- Produces: `replay::sample_reads(home, root, n, agent, event) -> Result<Vec<u128>>`, where `event` is `SessionStart` or `UserPromptSubmit`: `n` spawned `oboete hook <agent> <event>` runs, in microseconds, sorted. The report gains `read: {session_start: {warm, cold}, prompt: {warm, cold}}`, each with p50, p95, p99 and max.
- Warm: after `worker::drain` has run every consumer on the replayed home. Cold: before the drain, with `OBOETE_NO_SPAWN` set, which is what a hook sees before the worker is up (spec 4.2). The process holds the worker lock meanwhile, as `sample_spawns` does, so the spawned hooks start no worker.

- [ ] **Step 1: Failing test.** Spawned hooks run the binary, which `cargo test` does not build, so the test covers the in-process arm: `a_cold_read_shows_nothing_and_a_warm_one_shows_the_manifest` (before the drain SessionStart shows nothing for a new checkout; after it, the manifest with the replayed prompt, and the characters counted are those shown).
- [ ] **Step 2: Implement** `sample_reads` and the flag. UserPromptSubmit injects nothing yet: its time is the write plus the hook's fixed cost, the baseline Task 8 is compared with.
- [ ] **Step 3: Run** on WSL with `events-1000.jsonl` into a home that has claims (a copy of the M3 dev base with its knowledge.db), then on the iMac and on Windows native. Record VmHWM of the hook processes from `/proc` where it exists.
- [ ] **Step 4: Note.** `docs/spike/read-hook.md`: the numbers, the machines, and that they are a baseline. The read-hook line is set once in Task 12, from the first measurement of the delivered SessionStart's warm path (spec 8.2 Read hook row).
- [ ] **Step 5: Commit** `replay: time SessionStart and prompt hooks, warm and cold (milestone 4, Task 0)`.

---

## Task 1: The delivered set and SessionStart's content

**Files:**
- Modify: `src/claims.rs` (`DELIVERED`, `deliver`, `LINKS_END_DECISIONS`; `Claim` gains `later: Option<String>`)
- Modify: `src/curate.rs` (`restate` writes `restates`, not `supersedes`)
- Modify: `src/consumer/claims.rs` (edges of type `restates` from the op field)
- Modify: `src/consumer/manifest.rs` (`with_decisions` becomes the SessionStart sections of D4; built without a stored manifest row)
- Modify: `src/hook.rs` (`checkout_manifest` injects the delivered part when the stored text is absent)
- Modify: `docs/spec.md` (8.4's milestone 4 fixture: both until curator links pass M3's control line, then only the later one, #295 row 1)
- Test: `src/claims.rs`, `src/consumer/manifest.rs`, `src/hook.rs`

**Interfaces:**
- `claims::DELIVERED`: SQL over the `active` view (D1). The linker's condition reuses `anchored_through`'s owner predicate, moved into one constant that both use.
- `claims::deliver(k, repo, kinds: &[&str], limit) -> Result<Vec<Claim>>`: the delivered claims of `kinds` in `repo`, ranked, with each picked earlier claim's later claim placed directly before it (D2). With `LINKS_END_DECISIONS` true, an earlier claim ended by a curator link is left out.
- `claims::global(k)` keeps its query; SessionStart shows its claims first.

- [ ] **Step 1: Failing tests.**
  - `an_earlier_decision_ended_by_a_later_owner_decision_is_delivered_after_it_with_both_dates`;
  - `a_link_from_a_proposal_a_retracted_claim_or_an_earlier_claim_delivers_nothing_new`;
  - `a_restatement_ends_a_claim_as_before`;
  - `a_picked_earlier_claim_brings_its_later_claim_and_the_pair_takes_two_places` (Review Focus 1);
  - `with_links_ending_decisions_only_the_later_claim_is_delivered`;
  - `a_retraction_in_another_chunk_never_brings_the_decision_back` (row 54-6);
  - `global_preferences_come_first_and_about_ten_claims_have_bodies`;
  - `a_checkout_with_claims_and_no_manifest_row_still_gets_them`;
  - owner decision 31's fixture: a SessionStart lists a later decision above the earlier one its curator link ended, both with their dates.
- [ ] **Step 2: `restates`.** `restate` adds its uids to the op's `restates`; the claims consumer writes them as edges of type `restates`; `TIPS` treats every edge type as an end, as now.
- [ ] **Step 3: `DELIVERED` and `deliver`.** Rank for SessionStart by the number of the manifest's files and last-prompt words a claim's body shares, then by recency; the pair rule applies after ranking.
- [ ] **Step 4: SessionStart.** Global preferences, the manifest, the delivered claims (about 10 with bodies, the rest one line each), the tools line, the digest if fresh. `with_decisions` becomes a function that takes the stored text as an `Option`.
- [ ] **Step 5: Spec.** Reword 8.4's milestone 4 fixture as #295 row 1 says.
- [ ] **Step 6: Run** `cargo test claims manifest hook` and the whole suite.
- [ ] **Step 7: Commit** `delivery: the delivered set and SessionStart's content (milestone 4, Task 1)`.

---

## Task 2: Imported records are parked

**Files:**
- Modify: `src/curate.rs` (the window cut stops at a change of source; records of `oboete-v1` and `transcript` get a window op skipped with reason `imported:<source>`; `recurate` gains `--source`)
- Modify: `src/raw.rs` (the window's record query returns `source`)
- Modify: `src/consumer/manifest.rs` (facts from `hook` and `replay` records only; the not-yet-curated count leaves parked records out)
- Modify: `src/setup.rs` (doctor: `imported, not curated: N records (oboete-v1: a, transcript: b)`)
- Modify: `src/main.rs` (`Recurate { source: Option<String> }`)
- Test: `src/curate.rs`, `src/consumer/manifest.rs`

**Interfaces:**
- A parked span is a window op with outcome `skipped` and reason `imported:<source>`: the M2 coverage check counts it, and `recurate --skipped` does not take it.
- `oboete recurate --source <source> [--send]`: the parked spans of that source, their windows and the cost estimate; with `--send`, curated.

- [ ] **Step 1: Failing tests.** `an_imported_record_is_skipped_with_its_reason_and_never_sent` (the stub provider sees no call); `a_window_never_mixes_live_and_imported_records`; `recurate_source_curates_the_parked_spans_only`; `the_manifest_ignores_imported_records`; `the_backlog_line_does_not_count_parked_records`; `every_seq_is_curated_elided_or_skipped` still passes with imported records in the fixture.
- [ ] **Step 2: Implement** the cut and the skip in the curation phase, with no provider call and no budget spent.
- [ ] **Step 3: `recurate --source`**, with the v1 sessions over 16,000 characters listed first (spec 7.4).
- [ ] **Step 4: Manifest and doctor.**
- [ ] **Step 5: Run** the suite. **Commit** `curate: imported records are parked until recurate --source (milestone 4, Task 2)`.

---

## Task 3: Imported documents

**Files:**
- Modify: `src/raw.rs` (`OpKind::Import`; `Raw::append_imports(&[ImportDoc])`, one transaction per batch)
- Create: `src/consumer/imported.rs` (knowledge.db `imported(op_device, op_seq, uid, source, source_id, kind, repo, session, ts, title, body)`, primary key `(op_device, op_seq)`, `uid` indexed, and `imported_fts` trigram; rewind by `op_seq` per device)
- Modify: `src/worker.rs` (the consumer list)
- Modify: `src/import.rs` (the sink: a `Row` becomes an import op; the reader, the redaction and the source name stay)
- Modify: `src/main.rs` (`oboete import claude-mem <db> [--eval-store]` on B; v1's sink removed)
- Test: `src/import.rs`, `src/consumer/imported.rs`

**Interfaces:**
- `ImportDoc {uid, source, source_id, kind, repo, session, ts, title, body}`, serialized as the op body, at most `MAX_OP_BYTES` (a longer body is clipped with a marker).
- `Raw::import_keys(source) -> Result<HashSet<String>>`: the source ids already imported, read before appending.
- Batches of at most 500 documents, each its own `batch` (D5).
- The `--eval-store` guard stays: the flag is required, and `--home` must not be `~/.oboete` (docs/pr-b.md:21).

- [ ] **Step 1: Failing tests.** `importing_twice_appends_nothing_the_second_time`; `a_killed_import_resumes_without_duplicates` (the crash harness at a batch's commit); `the_source_name_and_uids_match_the_v1_import` (same hash, same `o`, `s`, `p` ids); `imported_documents_are_never_curated_or_injected`; `imported_fts_finds_an_imported_title`; `an_op_over_the_cap_is_clipped_with_a_marker`; `a_rewind_removes_the_rows_of_the_lost_ops_only` (D5). Forgetting an imported document is milestone 5's.
- [ ] **Step 2: The op type and the consumer** (Claude).
- [ ] **Step 3: The sink port** (Codex, with this task's tests as its acceptance; Claude reviews).
- [ ] **Step 4: Build the evaluation home** from `claude-mem-2026-09-24.db` into a new directory under `~/.oboete/eval/`, and record the count (178,502 expected), the import time and the knowledge.db size.
- [ ] **Step 5: Run** the suite. **Commit** `import: claude-mem history as import ops on B (milestone 4, Task 3)`.

---

## Task 4: The search core

**Files:**
- Modify: `src/search.rs` (`Query`, `Hit`, `query`; the claims, imported and raw legs; `fuse` reused)
- Modify: `src/mcp.rs` (tools on B; `get` for a claim uid, a raw `device:seq` and an imported uid; `timeline` of claims, imported documents and session starts around an anchor; the fence; the exclusion check's call)
- Modify: `src/main.rs` (CLI `search`, `get`, `timeline` on B; v1's halves removed)
- Test: `src/search.rs`, `src/mcp.rs`

**Interfaces:**
- `Query {text, repo: Option<String>, all: bool, since: Option<i64>, until: Option<i64>, history: bool, raw: RawArm, limit}`; `RawArm {Off, Below, Only}`, default `Below` (D7). The Raw run's third variant, RRF with a penalty, is `Below` with the penalty as a parameter of the evaluation entry.
- `Hit {key, class: Delivered | Current | Imported | Raw | Superseded { by }, repo, when, kind, status, label: Citable | QuoteOnly | Imported, title, snippet}`.
- `search::excluded(caller_repo, searched: &[String]) -> bool`: called before any leg that could send the query out (row 30-2). Task 4 has no such leg; Task 5 gives the function D13's list before it adds one.

- [ ] **Step 1: Failing tests.**
  - `a_superseded_decision_never_ranks_as_current_and_history_shows_it_labelled` (MUST-M11);
  - `a_delivered_earlier_decision_ranks_with_current_claims` (#295 row 3);
  - `since_and_until_filter_every_leg_before_fusion`, a table of cases (MUST-M12);
  - `every_hit_has_one_label_and_imported_hits_say_imported` (MUST-M13);
  - `the_exclusion_check_is_called_with_every_searched_repo` (row 30-2);
  - `mcp_replies_are_fenced_as_data`;
  - `get_resolves_a_claim_uid_a_raw_record_and_an_imported_uid`.
- [ ] **Step 2: The legs and the fusion.** Full text only here; the vector leg comes with Task 5 through the same `fuse`.
- [ ] **Step 3: MCP and CLI** on `query`, with the new arguments in the tools' JSON schemas.
- [ ] **Step 4: Remove** MCP's and the CLI's v1 paths. The viewer keeps v1's `search::find` until Task 7.
- [ ] **Step 5: Run** the suite. **Commit** `search: one entry point over claims, imported history and raw (milestone 4, Task 4)`.

---

## Tasks 5-12 (files and interfaces; step bodies in the follow-up PR)

**Task 5: the exclusion list, then embeddings.** First D13: `oboete exclude` in `src/main.rs`, `OpKind::Exclusion` in `src/raw.rs`, the list read by the egress gate before each outbound call, curation's `excluded` skip in `src/curate.rs`, and `search::excluded` on the list. Tests: row 30-1's exclusion half on curation and on embedding (an excluded repository: zero outbound requests), row 30-2. Then create `src/consumer/embed.rs`; modify `src/embed.rs` (the Workers AI call and the batching move over; the v1 tables go in Task 7), `src/search.rs` (the vector leg, top 100 into `fuse`), `src/providers_db.rs` (embedding calls in `provider_calls`), `src/config.rs` (`[embedding] monthly_usd`), `src/setup.rs` (doctor: pending, waiting until, generation coverage). Tests: rows 46-1, 46-2 (every way a uid's text changes: re-derivation, owner correction), 55-1, 55-5, 55-6, 55-7, 30-16.

**Task 6: the test-split run.** Modify `src/main.rs` and `src/search.rs` (`oboete eval` on B, with the Raw arm and its penalty as arguments), `docs/eval/report.py` (Holm against the hybrid; English minus Japanese with a 95% interval; the cross-lingual subset by a rule written before the run), `docs/eval/judge.py` (pool depth 50); create the rerank script under `docs/eval/`. Before judging: all runs present, the question counts per slice reported (Raw's N first).

**Task 7: the viewer on B.** Modify `src/view.rs` (the routes of D11), `assets/viewer/*`; remove `src/inject.rs`, v1's `search` functions and `embed.rs`'s v1 tables. Tests: every `/api/*` route answers 401 without the token; rows 53-1 to 53-4 on the B routes; a read during `rebuild` (Review Focus 6). Row 53-6's machine checks go in the PR text.

**Task 8: per-prompt.** Create `src/consumer/shortlist.rs`; modify `src/hook.rs` (UserPromptSubmit; corrections; Grok, agy and OpenCode after compaction; Cursor's resume), `src/config.rs` and `src/settings.rs` (`[inject] per_prompt`, `per_prompt_chars`, `correction`, `correction_chars`), `src/opencode.js` (drop the cached text on compaction), `src/setup.rs` (the per-agent status table), `src/consumer/manifest.rs` (MUST-M8's line for other checkouts of the repo on this device, MUST-M9's `M events / T min` shown past 2 minutes).

**Task 9: cut-over tools.** Create `src/migrate.rs`; modify `src/main.rs`, `src/raw.rs` (a batch append of records with the checkpoint op in the same transaction), `src/transcript.rs` (the import's per-session cut: entries older than the session's earliest raw record), `src/setup.rs` (doctor: old files awaiting `--finish`, evaluation copies). Tests: rows of spec 7.4 (rerun after new v1 events, kill between batches, `session_repos` kept after `rebuild`) and the Transcript line's fixtures for the Claude Code and Codex parsers. The forgotten-canary case needs forget's deny-list (milestone 5): until then the import calls a deny-list check that allows everything, and a test pins where it is called.

**Task 10: local bge-m3.** The spike compares runtimes on binary size, the model download, RSS and speed on the iMac and the slowest WSL machine, and measures agreement with Workers AI's vectors on a sample. The task adds `local` behind the embedder seam, the SHA-256-pinned download with resume and a free-space check, and `oboete setup --embeddings`.

**Task 11: the reranker.** Only if Task 6's Rerank run clears the M1 line. Loaded by the worker (spec 4.10); a reader falls back to plain RRF while no worker is up.

**Task 12: lines and the note.** The read-hook line (Task 0's tool on the delivered warm path, slowest machine, then fixed); M22 (MCP p95 at most 1.5 s and slowdown at most 20% under worker writes on the 178,502-document home; the injection hook's warm and cold paths at that scale; manifest truncation under each agent's size); the worker/no-worker one-device half (needs Task 10); the Transcript line; dev harnesses for M5's one-device lines, M6 (D12) and Inject. `docs/milestone-4.md`.

## Lines and when they run

| Line | Data | Runs in | Waits for |
|---|---|---|---|
| M1, Raw, Rerank, M21 English | the test split, the B evaluation home | Task 6 | Tasks 3-5 |
| Read hook | dev fixture, a home with claims, the slowest machine | Task 12 (tool: Task 0) | Tasks 1, 8 |
| M22 | the B evaluation home under a replaying writer | Task 12 | Tasks 4, 5, 8 |
| Worker, one-device half | dev transcripts | Task 12 | Task 10 |
| Transcript | fixtures | Task 9 | — |
| M5, one-device lines | held-out transcripts | after milestone 3's deciding run (A88) | dev harness in Task 12 |
| M6 | held-out transcripts, the panel's answer keys | after milestone 3's deciding run (A88) | dev harness in Task 12 |
| Inject | 100 no-answer and 100 false-premise questions | after milestone 3's deciding run (A88) | Task 8; the panel's labels |

Labels this milestone needs from the panel (spec 8.4 row 1, "During milestones 3-4"): yes/no on about 100 no-answer and 100 false-premise questions; M6's 40 questions and their answer keys; M5's open-item labels and its 20% spot-check; about 100 per-kind labels (MUST-M21).

## What needs the owner

Build work does not wait on these; some lines and defaults do.

1. **The embedding cap.** Claude proposes staying within Workers AI's daily free allowance, which v1's default of 200 requests a day already does (`src/config.rs`), plus at most USD 1 a month for a backfill: embedding the whole claude-mem store once cost about USD 0.80 (docs/pr-d.md:14). The owner confirms or gives another figure.
2. **Local embeddings.** After Task 10's spike: the model's download size on each machine (WSL's disk is short) against Workers AI's cost. The owner chooses per machine.
3. **Milestone 3's test labels** (still owed, docs/milestone-3-plan.md "What needs the owner" item 1). M5's, M6's and Inject's deciding runs wait for milestone 3's deciding run, which needs them (A88).
4. **`oboete migrate --finish`** deletes old files only after the owner answers yes, at the cut-over after milestone 5.

## Stop rule for this document's review

This PR merges when the Codex review of its final head adds no finding that changes an owner decision or one of D1-D13. After two consecutive rounds whose findings are mechanism only, the rest moves into an issue as acceptance tests for Tasks 5-12, each finding is answered and resolved, and the PR merges without a further commit (CLAUDE.md, "Design-doc PRs").
