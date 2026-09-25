# Do sections 1-4 still carry old-design constraints? (architect's answer, 2026-09-25)

## Short answer

Yes. None is as large as the two you already caught, but three come from the same place:

1. **Inherited latency numbers.** The 20 ms hook write, the "tens of ms" per-prompt search and the "strong GPU may rerank live in the hook" tier all come from the old design. The 8 ms figure behind 20 ms was measured under `synchronous=NORMAL` (the design now uses FULL). The per-prompt figure goes back to the old 300 ms budget, which contained a 150 ms Workers AI call inside the hook. The live-rerank tier puts computation back into the hook, which is what "hooks only read" removed.
2. **"knowledge.db can be dropped and rebuilt from raw."** This is the TypeScript spec's rebuild promise. For AI-made claims it means paying for curation again and getting different claims. Today `rebuild` keeps only the op log received from other devices (options-draft.md:366), so the device's own claims are curated again.
3. **Session as the organizing unit** (claude-mem's shape). It survives in the old schema (sessions table as foreign-key parent, `DELETE ... WHERE session_id`, src/db.rs:9-42,848) and in the new per-session shortlist.

The other findings are not inherited constraints. They are places where the section text drifted from your own requirements: a change with no "why" is dropped, there is no speaker value for "AI guess", `oboete pref add` is missing, and embeddings are tied to the AI tier. There is also one gap: capture handles text only, so images and binary tool results go unmodelled.

We checked the rest of the structure and it holds:
- raw log as the source of truth
- a full replica on each device
- one egress gate
- the hub as a star topology
- Rust + SQLite
- typed claims with code gates
- the worker computes ahead and hooks only read

The two proposals that argued for live search in the hook (S4-2, S4-5) were refuted for the right reason: they would bring back the hook-compute shape you already caught.

File:line references use the locations the skeptics verified. Where a proposal cited the wrong line, the verified one is used. Items marked *weak* survived the vote, but a skeptic's objection still stands, and I say which one.

---

## Section 1: overall shape

### 1. Relax or replace now

**S1-21: Embeddings are a separate setting from the AI tier.** Verdict: adopt.
- Change: section 1 says "no AI = full-text only". Rewrite it: recording and full-text search need no configuration, and semantic search works whenever `embedding.provider` is local or workers-ai, whatever the curation tier.
- Evidence:
  - This is already the shipped behaviour: src/config.rs:18-44, and search.rs checks only `embedding.provider`.
  - options-draft.md:181 already asks the two setup questions separately.
  - R07 (issue50.md) lists switching embedding models as its own must-keep.
- Cost: a doc fix in sections-1-4.md:10 and options-draft.md:137-141/257, plus one test.
- Decides it: a test with `providers=[]` and embeddings on still returns hybrid results, and doctor prints the tier and the embedding status as two lines.

**S1-5 + S3-21: `rebuild` must not silently pay for curation again.** Verdict: adopt narrowed.
- Change: keep the device's own outbound claim, correction and manifest ops in the same kept op log as the inbound ones. The sync consumer produces these ops already; store them even when no hub is configured.
  - Default `oboete rebuild` rebuilds FTS, vectors, digests and packets, and replays the kept ops. It makes zero AI calls and produces the same claims.
  - `oboete recurate [--skipped | <span>]` is the explicit path. It shows a cost estimate against R15 first. This also covers S3-21's backlog case, which is a manual command plus a doctor backlog count, not an automatic trigger.
- Evidence:
  - options-draft.md:110 (rebuild drops the database and replays from raw) and :366 (only inbound ops are kept).
  - The S7 egress ledger holds no payload (improvements-synthesis.md:475), so it cannot replay claims. This is why the proposal's original "cheap policy split" was wrong.
  - R15 ($5/month, issue50.md:49).
- Cost: about 0.5-1 PR. `forget` must also purge the kept outbound log, exactly as it purges the inbound one.
- Decides it: `rebuild` on the full corpus shows $0 of AI spend and an identical content hash for the claim set. `recurate` on a sample produces an estimate that is compared with actual provider billing.

**S1-10 + S2-18 + S4-18: Session is a label, not a key.** Verdict: adopt.
- Change: add one sentence to section 2: raw.db's only ordering and partition key is `(device, seq)`, and session, repo and branch are label columns. A session is never a required parent row, an index root, or a curation or checkpoint boundary.
  - Key the section-4 shortlist by `(session, checkout)`. When `touch_repo` sees a new checkout (src/db.rs:277, hook.rs:378), the worker rebuilds the shortlist, and until it is ready the hook uses the fast path.
  - The `timeline` command stays (R09).
- Evidence:
  - The old schema is exactly the risk described above (src/db.rs:9-42,848).
  - M3 already found that session-scoped supersession was a bug (improvements-synthesis.md:48-56).
  - Nothing in issue50 §B/§C keys claims by session.
  - A session that touches several repos (like this audit: oboete, free-mem and thedotmack) gets a shortlist for the wrong repo.
- Cost: one sentence and a schema-review checklist item; the shortlist key change is small.
- Decides it:
  - Schema review of the raw.db PR finds no session foreign key or index root.
  - Replaying a session that touches two or more repos shows zero mismatches between the shortlist's repo and the active repo.
  - M2 gets a cut in the middle of a session where no session-end event ever arrives.

**S1-4 + S2-11: Compression grain.** Verdict: adopt narrowed, modest gain.
- Change: keep per-record zstd, which keeps `forget` cheap, and add a trained shared dictionary with a `dict_id` column. Reject block compression, because deleting one record would mean recompressing its whole block.
- Evidence: the gain depends heavily on the data.
  - The events-1000.jsonl fixture: 1.6x becomes 5.2x.
  - A real Claude Code transcript sample (my run: 20,000 lines from 80 files, 40 MB, level 3; records over 4 KB are 53% of the bytes): plain per-record 2.13x; a 16 KB dictionary 3.04x; a 110 KB dictionary 3.24x. That is about 30-34% fewer bytes.
  - One skeptic's run on another transcript set found a similar gap (about 1.7x between per-record and block/dictionary).
  - At a few hundred MB to 1 GB a year, the saving is about 100-300 MB a year.
- Cost: small, inside the raw.db PR. Every dictionary must be kept forever so old rows can be decoded, and dictionaries must travel with raw sync if that is on. It is not worth a PR of its own.
- Decides it: the same comparison on a real raw.db after a week of dogfooding. Keep the dictionary only if it saves 20% or more.

*Weak survivors, not recommended:*
- **S1-12** (curate again on a peer device): decision 20 already copies the API keys to the iMac and Windows, and curating again on the same device covers the real case. It is folded into `recurate` above.
- **S1-23** (a user-run local server counts as a "remote API"): Ollama already fills this role in the free/local tier (issue50.md R05, options-draft.md:139). No change is needed.

### 2. Inherited constraints that are still right (checked)

- **S1-1** A full replica on every device. It serves offline use and R02, and the phone is on hold. The hub plus Vectorize is the thin-client path if the phone comes back.
- **S1-2** Hooks are the capture trigger. Kept, but the "never read transcripts" wording is dropped; see S2-17.
- **S1-3** Redaction before the write. raw.db is unencrypted, kept forever, and can sit in cloud-synced folders. The addition is a redaction ledger; see S2-2.
- **S1-6** A worker started by hooks that exits when idle. It matches decision 2 ("earn their place") and R14, and M5 can still flip it; see S2-21 for the wording.
- **S1-7** A per-device seq with a checkpoint per consumer. It fixes the real loss in src/db.rs:848 (raw deleted once summarized).
- **S1-8** Sync is one more consumer. claude-mem's CloudSync reached the same shape ("the database is the queue").
- **S1-9** No file-based delivery (CLAUDE.md or AGENTS.md). Those files don't reach the four agents still unverified live, and every agent has an injection hook.
- **S1-11** A closed set of claim kinds. Open-ended extraction collapsed to generic labels in Graphiti/Zep. The mapping from the old kinds is still missing; see S3-6.
- **S1-13** Two redaction passes. Query text never passes the capture hook. The capture pass has to scan everything that is kept; see S2-5.
- **S1-14** A star topology through one hub. This is decision 12, made knowingly, with a revisit trigger.
- **S1-15** Cloudflare as the hub. The wire format is plain HTTP and JSON anyway, since the Durable Object is out of reach from clients. No abstraction layer is needed now.
- **S1-16** One device needs no cloud. This is decision 12, and it serves decision 8 (easy install).
- **S1-17** No summarization in the cloud. This is decision 6 of 2026-09-23 and R08, and Workers cannot run the subscription CLIs.
- **S1-18** Raw sync is off by default. R08. The open question is under 3.
- **S1-19** Four AI tiers with caps. These are your own numbers, stated twice.
- **S1-20** Curator sessions are never captured. This was a real M1 bug (m1.md:227) and is fixed with `OBOETE_SKIP`.
- **S1-22** Rust + SQLite. The reason is the install goal (decision 8); Hindsight + pg0 failed its install spike (phase0.md).
- **S1-24** The measured search as the yardstick. Scores are already reported per slice (pr-d.md:117-131; M21 adds English and per-kind slices).
- **S1-25** One owner per install. No goal asks for a second person.

### 3. Needs your decision

**Where does raw exist besides the recording device?** This combines S1-18 and S2-24. options-draft.md §10 item 1 (raw sync) is the only §10 item with no entry in owner-decisions.md.

As designed today:
- Losing a disk loses its raw. Local backups don't survive losing the machine.
- Other devices get only the quote carried in each claim.
- On the none tier, lookups on other devices come back empty.

Options:
- (a) Keep it as it is.
- (b) Ask one question at `oboete setup`: turn raw sync on? Off by default for every tier. No code change beyond the question.
- (c) An encrypted off-device copy of the backup segments in R2, separate from sync. R2 costs cents, but `forget` must reach it, so about 1 PR or more. It also falls under decision 9 ("whether to archive raw", postponed).

**Recommendation:** (b) now, and (c) when you settle decision 9.

---

## Section 2: recording

### 1. Relax or replace now

**S2-5 (+ S2-13, S1-13 and half of S4-22): the hook-write latency is measured, not asserted.** Verdict: adopt.
- Change: replace "within 20 ms" with "the agent never notices", and set the number from M14's measurement under FULL on WSL, Windows and the M1. The measurement has to include two costs that nobody has timed yet:
  - **Redacting the whole stored output.** Today `clip()` redacts only the first 12,000 characters (MAX_FIELD 8,000 + REDACT_OVERLAP 4,000, src/hook.rs:15-18,559-579). Section 2 keeps outputs of up to about 256 KB, so the capture pass must scan every byte it stores. The trace for S1-13 saw the same tension but concluded that the deep scan belongs at the egress gate. That is wrong for raw.db: it is kept forever on disk and never passes the egress gate.
  - **macOS durability.** On macOS, FULL calls `fsync`, which does not flush the drive's cache. Durability across a power loss needs `PRAGMA fullfsync`, which is slower. Measure both on the M1.
- Evidence:
  - The only measurements (8 ms p95: plan.md:56, m1.md:241) were taken under NORMAL (src/db.rs:106).
  - M14 still lists the FULL measurement as to be done (improvements-synthesis.md:244).
  - Your plan.md item 7 says no cap numbers are needed, and decision 2 applies.
- Cost: the wording, plus the M14 run that is already planned, plus a fixture with 64 KB and 256 KB tool outputs.
- Decides it: `oboete replay` p95 and p99 on each device, FULL against FULL + fullfsync. Write the number into the spec after that run.

**S2-17 (+ S1-2 and S2-1, both refuted alone): capture sources.** Verdict: adopt narrowed.
- Change: remove "not by tailing transcripts" from the design. It appears in no owner document, and hooks already read transcript tails for agy, Codex and Cursor (src/hook.rs:243-368). Keep excluding a proxy on API traffic and wrapping the CLI.
  - Build only detection now: at Stop or SessionEnd, compare the transcript's turn count with the raw rows for that session, and feed the difference into the planned doctor line for hook failures per adapter (improvements-synthesis.md:535).
  - Backfilling from transcripts in the worker is LATER, and only if detection shows real gaps. It then needs three things:
    - A tombstone check, so a row removed with `forget` doesn't come back from a vendor transcript. M23 accepts that oboete can't purge vendor transcripts.
    - A provenance tag on every backfilled row.
    - One parser per transcript format. Grok stores sessions in SQLite, not JSONL, and OpenCode has no transcript at all (agent-adapters-2026-09-23.md:393).
- Evidence: issue50.md:73 asks for recovery from any transcript that is available.
- Cost: about 0.5 PR for detection.
- Decides it: the gap count per adapter over a week of dogfooding. Build backfill only if some adapter misses turns in normal use.

**S2-2 + S1-3 + S2-22: redaction you can inspect, and a rescan when the rules gain a pattern.** Verdict: adopt narrowed.
- Change:
  - Keep a redaction ledger: for each hit, the rule id, offset, length, time and ruleset version, never the value. claude-mem kept detections like this (redaction-pipeline.ts:34-43). src/redact.rs leaves only `[REDACTED]`, so a false positive can be seen but not explained or allow-listed.
  - When the ruleset gains a pattern, rescan old raw. Record each new hit as a new kind of tombstone that covers a byte range inside one record. Compaction then masks that range in place, keeping the seq.
    - This stays within options-draft.md:109's one exception ("never touched by derivation except tombstone compaction"), which was the objection one skeptic raised. It does not add a new way to write to raw.db.
    - `forget` today removes whole records (options-draft.md:171). The range tombstone is a finer form of the same thing, so it travels to other devices, reaches backups and joins the deny-list.
    - Claims whose quote overlaps the range are curated again, as `forget` already does for anchors that are only partly covered.
- Limitation: the ledger shows false positives but can't bring back the masked text. Plaintext never reaches disk, by design.
- Cost: under 0.5 PR for the ledger. The rescan is about 1 PR: the range tombstone, masking during compaction, and curating the affected claims again.
- Decides it:
  - Add a rule and run the rescan on an old record: the range is masked, the seq is unchanged, the claim quoting it is curated again, and M4's grep finds 0 hits, including in backups and on other devices.
  - Count ledger hits per rule id on a replay to find false positives such as git SHAs and hashes.

**S2-21: backups must not depend on the worker going idle.** Verdict: adopt.
- Change: give the backup job a `next_attempt_at` deadline. It is checked when the worker exits and periodically while it runs, the same pattern as M9. Also mark the worker's lifecycle in §1/§2 as "default, pending M5", since M5 has not run (options-draft.md:338-351).
- Evidence: M15 says "scheduled", but its only trigger is the idle exit (improvements-synthesis.md:251-256). A worker kept busy through a 24-hour session never goes idle, so it never backs up.
- Cost: small, inside M15.
- Decides it: a fixture that keeps the worker active for 48 hours still produces a backup segment within the interval.

**S2-25: images and binary tool results.** Verdict: adopt.
- Change: at capture, replace image and binary content blocks with a marker `{kind, mime, bytes, sha256}`. That covers the Anthropic image source, Claude Code's Read on an image file, and OpenAI's `image_url`, plus a check on base64 length for shapes nobody listed. The marker never goes to any curator or judge.
- Evidence:
  - src/pi.ts:16-17 drops images silently.
  - `compact()` in hook.rs turns base64 into text.
  - claude-mem strips these blocks (src/sdk/prompts.ts:190-233, field-optimizer.ts:176-188). Its issue #3606 measured 225k-501k tokens per summarization run caused by them.
  - Your own transcripts hold about 1,000 or more image blocks.
  - Once outputs are kept in full, the old 600- and 8,000-character cuts no longer hold base64 back.
- Cost: one match arm in the shared `compact()`, plus pi.ts and opencode.js, plus one fixture.
- Decides it: the curator window's token count and the raw bytes for a session with images, with and without the marker.

**S2-15: classify SQLITE_BUSY.** Verdict: *weak*, narrowed.
- Change: add BUSY/LOCKED to M16's list only as a label, so doctor can say "contention" instead of "disk full".
- Do not delay the alarm, which is what the proposal as written asked for. `busy_timeout` of 2000 ms (src/db.rs:101) absorbs short contention with no error, so a BUSY that gets through means the event really was lost. Hiding that breaks issue50.md:73.

### 2. Inherited constraints that are still right (checked)

- **S2-3** raw.db is append-only, per device, and kept forever. This is decision 7, and it reverses the purge at src/db.rs:848.
- **S2-4** Checkpoints are seq, never byte offsets. `forget` rewrites raw.db in place; claude-mem's byte-offset tailer needs a workaround for truncation.
- **S2-6** Hooks never wait on AI. It matches the per-agent hook timeouts.
- **S2-7** Curator sessions are excluded from capture. memU hit the same problem (ADR 0015). Optionally, check that `claude -p` with `disableAllHooks` also disables global hooks.
- **S2-8** The metadata set is kept. Addition: also stamp the HEAD SHA and the worktree's gitdir. Both are read from the filesystem without a git process, and two detached-HEAD worktrees otherwise share one manifest key.
- **S2-9** Repo identity is the origin URL. This is decision 7 of 2026-09-23. A repo renamed on GitHub splits into two keys; merge them with `oboete repo alias` if it ever happens.
- **S2-10** Head and tail kept beyond about 256 KB. The fixture has no large outputs to set the number from. Make it a config constant and set it from real raw.db data after launch.
- **S2-12** Raw kept forever. Your decision 7 overrides the caution in issue50.md:74. Watch growth in doctor and revisit under decision 9.
- **S2-13** raw.db uses FULL and knowledge.db uses NORMAL. This fixes the coverage hole found in M14. The fullfsync question is part of S2-5.
- **S2-14** Startup rewinds checkpoints above raw's highest seq. It prevents events being skipped after a crash.
- **S2-16** Backups are sealed segments, with a warning for cloud folders. Encryption through the OS keychain and a blocking integrity check were rejected on their merits.
- **S2-19** WSL and Windows are two devices. They can't share one SQLite file over drvfs/9P; see sqlite.org/howtocorrupt.
- **S2-20** SQLite, not a custom segment log. The segment log would add framing and compaction code you would have to write yourself, for no measured gain.
- **S2-23** Wall-clock order with a skew alarm. The logical clock stays LATER (M7). Addition: define `valid_from` as the time of the raw event the claim is anchored to, not the time the curator ran, so the skew alarm guards the key actually used for ordering.

### 3. Needs your decision

- An off-device copy of raw: see Section 1, question 3.
- Nothing else. The worker-lifecycle wording in S2-21 only restates decision 2 and M5.

---

## Section 3: making knowledge

### 1. Relax or replace now

**S3-12: a change with no "why".** Verdict: adopt. This makes the design comply with your issue.
- Change: three states.
  - A why with evidence: inject.
  - `why: unknown` on a real change: inject, tagged "reason not recorded".
  - A bare "N files changed" line: never becomes a change claim.
- Evidence: issue50.md:84 ("an unknown reason is not a reason to discard; only N-line noise is excluded") against sections-1-4.md:25 and options-draft.md:150, which inherited PR-K3 (search-sync-proposal-2026-09-23.md:440).
- Cost: one enum value and one match arm; this part is not built yet.
- Decides it: PR-K3's own test still drops the noise, and a sample of real changes with no recorded reason still appears, tagged.

**S3-7: a speaker value for "AI guess".** Verdict: adopt.
- Change: add `assistant_inferred` for lessons and repo facts the curator put together when nothing literally proposed them and nobody replied.
  - Such a claim can't become decided unless a tool result or a repeated statement supports it.
  - It is labelled, or left out, in the roughly 10 claims injected at SessionStart.
- Evidence:
  - issue50.md:81 wants four categories that are never mixed. The fourth slot went to `imported`, so an AI guess has nowhere to go.
  - M6 counts every current chain tip, not only decided ones (improvements-synthesis.md:102). So today a guess is injected exactly like a proposal you actually saw.
- Cost: about 0.25 PR.
- Decides it: a fixture where the curator infers a fact is labelled `assistant_inferred`. Real proposals still qualify.

**S3-23: tell a question or negation from consent.** Verdict: adopt, with two corrections to the proposal.
- Change:
  - A turn that ends in `?` or `？`, or contains a negation (reuse M5's list), never promotes a claim to decided.
  - The veto judge receives the text of the acceptance turn.
  - Build the list of acceptance phrases from real turns in events-1000.jsonl and the imported history. The three phrases in options-draft.md:147 are examples, not the full list.
- Evidence: issue50.md:83 (§B: tell questions, negations, reposts and hypotheticals apart from consent; keep ambiguous turns as proposed). Section 3 doesn't implement this.
- Cost: code only, under 0.5 PR.
- Decides it: false promotions to decided stay at 0% on the dev split, and recall of true decisions drops by at most 5 points. That is PR-K1's line (search-sync-proposal-2026-09-23.md:438), not the 10% the proposal cited. It serves goal 2 ("never apply retracted ones"), not goal 3.

**S3-6 + S1-11: mapping from the old kinds.** Verdict: adopt.
- Change:
  - State the mapping: feature becomes change; discovery becomes repo fact, or open item where it is unresolved.
  - A kind outside the enum from any provider is stored as `repo_fact` with status unverified, instead of being saved wrongly. This restores the fallback from PR #31 (commit 40fc349).
  - Add the legacy kinds as columns in M21's per-kind precision and recall.
- Evidence: a hand check of the 39 discovery and feature rows in oboete.db found none without a sensible new kind. issue50.md:27 says kinds must not change silently.
- Cost: a few hours.

**S3-22: curation windows for agents without clean turns, and subagents.** Verdict: adopt.
- Change:
  - Cut windows at the turn boundary when the adapter reports one, otherwise at a tool-call boundary, otherwise at a size cap. The Index stage already does this (options-draft.md:126).
  - A child session (parentConversationId or parentID) starts with its parent's carried goal and open items, not an empty state.
- Evidence: Cursor with `-p`, OpenCode standalone, agy's executionNum and Pi's steer are not clean turns (agent-adapters-2026-09-23.md:131,236,357,508). Parent fields are listed at :75, :383 and :555.
- Cost: under 0.5 PR.
- Decides it:
  - The Cursor `-p` and OpenCode fixtures leave no uncovered span in the coverage ledger.
  - A fixture with many subagents shows the child's first window carrying its parent's state.

**S3-4: the pending reason and next attempt.** Verdict: adopt. This is only a text fix.
- This is M9, already a MUST item under decision 11. Section 3 still says the backlog shows only when every provider fails. Put M9 back into the text: each pending window has a reason (failed, budget spent, cooldown, waiting for you to finish) and a `next_attempt_at`, and doctor flags a window as overdue.

**S3-20, fix 1: restore `oboete pref add` and the viewer button.** Verdict: adopt.
- This implements your decision 13 (search-sync-proposal-2026-09-23.md:37; options-draft.md:148). It was dropped from sections-1-4.md:25.
- Keep rejecting a free-text `remember` tool: critique.md:27 found it opens a prompt-injection path.
- *Fix 2 (a directive ends the curation window early) is weak.* The detector it wants to reuse exists only on the none tier (options-draft.md:138) and doesn't cut windows. Drop it.

**S3-1: window size.** Verdict: *weak*, narrowed.
- Change: make window size an M3 variable (16k, 50k, provider maximum), swept on dev sessions only. The default is the smallest size inside the pass line, not the provider's maximum.
- Evidence: accuracy drops as context grows, even with perfect retrieval (arXiv:2307.03172; aclanthology 2025.findings-emnlp.1264).
- Why weak: as written, the proposal sweeps on the 30 held-out sessions, which §9 forbids (options-draft.md:313). It may need new dev transcripts, and every run draws on the daily cap.

**S3-18: which judge model at which tier.** Verdict: *weak*, a wording change only.
- Change "paid = Jev" to "whichever candidate wins the PR-B evaluation". Candidates include a classifier trained on the §3.1 labels. That applies to role (b) only: no labels exist for role (a), and a classifier trained on your repos may not carry over to other users (decision 8).
- Evidence: in an independent test, a classifier trained on labelled data beat every zero-shot judge's raw score (search-sync-proposal-2026-09-23.md:211).

### 2. Inherited constraints that are still right (checked)

- **S3-2** Oversized outputs become markers counted as seen and elided. Required by issue50.md:71, and the text is still in raw FTS.
- **S3-3** The checkpoint moves in the same transaction as the knowledge. This is issue50.md:72; it is already `apply_batch`.
- **S3-5** The subscription CLIs wait while you work. Kept for now; see 3.
- **S3-8** Decided needs a verbatim quote or an acceptance. This is PR-K1; judges can only make it stricter.
- **S3-9** The taint gates. Addition: report how often an ordinary proposal that paraphrases file or tool text gets flagged, as its own slice.
- **S3-10** Global scope only from your explicit words. This is decision 13; a `tool_scope` tier was already rejected.
- **S3-11** Supersedes only among the candidates shown. The cap of 20 is not tied to 16k. Tune K on the dev split in M3 if recall turns out short.
- **S3-13** Current = chain tips, with the wall-clock tie-break. Define `valid_from` as the anchor time (see S2-23). An HLC only if the skew alarm fires in normal use.
- **S3-14** A stale digest is never used. A marked-stale digest would still be read as current, which is the Hindsight failure.
- **S3-15** The highest tier wins a re-derivation. This is M18, locked by decision 11; the recipe-rank version couldn't let a lower tier win anyway.
- **S3-16** Corrections are events. Addition: target them by uid and raw anchor, so that a claim curated again under a new uid doesn't leave the correction orphaned.
- **S3-17** Negation on the none tier is found by deterministic keywords. This is decision 5. Grow the fixture to 30-50 pairs, including decoy pairs.
- **S3-19** Curator runs are marked with `OBOETE_SKIP`, which already works (provider.rs:446). Session-id matching fits memU's setup, not this one.
- **S3-21** Curation doesn't restart automatically when the tier rises: R05/R15 forbid silent paid spending. The manual `recurate` is in the S1-5 item.
- **S3-24** Evidence is a quote plus an anchor on the recording device only. Syncing wider anchors would leak raw text. A hash of the quoted span can be added later if checking evidence from another device matters.
- **S3-25** Scope is repo or global. A branch-scoped claim would split repo-wide decisions by branch (R10's rule against splitting repo sharing) and has no rule for what happens on merge. Branch stays on raw and on the manifest.

### 3. Needs your decision

**S3-5 / S9: subscription CLIs while you are working.** You approved this as part of section 3, but its premise is thin.
- As written, it means:
  - If any hook fired in the last N minutes (N is unset), only free or local providers may curate.
  - Stop fires after every agent turn, so subscription curation in practice runs only after N minutes of silence.
  - Your default tier is subscription. Unless a free provider such as Groq is configured, claims wait until you pause. Manifests still sync at once, so resume on another device still works.
- plan.md:34 records you saying your subscription quota "goes to waste", which argues against scarcity.
- S9 is a SHOULD item, not one of the 23 MUST items, so decision 11 doesn't lock it. You approved it only as part of the section-3 text. Two skeptics treated it as locked, and that was wrong.
- Options: keep the gate, or let subscription curation run while you work, up to about 20% of the daily cap.
- **Recommendation:** keep the gate with N as a setting (default 10 minutes), and measure how long windows wait on a real heavy day. If they routinely wait an hour, drop the gate.

---

## Section 4: delivery and search

You approved section 4 with "OK if truly no problems". These are the problems. Two parts of the text contradict the section's own principle ("vectors" in the pre-worker hook, and a live reranker in the hook). Grok gets one injection per session. And the shortlist is keyed by session.

### 1. Relax or replace now

**S4-7 (+ the valid half of S4-22): remove the live reranker from the hook.** Verdict: adopt.
- Change: delete "users with a strong GPU may run the reranker live (target about 1 s)". Reranking happens only in the worker; the hook keeps a plain match.
  - Correct the timeout table in sections-1-4.md:33. Pi's 2 s is oboete's own guess (agent-adapters-2026-09-23.md:295), not a Pi limit. Claude Code's 60 s has no source in the repo; m1.md:257 records 1.5 s for SessionEnd. Cite each figure or drop it.
  - Set the hook's budget ("tens of ms", M22's 300 ms) from measurement, not from the old budget that included a 150 ms Workers AI call (search-sync-proposal-2026-09-23.md:180).
- Evidence: the live-rerank tier contradicts SHOULD item S3 ("the injection hook keeps plain RRF", improvements-synthesis.md:441) and section 4's first line. No requirement asks for it.
- Cost: delete one bullet.
- Decides it: no reranker call on any hook path, and hook p95 is the same across tiers.

**S4-6 + S4-4: the hook never embeds a query.** Verdict: adopt.
- Change: before the worker is up, the cold path is FTS plus the threshold, or the SessionStart packet's ranked claims. The worker builds the shortlist in two stages: first a provisional one from plain hybrid RRF (the bit-index scan takes 20-80 ms at 150k documents, search-sync-proposal-2026-09-23.md:144), then the reranked, judge-scored one.
- Evidence:
  - sections-1-4.md:31 ("hooks never wait on the network") contradicts :32 ("vectors").
  - There is no local embedder (src/config.rs:19), so query vectors mean a Workers AI call.
  - The cold path is the normal case, not a rare one: the WSL VM shuts down when idle, and the worker exits after 30 idle minutes.
- Cost: less code than now.
- Decides it: across 50 cold starts, the first prompt's hook p95, and injection precision of the provisional against the reranked shortlist on the 50/30 calibration set.

**S4-12: Grok gets corrections on every turn, not only the first.** Verdict: adopt.
- Change: Grok's UserPromptSubmit (registered at setup.rs:41-43) sets a pending flag, and the next PreToolUse consumes it. This reuses `reinject_pending` / `claim_reinjection` (db.rs:19,565-573).
- Evidence: today `sessions.injected_at` fires once per session (hook.rs:425-436), and the test at hook.rs:2285 asserts "never again".
- Cost: about 2 tests.
- Decides it: extend `grok_camelcase_fields_and_resume_without_injection` so that a second correction arrives in the same session.

**S4-10: gate on a confidence bound, not a point estimate.** Verdict: adopt. Note that this is stricter than the literal wording of your 10% line.
- Change: turn per-prompt injection on by default only if the one-sided 95% Wilson upper bound on irrelevant injections is 10% or less. Grow the two pools from 50 and ~30 toward about 100 each.
- Evidence: 3 wrong out of 30 reads as a 10% point estimate, but its upper bound is about 23%. LongMemEval uses its 30 abstention items as one slice of 500 questions, not as a gate on its own (arXiv:2410.10813).
- Cost: one formula and about 100 more questions; the no-answer questions cost nothing to write.
- Decides it: apply the bound to your dev data and see whether the ON/OFF call changes.

**S4-15: reranker on the CLI too.** Verdict: small, adopt only if S3 passes.
- Apply the same evaluation gate to the CLI. The CLI, MCP and viewer all call `search::find` (main.rs:213, mcp.rs:134, view.rs:305).
- One skeptic refuted this as "locked by decision 11", but S3 is a SHOULD item, not one of the 23 MUST items (improvements-synthesis.md:421,439).
- A one-shot CLI loads the reranker cold on every call, so S3's RSS measurement applies.

**S4-1: the retraction race.** Verdict: *weak as written*; drop the proposed mechanism.
- Comparing the shortlist's as-of time with raw's seq would fire almost every time, because curation always lags raw. S1's `reinject_pending` covers corrections to claims that were already injected.
- *My addition, not tested by the skeptics:* a claim retracted after the shortlist was built can still be injected once before the next refresh. The read-time check below doesn't have the original gate's problem of firing constantly. The fix is for the hook to check each shortlist uid's status at read time (about 50 indexed lookups, under 1 ms), or to refresh the shortlist when the curator commits a retract or supersede. M3's hard line (retracted claims injected = 0%) tests this.

**S4-19: manifest signals as a ranking feature.** Verdict: LATER, as an E-series experiment. The qrels have no manifest fields and no manifest code exists yet, so this needs new fixtures first.

### 2. Inherited constraints that are still right (checked)

- **S4-2 / S4-5** A precomputed shortlist, not a live search of the whole corpus per prompt. The live-search alternatives put computation that grows with the corpus back into the hook, the shape you caught.
- **S4-3** Refresh at Stop, every N events and on sync. Set N by measurement, instead of adding a topic-drift trigger with a second threshold to tune.
- **S4-8** About 10 claims with bodies, then an index. claude-mem's index is also a count (CLAUDE_MEM_CONTEXT_OBSERVATIONS); a cap on each body's length does the rest.
- **S4-9** Imports are never injected. This is your decision (search-sync-proposal §0/§6), and claude-mem's rows have no raw anchor. First count how many rows could have one before building a promotion path.
- **S4-11** Re-inject after compaction, with a per-agent status table. This is R01/R04, and M10 is costed.
- **S4-14** The hybrid FTS5 + bge-m3 + RRF search. Test-split margin +0.102 against a +0.03 bar. since/until is still unimplemented (R06; M12 adds it).
- **S4-16** No file or proxy delivery channel. Addition: state the real reasons in the text. Deletion can't reach repo history. Memory in an instruction file stops being fenced as data. And on public repos one user's memory would leak.
- **S4-17** The claim is the ranked unit. The diversity pass was rejected with a reason (improvements-synthesis.md:569), and chain tips already fold duplicates together.
- **S4-20** The SessionStart packet is read from disk even when the worker is down, and M5's run with no worker at all already covers a cold start.
- **S4-23** Search reads the local replica: this works offline, and the Workers AI reranker scores below BM25 on JQaRA. Optionally, `search --remote` on a newly set-up device until its first sync finishes, only if that first sync misses the 10-minute install line.

### 3. Needs your decision

1. **S4-13: resume from another device's manifest.**
   - Goal 3 says to resume across devices, but only a 30-minute "also active" line crosses devices today.
   - One skeptic pointed out that WSL and Windows are used side by side, so letting another device's live manifest replace your own would mislead.
   - **Recommendation:**
     - Your own manifest stays primary.
     - Add a labelled block "last worked on from <device>, 3 h ago" only when that device has been idle for more than 30 minutes and its manifest is newer than yours.
     - Carry the resumable fields only: failing command, todo list, last prompt. Never another clone's git state.
   - Yes or no?
2. **S4-21: an evaluation gate on data that isn't yours before public release** (decision 8).
   - All defaults are tuned on your Japanese-heavy data. A check on the public JQaRA/JaCWIR test splits would catch defaults that collapse on other people's data.
   - The real cost is about 0.5-1 PR, not "near zero": there is no importer for passages plus qrels, and JQaRA measures the model more than oboete's pipeline.
   - **Recommendation:** yes, run it before release as a sanity line, not as a tuning set.
3. **S4-10: a stricter reading of your 10% line** (FYI). It may keep per-prompt injection off for longer. Say if you prefer the literal point estimate.

---

## Order I would do this in

1. Doc fixes, no code, all today: S1-21, S3-12, S3-7, S3-6, S3-4, S3-20 (`pref add`), S4-7, S4-6, S1-10/S2-18.
2. Small code, inside PRs already planned: S4-12, S2-25, S2-21, S3-22, S3-23, and the ledger half of S2-2.
3. Measurements before any number goes into the spec: S2-5 (FULL, 256 KB redaction, fullfsync), then the S1-4 dictionary and S4-10's bound.
4. After your answers: raw outside the device, S9, S4-13, S4-21.