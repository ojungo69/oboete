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
- **`store_prompts = false`** omits the text of both kinds of prompt event, typed prompts and harness envelopes. The event stays, so Task 11 can count the turn (#83).
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
- Not yet: a hook starts the worker only after a write that succeeded, so a damaged raw.db is restored at the next worker run that something else starts. With #104 in, a hook whose write failed also starts the worker (part b). The rescan rewriting sealed segments (#83) follows Task 7's part b.
