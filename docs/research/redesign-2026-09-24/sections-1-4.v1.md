# Design B, sections 1-4 as presented to the owner (2026-09-25)

Approved by the owner unless noted. Owner decisions: owner-decisions.md. Detail behind each item: options-draft.md §4 and §11, improvements-synthesis.md (M1-M23, S1-S9).

## Section 1: overall shape (approved)

- Per device: agent hooks redact each event and append it to `raw.db` (raw records, the source of truth, kept forever). A worker per device (started by hooks, exits when idle) reads raw.db in order by per-device sequence number and runs consumers: full-text index, handoff manifest, embeddings, curation (AI), digest, sync. Derived data lives in `knowledge.db` and can be rebuilt from raw. Outputs: SessionStart and per-prompt injection, MCP search/get/timeline, viewer, CLI.
- Everything leaving the machine (LLM calls, embeddings, sync) passes one egress gate (redaction, exclusion).
- Devices: optional Cloudflare relay (Worker + Durable Object op log). No summarization in the cloud at first; the design keeps a cloud curator addable later (curation is a consumer of raw; raw sync is opt-in, off by default).
- AI tiers: none / free + local / subscription CLIs (daily cap) / paid (monthly cap). Recording, full-text search and manifests work with no AI.
- One program (Rust) + SQLite, justified by one-command install on Windows, macOS and Linux and by reusing the measured search.

## Section 2: recording (approved)

- Hook: agent events (prompt, tool call and output, assistant reply, compaction, end) are redacted and appended to raw.db with a per-device sequence number within 20 ms; hooks never wait on AI; the curator's own CLI sessions are not captured.
- Stored: device, agent, session, repo (origin URL key), branch, time. Tool outputs kept in full (today cut to 600 characters); a single output over about 256 KB keeps head and tail with an explicit marker and original size. Compressed per record, kept forever (estimate a few hundred MB to 1 GB per year, to be measured).
- raw.db uses synchronous=FULL; startup reconciles consumer checkpoints above raw's highest sequence.
- Write failures: agent never blocked; classified, doctor red, next injection says recording failed since T.
- Backups: sealed compressed segments with checksums on worker idle exit; warn if inside OneDrive/iCloud/Dropbox; quarantine and restore on corruption; forget rewrites backups.

## Section 3: making knowledge (approved, with two judge-model roles)

- Curation windows: the worker reads raw from its checkpoint, cuts at turn boundaries, sizes windows to the provider's context (not a fixed 16k). Oversized tool outputs become markers recorded as "seen, elided". The checkpoint moves only in the same transaction as the window's knowledge. Failed provider: next in chain; all failed: visible backlog. Subscription CLIs are not used while the owner is actively working.
- Claim kinds: decision, preference, lesson, fix (symptom, cause, fix, commit), open item, repo fact, change (with why). Fields: speaker (user / assistant proposal / tool result / imported), status (decided / proposed / retracted / done), evidence (verbatim quote + raw anchor), supersedes, scope (repo or global), valid_from.
- Gates in code: decided needs a verbatim user quote or an acceptance right after the proposal, and a proposal restating pasted or tool text needs the owner's restatement; done/retracted need a user quote or a passing run; global scope only from explicit owner statements; supersedes only among candidates shown (current decisions found by repo-wide hybrid search) and reversals within one window are linked; change without why is not injected; file and tool content is quotation, never instruction.
- Current = chain tips; concurrent conflicts resolved by (valid_from, device, seq) with a viewer flag and a clock-skew alarm; digests cite current claims and are not used when stale; re-derivation keeps uids (highest tier active); owner corrections are events that survive rebuild; the none tier hides a directive followed by a negation.
- Judge models (Jev and similar), both enabled only if evaluation shows a gain: (a) "shrink, never drop" selection of what the curator sees (deterministic rules first, judge for grey areas; user and assistant messages never filtered; shrinking recorded); (b) veto-only check on decided/supersedes (can only make stricter). Tier mapping: none = code gates only; free/local = small local model; subscription = curator double-check; paid = Jev.

## Section 4: delivery and search (third version; owner: "OK if truly no problems")

- Principle: hooks only read; the worker computes ahead. The worker keeps per checkout a SessionStart packet (ranked current decisions, manifest, fresh digest) and per session a shortlist of about 50 relevant current claims (reranked, judge-scored), refreshed at each Stop, every N events and when sync delivers new knowledge. Sync is done by the worker; hooks never wait on the network.
- Hook: reads the packet; per prompt, a light search of the prompt over the shortlist plus a threshold (tens of ms). Before the worker is up: fast path (hybrid FTS + vectors + threshold) and wake the worker.
- Latency budget is a setting, measured per tier; default read-only; users with a strong GPU may run the reranker live (target about 1 s). No agent caps hooks below 1 s (Claude Code and Cursor default 60 s, agy 30 s, OpenCode none, Pi 2 s set by oboete).
- SessionStart: explicit global preferences, the checkout's manifest, current decisions/open items/lessons (about 10 with bodies, ranked by relation to the manifest and recency; the rest as a one-line index with get/search/timeline guidance), the digest only if fresh; fenced as data, attributed.
- Imported memories (claude-mem ~150k and current oboete): labelled imported, search and timeline only, status unknown, never injected or used as current.
- Per-prompt injection: only current claims, never prompt text; threshold calibrated on 50 no-answer and 30 false-premise questions; default on only if irrelevant injections are 10% or less (decision 12).
- Compaction: re-inject open items and current claims after compaction (deduplicated); no double injection on resume; per-agent table of live-verified / implemented / unverified.
- Mid-session correction: at the next prompt (Grok: first tool use).
- Manifest (deterministic, per repo x branch x device): risky git state first, last failing command, current decisions and open items, the agent's todo list, last prompt and reply, files touched, as-of and not-yet-curated count, other active sessions on the repo; fixed drop order under each agent's size cap.
- Search (MCP, CLI, viewer): hybrid SQLite FTS5 trigram + bge-m3 vectors + RRF (measured nDCG@10 0.545 vs claude-mem 0.244 on the owner's data); current claims first, superseded labelled and shown with history=true; since/until; evidence-strength and repo labels; reranker on MCP and viewer if it clears the evaluation line; MCP output fenced as data.
