# oboete: three architectures and a recommendation

Design memo, 2026-09-24. Inputs: the product brief, owner decisions (search-sync proposal §0/§0.1 and the 2026-09-24 decisions in the brief), the scored and deepened ideas, and the verified landscape of existing memory engines. Where a landscape claim was refuted by a verifier, this memo uses the corrected text. Where the two votes split, it says "uncertain".

## 0. Summary

| | A. Hindsight as the engine | B. Self-built: raw log is truth, claims ledger, local replicas (**recommended**) | C. Current design repaired in place (null hypothesis) |
|---|---|---|---|
| Knowledge engine | Hindsight (MIT, Python, PostgreSQL) | oboete (Rust), two SQLite files per device | oboete (Rust), one SQLite file |
| Unit of knowledge | Hindsight facts and observations, plus an oboete decision ledger | Typed claims with speaker, anchor and supersedes edges; deterministic handoff manifests | Observations and session summaries with fields added (PR-K1..K4) |
| Process model | Hindsight daemon (pg0) plus oboete hooks | Hooks plus one worker per device, started by hooks and exiting when idle; a service only if measurement shows it pays | Hooks plus a detached `observe` that exits when done |
| Devices | One Hindsight server reachable by every device (no sync exists) | Op-log sync over an optional Cloudflare hub or a plain shared folder | Op-log sync per the existing §4 proposal |
| Biggest risk | Deletion and sync must-keeps fight the engine; Windows-native install unknown | The most self-written surface: sync and deletion correctness, and curator gate quality in Japanese | It keeps the artifacts that caused the failures (summaries as state, one DB mixing truth and derived data) |

Recommendation: **B**. Build it by reusing the parts of the current code that fit: the 7 adapters, redaction, hybrid search, repo identity, device ids and uids, the provider chain and the evaluation harness. Replace the event/observe core instead of patching it. The worker is not assumed. It has to earn its place in a pre-stated test (§9, M5). A stays alive only if two cheap Phase 0 checks pass: Windows-native install, and hard delete through the public API. After that, Hindsight's recall must beat the current hybrid by the existing +0.03 nDCG@10 line on the owner's 112 questions.

## 1. What the brief forces, whatever the architecture

These follow from the owner goals and must-keeps, not from any existing code:

1. **Capture never waits on AI.** On every tier, including none, a redacted record is stored within the hook and can be found by full-text search within seconds.
2. **The raw log is the only truth, and everything else is rebuildable.** Raw records (redacted, compressed) are kept forever. Summaries, claims, indexes, vectors and caches are derivations with a recipe (model, prompt version, input set), so "re-derive with a better model" is a normal operation.
3. **Progress is counted, never inferred.** Curation progress is a raw sequence number committed in the same transaction as the derived rows. Nothing can be marked processed unless it was sent. This makes the 24-hour/4% failure impossible.
4. **Knowledge has a truth model.** Every claim carries its speaker (user, assistant, tool, import). Decisions and preferences are current only if nothing supersedes or retracts them. Code decides status and scope. The LLM only proposes.
5. **Global scope comes only from an explicit owner channel** (decision 13: `oboete pref add` or the viewer button). A quote found inside a prompt is never enough. This settles the load-bearing risk of the attacker-8 idea by owner decision: its taint-span heuristic is optional research, not a requirement.
6. **Deletion is a tombstone plus a physical purge.** A tombstone is replicated and never compacted, every derived artifact is physically purged, and every inbound path (sync, import, re-derive, restore) checks a deny-list. A read-time filter is only a backstop (the logistics-5 trap).
7. **One egress gate.** The second redaction pass, per-repo sync exclusion, per-provider budgets and self-capture exclusion all sit on one path through which everything leaves the machine: LLM calls, embeddings and sync (§4.8 of the proposal).
8. **Curation runs on the recording device** (decision 6). Workers cannot run subscription CLIs, and shipping raw events to the cloud for summarisation widens exposure.

How the current design measures against these (from `docs/plan.md` and the code):

- Hook p50 7 ms / p95 8 ms. `observe` VmHWM 13 MB. Both are good, and the brief no longer requires them.
- `src/db.rs:848` deletes raw events once a session is summarised (`DELETE FROM events WHERE session_id=?1 AND id<=?2`). That breaks item 2 and is how the unsent middle of a long session disappears.
- `src/observe.rs` caps the prompt at `MAX_PROMPT_CHARS = 16_000` (first and last 8,000). Each summarizer run keeps at most `MAX_OBSERVATIONS = 12` observations (`take(MAX_OBSERVATIONS)` at line 229), however much work the run covered.

## 2. What existing engines already solve (a fair ledger)

The table lists what a self-build would have to write and each engine's conflict with the must-keeps. It uses corrected text where votes refuted a claim.

| Engine | Solves (a self-build must write it) | Conflicts with brief | Use |
|---|---|---|---|
| **Hindsight** (MIT, v0.10.1, verified facts) | Retain = one LLM call per chunk (facts, entities, time, causal); observations consolidated with evidence quotes and rebuilt when sources are deleted; recall with 4 arms (semantic, BM25, graph, temporal) + RRF + CPU cross-encoder, no LLM at recall, 100–600 ms; 25+ providers incl. claude-code and codex CLI and a none mode; official Claude Code plugin; a coding-agents package whose README names Claude Code, Codex CLI, Cursor CLI, opencode, Antigravity CLI (`agy`), pi and Grok Build (all 7 agents except the Cursor IDE), with automatic ingestion; docs list Windows x86_64 as fully supported with embedded pg0 (not tested by us) | PostgreSQL only (embedded pg0 server on port 5555; even embedded Python mode starts a daemon), 512 MB–1 GB RAM; no hard delete of a single memory (invalidate moves rows to an archive table); Mental Models not marked stale by deletions; no device sync (shared server or paid Hindsight Cloud); English-only defaults (bge-m3 and pgroonga are configurable); benchmarks vendor-run (LongMemEval 94.6% on its own AMB suite); one dominant contributor, 70 releases in 10 months | Candidate base engine (approach A) |
| **Cognee** (Apache-2.0, v1.6.0) | Embedded default stack (SQLite + LanceDB + Kuzu), no daemon for the SDK; keyless remember/recall via GLiNER + local embeddings (confirmed); forget() cascade with ownership tracking (fit table, not vote-checked); hook capture for Claude Code, agy, OpenCode (Codex hook capture: **uncertain**, votes split) | No content redaction on ingest (confirmed; oboete would redact first anyway); no sync; no subscription-CLI provider; Kuzu's upstream is unmaintained (Graphiti deprecated its Kuzu backend for that reason); all-Postgres mode is a licensed product; Japanese quality of default models unknown | Fallback engine for A, if Hindsight fails on install or deletion |
| **Graphiti** (Apache-2.0) | Bi-temporal facts with LLM-judged edge invalidation (fit table); strong episode provenance | Every write needs an LLM (no none tier); the chunking module is **not wired** into add_episode on current main (corrected), so it does not counter head/tail truncation; an embedded path exists (falkordblite, deprecated Kuzu) (corrected); no sync; no docs for 6 of the 7 agents (confirmed) | Design reference for supersession |
| **claude-mem** (Apache-2.0, open-core) | Continuous per-event write path; hook adapters for Claude Code, Codex, Cursor, Windsurf, agy, OpenCode; a sync-hub on Worker + Durable Objects with an append-only oplog | Supersession is "future work" (confirmed); Grok Bot is not Grok Build and Pi is absent (confirmed); sync-hub: the engine ships Apache-2.0 in-repo with pluggable verifier/projector URLs, and only the default token verification and projection point at cmem.ai (corrected; whether it runs fully self-hosted in practice is **uncertain**) | Donor for the sync op-log (owner decision 21) |
| **Mem0** OSS | Batching every 5 exchanges plus idle/session-end flush, splitting oversized input rather than truncating (agent-plugin-core); plugins ship local stdio MCP servers (corrected) and degrade to local-only without a key (corrected) | add() is additive only; supersession features (Dream, Temporal Reasoning, Memory Decay) are Platform-only (confirmed; OSS keeps a manual update()); no sync; OpenMemory removed 2026-07-29 | Pattern only |
| **Supermemory** | updates/extends/derives relations; chunked "dreaming" pipeline; single self-host binary on all three OSes | Self-host has no official MCP (confirmed; the REST API could be wrapped); memory-level forget is a soft delete (confirmed); no agy or Pi (a Grok Bot doc exists, not Grok Build) | Pattern only |
| **Basic Memory** (AGPL-3.0) | Markdown as source of truth; redaction module; Pi autoCapture stores turn text (corrected) | temporal.py valid-time model with manual range closing; whether it is bi-temporal is **uncertain** (votes split; one says uni-temporal with supersession as a separate decision-schema field); multi-device is paid cloud or manual git/Syncthing; AGPL | Reference only |
| **MemOS / memU / Letta** | Per-step capture (MemOS); host session-log adapters and the self-mining exclusion problem (memU ADR15); sleep-time async curation (Letta) | MemOS: brute-force vectors with a self-declared ~100K-row ceiling (confirmed), no adapters for the 7 agents. memU: no redaction (confirmed), no MCP (confirmed). Letta: the memory server is retired and Letta Code is a competing harness (confirmed) | Reference only |

No existing engine meets every must-keep: Hindsight itself has no hard delete of a single memory and no device sync. Hindsight is the only candidate whose gaps sit outside its core (deletion can be enforced around it, at the cost of coupling to its internal schema; sync needs one always-on server), while it already covers extraction, consolidation, 4-arm recall and 6 of the 7 agents. Cognee is a weaker fallback (no subscription-CLI provider, no sync, unmaintained Kuzu upstream). claude-mem was not evaluated as a base engine here (no supersession; its sync hub's self-host viability is uncertain).

## 3. Approach A: Hindsight as the engine

### Shape
One Hindsight server holds all knowledge, with one memory bank per repository keyed by origin URL. oboete shrinks to a shell around it: the 7 adapters, redaction, a local raw archive kept forever, an OpenAI-compatible **egress shim** through which Hindsight's LLM calls go, a small **decision ledger**, a deletion enforcer, and the injector and MCP facade. The decision ledger exists because nothing verified about Hindsight tells a proposal from a decision or guarantees that a retracted policy is never current.

### Pipeline
| Step | Where | When | AI tier |
|---|---|---|---|
| Hook: redact, append to the raw archive and to oboete's own raw FTS | Hook process | Per event | None |
| Chunk at turn/tool-call boundaries; elide oversized tool output with an explicit marker; `retain(document_id = oboete chunk uid)` | oboete ingester | Seconds to minutes | Hindsight → oboete shim → provider chain (subscription/free/paid, daily caps). Hindsight's native claude-code/codex providers are not used, because they would bypass the egress gate and the budgets |
| Facts, entities, time, causal links; consolidation into observations with evidence quotes | Hindsight | Per chunk (1 LLM call) plus consolidation | Same |
| Decisions, preferences and open items extracted by oboete's gated curator | oboete | Per window | Same |
| SessionStart: decision ledger + handoff manifest, fenced and attributed; `recall("state of <repo>")` only if it fits the budget | Hook → oboete (→ Hindsight) | On demand | None at recall (cross-encoder on CPU) |
| MCP search/get/timeline | oboete facade → Hindsight recall | On demand | None |

Hindsight recall takes 100–600 ms (verified), and the SessionStart injection budget is p95 ≤ 300 ms (§9, M5). A live recall at SessionStart would therefore likely miss the line by construction. A's resume would come from the ledger and manifest, with Hindsight used by MCP search (≤ 1.5 s budget) or a cache refreshed after each retain.

No-AI tier: what Hindsight's none mode stores and returns is **unknown**. oboete therefore keeps its own raw full-text index regardless, so A runs two search stacks.

### Currency
Observations are rebuilt when their sources are deleted (verified). Whether a later contradicting fact retires an earlier one inside observations is not in the verified facts (unknown). Mental Models are not marked stale by deletions, so A disables them. Supersession and retraction of decisions live in oboete's ledger. The weak point: Hindsight's recall text can still quote a superseded decision in resume or lookup output, and oboete cannot filter free text reliably. A therefore carries two truth models that can disagree in injected context.

### Devices
Hindsight has no sync. The only option that does not multiply AI spend is one server reachable by every device:
- **Owner:** Hindsight in WSL on the PC. Windows native reaches it via localhost forwarding. The iMac reaches it over LAN or Tailscale and loses recall while the PC is off.
- **Always-on server:** a VPS (undecided) or paid Hindsight Cloud. A normal Cloudflare Worker cannot host PostgreSQL or a Python daemon; whether Cloudflare's container offering could was not checked.
- Rejected: one Hindsight per device, each re-retaining synced raw. That multiplies LLM spend by the number of devices and gives each device different memories (the biology-8 trap).

Propagation is immediate while the server is reachable. A single device without cloud works, since the server is local.

### Deletion
oboete appends a tombstone, purges the raw archive span and its own FTS, and deletes the source document in Hindsight, whose observations are then rebuilt (verified). But Hindsight has no hard delete of a single memory: invalidated rows move to an archive table. Meeting "cannot be resurrected" means oboete deletes archive rows and their embeddings with SQL against Hindsight's internal schema. That is fork-level coupling to a schema moving at 70 releases in 10 months. A PostgreSQL restore brings rows back, so oboete must re-run the purge from its deny-list after any restore.

### Install for other users
Python (uv) + Hindsight + the embedded pg0 PostgreSQL daemon + the oboete binary. For Japanese, bge-m3 embeddings plus pgroonga for BM25. **Hindsight's docs list Windows x86_64 as fully supported with embedded pg0 (untested by us); whether pg0 ships pgroonga is unknown.** If either fails, Windows users need WSL or Docker, which breaks "easy install for anyone, day one". Idle RAM is 512 MB–1 GB.

### Known failures
| Failure | A's answer |
|---|---|
| 24 h session, ~4% reached the summarizer | Fixed structurally: per-chunk retain, no session batch. oboete keeps the coverage ledger (retain success per chunk uid), since Hindsight's own accounting is unknown |
| Proposal recorded as decision | oboete's gated decision ledger, the same as B. Hindsight's facts carry no verified speaker attribution |
| Retracted decision injected as current | The ledger handles decisions, but Hindsight recall text can still carry the old decision (two truth models) |
| "N lines changed" without a reason | Depends on Hindsight's extraction prompt (configurability unknown); oboete pre-filters and requires `why` in its ledger |
| Old summaries injected as current state | No session summaries exist; the temporal arm helps; oboete's dated handoff manifest is still needed |

### Cost
- **Build:** about 14 PR-sized units. Raw archive and ingester with coverage (2), egress shim (1), decision ledger and gates (3), deletion purge into Hindsight's schema plus deny-list (2), handoff manifest (1), cross-OS installer that provisions Python/pg0/Hindsight (2–3, the riskiest), remote MCP facade with per-repo grants (2). Tracking upstream is a permanent cost on top.
- **Run:** one LLM call per chunk. A heavy day has 1.5M characters of raw work. After eliding tool output, perhaps 0.3–0.5M remain. At an assumed ~3k characters per chunk that is ~100–170 retain calls plus consolidation. Through `claude -p`, that is roughly 0.5–1.5 CLI-hours on a heavy day, drawn from the daily cap (estimate; chunk size is configurable but unmeasured). The existing ~180k documents plus ~150k claude-mem observations would cost one call each (~330k calls), which is not feasible on the subscription tier or $5/month. They would stay outside Hindsight's fact memory, in none mode or in oboete's index only.

### Biggest risk
The must-keeps oboete cannot give up fight the engine. Hard deletion needs writes into an internal schema, multi-device needs an always-on PostgreSQL server that Cloudflare cannot host as a Worker, and injected context holds two truth models. The Windows-native install is also unknown.

## 4. Approach B: self-built, raw log is truth (recommended)

### Shape
Each device has two SQLite files and one short-lived worker:
- `raw.db`: append-only, redacted records (zstd per record), a per-device monotonic `seq`, never touched by derivation except tombstone compaction.
- `knowledge.db`: everything derived. That is raw-chunk FTS rows, vectors, typed **claims**, **handoff manifests**, repo digests, the coverage ledger, and a checkpoint per consumer committed in the same transaction as that consumer's output. `oboete rebuild` drops it and replays from `raw.db` as an ordinary operation (the oncall-3am-8 idea).

Consumers (index, embed, curate, digest, sync-out) each advance their own `seq` checkpoint. Devices exchange an op log (claims, manifests, vectors, tombstones, repo-touch sets, prompts) through a pluggable transport: an optional Cloudflare Worker + Durable Object hub, or a plain shared folder. Search, injection and MCP read the local store, so one device is the same program as N devices minus the transport.

The deepened idea proposed custom segment files with CRC framing. They are not needed: a separate SQLite file gives the same crash safety and the same "two failure domains". Sealed zstd segments appear only as a backup/export format.

### Re-justifying the old shape from the brief
- **Rust single binary:** kept because of the install goal (Windows native, macOS, Linux with no Python, PostgreSQL or Docker), and because the reusable assets (adapters, redaction, the 0.545 hybrid search) are Rust. Lightness is no longer the reason.
- **SQLite:** a per-device embedded replica with no server to install; FTS5 trigram is proven on the owner's Japanese (0.443 alone, 0.545 hybrid); 180k documents is small.
- **No resident process:** dropped as a rule, replaced by a measured choice. A worker is started by any hook when absent (pid/lock file), stays while hooks fire or work is pending, and exits after an idle window (default 30 minutes). It earns its place by curating windows *during* a 24-hour session, batching embeddings, keeping a local embedding model loaded, retrying failed windows with backoff, and pulling sync while a session runs. Registering it as an always-on service (systemd --user, launchd, Task Scheduler) is optional, and M5 in §9 decides whether that is the default. On WSL the VM shuts down when idle, so spawning from a hook is the real guarantee there. The reverse is a cost: a worker with a 30-minute idle window keeps the WSL VM, and its RAM, alive for 30 minutes after the last hook. M5 measures this. The viewer stays on demand (decision 18).
- **Batch summary after a session:** dropped. Curation is windowed and incremental.

### Pipeline
| Stage | Where | When | AI tier | Output |
|---|---|---|---|---|
| Capture | Hook process | Per event, ≤ 20 ms (today p95 8 ms) | None | Redaction #1 (`strip_blocks` + gitleaks rules), append to `raw.db` |
| Index | Worker | Seconds | None | Raw chunks at turn/tool-call boundaries in FTS (`kind=raw`, ranked below curated rows; exact handling decided by M1) and the timeline |
| Handoff manifest | Worker | Each Stop, compaction, every N minutes | None | Deterministic: repo (origin URL), branch, HEAD, files touched, the agent's todo list, last user prompts, last assistant message, last failing command tail. Dated "as of". Synced at once (logistics-2) |
| Embed | Worker | Batched per minute | none / local bge-m3 / Workers AI bge-m3 (decision 5) | Vectors with `embedder_id`, synced so no device re-embeds |
| Curate | Worker on the recording device | Whenever the unconsumed tail reaches a window, and at Stop or idle | Provider chain by tier | Typed claims with anchors; coverage rows |
| Gate | Worker (Rust, deterministic) | Per claim | None | Final status and scope |
| Digest | Worker | Lazily, when stale | LLM tier, or a deterministic list as fallback | Per-repo "current state" with a recipe hash |
| Sync | Worker (and a bounded pull in SessionStart) | Poll or WebSocket every 30–60 s | None | Ops out through the egress gate and the exclusion check; ops in through the deny-list |
| Inject / MCP | Hook / MCP server | On demand | None (optional CPU reranker, survey E-series) | Fenced, attributed, dated |

**Curation windows.** The unconsumed raw tail is cut at turn boundaries into windows sized to the chosen provider's context, not a fixed 16k. A tool output over a cap is replaced by a marker (length, hash, seq). The coverage ledger records that span as "seen, elided", never as silently processed. Each window gets the carried state of the session so far (goal, open items, and the live decisions shown for supersession, per PR-K2). The checkpoint moves only in the transaction that writes the window's claims and coverage rows. A failed provider leaves the window pending for the next provider in the chain. When every provider fails, the window stays in a visible backlog (doctor, viewer, and an "N windows pending since …" line in the injection). Curator runs set a marker that the adapters honour, so the curator's own `claude -p` sessions are never captured (memU ADR15 is the precedent).

**AI tiers.**
- **None:** record, raw FTS, manifest, explicit owner directives, imports. Owner lines containing directive markers ("from now on", "今後は", "やめて") appear in the manifest as dated, quoted, *unverified* owner lines. They never become decided claims.
- **Free APIs and local models:** the curator runs on free APIs or Ollama; embeddings come from local bge-m3 or Workers AI.
- **Subscription CLIs:** `claude -p`, `codex exec`, agy and grok, each with a daily cap.
- **Paid APIs:** the last fallback, with a monthly cap (owner default ≤ USD 5).

### Truth model (goal 2)
- **Claim kinds:** decision, preference, lesson (a failure and what to do instead), fix (symptom, cause, fix, commit), open_item, repo fact, change (must carry `why`).
- **Fields:** uid; repo; scope (repo|global); speaker (`user_said`, `assistant_proposed`, `tool_observed`, `imported`); status (`decided`, `proposed`, `retracted`, `done`); `user_quote` with anchor (device, seq span, byte range); `supersedes[]`; valid_from; recipe.
- **Gates, in Rust:**
  1. `decided` requires `user_quote` to appear verbatim in a user-authored record inside the window (PR-K1), or an acceptance turn ("はい", "それで", "OK, go ahead") directly after the assistant proposal it references. Anything else is `proposed`.
  2. `global` scope comes only from OwnerDirective events (`oboete pref add`, the viewer button, or an optional `/remember-global` prefix the adapter reads from the human input path) (decision 13). The LLM may only nominate "widen candidates".
  3. `supersedes` ids must come from the list shown to the curator. Unknown ids are dropped (PR-K2).
  4. A `change` without `why` is searchable but never injected (PR-K3).
  5. Text from tool output or file content can only become `tool_observed` evidence, rendered as a quotation, never as an instruction.
- **Current = chain tips.** A claim is current unless a live supersedes, retract or done edge points at it. This is a deterministic function of the synced claim set, so every device computes the same state once caught up (attacker-5, biology-7). The timeline shows the whole chain.
- **Concurrent supersession** from two devices produces two tips. They are shown as a conflict on both devices, not resolved automatically. With one owner this is rare.
- **Derived-summary freshness.** A digest records the (uid, version) set it was built from. Any supersession or deletion touching that set marks it stale (oncall-3am-7). A stale digest is never injected: SessionStart falls back to the claim tips plus the latest manifest. Session summaries are never injected as current state, only returned as dated search results.

### Lookup (goal 3)
The existing hybrid search (FTS5 trigram + bge-m3 bit index + RRF) runs over claims, digests, manifests, raw chunks and claude-mem imports. The imports are read-only with `source = claude-mem`, in search but not in auto-injection (decision 1), and need no LLM calls. A fix or decision answer cites its claim and the claim's raw span. On the recording device the span resolves to the exact tool call. Other devices get the quote carried in the claim, plus raw chunks if raw sync is enabled.

### Devices
- **Op log:** owner decision 21 takes claude-mem's CloudSync/SyncHub/canonical-content as the donor (Apache-2.0, credited in NOTICE). Its other clause ("search and recording without a resident process, as designed") is superseded by the 2026-09-24 decision that resident processes are acceptable if they earn their place.
- **Cloudflare hub (optional):** a Worker plus one Durable Object per user holding the op log. Vectorize serves only the remote MCP for the Claude app, with per-repo grants (decision 3; stop line $1.5/month). Propagation is a 30–60 s poll or a WebSocket, well within minutes. SessionStart does a bounded pull. If the pull does not finish in budget, the injection says "synced N min ago".
- **Folder transport (no cloud):** each device writes only its own append-only op files into a shared folder, and any file syncer moves them. With one writer per file there are no merge conflicts. WSL and Windows native on the same PC are two devices sharing `/mnt/c/...` files. They must not share one SQLite file, because locking over drvfs/9P is unsafe. For the iMac, the latency is whatever the syncer delivers (Syncthing is seconds; others unverified).
- **What travels:** claims, manifests, vectors, tombstones, repo-touch sets and prompts (decision 2). Raw chunks travel only if the owner opts in (§10). Curation happens once, on the recording device, so no leases are needed. Exclusion is checked at send time, and control ops (tombstones, touch-set updates) always travel (decision 11).
- **Considered and rejected: the hub as the only store.** Deletion would be simpler, but offline devices would lose recall, SessionStart would depend on network latency, and the search would need a second runtime in Workers. The hub stays a relay plus the remote-MCP index.

### Deletion
`oboete forget <uid|session|repo|span>` appends a tombstone that is never compacted. Each consumer then:
- purges its FTS rows and vectors;
- deletes claims whose anchors lie inside the span, and re-derives those whose anchors are partly inside;
- marks affected digests stale and rebuilds them;
- rewrites `raw.db` without the records, keeping seq numbers stable (checkpoints are seq, never byte offsets).

At the hub, the Durable Object replaces the payload with the tombstone, Vectorize deletes by id, and R2 objects are deleted if raw sync is on.

A deny-list (tombstoned uids plus content hashes) is checked on every inbound path: sync receive, folder import, claude-mem re-import (keyed by source id), re-derive, and `oboete restore`. oboete's backups are raw segments plus tombstones, and restoring one is an import against the current deny-list. An old device or an old backup therefore gets the tombstone back on its next sync and purges again.

Honest limit: a single device with no transport, restored from a whole-disk image older than the delete, has no surviving copy of the tombstone. No software on that device can prevent it.

### Install for other users
- **Get it:** binaries from GitHub Releases for Linux x64/arm64, macOS arm64/x64 and Windows x64, with a one-line sh or PowerShell installer.
- **`oboete setup`** detects the 7 agents, writes hooks, plugins and MCP config, asks for the AI tier and embedding mode, and offers the optional service.
- **`oboete doctor`** shows, per repository, minutes since the last successful index (oncall-3am-5), pending windows, coverage %, provider budget left and last sync.
- **`oboete update`** is explicit (decision 15).
- Local bge-m3 is an optional download during setup. It is large, on the order of a gigabyte or more; the exact size depends on the quantisation shipped and must be checked before release.

### Known failures
| Failure | B's answer |
|---|---|
| 24 h session, ~4% reached the summarizer; 12-observation cap | Windows consumed by seq; raw never deleted; coverage = 100% (curated, elided-with-marker, or skipped-with-reason) or a visible backlog; no per-session output cap |
| Proposal recorded as decision | Speaker field plus the verbatim-quote/acceptance gate, in code |
| Retracted decision injected as current | Supersedes/retract edges; only chain tips are injected; stale digests never injected |
| "N lines changed" without a reason | `why` required for injection |
| Old summaries injected as current state | Summaries are never state; manifests are dated; digests carry freshness |

### Cost
- **Build:** about 16–20 PR-sized units. Reused: adapters (4 not yet live-verified), redaction, hybrid search, repo identity, device ids and uids (PR-C2/C3), provider chain, evaluation harness. New:
  - `raw.db` + checkpoints: 2
  - worker lifecycle on 3 OSes: 2
  - windowed curator + coverage: 2
  - claims + gates + supersession: 3
  - manifest: 1
  - digest freshness: 1
  - deletion, deny-list and compaction: 2
  - hub port from the claude-mem donor + folder transport: 3
  - remote MCP with grants: 2
  - doctor/viewer: 1–2
- **Run:** 0.3–0.5M characters after elision on a heavy day, at ~50k characters per window, is ~6–10 curator calls plus a few digests. Whether that is fewer calls than A depends on both chunk sizes, which are configurable and unmeasured (M5). Embeddings through Workers AI stay within the included allowance (plan.md: 300/day ≈ 1% of the free neurons). The Durable Object fits in Workers Paid. Worker RAM target is < 150 MB without a local model (to measure); a loaded local bge-m3 adds several hundred MB while resident.

### Biggest risk
B is the largest self-written surface. Sync and deletion correctness across replicas is testable with canary strings (M4). The curator gate is the real unknown: too strict, and real decisions stay `proposed` and goal 2 quietly degrades; too loose, and failure 1 comes back. Gate precision and recall on Japanese sessions is measured before anything else is built on it (M3). A secondary risk is the worker's lifecycle on WSL (VM idle shutdown) and on Windows native. Spawning from a hook limits the damage.

## 5. Approach C: the current design repaired in place (null hypothesis)

### Shape
Keep one binary, one SQLite file and no resident process, with `observe` spawned detached by hooks under a single lease. Apply the existing PR plan:
- Stop deleting raw events (`db.rs:848`).
- Replace the head+tail 16k prompt with a loop over windows, with the event-id cursor committed in the same transaction.
- Remove the 12-observation cap.
- Add PR-K1 (status + user_quote), PR-K2 (supersedes), PR-K3 (why) and PR-K4 (index-form SessionStart).
- Sync and tombstones as in proposal §4.

The per-tool-call hook can spawn `observe` when pending events cross a threshold, so long sessions are curated while they run.

### Pipeline, currency, devices, deletion
The pipeline is the same as B's minus the worker. Curation and embedding run in short-lived processes started by hooks, and sync is pulled at SessionStart and pushed from hooks. Currency comes from the K1/K2 fields on observations. Deletion comes from §4.3 tombstones, but raw and derived rows share one database, so "drop derived and replay" is not an operation C has. Install is the same single binary as B, with no service at all.

### Where C converges on B, and where it does not
It converges on B in raw retention, windows, transactional cursors, speaker/quote gates, supersedes, and tombstones. It differs in four places:
1. **Session summaries stay a first-class artifact.** The failure "old summaries injected as current" is then held off by injection rules rather than removed.
2. **Truth and derived data share one file.** Re-deriving with a better model means careful in-place migration instead of rebuilding a file.
3. **Nothing runs between hooks.** Retries wait for the next hook, SessionStart must pull synchronously, and a local embedding model is reloaded by every process that needs it.
4. **The current schema bends the new fields.** Observations carry kind/title/body. A claim model with anchors and edges is bolted onto it.

### Known failures
| Failure | C's answer |
|---|---|
| 24 h session, ~4% reached the summarizer; 12-observation cap | Raw kept; `observe` loops over windows with a transactional cursor; cap removed; hooks spawn `observe` on a pending threshold. Retries wait for the next hook |
| Proposal recorded as decision | PR-K1 (status + verbatim user_quote check), as in B |
| Retracted decision injected as current | PR-K2 supersedes, as in B |
| "N lines changed" without a reason | PR-K3 `why`, as in B |
| Old summaries injected as current state | By injection rule only: PR-K4's index-form SessionStart and a "never inject summaries as state" rule, while session summaries remain stored and searchable artifacts |

### Cost
- **Build:** the lowest of the three, about 10–12 PRs, much of it already specified (PR-K1..K4, sync §7).
- **Run:** the same LLM calls as B, with no idle RAM.

### Biggest risk
C preserves the artifacts that caused the failures and fixes them by rule rather than by structure. Its process model is only acceptable if it passes the same latency and propagation lines as B's worker (M5). In practice, C's increments are a valid **build order** toward B, but not a valid **end state**, because of points 1 and 2.

## 6. Side by side

| Criterion | A | B | C |
|---|---|---|---|
| Resume across agents/devices | Immediate if the server is reachable; iMac blind when the PC is off (no VPS); live recall (100–600 ms) cannot fit SessionStart's 300 ms, so resume rests on oboete's ledger and manifest | Within minutes; offline devices still have everything synced so far | Within minutes of the next SessionStart pull |
| Decisions never stale | Ledger yes; recall text may still carry old decisions | Structural (chain tips + digest freshness) | By injection rules |
| Lookup quality | Possibly highest (4 arms + cross-encoder), unmeasured on Japanese | Current 0.545 hybrid + claims + raw chunks; reranker addable | Same as B |
| No-AI tier | Two stacks (Hindsight none mode unknown + oboete FTS) | Record, FTS, manifest | Record, FTS |
| Deletion must-keep | Needs SQL into Hindsight's internal schema | Designed in (tombstone, purge, deny-list) | Designed in, but no rebuild-from-raw |
| Install on Windows native | Unknown (pg0) | Single binary | Single binary |
| Cloud optional | Needs an always-on server for multi-device | Hub or folder | Hub (folder possible) |
| Build cost | ~14 PRs + permanent upstream tracking | ~16–20 PRs | ~10–12 PRs |
| Run cost (heavy day) | Per-chunk LLM calls (chunk size unmeasured), 0.5–1 GB RAM | Per-window LLM calls (window size unmeasured), < 150 MB (target) | Same as B, no idle RAM |

## 7. Recommendation

**B.** It is the only option where each of the five known failures is excluded by structure rather than by rule, and where every must-keep has a designed mechanism: rebuild from raw, seq coverage, chain tips, tombstones plus deny-list, one egress gate. It also keeps the single-binary install that A cannot promise on Windows native.

Build it in an order that starts from C's cheapest increments, so value ships early:
1. Stop deleting raw events and split `raw.db` from `knowledge.db`.
2. Windowed curator with coverage, plus the crash-injection test (red against today's code first).
3. Claims, gates and supersession.
4. Manifest and digest freshness.
5. Deletion and deny-list.
6. Sync hub and folder transport.
7. Worker lifecycle, only if M5 says it pays.

What would change this recommendation:
- **Toward A:** if Hindsight installs on Windows native in one command, hard-deletes through its public API with nothing left in archive tables, and beats the hybrid by ≥ +0.03 nDCG@10 on the test split. A's retrieval would then justify its operational cost. Cognee is the fallback engine if Hindsight fails only the install or deletion check.
- **Toward C's process model inside B:** if SessionStart p95 and propagation p95 meet their lines without a worker (M5).

## 8. Facts still unknown

**Hindsight (all decide A):**
- whether pg0 actually runs on Windows native as documented (docs say fully supported; untested);
- whether pg0 ships pgroonga, and Japanese BM25 quality without it;
- what the none mode stores and returns;
- chunk size and LLM calls per 1.5M characters;
- whether deleting a source document physically removes archived facts and their embeddings;
- recall p95 on a large bank;
- Japanese extraction quality;
- whether retain accepts oboete's document ids so results map back to qrels;
- whether Hindsight's LLM client accepts a custom OpenAI-compatible base URL, which the egress shim depends on;
- whether its coding-agents package works live with each of our agents (the README names Grok Build, pi and agy).

**Cognee:** whether Codex capture is hook-based (votes split); the impact of Kuzu's unmaintained upstream; the language coverage of its default local models.

**claude-mem sync-hub:** whether it runs fully self-hosted with oboete's own verifier (the corrected text says the URLs are pluggable; untested).

**Cloudflare:** whether Durable Object SQLite supports FTS5 with the trigram tokenizer, which the remote MCP's full-text arm needs. Otherwise the remote side is Vectorize-only.

**B:**
- gate precision and recall on Japanese sessions;
- elision ratio of tool output on real heavy days;
- subscription CLI daily-cap numbers per provider;
- local bge-m3 size and speed on M1 and on Windows native;
- worker lifecycle under WSL idle shutdown and Windows Task Scheduler;
- folder-transport latency with the owner's syncer for the iMac.

**Data:** whether raw agent transcripts still exist for replay (`~/.claude/projects`, `~/.codex/sessions` and the others), including the 24-hour, 1,923-call session. oboete's own raw events were deleted after summarisation.

## 9. Measurement plan (pass lines fixed before measuring)

General rules carried over from proposal §3.3: tune on the dev split (70%) only, decide on the untouched test split (30%), one comparison, no fix-and-remeasure loop. The LLM judge is used for pass/fail only after it meets the PR-B trust condition. Values marked "proposal" are new and are fixed here, before any run.

**Phase 0: A's gate checks (cheap, no LLM spend).**
- **A-install.** Hindsight + pg0 on Windows 11 native, macOS arm64 and WSL, each from one documented command, with no Docker or WSL on Windows. Record idle and load RSS, whether pgroonga is present, and that the bge-m3 configuration is active. *Pass:* all three install; RSS ≤ 1 GB. A failure on Windows native disqualifies A as the product engine.
- **A-delete.** Retain a document containing a canary string, delete it through the public API, then `pg_dump | grep canary` across all tables, including the archive and embedding tables. *Pass:* 0 hits. Hits only in the archive table mean A needs a schema-coupled purge; A is then kept only if M1's margin is ≥ +0.05.

**M1: retrieval on the 112-question set.**
- **Corpus:** a fixed sub-corpus containing every judged document plus random distractors, up to N documents. N is chosen *before* the run from a measured retain cost per 100 documents, so that Hindsight's retain costs ≤ USD 5 on a paid API or ≤ 2 days of subscription cap.
- **Systems on the same sub-corpus:** FTS only; the current hybrid; Hindsight recall configured with bge-m3 (pgroonga if available, reporting which BM25 tokenizer ran); claude-mem (decision 22); optionally Cognee.
- **Scoring:** Hindsight results map to oboete document uids through `document_id` provenance; unjudged results go to the existing judge.
- **Pass lines:**
  - a system replaces the current hybrid only with nDCG@10 ≥ hybrid + 0.03, p < 0.05;
  - no slice may drop recall@10 by more than 0.02;
  - nothing ships as default below claude-mem on any slice (decision 22).
- **Same run, for B:** the three raw-chunk variants (excluded, included with an RRF penalty, raw only) under the "individual improvement" line (+0.02 nDCG@10, no slice drop > 0.02).

**M2–M6: end-to-end on replayed real sessions.** The replay set is fixed before building: `events-1000.jsonl`, plus 30 held-out real transcripts stratified by length, language (Japanese/English) and agent, plus the 24-hour session if its transcript exists. The same runs apply to B and, if it survived Phase 0 and M1, to A.

- **M2 coverage.** For every session, coverage = 100% of seqs (curated, elided-with-marker, or skipped-with-reason). Crash injection kills the worker at 20 random points mid-window, including between the derived-row write and the checkpoint. Both are one transaction, so either both land or neither does. Derived rows must be identical to an uninterrupted run. The test is written first and must be red against today's code. *Pass:* 100%, identical (hard).
- **M3 decisions.**
  - Overturned-decision slice (≥ 50 human-confirmed pairs, PR-B): superseded or retracted decisions injected or ranked top-10 as current = 0% (hard); control pairs wrongly dropped ≤ 2%.
  - Proposal/decision slice: precision of `decided` ≥ 0.95 and recall ≥ 0.80 (proposal), and better than the current prompt at p < 0.05 (survey row 3).
  - Injection canary: global-scope claims not originating from an OwnerDirective event = 0 (hard).
  - `change` claims without `why` that were injected = 0.
- **M4 deletion.** A canary in a recorded record and one in a claude-mem import. Delete them, then run re-index, re-derive, re-sync from a device that was offline during the delete, claude-mem re-import, and restore from an oboete backup taken before the delete. Grep every file on every device, the hub's Durable Object, the Vectorize ids and R2. *Pass:* 0 hits (hard).
- **M5 resume, and whether the worker earns its place.** Cut each held-out session at a random point after ≥ 30 minutes of work. Label open items and the next step from the rest of the transcript (judge, with owner spot-checks on 20%). Start a fresh session on a second device (separate home directory, over the real transport).
  - *Pass:* open-item recall ≥ 0.80; items shown as open that were already closed or retracted ≤ 10%; knowledge available on device 2 ≤ 5 minutes after the cut (p95); SessionStart p95 ≤ 300 ms; with the none tier, open-item recall ≥ 0.50 from the manifest alone (proposal).
  - Run M5 twice, with the worker and without it (C's process model). The worker is kept only if running without it misses SessionStart p95 ≤ 300 ms, propagation p95 ≤ 5 minutes, or MCP search p95 ≤ 1.5 s on the slowest device with local embeddings. Registering it as an always-on service is the default only if the hook-spawned worker misses the same lines.
- **M6 lookup.** 40 "how did we fix X / why did we decide Y" questions, drawn from the matching types in the 112 plus new ones from held-out sessions. An answer counts only if it is correct *and* cites a span that contains the answer. *Pass:* ≥ 0.70 and ≥ the current oboete + 0.10; cited-span validity ≥ 0.95 (proposal).
- **Cost and install**, reported with every run:
  - curator calls and CLI minutes per heavy day ≤ 20% of each provider's daily cap (proposal); paid spend ≤ USD 5/month;
  - hook p95 ≤ 20 ms; injection hook p95 ≤ 300 ms;
  - worker RSS reported;
  - fresh Windows 11 native, macOS arm64 and Ubuntu: install → setup two agents → a memory recalled in a new session in ≤ 10 minutes, with no manual dependency install and a clean doctor.

**Decision rule.**
- A is chosen only if Phase 0 passes, M1 clears +0.03 (or +0.05 when the delete needs a schema-coupled purge), and M2–M4 pass.
- Otherwise B.
- The worker ships only if M5 shows it pays.

## 10. Decisions this raises for the owner

1. **Raw-chunk sync.**
   - Default: off.
   - On: other devices can search a session's raw tail before it is curated, and lookups on the none tier work across devices.
   - Cost: more redacted raw text leaves the machine.
2. **Folder transport.** Whether a no-cloud transport (a shared folder) is wanted alongside the Cloudflare hub. It is how WSL and Windows would sync without the cloud, and how other users would get multi-device without Cloudflare.
3. **Measurement budget for A.** Retain costs for the M1 sub-corpus: up to USD 5 on a paid API or 2 days of subscription cap, spent only if Phase 0 passes.
4. **Worker lifecycle.** Accept a per-device worker started by hooks, with the always-on service decided by M5.
## 11. Owner-facing corrections and open items added after the critic (2026-09-24 22:55)

- Owner corrections are stored as raw/op-log events (OwnerCorrection), so `oboete rebuild`, re-derivation and sync replay them; M3/M4 test that they survive.
- Fully automatic curation: concurrent supersession from two devices resolves to the later `valid_from` automatically and is flagged in the viewer, never queued for approval. "Widen candidates" are dropped: global scope comes only from what the owner explicitly states (decision 13).
- `rebuild` replays the local raw log plus the kept inbound op log, so knowledge received from other devices survives.
- The remaining critic items (critique.md) become open items of the written spec.
