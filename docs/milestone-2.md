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
