# Milestone 2: Record

The plan is docs/milestone-2-plan.md. The spec is docs/spec.md sections 1-2, 4.9 and 8.4 item 2. Acceptance tests carried from the plan's review are in issue #83.

## The `v1` branch (Task 0, 2026-09-26)

- `v1` was branched from `main` at 431bafc. Its `src/`, `Cargo.toml` and `Cargo.lock` are the same as at b95b827, the commit of the owner's last install (#71): `git diff --stat b95b827 431bafc -- src Cargo.toml Cargo.lock` prints nothing.
- `v1` is protected by the same ruleset as `main`: no deletion or force push, changes by pull request, linear history, required checks.
- Until the cut-over after milestone 5 (spec 7.5), the owner's binary is built from `v1`. A v1 fix (real loss or safety only) is a PR into `v1`. `main` is Design B from Task 1 on.
- Design B's hooks in the dogfood user (Task 0 step 4) are set up once Task 2 gives `main` a hook path of its own. Until then `main` builds the same binary as `v1`.

## Compression (Task 10, 2026-09-27)

- The `compress` consumer runs last. It rewrites the plain event bodies up to the lowest checkpoint of the other consumers as zstd level 3 (`zstd` 0.14), and records `enc = 'zstd'`. With no other consumer, it goes up to raw's highest seq. `Raw::after` decompresses, so no reader sees `enc`. A consumer added later reads compressed records the same way, so compression does not wait for one that has not started.
- A body that zstd does not make smaller stays plain; its checkpoint still moves, so it is not tried again. `Raw::compress_through` takes the range after the consumer's checkpoint, not every plain body from seq 1 (the plan's signature had only the upper end).
- Bodies are read and compressed outside the write lock, then written 200 at a time in one short `BEGIN IMMEDIATE` transaction, so a hook waits on it no longer than on another hook.
- `Raw::after` refuses a frame that expands past 64 MiB (`MAX_BODY_BYTES`), so a crafted one, as a restored or synced record could carry, cannot expand without bound. A body above that size stays plain, since capture caps each string, not a whole body.
- Fixture of record, replayed (228 Claude Code and Codex events), then one worker run: bodies 40,748 → 28,497 bytes (−30%), 89 of 228 compressed, 0.01 s. The zstd dictionary (spec 2.4) is decided on a week of real raw.db.

## Tombstones (Task 7, part a, 2026-09-27)

- `Raw::append_tombstone` writes a tombstone as the device's next seq. `Raw::after` hides what the tombstones of the returned records target, in one query per read by target (a partial index on `(target_device, target_seq)` for tombstones only): an event targeted whole comes back as `Item::Removed`, a byte range as `*`. A range is widened to whole characters and cut at the body's end, so the body stays valid UTF-8, offsets stay valid, and masking twice changes nothing (D8). A tombstone of a tombstone hides nothing.
- The FTS consumer indexes a tombstone's target again as `Raw::after` returns it: masked text, or the document removed. Search, `oboete get` and every consumer read through `Raw::after`, so none can show what a tombstone covers.
- Every tombstone this milestone comes from the redaction rescan (source `rescan`).

## The redaction rescan (Task 7, part b, 2026-09-27)

- The `rescan` consumer runs first in each pass. It keeps, per device, the ruleset version (Task 3b) the records were last scanned with. Under the same version it only follows new records, which capture already scanned. Under another (a user rule added, a kept value removed), it starts again from seq 1 and appends a range tombstone for each range the rules now find in a body, from both views capture scans (the stored text and its JSON-unescaped view). What capture masked, and a tombstone's `*`s, give no new range. A settings file that does not load stops the rescan as it stops capture; doctor names it.
- Its checkpoint moves back when it starts again, so the worker now sets any checkpoint a step returns, not only a higher one.
- The tombstones reach the other consumers as records: the FTS consumer indexes each target again, masked. The rescan appends them before the FTS consumer reads the same batch.
- A rewind (raw lost commits, which may include its tombstones) and a new knowledge.db (a restore moves it aside) forget the stored version, so the whole store is scanned again and the tombstones are derived again. Tombstones no rule derives (`forget`, milestone 5) will need their deletion state kept apart (#83).
- It scans each JSON string of a body on its own, as capture scanned the fields, so a rule anchored to a field's start or end (`^`, `$`) matches as it would have at capture.
- Reads do not wait for it. Every read path prints stored text through the egress gate with the rules as they are now: the MCP answers (#108), SessionStart's manifest (#111) and the CLI's output (`search` gates each hit's text before its snippet is cut and each label on its own; `get` gates the body field by field). So a rule hides its value on every read from the moment it is saved; the rescan then tombstones it in raw.db, the index and the next backups when a worker runs. A hit in a label (repo, branch, cwd, session) is hidden on reads but stays stored until a tombstone can target a label (#83).
- Fixture of record (405 Claude Code and Codex records), release build, WSL: the first worker run, which scans everything once, took 0.04 s and added no tombstone; after a user rule was added, 0.06 s and 24 tombstones.

## Redaction and capture settings (Task 3b, 2026-09-27)

`config.toml` gains two tables (spec 1.5, 6.4). Capture reads only these two, so a mistake in `[[providers]]` never stops recording.

```toml
[redaction]
# Added to the bundled rules, which cannot be removed. Named `user:<id>` in the ledger.
extra_rules = [{ id = "acme", regex = 'acme-[0-9]{6}', keywords = ["acme"] }]
# The SHA-256 (hex) of one exact value to keep, never a pattern.
allowlist = ["<sha256 of the value>"]

[capture]
store_prompts = true        # false: the turn is recorded as {"omitted": true}
tool_output = "full"        # or "head-tail"
```

Decisions (Claude; overrulable):

- **A wrong table records nothing.** A bad regex, id, `secret_group`, allowlist entry or unknown key makes the hook of a ported agent fail before it opens `raw.db`, so no text is stored under rules the user did not get. MUST-M16's marker and doctor report it; doctor names the mistake and exits non-zero. Every other command except `doctor` and `setup` stops on it too: several of them (`observe`, `reindex`, and `mcp` and `search` with embeddings) send text out, and the egress gate must apply the user's rules.
- **The ruleset version** is the bundled files' hash when the user adds nothing, so a store upgraded without settings keeps its version and Task 7 does not rescan for nothing. With user rules or kept values, it also covers them in a canonical form: reordering the file changes nothing; adding or removing a rule or an allowlist entry changes the version.
- **A rule without keywords always runs**, as in gitleaks. With keywords, it runs when one appears in the text (any case).
- **Each rule looks at the text with its own context** (Codex security review, three rounds). A rule finds one secret per context per look: curl-auth-user's greedy `.*` takes the last `-u` of a line. So every rule first looks at the original text. Each rule that found or kept something then looks again on its own, until it finds nothing new. It looks at a copy where only its own findings and kept values are blanked out with spaces of the same length. Offsets stay those of the original, which is masked once at the end. No rule takes context another needs: a user rule matching `curl`, a bundled mask over a `Bearer` token that a user rule reads, or two user rules that share a prefix. The old pass over the masked text stays after that, and it can only add masks.
- **A kept value does not hide its neighbors.** When a greedy match is a kept value, the value is blanked in the next look, so the rule finds the secret it passed over. The stored text still holds the kept value.
- **The pass limit fails closed.** A rule still finding after 64 looks masks the whole text, so no look stops with a secret left unlooked-at.
- **`oboete mcp` answers pass the egress gate.** What it returns goes into the agent's context, and so to the agent's model provider. So the user's rules as they are now apply to stored text too, including rules added after it was stored.
- **Egress follows the file as it is.** `redact::outbound` rereads `config.toml` at each call and rebuilds the rules when it changed, so a long-running `oboete mcp` applies a rule added, or a kept value removed, while it runs. If the table turns wrong while a process runs, nothing of the text leaves: it is sent as one mask.
- **Errors never quote a value.** An allowlist entry is named by its position and a regex error only by its rule id. A TOML error in `config.toml` gives only its line number, wherever it is: TOML and serde messages can quote the value on the line. Telling `[redaction]` apart by reading the file failed on legal forms (`[ redaction ]`, `[[redaction.extra_rules]]`). This holds for capture, doctor and a running `oboete mcp`, since `config::load` uses the same formatting.
- **The allowlist** is checked against the value as it appears in the text being scanned. A value inside a flattened tool field that holds `\"` or `\n` needs the hash of that escaped form.
- **`tool_output = "head-tail"`** keeps the first and last 4 KB of each tool output (`capture::HEAD_TAIL_BYTES`, 8 KB in all, about v1's 8,000 characters). `"full"` is spec 2.4's default: whole up to `MAX_FIELD_BYTES`, head and tail above it. Tool input and the other fields are unaffected.
- **`store_prompts = false`** omits the text of both kinds of prompt event, typed prompts and harness envelopes. The event stays, so Task 11 can count the turn (#83). A prompt that was all `<private>` records nothing under either setting, so whether a turn is recorded never depends on the settings.
- **Agents not yet ported** (v1's write path, until Task 2b) keep the bundled rules only.
- `oboete setup --advanced` (the hidden prompt that stores an allowlist value as its hash) is not part of this task. Until then the user writes the hash, for example `printf %s '<value>' | sha256sum`.

Replay of the fixture of record (release build, WSL), before and after, each run twice:

| | in-process p50 / p95 (µs) | spawned hook p50 / p95 (ms) |
|---|---|---|
| before (main at 481682f) | 1517 / 2198 | 9 / 11 |
| after, no config.toml | 1513 / 2109, 1497 / 2171 | 9 / 11, 9 / 10 |
| after, 2 extra rules and 1 allowlist entry | 1487 / 2124, 1520 / 2140 | 9 / 11, 9 / 11 |
| after the security fixes, no config.toml | 1604 / 2231, 1911 / 2597 | 9 / 11, 9 / 12 |
| after the security fixes, the same settings | 1619 / 2289, 1513 / 2274 | 9 / 11, 10 / 10 |
| after the third round (each rule its own look), no config.toml / the same settings | 1593 / 2183, 1605 / 2244 | 9 / 11, 10 / 11 |

The in-process numbers move by 25% between identical runs on this machine (load average about 1), so they show no change beyond that noise; the spawned hook, which also reads `config.toml`, stays at 9-10 ms.

## Backups (Task 8, part a, 2026-09-27)

- A segment is `<device>-<first seq>-<last seq>.seg.zst` (seqs zero-padded to 12 digits) in `[backup] dir` (relative to the home), default `<home>/backups`. It holds one JSON line per record as `Raw::after` returns it: masked by tombstones (D8), a whole-removed event as `{"type": "removed"}`, and each event's redaction ledger rows (#83: the ledger survives a restore). `backup.rs` reads only `[backup]` from config.toml, so a mistake elsewhere in the file does not stop backups.
- Sealing: the `.sha256` beside the segment is written first, then the segment, each to a temporary name, synced and renamed, then the directory is synced. The last backed-up seq is the highest `<last seq>` in this device's segment names, so it survives a lost knowledge.db. A segment holds at most 8 MB of lines (D11; Task 12 may halve it), and one run writes segments until it is caught up.
- The worker backs up at every idle exit that has new seqs, under its lock (a worker started after the release cannot export the same seqs), and every 30 minutes while it runs. The 30-minute deadline is kept in memory: every idle exit backs up too, so a lost deadline only brings the next backup forward. A failed backup is printed and never stops the worker; doctor shows how far the backups reach.
- Restore: the worker restores when opening raw.db reports SQLITE_CORRUPT or SQLITE_NOTADB, or its `quick_check` reports a problem. Busy, permission and other errors stay errors. raw.db and its `-wal` and `-shm` are moved aside as `raw.db*.quarantined-<ms>` (SQLite binds the WAL to the name). A new file is built at `raw.db.restoring` from the segments that verify, in seq order, with the device id read from the damaged file, or else from the one device the segments name (never a guess between two), and renamed into place. Its identity (`meta.store_file`) is the new file's own, which a rename keeps, so the device id stays. Bodies are stored as zstd where smaller. A restored tombstone has ts 0 and source `restore` (`Raw::after` does not return them). A whole-removed event is a `removed` row. What was done goes to stderr and `<home>/state/restored`, which doctor prints. `oboete restore` does the same by hand, under the worker's lock.
- A restore holds `<home>/raw.lock` exclusively while it reads, rebuilds and swaps the file; every open of raw.db holds it shared, waiting at most 2 s. So no hook keeps the old file open across the swap and loses its event there: it waits, or its write fails with MUST-M16's marker. A restore waits at most 10 s for open stores to close. The rebuild is made as `raw.db.restoring` and renamed `raw.db.restored` only once its records are committed. Before the swap, knowledge.db and any skipped segment are moved aside; then the damaged raw.db goes aside and `raw.db.restored` is renamed in. A restore that stops before the swap is redone by the next worker (raw.db is still damaged); one that stops between the two renames is finished by the next open, which renames `raw.db.restored` (never a partial `.restoring`) into place instead of creating an empty store.
- After a restore, knowledge.db is moved aside too and rebuilt from the restored records: a skipped (damaged) segment leaves a gap below raw's highest seq that the old index and checkpoints would still cover. The skipped segment and its checksum are moved aside (`.quarantined-<ms>`), so the export cursor never trusts its name and the seqs it claimed are backed up again as they are reused.
- A record a tombstone masks is exported with its ledger rows' `field` replaced by `~tombstoned`: the field is a JSON pointer that names keys of the body, which the tombstone may cover.
- A damaged knowledge.db (and its `-wal`, `-shm`) is moved aside the same way and started empty; every consumer rebuilds from seq 0. raw.db and the segments are not touched.
- doctor: the backup directory, how many segments and through which seq of raw's highest, each segment whose checksum does not match or is missing, a warning when the data or backup directory is inside OneDrive, iCloud Drive, Dropbox or Google Drive, the last restore, and raw.db's full `integrity_check` (doctor only, never on an automatic path). A bad segment or a failed check turns doctor red.
- A hook starts the worker after a written row, and also after a write that failed because raw.db is damaged (SQLITE_CORRUPT or SQLITE_NOTADB): the failure is marked (MUST-M16) and the worker restores the file. Other write failures (a full disk, busy) start none. A worker already running opened raw.db before the damage and may not see it, so the hook also leaves `<home>/state/restore-wanted`: the worker checks for it while it waits and again after it releases the lock, and then closes its stores and opens them again, which restores raw.db. A read of its own that finds raw.db damaged does the same, twice at most per run. Tests: `tests/damaged_raw.rs`, through the binary.
- Not yet: the rescan rewriting sealed segments (#83) follows Task 7's part b.

## The manifest (Task 9, 2026-09-27)

The `manifest` consumer keeps one text per checkout (repo, branch, device) in knowledge.db, and SessionStart shows it (spec 4.9, D12, D13). Facts point at records by (device, seq); a build reads the few bodies it shows back through `Raw::after`, so what a tombstone hides there is hidden here too. File paths are the one thing copied (as labels).

- **Same records, same bytes.** Every part except the git state comes from records. A record on any branch marks every checkout of its repo on the device for a rebuild, since the owner's directives and the other sessions are the repo's. A rebuild from an empty knowledge.db gives the same text.
- **SessionStart reads, never writes.** It opens knowledge.db read-only and shows nothing when the worker has not built the checkout yet. It also shows nothing while the saved text may show what raw now hides: a tombstone after the consumer's checkpoint, or a checkout still marked for a rebuild. The recording-failure line (Task 4) comes first, then the manifest inside `<oboete-memory>`, which says the text is data, not instructions; recorded text cannot close the fence. A resume shows nothing (its context has the manifest already); a compaction's SessionStart shows it again.
- **Redaction rules as they are now (spec 6.4).** A build gates each field with the rules before it flattens whitespace, clips or splits it, and saves the ruleset version with the text. SessionStart shows a text only when that version is the current one (then gates it once more and cuts it to its cap at a line), and the worker's next pass builds every checkout saved under another version: a flattened, clipped text cannot take a rule written for the field it came from.
- **Directives by line (D13).** A prompt is split at newlines and `。`; each line with a directive marker is one directive, and a negation line takes back only the lines it shares a content word with. English sentences on one line stay one line. Only the last 500 owner lines are read (a claims table replaces the scan in milestone 3).
- **The last failing command** is the newest failure (the hook said so, or a non-zero exit in Grok's `exit_code` or Codex's `Exit code: N` / `Process exited with code N` header, read only before `Output:`) with no later success of the same call. An interrupted call is neither.
- **Git state (D9)** comes from `git status --porcelain=v2 --branch` in the worker, given up after 500 ms, with `--no-optional-locks` and `core.fsmonitor=false`. A filter a repository's config names (`clean`/`process`) can still run during status: its command is set only in git config (local, global or system), which a clone cannot set, and the owner's own `git status`, editor and coding agent run the same filters in that checkout. When git is given up on, its reader thread is left to finish, so a process git started that holds the pipe cannot stop the worker.
- SessionStart with a manifest, release build, WSL, 200 records: 10.2 ms median against 9.7 ms for a resume (no manifest read); the text was 1.5 KB.

## Transcript gaps (Task 11, 2026-09-27)

When a session ends, the `gaps` consumer counts the turns the agent's transcript holds against the ones raw recorded, and doctor prints one line per agent: how many ended sessions were short of their transcript and by how many turns, and how many were not checked (spec 2.3). Backfilling from transcripts comes later, and only if gaps show up.

- The trigger is the `end` record. SessionEnd's hook input carries `transcript_path`, which capture now keeps in the `end` body as `transcript`. Both ported agents (Claude Code and Codex) send SessionEnd, so no Stop needs to parse a transcript; the plan's Stop trigger is for agents without a reliable SessionEnd (agy, OpenCode), which are not ported yet.
- Both sides count the same thing by construction. Raw's side is the session's `prompt` and `envelope` events (`Raw::turns`, a scan by label). The transcript's side is the prompts `oboete transcript` implies, each passed through `capture::events`, so a prompt that was all `<private>` is no turn on either side. Whether capture records a turn does not depend on the settings (`store_prompts` changes only its text), so a change of settings between capture and the check, or a rebuild of knowledge.db, gives the same count.
- A transcript with a record type the parsers pass over and the list of known ones does not name is "not checked" too: a new type may be a new place for a prompt. The list holds the types in docs/milestone-1.md's transcript notes and every type passed over in the owner's 300 newest Claude Code and 150 newest Codex transcripts (2026-09-27, type names and counts only; none of them had an unreadable line).
- A transcript with a line that is not JSON is "not checked" (as one that is missing, or without a path): the turn a lost line held would make the counts agree.
- An `end` without a transcript path, with a file that is gone, or of an agent without a parser (only `claude` and `codex` have one) is "not checked", never a gap.
- The transcript is parsed once per session end, one line kept at a time (the parser itself holds the lines to sort them: 39 MB for the largest dev session, 111 MB with its subagent files, 0.3 s). A resumed session is checked again at its next end.
- Replaying `src/testdata/transcripts/{claude,codex}-basic.jsonl` through the hooks and running the worker gives 6 of 6 and 2 of 2 turns: no gap. The Claude fixture holds one line that is not JSON on purpose, so under the rule above it is now "not checked"; the Codex one still gives 2 of 2.

## Replay and M14 (Task 12, 2026-09-27)

`oboete --home <tmp> replay ../free-mem/test/fixtures/events-1000.jsonl --spawn-sample 300 --sizes 1,64,128,256`, release build. Replay records each event at the fixture's `ts` (issue #65; the 24-hour fixture keeps its day in raw). Then, per size, it spawns 300 `oboete hook claude PostToolUse` whose tool output is tool-output-like text of that size (paths, code, Japanese, every 20th line with words that wake redaction rules but no secret, as in Spike 1). Each spawn loads the settings, scans in full, writes the event and its ledger rows, and makes the worker-lock attempt it makes after every write; replay holds the lock meanwhile, so no worker starts. Then it exports the backup and times each segment.

Hook, spawned, milliseconds (p50 / p95 / p99 / max):

| Machine | 1 KB | 64 KB | 128 KB | 256 KB |
|---|---|---|---|---|
| WSL (ext4) | 10.5 / 12.1 / 12.7 / 25.8 | 12.1 / 13.8 / 15.9 / 18.9 | 12.8 / 14.4 / 15.2 / 15.7 | 14.9 / 16.2 / 16.7 / 17.2 |
| Windows, GNU build (NTFS, Defender on) | 19.9 / 22.8 / 80.0 / 85.4 | 23.5 / 25.5 / 26.7 / 100.9 | 25.4 / 27.3 / 70.8 / 506.9 | 27.7 / 30.8 / 108.1 / 189.7 |
| M1 iMac (APFS, fullfsync) | pending | pending | pending | pending |

- An earlier Windows run without 128 KB gave p95 22.1, 24.8 and 31.0 ms at 1, 64 and 256 KB; the rules below use the worse of the two runs.
- Against Spike 1's harness at full redaction (docs/spike/hook-m14.md), Windows' 1 KB p95 rose from 13.5 to 22.8 ms and its 256 KB p95 fell from 41.4 to 30.8 ms; WSL's moved from 9.4 to 12.1 ms and from 20.3 to 16.2 ms. This hook does more than that harness (settings, the ledger, the worker-lock attempt); which part costs Windows the extra 9 ms at 1 KB is not measured.
- The iMac is offline: Tailscale showed it last seen about an hour before 10:05 JST. FileVault stops a restart at the unlock screen, so it may need the owner at the machine. It is measured with the same command when it is back, and the rules below are applied again.
- The Windows run found a replay bug: the fixture's root placeholder was replaced with an unescaped Windows path, which broke the JSON (`invalid escape`). It is escaped now (`a_repo_root_with_a_backslash_replays`).
- Windows is the GNU cross-build (`cargo zigbuild`), as in Spike 1. The MSVC artifact is unmeasured until CI builds it (D14).
- Each size is measured as written whole: the runs above were made while the cap was 256 KB, and replay now sets `OBOETE_FIELD_CAP` on the hooks it spawns to the size measured (read only between 1 and 256 KB), so the iMac run measures the same writes under the lowered cap.

The rules as applied, provisional until the iMac row is filled:

- **The line** is the slowest machine's p95 at 1 KB: the floor every hook pays, which no cap can lower (on the iMac, `F_FULLFSYNC`; D3). Measured so far: Windows, 22.8 ms. Spike 1's iMac value (24.9 ms) came from a lighter harness, and Windows' 1 KB rose by 9 ms between that harness and this hook, so it is not reused.
- **The cap (D4)** is the largest of 64, 128 and 256 KB whose p95 on the slowest machine stays within the line. With WSL and Windows none does (Windows is at 25.5 ms from 64 KB on), so it is the rule's smallest, 64 KB: `capture::MAX_FIELD_BYTES` goes from 256 KB to 64 KB, and a longer stored string keeps its first and last 32 KB around the cut marker. It moves to 128 KB if the iMac's 1 KB p95 is at least 27.3 ms and its own 128 KB p95 is within that; to 256 KB if at least 31.0 ms and its own 256 KB p95 is within that.
- **M14's line per size (D14)**, for sizes up to the cap: 1 KB 22.8 ms, 64 KB 25.5 ms p95 (Windows), until the iMac is measured.
- **The backup segment cap (D11)**: p95 per 8 MB segment is 117.7 ms on WSL and 153.8 ms on Windows (18 segments each), under 1 s, so `backup::SEGMENT_BYTES` stays 8 MB. Segments were 1 to 11 KB compressed: the synthetic payload repeats, so these sizes are a lower bound for real records; only the time feeds the rule.
- Also measured: the in-process store path per fixture event (255 events, no spawn) is 1.5 / 2.1 ms p50 / p95 on WSL and 0.8 / 1.2 ms on Windows; the replay process's peak RSS on WSL is 126,000 KB (VmHWM is read from `/proc`, so Windows reports none).

## The other agents (Task 2, part b, 2026-09-27)

Pi and OpenCode send Claude Code's hook fields, so they record to raw.db through the same capture path and read the manifest at SessionStart. Grok, agy and Cursor follow, each with its own payload shape (below).

Decisions (Claude; overrulable):

- **Per-session hook state lives in files.** A few hook calls need to know what an earlier call of the same session did: Grok and agy inject once per session, and Cursor injects again on the first prompt after its compaction marker. Neither store fits: raw.db has no session key (spec 1.6), and knowledge.db is the worker's, which SessionStart only reads. So each flag is an empty file under `<home>/state/hooks/<agent>/<hash of the session id>/`, as the recording-failure marker is a file. `File::create_new` makes a claim atomic between concurrent hooks (Grok runs tool calls in parallel), and a removal succeeds once. Losing the files costs one extra injection, never a record, so they are neither backed up nor synced. The worker removes the flags of sessions unchanged for 7 days at its idle exit (agy and OpenCode send no SessionEnd).
- **Each agent injects at its own point.** Claude Code, Codex, Pi and OpenCode: SessionStart, not on a resume. Grok ignores SessionStart's output, so its first tool call of a session injects; agy reads PreInvocation, once per session too. Cursor: SessionStart, and the first prompt after its compaction marker. The text is the manifest in its fence, after the recording-failure line, in the shape the agent reads (`hookSpecificOutput`, agy's `injectSteps`, Cursor's `additional_context`).
- **A point is claimed before the manifest is read.** A Grok or agy session whose checkout has no manifest yet gets none later in that session, as a Claude Code session started before the worker built one gets none (v1 retried on every tool call until it had text; that is one knowledge.db read per call for a session that may never get one).
