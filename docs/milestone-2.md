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
- Every tombstone this milestone comes from the redaction rescan (source `rescan`). The rescan itself (part b) needs the ruleset version of Task 3b (#108).

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
- **Each group of rules looks at the text with its own context** (Codex security review, two rounds). A rule finds one secret per context per look: curl-auth-user's greedy `.*` takes the last `-u` of a line. So the bundled rules and the user's each run to their own fixpoint on a copy of the text. In that copy, only what the group has already found, and every value the user keeps, is blanked out with spaces of the same length. Offsets stay those of the original, which is masked once at the end. A user rule therefore never takes context a bundled rule needs (a rule matching `curl`), and a bundled mask never takes context a user rule needs (a rule reading `Bearer …; otp=`). The old pass over the masked text stays after that, and it can only add masks.
- **A kept value does not hide its neighbors.** When a greedy match is a kept value, the value is blanked in the next look, so the rule finds the secret it passed over. The stored text still holds the kept value.
- **The pass limit fails closed.** A group still finding after 64 looks masks the whole text, so no look stops with a secret left unlooked-at.
- **Egress follows the file as it is.** `redact::outbound` rereads `config.toml` at each call and rebuilds the rules when it changed, so a long-running `oboete mcp` applies a rule added, or a kept value removed, while it runs. If the table turns wrong while a process runs, nothing of the text leaves: it is sent as one mask.
- **Errors never quote a value.** An allowlist entry is named by its position and a regex error only by its rule id. A TOML error never prints the source line. One in `[redaction]` (or a root `redaction` key, or one whose place is unknown) gives only its line number, since TOML and serde messages can quote the value there. This holds for capture, doctor and a running `oboete mcp` (`config::load` uses the same formatting). Elsewhere the message stays.
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

The in-process numbers move by 25% between identical runs on this machine (load average about 1), so they show no change beyond that noise; the spawned hook, which also reads `config.toml`, stays at 9-10 ms.
