<!--
Sync Impact Report
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
Grok Build, and Pi without manual handoff bookkeeping. All four agents MUST share one memory
store. Destination boundaries MUST be defined by sensitivity and repository only; an agent
identifier MUST NOT silo memory. The `agent` column on a session records provenance and MUST
NOT affect eligibility. Memory failures of any kind MUST NOT block, slow beyond budget, or
corrupt the coding agent's turn. Observation granularity and memory types follow claude-mem:
prompts and tool inputs and outputs are given to the observer; only summaries are stored.

### II. One File, No Daemon

The product is the SQLite file `~/.oboete/memory.db`. There MUST be no resident daemon, RPC
server, or listening port. Hooks run as short-lived processes that write directly to the
database in WAL mode. Background work (`oboete observe`) MUST be a detached process that is
single per machine through a heartbeat `worker_lease` row in the database. The SQLite schema
plus the CLI contract is the language-neutral seam: any component MAY later be rewritten in
another language only behind that seam. The engine is TypeScript on Node.js >= 22.16 using
`node:sqlite` with FTS5, bundled into a single file.

### III. Local-First, Fail-Closed Classification

Memory, indexes, configuration, and operational state live locally by default. Sensitivity is
decided at capture and written as `local_only`; a row MAY be promoted to `eligible` only after
the background worker has passed it through secret detection and entropy checks. Secrets MUST
be redacted before storage, including inside generated summaries. Egress is governed by one
table of rules: remote observer <- eligible; local observer <- same-repository eligible,
local_only, and private; sync <- everything except secret, encrypted end to end; injection <-
same repository. Availability fails open; classification fails closed; both directions MUST
have tests. Injected text MUST be marked and never re-observed. Before any remote provider or
sync target is enabled, setup MUST show destination host, credential source, cost class, and
data egress. oboete MUST NOT read credentials from other agents' sessions or subscription
stores without explicit user selection.

### IV. Honest Degradation and Bounded Resources

With zero credentials, capture and lexical search MUST work. When no LLM is reachable or the
daily allowance is exhausted, a rule-based observer MUST produce summaries in the same schema,
and every injection pack and `oboete doctor` MUST expose the degraded reason. Empty or stale
results MUST NOT be reported as healthy. Budgets are explicit: hook process 300 ms (overflow
appends to a spool file and exits 0), background worker RSS 150 MB, observer input 12,000
characters, and a self-counted daily Workers AI Neuron allowance that resets at UTC midnight.
Raw events are deleted after 7 days; memories are permanent and removed only through
tombstones and supersession. Injection volume is adaptive: a relevance threshold plus a cap
proportional to the agent's context window, never a fixed token count. Resource evidence from
a committed 1,000-event fixture (hook time, RSS, database growth) MUST exist before a
milestone is called done.

### V. Parity Target and Milestones

Done means functional parity with claude-mem plus cmem pro without a subscription, with
higher configurability and fewer defects. Milestones are delivered one at a time: M1 self-use
Alpha (four agents, web viewer, doctor, isolated dogfood for one to two weeks); M2 R2 sync and
hybrid search (`sqlite-vec`, Workers AI `bge-m3` default embedding, optional local model); M3
npm publication; M4 macOS and private MCP link; M5 Windows. Vector search, cloud sync, and
non-Linux platforms MUST NOT delay M1, and M1 MUST NOT hardcode anything that prevents them.
Only one milestone may be in progress.

### VI. Portable and Minimal

No Linux-only assumption may be written: no Unix sockets, `flock`, or bash-only hooks; paths
follow XDG and AppData conventions. Dependencies are limited to those adopted by the design
record (`node:sqlite`, the Vercel AI SDK with the OpenAI-compatible and Workers AI providers,
`@secretlint/node`, `age-encryption`, `aws4fetch`, `zod`, Hono, Preact, Vite); any addition
requires a written reason in the plan. oboete does NOT build a resident daemon, a manifest
compiler, Verified Continuity, a Chroma or Python sidecar, Vectorize, a hosted viewer,
team or RBAC features, a Rust rewrite ahead of measurement, or subscription OAuth reuse.
Deletion is preferred over addition; every abstraction needs a second concrete user.

## Product and Technical Constraints

- Data model: `repos` (id derived by the hook from the normalized git remote or toplevel
  realpath hash, never self-reported), `sessions`, append-only `raw_events` as the acceptance
  point, `memories` (`content_hash` UNIQUE, `deleted_at` tombstone, `superseded_by`,
  bitemporal validity), `memories_fts` (FTS5 trigram plus a CJK bigram shadow column;
  queries shorter than three characters use LIKE), optional `memory_vec`, an `injections`
  ledger, and `sync_conflicts`. Migrations are numbered SQL files.
- Agent integration: Claude Code and Grok Build use the same JSON hook command; the command
  identifies its caller from the environment and never assumes Claude Code. Codex uses
  `~/.codex/config.toml` hooks; injection happens only at session start and prompt submit.
  Pi loads an in-process extension that imports the capture functions and MUST wrap every call
  in try/catch with a timeout. Search is exposed through `oboete mcp` over stdio.
- Observer: one OpenAI-compatible client with presets (Cloudflare Workers AI default, NVIDIA
  NIM, OpenRouter, Gemini, Ollama, Anthropic). The prompt classifies against nearby existing
  memories as ADD, UPDATE, DELETE, or NOOP. Free-model availability is resolved from the
  provider catalog at run time, not hardcoded.
- Retrieval and injection: session start injects the latest summary and pinned memories;
  prompt submit runs FTS5 BM25 (plus vectors from M2), fused by RRF then MMR with character
  n-gram similarity, with citation staleness checked against HEAD. The same session is never
  re-injected with the same memory. `oboete why` explains any pack.
- Sync (M2): `oboete sync push|pull` ships `VACUUM INTO` diffs encrypted with age to R2 over
  the S3 API with SigV4; merge is `content_hash` union with tombstone propagation; divergence
  lands in `sync_conflicts`; no CRDT.
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
  agent logins, driving `claude -p`, `codex exec`, `grok --print`, and `pi -p` with real
  hooks. oboete MUST NOT be installed in the maintainer's own agent environment until the
  isolated E2E has been green for at least one week and the maintainer approves it again at
  that time. The OCI A1 host is excluded.
- Pull requests follow the repository CI, DCO, and merge gates. Merge, publication, and
  cloud enablement remain separate explicit decisions.

## Governance

This constitution supersedes version 2.0.0, the Product Reset specifications
(`legacy/specs/005-product-reset`, `legacy/specs/006-slice1-runtime`), and every earlier continuity
specification as product authority. Those artifacts are historical evidence under `legacy/`
until a new approved specification explicitly reactivates part of them. Amendments require an
updated Sync Impact Report, user approval for changes to product purpose, privacy boundaries,
or milestone order, and a disposition plan for affected specifications and issues.

Versioning follows semantic versioning: MAJOR for removed or redefined principles, MINOR for a
new or materially expanded principle, and PATCH for non-semantic clarification. Every feature
plan and pull request MUST state whether it complies with Principles I-VI and identify any
approved exception. Unexplained violations block implementation or merge.

**Version**: 3.0.0 | **Ratified**: 2026-08-12 | **Last Amended**: 2026-09-02
