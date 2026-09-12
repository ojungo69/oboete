<!--
Sync Impact Report
- Version change: 4.0.0 -> 5.0.0 (2026-09-10, owner explicitly permits resident processing
  where it improves convenience and requests model/provider failover after free-tier failures).
- Redefined II: a single leased background worker may wait for new/due work across bounded
  processing passes. Capture remains direct-to-SQLite; this does not restore daemon-owned RPC.
- Expanded IV/observer: configured, consented fallback targets are tried under bounded attempts,
  per-target privacy and verified cost policy. An unselected paid destination remains forbidden.
- Disposition: feature 009 carries the amendment; 007/008 and earlier no-residency language remain
  historical. Runtime activation/daily installation, provider spending, PR/merge and cloud changes
  retain their existing authorization boundaries. The local Spec Kit copy is synchronized.

Previous report (3.2.0 -> 4.0.0):
- Version change: 3.2.0 -> 4.0.0 (2026-09-09, owner-confirmed completion target in
  specs/009-memory-core/spec.md after product review and clarification)
- Redefined I, IV and V: task-aware continuity plus long-term knowledge; recoverable generation
  and evidence retention; Linux/WSL and macOS, migration and device sync required for completion.
- Updated III and product constraints: work/project/personal scope is distinct from sensitivity;
  provider and cost class are user choices; no automatic paid fallback.
- Preserved: SQLite/CLI seam, privacy checks, fenced single worker, agent adapters, explicit
  cloud activation, isolated dogfood and release authorization. Existing code is reusable,
  not evidence that the new product contract has been met.
- Disposition: 007/008 remain historical evidence; 009 supersedes conflicting completion,
  scope, retention and milestone assumptions. Outstanding external issues/PRs are not closed
  or relabeled by this amendment. The local Spec Kit constitution is synchronized.

Previous report (3.1.0 -> 3.2.0):
- Version change: 3.1.0 -> 3.2.0 (2026-09-04, owner decision A19: the Anthropic observer preset is
  removed from the product constraints; decision record in docs/research/m1-amendments-2026-09.md;
  affected: spec 007 research R2/R13, plan Complexity Tracking row 15, contracts/cli.md,
  contracts/observer.md, tasks T035, scripts/e2e/probes/providers.mjs)
- Version change: 3.0.0 -> 3.1.0 (2026-09-03, M1 plan amendments A1-A10; decision record in
  docs/research/m1-amendments-2026-09.md)
- Modified principles:
  - II. One File, No Daemon: "single file" defined as the engine bundle (heavy runtime packages
    stay in node_modules); a loopback port bound by the foreground `oboete view` is allowed
  - IV. Honest Degradation and Bounded Resources: capture hook 300 ms; session-start injection
    may wait up to 1 s for a pending summary
  - VI. Portable and Minimal: one data directory `~/.oboete/` (relocatable via OBOETE_HOME),
    XDG/AppData split deferred (MINOR: expands the portability requirement's timing);
    `@secretlint/core` + preset replaces `@secretlint/node`
- Modified sections: Product Constraints (Codex hooks.json + trust row; Pi child-process
  capture), Development Workflow (`grok -p`)
- Templates: .specify/memory/constitution.md synced; specs/007-oboete-m1-alpha updated
- Follow-up: M1 plan.md Constitution Check now passes without exceptions A1-A10

Previous report (2.0.0 -> 3.0.0):
- Version change: 2.0.0 -> 3.0.0 (product renamed to oboete; foundation, agent set,
  and delivery model redefined)
- Modified principles:
  - Automatic Memory UX First -> I. Automatic, Agent-Neutral Memory
  - Durable Capture and Honest Degradation -> II. One File, No Daemon
  - Local-First and Explicit Egress -> III. Local-First, Fail-Closed Classification
  - Bounded and Predictable Resources -> IV. Honest Degradation and Bounded Resources
  - Product Slices Before Speculative Platforms -> V. Parity Target and Milestones
- Added sections:
  - VI. Portable and Minimal
  - Legacy disposition (Product and Technical Constraints)
  - Isolated dogfood environment (Development Workflow and Gates)
- Removed requirements:
  - daemon-owned sole writer, RPC, bounded spool as the mutation path
  - versioned capability manifest compiled by setup
  - pinned Codemem safety kernel as the implementation base
  - Technical Alpha boundary limited to Linux/WSL and Claude Code + Codex
- Follow-up work:
  - write the M1 (self-use Alpha) feature specification with /speckit-specify
  - record verified hook and API contracts in docs/research/ before the M1 plan
  - (done in the same change) move vendor/, specs/, evidence/, harness/ and root spec files under legacy/
  - route or close Product Reset issues (#136-#139, #148, #150-#153) after M1 is
    authoritative
-->

# oboete Constitution

## Core Principles

### I. Automatic, Agent-Neutral Memory

oboete MUST capture, summarize, store, retrieve, and inject memory for Claude Code, Codex,
Grok Build, and Pi without manual handoff bookkeeping. All four agents MUST share one logical
memory store across the developer's opted-in devices. Agent identity records provenance, not
a memory silo. Work progress, reusable project knowledge and personal shared knowledge MUST
have separate scope. Worktrees separate active work; a clear new purpose can form another work
item within a conversation, while related investigation stays together. Continuation is automatic
when unambiguous and otherwise requires one brief selection. Integration can adopt knowledge
without completing outstanding work. Memory failures MUST NOT block, exceed the turn budget,
or corrupt the coding agent's turn. Accepted sanitized activity and important supporting evidence
remain recoverable; generation of a summary alone is not proof of useful retained knowledge.

### II. Local SQLite and a Single Leased Worker

The product is the SQLite file `~/.oboete/memory.db`. A resident background worker MAY wait for
new work and due retries so recovery does not depend on another coding event. It MUST preserve
bounded processing passes, low idle resource use, explicit pause/stop and current configuration,
consent and cost checks. There is no background RPC server; the only port oboete binds is the loopback port of `oboete view`
while the developer runs that command in the foreground. Hooks run as short-lived processes that write directly to the
database in WAL mode. Background processing MUST have one owner per local store through its
heartbeat `worker_lease`, including idle residency and recovery. The bounded one-shot
`oboete observe` remains available for explicit processing and verification. The SQLite schema
plus the CLI contract is the language-neutral seam: any component MAY later be rewritten in
another language only behind that seam. The engine is TypeScript on Node.js >= 22.16 using
`node:sqlite` with FTS5, bundled into a single engine file: oboete's own code plus the small
pure-ESM packages the hook path needs; heavy runtime packages (AI SDK, Hono, Preact) stay in
`node_modules` and are loaded lazily off the hook path.

### III. Local-First, Fail-Closed Classification

Memory, indexes, configuration, and operational state live locally by default. Sensitivity is
decided at capture and written as `local_only`; a row MAY be promoted to `eligible` only after
the background worker has passed it through secret detection and entropy checks. Secrets MUST
be redacted before storage, including inside generated summaries. Egress is governed by one
table of rules: remote observer <- eligible; local observer <- same-repository eligible,
local_only, and private; opted-in sync <- everything except secret, encrypted end to end;
injection <- selected work, its project knowledge and explicitly shared personal knowledge.
Sensitivity and destination consent MUST still be checked after scope selection. Direct,
unambiguous personal preferences may be shared automatically; inferred cross-project knowledge
requires confirmation. Imported, quoted and tool-generated instructions cannot establish a
direct user preference. Availability fails open; classification fails closed; both directions MUST
have tests. Injected text MUST be marked and never re-observed. Before any remote provider or
sync target is enabled, setup MUST show destination host, credential source, cost class, and
data egress. oboete MUST NOT read credentials from other agents' sessions or subscription
stores without explicit user selection.

### IV. Honest Degradation and Bounded Resources

With zero credentials, capture and lexical search MUST work. When no LLM is reachable or the
selected allowance is exhausted, the engine tries the user's configured and consented alternative
models/providers within the selected cost policy. After eligible targets are exhausted,
rule-based output MAY provide temporary activity summaries,
but MUST leave accepted sources recoverable for processing by the user's chosen model.
Every injection pack and `oboete doctor` MUST expose the degraded reason. Capture, generation,
retrieval and sync health MUST be distinguished. Empty, partial or stale results MUST NOT be
reported as ordinary-quality completion. Budgets are explicit: capture hook process 300 ms (overflow
appends to a spool file and exits 0), session-start injection normally 300 ms with a 1.3 s hard
limit for an in-flight capture barrier, background engine RSS 150 MiB. Injection reads the current
work checkpoint immediately and labels that work's pending activity; generation never blocks it.
Unresolved selection supplies only choices and applicable knowledge. Requests are bounded, but information omitted by a
request limit remains pending. Processed full activity defaults to 30 days from processing;
unprocessed accepted activity MUST NOT be deleted because of age alone. Important evidence
is retained with its knowledge. Memories remain until deletion or supersession. The user chooses
local, free or paid generation and its spending policy; free/local modes MUST NOT switch to
paid generation automatically. Injection volume is adaptive: a relevance threshold plus a cap
proportional to the agent's context window, never a fixed token count. Resource evidence covers
1,000/10,000/100,000-event workloads and seven days of real use, including the engine and optional
local model separately and together. Deferred work and storage growth remain visible.

### V. Outcome-Based Completion

Done means useful long-term memory and correct work continuation across all four agents and
multiple devices, with no mandatory service subscription. Initial completion includes Linux,
WSL and macOS, safe migration of supported claude-mem/CMEM Pro data, and opt-in encrypted sync.
Native Windows and general chat-application capture follow later. Existing memories, provenance
and deletion choices MUST survive migration; historical progress MUST NOT silently become
current work. A local-only or single-agent increment is not the finished product.

Deliver independently verified increments: reliable source processing first; work continuity,
scope and recall next; migration, sync and platform completion thereafter. Semantic retrieval
is adopted when the measured recall target needs it, not deferred by milestone numbering.
The reference-provider Japanese and English recall target is at least 90%; capture, request
coverage, application, retrieval, delivery and answer quality are measured separately. Wiring
success, fallback fixtures and green CI are not substitutes for real-model evidence.

### VI. Portable and Minimal

No Linux-only assumption may be written: no Unix sockets, `flock`, or bash-only hooks; paths
live under one data directory (`~/.oboete/`, relocatable through `OBOETE_HOME`); a per-platform
XDG/AppData split is deferred to a later milestone. Dependencies are limited to those adopted by the design
record (`node:sqlite`, the Vercel AI SDK with the OpenAI-compatible and Workers AI providers,
`@secretlint/core` with `@secretlint/secretlint-rule-preset-recommend`, `age-encryption`, `aws4fetch`, `zod`, Hono, Preact, Vite); any addition
requires a written reason in the plan. oboete does NOT build a manifest
compiler, Verified Continuity, a Chroma or Python sidecar, Vectorize, a hosted viewer,
team or RBAC features, a Rust rewrite ahead of measurement, or subscription OAuth reuse.
Deletion is preferred over addition; every abstraction needs a second concrete user.

## Product and Technical Constraints

- Data model: `repos` (id derived by the hook from the normalized git remote or toplevel
  realpath hash, never self-reported), `sessions`, append-only `raw_events` as the acceptance
  point, `memories` (`content_hash` UNIQUE, `deleted_at` tombstone, `superseded_by`,
  bitemporal validity and work/project/personal scope), durable processing outcomes and source
  evidence, work contexts and work items, `memories_fts` (FTS5 trigram plus a CJK bigram shadow column;
  queries shorter than three characters use LIKE), optional `memory_vec`, an `injections`
  ledger, and `sync_conflicts`. Migrations are numbered SQL files.
- Agent integration: Claude Code and Grok Build use the same JSON hook command; the command
  identifies its caller from the environment and never assumes Claude Code. Codex uses
  handlers in `~/.codex/hooks.json` with the trust-hash row in `~/.codex/config.toml`, both
  inside oboete-managed blocks; injection happens only at session start and prompt submit.
  Pi loads an in-process extension that only enqueues to a detached `oboete capture` child under
  a cooperative deadline and MUST wrap every call in try/catch; the child imports the capture
  functions. Search is exposed through `oboete mcp` over stdio.
- Observer: one OpenAI-compatible client with user-selected presets (Cloudflare Workers AI, NVIDIA
  NIM, OpenRouter, Gemini, Ollama; the Anthropic API is not a preset, owner decision A19 of
  2026-09-04). The prompt classifies against nearby existing
  memories as ADD, UPDATE, DELETE, or NOOP and accounts for the source information considered.
  Free-model availability is resolved from the provider catalog at run time, not hardcoded.
  Free-tier/API failures may advance to another configured model or provider; each target must
  satisfy current data-egress consent and free/paid admission before a request is sent. Free
  eligibility alone is not proof that a billing-enabled account cannot charge.
  Rejected, omitted or failed processing remains inspectable and retryable where appropriate.
- Retrieval and injection: session start injects the selected work's current checkpoint and
  applicable pinned knowledge; prompt submit runs FTS5 BM25 (plus measured semantic retrieval
  when required), fused by RRF then MMR with character
  n-gram similarity, with citation staleness checked against HEAD. The same session is never
  re-injected with the same memory. `oboete why` explains any pack.
- Sync: `oboete sync push|pull` transfers versioned records encrypted with age to an opted-in
  destination. Stable origin identities, idempotent delivery and tombstone propagation are
  required. Incompatible concurrent progress updates land in `sync_conflicts`; machine clock
  order alone cannot resolve them. R2/S3 is a supported destination, not a required subscription.
- Viewer: `oboete view` serves a Preact + Vite static SPA through Hono with SSE.
- Distribution and configuration: npm package `oboete`, Apache-2.0, no telemetry.
  `~/.oboete/` holds `config.toml`, `memory.db`, `spool/`, and `logs/`. Summary language
  follows content.
- Legacy disposition: the previous free-mem implementation (`vendor/codemem`), its
  specifications, evidence, and harness are moved under `legacy/` as read-only evidence.
  Only the boundary SQL, test fixtures, and mutation gate MAY be ported, and each ported
  piece is deleted from `legacy/` once it lands.

## Development Workflow and Gates

- Every milestone follows the Spec Kit sequence `specify -> clarify -> plan -> tasks ->
  implement -> verify-tasks`. Bugs go through `bug-assess -> bug-fix -> bug-test`.
- Implementation happens in an isolated branch and worktree; the shared checkout is never
  rewritten. Correctness and security review precede the over-engineering review, and
  findings are evidence, not authority. Security-related changes are not delegated to
  external coding CLIs.
- Third-party contracts (hook payloads, provider APIs, extension events) MUST be verified
  against primary sources or a live probe and recorded under `docs/research/` before a plan
  depends on them.
- New behavior requires the smallest test that fails without it. Release evidence includes
  build, typecheck, lint, tests, a packed-install check, real hook-to-injection E2E for each
  agent, provider-failure fallback, and the resource fixture numbers.
- Dogfood runs on this WSL host under a separate Linux user with an isolated home and its own
  agent logins, driving `claude -p`, `codex exec`, `grok -p`, and `pi -p` with real
  hooks. oboete MUST NOT be installed in the maintainer's own agent environment until the
  isolated E2E has been green for at least one week and the maintainer approves it again at
  that time. The OCI A1 host is excluded.
- Pull requests follow the repository CI, DCO, and merge gates. Merge, publication, and
  cloud enablement remain separate explicit decisions.

## Governance

This constitution supersedes version 4.0.0 and conflicting 007/008 product assumptions,
the Product Reset specifications
(`legacy/specs/005-product-reset`, `legacy/specs/006-slice1-runtime`), and every earlier continuity
specification as product authority. Those artifacts are historical evidence under `legacy/`
until a new approved specification explicitly reactivates part of them. Amendments require an
updated Sync Impact Report, user approval for changes to product purpose, privacy boundaries,
or milestone order, and a disposition plan for affected specifications and issues.

Versioning follows semantic versioning: MAJOR for removed or redefined principles, MINOR for a
new or materially expanded principle, and PATCH for non-semantic clarification. Every feature
plan and pull request MUST state whether it complies with Principles I-VI and identify any
approved exception. Unexplained violations block implementation or merge.

**Version**: 5.0.0 | **Ratified**: 2026-08-12 | **Last Amended**: 2026-09-10
