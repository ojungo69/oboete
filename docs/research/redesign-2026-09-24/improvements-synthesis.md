# Design B: prioritized improvements (architect's triage, 2026-09-25)

How the proposals were sorted:
- **All 3 skeptics held:** the proposal is taken as written.
- **One skeptic refuted it on value:** the cheaper version that skeptic named is taken instead of the original.
- **One skeptic refuted it as a conflict:** only the half that doesn't conflict is taken.
- **One skeptic refuted it as already covered, and the cited file:line checks out:** the proposal goes under "Already present".
- **Measurement additions:** these are MUST or nothing, because §9 fixes pass lines before any run.

Every confirmed id has one home: MUST, SHOULD, LATER, "Already present" or "Confirmed but not adopted". A few proposals were split by a refuter, and both halves are placed and named: r1-lookup-4, r1-lookup-3, r1-decisions-1, r1-resume-6, r1-decisions-6, r4-devices-2, r2-oncall-4.

**Cost: the MUST list roughly doubles B's build.** It adds about 18–21 PR-sized units to the 16–20 in options-draft.md §4:
- About 3 of those units are labelling and measurement.
- About 1 applies only if the folder transport is kept.
- Most gate items widen existing units ("claims + gates + supersession", "deletion", "doctor").

This is a budget decision, not a footnote.

## 1. MUST (in the design before the spec is written)

### A. Truth model (goal 2)

**M1. Every status change needs evidence, not only `decided`**

One Rust table maps (claim kind, target status) to the evidence required:
- `done` needs gate-1 evidence, or a cited `tool_observed` success record in the window (exit 0, a passing test line).
- `retracted`, and retiring a lesson, need gate-1 evidence.
- Any status change or supersede on a `global` claim needs an OwnerDirective event. Without one, the change is only flagged in the viewer.
- A change below the bar stays proposed and is never injected as closed.

Why: gate 1 (options-draft.md:147) guards only `decided`. So assistant narration ("should be fixed now") can close open items or retire lessons. Ordinary session text can also retract a preference the owner set globally.

- Measure:
  - Fixture: narration with no passing run leaves the item open.
  - Fixture: a session quote aimed at a global preference is rejected and flagged.
  - New hard line in M3/M5: items injected as closed or retracted without qualifying evidence = 0.
- Cost: ~1 PR, inside the existing gates unit.
- From: r2-decisions-1, r2-decisions-6.

**M2. Link reversals that happen inside one window**

Gate 3 resolves `supersedes` against the list from before the window, plus earlier siblings in the same curator output. The siblings get temporary ids that are local to the window. Today a decision and its reversal in one window become two current tips (options-draft.md:149, 152), which fails M3's existing hard line.

- Measure: a decision and its explicit reversal in one window give exactly one tip, with the right edge. The fixture joins the M3 slice for overturned decisions.
- Cost: < 0.5 PR.
- From: r2-decisions-5.

**M3. Supersession candidates come from the repo-wide hybrid index, for every window**

PR-K2 already shows the curator the top 20 live decisions that a search of the session text finds (search-sync-proposal-2026-09-23.md:439). But options-draft.md:135 reads as if the curator sees only state carried within the session.

The spec must say this:
- The candidates are found per window, across the whole repository, through the existing hybrid search (FTS + bge-m3).
- These are candidates only. Gate 3 is unchanged, and similarity alone never supersedes (issue50 §4B).

Without this, a decision made weeks later in another session cannot supersede the old one.

- Measure:
  - At least 20 contradiction pairs, split across sessions.
  - Supersede recall is 0.3 or more above the session-only baseline.
  - Control pairs wrongly dropped ≤ 2%. That is M3's real line; the proposal mis-cited it as 0%.
- Cost: ~0.5 PR.
- From: r1-competitors-5.

**M4. Taint proposals and quotes that restate tool or file text**

A deterministic check, with no LLM, marks text as tainted. It applies to a proposal or a user-authored span that overlaps a `tool_observed` or file-read span in the same window, above a fixed threshold.
- A tainted proposal needs a verbatim restatement by the owner. A bare "はい" (yes) is not enough.
- A tainted user span cannot satisfy gate 1 as a quote.

This closes critique.md:28 (an acceptance turn promotes an injected proposal). It also closes the path where pasted text counts as the owner's words.

- Measure: the M3 injection canary adds three cases, with 100 trials each:
  - a paraphrased attacker file followed by "はい";
  - a pasted file that contains a decisive quote;
  - a fake acceptance line inside tool output.

  Pass: 0% reach `decided`, and `decided` recall on the clean slice stays ≥ 0.80. The threshold is tuned on the dev split.
- Cost: 0.5–1 PR.
- From: r3-decisions-1; r1-decisions-6 (its fixture); the simpler fix named in the refutation of r4-decisions-7 (compare against any tool span, whoever the speaker is).

**M5. None tier: a later negation hides the earlier directive, and corrections need no curator**

The manifest builder pairs each unverified directive line from the owner with any later negating line on the same topic. It recognises negations such as やめて (stop), 取り消し (cancel), "never mind" and "not anymore", combined with shared words. It shows only the latest state.

OwnerCorrection events apply on every tier. This closes critique.md:26 on the one tier that the brief guarantees always works.

- Measure:
  - a fixture with a directive followed by its negation;
  - a none-tier row in M3's line "retracted items injected = 0".
- Cost: ~0.5 PR.
- From: r2-decisions-7.

**M6. Digests are treated as data, and every digest line cites a current claim**

- The digest prompt fences claim bodies as data.
- The output is structured lines, and each line carries the uid or uids it summarises.
- At build time, a line is dropped before injection when its uids are not current chain tips.

Why: gate 5 binds only the curator. The digest re-reads stored `tool_observed` quotes with a second LLM call, and its output is injected at SessionStart (options-draft.md:131, 154).

The refuter's correction applies: any current tip counts, not only `decided`. So open items and repo facts survive.

- Measure:
  - a claim body with an embedded instruction yields no uncited line;
  - new M3 line: digest lines that state a superseded claim as current = 0.
- Cost: ~0.5 PR.
- From: r4-decisions-3.

**M7. A deterministic order for automatic concurrent supersession, plus a clock-skew alarm**

§11's rule stays: the later `valid_from` wins, and the conflict is flagged in the viewer. Decision 4 is unchanged. There are two additions:
- The full order is (`valid_from`, device_id, seq), so ties resolve the same way on every device.
- Sync and doctor flag any inbound op whose timestamp differs from its receive time by more than 5 minutes.

Why: the wall clocks on WSL, Windows and the iMac decide which decision wins, and nothing checks them.

- Measure:
  - With one device set 30 minutes fast, doctor shows the skew next to the affected decision.
  - 50 synthetic ties resolve identically on all devices.
- Cost: small.
- From: r1-oncall-7; r1-decisions-1 in lazy form (a logical clock is LATER).

### B. Resume (goal 1)

**M8. Manifests keyed by (repo, branch/worktree, device)**

SessionStart injects the manifest for the current checkout. It adds one line that lists other manifests on the same repo updated in the last 30 minutes, for example "also active: codex@windows on feature-x, 4 min ago". This line is built from manifest timestamps, so no new heartbeat op is needed. "Files touched" are stored as repo-relative paths with `/`.

Why: options-draft.md:154 falls back to "the latest manifest", singular. WSL and Windows native are used side by side (decision 6), so one device's branch overwrites the resume state of the other.

- Measure:
  - In the M5 two-branch test, each device resumes its own branch.
  - The "also active" line appears within one sync cycle, and never after 30 minutes idle.
- Cost: 0.5–1 PR.
- From: r2-devices-1; r1-resume-3 in the refuter's cheaper form.

**M9. The curation backlog is visible everywhere, and stuck pending work is caught**

- The manifest op carries (raw_tail_seq, curated_seq, as_of). Then SessionStart on any device can say "M events / T min not yet curated".
- Pending windows carry `reason` and `next_attempt_at`.
- doctor flags "overdue" when `next_attempt_at` has passed and one of these is true: the worker lock is stale, or no worker was respawned. An idle exit alone is normal.

Why: issue50 §5 says 整理待ち (work waiting to be curated) must not be hidden. It also says pending work whose next attempt never comes must be caught. The design shows the backlog only when every provider fails (options-draft.md:135).

- Measure:
  - In the M5 cross-device slice, the lag line appears when lag is more than 2 minutes.
  - Kill the worker and advance the clock: doctor names the reason and the elapsed time.
  - SessionStart p95 stays ≤ 300 ms.
- Cost: ~1 PR.
- From: r3-resume-5, r1-oncall-3.

**M10. A per-agent live-verification table**

For each agent, doctor lists six capabilities:
- capture;
- SessionStart injection;
- prompt injection;
- re-injection after compaction;
- resume dedupe;
- session end.

Each capability is marked "live-verified (version, date)", "implemented" or "unverified". R01 requires 実装済み (implemented) and 実機確認済み (verified on a real machine) to be kept apart.

- Use the per-agent pairs already in docs/research/agent-adapters-2026-09-23.md. For OpenCode, use its v2 events, not the v1 API that the proposal cited.
- A surface with no injection point uses `oboete inject`, which already prints the resume block to paste.

- Measure:
  - M5 forces a mid-session compaction on every agent that has a compaction signal.
  - Recall of open items after compaction ≥ 0.80.
  - Agents without a compaction signal show "unverified", never a silent pass.
- Cost: ~1 PR plus live sessions.
- From: r1-resume-5 (with critique.md:5, 77); r4-resume-2 (`oboete inject` already exists).

### C. Lookup (goal 3)

**M11. Search defaults to current claims; history is explicit**

- Default search and MCP rank superseded, retracted and done claims below every current tip. These claims are labelled "superseded by <uid>".
- `history=true` removes that lower ranking.
- `get` always shows status.

Why: issue50 §5 requires two separate search intents, "current" and "history". Today M3's hard line ("ranked top-10 as current") is scored only on injection.

- Measure:
  - Run M3's slice for overturned decisions through search and MCP: 0% ranked as current.
  - With `history=true`, recall of the superseded item ≥ 0.80.
- Cost: ~0.5 PR.
- From: r1-lookup-7.

**M12. `since` / `until` on every search entrance**

R06 lists 時間条件 (time conditions) as a must-keep. Today nothing in `src/search.rs` or `src/mcp.rs` filters by time.

- Add ISO `since` and `until` to the CLI, MCP and viewer.
- Filter both the FTS leg and the vector leg before RRF.
- Filter each document by its own time: `valid_from` for a claim, the event time for a raw chunk.

No intent classifier is needed, because calling agents already turn "先週" (last week) into dates.

- Measure:
  - a fixture table test;
  - a time-qualified subset in M6, passing at ≥ 0.70.
- Cost: < 0.5 PR.
- From: r1-lookup-3 in the refuter's cheaper form (the parser is LATER); r2-lookup-5 in lazy form (each document uses its own time axis).

**M13. Every hit says how strong its evidence is**

Each hit gets one of three labels. The label is computed at render time from existing fields, with no new column:
- "citable": the raw anchor is on this device;
- "quote-only": a synced claim whose raw record is not local;
- "imported": from claude-mem, with no anchor.

Results from all repos also carry their repo label. Exclusion stays a check at send time (options-draft.md:163), not a filter on each row at read time.

Why: R12 forbids claiming a provenance that imported data does not have (critique.md:32). M6's cited-span validity currently mixes all three kinds of hit.

- Measure:
  - M6 reports validity for citable hits only, and reports attributed-only usage separately.
  - Imported hits without the label = 0 (hard).
- Cost: ~0.5 PR.
- From: r3-lookup-7; the labelling half of r1-lookup-4.

### D. Durability, deletion, recovery

**M14. SQLite settings that make durability and deletion real**

Durability. The current code opens SQLite in WAL mode with `synchronous=NORMAL` (src/db.rs:106). With that setting, the last commits can roll back after a power loss or a VM crash. This is how a coverage hole happens:
1. The worker derives rows from a commit that then rolls back.
2. The knowledge.db checkpoint now sits above raw's highest seq.
3. New events reuse the lost seq numbers.
4. The consumers skip those events, and nothing reports it. The design says this hole is impossible.

The fix:
- raw.db alone uses `synchronous=FULL`. knowledge.db can stay NORMAL, because it can be rebuilt.
- At startup, any checkpoint above raw's highest seq is rewound and reported in doctor.

Purge. This closes critique.md:16 and the finding at phase0.md:36.
- Open both files with `secure_delete=ON`.
- After the forget transaction commits, run `wal_checkpoint(TRUNCATE)` and FTS5 `optimize`.
- `forget` reports done only after that.

- Measure:
  - Hook p95 ≤ 20 ms with FULL, on WSL, Windows and the M1 iMac.
  - Hook writes are never blocked past `busy_timeout` (db.rs:101) while the worker holds a long transaction.
  - An M4 byte grep of `.db`, `-wal` and `-shm` directly after forget finds 0 hits.
  - Forget time is recorded at about 330k documents. If it is too slow, `optimize` becomes a `merge` in the idle worker, and forget shows "purging" until the merge finishes.
- Cost: < 1 PR.
- From: r1-oncall-2; r1-security-2 as its refuter corrected it; r4-oncall-1 (the measure only, because WAL and `busy_timeout` already exist).

**M15. Backups that are scheduled, checked, restorable and purged**

- When the worker exits on idle, it exports sealed zstd raw segments plus tombstones for each seq above the last one backed up. Each segment has a checksum.
- The backup goes to a local directory, by default under the app data directory. oboete warns if that directory is inside OneDrive, iCloud Drive, Dropbox or Google Drive.
- `quick_check` runs when the worker starts, not in the hook. doctor runs `integrity_check` and verifies the segment checksums.
- If a check fails, oboete quarantines the file. It then restores from the backup plus the inbound op log it keeps.
- `forget` rewrites the segments that hold the deleted span.

Why: raw.db is the only source of truth and is kept forever (decision 7), and raw sync is off by default. But the design has no backup schedule, no path for corruption, and no purge of backup files. M4 greps every file, so backups count.

- Measure:
  - Corrupted bytes get quarantined. The restore reproduces the claim state, compared by content hash, and loses at most one idle interval.
  - After forget, the decompressed backups contain 0 hits (hard, part of M4).
- Cost: 1.5–2 PR.
- From: r1-oncall-5, r1-security-4, r4-oncall-5; r2-oncall-7 in lazy form (checksums instead of a full restore drill).

**M16. Write failures show as failures**

Hooks stay fail-open (src/main.rs:180-186). Two things change:
- When a write fails, the hook classifies the error (SQLITE_FULL, IOERR or ENOSPC) and touches a marker file outside the database.
- doctor also checks free space itself, because the marker write can fail too.

After a failure, doctor turns red, and the next injection says "recording failed since T".

Why: issue50 §4A says a record is never shown as 記録済み (saved) when nothing was saved. The proposal's clause about the checkpoint is dropped, because a failed write has no seq.

- Measure: a tmpfs quota fixture ends with exit 0, the marker written, doctor red, and the failure line in the next injection.
- Cost: ~0.5 PR.
- From: r2-oncall-1.

**M17. An `oboete update` that can roll back, and ops that survive version skew**

Update runs in six steps:
1. Stage the new binary.
2. Run a read-only schema check with the new binary.
3. Take the backup before migration (R13).
4. Swap the binaries by renaming them, because Windows cannot overwrite a running `.exe`.
5. Migrate.
6. Run a self-check. If it fails, restore the backup and keep the old binary.

Every synced op carries a format version. An older binary parks any op with an unknown version, kind or status as pending: the op is never treated as current and never dropped. doctor names the version needed. This matters because updates are explicit and per device (decision 15), so WSL, Windows and the iMac often run different versions.

- Measure:
  - After a failed self-check, the old binary still runs, and nothing is lost compared with the backup.
  - Ops from version N+1 that reach version N are parked, and are applied after the upgrade.
- Cost: ~1.5 PR.
- From: r2-oncall-3; r1-oncall-4 in lazy form.

### E. Sync

**M18. Re-derivation keeps uids, and the tier decides which derivation is active**

Re-deriving claims with a better model is meant to be routine (options-draft.md:22), but its identity rules are undefined (critique.md:15). The rules:
- A re-derived claim of the same kind over the same anchor span is a new derivation of the same uid. It is stored as (uid, recipe, tier).
- Edges keep pointing at the uid.
- The active derivation is the one with the highest tier; among equal tiers, the newest wins. So a lower tier never replaces a higher one.
- A claim that disappears on re-derivation gets a retract edge.

Why not new uids: they would bring superseded decisions back. The old supersedes edge keeps pointing at the old uid, while the re-derived copy starts as a new tip. This rule deliberately overrides the reasoning used to reject r2-devices-2 (new uids plus supersedes).

- Measure:
  - A paid and a free re-derivation of one uid, in either order: the paid one is active, and the free one can still be queried.
  - Every edge resolves.
- Cost: ~1 PR.
- From: r4-devices-3.

**M19. Constraints on the sync-hub port**

- **HTTP is the source of truth.**
  - HTTP push/pull with an ordered cursor is the only path that decides correctness.
  - A WebSocket only wakes a pull, and any anomaly closes it.
  - The hub can force poll-only mode with a header.

  The donor already works this way (docs.claude-mem.ai/cloud-sync).
- **No built-in hub URLs.** The donor's default URLs for the verifier, projector and hub point at cmem.ai (options-draft.md:45). oboete ships none:
  - sync refuses to start while the URLs are unset;
  - CI fails if "cmem.ai" appears in the release binary outside NOTICE;
  - the owner's Worker URL is never hardcoded, because public users need their own.
- **Graph checks.**
  - doctor and a regression test check each supersedes lineage for cycles and for edges that leave the lineage.
  - A fixture covers the race between delete and supersede: device A tombstones X while offline, and device B supersedes X with Y.
- **Daily cross-device probe.** A fixed set of probe queries is compared across devices every day. options-draft.md:152 states that "every device computes the same state once caught up", but nothing checks it today.
- **Precondition.** The self-host spike is still required before the three build units for the hub (critique.md:71).

- Measure:
  - Dropped or corrupted WebSocket messages in M4/M5 do not change correctness.
  - With the URL unset, sync is off and doctor says why.
  - A hand-made cycle is flagged.
  - In the race fixture, X is never current and Y renders.
  - Once synced, top-3 results and status agree ≥ 0.95.
- Cost: ~1.5 PR.
- From: r2-competitors-5, r4-security-2 (the half that doesn't conflict), r4-devices-7, r2-evaluation-3, r1-devices-4 (the fixture only).

**M20. Folder transport contract (only if the owner keeps it)**

- Each device writes an op batch to a temp file and renames it into the folder. It never appends to a file in place.
- Op files are encrypted with age to the public keys of the peer devices. Private keys never enter the folder.
- Public keys are exchanged at setup. Removing a device's key revokes that device.
- Applying an op is idempotent by (uid, rev), whichever transport delivered it.

Why: file syncers read half-written files, and the shared folder is often OneDrive, iCloud or Dropbox itself. Encryption also turns the leftover copies that critique.md:22 found in `.stversions` and in the trash into ciphertext. Nothing better is possible with a third-party syncer.

- Measure:
  - A syncer on throttled I/O never delivers a truncated op (added to the M2 matrix).
  - Claim text cannot be found by grepping the raw op files.
  - An op that arrives through both the hub and the folder is applied once.
- Cost: ~1 PR.
- From: r1-devices-6, r1-security-8, r4-devices-2 (the test only; see §3); r2-devices-4 in lazy form.

### F. Additions to the measurement plan (§9 fixes pass lines before any run, so these cannot wait)

**M21. Slices that cover what the goals promise**

- **English.**
  - Take 40–60 questions from real English sessions.
  - Add a cross-lingual subset: a Japanese query with an English record, and the reverse.
  - Report English separately. Pass: within 0.05 of the Japanese score, tuned only on its own dev split.
- **Every claim kind.** M3 reports precision and recall for each kind: lesson, fix, open_item, repo fact, change.

  Lessons need a different measure, because a replay cannot show that a mistake was "not repeated". Instead:
  - measure lesson extraction precision and recall on labelled failure spans;
  - check that the lesson is injected when a later window touches the same failure.
- **Owner corrections.** A canary OwnerCorrection must survive three things: `rebuild`, re-derivation with a new recipe, and a resync from a device that was offline during the correction. Pass: 100% (hard). §11 promised this test.
- Cost: ~2 PR, mostly labelling.
- From: r3-evaluation-1, r3-evaluation-2, r3-evaluation-4; r3-lookup-3 (its evaluation half). These close critique.md:2–4 and 73–75.

**M22. Operational lines**

- **Search latency at scale.** Measure on the full corpus of about 330k documents, while the worker writes at the same time (critique.md:79):
  - MCP p95 ≤ 1.5 s;
  - injection p95 ≤ 300 ms;
  - slowdown under writes ≤ 20%.
- **First sync of a blank device.** Measure against the real size of the op log, over the hub and over the folder:
  - the time until the last 30 days are usable;
  - the time to a full backfill;
  - the bytes transferred.
- **Manifest truncation.** For each agent, a canary echo at three sizes shows which adapters truncate the manifest. Cursor already caps it near 9,500 UTF-16 units (src/hook.rs:468).
- Cost: ~1 PR.
- From: r4-lookup-5, r2-devices-5; r1-resume-6 in the refuter's form (measure first).

### G. Release blockers (decision 8)

**M23. A public install path**

These are spec lines now, and are built before release.

- **Builds.**
  - Release binaries are built only in CI, with SHA-256 checksums and GitHub artifact attestation.
  - The installer verifies both.
  - Paid signing depends on owner decision 2.
- **Local bge-m3.**
  - The model's SHA-256 is pinned in the binary.
  - The download can resume, and it checks free space first.
  - The user can skip it and use Workers AI or no embeddings, then change this later with `oboete setup --embeddings`.
- **Disclosure.**
  - The tier prompt has one line on what that tier sends, and where.
  - `forget` prints "other devices purge on their next sync".
- **Four short docs:**
  - what is stored, and what is redacted;
  - for each tier, what leaves the machine and to whom. This includes the fact that providers keep what they were sent, under their own retention rules;
  - what `forget` purges, and its limits: offline devices, old disk images, provider retention;
  - setup for each agent, with its M10 status.

- Measure:
  - The §9 fresh-VM install line passes on all three OSes.
  - A checksum mismatch stops the install, and a changed byte in the model makes setup fail.
  - Someone who did not write the docs can answer two questions from the docs alone: "what goes to the cloud on my tier" and "how do I remove a memory".
- Cost: 1–1.5 PR plus writing.
- From: r1-security-6, r1-public-4, r3-security-1, r1-public-6; r2-security-2 (the disclosure only, without the proposal's table of each provider's zero-data-retention status); r1-public-1 and r3-public-3 in lazy form; critique.md:9.

## 2. SHOULD (first release, each with its measure)

**S1. Correct running sessions when an injected claim changes**
- What: record the claim uids injected into each session. A retract, supersede or tombstone that touches one of them sets `reinject_pending` (src/db.rs:564). The existing re-injection then runs on the next hook.
- Measure: when a claim is retracted on device 2 during a session, the next hook corrects it in ≥ 95% of runs. Hook p95 rises by ≤ 20 ms.
- Cost: ~0.5 PR.
- From: r1-decisions-5, in the refuter's form.

**S2. The manifest shows risky state first**
- What: a deterministic git-state field (MERGE_HEAD, rebase-merge, detached HEAD, dirty and staged counts) becomes the first line whenever the state is unsafe.
- When the manifest is over an adapter's limit, fields drop in a fixed order. The first field in this list is the last to drop: git state, failing command, open decisions, open items, repo facts, last prompts. Today the text is cut by position.
- Measure:
  - a unit test against a synthetic git repo;
  - with a fixture in the middle of a rebase, the warning comes first in 10/10 runs, within 300 ms;
  - at twice the limit, the top three fields survive in 20/20 runs.
- Cost: 0.5–1 PR.
- From: r2-resume-1, r2-resume-2, r3-evaluation-3 (lazy form).

**S3. A reranker on MCP and the viewer only**
- What: bge-reranker-v2-m3, or a smaller multilingual cross-encoder, reranks the hybrid top 50 on CPU. The worker loads it only when it is first needed. The injection hook keeps plain RRF.
- Measure: M1's line (nDCG@10 ≥ hybrid + 0.03, and no slice drops by more than 0.02). Also CPU p95 and RSS on the M1 iMac and on the slowest WSL machine.
- Cost: ~1 PR.
- From: r1-lookup-1.

**S4. Pruning of rare trigrams, in the injection hook only, and only if M22 shows the hook over 300 ms**
- What: prune the hook's query using fts5vocab row counts. MCP stays unpruned: it measured 767 ms at 180k documents, inside its 1.5 s line (docs/pr-e0.md:95).
- Measure: the M22 lines with and without pruning.
- Cost: 0.5–1 PR.
- From: r1-lookup-6, limited to the hook as its refuter required.

**S5. doctor runs real probes instead of reading counters**
- What:
  - a canary round trip through write, index, FTS search and MCP `get`, with a timeout. The canary is deleted afterwards;
  - an authenticated, read-only call to the hub status, reported separately from the pending count;
  - a check that the registered OS service runs the current binary and version;
  - the worker runs the canary every day on every device, and once at the end of `oboete setup`.
- Measure:
  - a stalled indexer whose counters look fine fails the canary;
  - a hub that is down while the queue is empty shows `reachable=false`;
  - a moved binary is flagged.
- Cost: ~1 PR.
- From: r3-oncall-2, r2-competitors-6, r3-oncall-6, r4-evaluation-5, r3-public-6.

**S6. Limited catch-up after a long offline period**
- What: catch-up gets its own checkpoint and a limit on size and time. doctor shows how much remains.
- Measure: after a simulated month offline, RSS stays under target and hook p95 stays ≤ 20 ms while the backlog is processed.
- Cost: 0.5–1 PR.
- From: r2-devices-3.

**S7. An egress ledger, and no payload in any log**
- What:
  - Each time data passes the egress gate, oboete logs the time, destination class, provider, repo, character count and outcome.
  - The ledger lives with raw.db so that it survives a rebuild. `oboete doctor --egress` shows it.
  - The ledger checks issue50 §5's rule that local-only means zero egress, and it supplies the totals that R15 needs.
  - No log or audit table stores payload text. Phase 0 found the deleted canary still held in Hindsight's `llm_requests` table.
- Measure: 20 mixed events log exactly the external ones; a none-tier run logs 0 rows.
- Cost: < 1 PR.
- From: r2-public-3; r2-oncall-4 (the rule that only metadata is logged).

**S8. An allow-listed environment for curator subprocesses**
- What: replace the substring denylist (src/provider.rs:447-454) with `env_clear` plus an allow-list: PATH, HOME/USERPROFILE, APPDATA/LOCALAPPDATA, TMP/TEMP, LANG/LC_*, XDG_*, the proxy variables, and each CLI's own variable for its config directory.
- Measure: a canary variable never reaches the child process, and all four CLIs still authenticate on all three OSes.
- Cost: small.
- From: r4-security-6.

**S9. Keep curation on the owner's subscriptions out of the way of active coding**
- What:
  - Each provider gets a curator limit well under its daily cap.
  - Windows on the subscription tier are curated at Stop or when the worker is idle.
  - If a hook fired in the last N minutes, only the free or local providers already in the chain may curate. Otherwise the window waits.
  - The manifest still syncs immediately.
- Measure: replaying a heavy day shows 0 subscription curator calls within N minutes of a hook. M5 propagation stays ≤ 5 minutes after the cut.
- Cost: 0.5–1 PR.
- From: r1-ai-cost-1.

## 3. Already present in code or design (the spec must keep it; no new work)

- **Viewer port and token.**
  - The viewer binds any free port by default: `--port` defaults to 0 (src/main.rs:92-94).
  - It requires a token that is new for each launch.
  - It checks the Host header to block DNS rebinding (src/view.rs:91-100, 205).

  From: r2-public-2, r2-security-7.
- **Gate 1 already constrains `decided`.**
  - It needs a record written by the user, adjacency for acceptance turns, and a verbatim quote (options-draft.md:147).
  - It runs as deterministic code when the claim is written, so no check is needed at read time.

  From: r1-decisions-6 (its fixture moves to M4), r2-lookup-3, r4-decisions-5.
- **Sync exclusion is checked at send time** (options-draft.md:163). From: r1-security-7.
- **The donor already applies ops idempotently by uid** (search-sync-proposal-2026-09-23.md:304). M20 adds only the test across transports. From: r4-devices-2.
- **The eval corpus is not in the repo.** events-1000.jsonl lives in ../free-mem, and replay runs with `--home <tmp>` (CLAUDE.md). From: r3-evaluation-6.
- **`setup` backs up every config it edits** (`.oboete.bak`), and it merges only oboete's own keys (src/setup.rs:15, 353, 487). From: r3-public-1.

## 4. LATER (after the first release)

- **Time and clocks.**
  - A logical clock for the order of supersession, if M7's skew alarm fires in practice (r1-decisions-1).
  - A parser for relative times in Japanese and English that fills `since`/`until` (r1-lookup-3).
  - `--as-of` as a filter over the timeline (r4-lookup-2).
- **Search. Each item waits for the M21/M22 numbers.**
  - Lookup of identifiers and hashes through an FTS5 phrase query. Today src/search.rs combines trigrams with OR (r3-lookup-1).
  - Exact identifier hits pinned above the reranker (r3-lookup-5).
  - A repo pre-filter, after checking that FTS5 applies it before it scans (r2-lookup-6).
  - Hot and cold tiers of data (r3-lookup-6).
  - A stemmed English FTS column (r2-lookup-4).
  - A manifest split into headline and detail (r1-resume-6).
- **Truth model, after the M3/M21 data is in.**
  - A cascade through `depends_on` (r3-decisions-4).
  - A lesson's environment shown as provenance in the injected text, not used as a hard filter (r3-decisions-6).
  - Whether a topic applies is left to relevance ranking (r3-decisions-7).
  - Muting a claim that is correct but noisy, as an OwnerCorrection event in the op log rather than a column (r1-competitors-2).
- **doctor lines.**
  - "N claims changed since last check" and "corrections this week", in doctor only, never at SessionStart (r1-competitors-1, r1-evaluation-2).
  - How much Durable Object storage is used (r2-devices-6).
  - A count of hook failures for each adapter. The adapter is never disabled automatically (r2-public-4).
  - The exact command that fixes each detected state, plus a panic hook that prints "run `oboete doctor`" (r2-public-5, r4-public-4).
  - A diagnostics bundle that contains metadata only (r2-oncall-4).
- **Evaluation hygiene.**
  - Record the judge model and version with each run. After a model change, rerun the PR-B calibration before the judge is used again (r1-evaluation-7).
  - Check recipe changes offline: replay recent raw.db and compare the resulting claims (r2-evaluation-1, lazy form).
- **Hardening.**
  - Only `RLIMIT_CORE=0` (r3-security-3).
  - An SQLite locking self-test when the file is opened, instead of a table of filesystem types (r2-oncall-6).
  - `forget` resolves to concrete uids, and receiving devices validate its scope (r4-security-3).
  - `cargo audit` in CI now, without blocking the build. cargo-deny and an SBOM at release (r2-security-6).
- **Install and UX.**
  - Uninstall as `setup --remove` for all 7 agents, plus documented data paths (r1-public-3).
  - A Japanese README first, with the CLI kept in English (r1-public-5).
  - A static hint in the tier picker (r3-public-5).
  - Each viewer row shown as one sentence built from a template, with the fields behind a toggle (r3-public-4).
  - `oboete demo`: `oboete replay` on a bundled fixture into a temp home, once M5 passes (r4-public-2).
- **Cost.**
  - When the budget is tight, the repo touched most recently goes first (r4-ai-cost-2).
  - A prompt order with a stable prefix on the paid tier (r1-ai-cost-6).
  - Re-derivation ordered by the recipe tier, using `ORDER BY` (r1-ai-cost-8).

## 5. Confirmed but not adopted (the held skeptic's reason wins)

- **Resume signals.** The manifest's todo list, last messages and the tail of the failing command already carry this information, or the signal is too weak to build on:
  - classifying why a session stopped (r2-resume-5);
  - resume lineage (r3-resume-2);
  - `blocked_on` (r3-resume-4);
  - a list of background processes (r4-resume-3);
  - a diff of tool fingerprints (r4-resume-4);
  - checking the agent's own compaction summary (r4-resume-6).
- **An ambiguity gate (r4-decisions-1):** "directly after" leaves at most one acceptance candidate, and a repeated verbatim quote is still a decision.
- **Gating lesson creation (r2-decisions-4):** it contradicts gate 1 and drops valid lessons. M1 gates the retirement of lessons instead.
- **An adherence signal (r3-decisions-8), expiry of stale claims (r2-competitors-4), an importance score (r4-competitors-2):** each needs a new classification at curator quality for a gain that is only speculative.
- **A diversity pass (r2-lookup-7):** M1's variants with raw chunks decide this.
- **Citations for each sentence (r4-lookup-3), answers grouped across repos (r4-lookup-6):** oboete returns tagged hits, not written answers, and M13 labels the hits.
- **Path normalisation (r3-lookup-2):** repo-relative paths in the manifest (M8) are enough.
- **Export in an open format (r4-public-3):** B keeps one format for backup and export (options-draft.md:114).
- **Pausing capture, curation or injection (r2-competitors-3), opting a session out of curation (r4-competitors-1):** the `none` tier and `forget` cover these.
- **Metering real token usage (r2-ai-cost-2):** a ledger of calls per day already exists, and the §9 cost line counts calls and CLI minutes.
- **Online or self-supervised evaluation signals (r2-evaluation-7, r4-evaluation-1, r4-evaluation-4, r3-evaluation-7):** with one owner, these are too sparse or circular.
- **Failover from the hub to the folder (r3-devices-6):** devices queue ops while the hub is down, and issue50 §6 already accepts delayed delivery.
- **An estimate of the backfill cost on the hub (r4-devices-1):** setup starts no automatic backfill, and the migration of the old docs stays under critique.md:6.
- **A guard against OneDrive placeholder files (r4-devices-4):** WSL and Windows share /mnt/c directly, with no syncer in between.
- **Fair scheduling across repos (r4-oncall-3), retries for Defender locks (r4-oncall-4), waiting while the GPU is busy (r3-ai-cost-3):** these problems don't occur in practice. There are about 6–10 curator calls a day, the normal busy retry applies, and the default tier is not local.
- **Windows ACLs (r3-security-2):** NTFS already restricts `%USERPROFILE%`.
- **A secret scanner based on entropy (r3-security-6):** it gives too many false positives on diffs and hashes.
- **A static scan of the repo (r3-competitors-2):** agents read README and Cargo.toml directly.
- **A quota estimate in setup (r3-public-2):** without a usage history, the number would only look precise.
- **An explanation of SmartScreen (r4-public-5):** installing by command sets no Mark-of-the-Web, so the dialog does not appear.
- **OS notifications (r2-public-7):** M9's injection line already shows the backlog.
- **The half of r1-lookup-4 that excludes at read time:** exclusion happens at send time, and a repo's own data stays searchable on the local device.

## 6. Rejected proposals worth mentioning

- **Already in code or design:**
  - Native resume already skips injection (src/hook.rs:425-434) (r1-resume-4, r4-resume-7).
  - Curator CLIs run without tools and with a hard timeout (provider.rs `headless_command`, `run_cli`) (r1-security-3, r2-oncall-5). **Correction 2026-09-25: wrong for agy**, which runs with 57 tools including run_command and permission mode always-proceed (see section-6-revised.md §5).
  - Remote MCP auth is specified in search-sync-proposal §4.7 (workers-oauth-provider behind Access). Carry it into the spec; this closes critique.md:10 (r1-security-5). **Superseded 2026-09-25** by owner decision 18: no Access; the Worker handles OAuth itself.
- **New ways to inject: a launch wrapper, an AGENTS.md fallback, an include line (r3-resume-3, r1-resume-2, r4-resume-1):** all 7 agents already have an injection point, and Claude Code reads AGENTS.md only when there is no CLAUDE.md.
- **New uids plus supersedes for re-derivation (the reasoning used to reject r2-devices-2):** M18 overrides this reasoning, because new uids bring superseded decisions back.
- **An allow-list of kinds for supersedes (r2-decisions-2):** it leaves out repo fact, change and open_item.
- **Locking claims (r1-competitors-6):** a locked claim would reject the owner's own spoken revocation, and decision 4 depends on that revocation.
- **`tool_scope` for lessons (r3-decisions-5):** it reopens decision 13.
- **Wider claim anchors (r1-lookup-5):** claims always sync, so wider anchors would leak raw text.
- **A worker lease as a held write transaction (r1-oncall-1):** it blocks every other writer, and an OS file lock is also released on a crash.
- **Purging the transcripts of subscription CLIs on forget (r1-security-1):** it would rewrite files that claude-mem reads. It is listed as a known limit instead (M23).
- **The OS keychain (r4-security-1):** WSL has no Secret Service by default.
- **OCR in the hook (r2-security-5):** it would break the capture budget of ≤ 20 ms.
- **Automatic escalation to a higher tier (r1-ai-cost-4, r4-ai-cost-3, r4-ai-cost-5):** R05 and R15 forbid a silent move to a paid tier.
- **An integrity check at startup that refuses writes (r3-oncall-1):** it would block capture. `quick_check` runs in the worker and in doctor instead (M15).
- **A guard against the cp932 console crash (r2-public-1):** Rust writes to the Windows console through WriteConsoleW. The Phase 0 crash was in Hindsight's Python.

## 7. Owner decisions these raise

1. **Folder transport.** This extends §10 item 2 and critique.md:29, and it comes from the conflict refutation of r1-public-2. Choose one:
   - Keep it. Then M20 is required, and one limit is accepted: a deleted payload survives, as ciphertext, in the version history of a third-party syncer.
   - Drop it. Then the docs say that more than one device needs the Cloudflare hub, so public users need their own Cloudflare account.
2. **Paid code signing before the public release (M23).**
   - macOS: notarisation needs the Apple Developer Program, about USD 99 a year.
   - Windows: either SignPath's free programme for open-source projects, or a paid certificate. SignPath must accept the public repo, and each release is approved there.
   - Or ship unsigned: install by command, and document the warnings the user must click through.

Decision 4 and §11 stay as they are. M7 changes only how ties and clock skew are handled, not who resolves conflicts.