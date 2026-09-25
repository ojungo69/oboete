# Design B, section 5: devices and sync (revised after the audit, 2026-09-25)

Finding ids in brackets mark the bullets that changed. A to D are gaps I found while revising; they were not in the audit list. Short names used below:
- "proposal" means docs/research/search-sync-proposal-2026-09-23.md.
- "RD/" means docs/research/redesign-2026-09-24/.
- Donor paths are in the local claude-mem checkout (~/.claude/plugins/marketplaces/thedotmack, commit c4bfa45).

A copy of this text is at /tmp/claude-1000/-home-jura-projects-oboete/035bf06c-56a7-4208-83ad-48d576b8d187/scratchpad/section-5-revised.md.

## 1. Revised section 5

- **Topology** [inherited-1, inherited-2]: a star through one hub: a Cloudflare Worker plus one Durable Object (DO) with a fixed name, holding an append-only op log. One deployment serves one owner, so the donor's per-user routing (`getByName(userId)`, `X-User-Id`, workers/sync-hub/src/index.ts:13, :363) is not ported; it is still the single DO that proposal:68 chose. Devices push and pull over HTTP with an ordered cursor, and this is the only path that decides correctness (M19). The first release polls and has no WebSocket; a WebSocket that only wakes a pull (M19) is added only if M5 shows polling misses the timing line. One device needs no hub. More than one device needs it, including WSL and Windows native on the same PC (decision 12; owner question 2). The user sets the hub URL; none is built in (M19).
- **Hub scope** [inherited-6, A]: ported from the donor: the canonical op envelope with its format version (canonical-content.ts), push with a per-op ack, paged cursor pull, the device table, the epoch (do/SyncHub.ts:232), and the body and batch caps. Not ported: the cmem.ai verifier and its KV verdict cache (index.ts:9-12, :212-280); the Pro projection lane and its lease (`INTERNAL_PROJECTOR_URL` in wrangler.jsonc, projection-protocol.ts); the watchdog and Discord paging (watchdog.ts); the kill switch (kill-switch.ts), which only forces poll mode, and the first release only polls; the control-plane probe and canary; the internal admin routes (index.ts:25-31); and the per-entity revision check that refuses a whole batch on a stale or conflicting rev (do/SyncHub.ts:404-418). oboete's hub stores immutable ops and never compares revs or tiers. The donor never purges: compaction is disabled and "every canonical operation remains replayable" (do/SyncHub.ts:810-818). oboete's hub does purge (see Deletion). Decision 17 of 2026-09-23 (deletion reaches every device and the cloud, proposal:41) outranks decision 21's "follow claude-mem where they differ" (proposal:46) on this point.
- **Setup and cost** [public-1, public-3, public-4, public-5, public-6, C]: `oboete hub deploy` creates the Worker and the DO in the user's own Cloudflare account, plus the R2 bucket (raw sync) and the Vectorize index (remote search) when those are chosen.
  - Wrangler needs Node.js and npm (developers.cloudflare.com/workers/wrangler/install-and-update/). The self-host spike decides whether the binary uploads the bundled Worker through the Cloudflare API instead, so public users need no Node.
  - Plan assumption: the recommended auth (hub-issued tokens, no Access in front) runs on either plan. On Workers Free a leaked hub URL lets anyone use up the daily request quota with rejected requests, which stops sync until the daily reset; the Worker rejects them before the DO, so stored data is safe, and the hub URL carries a random path secret to make leaks less likely. On Workers Paid (the owner's plan) such a flood costs cents instead of stopping sync. The docs state this trade-off.
  - Device sync fits the Workers Free plan in steady state. SQLite DOs are available on Free with 100,000 requests/day, 100,000 rows written/day and 5 GB in total, and once a Free limit is exceeded, further operations of that kind fail (developers.cloudflare.com/durable-objects/platform/pricing/). The first push of an existing history (about 1M rows, an unmeasured estimate, proposal:342) exceeds one Free day's writes, so M22 also measures the first sync on Free.
  - Remote semantic search needs Workers Paid (USD 5/month minimum). Cloudflare's pricing page says Vectorize is Paid-only, while the Vectorize intro says Free or Paid. Either way, the Free allowance of 5M stored dimensions holds only about 4,900 bge-m3 vectors (developers.cloudflare.com/workers/platform/pricing/, developers.cloudflare.com/vectorize/get-started/intro/).
  - Nothing needs Zero Trust. Its onboarding asks for payment details even on the free plan (developers.cloudflare.com/cloudflare-one/setup/), and its mTLS is not in the Free plan (RD/hub-platform.md §3). So sync uses hub tokens and the remote MCP handles OAuth in the Worker (see Auth and Remote MCP).
  - The docs say all of this. They also say that hub data sits in the user's Cloudflare account under Cloudflare's own terms, including 30 days of DO point-in-time history. This goes into M23's "what leaves the machine and to whom" doc (RD/improvements-synthesis.md:410).
  - The self-host spike (M19) ends with a timed setup on a fresh Free-plan account, from the docs alone, by someone who did not write them. This is M23's test (RD/improvements-synthesis.md:417).
- **What travels** [inherited-4, failure-2]:
  - Content: claims (full rows); status changes and owner corrections, as events targeted by uid; manifests; digests (they cite claim uids, and staleness is computed locally); vectors with embedder_id; tombstones, withdrawals and exclusion ops; repo-touch sets; prompts (decision 2 of 2026-09-23).
  - Op identity: the hub keys every op by (origin device, origin seq), just as the donor derives ids from (kind, device, local id) (canonical-content.ts:136, :250). Two derivations of one uid therefore never collide and are never dropped as duplicates. Each claim op carries (uid, recipe, tier), and each device picks the active derivation by M18's rule: highest tier, then newest (RD/improvements-synthesis.md:304-306).
  - Size: every op is at most 64 KB. The hub refuses a larger op with 413, and the device moves it to a dead-letter list with the reason instead of retrying (proposal:378; issue #30; the donor does the same, CloudSync.ts:33-36).
  - Raw travels only if raw sync was chosen at setup (decision 14). It travels as R2 objects, never as ops (see Raw in R2).
- **Exclusion** [requirements-1]:
  - Per-repo sync exclusion is checked at send time against every repo a session touched. Content ops of an excluded repo never leave. Control ops always travel: tombstones, withdrawals, exclusion ops and touch-set updates (decision 11 of 2026-09-23).
  - An exclusion is itself an op that reaches every device. The hub refuses content ops of sessions that touched an excluded repo.
  - Before any call that sends content out (sync, embedding, curation), the egress gate re-reads the exclusion list from the hub. If it cannot, it sends nothing (issue #30; proposal:374).
  - Withdrawal: excluding a repo whose sessions already synced asks whether to withdraw them (default: keep). A synced session whose touch set grows to include an excluded repo is withdrawn automatically. The hub then purges that session's content as it would for a tombstone, including sessions from devices this one has not pulled yet. Other devices drop their copies, and the recording device keeps its own (proposal:35, :307; issue #30).
  - A withdrawal is not a deletion (R11, RD/issue50.md:45). Un-excluding makes the recording devices publish those sessions again.
  - The issue #30 tests run in its stated order: sync first, exclude afterwards.
  - The first sync lists the repos and their counts and asks for confirmation.
- **Order and conflicts** [requirements-6, failure-4]:
  - Ops are immutable and idempotent by op id.
  - Acks: a device marks an op as synced only when an ack names that op's id and hash. A change made while a push is in flight is a new op, so an older ack never covers it. The push loop advances only on acks. This is the donor's guard (CloudSync.ts:23-31, :39-41) in immutable form.
  - Tombstones and withdrawals are pushed first (CloudSync.ts:12-13).
  - Each device computes current state from the synced claims (chain tips, with M18's active derivation), so every device reaches the same state once caught up.
  - Status changes to one uid: the higher rev wins, then the device id decides.
  - A tombstone outranks every op on its target, whatever the arrival order. A session tombstone also covers that session's ops that arrive later (proposal:306, :308; issue #30).
  - A claim that supersedes a tombstoned claim is not affected. In M19's race fixture, X is never current and Y renders (RD/improvements-synthesis.md:331, :339).
  - Concurrent supersession is ordered by (valid_from = anchor time, device, seq) and flagged in the viewer. An op whose timestamp differs from its receive time by more than 5 minutes raises a clock-skew alarm (M7).
- **Version skew**: every op carries a format version. An older binary parks ops it does not understand; it never applies or drops them, and doctor names the version needed (M17).
- **Deletion** [requirements-2, A]:
  - Tombstones are replicated, carry no body, and are never compacted (RD/issue50.md:132).
  - On a tombstone, the hub removes the payload from its log and from its FTS5 rows (the donor keeps payloads; see Hub scope). It queues the Vectorize delete in the same transaction, and the DO alarm sends it until it succeeds (proposal:320). It also deletes the target's R2 objects.
  - Every inbound path checks a deny-list: sync, import, re-derive and restore.
  - An offline device purges on its next sync, with control ops first (see Catch-up).
  - Hub restore: point-in-time recovery is an API inside the DO's own code (`onNextSessionRestoreBookmark` plus `ctx.abort()`, developers.cloudflare.com/durable-objects/api/sqlite-storage-api/), and oboete ships no route that calls it. A lost or wiped hub is re-seeded by the devices pushing their own ops, since every device is a full replica.
  - Rollback guard: a device may see the hub's head below the highest seq the hub has acked to it, or a new epoch. It then pushes its whole tombstone and withdrawal set before anything else.
  - Cloudflare keeps 30 days of point-in-time history. That history still holds deleted payloads, and oboete cannot purge it. M23's forget-limits doc says so (RD/improvements-synthesis.md:411), because RD/issue50.md:132 forbids claiming that offline media is erased.
  - M4 adds a hub wipe followed by re-seeding from a device that still holds a pre-delete copy. Pass: 0 hits.
- **Raw in R2** [failure-8] (only with raw sync on, decision 14):
  - Each sealed raw segment is one R2 object, keyed by device, seq range and sha256. This follows raw.db's (device, seq) key (RD/sections-1-4.md:13).
  - Uploads go through the Worker's R2 binding, so a segment must stay under the 100 MB request-body limit (developers.cloudflare.com/workers/platform/limits/).
  - The device uploads the object first, then pushes the op that references it. The hub parks an op whose object is missing and never applies it. A retried upload writes the same key.
  - A tombstone over a span deletes by the device's key prefix and seq range, so an object whose op never landed is purged too. A segment only partly inside the span is rewritten by its recording device under a new key.
  - M4 greps R2 (RD/options-draft.md:337).
- **Catch-up** [failure-1]:
  - Every pull makes two passes over the same cursor range: control ops (tombstones, withdrawals, exclusion ops) first, then everything. The repeated control ops are no-ops because ops are idempotent.
  - So a device that was offline for months, or restored from an old disk image, purges before it shows anything new (RD/issue50.md:131).
  - A stale device that re-sends discarded content cannot revive it at the hub, because of the deny-list.
  - After the control ops, a new device gets the last 30 days first, then backfills. M22 measures the time, the bytes and the Free-plan write limit.
  - Catch-up has its own checkpoint and limits on size and time (S6).
- **Timing** [inherited-2, inherited-3, B]:
  - The worker pushes right after it commits new ops. It pulls when it starts, before it refreshes any packet, and then at an interval while it runs.
  - M5 sets the interval against the line below and the hub's request count, starting from 30 s. The interval is not carried over from RD/options-draft.md:161.
  - Line: p95 ≤ 5 minutes from the window cut on device 1 to availability on device 2 (M5, RD/options-draft.md:339). This is Claude's reading of decision 6's "a few minutes" (RD/owner-decisions.md:10). Most of that time is curation on device 1, so M5 also reports the transport share (from push to usable on device 2) on its own.
  - Remote MCP adds Vectorize's write-to-query delay: median under 30 s, p99 under 2 min (proposal:324).
  - A device whose worker is not running does not pull. Whether it pulls on a schedule (an OS service) is M5's worker-lifecycle decision (RD/options-draft.md:340).
  - The SessionStart packet and doctor show the time of the last successful pull (RD/issue50.md:131).
- **Auth** [requirements-5, public-2, public-3, public-4, failure-6] (owner question 1 settled 2026-09-25: Cloudflare stays, Access is not the default; RD/hub-platform.md):
  - Device sync uses per-device bearer tokens issued by the hub. `oboete hub device add` mints one, and the DO keeps only its SHA-256. Deleting that row revokes one device.
  - This meets the owner's rule of a per-device, revocable key that is not a full-permission key (decision 16 of 2026-09-23, proposal:40). It replaces the donor's cmem.ai verifier, which decision 21 excludes (proposal:46), and it needs no Zero Trust.
  - Tokens live in a file only the user can read. They never go into a subprocess environment or onto a command line (src/provider.rs:446-454; proposal:333, :365). This is a security-relevant path and is reviewed under rules/security.md.
  - Access is not the default. On the free public path it cannot be: Zero Trust onboarding needs payment details, and Access mTLS is Enterprise or pay-as-you-go only. On the owner's Paid account it would add a second product and expiring service tokens in front of a gate the hub tokens already provide.
  - Optional hardening for a user with a domain on Cloudflare: serve sync on its own hostname (for example `sync.<domain>`) with zone mTLS (Cloudflare-managed CA, free on all plans), a WAF rule that blocks unverified or revoked certificates before the Worker runs, and `workers.dev` turned off. The MCP stays on a separate hostname without mTLS, because the Claude app cannot present a client certificate. The owner has four zones on Cloudflare (Free Website plan, checked 2026-09-25), so this is available to the owner; which zone is used is asked when the hub is built, since one of them serves p-cipher.
  - The owner turns on Cloudflare two-factor authentication before sync starts (decision 16).
  - Revocation stops only future sync. A lost device keeps its replica, and raw.db is unencrypted (RD/constraints-synthesis.md:85, S1-3), so the replica relies on OS disk encryption. M23's forget-limits doc says so. There is no remote wipe, because a thief can keep the device offline.
- **Remote MCP for the Claude app** [requirements-3, requirements-4, public-2, failure-7]:
  - It is only for clients without a local oboete (proposal:363).
  - It goes through the hub with OAuth (workers-oauth-provider), and the Worker handles the login itself; no Access, no Zero Trust. This reverses proposal:364 (Access as the login). Login: the page asks for a short-lived, single-use approval code that an already-enrolled device creates (`oboete hub approve`). Alternative: GitHub OAuth restricted to one user id. This is a security path, reviewed under rules/security.md. Claude's decision; the owner may overrule.
  - The Claude app's connectors call the MCP from Anthropic's cloud (egress 160.79.104.0/21), not from the user's machine (support.claude.com/en/articles/11175166; platform.claude.com/docs/en/api/ip-addresses; checked 2026-09-25). Anthropic does not publish the region. The DO stays near the devices (one DO), and spike item 2 measures MCP p95 from a US host against the 1 s line.
  - Per-repo grants: search, get and timeline never cross grants, including `all` and direct ids (R09).
  - Every result is re-checked at return time against the DO's current grants, tombstones and withdrawals. So a vector that Vectorize still returns after a delete, or a grant revoked mid-session, never comes back. Local search follows the same rule (RD/issue50.md:130, :182).
  - Remote search uses Vectorize and the DO's FTS5 trigram index. The Vectorize stop line is USD 1.5 per month, checked by hand against the first invoice (proposal:14).
  - Trigram is likely available: workerd allows the fts5 module (cloudflare/workerd src/workerd/util/sqlite.c++:1334-1343), and trigram is one of FTS5's built-in tokenizers (sqlite.org/fts5.html §4.3). The hub spike confirms it with one statement.
  - If trigram is missing, remote search is Vectorize-only (RD/options-draft.md:299) and then needs embeddings on. A hub-side bigram column is LATER.
- **Other devices' manifests** [B]:
  - A labelled block appears when that device has been idle over 30 minutes and its manifest is newer (S4-13).
  - When a pull brings such a manifest for the checkout of a running session, the worker sets the existing re-injection flag (`reinject_pending`, src/db.rs:565, as in S1). The block then reaches the session at its next prompt.
  - This is the device-switch resume path: device 2's worker has just started, and its SessionStart used the packet built before the pull.
- **Donor** [inherited-6, public-6]: claude-mem CloudSync / SyncHub / canonical-content (Apache-2.0, credited in NOTICE), limited to the scope in Hub scope. The self-host spike comes before the hub build (M19, RD/improvements-synthesis.md:333). Because oboete's hub purges and the donor never does, the donor is a starting point to fork, not a wire-compatible reference; the spike's question is "can we fork it, add purge and run it without cmem.ai". It also covers the FTS5 trigram statement, the Free-plan limits on a first push, and the fresh-account setup from the docs.
- **Protocol doc**: `docs/hub-protocol.md` specifies the wire protocol (op envelope, push and ack, two-pass pull, tombstones, caps, auth). The fake hub used in client tests implements it, so the protocol, not the Cloudflare code, is the reference, and another host stays possible.
- **Spike** (RD/hub-platform.md §4): items 1 to 3 decide (Node-free deploy, latency, Japanese full-text in the DO). If item 1 fails, public users get another hub host; the owner's hub stays on Cloudflare. Items 4 to 8 are hardening and optimization (bit search in the DO to drop Vectorize is optional, mTLS, flood quota, first push on Free, OAuth end to end).
- **Later**:
  - a cloud curator as another consumer of synced raw (section 1);
  - folder transport, which would also give WSL and Windows a path without the cloud (decision 12; owner question 2);
  - the WebSocket wake, together with M19's WebSocket clause and its measure, if M5 needs it;
  - a hub-side bigram column, if trigram is missing.

## 2. What changed and why

- **inherited-1:** one fixed DO with no userId routing. Each deployment has one owner, so the donor's per-user sharding (index.ts:13) is not needed.
- **inherited-6 + A:** new Hub scope bullet listing what is ported and what is not. It records that the donor never purges (do/SyncHub.ts:810-818), so decision 17 outranks decision 21 on purging.
- **inherited-4:** ops are keyed by (origin device, seq), and claims carry (uid, recipe, tier). The donor's per-entity rev check (do/SyncHub.ts:404-418) would refuse or drop a second derivation.
- **failure-2:** the 64 KB op cap, the hub's 413 and a device dead-letter list are restored from proposal:378 and issue #30.
- **requirements-1:** restored from the issue #30 rows the draft dropped: the exclusion op, the hub's refusal, re-reading the list before each send, withdrawal, and re-publishing on un-exclude.
- **requirements-6:** the ack rule is stated in immutable form after the donor's stamping guard (CloudSync.ts:23-31), with tombstones pushed first.
- **failure-4:** the rule is now "a tombstone outranks every op on its target, and a session tombstone covers late children". The finding's "erases the superseding op" contradicts M19's pass line (Y renders), so that part was not taken.
- **requirements-2:** no PITR route ships; a rollback guard keyed on the highest acked seq; the hub's FTS5 rows are purged; the 30-day history is disclosed; M4 gains a case. Claude's decision; the owner may overrule.
- **failure-8:** new Raw in R2 bullet: the object is uploaded before its op, keys are content-addressed, an op whose object is missing is parked, and purge is by prefix.
- **failure-1:** pulls serve control ops first, in a second pass over the same cursor range, before the 30-days-first backfill, as RD/issue50.md:131 requires.
- **inherited-2 + inherited-3:** the 30-60 s poll becomes an interval that M5 sets. The 5-minute line is labelled as Claude's operationalization, mostly curation time, with the transport share reported separately.
- **WebSocket (Claude's decision):** dropped from the first release. M19's WebSocket clause and its "dropped messages" measure move to LATER with the wake, so this MUST item is narrowed, not removed.
- **B:** the worker pulls when it starts; the "last pull" time is shown (RD/issue50.md:131; the draft had dropped it from RD/options-draft.md:161); the other device's manifest arrives at the next prompt through reinject_pending.
- **requirements-5 + public-2 + public-3 + public-4:** devices use hub-issued tokens, and the remote MCP's OAuth login is handled by the Worker with a device-issued approval code, so nothing needs Access or Zero Trust (USD 0, but payment details are required, and Access mTLS is not in Free). Zone mTLS is optional hardening for users with their own domain.
- **failure-6:** revocation is stated to stop only future sync. Lost-device protection is OS disk encryption, stated in the docs, with no remote wipe. Claude's decision; the owner may overrule.
- **requirements-3 + requirements-4:** remote results are re-checked at return time against grants, tombstones and withdrawals. This covers the Vectorize delay and revoked grants.
- **failure-7:** the fallback is named (Vectorize-only, which needs embeddings on; bigram column LATER). The workerd source makes trigram likely.
- **public-1:** the plan requirements are stated. Free is enough for device sync, and remote semantic search needs Paid. Cloudflare's own docs contradict each other on Vectorize. The 100k rows/day limit slows a first push on Free.
- **public-5:** the docs disclose that hub data, including the 30-day history, sits under Cloudflare's own terms.
- **public-6 + C:** the spike gains a fresh-account, docs-only setup test on Free, and decides between wrangler (which needs Node.js) and a direct API upload.
- **failure-5 (refuted):** the stop line is reworded as a manual check of the first invoice (proposal:14), which is the mechanism the refuters cited.

## 3. Owner questions

1. **Device auth: hub-issued tokens, or Cloudflare Access service tokens as in proposal §4.4?** Settled 2026-09-25: hub-issued tokens, and no Access anywhere (RD/hub-platform.md). The owner asked for Access as the default if it were better and for other platforms if better; neither holds. The original arguments:
   - For hub tokens:
     - Zero Trust onboarding asks for payment details even on the free plan (developers.cloudflare.com/cloudflare-one/setup/). That works against decision 8 on the basic multi-device path.
     - Access service tokens expire after the duration chosen when they are created.
     - Decision 16's rule holds either way.
   - Against hub tokens:
     - With Access, "every request is checked before your Worker runs" (developers.cloudflare.com/workers/configuration/cloudflare-access/).
     - Without Access, anyone who knows the workers.dev URL can use up the Free plan's daily request quota with rejected requests. That stops sync until the daily reset (DO pricing page).
     - oboete owns an auth path that faces the internet.
2. (Settled, not asked: decision 12.) WSL and Windows on one PC sync through the hub like any two devices; while the PC is offline they drift apart until it reconnects. Folder transport stays on the post-release list.
   - In the first release, WSL and Windows sync through the hub like any two devices. While the PC is offline, they drift apart until it reconnects.
   - An op exchange over /mnt/c would be M20 without encryption: atomic rename, apply-once across two transports, and purging op files on a tombstone (RD/critique.md:22). That is about 1 PR for a gain that exists only while the PC is offline or the hub is down.
   - Folder transport stays on the post-release list.

Decided by Claude (the owner may overrule):
- hub restore and PITR (requirements-2);
- lost devices (failure-6);
- the hub-data disclosure (public-5);
- the fresh-account test (public-6);
- no WebSocket in the first release;
- decision 17 over decision 21 on purging.

## Refuted findings worth a note

- **failure-3 (10 GB per DO, tombstones kept forever):** the DO-storage doctor line is already LATER (RD/improvements-synthesis.md:534), and tombstones carry no body (RD/issue50.md:132). The Free plan's 5 GB total is now named under Setup and cost.
- **failure-5 (stop line not enforced):** the settled mechanism is the manual first-invoice check (proposal:14; RD/issue50.md:49, "a design budget, not a pricing guarantee"). Only the wording changed.
- **Points taken from the refuters:**
  - inherited-1's fit refuter is right that one DO was chosen knowingly (proposal:68), so only the userId routing is removed.
  - inherited-3's "a few thousand DO requests a month" is sourced (proposal:341). But 1M/month is the Paid allowance; Free is 100k/day.
## Verified after the audit (2026-09-25)

- Zero Trust onboarding asks for payment details even on the Free plan (developers.cloudflare.com/cloudflare-one/setup/index.md, step 3: "If you chose the Zero Trust Free plan, this step is still needed but you will not be charged").
- workerd allows the fts5 and fts5vocab modules (cloudflare/workerd src/workerd/util/sqlite.c++:1334-1341); trigram is a built-in FTS5 tokenizer, confirmed by one statement in the hub spike.
- No WebSocket in the first release narrows MUST M19 (the WebSocket clause and its measure move to LATER); the owner approved the 23 MUST items as a set, so this is stated to the owner.
