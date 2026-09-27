# Milestone 3 (Curate) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Depth, unlike milestone 2's plan:** every decision and every task's files and interfaces are here. Step bodies are written for Tasks 1-5, which start first and settle the provider layer and the window shape the rest builds on. Tasks 6-13 get their step bodies in a follow-up PR once Tasks 1-5 have merged, so that they are written against code that exists. A design doc that specifies mechanism does not converge under review (PR #29: 21 rounds), and this milestone is larger than milestone 2.

**Goal:** Design B's curation layer: the worker cuts windows from raw.db, a provider chain that works on the owner's real traffic turns each window into claims with speaker, status, evidence and supersedes, code gates decide what a claim may say, and `rebuild` gives back the same claims with no AI call. The provider layer comes first: the owner's direction of 2026-09-27 is that the product does not work unless the summarizing providers work ("要約プロバイダをちゃんと使えるようにしないと成立しない").

**Architecture:** Curation is a worker phase next to the milestone 2 consumers, not one of them: a window is read, sent to a provider with no database transaction open, and its result written as ops to raw.db in one transaction together with the window's range, which is the curation checkpoint. A deterministic consumer turns ops into claims in knowledge.db. Provider state (cooldowns, the call and egress ledger, token and money budgets) lives in a third file, `providers.db`, which neither `rebuild` nor a raw.db restore touches.

**Tech Stack:** Rust (edition 2024), rusqlite (bundled SQLite, FTS5 trigram), ureq for OpenAI-compatible APIs, the vendors' own `claude` and `codex` binaries for the subscriptions, no async runtime.

**Spec:** `docs/spec.md` sections 1.4, 1.7, 3, 6.4, 6.5, 8.2 (M2, M3, Isolation, Window, Judge, Cost) and 8.4 row 3. Research: `docs/research/curator-providers-2026-09-25.md`, `docs/research/curator-providers-2026-09-27.md`, `docs/spike/curator-isolation.md`, `docs/research/redesign-2026-09-24/improvements-synthesis.md` (MUST-M1 to M7, M18, M21). What milestone 2 left: `docs/milestone-2.md`, "What milestone 3 inherits".

## Global Constraints

- raw.db's only ordering and partition key is (device, seq). Session, repo and branch are labels, never a curation or checkpoint boundary (spec 1.6).
- The checkpoint moves only in the same transaction as the window's knowledge (spec 3.1).
- `rebuild` makes zero AI calls and yields identical claims (spec 1.7).
- Every seq is curated, elided with a marker, or skipped with a reason (M2, 100%).
- Keys are read from files in-process, sent only as a request header, and never go into a subprocess environment, a command line or a log (spec 6.4; CLAUDE.md "Keys").
- Curator subprocesses get S8's environment: `env_clear` plus the allow-list of spec 6.4, never an `ANTHROPIC_*` or `CLAUDE_CODE_*` name (spec 6.4). This holds before claude curates on Design B (spec 8.4 row 3).
- A curator is a model call, not an agent. A CLI with no proven no-tool mode is skipped for every role that reads recorded text, with a doctor line (spec 6.5).
- Text leaving the machine passes the egress gate: the second redaction pass with the current ruleset, `<private>` removal and exclusions, on whole events even when a window splits one (spec 3.1, 6.4).
- Subscriptions are never read or injected as OAuth tokens and never reached through a proxy: oboete spawns the unmodified vendor binary, which holds its own login (docs/research/curator-providers-2026-09-27.md §5).
- A provider's error body is never stored: only status, a vetted error code and retry values (issue #91, owner decision of 2026-09-27).
- The owner's chain order (2026-09-27): groq → groq-20b → groq-qwen → openrouter (free) → mistral (free) → nim (free) → opencode-go → codex (gpt-6-luna) → claude (haiku). Gemini only when the owner configures a key. grok never curates (owner decision 23).
- Caps: claim body 1,000 characters, digest 2,000, provider response 1 MB, op 64 KB (spec 6.5, A42).
- Paid APIs at most USD 5 a month; embedding (Workers AI) spend has its own cap, outside that one (owner decision of 2026-09-27, issue #90).
- The owner's machines keep running v1 until the cut-over after milestone 5. Design B curation runs only in the `oboete-dogfood` user and in temporary homes (CLAUDE.md). Curator CLI tests run only in the dogfood user.
- Claude Code writes the core and every security-scope part: the provider layer's key handling, curator isolation, the environment allow-list, the egress gate, the code gates (spec 8.4 "Who builds").
- CI: `cargo fmt --check`, `cargo clippy`, `cargo test` on Linux, macOS and Windows (#122).

## Review Focus

1. **A provider that never answers.** Mistral's key reports `x-ratelimit-limit-req-minute: 0` and answers every request with 429 and no reset (2026-09-27). Expected: after the first 429 with no reset, the cooldown doubles on each further one up to 1 hour, so the window text is not uploaded to it once every 45 seconds. Test in Task 1.
2. **A window larger than one provider's ceiling.** A Japanese-heavy window of 12,000 characters is about 8,400 tokens; Groq free refuses anything over 8,000 with 413. Expected: Groq is skipped before the call (no upload, no budget spent), the chain goes on, and after a real 413 the other entries with the same ceiling are skipped for that window. Test in Task 4.
3. **The owner keeps working.** Hooks fire every few seconds for hours. Expected: no subscription provider (codex, claude, opencode-go) is called until 10 minutes after the last hook, free providers still curate, and the waiting window says "waiting for the owner to finish" in doctor. Test in Task 5.
4. **A crash between the provider's answer and the write.** The worker dies after the answer arrives and before the ops commit. Expected: the window is curated again from the same checkpoint, no claim is written twice, and no seq is marked done unseen. Test in Task 5 (and M2's 20 crash points in Task 13).
5. **An update that brings tools back.** claude or codex updates and its no-tool mode no longer holds. Expected: claude's `system/init` shows a tool and the result is discarded before it is parsed; codex's stored gate result is for another version, so it is re-run before the next call and codex is skipped with a doctor line if it fails. Test in Task 3.

---

## Decisions

Each decision is Claude's unless marked otherwise, and the owner can overrule it. They settle what the spec leaves to milestone 3 or leaves open.

**D1. The op log is a table in raw.db (settles spec Appendix C item 17).** `ops(device, op_seq, type, ts, body)` with `PRIMARY KEY (device, op_seq)`: this device's window, claim, correction and digest ops, and, from milestone 6, inbound ones under their origin device. It sits in raw.db because it is the kept, non-derivable record the spec puts beside raw (spec 1.7, 7.3 step 3): it gets raw.db's `synchronous=FULL`, its backups and its restore. Backups export ops with a cursor of their own (the highest `op_seq` exported), not with the records' cursor: an op is often appended when no new record is (a correction, a window curated long after its records were backed up), and the records' cursor would never reach it (Task 5). knowledge.db stays fully derived.

**D2. The curation checkpoint is the op log itself (amends milestone 2's D10 for curation only).** A window op records the range it covered, from (from_seq, from_offset) to (to_seq, to_offset), and its outcome: curated (with the claim ops written in the same transaction), elided, or skipped with a reason. The offsets are byte offsets into an event's text: an event larger than the window is curated in parts (spec 3.1, issue #54), and a window that ends inside it has a `to_offset` short of its end, so the next window starts at that offset in the same event instead of after it. A window that ends at an event's end has `to_offset` = null. The curation checkpoint of a device is the greatest (to_seq, to_offset) among its window ops that are not recurations. A window op that `oboete recurate` writes (Task 11) carries `recurate: true` and never moves the checkpoint, backwards or forwards: re-curating an old or skipped span must not make the curation phase send every later window again. So the window's knowledge and the checkpoint move in one raw.db transaction (spec 3.1), a crash before the commit leaves both unmoved, and a raw.db restore brings both back together. D10 still holds for every knowledge.db consumer, including the claims consumer of D4.

**D3. Curation is a worker phase outside `consumer::pass`.** `pass` holds a knowledge.db transaction across each `step` (src/consumer.rs). A provider call takes seconds to minutes, so a curation step inside it would pin a WAL snapshot and hold every other consumer behind the network. The curation phase runs after `pass` has drained: read the next window (no transaction), call the chain (no transaction), then append its ops in one raw.db transaction. The milestone 2 consumers keep their shape.

**D4. Claims are derived from ops by a consumer.** `consumer::Claims` reads ops after its checkpoint (an `op_seq`, per device) and writes claims, edges and derivations into knowledge.db. It is deterministic, so `rebuild` (drop knowledge.db, run the consumers) yields identical claims with no AI call (spec 1.7). Its rewind rule is D10's with the op log's highest `op_seq` in place of raw's highest seq.

**D5. Provider state lives in `providers.db`.** A third SQLite file beside raw.db and knowledge.db (WAL, `synchronous=NORMAL`) holds `provider_state` (cooldown until, failure count, backoff step, reset source), `provider_calls` (the egress ledger: every attempt, failed ones too, with role, span, bytes sent, a vetted detail (status, error code, retry values), latency, token usage, and the uncalibrated token estimate of what was sent, which Task 4's calibration divides by) and the gate results of Task 3. It is device-local and never synced. Neither `rebuild` nor a raw.db restore touches it, so a cooldown or a month's spend survives both. Losing the file resets the month's spend to zero: `ponytail:` accepted, since the default chain has no paid entry and a lost file needs a lost disk; doctor says when the file is younger than the current month.

**D6. The provider layer on main is v1's, plus spec 6.5, plus what v1 never got.** Ported from `v1` (already reviewed there): the short curator system prompt (#96), 429 until the provider's reset and Groq low reasoning (#97), cooldowns across runs and the breaker after 3 bad answers (#99), groq-qwen (#100), token usage per answer (#101), Gemini as an opt-in entry with its error shapes (#116), the chain order and error codes without bodies (#91, #93). Added: a 429 with no reset doubles its cooldown from 45 s up to 1 h (Review Focus 1); an error body whose fields sit at the root (Mistral's `{"type": "rate_limited", "code": "1300"}`) is read like one under `error`; spec 6.5's claude flags (Task 2); S8's environment (Task 2); token and money budgets, the pre-flight size check and Groq's rate-limit headers (Task 4). `provider.rs` stops taking v1's `oboete.db` connection: it takes `providers.db`.

**D7. Tokens, not characters.** A window's size and a provider's ceiling are in estimated tokens. The estimate counts characters by script: CJK and kana at 0.7 tokens, others at 0.25 (docs/research/curator-providers-2026-09-27.md §3), then multiplies by a per-provider factor learned from recorded usage (`prompt_tokens` over the estimate, a running median of the last 50 answers, starting at 1.0). An entry may carry `max_request_tokens` (Groq free: 8,000); the chain skips it before the call when the calibrated estimate is over, and records `too_big` with no upload.

**D8. The window size is measured, with an interim default.** `window_tokens` defaults to what the size sweep (Task 13's note `docs/spike/curator-sizes.md`, first run 2026-09-27) shows every free entry answering validly, and Measurement Window replaces it at the end of this milestone (spec 3.1, 8.2 Window). The interim is chosen so that the first free entry, Groq, can take a window: below 8,000 tokens including the prompt's fixed part and an earlier-context block.

**D9. The idle gate reads raw.db.** "The owner is working" means a hook record (`type = 'event'`, `source = 'hook'`) newer than the gate's wait (10 minutes by default, a setting of at most 30, D10's stay-up; spec 3.1). The worker reads the newest such record by walking seqs downward from the top, so tombstones the rescan appends and imports never hold the gate shut. `oboete replay` sends fixture events through `hook::record`, which stamps them `source = 'hook'` today; Task 5 makes replay stamp `source = 'replay'`, so a replay is not taken for the owner at work. The check runs right before each subscription call, not once per run (docs/research/curator-providers-2026-09-27.md §7 item 5); a curator CLI is asked again after its isolation probe, which can take seconds. A window that reaches the device's last record (not cut by size) waits for the same time before any provider, free ones included: otherwise every hook's drain would send the few records it added. So the last window of a stretch of work is curated about 10 minutes after the owner's last hook, and it writes no pending row (it is not a failure, and doctor shows nothing for it).

**D10. The worker stays up while curation only waits on time.** Today the worker exits when idle. With subscription windows waiting for the gate, an exit would leave them for the next hook, which is by definition the owner working again. So when every pending window waits only for a time (the idle gate, a cooldown, a `next_attempt_at`) within the next 30 minutes, the worker sleeps until that time instead of exiting; beyond 30 minutes it exits and the next hook starts it again. A window waiting on a budget reset or on the owner (credits, a failed isolation gate) never keeps it up. With curation on, the last window's wait (D9) keeps the worker up while the owner works, since each hook moves that time. A window that waits on the owner is tried again an hour later, or at the worker's next run after `oboete resume`.

**D11. A window that every provider fails is skipped after three attempts.** Each attempt tries each provider of the chain at most once (spec 3.1). After the third attempt with no valid answer and no reason that will pass by itself (a cooldown or budget reason does not count), the window op is written as skipped with the reason, the checkpoint moves on, and `oboete recurate --skipped` takes it again later (spec 1.7). A cooldown that a failure in the attempt itself set (a 429, a timeout, an outage, the breaker) is such a reason too (review on #137): an offline laptop or a provider's outage must not skip windows. Attempts are 10 minutes apart. So a window that answers badly (a 4xx, an invalid answer) is skipped after three attempts, and the breaker's cooldown only delays that; a window that makes every provider time out or fail with 5xx each time is never skipped, and doctor shows it waiting. That residue is accepted: an outage is far more common than such a window.

**D12. Windows cover the device's stream, not a session.** A window is a seq range of one device, cut at a turn boundary where the adapter reports one, else at a tool-call boundary, else at the size cap (spec 3.1). Events of concurrent sessions in the range are rendered grouped by session inside the prompt. The session's carried context (its goal, open items and the previous window's unresolved proposals) comes from knowledge.db, so a proposal and its acceptance in different windows are gated as if in one (spec 3.3).

**D13. Claim status (settles spec Appendix C item 6).** Stored status is one of decided, proposed, retracted, done, unverified (a claim of an unknown kind, spec 3.2) or unknown (imported memories, spec 4.5). The gates of Task 8 move a claim only among the first four; unverified and unknown change only by a new derivation or an owner correction.

**D14. `observe` leaves main with Task 5.** `src/observe.rs` reads and writes v1's `oboete.db`, which no Design B hook has written since #118. When the curation phase lands, `oboete observe` and its spawn are removed from main, together with `provider.rs`'s `oboete.db` use (D6). MCP and the viewer keep reading `oboete.db` until milestone 4 moves them (docs/milestone-2.md).

**D15. The claude curator's flags are spec 6.5's, after one probe.** Appendix C items 1 and 5 are settled by one dogfood call with synthetic text before Task 2 lands: whether `rate_limit_event` arrives on every request, whether `system/init` arrives before stdin is read (then the prompt is held until the tool check passes), whether `--json-schema` puts a tool in `init.tools` (then the schema goes into the prompt and the answer is read from the text, as measured in docs/research/curator-providers-2026-09-27.md §4.1), and whether `--max-turns 1` and `--effort low` are accepted. The results go into `docs/spike/curator-isolation.md`.

---

## Tasks

| # | Task | Executor | Depends on | Produces |
|---|---|---|---|---|
| 1 | `providers.db`; v1's provider layer on main; 429 backoff; root-level error bodies | Claude | — | `providers_db::open`, `Chain::new(providers, &providers_db)`, `Chain::run(role, span, prompt, schema)`, `provider_calls` rows |
| 2 | S8 environment; spec 6.5's claude flags and `system/init` check; `rate_limit_event` and `credits_required` | Claude | 1 | `provider::curator_env`, `claude_stream` parsing, subscription cooldowns |
| 3 | Isolation gate: reported tools empty, or a direct sandbox probe; canaries as supporting evidence; stored per CLI version; doctor lines; security review | Claude | 2 | `isolation::{gate, probe}`, the per-CLI table in `docs/spike/curator-isolation.md` |
| 4 | Budgets: tokens and calls per day, `monthly_usd`, `max_request_tokens` with the calibrated estimate, Groq's rate-limit headers | Claude | 1 | `budget::{estimate, admit}`, `Usage` with bytes sent |
| 5 | Windows and the curation phase: the op log, window cut, egress gate, pending reasons, idle gate, worker stay-up, skip after three attempts; `observe` removed | Claude | 1, 4 | `raw::Op`, `Raw::{append_ops, ops_after, curation_checkpoint}`, `curate::{next_window, run_phase}` |
| 6 | Claims schema and the claims consumer; derivations (MUST-M18); chain tips and the tie order (MUST-M7) | Claude | 5 | `consumer::Claims`, knowledge.db `claims`, `edges`, `derivations` |
| 7 | The curator prompt and answer: claims with speaker, status, evidence and supersedes; candidates from the repo-wide index (MUST-M3); window-local ids (MUST-M2); answer failure kinds | Claude | 6 | `curate::{prompt, parse}` |
| 8 | Code gates: evidence per status change (MUST-M1), taint (MUST-M4), acceptance and negation, global scope, verbatim evidence, caps | Claude | 7 | `gates::check(window, candidates, answer) -> Vec<Claim>` |
| 9 | Digests (MUST-M6) and the manifest's "Current decisions" | Claude | 6, 8 | `consumer::Digest`, manifest part |
| 10 | Owner corrections as ops; they survive rebuild and re-derivation (MUST-M21) | Claude | 6 | `oboete correct`, correction ops |
| 11 | `oboete rebuild` and `oboete recurate [--skipped | <span>]` with a cost estimate first | Claude | 5, 6, 10 | commands; rebuild test: identical claims, zero calls, spend unchanged |
| 12 | Judge roles (selection and veto), kept only if Measurement Judge shows a gain | Claude | 8 | `judge::{select, veto}` behind a setting |
| 13 | Lines: size sweep, M2 (coverage, 20 crash points), Isolation, Window (dev), Cost, M3 (dev tuning, then the deciding run on test labels); the milestone note | Claude | 1-12 | `docs/milestone-3.md`, `docs/spike/curator-sizes.md` |

---

## Task 1: `providers.db` and the provider layer on main

**Files:**
- Create: `src/providers_db.rs` (schema, open, state and ledger rows)
- Modify: `src/provider.rs` (port v1's changes, #96 to #116; take `providers.db`; 429 backoff; root-level error bodies)
- Modify: `src/config.rs` (v1's `default_providers()`, the Gemini entry and `GeminiPlace`)
- Test: `src/provider.rs`, `src/providers_db.rs`

**Interfaces:**
- Produces:
  - `providers_db::open(home) -> Result<Connection>`: WAL, `synchronous=NORMAL`, tables `provider_state(provider TEXT PRIMARY KEY, down_until INTEGER, fails INTEGER, backoff INTEGER)` and `provider_calls(id INTEGER PRIMARY KEY, ts INTEGER, provider TEXT, role TEXT, span TEXT, outcome TEXT, ms INTEGER, detail TEXT, bytes_out INTEGER, est_tokens INTEGER, prompt_tokens INTEGER, completion_tokens INTEGER, cached_tokens INTEGER, reasoning_tokens INTEGER)`. `outcome` is one of ok, invalid, error, wait, budget, too_big. `span` is what the call was for (a window's range, a session); `window` is an SQL keyword. `detail` holds only a vetted status, error code or retry value (#91). `est_tokens` stays null until Task 4.
  - `Chain::new(providers: &[Provider], db: &Connection) -> Chain` loads the persisted state; `Chain::run(&mut self, role: &str, span: &str, prompt: &str, schema: &Value) -> Result<ChainResult>`.
  - `provider::error_code(body: &str) -> Option<String>` reads `error.code`, an array root (`[{"error": {...}}]`, Gemini) and root-level fields (`code`, `type`, Mistral).
- Consumes: v1's `provider.rs` and `config.rs` at 4bfe458.

- [ ] **Step 1: Failing tests.** `a_429_with_no_reset_doubles_its_cooldown_up_to_an_hour` (45 s, 90 s, ... 3,600 s; an answer resets the step); `an_error_body_at_the_root_gives_its_code` (Mistral's shape, no text kept); `a_cooldown_survives_a_new_chain` (state from `providers.db`); `no_error_body_reaches_provider_calls` (v1's #91 test, on the new table); v1's tests for #96, #97, #99, #101 and #116, moved.
- [ ] **Step 2: Port.** Apply v1's `provider.rs` and `config.rs` changes from 3ff374c..4bfe458 onto main, replacing each `db::` call with `providers_db::`. Keep main's `CODEX_PROFILE` and the isolation flags that main already has (#73).
- [ ] **Step 3: The backoff.** In `cooldown_for`, a 429 with no `retry_after_s` returns `COOLDOWN_429 << backoff` capped at 1 h, and the chain stores `backoff + 1`; any other outcome stores 0.
- [ ] **Step 4: Run** `cargo test provider providers_db` and the whole suite; the Windows and macOS jobs of CI too.
- [ ] **Step 5: Commit** `providers: v1's provider layer on main, with its state in providers.db (milestone 3, Task 1)`.

---

## Task 2: The curator's environment and claude's flags

**Files:**
- Modify: `src/provider.rs` (`headless_command`, `cli_headless`)
- Modify: `docs/spike/curator-isolation.md` (D15's probe)
- Test: `src/provider.rs`

**Interfaces:**
- Produces: `provider::curator_env(parent: impl Iterator<Item = (OsString, OsString)>) -> Vec<(OsString, OsString)>`: spec 6.4's allow-list (PATH, HOME, USERPROFILE, APPDATA, LOCALAPPDATA, TMP, TEMP, LANG, LC_*, XDG_*, the proxy variables, `CODEX_HOME`, `CLAUDE_CONFIG_DIR`), compared case-insensitively on Windows, plus the self-capture marker. Never an `ANTHROPIC_*` or `CLAUDE_CODE_*` name.
- The delta to v1's claude command, for the review: kept `-p`, `--setting-sources ""`, `--tools ""`, `--strict-mcp-config`, `--no-session-persistence`, hooks off, `--model haiku`, prompt on stdin, private cwd. Changed: `--system-prompt <text>` becomes `--system-prompt-file`; `--output-format json` becomes `stream-json --verbose`. Added: `--permission-mode dontAsk`, `--permission-prompts none` (doctor probes the CLI accepts it, 2.1.259 or later), `--disallowedTools Agent Task Monitor mcp__*`, `--disable-slash-commands`. `--json-schema`, `--max-turns 1` and `--effort low` follow D15's probe.

- [ ] **Step 1: The probe (D15).** One call in the dogfood user with a synthetic window and the new flags; record the four answers and the exit at a usage limit if one is reached; write them into `docs/spike/curator-isolation.md`.
- [ ] **Step 2: Failing tests.** `the_curator_environment_is_the_allow_list` (a parent env with `ANTHROPIC_BASE_URL`, `CLAUDE_CODE_EFFORT_LEVEL`, `AUTHORIZATION`, `google_application_credentials` and `SSH_AUTH_SOCK` gives none of them; `Path` on Windows is kept); `a_tool_in_system_init_discards_the_answer`; `a_rate_limit_warning_cools_claude_down_until_its_reset` (`rate_limit_event` with `allowed_warning` and `resetsAt`); `credits_required_stops_claude_until_the_owner_acts`.
- [ ] **Step 3: Implement,** replacing v1's substring denylist with `env_clear` plus `curator_env`, for every CLI provider.
- [ ] **Step 4: Run** the tests; one real call per CLI in the dogfood user; the suite.
- [ ] **Step 5: Commit** `providers: S8's environment for curator CLIs; claude's spec 6.5 flags and its init check (milestone 3, Task 2)`.

---

## Task 3: The isolation gate

**Files:**
- Create: `src/isolation.rs`
- Modify: `src/provider.rs` (skip a CLI whose gate has not passed for its version), `src/setup.rs` (doctor lines)
- Test: `src/isolation.rs`; `tests/isolation.rs` (real CLIs, run only when `OBOETE_ISOLATION_LIVE=1`, in the dogfood user)

**Interfaces:**
- Produces: `isolation::probe(cli: &str) -> Result<GateResult>`. The gate tests capability, not obedience (spec 6.5: agy ignored a canary while holding 57 tools), so a model that merely declines the instructions never passes it:
  - **A CLI that reports its tools** (claude's `system/init` under stream-json, agy's init event) passes only when the reported tool list, MCP servers and plugins are empty and the permission mode is the one asked for. The same check runs on every curation call (Task 2), and an answer is discarded when it fails.
  - **codex**, which does not report a tool list: the permission profile is probed directly, with no model, through `codex sandbox` under the curator's profile and flags: a command that writes a canary file and one that connects to a local listener must both be refused, and the disabled features (plugins, apps, browser, computer use, image generation, web search) must be absent from the run's configuration. Where codex cannot start a command at all under the profile (Linux, docs/spike/curator-isolation.md), that refusal is the result.
  - **The canary curation call** of spec 6.5 (touch a canary file, read one into the answer, fetch a local listener's URL) runs as well, as supporting evidence and to list the files the CLI writes under its home during a call (spec 6.3). It can fail the gate; it can never pass it alone.
  - `isolation::gate(db, cli, version) -> Gate` returns Passed, Failed or Unknown from `providers.db`; the chain runs a CLI only on Passed and probes again when the version changes.

- [ ] **Step 1: Failing tests** with fake CLI scripts: one that obeys each instruction (fails); one that ignores them but reports a tool in its init event (fails: `a_cli_that_declines_the_canary_but_holds_a_tool_fails_the_gate`); one that reports no tools and ignores them (passes); a fake `codex sandbox` that lets the canary write through (fails); `a_new_version_is_probed_again`.
- [ ] **Step 2: Implement** the reported-tool check, the codex sandbox probe, the listener on 127.0.0.1 with a random port, the canary files in a private directory, and the version from `<cli> --version`.
- [ ] **Step 3: Live run** in the dogfood user for claude, codex (also checking that a spawned sub-agent keeps the permission profile, spec 6.5) and agy with a custom no-tool `--agent`; fill the per-CLI table.
- [ ] **Step 4: Security review** under rules/security.md of Tasks 2 and 3 together.
- [ ] **Step 5: Commit** `curation: the isolation gate, per CLI version (milestone 3, Task 3)`.

---

## Task 4: Budgets and the pre-flight size check

**Files:**
- Create: `src/budget.rs`
- Modify: `src/config.rs` (`max_request_tokens`, `daily_tokens`, `monthly_usd` with `usd_per_mtok_in` and `usd_per_mtok_out` on an entry), `src/provider.rs`
- Test: `src/budget.rs`

**Interfaces:**
- Produces: `budget::estimate(text: &str) -> u32` (D7's script counts); `budget::factor(db, provider) -> f64` (median of the last 50 answers' `prompt_tokens / est_tokens`, from `provider_calls`, where the chain stores each call's uncalibrated estimate; 1.0 with fewer than 5); `budget::admit(db, entry, estimated_tokens) -> Admit` (Go, TooBig, DailyCalls, DailyTokens, MonthlyUsd, RateHeader). A paid entry is admitted only when the month's spend plus this call's input estimate plus its largest possible output (`max_tokens` or the entry's output cap, at the output price) stays within `monthly_usd`. A Groq answer's `x-ratelimit-remaining-requests` and `-tokens` are stored in `provider_state` and a later call that would exceed them waits.

- [ ] **Step 1: Failing tests.** `a_japanese_window_over_groqs_ceiling_is_skipped_without_a_call` (Review Focus 2; the fake endpoint sees no request); `after_a_413_the_entries_with_the_same_ceiling_are_skipped_for_that_window`; `the_factor_follows_recorded_usage`; `a_month_over_its_usd_cap_skips_the_paid_entry`; `a_call_whose_largest_output_would_cross_the_cap_is_not_admitted` (spend just below the cap); `a_rebuild_leaves_the_months_spend` (with Task 11).
- [ ] **Step 2: Implement.** Calibrate D7's two coefficients on the sweep's recorded usage before fixing them.
- [ ] **Step 3: Run** the tests and the suite.
- [ ] **Step 4: Commit** `providers: token and money budgets and the pre-flight size check (milestone 3, Task 4)`.

---

## Task 5: Windows and the curation phase

**Files:**
- Create: `src/curate.rs`
- Modify: `src/raw.rs` (the `ops` table, `append_ops`, `ops_after`, `curation_checkpoint`; op lines for `Rebuild`), `src/backup.rs` (ops exported with their own `op_seq` cursor, D1), `src/replay.rs` (`source = 'replay'`, D9), `src/worker.rs` (the phase after `pass`; D10's stay-up rule), `src/setup.rs` (doctor: pending windows with reason and `next_attempt_at`, overdue flagged), `src/main.rs`, `src/hook.rs` (drop `observe` and its spawn, D14)
- Delete: `src/observe.rs`
- Test: `src/curate.rs`, `src/raw.rs`, `src/worker.rs`

**Interfaces:**
- Produces:
  - `raw::Op { device, op_seq, kind: OpKind, ts, body: Value }` with `OpKind::{Window, Claim, Correction, Digest}`; `Raw::append_ops(&mut self, ops: &[OpBody]) -> Result<Vec<i64>>` in one transaction; `Raw::ops_after(device, op_seq, limit)`; `Raw::curation_checkpoint(device) -> Result<(i64, Option<i64>)>` (seq and offset, D2).
  - `curate::next_window(raw, k, settings) -> Result<Option<Window>>`: pages through `Raw::after` from the checkpoint, bounded by events and bytes (issue #54), cut per D12 at `window_tokens` (D8); an event over the cap is split into parts with evidence offsets inside it, or elided with the "seen, elided" marker if it is a tool output.
  - `curate::run_phase(home, raw, k, db, settings) -> Result<Phase>` where `Phase` is Curated, Waiting(until) or Idle. Before each subscription call it checks D9's gate.
  - Pending windows: `pending(device, from_seq, to_seq, reason, attempts, next_attempt_at)` in `providers.db`. A pending row counts only for the same request: the phase replaces a row whose range (start and end) is not the next window's (after a restore, or when records added since made the window longer), whose request text changed (new rules, another language), or whose chain or idle gate changed (the owner edited the providers, their caps or `idle_minutes`, so a hold is the old one's), keeping the call ledger and budget state. Test: `a_restore_that_rewinds_raw_drops_the_pending_rows_above_it`.
- Consumes: `Chain::run` (Task 1), `budget::admit` (Task 4), `isolation::gate` (Task 3), `redact::outbound` and the exclusion check (the egress gate).

- [ ] **Step 1: Failing tests.**
  - `the_window_and_its_checkpoint_commit_together`: a fake chain answers, the worker is stopped between the answer and `append_ops` (a seam), and on restart the same window is curated again with no duplicate op (Review Focus 4).
  - `a_subscription_waits_while_hooks_arrive_and_a_free_provider_does_not` (Review Focus 3): hook records every minute; a chain of [free that fails, subscription] leaves the window waiting with reason "waiting for the owner to finish"; ten minutes after the last hook record the subscription is called. Neither a tombstone appended by the rescan nor a replayed record resets the wait.
  - `a_window_every_provider_fails_is_skipped_after_three_attempts_and_the_next_one_goes_on` (D11).
  - `every_seq_is_curated_elided_or_skipped` (M2's coverage part, on the replay fixture with a fake chain).
  - `an_event_larger_than_the_window_is_split_and_no_part_is_marked_done_unseen` (issue #54).
  - `a_secret_added_to_the_rules_after_capture_does_not_leave_in_a_window` (the egress gate's second pass on a split event, whole).
  - `ops_survive_a_backup_and_restore`, and `an_op_appended_after_its_records_were_backed_up_is_exported` (records exported first, then a correction op with no new record: the next export carries it).
  - `a_split_events_next_part_starts_where_the_last_window_ended` (a non-tool event over `window_tokens`: its parts are curated in order across a restart between them, none twice, none skipped).
  - `the_worker_sleeps_until_the_gate_opens_and_exits_when_the_wait_is_longer` (D10).
- [ ] **Step 2: Implement** the op log and its backup lines, then the window cut, then the phase and the worker's stay-up rule, then the doctor lines. The prompt of this task is v1's observation prompt; Task 7 replaces it with claims.
- [x] **Step 3: Remove `observe`** (D14) and check that no test or command still reaches `oboete.db` except MCP, the viewer and `import`. Done in part 4: what still opens it is MCP, the viewer, `import`, their CLI twins (`search`, `get`, `timeline`), `eval` and `reindex`. v1's writer (`db::apply_batch`) stays as a test fixture for those readers. Nothing embeds new `oboete.db` documents by itself any more (observe did after each run); `oboete reindex` does.
- Curation is opt-in until the cut-over (spec 7.5): `[summary] curate = true` turns the phase on, with `window_tokens` (D8) and `idle_minutes` (D9) beside it. By default the worker sends nothing, so no test or older home reaches a provider through it.
- [ ] **Step 4: Dogfood.** Build, install as `oboete-b`, run a day of the dogfood user's own sessions through it; doctor shows no overdue window; `provider_calls` shows which providers answered.
- [ ] **Step 5: Commit** `curation: windows, the op log as checkpoint, and the curation phase (milestone 3, Task 5)`.

---

## Task 6: Claims and the claims consumer

**Files:** Create `src/claims.rs`, `src/consumer/claims.rs`; modify `src/knowledge.rs`, `src/worker.rs`.

**Interfaces:**
- knowledge.db: `claims(uid TEXT PRIMARY KEY, kind, status, speaker, scope, repo, body, valid_from, device, op_seq)`, `evidence(uid, quote, anchor_device, anchor_seq, offset, length)`, `edges(from_uid, to_uid, type)` with type supersedes or retracts, `derivations(uid, recipe, tier, op_device, op_seq)`.
- Tombstones after curation: when the rescan (or, from milestone 5, forget) tombstones a record or a range that a claim's or digest's evidence anchors on, the claims consumer drops that claim and every digest citing it from knowledge.db, and the span is queued for recuration (Task 11). Rebuild does the same, so a masked secret cannot come back through a paraphrase. Test: `a_rule_added_after_curation_drops_the_claims_quoting_the_masked_text`.
- `claims::current(k, repo) -> Vec<Claim>`: chain tips, ordered by (valid_from, device, seq) (MUST-M7); the active derivation of a uid is the highest tier, then the newest (MUST-M18).
- Kinds: decision, preference, lesson, fix, open item, repo fact, change; old kinds map as spec 3.2 says; unknown kinds are stored as repo fact with status unverified (D13).
- Identity (MUST-M18): a claim's uid is derived in code, never by the model: a hash of its kind and its evidence anchor (device, seq, and the offset of the quote's start rounded down to its sentence). A recuration whose claim has the same kind and an anchor in the same sentence is a new derivation of that uid, whatever its wording; a claim that no longer appears gets a retract edge. Test: `a_recuration_that_rewords_a_claim_keeps_its_uid_and_its_owner_correction`.

## Task 7: The curator prompt and answer

**Files:** Modify `src/curate.rs`.

**Interfaces:** `curate::prompt(window, carried, candidates) -> String` fences all recorded text as data, lists the window's lines with ids, gives up to 20 current claims found by full-text search of the window across the repository as supersede candidates (MUST-M3; vectors join at milestone 4), and asks for claims with window-local ids so a reversal inside the window can point at its sibling (MUST-M2). `curate::parse(answer) -> Result<Vec<Draft>, AnswerFailure>` with `AnswerFailure::{Empty, Prose, Shape, OverCap}`, each recorded as its own outcome in `provider_calls`.

## Task 8: Code gates

**Files:** Create `src/gates.rs`.

**Interfaces:** `gates::check(window, candidates, drafts) -> Vec<Claim>`: decided needs a verbatim user quote or an acceptance right after the proposal, and a turn ending in "?" or holding a negation never promotes (spec 3.3; the acceptance phrases are collected from the dev transcripts); MUST-M1's table of evidence per (kind, target status); MUST-M4's taint of proposals and quotes that restate tool or file text; global scope only from `oboete pref add` or the viewer; supersedes only among the candidates shown; every evidence quote verbatim in the window; bodies redacted and capped. A draft that fails a gate is stored lower (proposed, or not at all), never raised.

## Task 9: Digests and the manifest's current decisions

**Files:** Create `src/consumer/digest.rs`; modify `src/consumer/manifest.rs`.

**Interfaces:** a digest op per session after its latest window, from the digest chain (spec 1.4: each role has its own chain); each digest line carries the uids it summarises and is dropped at build time when they are not current tips (MUST-M6); a digest is stale when any cited uid is no longer current. The manifest's "Current decisions" part lists current decided claims of the repo.

## Task 10: Owner corrections

**Files:** Modify `src/main.rs`, `src/claims.rs`.

**Interfaces:** `oboete correct <uid> --status ... | --body ...` appends a correction op targeted by uid and raw anchor (spec 3.4); the claims consumer applies it after every derivation, so it survives rebuild and re-derivation (MUST-M21, hard).

## Task 11: `rebuild` and `recurate`

**Files:** Modify `src/main.rs`, `src/worker.rs`.

**Interfaces:** `oboete rebuild` drops knowledge.db and runs the consumers; zero provider calls (asserted through `provider_calls`), identical claims, the month's spend unchanged. `oboete recurate [--skipped | <device>:<from>-<to>]` prints the windows it would send and a cost estimate from D7 and the entries' prices, then, on `--yes`, appends new window ops marked `recurate: true` (they never move the checkpoint, D2) whose claims become new derivations of the same uids (MUST-M18). Test: `recurating_an_old_span_does_not_send_later_windows_again`.

## Task 12: Judge roles

**Files:** Create `src/judge.rs`.

**Interfaces:** selection ("shrink, never drop") and a veto on decided and supersedes, each behind a setting that stays off unless Measurement Judge shows its gain (spec 3.5): selection only if curator input shrinks by 30% or more while recall of decisions, lessons and fixes drops by 0.02 or less.

## Task 13: Lines and the milestone note

**Files:** Create `docs/milestone-3.md`, `docs/spike/curator-sizes.md`.

- The size sweep: {4K, 8K, 12K, 16K, 24K} characters × {Japanese-heavy, English and code} × each entry, at least 4 samples a cell, Groq spread over days; validity, latency, tokens; pass lines per docs/research/curator-providers-2026-09-27.md §8. It feeds D7's coefficients and D8's interim; it does not decide Window.
- M2: coverage 100%, and a crash at 20 points gives identical rows; the test fails against v1 first (spec 8.2).
- Isolation: the per-CLI table (Task 3).
- Window: the smallest window size that passes M2, M3 and M6 on dev transcripts (spec 8.2).
- Cost: curator calls at most 20% of each daily cap on a heavy day; paid at most USD 5 a month.
- M3: tuned on the dev labels of milestone 1; the deciding run on the test labels, which need the owner (see below).
- MUST fixtures: M1, M2, M3, M4, M6, M7, M18, M21.

---

## What needs the owner

Build work does not wait on these; the lines and some defaults do.

1. **M3's test labels** (spec 8.4 row 1, "before milestone 3's deciding run"): 50 overturned pairs with 50 controls (at least 20 across sessions) and 100 real decisions in the held-out transcripts, in owner decision 29's plain-Japanese form; later, 100 claims the curator marked decided. About 3-4 hours in sittings of an hour or less.
2. **Mistral**: the owner's key is limited to 0 requests a minute (`x-ratelimit-limit-req-minute: 0` on 2026-09-27), so every request is refused with 429. Re-enable the free plan in Mistral's console, or take Mistral out of the chain.
3. **Groq Developer tier** (billed per token, about USD 1 a month at the owner's volume, unverified until usage is recorded): it removes the 8,000-token ceiling and the daily-token stops. Or stay free, with Groq taking only windows that fit.
4. **Providers that may train on input** (OpenRouter free models, Mistral free before the opt-out): whether a window may fall through to them at all.

## Stop rule for this document's review

This PR merges when the Codex review of its final head adds no finding that changes an owner decision or one of D1-D15. After two consecutive rounds whose findings are mechanism only, the rest moves into an issue as acceptance tests for Tasks 6-13, each finding is answered and resolved, and the PR merges without a further commit (CLAUDE.md, "Design-doc PRs").
