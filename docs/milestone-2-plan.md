# Milestone 2 (Record) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Design B's recording layer: `raw.db` keyed by (device, seq), full redaction with a ledger, durable writes, write-failure reporting, backups and tombstones from day one, raw full-text search and a deterministic manifest, so that the none tier works end to end (record, search, resume) in the dogfood user.

**Architecture:** Hooks append redacted events to `raw.db` (synchronous=FULL). A worker, started by hooks and exiting when idle, runs the milestone's consumers in order of (device, seq): raw full-text index, manifest, compression, backups, redaction rescan. Derived data and consumer checkpoints live in `knowledge.db` (synchronous=NORMAL), which can be rebuilt from `raw.db`. The v1 store (`oboete.db`) is never opened by this code.

**Tech Stack:** Rust (edition 2024), rusqlite (bundled SQLite, FTS5 trigram), the existing gitleaks-based `redact.rs`, `zstd` (new dependency), no async runtime on the hook path.

**Spec:** `docs/spec.md` sections 1-2, 4.9, 8.2 (M14) and 8.4 row 2. Research behind it: `docs/research/redesign-2026-09-24/`. Spike 1: `docs/spike/hook-m14.md`.

## Global Constraints

- `raw.db`'s only ordering and partition key is (device, seq). Session, repo and branch are labels, never a required parent row, an index root, or a curation or checkpoint boundary (spec 1.6).
- Session events such as Stop and SessionEnd remain triggers (when to curate, refresh or back up). They are never keys (spec 1.6).
- Hooks never wait on AI. The hook path starts no async runtime and nothing of the viewer (spec 2.1; issue #53 checkbox 5). `cargo tree` shows the hook's code path does not reach tokio, and a test runs the hook with no runtime initialised.
- The curator's own CLI sessions are not captured (spec 2.1).
- Redaction scans every byte that is stored. The ledger records rule id, offset, length, time and ruleset version, never the value (spec 2.2). The redaction change is reviewed under rules/security.md (spec 2.2).
- `raw.db` uses synchronous=FULL; startup reconciles consumer checkpoints above raw's highest sequence (spec 2.5).
- A write failure never blocks the agent (spec 2.5).
- Backups are sealed, compressed segments with checksums (spec 2.6).
- Tombstones carry no body and are never compacted (spec 5.8).
- Keys: never printed, never passed in a subprocess environment (CLAUDE.md, `provider.rs`).
- The owner's machines keep running v1 until the cut-over after milestone 5 (spec 7.5). Design B runs only in the `oboete-dogfood` user and in temporary homes until then (CLAUDE.md).
- Claude Code writes the core (raw store, delivery) and every security-scope part (redaction). Codex or Grok take independent pieces only: agent adapter ports and transcript parsers (spec 8.4, "Who builds").
- CI: `cargo fmt --check`, `cargo clippy`, `cargo test`.

## Review Focus

1. **Two hooks at once.** Two agents (or a hook and the worker) append in the same millisecond. Expected: both events land, with distinct consecutive seqs, and neither hook waits past the busy timeout. Test in Task 1.
2. **A secret across a cut.** A token straddles the head/tail cut of an oversized output, or the boundary of two redaction windows. Expected: no unmasked fragment of it is stored. Test in Task 3.
3. **A crash between raw and a checkpoint.** Power is lost after a consumer checkpoint moved but before raw's commit was durable (the MUST-M14 hole). Expected: on the next worker start the checkpoint is rewound together with that consumer's output for the lost seqs, doctor says so, and no later event is skipped or collides with a ghost row. Test in Task 5.
4. **A full disk.** The hook cannot write. Expected: the agent is not blocked, nothing claims the event was recorded, a marker outside the database records the failure class and time, doctor is red, and the next injection says recording failed since T (MUST-M16). Test in Task 4.
5. **A wake-up lost at worker exit.** An event arrives after the worker's last check for pending work and before it exits. Expected: that event is indexed without another hook (spec 1.1, issue #55). Test in Task 5.

---

## Decisions

Each decision is Claude's unless marked otherwise, and the owner can overrule it. They settle what the spec leaves to milestone 2 or leaves open.

**D1. v1 is frozen on a branch before the first code PR.** Before Task 1 merges, branch `v1` from the main commit the owner's binary is built from. The owner's reinstalls come from `v1` from then on. A v1 fix (real loss or safety only, CLAUDE.md) is a PR into `v1`. `main` is Design B from Task 1 on. Reason: spec 7.5 assumes the old binary stays at the hooks' path until the cut-over, and today the owner's binary is rebuilt from `main`.

**D2. Two files beside v1's.** `raw.db` (synchronous=FULL; `fullfsync` on macOS) and `knowledge.db` (synchronous=NORMAL) live in the oboete home next to v1's `oboete.db`, which Design B never opens before milestone 4's import (spec 7.5: "The new version never writes the old store"). Both use WAL and a 2 s busy timeout (today's `src/db.rs:103`).

**D3. macOS keeps fullfsync.** Spike 1 measured the iMac at 23-27 ms p95 with `F_FULLFSYNC` even at 1 KB, and 3-13 ms without it (docs/spike/hook-m14.md:65). FULL exists to close a durability hole; dropping fullfsync would reopen it on Apple SSDs. So the write-hook line is set from measurement with fullfsync on (Task 12), expected near 25-27 ms instead of the provisional 20 ms. A hook of that length is not felt at a prompt.

**D4. Oversized outputs: a head-and-tail cap set by measurement.** `capture.max_output_bytes` is a config constant. Task 12 sets its default: the largest of 64, 128 and 256 KB at which the slowest machine's hook p95 stays within the line (spec 2.2; hook-m14.md:70 found Windows at 41 ms p95 for 256 KB). Above the cap, the stored text keeps the first and last half of the cap, with a marker giving the original size. The whole output is scanned before the cut (spec 2.2: "redacted in full"): a rule can need context far from its secret (curl-auth-user reads a whole line), so a margin around each cut is not enough (Codex security review of #88). A secret across a cut is kept whole in its mask.

**D5. Compression in the worker.** The hook stores records uncompressed: Spike 1 found zstd at write time pays only at 256 KB and only on Windows (hook-m14.md:68-69). The worker's compression consumer rewrites a record's body as zstd once its other consumers have passed it (`enc` column: `plain` or `zstd`). A zstd dictionary is decided after a week of dogfooding, as spec 2.4 says (A9).

**D6. A minimal worker now; its lifecycle verdict at milestone 4.** `oboete worker` is started by hooks (detached, as `observe` is today), holds a lock file so one runs per home, runs the consumers of this milestone in order, and exits when idle. Every hook that appends then tries the lock and starts a worker only if it is free. So the worker releases the lock first and then checks once more for records above its checkpoints; if there are any, it takes the lock again and goes on (or leaves them to the worker that took it). Either the worker's last check sees the event, or the hook's lock attempt comes after the release and starts a new worker: a wake-up is never lost (spec 1.1, issue #55). A last check made while still holding the lock would lose an event appended between that check and the release. Consumers are library functions, the same code in either process model (spec 8.4), so milestone 4's comparison can change only the wrapper.

**D7. Raw FTS in knowledge.db.** An FTS5 trigram table over each event's text (prompt, reply, tool input and output), keyed by (device, seq), built by the worker from its checkpoint with today's tokenizer and ranking (`src/search.rs`). A tombstoned target is never indexed and is filtered at query time. The none tier's search at this milestone is the CLI (`oboete search`); MCP and the viewer follow at milestone 4 (spec 8.4 row 4).

**D8. Tombstones from day one; physical removal at milestone 5.** Tombstones are records in the same (device, seq) sequence, with no body. A tombstone targets a record (device, seq) or a byte range in one (device, seq, offset, length). From this milestone every read path (FTS indexing, search, manifest, backups) gets the targets hidden or masked, in one place: `Raw::after`, the only way records leave `raw.rs`. A range is masked by as many bytes of `*` as it covers (every character it touches, whole), so offsets stay valid and masking twice changes nothing: a backup of a masked body restores with its tombstone and reads the same. Rewriting `raw.db` without them is milestone 5's forget pipeline (spec 6.2 step 2). This milestone writes tombstones from one source only: the redaction rescan (spec 2.2). v1's hard deletes (`delete_doc`, `delete_session`) are not carried into Design B; its deletions go through forget.

**D9. Git fields read from files in the hook.** The hook reads branch, HEAD SHA and the worktree gitdir from `.git` files (`HEAD`, the ref file or `packed-refs`, a worktree's `gitdir`) without starting `git`. Risky git state for the manifest (uncommitted changes, a rebase or merge in progress, detached HEAD) is computed by the worker with `git status --porcelain=v2 --branch`, never in the hook.

**D10. Checkpoints are seqs in knowledge.db.** Each consumer's checkpoint is the highest seq of this device it has finished. On start, the worker rewinds any checkpoint above raw's highest seq for this device and reports it in doctor (spec 2.5, MUST-M14). A consumer moves its checkpoint in the same knowledge.db transaction as its output.

**D11. The backup interval, defined.** The spec sets the interval by measurement but names no measurement (Appendix C item 7). Definition: backups run at every idle exit that has new seqs, and every 30 minutes while the worker runs (`next_attempt_at`). A run exports segments of at most 8 MB of records each and repeats until it is caught up, so the time of one export does not grow with the interval or with activity. Task 12 measures the time per segment and its compressed size on the three machines; if the slowest machine's p95 per segment exceeds 1 s, the segment cap halves until it does not. The interval stays 30 minutes, so MUST-M15's "loses at most one idle interval" holds by construction.

**D12. The manifest at the none tier.** From raw alone the manifest has: risky git state; last failing command; the owner's directive lines (dated, quoted, marked unverified) with MUST-M5's negation pairing; the agent's todo list; last prompt and reply; files touched; as-of and the count of records no consumer of curation has seen (all of them until milestone 3); other active sessions on the repo. "Current decisions" is empty until milestone 3's claims. The drop order is spec 4.9's. SessionStart injects the manifest, fenced as data; packets and ranked claims are milestone 4 (spec 4.4).

**D13. MUST-M5's negation rule.** Fixed lists in code, with tests: directive markers (今後は, これからは, 必ず, 常に, from now on, always, never, don't) and negation markers (やめて, 取り消し, 撤回, もういい, never mind, not anymore, cancel that, ignore what I said). A negation line hides an earlier directive line of the same session or repo when they share at least one content word (after dropping stop words; trigram overlap for Japanese). Only the latest state is shown.

**D14. M14 on three machines.** WSL and the M1 iMac (SSH) run the release build. Windows native runs the `x86_64-pc-windows-gnu` cross-build again, as in Spike 1; the MSVC artifact is unmeasured until CI builds it (hook-m14.md:14), and the note says so. The line is set from the slowest machine at each size after D4's cap.

**D15. Obligations carried from milestone 1.** Replay stamps each event with the fixture's `ts`, not now (issue #65, last row). Teammate messages (`Another Claude session sent a message: <teammate-message ...>`) are an envelope, not a typed prompt (docs/milestone-1.md:139).

**D16. Settings read at this milestone.** Capture detail (whether prompts are stored; tool output full or head-and-tail), the backup location, and extra redaction rules with an allowlist (spec 1.5). Capture exclusion and the retention period come with milestone 5's forget and exclusion (spec 7.5).

---

## Tasks

| # | Task | Executor | Depends on | Produces |
|---|---|---|---|---|
| 0 | `v1` branch; Design B's setup in the dogfood user | Claude | — | branch `v1`; `oboete setup` registers B's hooks in a temp or dogfood home |
| 1 | `raw.db`: schema, open (FULL, fullfsync), append with seq | Claude | 0 | `raw::open`, `Raw::{append, max_seq, after}`, `Event`, `Record`, `Item`, `Target` |
| 2 | Capture: full events, envelopes, binary markers, `<private>`, git fields | Claude; adapter ports by Grok | 1 | `capture::events(agent, event, &Value, ts) -> Vec<Event>`; `hook::record` |
| 3 | Redaction: full scan, head-and-tail cap, ledger, extra rules and allowlist; security review | Claude | 2 | `redact::{Rules, Finding, scan}`; `capture::cut_and_redact`; ledger rows |
| 4 | Write-failure classification, marker, doctor, injection line (MUST-M16) | Claude | 2 | `failure::{classify, mark, since}` |
| 5 | Worker: lock, spawn, idle exit, lost-wakeup rule, checkpoints, rewind (MUST-M14) | Claude | 1 | `worker::{lock, drain, run_with, run, run_once}`, `checkpoint::{get, set, rewind}` |
| 6 | Raw FTS consumer and `oboete search` | Claude | 5 | `consumer::Fts`, `search::raw` |
| 7 | Tombstones: records, read-path filtering, redaction rescan | Claude | 3, 5, 6 | `Raw::append_tombstone`, masking inside `Raw::after`, `consumer::Rescan` |
| 8 | Backups: segments, checksums, schedule, cloud-folder warning, quarantine and restore (MUST-M15) | Claude | 5, 7 | `backup::{export, verify, restore}` |
| 9 | Manifest and SessionStart injection; MUST-M5 negation | Claude | 5, 6 | `consumer::manifest`, `manifest::render` |
| 10 | Compression consumer | Claude | 5 | `consumer::compress` |
| 11 | Transcript gap check (spec 2.3) | Claude | 5 | `consumer::gaps`, doctor rows per adapter |
| 12 | Replay on Design B and M14 on three machines; set the line, the cap and the backup segment cap | Claude | 1-11 | numbers in `docs/milestone-2.md` |
| 13 | None tier end to end in the dogfood user; the milestone note | Claude | 12 | `docs/milestone-2.md` |

Task bodies follow, one per section, each with its interfaces and its failing test first.

---

## Task 0: `v1` branch and Design B in the dogfood user

**Files:**
- Create: `docs/milestone-2.md` (the milestone note; its first section records the `v1` branch point)
- Modify: `CLAUDE.md` ("Scope" line: where v1 fixes go)
- Modify: `src/main.rs` (`--home` default for Design B is unchanged; nothing else)

**Interfaces:**
- Produces: branch `v1`; the rule "the owner's binary is built from `v1` until the cut-over".

- [ ] **Step 1: Find the commit the owner runs.** `oboete --version` prints only `0.1.0`, so check instead that `src/`, `Cargo.toml` and `Cargo.lock` are unchanged between the commit of the owner's last install (b95b827, #71, 2026-09-26) and `main`: `git diff --stat b95b827 main -- src Cargo.toml Cargo.lock` prints nothing. If it prints something, branch from b95b827 instead.
- [ ] **Step 2: Create the branch.** `git branch v1 main && git push origin v1` (or from b95b827, per Step 1). Protect it with the same ruleset as `main` (check + CodeQL, threads resolved).
- [ ] **Step 3: Write the rule down.** `CLAUDE.md` Scope line: "Until the cut-over, the owner's binary is built from `v1`: `cargo install --path . --locked` on a `v1` checkout. A v1 fix (real loss or safety only) is a PR into `v1`; `main` is Design B." The milestone note's first section records the branch point and the date.
- [ ] **Step 4: Design B in the dogfood user.** In the dogfood user: build `main`, `oboete --home ~/.oboete-b setup --agent claude` (and codex), so its hooks call Design B with a home of its own. Check that `~/.oboete` of the owner is untouched (`stat` before and after).
- [ ] **Step 5: Commit** `docs: v1 branch for the owner's binary; Design B from here (milestone 2, Task 0)`.

---

## Task 1: `raw.db`

**Files:**
- Create: `src/raw.rs`
- Modify: `src/main.rs` (`mod raw;`)
- Modify: `Cargo.toml` (`[dev-dependencies] tempfile = "3"`: a temporary home that is removed even when a test panics; today's tests leave `temp_dir()` folders behind)
- Test: `src/raw.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `db::private` (file modes), `db::ensure_device`'s device-id rule (moved into `raw.rs`: the id belongs to `raw.db` now).
- Produces:
  - `pub struct Raw { conn: Connection, device: String }`
  - `pub fn open(home: &Path) -> Result<Raw>`: WAL, `synchronous=FULL`, `fullfsync=ON` on macOS, busy timeout 2 s, schema, device id.
  - `pub struct Event { pub agent: String, pub session: String, pub kind: String /* prompt, tool, reply, compaction, end */, pub ts: i64, pub repo: Option<String>, pub branch: Option<String>, pub head: Option<String>, pub gitdir: Option<String>, pub cwd: Option<String>, pub source: String, pub body: String, pub original_bytes: Option<i64> }`
  - `pub enum Target { Record { device: String, seq: i64 }, Range { device: String, seq: i64, offset: i64, length: i64 } }`: what a tombstone points at. The columns exist from this task; Task 7 writes them.
  - `pub struct Record { pub device: String, pub seq: i64, pub item: Item }` with `pub enum Item { Event(Event), Removed /* an event a tombstone targets whole */, Tombstone(Target) }`. `Raw::after` is the only way records leave `raw.rs`: bodies come back decompressed (Task 10) and masked by every tombstone (Task 7, D8). The stored bytes stay private to `raw.rs`, so no consumer can read what a tombstone covers.
  - `impl Raw { pub fn append(&mut self, e: &Event) -> Result<i64> /* seq */; pub fn max_seq(&self) -> Result<i64>; pub fn after(&self, device: &str, seq: i64, limit: usize) -> Result<Vec<Record>>; pub fn device(&self) -> &str }`
  - `#[cfg(test)] pub fn test_event(body: &str) -> Event` (agent `claude`, session `s`, kind `prompt`, source `hook`), used by every later task's tests.

Schema (`raw.db`):

```sql
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- One sequence per device holds events and tombstones (spec 1.6, 5.8).
CREATE TABLE IF NOT EXISTS records (
  device TEXT NOT NULL,
  seq INTEGER NOT NULL,
  type TEXT NOT NULL,          -- 'event' or 'tombstone'
  ts INTEGER NOT NULL,         -- unix ms, the event's own time (replay: the fixture's)
  kind TEXT,                   -- Event.kind; NULL for a tombstone
  agent TEXT, session TEXT,    -- labels, never keys
  repo TEXT, branch TEXT, head TEXT, gitdir TEXT, cwd TEXT,
  source TEXT NOT NULL,        -- 'hook'; later 'oboete-v1', 'transcript'
  enc TEXT NOT NULL DEFAULT 'plain',             -- 'plain' or 'zstd' (D5)
  body BLOB,                   -- NULL for a tombstone
  original_bytes INTEGER,      -- set when head-and-tail cut the body (D4)
  target_device TEXT, target_seq INTEGER, target_offset INTEGER, target_length INTEGER,
  PRIMARY KEY (device, seq)
);  -- a rowid table: bodies up to the cap are far above the row size WITHOUT ROWID suits (under 1/20 of a page, sqlite.org/withoutrowid.html)
CREATE TABLE IF NOT EXISTS ledger (   -- spec 2.2: never the value
  device TEXT NOT NULL, seq INTEGER NOT NULL, rule TEXT NOT NULL,
  offset INTEGER NOT NULL, length INTEGER NOT NULL, ts INTEGER NOT NULL, ruleset TEXT NOT NULL
);
```

No index on session, repo or branch: a schema review should find none (spec 1.6).

- [ ] **Step 1: Write the failing tests.**

```rust
#[test]
fn two_writers_get_consecutive_seqs_and_both_land() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();                      // a &Path each thread can copy
    raw::open(p).unwrap();                    // create the file first
    let seqs: Vec<i64> = std::thread::scope(|s| {
        let h: Vec<_> = (0..2).map(|i| s.spawn(move || {
            let mut r = raw::open(p).unwrap();
            (0..50).map(|n| r.append(&test_event(&format!("{i}-{n}"))).unwrap()).collect::<Vec<_>>()
        })).collect();
        h.into_iter().flat_map(|t| t.join().unwrap()).collect()
    });
    let mut sorted = seqs.clone();
    sorted.sort();
    assert_eq!(sorted, (1..=100).collect::<Vec<_>>());
}

#[test]
fn raw_db_is_full_and_has_no_session_index() {
    let home = tempfile::tempdir().unwrap();
    let r = raw::open(home.path()).unwrap();
    let sync: i64 = r.conn.query_row("PRAGMA synchronous", [], |x| x.get(0)).unwrap();
    assert_eq!(sync, 2);                      // FULL
    let idx: i64 = r.conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='index' AND sql LIKE '%session%'", [], |x| x.get(0)).unwrap();
    assert_eq!(idx, 0);
}

#[test]
fn device_id_stays_with_the_file_and_changes_on_a_copy() { /* port of the db.rs test of the same name onto raw.db, unchanged but for the file */ }
```

- [ ] **Step 2: Run them, expect failure** (`cargo test raw::`): `raw` does not exist.
- [ ] **Step 3: Implement.** `open` follows `db::open` (the WAL retry loop included) with `PRAGMA synchronous=FULL` and, under `#[cfg(target_os = "macos")]`, `PRAGMA fullfsync=ON`. `append` runs `BEGIN IMMEDIATE`, reads `COALESCE(MAX(seq),0)+1` for this device, inserts, commits: the write lock makes the read-then-insert atomic across processes.
- [ ] **Step 4: Run the tests, expect pass.** Also `cargo clippy -- -D warnings`.
- [ ] **Step 5: Commit** `raw: raw.db keyed by (device, seq), synchronous=FULL (milestone 2, Task 1)`.

---

## Task 2: Capture

**Files:**
- Create: `src/capture.rs`
- Modify: `src/hook.rs` (`hook::record`: `capture::events` then `Raw::append`; `run_io` sends the agents in `capture::PORTED` there and the others to v1's `handle` until their port lands, 2b; a ported agent's SessionStart prints nothing until Task 9)
- Modify: `src/replay.rs` (ported agents replay into `raw.db`), `src/transcript.rs` (its replay test reads `raw.db`), `src/repo.rs` (`gitdir`, `common_dir` shared with `capture::git`)
- Test: `src/capture.rs`, `src/hook.rs`

**Interfaces:**
- Consumes: `raw::Event`; today's per-agent parsing in `hook::handle` (`src/hook.rs:204-460`), `STRIP_BLOCKS` and `strip_blocks` (`src/hook.rs:25, 474-536`), `is_envelope` (`src/hook.rs:539`), `repo::key` (`src/repo.rs:9-58`).
- Produces: `pub fn events(agent: &str, event: &str, payload: &serde_json::Value, ts: i64) -> Vec<raw::Event>` (a hook call can carry several: cursor's SessionEnd recovers turns, agy's hooks read a transcript tail); `pub const PORTED: &[&str]`; `pub fn git(cwd: &Path) -> Git { branch, head, gitdir }`; `hook::record(&mut Raw, agent, event, &Value)`.

Rules (spec 2.1-2.4, D9, D15):
- Kinds: prompt, tool call with its output, assistant reply, compaction, end. Text fields are kept whole (no `MAX_FIELD` clip); D4's cap is Task 3's.
- `<private>` blocks go before anything else; an unclosed one in a typed prompt hides the rest (kept from today).
- Envelopes are not typed prompts: today's list plus the teammate message (`Another Claude session sent a message:`).
- Images and binary content become `{kind, mime, bytes, sha256}`.
- The curator's sessions are skipped (`SKIP_ENV`, the environment marker set on curator CLIs, `src/hook.rs:20`).
- Repo labels keep today's rules and tests: one key for every way to clone, no userinfo (`one_key_for_every_way_to_clone_and_no_secrets`, #30 row 3); an alias only from the owner's settings (#30 row 4); the repos a session touches are recorded from tool working directories and file paths (`sessions_record_every_repo_and_idless_events_stay_on_this_device`, #30 rows 5 and 20), as a `session_repos` table in `knowledge.db` kept by the FTS consumer (Task 6).
- Git fields from files (D9): `.git` may be a file (`gitdir: …`) in a worktree.

- [ ] **Step 1: Failing tests** (in `src/capture.rs`): a 100,000-character tool output is kept whole and a token past today's 12,000-character window is masked; `<private>` blocks go and an unclosed one hides the rest; envelopes, the teammate message among them, are recorded as `envelope`, never `prompt`; images become markers in each shape agents send; replies come from the payload or Codex's transcript; git fields come from files in a main checkout, a linked worktree, after `pack-refs`, and detached.
- [ ] **Step 2: Run, expect failure.**
- [ ] **Step 3: Implement** in two parts. **2a** (Claude): `capture.rs` with the Claude Code and Codex paths, redacting each text field in full with today's `redact::redact` until Task 3 brings `scan` and the ledger. **2b**: the agy, cursor, grok, opencode and pi ports, then v1's write path in `hook::handle` is deleted. 2b needs per-session hook state that 2a does not (inject once, agy's prompt-step claims, cursor's recovered-turn count). Neither store fits it as is: `knowledge.db` is rebuilt from raw, and raw has no session key (spec 1.6). So 2b starts by settling where that state lives, with Task 9's injection (spec 4.2, 4.4). After that the ports are independent pieces for Grok, with the `hook.rs` tests as acceptance (spec 8.4, "Who builds").
- [ ] **Step 4: Run all tests.**
- [ ] **Step 5: Commit** `capture: full events, envelopes, binary markers and git fields (milestone 2, Task 2)`.

---

## Task 3: Redaction (security-relevant)

**Files:**
- Modify: `src/redact.rs`
- Modify: `src/capture.rs` (call the scan, apply D4's cap)
- Modify: `src/config.rs` (`[redaction] extra_rules`, `allowlist`; `[capture] max_output_bytes`, `store_prompts`, `tool_output = "full" | "head-tail"`)
- Test: `src/redact.rs`, `src/capture.rs`

**Interfaces** (in two PRs: **3a** the full scan, the ledger and the cap on the built-in rules; **3b** the settings):
- Produces (3a):
  - `pub struct Finding { pub rule: String, pub offset: usize, pub length: usize }`: the rule, where its mask starts in the stored text, and the secret's own length, in bytes; never the value. Two rules on one token share one mask and give two findings.
  - `pub fn scan(text: &str) -> (String, Vec<Finding>)`; `redact(text)` stays, as `scan(text).0`, for v1's `clip` and `outbound`.
  - `pub fn scan_capped(text: &str, cap: usize) -> (String, Vec<Finding>, Option<usize>)`: the whole text scanned, then head and tail kept above `cap` (D4); a key block cut in half (any case) is dropped from the part holding it; then a second pass over what is kept; the third value is the full size when it was cut. `redact::ruleset()` names the rules' version.
  - `capture::Captured { event, ledger: Vec<(String, Finding)> }` and `capture::MAX_FIELD_BYTES` (256 KB until Task 12): every stored string passes `capture::Gate`, which applies `scan_capped` and records each finding with its field (a JSON pointer into the body, `#key` for an object's key, or a label column).
  - `raw::Raw::append_with_ledger(&mut self, e: &Event, ledger: &[(String, Finding)]) -> Result<i64>`: the event and its ledger rows in one transaction; the ledger gains a `field` column.
- Produces (3b): `Rules` with the settings' extra rules and allowlist (`[redaction] extra_rules`, `allowlist`), its version in the ledger, `outbound` using them too; `[capture] store_prompts` and `tool_output = "full" | "head-tail"`.

- [ ] **Step 1: Failing tests.**

```rust
#[test]
fn every_byte_is_scanned_and_the_ledger_never_holds_the_value() {
    let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g"); // a low-entropy token is not a secret to the rule; split so secret scanners pass it
    let text = "x".repeat(200_000) + &key;          // far past today's 12,000-character window
    let (masked, found) = redact::scan(&text, &Rules::default());
    assert!(!masked.contains(&key));
    assert_eq!(found.len(), 1);
    assert!(!format!("{found:?}").contains(&key));
}

#[test]
fn a_secret_across_the_head_tail_cut_is_masked_in_what_is_kept() {
    let key = format!("ghp_{}", "q9Zx8mL2vB4nR7tY1wK3pS6dJ0aF5hU2cE8g"); // a low-entropy token is not a secret to the rule; split so secret scanners pass it
    let cap = 64 * 1024;
    let text = "y".repeat(cap / 2 - 10) + &key + &"z".repeat(cap * 2);
    let e = capture::cut_and_redact(&text, cap, &Rules::default());
    for i in 8..key.len() {                            // no fragment of 8+ characters survives
        assert!(!e.body.contains(&key[i - 8..i]), "fragment at {i}");
    }
    assert!(e.original_bytes == Some(text.len() as i64));
}

#[test]
fn an_allowlisted_false_positive_is_kept_and_an_extra_rule_masks() { /* config-driven */ }
```

- [ ] **Step 2: Run, expect failure.**
- [ ] **Step 3: Implement.** `scan` runs today's pipeline over the whole text (keyword gate, regex, entropy, allowlist, merge, mask) and returns the merged spans as findings. `scan_capped` scans the whole text, then keeps the cap's halves around a marker `…[cut: N bytes in full]…` (D4).
- [ ] **Step 4: Run the tests; then the security review.** rules/security.md: the `security-audit-skill`, `semgrep scan` on `src/redact.rs src/capture.rs`, and `/codex-review mode=security`. Record the review's outcome in the PR.
- [ ] **Step 5: Commit** `redact: full scan, ledger, head-and-tail cap, extra rules and allowlist (milestone 2, Task 3)`.

---

## Task 4: Write failures (MUST-M16)

**Files:**
- Create: `src/failure.rs`
- Modify: `src/main.rs` (the `Cmd::Hook` arm: classify and mark instead of `eprintln!` only, `src/main.rs:189-194`)
- Modify: `src/setup.rs` (`doctor`: red on a marker or low free space)
- Modify: `src/hook.rs` (SessionStart prints the "recording has failed since" line; Task 9 adds the manifest after it)
- Test: `src/failure.rs`; `tests/disk_full.rs` (Linux only)

**Interfaces:**
- Produces: `pub enum Class { DiskFull, Io, Busy, Other }`; `pub fn classify(e: &anyhow::Error) -> Class` (SQLITE_FULL / ENOSPC, SQLITE_IOERR, SQLITE_BUSY; A10); `pub fn prepare(home: &Path)` (after a successful append: creates `<home>/state/recording-failed` at a fixed 64 bytes if it is missing); `pub fn mark(home: &Path, class: Class)` (rewrites that file in place with the class and the first failure's time, never overwriting that time: an in-place write needs no new disk blocks, so it works on a full disk; on copy-on-write filesystems such as APFS or btrfs it can still fail, and doctor's free-space check covers that); `pub fn since(home: &Path) -> Option<(Class, i64)>`; `pub fn clear(home: &Path)` (after a successful append, in place).

- [ ] **Step 1: Failing tests.** Unit tests for `classify` on constructed `rusqlite::Error`s (`SQLITE_FULL`, `SQLITE_IOERR`, `SQLITE_BUSY`, an `io::Error` with `ENOSPC`), and the MUST-M16 fixture:

```rust
//! tests/disk_full.rs. MUST-M16: a full disk never blocks the agent; doctor and the next
//! SessionStart say recording has failed.
#[cfg(target_os = "linux")]
#[test]
fn a_full_disk_never_blocks_the_agent_and_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let h = dir.path().join("home");
    std::fs::create_dir(&h).unwrap();
    let b = env!("CARGO_BIN_EXE_oboete");
    // In a private mount namespace a 1 MiB tmpfs becomes the home. One hook creates raw.db and the
    // marker, `dd` takes the rest of the space, then one more hook, doctor and a new SessionStart.
    let script = format!(r#"
        mount -t tmpfs -o size=1m tmpfs {h} || exit 77
        echo '{{"session_id":"s","prompt":"first"}}' | {b} --home {h} hook claude UserPromptSubmit
        dd if=/dev/zero of={h}/fill bs=4k 2>/dev/null
        echo '{{"session_id":"s","prompt":"second"}}' | {b} --home {h} hook claude UserPromptSubmit; echo "hook=$?"
        {b} --home {h} doctor; echo "doctor=$?"
        echo '{{"session_id":"t","source":"startup"}}' | {b} --home {h} hook claude SessionStart
    "#, h = h.display());
    let out = std::process::Command::new("unshare").args(["-rm", "sh", "-c", &script]).output().unwrap();
    if out.status.code() == Some(77) || String::from_utf8_lossy(&out.stderr).contains("unshare failed") {
        eprintln!("skipped: no unprivileged tmpfs mount on this kernel");
        return;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains("hook=0"), "{s}");                                  // the agent is not blocked
    assert!(!s.contains("doctor=0"), "{s}");                               // doctor is red
    assert_eq!(s.matches("recording has failed since").count(), 2, "{s}"); // doctor and SessionStart
}
```
- [ ] **Step 2: Run, expect failure.**
- [ ] **Step 3: Implement.** The hook's error path calls `mark`; the marker lives outside the database, so it can be written when the database cannot. If the marker cannot be written either, doctor's own free-space check still turns red.
- [ ] **Step 4: Run the tests.**
- [ ] **Step 5: Commit** `failure: classify write failures, mark them outside the database (milestone 2, Task 4)`.

---

## Task 5: The worker, checkpoints and rewind (MUST-M14)

**Files:**
- Create: `src/worker.rs`, `src/knowledge.rs`
- Modify: `src/hook.rs` (after every successful append, and at SessionStart, try the worker lock and spawn `oboete worker` detached when it is free, in place of `spawn_observe`, `src/hook.rs:680-700`)
- Modify: `src/main.rs` (`Cmd::Worker { idle_ms }`)
- Test: `src/worker.rs`

**Interfaces:**
- Consumes: `raw::open`, `Raw::max_seq`, `Raw::after(device, seq, limit) -> Result<Vec<Record>>`, `raw::test_event`.
- Produces:
  - `knowledge::open(home) -> Result<Connection>`: WAL, `synchronous=NORMAL`, table `checkpoints(consumer TEXT, device TEXT, seq INTEGER NOT NULL, PRIMARY KEY (consumer, device))`.
  - `checkpoint::get(k: &Connection, consumer: &str, device: &str) -> Result<i64>` (0 when absent); `checkpoint::set(k, consumer, device, seq) -> Result<()>`.
  - `pub trait Consumer { fn name(&self) -> &'static str; fn step(&mut self, raw: &Raw, k: &Connection, after: i64) -> Result<i64> /* new checkpoint */; fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()> /* delete this consumer's output above `to` */; }`
  - `checkpoint::rewind(raw: &Raw, k: &Connection, consumers: &mut [Box<dyn Consumer>]) -> Result<Vec<(String, i64, i64)>>` (consumer, was, now) for this device: for each checkpoint above raw's highest seq, the consumer's `rewind` and the checkpoint move share one transaction, so no output of a lost seq survives to collide with the event that reuses it. Also written to `<home>/state/rewound` for doctor.
  - `worker::consumers() -> Vec<Box<dyn Consumer>>`: this milestone's consumers in order. It starts empty; each later task adds its own.
  - `worker::drain(raw: &Raw, k: &mut Connection, consumers: &mut [Box<dyn Consumer>]) -> Result<()>`: runs each consumer from its checkpoint until none advances. Each step and its checkpoint move share one knowledge.db transaction (D10).
  - `worker::lock(home) -> Result<Option<Lock>>`: std's `File::try_lock` on `<home>/state/worker.lock`; `None` when another process holds it. The hook uses it too.
  - `worker::run_with(home, idle_ms, consumers, before_exit: impl FnMut()) -> Result<()>`: take the lock (return at once without it), `quick_check` both files (a failure stops the worker with an error until Task 8), `checkpoint::rewind`, `drain`; wait for new records or `idle_ms`; when idle, release the lock, call `before_exit` (a test seam), check for records above the checkpoints, and if there are any, take the lock again and go on (D6).
  - `worker::run(home, idle_ms)` is `run_with(home, idle_ms, consumers(), || {})`; `worker::run_once(home)` is the same with `idle_ms = 0`.

- [ ] **Step 1: Failing tests.**

```rust
/// A consumer that writes each seq it sees into knowledge.db, so these tests need no index (Task 6).
struct Seen;
impl Consumer for Seen {
    fn name(&self) -> &'static str { "seen" }
    fn step(&mut self, raw: &Raw, k: &Connection, after: i64) -> Result<i64> {
        k.execute("CREATE TABLE IF NOT EXISTS seen(device TEXT, seq INTEGER)", [])?;
        let recs = raw.after(raw.device(), after, 100)?;
        for r in &recs { k.execute("INSERT INTO seen VALUES (?1, ?2)", (&r.device, r.seq))?; }
        Ok(recs.last().map_or(after, |r| r.seq))
    }
    fn rewind(&mut self, k: &Connection, device: &str, to: i64) -> Result<()> {
        k.execute("DELETE FROM seen WHERE device = ?1 AND seq > ?2", (device, to))?;
        Ok(())
    }
}

fn seen(k: &Connection) -> Vec<i64> {
    let mut st = k.prepare("SELECT seq FROM seen ORDER BY seq").unwrap();
    st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

/// raw.db as if its commits above `seq` had never reached the disk (MUST-M14).
fn lose_after(home: &Path, seq: i64) -> Raw {
    let c = rusqlite::Connection::open(home.join("raw.db")).unwrap();
    c.execute("DELETE FROM records WHERE seq > ?1", [seq]).unwrap();
    drop(c);
    raw::open(home).unwrap()
}

#[test]
fn a_checkpoint_above_raw_is_rewound_with_its_output_and_later_events_are_not_skipped() {
    let home = tempfile::tempdir().unwrap();
    let mut raw = raw::open(home.path()).unwrap();
    let mut k = knowledge::open(home.path()).unwrap();
    let mut consumers: Vec<Box<dyn Consumer>> = vec![Box::new(Seen)];
    for i in 0..8 { raw.append(&raw::test_event(&i.to_string())).unwrap(); }
    worker::drain(&raw, &mut k, &mut consumers).unwrap();       // output and checkpoint at 8
    drop(raw);
    let mut raw = lose_after(home.path(), 5);
    assert_eq!(checkpoint::rewind(&raw, &k, &mut consumers).unwrap(), vec![("seen".into(), 8, 5)]);
    assert_eq!(seen(&k), vec![1, 2, 3, 4, 5]);                   // no output left for the lost 6-8
    raw.append(&raw::test_event("new")).unwrap();                // seq 6 again, a different event
    worker::drain(&raw, &mut k, &mut consumers).unwrap();
    assert_eq!(seen(&k), vec![1, 2, 3, 4, 5, 6]);
}

#[test]
fn an_event_that_arrives_while_the_worker_decides_to_exit_is_still_processed() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    raw::open(p).unwrap().append(&raw::test_event("first")).unwrap();
    let mut late = true;
    // `before_exit` runs after the lock is released and before the last check: a hook appending
    // here finds the lock free, but this test starts no second worker, so only the last check
    // can pick the event up.
    worker::run_with(p, 0, vec![Box::new(Seen)], || {
        if std::mem::take(&mut late) {
            raw::open(p).unwrap().append(&raw::test_event("late")).unwrap();
        }
    }).unwrap();
    let device = raw::open(p).unwrap().device().to_owned();
    assert_eq!(checkpoint::get(&knowledge::open(p).unwrap(), "seen", &device).unwrap(), 2);
}

#[test]
fn a_second_worker_exits_at_once() {
    let home = tempfile::tempdir().unwrap();
    let _held = worker::lock(home.path()).unwrap().expect("the first lock");
    assert!(worker::lock(home.path()).unwrap().is_none());
    let t = std::time::Instant::now();
    worker::run(home.path(), 60_000).unwrap();                   // returns at once: the lock is held
    assert!(t.elapsed() < std::time::Duration::from_secs(1));
}
```

- [ ] **Step 2: Run, expect failure.**
- [ ] **Step 3: Implement.** No async runtime: a loop with `std::thread::sleep` on a short poll plus the idle deadline. Every hook that appends then calls `worker::lock` and spawns a worker only when it gets the lock (dropping it at once), so D6's order holds for every event, not only at Stop. While a worker runs (it stays up `idle_ms` after the last record), the attempt fails and costs one open and one `flock`; a process is spawned only when no worker runs. Task 12's M14 runs include this.
- [ ] **Step 4: Run the tests.**
- [ ] **Step 5: Commit** `worker: consumers by seq, checkpoints, rewind, lost-wakeup rule (milestone 2, Task 5)`.

---

## Task 6: Raw FTS and `oboete search`

**Files:**
- Create: `src/consumer/fts.rs`
- Modify: `src/search.rs` (a `raw` entry point reusing the trigram query builder, bm25 and the short-query `LIKE` fallback, `src/search.rs:44-90, 167-225`)
- Modify: `src/main.rs` (`oboete search` reads Design B's index)
- Test: `src/search.rs`

**Interfaces:**
- Produces: `knowledge.db` table `raw_fts USING fts5(text, tokenize='trigram')` with `raw_docs(rowid INTEGER PRIMARY KEY, device, seq, kind, ts, repo, session)`; `consumer::Fts`; `search::raw(home, query) -> Result<Vec<RawHit>>` with `pub struct RawHit { device, seq, kind, ts, repo, snippet }` (today's `search::Hit` stays v1's) (this repository unless `--all`, as today).

- [ ] **Step 1: Failing tests:** an event is found by a Japanese two-character query and by an English word; a tombstoned record is not found (after Task 7, the test is added there); results carry (device, seq).
- [ ] **Step 2: Run, expect failure.**
- [ ] **Step 3: Implement** the consumer (index each event's text, move the checkpoint in the same transaction) and the query.
- [ ] **Step 4: Run the tests.**
- [ ] **Step 5: Commit** `search: raw full-text index and search for the none tier (milestone 2, Task 6)`.

---

## Task 7: Tombstones and the redaction rescan

**Files:**
- Modify: `src/raw.rs` (`append_tombstone`), `src/search.rs`, `src/consumer/fts.rs`
- Create: `src/consumer/rescan.rs`
- Test: those files

**Interfaces:**
- Consumes: `Target` and `Record` (Task 1), `worker::run_once` (Task 5), `search::raw` (Task 6).
- Produces: `Raw::append_tombstone(&mut self, t: Target) -> Result<i64>`; masking inside `Raw::after` (D8: the targets are loaded once per call; a range becomes the same number of bytes of `*`; an event targeted whole comes back as `Item::Removed`); `consumer::Rescan`: when `Rules::version()` differs from the one stored in `knowledge.db`, scan records with the new rules from seq 1 and append a range tombstone per new hit (spec 2.2), then store the new ruleset.

- [ ] **Step 1: Failing tests.** The manifest (Task 9) and backup export (Task 8) read through `Raw::after` and add their own assertion on this fixture. Here:

```rust
#[test]
fn a_record_tombstone_hides_it_and_a_range_tombstone_masks_only_its_range() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = raw::open(p).unwrap();
    let a = raw.append(&raw::test_event("alpha zqx-private-words tail")).unwrap();
    let b = raw.append(&raw::test_event("bravo visible")).unwrap();
    let dev = raw.device().to_owned();
    raw.append_tombstone(Target::Record { device: dev.clone(), seq: b }).unwrap();
    raw.append_tombstone(Target::Range { device: dev.clone(), seq: a, offset: 6, length: 17 }).unwrap();
    worker::run_once(p).unwrap();
    assert!(search::raw(p, "bravo").unwrap().is_empty());
    assert!(search::raw(p, "zqx-private").unwrap().is_empty());
    let hits = search::raw(p, "alpha").unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].snippet.contains("tail") && !hits[0].snippet.contains("zqx"));
    let recs = raw.after(&dev, 0, 10).unwrap();
    assert!(matches!(&recs[0].item, Item::Event(e) if e.body == "alpha ***************** tail"));
    assert!(matches!(recs[1].item, Item::Removed));
}

#[test]
fn a_new_rule_tombstones_old_records_once() {
    // An extra rule added in the settings: Rescan appends one range tombstone per new finding in
    // old records, none for spans already masked at capture, and none on a second run.
}
```
- [ ] **Step 2: Run, expect failure.**
- [ ] **Step 3: Implement.** `Raw::after` masks before any text leaves `raw.rs` (D8), so a later read path cannot forget it. The rescan scans the masked bodies; its offsets stay valid on the stored body because masking keeps byte lengths.
- [ ] **Step 4: Run the tests.**
- [ ] **Step 5: Commit** `raw: tombstones in the sequence, masked on every read; redaction rescan (milestone 2, Task 7)`.

---

## Task 8: Backups (MUST-M15)

**Files:**
- Create: `src/backup.rs`
- Modify: `Cargo.toml` (`zstd`), `src/config.rs` (`[backup] dir`, default `<home>/backups`), `src/worker.rs` (the backup step at idle exit and on `next_attempt_at`; a failed `quick_check` now restores), `src/setup.rs` (`doctor`: `integrity_check`, segment checksums, a cloud-folder warning, the last restore), `src/main.rs` (`oboete restore`)
- Test: `src/backup.rs`

**Interfaces:**
- Produces:
  - Segment files `<backup dir>/<device>-<first seq>-<last seq>.seg.zst`: the records (events and tombstones) as JSON lines, bodies as `Raw::after` returns them (masked, D8), zstd-compressed, each with a `.sha256` beside it.
  - `backup::export(home) -> Result<Option<PathBuf>>`: the records above the last backed-up seq; `None` when there are none.
  - `backup::verify(dir) -> Vec<Problem>`.
  - `backup::restore(home, dir) -> Result<()>`: quarantine the damaged `raw.db` as `raw.db.quarantined-<time>`, rebuild it from the segments in seq order, keep each record's (device, seq) and the file's device id (the segments are this device's own history).
  - `Raw::hashes(&self) -> Result<BTreeMap<(String, i64), String>>`: sha256 of each record's content (labels and body as `Raw::after` returns it, so compression does not change it; `sha2` is already a dependency).
  - `worker::run_with`: a failed `quick_check` of `raw.db`, or `SQLITE_CORRUPT`/`SQLITE_NOTADB` on opening it, calls `backup::restore` and writes `<home>/state/restored` for doctor. A failed check of `knowledge.db` only quarantines it as `knowledge.db.quarantined-<time>` and starts an empty one: every consumer rebuilds from seq 0 (spec 1.7), and `raw.db` and the segments are not touched.

- [ ] **Step 1: Failing tests.** MUST-M15, plus unit tests that: a backup directory under `OneDrive`, `iCloud Drive`, `Dropbox` or `Google Drive` gives the doctor warning; Task 7's fixture exports no `zqx-private-words` and no `bravo`, and after a restore reads the same through `Raw::after` (masking twice changes nothing, D8); a damaged `knowledge.db` is quarantined and rebuilt while `raw.db` and the segments stay byte for byte:

```rust
#[test]
fn a_damaged_raw_db_is_quarantined_and_rebuilt_from_its_segments() {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    let mut raw = raw::open(p).unwrap();
    for i in 0..100 { raw.append(&raw::test_event(&format!("event {i}"))).unwrap(); }
    let (before, device) = (raw.hashes().unwrap(), raw.device().to_owned());
    assert!(backup::export(p).unwrap().is_some());
    drop(raw);                                                   // the last close checkpoints the WAL
    overwrite(&p.join("raw.db"), 4096, &[0xA5; 4096]);           // page 2
    worker::run_once(p).unwrap();                                // quick_check fails: quarantine, restore
    assert!(std::fs::read_dir(p).unwrap()
        .any(|e| e.unwrap().file_name().to_string_lossy().starts_with("raw.db.quarantined-")));
    let restored = raw::open(p).unwrap();
    assert_eq!((restored.hashes().unwrap(), restored.device().to_owned()), (before, device));
    let seg = std::fs::read_dir(p.join("backups")).unwrap().map(|e| e.unwrap().path())
        .find(|f| f.to_string_lossy().ends_with(".seg.zst")).unwrap();
    overwrite(&seg, 10, &[0xA5; 16]);
    assert_eq!(backup::verify(&p.join("backups")).len(), 1);     // the checksum names the segment
}

fn overwrite(path: &std::path::Path, at: u64, bytes: &[u8]) {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.seek(SeekFrom::Start(at)).unwrap();
    f.write_all(bytes).unwrap();
}
```
- [ ] **Step 2: Run, expect failure.**
- [ ] **Step 3: Implement.** Segments are sealed: written to a temporary name, fsynced, renamed. The last backed-up seq is read from the segment names (this device's highest `<last seq>`), so it survives a lost `knowledge.db`; `next_attempt_at` lives in `knowledge.db` (lost, it only brings the next export forward). Each run writes segments of at most the cap (D11) until it is caught up. Forget's rewrite of segments is milestone 5's (spec 6.2 step 6); the format allows it because each segment covers a seq range and can be rewritten alone.
- [ ] **Step 4: Run the tests.**
- [ ] **Step 5: Commit** `backup: sealed, checksummed zstd segments; quarantine and restore (milestone 2, Task 8)`.

---

## Task 9: The manifest and SessionStart (MUST-M5)

**Files:**
- Create: `src/manifest.rs`, `src/consumer/manifest.rs`
- Modify: `src/hook.rs` (SessionStart reads the manifest for this checkout and prints it, fenced as data; with the MUST-M16 line from Task 4 when a failure is marked)
- Test: `src/manifest.rs`; a fixture under `src/testdata/` (a directive followed by its negation)

**Interfaces:**
- Produces: `knowledge.db` table `manifests(repo, branch, device, built_at, text)`; `consumer::Manifest` rebuilds the rows whose checkout got new records; `manifest::render(parts, cap) -> String` with spec 4.9's fields and drop order (D12); `manifest::directives(lines) -> Vec<Line>` applying D13.

- [ ] **Step 1: Failing tests.**

```rust
#[test]
fn a_directive_followed_by_its_negation_is_not_shown() {
    let lines = vec![owner("2026-09-01", "今後はテストを先に書いて"), owner("2026-09-03", "テストを先に書くのはやめて")];
    assert!(manifest::directives(&lines).iter().all(|l| !l.text.contains("今後は")));
}

#[test]
fn the_manifest_is_the_same_bytes_for_the_same_records() {
    // build twice from the same raw.db: identical text (determinism, spec 4.9)
}

#[test]
fn fields_drop_in_the_fixed_order_under_the_cap() { /* git state and failing command stay last */ }
```

- [ ] **Step 2: Run, expect failure.**
- [ ] **Step 3: Implement** from raw only: last failing command (a tool record with a non-zero exit or an error field), todo list (the last TodoWrite / `update_plan` / todo tool state), last prompt and reply, files touched (tool file paths), owner directive lines, uncurated count, other sessions with records in the last 30 minutes; risky git state from `git status --porcelain=v2 --branch` in the worker (D9).
- [ ] **Step 4: Run the tests, then the MUST-M5 fixture** through the hook: SessionStart output does not contain the retracted directive.
- [ ] **Step 5: Commit** `manifest: deterministic, from raw, with directive negation (milestone 2, Task 9)`.

---

## Task 10: Compression

**Files:**
- Create: `src/consumer/compress.rs`
- Test: `src/consumer/compress.rs`

**Interfaces:**
- Produces: `consumer::Compress`, which calls `Raw::compress_through(&mut self, device: &str, seq: i64)`: inside `raw.rs`, it rewrites `body` as zstd (level 3) and `enc='zstd'` for records every other consumer has passed; `Raw::after` decompresses transparently, so no reader sees `enc`.

- [ ] **Step 1: Failing test:** after the worker runs, a record's `enc` is `zstd`, its text reads back identical, and search still finds it.
- [ ] **Step 2: Run, expect failure.**
- [ ] **Step 3: Implement.** Only the body changes; (device, seq) and every label stay.
- [ ] **Step 4: Run the tests.**
- [ ] **Step 5: Commit** `raw: per-record zstd in the worker (milestone 2, Task 10)`.

---

## Task 11: Transcript gap check (spec 2.3)

**Files:**
- Create: `src/consumer/gaps.rs`
- Modify: `src/setup.rs` (`doctor`: gaps per adapter), `src/worker.rs` (run at Stop and SessionEnd wake-ups)
- Test: `src/consumer/gaps.rs`

**Interfaces:**
- Consumes: `transcript::events(path, agent)` (milestone 1's parsers, `src/transcript.rs`); `raw::Raw::session_count(session, kind) -> i64` (a scan by label; no index, spec 1.6).
- Produces: `knowledge.db` table `gaps(agent, session, transcript_turns, raw_turns, checked_at)`; doctor prints one row per adapter with sessions short of their transcript.

- [ ] **Step 1: Failing test:** a transcript fixture with 5 typed prompts, raw holding 4 of them: `gaps` records 5 against 4, and doctor names the adapter. Backfilling stays LATER (spec 2.3).
- [ ] **Step 2: Run, expect failure.**
- [ ] **Step 3: Implement.** The transcript path comes from the hook input (`transcript_path` where the agent sends it); an agent that sends none is reported as "not checked", never as a gap.
- [ ] **Step 4: Run the tests.**
- [ ] **Step 5: Commit** `gaps: transcript turns against raw per adapter in doctor (milestone 2, Task 11)`.

---

## Task 12: Replay and M14 on three machines

**Files:**
- Modify: `src/replay.rs` (Design B's hook path; the fixture's `ts`, issue #65; a FULL + fullfsync run; 64 KB and 256 KB payloads; VmHWM)
- Modify: `docs/milestone-2.md`

**Interfaces:**
- Consumes: everything above.
- Produces: the write-hook line (M14), `capture.max_output_bytes`'s default (D4), the backup segment cap (D11), written in `docs/milestone-2.md` and as constants in code.

- [ ] **Step 1: Failing test:** `replay` of the long-24h fixture records events whose first and last `ts` are at least 23.5 hours apart (issue #65).
- [ ] **Step 2: Run, expect failure; implement; pass.**
- [ ] **Step 3: Measure** on WSL, the M1 iMac (SSH) and Windows (the GNU cross-build, D14): 300 hook runs per size (1, 64, 256 KB), with redaction and the ledger, spawned processes, p50/p95/max; backup export time and segment size per idle exit.
- [ ] **Step 4: Set** the line from the slowest machine at each size after the cap; set the cap (D4) and the segment cap (D11) by their rules; write the numbers and the rules applied.
- [ ] **Step 5: Commit** `replay, M14: the write-hook line on three machines (milestone 2, Task 12)`.

---

## Task 13: The none tier end to end

**Files:**
- Modify: `docs/milestone-2.md`

- [ ] **Step 1:** In the dogfood user with Design B's hooks (Task 0): run a real Claude Code session and a Codex session in a scratch repository, with no provider configured. Then: `oboete search` finds a phrase from each; a new session's SessionStart shows the manifest (last prompt, failing command, todo list); `doctor` is green; the worker has exited; a backup segment exists.
- [ ] **Step 2:** Fill the disk of a tmpfs home (Task 4's fixture) and repeat one hook: doctor red, the next SessionStart says recording failed since T.
- [ ] **Step 3:** Write the milestone note: the decisions as applied, the numbers, what milestone 3 inherits (the curation checkpoint, claims, the judge chain).
- [ ] **Step 4: Commit** `docs: milestone 2 note (milestone 2, Task 13)`.

---

## After the tasks

- `cargo test` passes on the three platforms in CI; `docs/milestone-2.md` has the branch point, the numbers of Task 12 and the none-tier run of Task 13.
- The tests to keep from today's code still pass in their Design B form: `<private>` stripping, redaction rules and allowlist, device id per file, uid uniqueness, the busy-timeout race, raw kept after it is consumed. `delete_takes_the_search_rows_along` is not ported: Design B deletes through forget (D8).
- Next: milestone 3 (Curate), after the curator spike's gate-quality part.
