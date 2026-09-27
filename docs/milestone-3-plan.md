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
  - `curate::next_window(raw, device, window_tokens, rules) -> Result<Option<Window>>`: pages through `Raw::after` from the checkpoint, bounded by events and bytes (issue #54), cut per D12 at `window_tokens` (D8); an event over the cap is split into parts with evidence offsets inside it, or elided with the "seen, elided" marker if it is a tool output.
  - `curate::run_phase(raw, db, rules, summary, chain, curator) -> Result<Phase>` where `Phase` is `Covered`, `Waiting { until, up }` or `Idle` (as built in #140). The curator is injected; the worker's is the chain with `Chain::idle_gate`, which checks D9's gate before each subscription call. `chain` is the providers and caps as text, part of the pending row's identity.
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
- [ ] **Step 4: Dogfood.** Build, install as `oboete-b`, run a day of the dogfood user's own sessions through it; doctor shows no overdue window; `provider_calls` shows which providers answered. This waits for Task 7: the claim ops a day would write are kept records (ops are never rewritten), so they should have Task 7's shape, not the observation shape of this task. Until then the live check in #140 stands in: a release build at bb46bc4 with `curate = true` and two free entries curated the synthetic `overturn-cross` fixture (136 records) in two windows, 24 claim ops, summaries in Japanese, no pending row.
- [ ] **Step 5: Commit** `curation: windows, the op log as checkpoint, and the curation phase (milestone 3, Task 5)`.

---

## Task 6: Claims and the claims consumer

**Files:** Create `src/claims.rs`, `src/consumer/claims.rs`; modify `src/worker.rs` (the `Consumer` trait, `consumers`, `pass`, `behind`), `src/knowledge.rs` (`op_checkpoints`, `checkpoint::rewind`), `src/raw.rs` (`op_devices`, `max_seq_of`, `max_op_seq_of`, `Op.batch`), `src/curate.rs` (`long_text`, which the window cut and the claims consumer share), each `src/consumer/*.rs` (`step` takes the device).

**Interfaces:**
- Consumes: `Raw::{ops_after, tombstones_after}`, `OpKind::{Claim, Correction}`, `knowledge::checkpoint::{get, set, rewind}`, `worker::Consumer`.
- Produces:
  - `Consumer::devices(&self, raw) -> Result<Vec<String>>` and `Consumer::top(&self, raw, device) -> Result<i64>`, and `step` gains the device it reads. By default a consumer reads the local device up to `raw.max_seq()` (raw-seq consumers read other devices' records from milestone 6, with sync). An op consumer reads every origin device that has ops (`Raw::op_devices`, new) up to `Raw::max_op_seq_of(device)` (new; `max_op_seq` stays the local one): its checkpoint is an `op_seq` per origin device (D4), kept in its own table, `op_checkpoints`, because compression waits for the lowest checkpoint in `checkpoints` and would read an op_seq as a seq (`Consumer::checkpoints`). `checkpoint::rewind` goes through the checkpoint rows a consumer has, not only the devices it reads now: a device that lost all its ops is no longer listed. `pass`, `behind` and `checkpoint::rewind` go through each consumer's devices with that device as the checkpoint key, and compare each checkpoint with the same consumer's `top` for that device, never with raw's highest seq. So an op another device wrote (sync, milestone 6) reaches `current` through the worker's own path.
  - `claims::ClaimOp`, the body of a claim op (serde), which Task 7 writes: `{id, kind, status, speaker, scope, body, evidence: [Evidence], supersedes: [String], recipe, tier}` (#144: a rebuild picks the active derivation from the ops alone; tier 0 code only, 1 free or local, 2 subscription, 3 paid), where `Evidence = {device, seq, offset, length, sentence, quote}`. `offset` and `length` locate the quote in the event's text as the window read it (byte offsets, as `Window.from_offset`); `sentence` is the offset of the start of the sentence the quote starts in, which the phase computes (Task 7) so that the consumer never reads raw to find it. `id` is local to the window (MUST-M2); `supersedes` holds candidate uids or sibling ids.
  - `claims::uid(kind, &Evidence) -> String`: the hex SHA-256 of (kind, device, seq, sentence) of the first evidence (MUST-M18). The model never names a uid.
  - knowledge.db (all derived, dropped by `rebuild`):
    - `derivations(op_device, op_seq, uid, ts, tier, recipe, kind, status, speaker, scope, repo, body, valid_from, anchor_device, anchor_seq)`: one row per claim op; the active one of a uid is the highest tier, then the newest by the op's `ts`, with `(op_device, op_seq)` only to break a tie (MUST-M18, #144).
    - `evidence(op_device, op_seq, idx, device, seq, offset, length, sentence, quote)`: keyed by derivation (#144), so a masked quote removes only the derivation that quoted it.
    - `edges(op_device, op_seq, to_uid, type)`: what a derivation supersedes; it counts while that derivation is active.
    - `claims(rowid, uid UNIQUE, op_device, op_seq)`: each uid's active derivation.
    - `claim_skips(op_device, op_seq, reason)`: claim ops that gave no claim (the observation shape, no evidence, a quote that no longer reads), for doctor's count.
    - Task 10 adds `corrections`, applied over the active derivation.
    - `claims_fts` (FTS5 trigram over body and quotes, rowid = the claim's rowid), which Task 7 searches for candidates.
  - `claims::current(k, repo) -> Result<Vec<Claim>>`: the chain tips of `repo` (no active derivation's edge points at them) that are not retracted once the correction overlay is applied (Task 10), ordered by spec 3.4's (valid_from, device, seq) of the first quote's event, then the uid, so two claims of one event keep one order on every device (MUST-M7).
  - A claim is live only while each of its quotes still reads verbatim at its anchor in raw as `Raw::after` returns it (a tombstoned record reads as nothing, a masked range as its mask). This is what keeps a secret masked after curation from coming back through a claim (spec 6.4), and it depends on raw only, so rebuild reaches the same state.
  - `consumer::claims::Claims` (after `Fts`, before `Manifest`): reads ops after its checkpoint and, for each claim op whose quotes are live, writes a derivation and recomputes the uid's active row; a `supersedes` entry that names a sibling id resolves to that sibling's uid in the same batch (ops of one `append_ops` share a `batch`).
  - `consumer::claims::Anchors` (after `Claims`): a raw-seq consumer. For each tombstone after its checkpoint (`Raw::tombstones_after`), it checks the derivations with a quote in the target record and deletes those no longer live, with their quotes, edges and search rows (the uid's next derivation becomes active), and records the span of the window op appended with the claim op (`Raw::window_of`, same batch) in `recurate(device, from_seq, to_seq, op_device, op_seq)` for Task 11 (the claim op that lost the quote is kept, so `Claims`'s rewind of a lost op takes its row back out). `Claims` queues the same span when it skips a claim op for a quote that no longer reads, so a rebuild gives the same `recurate`. Its rewind does nothing: a restore that loses a tombstone leaves the claims it dropped hidden until `rebuild`.
- Kinds and status: decision, preference, lesson, fix, open item, repo fact, change; the old kinds map as spec 3.2 says (feature to change; discovery to repo fact); any other kind is stored as repo fact with status unverified (D13). A claim op with no `evidence` (the observation shape of part 3b, written only where `curate = true` before Task 7) is skipped and counted in doctor's line.
- `valid_from` is the `ts` of the record the first evidence anchors on (spec 3.4), read from the op's evidence device and seq when the consumer writes the row; a record gone since (tombstoned) removes the claim anyway (`Anchors`).
- The offsets are into the text `curate` cuts windows in (the event's long text, as `Window.from_offset`); the consumer reads that text through the same function the window cut uses, so the two never disagree.

- [ ] **Step 1: Failing tests.**
  - `a_restore_that_loses_ops_rewinds_the_op_consumers_only` (`src/worker.rs`): ops appended, the claims checkpoint past them, then `ops` rows deleted as a restore without them would leave: `checkpoint::rewind` moves `claims` back to `max_op_seq` and leaves `fts` where it was.
  - `a_claim_ops_uid_is_its_kind_and_the_sentence_its_quote_starts_in` (`src/claims.rs`): two ops, same kind and `sentence`, different wording and offset: one uid, two derivations, the newer one active.
  - `a_reversal_in_one_append_supersedes_its_sibling` (MUST-M2): ops `c1` (decided) and `c2` (decided, supersedes `c1`) in one batch: one tip, one edge.
  - `ties_resolve_the_same_way_whatever_order_the_ops_arrive` (MUST-M7): 50 pairs of claims with equal `valid_from` under two devices (the second device's op rows written directly, as sync will from milestone 6, then consumed by the worker's `pass`); `current` is the same list whichever device's ops the consumer reads first.
  - `a_rule_added_after_curation_drops_the_claims_quoting_the_masked_text`: a claim's quote falls in a range the rescan tombstones; after the next pass the claim, its evidence and its search row are gone and `recurate` holds its span; `rebuild` gives the same.
  - `an_unknown_kind_is_stored_as_an_unverified_repo_fact` and `an_op_with_no_evidence_is_skipped`.
- [ ] **Step 2: Implement** `Consumer::{devices, top}`, the device argument of `step` and their three callers first (the other consumers keep the default), then the schema, `ClaimOp`, `uid`, `Claims`, `Anchors`, `current`.
- [ ] **Step 3: Commit** `claims: the claims consumer, uids from evidence, chain tips and the tie order (milestone 3, Task 6)`.

## Task 7: The curator prompt and answer

**Files:** Modify `src/curate.rs`, `src/worker.rs` (the phase gets knowledge.db), `src/provider.rs` (the answer check), `src/raw.rs` (`first_prompt`, `last_window_ops`), `src/config.rs` (`Provider::tier`), `src/search.rs` (`trigrams_upto`), `src/providers_db.rs` and `src/view.rs` (the refused outcomes count as sent requests and failures).

**Interfaces:**
- Consumes: `curate::{next_window, run_phase, Window}`, `redact::hidden` and the pieces' `from`/`to`, `claims::{current, ClaimOp, Evidence}`, `claims_fts`.
- Produces:
  - `Window.lines`: each piece's line in `text` with a window-local line id (`L1`, `L2`, ...) and the piece's (device, seq, from, to), so a quote can be traced to its event.
  - `curate::carried(raw, k, rules, window) -> Result<String>`: per session in the window, its goal (its first prompt, `Raw::first_prompt`, gated and cut to 200 characters), its current open items (the session's own in every repository its lines are in, newest first, up to 50, `Raw::session_key` reading the labels only), and the claims its previous window left proposed and that are still current (`claims::tip_repo`: a sibling or a later window may have settled one) (`Raw::previous_window_ops`: the ops appended with the last window op, not a recuration, that covered the session's latest event before this window; per session, since another session's window may come between) (D12, spec 3.3: a proposal and its acceptance in two windows are gated as if in one). Every value goes through the gate; fenced as data, each session's block headed `### <agent> session <id>` as its lines are. As built, a child session (a subagent) carries nothing of its parent's until capture records the link (marked `ponytail:`; spec 3.1).
  - `curate::candidates(k, repo, text) -> Result<Vec<Claim>>`: up to 20 current claims of the repo that `claims_fts` finds for the window's text, as one OR query of 64 trigrams spread over the whole text (`search::trigrams_upto`), filtered to the repository before ranking so another repository's better matches never crowd it out; each repository of the window once, found by that repository's own lines, and shown under `### in <repository>` as the window's headings name it. A session's heading is shown again where its checkout changes inside a window (the budget counts each), and each carried proposal and open item names its repository, so every line and uid sits under its own repository. The candidates and what the sessions carry share a fifth of `window_tokens` (1,000 at the default), in whole lines from the start of each part: the carried context takes at most half (every session's goal and proposals before any session's open items; a proposal is its uid's active derivation, once, while that is still a current proposal), the candidates the rest, each repository's block a share, the smallest first so what one leaves goes to the others (`curate::fit`); a uid cut from the prompt is superseded by nothing. The prompt then stays within a free provider's request ceiling (Groq's 8,000 tokens). A draft's `supersedes` keeps a sibling's id when the sibling's line is in the draft's line's repository, a candidate uid shown for that repository, or a uid its session carried in (an open item, a proposal) of the same repository: another repository's claim would leave that repository's current tips (Task 8's gates narrow it further) (MUST-M3: repo-wide, every window; vectors join at milestone 4). Candidates only: similarity never supersedes. A draft whose claim op is over `raw::MAX_OP_BYTES` is no claim, so the window is still covered. A draft id shaped like a uid (64 hex) makes the answer `shape`: the claims consumer reads a `supersedes` entry that names no sibling as a uid. A trigram never holds a control character (FTS5 reads a query as a C string and stops at a NUL). A session's previous window is the one that covered its latest event with text, not a resumed session's lone `start`. The prompt asks for quotes from a line's prompt, reply or tool output, never a tool's input, which `long_text` does not keep.
  - `curate::prompt(language, text, candidates, carried) -> String`: all recorded text between two `=== RECORD <token> ===` lines, the token the first 16 hex digits of the SHA-256 of what they enclose, so no record can close the fence (spec 3.3: file and tool content is quotation, never instruction); the lines with their ids, the candidates with their uids (their bodies through the gate), and the answer schema `curate::schema()`: `{claims: [{id, kind, status, speaker, scope, body, quote, line, supersedes}], summary}`.
  - `curate::parse(answer) -> Result<(String, Vec<Draft>), AnswerFailure>` (the summary and the drafts) with `AnswerFailure::{Empty, Prose, Shape, OverCap, Unanchored}`; more than `MAX_CLAIMS` (50) drafts is `OverCap`, an object with neither `claims` nor `summary` but other keys is `Shape`, and so are two drafts with one id (a sibling's `supersedes` would link the wrong one). `Chain::check(&AnswerCheck)` (`Fn(&Value) -> Option<&'static str>`) takes the phase's `curate::check(window, answer)` (`parse`, then `locate` for `Unanchored`), which the phase hands the chain as the curator's fourth argument (`curate::Curator`). It runs before the schema test, and an answer it accepts is not tested against the schema (a line id written as a number is usable): an answer it refuses is recorded under the failure's own `provider_calls.outcome` (`empty`, `prose`, `shape`, `over_cap`, `unanchored`) and the chain goes on to its next entry, as a schema mismatch does with `invalid`. In `ChainFailed` it is a `Skip::Failed`, so it counts for D11. A model's answer that is not JSON reaches the check as text (`Value::String`), so prose and an empty answer get their own outcomes; without a check the schema refuses it as `invalid`. The detail keeps why the text is not JSON (an answer cut at `max_tokens`), never the text. The daily request budget (`calls_today`), the unmetered usage and `oboete view`'s failures count the five outcomes as they count `invalid`.
  - `curate::locate(window, line, quote) -> Option<Evidence>`: the line id as models write it (`L4`, `4`, `[L4]`, `l4`, or the number 4; 67 of 140 drafts of the dev label run were lost to that alone, docs/milestone-1.md), and the quote found verbatim in that line's text as the gate showed it, mapped back to the event's own offsets through the piece's range and `redact::hidden`'s map, with `sentence` set to the start of its sentence in the whole event (the piece keeps where the sentence it starts inside began, so a split never changes a uid) (after `。`, `.`, `?`, `!`, `？`, `！` or a line break, else the piece's start). A draft whose quote is not found is not a claim (Task 8 drops it); an answer in which no draft is found is `AnswerFailure::Unanchored`.
  - The phase writes `ClaimOp`s (Task 6) instead of the observation shape, with the answering entry's name as `recipe` and its tier (`ChainResult.tier`, from `Provider::tier`: 3 paid, 2 subscription, 1 otherwise); the window op keeps `summary`.
  - `run_phase` gains `k: &Connection` (knowledge.db, read only, for `carried` and `candidates`), and `worker::CurationPhase` becomes `FnMut(&mut Raw, &Connection) -> Result<Phase>`, called with the `k` that `serve` holds. The pending row's identity (today a SHA-256 over the chain, the idle gate and the whole prompt, #140) moves to the chain, the idle gate, the language and the window's gated text: the carried context and the candidates change while a window waits (an owner correction, a recuration, a claim a rescan drops, and from milestone 6 another device's claims), and a window every provider fails must still reach D11's three counted attempts. Test: `a_window_pending_while_its_candidates_change_keeps_its_attempts`.
- [ ] **Step 1: Failing tests.**
  - `a_quote_is_located_in_its_event_through_masks_and_splits`: a quote after a masked secret and in the second part of a split event gets the event's own offsets; the same quote in text the gate hid is not found.
  - `each_answer_failure_is_recorded_as_its_own_outcome`: empty, prose, wrong shape, over the cap, no draft anchored.
  - `the_candidates_are_the_repos_current_claims_the_window_mentions` (MUST-M3): a decision from another session of the repo is a candidate; one from another repo is not.
  - `recorded_text_is_fenced_as_data`: an instruction inside a tool output reaches the prompt only inside the data fence.
  - `a_window_pending_while_its_candidates_change_keeps_its_attempts` (D11): a window every provider fails keeps its counted attempts when one of its candidates is dropped between two attempts (a rule the rescan applies tombstones its quote, Task 6's `Anchors`; owner corrections come in Task 10).
- [ ] **Step 2: Implement.** Keep `schema()` the chain's shape test; the observation fields go.
- [ ] **Step 3: Commit** `curation: claims with speaker, status, evidence and supersedes; candidates from the repo's current claims (milestone 3, Task 7)`.

## Task 8: Code gates

**Files:** Create `src/gates.rs`; modify `src/curate.rs` (the phase calls the gates before `append_ops`).

**Interfaces:**
- Consumes: `Draft`, `Window.lines` (speaker, kind of each line), `curate::locate`, candidates.
- Produces: `gates::check(window, candidates, drafts) -> Gated { claims: Vec<ClaimOp>, dropped: Vec<(String, &'static str)> }`. The gates run in the phase, so a claim op holds the gated status and rebuild needs no gate (D4). A draft that fails a gate is stored lower (proposed) or not at all, never raised; the window op records `dropped` with each reason.
  - Gate 1 (spec 3.3): decided needs a verbatim user quote, or a user acceptance right after the proposal. A turn that ends in `?`/`？` or holds a negation never promotes. The acceptance phrases (`はい`, `それで`, `OK`, `進めて`, ...) and negations (`ない`, `やめ`, `not`, `don't`, ...) are two lists in `gates.rs`, collected from the dev transcripts (counts only in the PR, spec 8.4).
  - MUST-M1's table, (kind, target status) to the evidence it needs: done needs a user quote or a passing run in the window (a tool line with exit 0 or a passing test); retracted, and retiring a lesson, need a user quote; any change to a global claim needs an owner directive (`oboete pref add`, the viewer), else it is flagged, not applied.
  - MUST-M4's taint: a proposal or a user span is tainted when it overlaps a tool output or file read in the same window by either of two deterministic measures, both after NFKC, lower case and collapsed whitespace: a shared run of 40 characters or more (a paste), or the share of its character trigrams found in those spans at or above τ (a paraphrase that keeps the words). A tainted proposal needs a verbatim restatement by the owner (a bare acceptance is not enough), and a tainted user span is not a quote for gate 1. The 40 and τ are tuned on the dev split (Task 13) with the three canaries at 0% and `decided` recall at 0.80 or more; if no τ meets both, Task 13 records it and the gate falls back to provenance: every proposal made after a tool output or file read in the same turn needs the owner's restatement.
  - Speaker: an inferred claim (assistant inferred) cannot become decided without a tool result or a repeated statement (spec 3.2).
  - Scope: global only through `oboete pref add` or the viewer (spec 3.3); a draft's `global` becomes `repo`.
  - Supersedes only among the candidates shown and the window's siblings (MUST-M2, M3); anything else is dropped from the draft.
  - A change carries why with evidence or `why: unknown` (spec 3.3); a bare "N files changed" is dropped.
  - Bodies and quotes through the egress gate; a body over the claim cap (1,000 characters, Global Constraints, spec 6.5) is dropped with the reason `over_cap`, never cut.
- [ ] **Step 1: Failing tests.** One per gate above, plus the three M4 canaries as fixtures (a paraphrased attacker file then `はい`; a pasted file with a decisive quote; a fake acceptance line inside tool output): none reaches decided.
- [ ] **Step 2: Implement** the gates as plain functions over the lines, no model call (spec 3.5: the none tier is code gates only).
- [ ] **Step 3: Commit** `curation: code gates on status, speaker, taint, scope and supersedes (milestone 3, Task 8)`.

## Task 9: Digests and the manifest's current decisions

**Files:** Create `src/consumer/digest.rs`, `src/digest.rs`; modify `src/consumer/manifest.rs`, `src/curate.rs` (the digest call), `src/worker.rs`.

**Interfaces:**
- Consumes: `claims::current`, `Chain::run` with role `digest`, `OpKind::Digest`, the manifest's parts.
- Produces:
  - A digest op per session, written by the curation phase after the session's latest window is covered and the session has ended or been idle for `idle_minutes`: `{session, repo, lines: [{text, uids}]}`. The prompt fences claim bodies as data (MUST-M6).
  - The digest uses the same chain with `role = "digest"` in `provider_calls` (spec 1.4 lets each role have its own chain; one list until measurement asks for two; Claude's, overrulable).
  - `consumer::digest::Digests` (op consumer, `top` = `max_op_seq`): stores the latest digest per session; a line whose uids are not all current tips is dropped at build time, and a digest with a dropped line is marked stale (MUST-M6, spec 3.4).
  - The manifest gets a "Current decisions" part: the repo's current decided claims, newest first, within the manifest's cap.
  - Digests are built in two parts. Part B1, on its own: `digest::DigestOp` `{agent, session, repo, through: {device, seq}, lines: [{text, uids}]}`, refused with a reason in `digest_skips` when a line is empty or cites no 64-hex uid, or the text is over 2,000 characters (spec 6.5); `consumer::digest::Digests` keeps every digest op and the uids it cites (`digest_cites`, so the forget path of spec 6.2 finds a deleted claim's digests); SessionStart's manifest shows the repository's newest digest, after the current decisions, only while every uid it cites is a current claim of that repository (one indexed lookup per uid at read time), else nothing, never an older digest. Part B2, after Tasks 7 and 8 merge: the curation phase's digest call (its trigger, the prompt with claim bodies fenced as data, `Chain::check` for role `digest`, a hold while a digest waits).
  - Built first, on its own: "Current decisions and open items" (spec 4.9's words), the repo's current tips that are decided or open items not done, newest first, at most 15 (`claims::decisions`: the filter, the order and the limit are its query's, and the `derivations_repo` and `claims_op` indexes let it walk the repository's newest first and stop at the limit, as the hook runs it at every SessionStart: 0.06 ms against 52 ms for a scan at 100,000 claims), one line each clipped like the other parts, before the owner's directives. It is composed when the manifest is read, not built with it: curation runs while the owner is idle and appends no record the manifest consumer steps on, so a part built with the text would miss the last session's decisions at the next SessionStart. `claims::current` no longer runs the schema DDL, so the read-only manifest connection writes nothing.
- [ ] **Step 1: Failing tests.** `a_digest_line_citing_a_superseded_claim_is_dropped`, `an_instruction_in_a_claim_body_yields_no_uncited_line` (MUST-M6), `the_manifest_lists_the_current_decisions_of_its_repo`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Commit** `claims: digests that cite current claims, and the manifest's current decisions (milestone 3, Task 9)`.

## Task 10: Owner corrections

**Files:** Modify `src/main.rs`, `src/claims.rs`, `src/consumer/claims.rs`.

**Interfaces:**
- Produces: `oboete correct <uid> (--status <s> | --body <text>)` appends a correction op `{uid, anchor: {device, seq}, status?, body?}` (spec 3.4); `Claims` writes it into `corrections`; `claims::current` applies the newest correction of a uid over whatever derivation is active, so it survives rebuild, recuration and re-derivation (MUST-M21, hard). A correction for a uid with no claim yet is kept and applies when one appears.
- **As built**: the correction op is `claims::CorrectionOp` `{uid, anchor: {device, seq}, status?, body?}`; `oboete correct <uid> (--status <s> | --body <text>)` finds the claim's anchor in knowledge.db (holding raw.db first, as every reader does), puts the body through the storage gate (`redact::scan`), refuses an unknown uid, an unknown status, an empty body or one over 1,000 characters, appends the op and runs the consumers. `Claims` keeps each correction in `corrections` (a faulty one goes to `claim_skips` with its reason; a rewind drops what the lost ops gave), and the search text of a corrected uid is its corrected body. Every query of current claims reads the `active` view: each claim's active derivation with, per field, the newest correction of its uid over it, so it holds over recuration, re-derivation and rebuild, and one for a uid with no claim yet applies when the claim arrives. The manifest's read-only connection shows decisions only once the view exists (a knowledge.db the worker has not yet given this schema shows none). `oboete claims` lists the current claims of the current directory's repository, each with its uid, kind and status: where the owner or an agent finds the uid `correct` takes, until milestone 4's search shows claims.
- [ ] **Step 1: Failing test.** `a_recuration_that_rewords_a_claim_keeps_its_uid_and_its_owner_correction` (MUST-M18, M21): correct a claim to retracted, recurate its window with a reworded answer, rebuild: the uid is the same, the correction still applies, zero provider calls during the rebuild.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Commit** `claims: owner corrections as ops, applied over every derivation (milestone 3, Task 10)`.

## Task 11: `rebuild` and `recurate`

**Files:** Modify `src/main.rs`, `src/worker.rs`, `src/curate.rs`.

**Interfaces:**
- `oboete rebuild`: under the worker lock, knowledge.db is moved aside and the consumers run from zero; no provider is called (asserted through `provider_calls`), the claims are identical, the month's spend unchanged (spec 1.7). The old file is removed once the new one is complete. Built first, on its own (`worker::rebuild`): under raw.lock held exclusively, as a restore holds it (every reader of knowledge.db holds raw.db open while it reads), the file is moved aside as `knowledge.db.rebuilding-<ms>` with its sidecars under the names SQLite looks for beside it (`...-wal`, `...-shm`; the WAL first), so a rebuild that fails keeps a file that opens with its last commits, named in the error. The consumers then run under the worker lock with no curation phase. A running worker, or a store open elsewhere past raw.lock's 10-second wait, makes it refuse, as `restore` does.
- `oboete recurate [--skipped | <device>:<from>-<to>] [--yes]`: lists the windows it would send (skipped window ops, the `recurate` spans of Task 6's `Anchors`, or the given range, cut as `next_window` cuts them) and an estimate from D7's tokens and the entries' prices; with `--yes` it appends window ops marked `recurate: true` (they never move the checkpoint, D2), whose claims become new derivations of the same uids (MUST-M18). A claim whose first evidence anchors inside the span and whose uid the new answer does not produce gets a derivation with status `retracted` in the same `append_ops` (newer at the same tier, so it is the active one), so a recuration removes a false or obsolete claim; an owner correction still applies over it (Task 10). A span is taken off `recurate` when its window op lands, and one a later `recurate: true` window op covers is not queued again (a rebuild queues the spans of every dropped claim op again, recurated or not).
- [ ] **Step 1: Failing tests.** `rebuilding_gives_the_same_claims_with_no_call`, `recurating_an_old_span_does_not_send_later_windows_again`, `a_skipped_window_is_curated_by_recurate_skipped`, `a_claim_a_recuration_no_longer_produces_is_retracted`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Commit** `curation: rebuild and recurate (milestone 3, Task 11)`.

## Task 12: Judge roles

**Files:** `src/judge.rs`, only if Measurement Judge shows a gain.

**Interfaces:** selection ("shrink, never drop") and a veto on decided and supersedes, each behind a setting that stays off unless Measurement Judge (Task 13) shows its gain (spec 3.5): selection only if curator input shrinks by 30% or more while recall of decisions, lessons and fixes drops by 0.02 or less; the veto only if it removes wrong decided or supersedes without lowering decided recall below the M3 line. No step bodies until the measurement: when neither role passes, this task ends with its result in the milestone note and no code.

## Task 13: Lines and the milestone note

**Files:** Create `docs/milestone-3.md`; extend `docs/spike/curator-sizes.md`.

- The size sweep: {4K, 8K, 12K, 16K, 24K} characters × {Japanese-heavy, English and code} × each entry, at least 4 samples a cell, Groq spread over days; validity, latency, tokens; pass lines per docs/research/curator-providers-2026-09-27.md §8. It feeds D7's coefficients and D8's interim; it does not decide Window.
- M2: coverage 100% (`every_seq_is_curated_elided_or_skipped` on the dev transcripts), and a crash at 20 points gives identical rows; the test fails against v1 first (spec 8.2).
- Isolation: the per-CLI table (Task 3).
- Window: the smallest window size that passes M2, M3 and M6 on dev transcripts (spec 8.2).
- Cost: curator calls at most 20% of each daily cap on a heavy day; paid at most USD 5 a month.
- M3: tuned on the dev labels of milestone 1; the deciding run on the test labels, which need the owner (see below).
- MUST fixtures, each a test of the task that owns it: M1, M4 (Task 8), M2, M7, M18 (Task 6), M3 (Task 7), M6 (Task 9), M21 (Task 10).

---

## What needs the owner

Build work does not wait on these; the lines and some defaults do.

1. **M3's test labels** (spec 8.4 row 1, "before milestone 3's deciding run"): 50 overturned pairs with 50 controls (at least 20 across sessions) and 100 real decisions in the held-out transcripts, in owner decision 29's plain-Japanese form; later, 100 claims the curator marked decided. About 3-4 hours in sittings of an hour or less.
2. **Mistral**: the owner's key is limited to 0 requests a minute (`x-ratelimit-limit-req-minute: 0` on 2026-09-27), so every request is refused with 429. Re-enable the free plan in Mistral's console, or take Mistral out of the chain.
3. **Groq Developer tier** (billed per token, about USD 1 a month at the owner's volume, unverified until usage is recorded): it removes the 8,000-token ceiling and the daily-token stops. Or stay free, with Groq taking only windows that fit.
4. **Providers that may train on input** (OpenRouter free models, Mistral free before the opt-out): whether a window may fall through to them at all.

## Stop rule for this document's review

This PR merges when the Codex review of its final head adds no finding that changes an owner decision or one of D1-D15. After two consecutive rounds whose findings are mechanism only, the rest moves into an issue as acceptance tests for Tasks 6-13, each finding is answered and resolved, and the PR merges without a further commit (CLAUDE.md, "Design-doc PRs").
