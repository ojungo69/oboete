- **All approaches: goals and must-keeps not covered**
  - **Owner correction path is missing (brief: "he only corrects mistakes").** No approach has a correct, demote or retract command for a claim or observation. B also does not say where corrections live. If they sit in `knowledge.db`, `oboete rebuild` and "re-derive with a better model" silently wipe them. Corrections need to be raw or op-log events, and nothing tests that they survive a rebuild and a sync.
  - **"Past failures not repeated" (goal 2) has no test.** The `lesson` claim kind exists, but M3 measures only decisions and proposals.
  - **English is never measured.** M1 and M6 draw from the 112 Japanese questions, and M3's gate is tuned on Japanese. The brief requires Japanese and English records.
  - **The 4 unverified adapters (agy, OpenCode, Pi, Cursor CLI/IDE) are not in any milestone.** No plan verifies them live. No approach says what an agent without a SessionStart-style injection point gets for resume (goal 1, "another agent").
  - **Migrating the current oboete DB is missing.** The ~180k existing documents include the old summaries that caused the "old summaries as current state" failure. No approach says how they are imported, labelled or kept out of injection.
  - **Per-repo sync exclusion set after data has synced is undefined.** Nothing says whether the repo is purged from the hub and from other devices, or only stops syncing.
  - **MCP tool output is not said to be fenced or attributed.** The memo covers only injected SessionStart text. The brief says memory is data, never instructions, on every read path.
  - **Easy install on day one misses OS trust prompts.** Code signing and notarisation (Windows SmartScreen, macOS Gatekeeper) are not addressed for the single-binary installer.
  - **Remote MCP auth is missing.** How the Claude app authenticates to the hub (OAuth, token) is not addressed. Only per-repo grants are.
  - **Semantic search on the none tier is undefined.** It is unclear whether local bge-m3 counts as "AI". Without it, the must-keep "semantic + full-text" drops to full-text only.

- **B: contradictions and gaps**
  - **Rebuild contradicts multi-device.** `oboete rebuild` drops `knowledge.db` and replays the local `raw.db`. But claims, manifests and vectors received from other devices (with raw sync off by default) are not in the local `raw.db`. A rebuild loses them unless the inbound op log is kept, and the memo never says it is.
  - **Re-derivation across devices is undefined.** Does a new model produce new uids? What happens to `supersedes` edges that point at old uids, and to copies of old claims already on other devices? The recipe field alone does not settle this.
  - **Physical purge inside SQLite is not designed.** Deleted content can survive in:
    - free pages, unless `secure_delete` is on or the file is vacuumed;
    - the WAL, until a checkpoint;
    - FTS5 shadow segments, until a merge or optimize.

    M4 would catch this, but the design does not prevent it.
  - **Deletion through the folder transport is unaddressed.** Op files are append-only with one writer per file, so the deleted payload stays in the shared folder. It also stays in syncer version or trash folders (e.g. Syncthing `.stversions`). M4 does not grep the shared folder or the syncer's history.
  - **Hub backup restore is unaddressed.** Durable Object SQLite point-in-time recovery (about 30 days) could bring deleted payloads back. This is from Cloudflare's docs, not from the landscape data, and the memo does not consider it. R2 object versioning and Vectorize delete latency are not addressed either.
  - **Rebuild cost at scale is missing.** Vectors live in `knowledge.db`, so a rebuild re-embeds the ~180k documents plus ~150k imports. That conflicts with the "300/day ≈ 1% of neurons" run-cost claim, and no rebuild time or cost is estimated.
  - **Two B mechanisms may conflict with "curation is fully automatic".** Concurrent-supersession conflicts are "shown, not resolved automatically". "Widen candidates" nominated by the LLM also imply an owner review queue. Both need owner action.
  - **On the none tier, retracted directives can be injected.** "Unverified owner lines" in the manifest carry no retraction check, so a retracted "今後は…" can still appear. That breaks goal 2 ("never applied as current"), and M3 does not test the none tier.
  - **The `/remember-global` prefix is an injection path.** Pasted file text that begins with the prefix, on the human input path, would become global. The brief forbids pasted text becoming a global rule, and M3's injection canary does not cover this case.
  - **The acceptance-turn gate ("はい") can promote an injected proposal.** If the assistant's proposal came from tool or file content, a user "はい" makes it a repo-scope decision. The gate does not check where the proposal came from.
  - **Folder transport is treated as optional, but may be required.** §10 makes it an owner decision. The brief says cloud is "never required", and without the folder transport, other users' multi-device setups require the cloud. The memo should build it by default or state that multi-device needs the cloud.
  - **Fallback if B fails its own lines is missing.** The decision rule says "Otherwise B", but has no branch for B failing M3 (gate precision ≥ 0.95 or recall ≥ 0.80), M5 or M6.
  - **The claude-mem sync-hub donor has no gate test.** Its self-host viability is "uncertain", yet 3 of B's build units depend on it. A has a Phase 0 check for its engine; B has none for this donor.
  - **Decisions in the claude-mem import never reach goal 2.** Imports are search-only and never curated into claims. Imported old decisions can also appear in search without a status.

- **A: gaps and contradictions**
  - **Re-derive with better models is not addressed.** For A it means re-retaining all raw data, which is the same ~330k-call infeasibility the memo notes for imports.
  - **Per-repo sync exclusion is not addressed.** The iMac's data always travels to the PC server.
  - **Remote MCP for the Claude app has no stated data location.** Hindsight sits on WSL on the PC, so the Claude app cannot reach it unless exposed to the internet or copied to the cloud.
  - **WSL idle VM shutdown is not addressed.** It stops the Hindsight daemon, so Windows native and the iMac lose recall even while the PC is on. The memo mentions only "PC off".
  - **Nothing shows provider-failure backlog visibility in A.** Only the coverage ledger is mentioned.
  - **The viewer UI is not addressed.** It is unclear whether it is Hindsight's or oboete's.
  - **Local embedding placement is not addressed.** Where bge-m3 runs, its RAM, and whether embeddings pass the egress gate are not stated.
  - **The Cognee fallback in §7 ignores the table's conflicts.** Cognee has:
    - no subscription-CLI provider, which clashes with the owner's default tier;
    - no sync;
    - Kuzu, whose upstream is unmaintained;
    - forget() cascade marked "not vote-checked".

    Passing Hindsight's install and delete checks does not transfer to Cognee.

- **C: gaps**
  - **Viewer, remote MCP, setup, doctor and update are not addressed.**
  - **The cost of ~10–12 PRs is not like-for-like with B.** It is unclear whether it includes the hub port, remote MCP and the viewer, which make up about 6–7 of B's units.
  - **The no-AI row in §6 omits the manifest ("Record, FTS"), but nothing prevents C from building it.** The distinction from B is not justified.
  - **"Lookup quality same as B" is not supported.** It is unclear whether C indexes raw chunks.

- **External-engine claims not backed by the landscape data, or inconsistent with it**
  - **§2 says "only Hindsight is credible; every other engine lacks at least two of: hard delete, sync, none tier, 4+ of 7 agents".** This is inconsistent with the table:
    - Hindsight itself lacks hard delete and sync, which is two of the four.
    - claude-mem covers 5 of the 7 agents and has a sync-hub. The table says nothing about it lacking hard delete or a none tier, so it was never evaluated as a base engine.
    - For Supermemory, the table gives no evidence that it lacks sync.
  - **§3 claims "Hindsight's native claude-code/codex providers bypass the egress gate and budgets".** The landscape says nothing about their data path or budgets. The shim also depends on custom base-URL support, which is unknown.
  - **§4 claims Workers AI embeddings "stay within the included allowance (300/day ≈ 1%)".** This comes from `plan.md`, not the landscape. It does not cover the import of about 330k documents or rebuild re-embeds.
  - **§3 claims "a normal Cloudflare Worker cannot host PostgreSQL".** This is not in the landscape data (plausible, but unsourced).
  - **§4 claims "Syncthing is seconds".** This is not in the landscape data.
  - **A's cost assumes about 3k characters per chunk while B assumes about 50k per window, and neither is measured.** The table says Hindsight chunk size is configurable, so "an order of magnitude below A" is not supported.

- **Measurements missing to choose between approaches**
  - LLM calls and CLI minutes per heavy day for Hindsight on real raw data at a fixed chunk size. M1 measures retain cost per 100 documents, not per heavy day.
  - Rebuild and re-derive time and cost on 180k documents plus 150k imports, for A, B and C.
  - `raw.db` storage growth per year at 1.5M characters per heavy day.
  - Self-host test of the claude-mem sync-hub with oboete's own verifier, before B commits.
  - Curator and gate quality per AI tier: local or free models (e.g. Ollama on M1) against subscription CLIs. The brief says more AI means finer curation, but no tier is compared.
  - Retrieval and lookup on English questions.
  - Lesson recall (goal 2, past failures).
  - Whether owner corrections survive rebuild, re-derive and sync.
  - A deletion test through the folder transport, syncer version history and hub point-in-time recovery, added to M4.
  - Live checks of the 4 unverified adapters, including where each agent can inject at session start.
  - Sample size and power of the M3 proposal/decision slice. It is unspecified; only the overturned-decision slice has ≥ 50 pairs.
  - Hybrid search latency at the full corpus of about 330k documents on the slowest device. M5's MCP search p95 line does not state the corpus size.
  - The decision rule omits M5 and M6 for A. A can therefore be chosen without passing the resume and lookup lines.