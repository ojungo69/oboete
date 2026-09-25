# Design B, section 7: running and releasing (revised after the audit, 2026-09-25)

Finding ids in brackets mark the bullets that changed. Letters A to G mark gaps I found while revising; they were not in the audit list.

Short names:
- "RD/" means docs/research/redesign-2026-09-24/.
- MUST and SHOULD ids (M15 backups, M16 write failures, M17 update, M23 public install, S5 probes, S7 egress ledger) are in RD/improvements-synthesis.md.
- Measurement ids (M1, M5, M10) are in RD/options-draft.md §9 and RD/improvements-synthesis.md.

Settled inputs:
- owner decisions 8, 9, 13, 14, 16 and 17-20 (RD/owner-decisions.md);
- "updates only by explicit `oboete update`", "claude-mem is never stopped, deleted or reconfigured" and "finish every feature before release" (RD/owner-decisions.md:16, CLAUDE.md);
- MUST-M23 and the LATER install items;
- project CLAUDE.md: new features go to the isolated `oboete-dogfood` user before the owner's environment.

## 1. Revised section 7

### 1. Install

- **Builds** [inherited-4]: release binaries for Linux x64/arm64, macOS arm64/x64 and Windows x64 are built only in CI, with SHA-256 checksums and GitHub artifact attestation. A one-line `sh` / PowerShell installer verifies both (MUST-M23, RD/improvements-synthesis.md:397-399).
  - The owner's devices cover only Linux x64 (WSL), Windows x64 and macOS arm64 (CLAUDE.md scope). Nobody can dogfood or cut over on Linux arm64 or macOS x64.
  - Those two targets get the §9 install line in CI: install, set up two agents, recall a memory in a new session, clean doctor (RD/options-draft.md:346). It runs on GitHub's hosted `ubuntu-24.04-arm` and `macos-15-intel` runners, which are free on public repos (docs.github.com/en/actions/reference/runners/github-hosted-runners).
  - The release table labels them "CI-verified only" (owner question 1).
- **Unsigned** [public-3]: no signing at first (decision 13, RD/owner-decisions.md:22). Before release, check two install paths on real machines:
  - the command install, where no Mark-of-the-Web or quarantine prompt is expected;
  - a binary downloaded from the Releases page in a browser. This path does get Mark-of-the-Web / quarantine, because decision 13's reasoning covers only the command install (RD/improvements-synthesis.md:584).
  - The install doc gives each OS's click-through for the second path, written from that check. This is the unsigned option as the synthesis states it: "document the warnings the user must click through" (RD/improvements-synthesis.md:616). The release notes put the install command first.
  - If the command path shows a prompt after all, signing is reopened under decision 13.
- **No OS package managers** [inherited-7]: no Homebrew, deb/rpm, AUR, winget or Scoop package.
  - Each would add a second update path (`brew upgrade`, `apt upgrade`, `pacman -Syu`) that bypasses the explicit `oboete update` (RD/owner-decisions.md:16).
  - macOS packages would also need signing the owner has not approved.
  - This was rejected in the improvement sweep (r4-public-1, RD/improvements-result.json:4133-4134) but never carried into the synthesis.
- **One static binary** [G]: SQLite bundled, rustls, no absolute dylib paths (phase0: pg0 failed on macOS without Homebrew, RD/phase0.md:11).
  - The Japanese (cp932) Windows console is a real-machine release check (RD/phase0.md:10, :37).
  - No guard is built for it, because Rust writes to the console through WriteConsoleW (RD/improvements-synthesis.md:606).
- **Local bge-m3** [inherited-6]: an optional download with a SHA-256 pinned in the binary, resume and a free-space check. The user can choose Workers AI or no embeddings instead, and can change this later with `oboete setup --embeddings` (MUST-M23, RD/improvements-synthesis.md:401-404).
  - Because of the pin, the host does not matter for integrity. Disclosure still needs one named host.
  - Recommended host: oboete's own GitHub Releases, next to the binary, if the shipped file fits the release-asset limit (checked at release). Otherwise the upstream host is named.
  - Setup names the host and the size before it downloads anything.
  - The model's licence goes into NOTICE when the release hosts the model.
  - Claude's decision; the owner may overrule.
- **Uninstall** [public-5]: `oboete setup --remove` takes every adapter entry out again (src/setup.rs:1-4; `all` is accepted, src/main.rs:81).
  - With a hub, uninstall first revokes this device's hub token (section 5's device-row delete, RD/section-5-revised.md:74) and deletes the local token file.
  - Content that already synced stays on the hub and on the other devices, because uninstall is not forget. If the content must go, forget it first.
  - The docs list the local data paths to delete by hand. For the last device, they also say how to delete the Worker, DO, R2 bucket and Vectorize index from the user's own Cloudflare account.
  - Pulled forward from LATER (r1-public-3, RD/improvements-synthesis.md:547), because a public tool needs a way out.

### 2. Setup

- `oboete setup` detects the 7 agents and writes hooks, plugins and MCP config.
  - It keeps a `.oboete.bak` copy of each file it edits (src/setup.rs:4, :353) and preserves comments in JSONC files (Cursor adapter).
  - Hook entries call the binary by its absolute path (src/setup.rs:162-172); §5 depends on this.
- **Questions** [inherited-6, public-1, public-2]: setup asks only about settings with a cost, a network destination or a privacy effect. Each question has one line on what it sends where (MUST-M23, RD/improvements-synthesis.md:406).
  - **AI tier preset and chain** (section 1). Subscription CLIs are an explicit opt-in (owner decision 21, option (b)): off by default; the tier line quotes Anthropic's policy, which does not permit third-party developers "to route requests through Free, Pro, or Max plan credentials on behalf of their users" (code.claude.com/docs/en/legal-and-compliance, checked 2026-09-25), and says the user carries that risk; the user types yes. The same applies to codex, grok and agy after their terms are checked before release.
  - **Embeddings**: none, local (with host and size, §1) or Workers AI.
  - **Raw sync** (decision 14, default off).
  - **Capture exclusions** (section 6).
  - **Hub** (optional; `oboete hub deploy`, section 5). Before anything is created, its line states:
    - a Cloudflare account is needed;
    - the user creates a scoped API token in the dashboard (RD/hub-platform.md:113). It is stored only with `--keep-token` (RD/section-6-revised.md:78);
    - device sync fits Workers Free in steady state. The first push of a long history can exceed Free's 100,000 rows written per day, and so take several days;
    - remote semantic search needs Workers Paid, USD 5/month minimum (RD/section-5-revised.md:17-18).
  - **Transcript import** (§4).
- **Other settings** [inherited-5, requirements-8]: decision 16's remaining settings are keys in `config.toml` (RD/owner-decisions.md:25): extra redaction rules and the allowlist, injection on/off and size per kind, raw retention, capture detail, and backup location. `config.toml` already holds `[[providers]]`, `[summary]` and `[embedding]` (src/config.rs:8-16).
  - Setup takes their defaults without asking. At the end, it prints the file path and each default it took.
  - `oboete setup --advanced` asks them one by one. An allowlist value is read from a hidden prompt and stored only as its SHA-256 (RD/section-6-revised.md:74).
  - doctor prints the effective values.
  - A settings page in the viewer is LATER.
  - Claude's decision; the owner may overrule.
- **After setup**: three checks run.
  - the curator isolation check for each chosen CLI (section 6);
  - the per-agent status table (live-verified / implemented / unverified, M10);
  - one run of S5's canary round trip (RD/improvements-synthesis.md:456).
- **Scripted setup and language** [public-4]: `oboete setup --yes` takes the defaults (tier none, no embeddings, no hub, no transcript import) for scripts.
  - Setup's prompts are in English, like the rest of the CLI. The Japanese README walks through each question (r1-public-5, RD/improvements-synthesis.md:548).
  - The draft's "UI follows the locale" is dropped. It had no owner decision (RD/owner-decisions.md:16 says only "Japanese and English"), no budget line, and no code: there is no locale detection in src/.

### 3. Update

- **Channel** [inherited-4, public-6]:
  - `oboete update` installs the latest stable release (owner rule: only this command updates). `oboete update --pre` installs the latest pre-release, using GitHub's pre-release flag.
  - Every release is first published as a pre-release and run by the `oboete-dogfood` user. It is marked stable only after that run.
  - The owner's machines use plain `oboete update`. So the channel enforces the dogfood-first rule, and nobody has to remember it.
  - `oboete update --check` only reports whether a newer release exists and whether its notes mark it as a security fix. It installs nothing.
  - There is no scheduled check (owner question 3). doctor shows "last release check: N days ago / never".
- **Steps** [requirements-1, failure-2, requirements-6, failure-6]: M17's six steps, in M17's order (RD/improvements-synthesis.md:283-289), with the worker stopped first.
  - 0. Take the worker lock: the pid/lock file of RD/options-draft.md:119 (today `~/.oboete/observe.lock`).
    - A running worker is asked to exit after its current transaction.
    - No hook starts a new worker until step 6 ends.
    - This also covers renaming the exe while a worker runs it on Windows.
  - 1. Download to a staging path, then verify the checksum and the attestation.
  - 2. Run a read-only schema check with the staged binary. It compares `PRAGMA user_version` (used today at src/db.rs:179, :219) with the versions it can migrate from. A store newer than the binary is refused, so there is no downgrade across a schema version.
  - 3. Check that there is free space for the backup plus the staged binary. Then take the backup: M15's sealed segments of raw.db and the op log (knowledge.db is rebuildable, section 1).
    - The backup comes before any change (R13, RD/issue50.md:47).
    - forget reaches it like any other backup.
  - 4. Swap the binaries by renaming them, because Windows cannot overwrite a running `.exe`.
  - 5. Migrate: forward-only, one transaction per file.
    - Hooks that fire meanwhile wait on SQLite's busy timeout (2 s, src/db.rs:103). After that, they fail open with M16's lock-contention marker (RD/sections-1-4.md:26).
    - So step 5 must stay short. A raw.db change that rewrites rows runs afterwards as a resumable worker job, following the `forget_jobs` pattern (RD/section-6-revised.md:31).
  - 6. Run a self-check with the new binary: open both files, run `quick_check`, and write, index and search a canary inline. The worker is stopped (step 0), so the new binary does the indexing itself.
    - If the self-check fails, restore the backup and swap the old binary back. Then copy back the raw records written after the backup (seq above its last).
    - The old binary can read those records, because step 5 only adds columns or tables to raw.db. If a release cannot keep to that, those writes are lost, which is M17's stated bound (RD/improvements-synthesis.md:294).
    - Then `oboete setup --refresh` rewrites any adapter entries whose format changed, and the lock is released.
  - Claude's decision; the owner may overrule: step 0, the free-space check and the copy-back after a restore are additions to M17.
- **Other devices and the hub** [requirements-5]:
  - Other devices on an older version park ops they do not understand (section 5, M17), and doctor names the version needed.
  - The hub's Worker has its own build version on its status route. When a release bundles a newer Worker, `oboete update` and doctor both say "hub runs X, this binary bundles Y: run `oboete hub deploy`".
  - The redeploy is never silent, because the Cloudflare API token is kept only with `--keep-token` (RD/section-6-revised.md:78). One device redeploys for the whole fleet.
  - A new Worker keeps serving the previous protocol version (docs/hub-protocol.md, section 5). So the order of the device updates and the redeploy does not matter, and M17's parking covers the op format in between.
  - Claude's decision; the owner may overrule.

### 4. Moving existing data in

- **The current oboete store** [inherited-3]: ~/.oboete/oboete.db on the owner's machines. On WSL, when the draft was written (2026-09-25), it held 14,826 raw events, 141 observations, 17 summaries, 14 prompts and 20 sessions.
  - Since PR #52 (96cc106, decision 20, RD/owner-decisions.md:29), the current code keeps raw events after summarizing and moves `sessions.observed_event_id` instead (src/db.rs:21, :863). So the store grows until cut-over.
  - Raw that the old `DELETE` removed before that commit is gone from the store.
  - The new version reads the store and never writes it.
  - **Import keys** [failure-1, failure-3]: every imported item is keyed by two things: the old store's `device_id` (its `meta` table, src/db.rs:23-24), and the old row id or document uid (src/db.rs:150-163). This is the same pattern as the claude-mem import's source ids.
    - The keys and a checkpoint (the highest old event id imported) are written in the same transaction as the rows.
    - So a killed `oboete migrate` resumes where it stopped, and a rerun imports nothing twice.
    - A rerun after the hook switch imports only what the old binary wrote since then (§5).
    - Old event ids stay stable, because the old code has deleted no events since 96cc106.
  - Events become raw records in raw.db:
    - labelled `source = oboete-v1`;
    - with new seq numbers, in timestamp order within each pass;
    - redacted again with the current rules;
    - curated only on request (`oboete recurate --source oboete-v1`, with a cost estimate first).
  - Observations, summaries and prompts become imported documents: search and timeline only, never injected, status unknown (section 4). They keep their uids, so the 112-question judgments still map to them (§5).
  - **Settings** [C]: the old `config.toml` (`[[providers]]`, `[summary]`, `[embedding]`, src/config.rs:8-16) becomes the new provider chain and the embedding setting.
    - The user's providers, their order and their models survive, as R05 requires (RD/issue50.md:39).
    - The embedding key file is referenced by path, never copied.
    - Setup then asks only what the old file does not answer.
  - **Old files** [E]: they stay untouched until `oboete migrate --finish`. That command first runs one more import pass. Then it lists these old files and asks before deleting them:
    - oboete.db and its -wal/-shm;
    - the pre-*.db snapshots and their -wal/-shm;
    - pre-rollout-*/, spool/, cache/, logs/ and memory.db.

    This list is what ~/.oboete held on WSL on 2026-09-25.
    - Until `--finish`, doctor lists these files and forget prints them as a limit (section 6 §3).
    - This settles the choice RD/section-6-revised.md:134 left to section 7: the snapshots are listed as a limit, not rewritten.
    - eval/ is not an old runtime file. It holds the evaluation set (queries.jsonl, judgments.jsonl) and an 880 MB copy of claude-mem's database. `--finish` leaves it alone. doctor lists it as "evaluation copies" that forget cannot reach, and M4's grep covers it.
- **claude-mem history** [requirements-2]: read-only import (owner decision), opened with `mode=ro` and never written.
  - Titles and bodies are redacted with the current rules on the way in (src/import.rs:86-87).
  - Items are labelled imported and keyed by source id, so a re-import skips tombstoned items (section 5 deny-list).
- **Transcript import** [A, failure-5]: optional and off by default. It is new scope, not one of the 23 MUST items. Claude's decision; the owner may overrule.
  - At setup, the user may build raw records from agent transcripts written before the install. This gives a new user day-one memory, and gives the owner back the session heads that the old `DELETE` removed.
  - This is not section 2's gap backfill, which stays LATER (RD/sections-1-4.md:22). It needs the same three things (RD/constraints-synthesis.md:139-142):
    - a deny-list check, so forgotten content does not come back from a vendor transcript;
    - `source = transcript` on every record;
    - one parser per format. Hooks already read transcript tails for agy, Codex and Cursor (src/hook.rs:243-368). Claude Code's ~/.claude/projects JSONL needs a new parser. Grok (SQLite) and OpenCode (no transcript) show "no parser" in the status table (RD/constraints-synthesis.md:142).
  - Setup shows a count and a size first. Records are redacted, and they are curated only when the user asks.
  - **Dedup**: raw stores no turn number (RD/sections-1-4.md:24; the events table has none either, src/db.rs:32-38). So the draft's key (agent, session, turn) cannot be computed. The rule instead:
    - Per session, import only the transcript entries older than the session's earliest raw record, whether that record was hook-captured or `oboete-v1`. Both come from the same device, so they share one clock.
    - A session with no raw is imported whole.
    - A session the old `DELETE` trimmed gets back its deleted head.
    - Gaps after the first raw record are section 2's LATER backfill.
    - At the boundary, the hook stamps a turn later than its transcript entry does. So at most one turn per session can appear twice, and both copies are labelled by source.

### 5. Cut-over on the owner's machines

- **Order** [B, D, failure-3]: first the dogfood user, then WSL, then Windows native, then the M1 iMac. On each machine:
  1. Install the new binary at a path the hooks do not call.
     - Today's hooks call `/home/jura/.cargo/bin/oboete` by absolute path (~/.claude/settings.json; src/setup.rs:162-172). A new binary installed there would switch every agent before the checks below.
     - Until step 4, the old binary keeps that path, and the new one is called by its full path.
  2. Run the migration (§4). Answer yes to the transcript import, so the sessions the old `DELETE` trimmed get their heads back (§4). Run doctor.
  3. Compare search on the 112 questions against the old store. Pass:
     - no slice's recall@10 drops by more than 0.02 (M1's no-regression line, RD/options-draft.md:325);
     - overall nDCG@10 drops by no more than the same 0.02. This half is Claude's adaptation of M1's line.
  4. Switch the hooks: setup rewrites the entries to the new path.
     - From then on, `oboete` on PATH resolves to the new binary. The old one is kept under another name for rollback.
     - Then run `oboete migrate` again. It imports only what the old binary wrote since step 2 (§4 keys).
  5. Run `oboete migrate --finish` after a period the owner chooses. It runs one more import pass first.
     - Agents may keep the hook commands they loaded at session start. So the old binary can still write to oboete.db until every running session restarts, and that is why `--finish` imports once more.
- **Hub last** [failure-7]: the owner's hub is connected only after every device has passed step 4.
  - Until then no device pushes anything. So rolling one device back never leaves its ops on the hub or on other devices.
  - Today's code has no hub sync, so nothing is lost while the hub waits.
  - Once the hub is on, rolling a device back to the old design only stops that device's sync. What it pushed stays valid, and forget from any device reaches it.
- **Dogfood** [F]: the dogfood user migrates a copy of the owner's old store and syncs through its own test hub, never the owner's. The copy is deleted when the dogfood run ends.
- **Rollback**: put the previous binary back at the hooks' path. The new version never writes the old store, so it is intact.
- **claude-mem**: it keeps running unchanged next to oboete, and both inject at SessionStart as they do today. The public docs say running both is supported and that each injects its own block.

### 6. Running

- **Worker lifecycle** [inherited-1 note]: the worker is hook-started and exits when idle. This is the default until M5 decides (RD/options-draft.md:340).
  - Periodic jobs run when the worker starts, if they are overdue: the backup deadline (RD/sections-1-4.md:27) and S5's daily canary (RD/improvements-synthesis.md:456). So a device that was idle for days catches up at its next session.
  - If M5 makes an OS service the default:
    - setup registers the service, and `setup --remove` unregisters it;
    - update stops and restarts it (§3 step 0);
    - doctor checks that it runs the current binary (S5, :455).
- **doctor lines** [requirements-3]:
  - pending windows, with reasons and next attempt;
  - coverage;
  - provider budget left;
  - last sync, last pull and last backup, with the result of M15's `integrity_check` and segment-checksum check (RD/improvements-synthesis.md:255);
  - S5's probes (:453-454): a canary round trip through write, index, FTS search and MCP `get`, and an authenticated, read-only hub status call, reported separately from the pending count;
  - hook entries that point at a binary other than the running one (src/setup.rs:163). S5's service check replaces this line if M5 makes the service the default;
  - curator isolation status;
  - capture failures (M16);
  - unfinished forget jobs;
  - free space, disk use and growth;
  - whether the data volume is encrypted (RD/section-6-revised.md:81);
  - version skew, and the hub's Worker version (§3);
  - adapter status;
  - effective settings (§2);
  - last release check (§3);
  - old files awaiting `migrate --finish`, and evaluation copies (§4).

  `oboete doctor --egress` shows S7's ledger (RD/improvements-synthesis.md:473).
- **Logs**: logs hold no payload (S7), rotate and are size-capped. `oboete doctor --bundle` writes metadata only, for bug reports (LATER r2-oncall-4, pulled forward for public support).
- **Network** [inherited-6]: no telemetry. oboete connects only to:
  - the curation providers and the embedder that the user configured;
  - the user's hub;
  - the bge-m3 host, once, when local embeddings are chosen (§1);
  - the Cloudflare API during `oboete hub deploy`, with the user's token;
  - GitHub Releases during `oboete update` and `oboete update --check`.

  M23's "what leaves the machine" doc lists the same destinations for each tier.

### 7. Public release

- **Licence and NOTICE** [public-7]: Apache-2.0 (LICENSE exists).
  - NOTICE reproduces the donor's NOTICE text verbatim, as Apache-2.0 §4(d) requires: "Claude-Mem / Copyright 2026 Alex Newman / This product includes software developed for the Claude-Mem project. / Licensed under the Apache License, Version 2.0." (~/.claude/plugins/marketplaces/thedotmack/NOTICE, commit c4bfa45).
  - NOTICE also carries the bge-m3 licence when the release hosts the model (§1).
  - The ported files are in the donor's Apache-2.0 repo: src/services/sync/CloudSync.ts, workers/sync-hub/src/do/SyncHub.ts and workers/sync-hub/src/canonical-content.ts.
  - The donor's docs/ip-boundary.md lists "Team/org memory sync" among the reserved areas that "are not shipped by Claude-Mem Server v0.1", and its rule keeps reserved code out of the public repo. So the port takes only code published there, and this is re-verified for each ported commit (RD/issue50.md:151).
- **Docs** [public-2, public-6, G]:
  - a Japanese README first, then English (LATER r1-public-5, pulled forward);
  - MUST-M23's four docs:
    - what is stored and what is redacted;
    - what leaves the machine for each tier, including §2's hub costs and the subscription-terms line;
    - what forget purges, and its limits;
    - setup for each agent, with its M10 status;
  - docs/hub-protocol.md (section 5);
  - SECURITY.md with a private reporting address;
  - security fixes published as GitHub Security Advisories and marked in the release notes, so `update --check` and a watch on the repo's releases both show them;
  - a CHANGELOG, and semver.
- **CI**: `cargo audit` now, non-blocking. cargo-deny and an SBOM at release (LATER r2-security-6).
- **Release gate** [requirements-7], all of these:
  - every feature finished (owner rule);
  - the pass lines of RD/options-draft.md §9 (:311-352) met;
  - the acceptance and completion checks of RD/issue50.md §9 (:170-204) met;
  - the real-machine checks in §1 done;
  - the release promoted from pre-release after the dogfood run (§3).
  - the evaluation lines of section 8 (evaluation and build order), once that section is settled.

## 2. What changed and why

- **inherited-3:** owner question 1 is removed, because decision 20 and PR #52 (96cc106) already stopped the raw deletion. §4 now describes the current code (src/db.rs:21, :863), and the stale citation of src/db.rs:848 is gone.
- **inherited-4:** a pre-release channel makes `oboete update` enforce dogfood-first, instead of relying on discipline. The two targets with no owner device get the §9 install line on hosted runners and a "CI-verified only" label (owner question 1).
- **inherited-5 + requirements-8:** decision 16's settings get a named home: `config.toml`, `setup --advanced`, printed defaults and doctor. Setup still asks only the questions that have a cost or network consequence.
- **inherited-6 + public-2:** the network line now lists every destination (model host, Cloudflare API, GitHub). The hub question states the dashboard token step and the Free/Paid limits before anything is created. The bge-m3 host has a recommendation.
- **inherited-7:** Install now records the rejection of OS package managers and its reason (a second update path).
- **requirements-1 + failure-6:** update follows M17's six steps in order. The backup comes before the swap, the read-only check uses `user_version`, and the self-check (quick_check plus an inline canary) restores automatically. The free-space check is part of step 3.
- **requirements-6 + failure-2 (+ inherited-1's residue):** update takes the worker lock first, which also covers the Windows rename while a worker runs. Hooks in the migration window wait on the 2 s busy timeout and then fail open with M16's marker. So migrations that rewrite raw rows move to a resumable worker job.
- **requirements-5:** the hub Worker's version is shown, and redeploy is explicit (`oboete hub deploy`, since the token is not kept). A new Worker also serves the previous protocol version.
- **public-6:** adds `oboete update --check`, security advisories and doctor's last-check line. There is no background call (owner question 3).
- **requirements-2:** the claude-mem import bullet now says it redacts, as src/import.rs:86-87 does.
- **requirements-3:** doctor gains:
  - M15's integrity and checksum result;
  - S5's canary and hub probes;
  - S7's `--egress`;
  - the encryption line;
  - a hook-path check that stands in for S5's service check until M5 decides.
- **requirements-7:** the release gate names RD/options-draft.md §9 and RD/issue50.md §9. The draft's "section 8" matched neither: issue50 §8 is build order, and options-draft §8 is unknowns.
- **failure-1 + failure-3:** the v1 import is keyed by (old device_id, old id), with a checkpoint in the same transaction. That makes it resumable and idempotent. Delta reruns after the hook switch and at `--finish` close the window between migration and switch, including sessions still running on the old hooks.
- **failure-5 + A:** a time cut per session replaces the (agent, session, turn) key: only entries older than the session's earliest raw are imported, and the one-turn overlap at the boundary is disclosed. The draft's "new" backfill is reconciled with settled S2-17: it becomes a separate, optional feature with S2-17's three conditions, flagged as scope outside the 23 MUST items.
- **failure-7:** the owner's hub is connected only after every device has cut over. This uses ordering instead of compensation, so a rollback never leaves ops elsewhere.
- **public-1:** checked against Anthropic's policy page on 2026-09-25: third-party developers may not route requests through users' plan credentials. Public setup therefore does not offer subscription CLIs (owner question 2). One quote in the audit's draft was not on the page and was withdrawn.
- **public-3:** the real-machine check also covers a browser download from Releases, and the docs give the click-through, as the unsigned option already required (RD/improvements-synthesis.md:616).
- **public-4:** the locale-following setup UI is dropped. Prompts are in English, and the Japanese README explains them (r1-public-5).
- **public-5 (narrowed):** uninstall revokes this device's hub token, and the docs say how to delete the hub. Shared content is not purged (the fit refuter's point).
- **public-7:** NOTICE reproduces the donor's text verbatim, and the ported files' licence boundary is recorded against docs/ip-boundary.md.
- **B:** during cut-over, the new binary must not be installed at the path the hooks call (/home/jura/.cargo/bin/oboete). Otherwise the install itself switches every agent before the checks.
- **C:** migration carries the old `config.toml` over, as R05 requires (keep providers, order and models). The draft migrated data but not settings.
- **D:** the 112-question comparison gets a pass line (M1's 0.02 no-regression lines). Without one it was not a gate.
- **E:** `migrate --finish` lists every old file actually found in ~/.oboete. eval/ (the evaluation set plus an 880 MB claude-mem copy) stays out of the delete list and is shown as evaluation copies. This settles RD/section-6-revised.md:134.
- **F:** the dogfood user works on a copy of the old store with its own test hub, and the copy is deleted afterwards.
- **G:** the cp932 and dylib checks cite phase0.md and the synthesis's "no guard needed". The Japanese README is labelled as pulled forward from LATER.

## 3. Owner questions

1. **Linux arm64 and macOS x64 (inherited-4).** No owner device runs them, and CI passing does not prove they work (CLAUDE.md).
   - Recommendation: ship them. Run the §9 install line on GitHub's `ubuntu-24.04-arm` and `macos-15-intel` runners (both listed as standard runners, "free and unlimited on public repositories", docs.github.com/en/actions/reference/runners/github-hosted-runners, checked 2026-09-25; the repo is public), and label both "CI-verified only".
   - Alternative: drop them from the first release. MUST-M23 then lists three targets.
2. **The subscription tier for public users (public-1).** Checked 2026-09-25 against code.claude.com/docs/en/legal-and-compliance (the audit's third quote, about "the unmodified Claude Code binary", is not on that page and is withdrawn). The page says:
   - OAuth "is intended exclusively for purchasers of Claude Free, Pro, Max, Team, and Enterprise subscription plans and is designed to support ordinary use of Claude Code and other native Anthropic applications";
   - "Developers building products or services that interact with Claude's capabilities, including those using the Agent SDK, should use API key authentication";
   - "Anthropic does not permit third-party developers to offer Claude.ai login or to route requests through Free, Pro, or Max plan credentials on behalf of their users", enforced "without prior notice";
   - "Advertised usage limits for Pro and Max plans assume ordinary, individual usage of Claude Code and the Agent SDK."

   A public oboete that runs the user's `claude` login in the background to curate is close to "route requests through ... plan credentials on behalf of their users". The owner running it for themself is personal use of their own plan, which the page does not forbid but does not clearly bless either.
   - Precedent (checked 2026-09-25): claude-mem (hardened-options.ts, Agent SDK on the user's Claude Code login) and Hindsight (claude_code_llm.py) are public tools that do this today with no gate. Cline has a Claude Code provider that runs the user's signed-in CLI. CC Pocket switched to `ANTHROPIC_API_KEY` by default and keeps subscription login behind an explicit opt-in (`BRIDGE_ALLOW_CLAUDE_OAUTH=1`), deprecating older versions "because current Anthropic Claude Agent SDK docs do not permit third-party products to use Claude subscription login" (npmjs.com/package/@ccpocket/bridge). Third-party pages cite an earlier version of the policy page saying it does not prevent hosting "the unmodified Claude Code binary" with each user's own login; the current page no longer has that sentence.
   - Option (a): public setup does not offer subscription CLIs as curators; presets use free APIs, local models and API keys; a user may add their own CLI to `[[providers]]` in config.toml, and the docs quote the policy. Consequence for the owner: the owner's config.toml has no `[[providers]]` today, so migration writes the owner's chain explicitly.
   - Option (b): public setup offers it as an explicit opt-in (the CC Pocket pattern): off by default, the tier line quotes the policy, and the user types yes. This keeps parity with claude-mem, the owner's yardstick, while disclosing the risk.
   - Either way, the owner's own config keeps claude (decision 5), and the terms for codex, grok and agy are checked the same way before release.
3. **How a public user learns about a release (public-6).**
   - Recommendation: the explicit `oboete update --check`, GitHub Security Advisories, and doctor's "last release check" line. Nothing runs on a schedule.
   - Alternative: the old design's daily notify-only check (docs/research/search-sync-proposal-2026-09-23.md:436). It would be the only network call oboete makes on its own.

Decided by Claude (the owner may overrule):
- the update additions: the worker lock, the free-space check, copying raw back after a restore, and row-rewriting migrations as worker jobs;
- the pre-release channel for dogfood;
- the hub connected last during cut-over;
- the explicit hub redeploy, with the previous protocol version still served;
- decision 16's settings in `config.toml` and `setup --advanced`;
- dropping the locale-following UI;
- the optional transcript import with its time-cut dedup;
- the recommended bge-m3 host;
- the separate install path during cut-over;
- the pass line for the 112-question comparison;
- the `--finish` file list, with eval/ kept;
- the narrowed uninstall.

## Refuted findings worth a note

- **inherited-1 (the worker lifecycle does not cover resident jobs):** the lifecycle is an explicit M5 decision, not an inherited constraint (RD/options-draft.md:119, :340). M15's backups run at idle exit, and M9 treats idle exit as normal. I took the two real residues:
  - update stops the worker before the Windows rename (§3 step 0);
  - overdue periodic jobs, including S5's daily canary, run at worker start (§6).
- **inherited-2 (bring back the daily update check):** superseded by public-6, which survived. The explicit `--check` covers the need without a call that oboete makes on its own. Reinstating the old daily check is owner question 3's alternative.
- **requirements-4 (cut-over never runs backfill):** setup offers the import on every machine, so the refuters are right. I took their cheap residue: §5 step 2 says to answer yes, and §4's time cut gives back the heads of the sessions the old `DELETE` trimmed. The finding's "most raw is already gone" is overstated: the old store still held 14,826 events.
- **failure-4 (no combined SessionStart size with claude-mem):** no change.
  - oboete caps only its own block (RD/sections-1-4.md:42). claude-mem's block is outside our control (owner rule), and each hook entry has its own timeout.
  - The finding's "Pi 3 s" is OpenCode's context timeout. Pi's is 2 s (RD/sections-1-4.md:41; agent-adapters-2026-09-23.md:295).
- **Points taken from refuters of surviving findings:**
  - public-5's fit refuter: uninstall never purges shared hub content.
  - public-7's worth refuter: the NOTICE fix is one clause.
  - failure-6's worth refuter: the transaction already rolls back on ENOSPC, so the free-space check guards only the backup, as one clause in step 3.
  - requirements-7's evidence refuter: issue50 and options-draft do have a §8, but neither holds evaluation lines. So the reference is renumbered, not just removed.

(Copy of this text: /tmp/claude-1000/-home-jura-projects-oboete/035bf06c-56a7-4208-83ad-48d576b8d187/scratchpad/section-7-revised.md)