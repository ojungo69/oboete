# Design B, section 7: running and releasing (draft, 2026-09-25)

Short names as in sections 5 and 6. Settled inputs: owner decisions 8 (public from the start), 9 (server location and archiving raw now: postponed until the design is settled), 13 (no code signing at first), 14 (raw sync at setup), "updates only by explicit `oboete update`", "claude-mem is never stopped, deleted or reconfigured", "finish every feature before release"; MUST-M23 (public install path) and the LATER install items in RD/improvements-synthesis.md; project CLAUDE.md (new features go to the isolated `oboete-dogfood` user before the owner's environment).

## 1. Install

- Release binaries for Linux x64/arm64, macOS arm64/x64 and Windows x64 are built only in CI, with SHA-256 checksums and GitHub artifact attestation; a one-line `sh` / PowerShell installer verifies both (MUST-M23). No signing at first (decision 13); before release, check on real machines that the command install shows no Mark-of-the-Web or quarantine prompt.
- One static binary: SQLite bundled, rustls, no absolute dylib paths (phase0: pg0 failed on macOS without Homebrew). Console output survives a Japanese (cp932) Windows console (phase0: Hindsight's banner crashed there).
- Local bge-m3 is an optional download with a pinned SHA-256, resume and a free-space check; the user can choose Workers AI or no embeddings and change it later with `oboete setup --embeddings` (MUST-M23).
- Uninstall: `oboete setup --remove` takes every adapter entry out again (it exists today, src/setup.rs:2); the docs list the data paths to delete by hand. Pulled forward from LATER (r1-public-3), because a public tool needs a way out.

## 2. Setup

- `oboete setup` detects the 7 agents, writes hooks, plugins and MCP config, keeping a `.oboete.bak` copy of each file it edits (src/setup.rs:4, :353) and preserving comments in JSONC files (Cursor adapter).
- Questions, each with one line on what it sends where (MUST-M23): AI tier preset and chain (section 1), embeddings (none / local / Workers AI), raw sync (decision 14, default off), capture exclusions (section 6), hub (optional; `oboete hub deploy`, section 5), transcript backfill (§4 below).
- After setup: the curator isolation check for each chosen CLI (section 6), and the per-agent status table (live-verified / implemented / unverified, M10).
- `oboete setup --yes` takes the defaults (tier none, no embeddings, no hub, no backfill) for scripts. The UI follows the locale (Japanese or English); the CLI names stay English.

## 3. Update

- Only `oboete update` updates (owner rule). It downloads the release, verifies checksum and attestation, and swaps the binary atomically (on Windows, rename the running exe and replace it).
- Before a schema migration, the worker runs a backup (the sealed segments of MUST-M15), so forget reaches that copy like any other backup. knowledge.db is rebuildable (section 1), so the backup holds raw.db and the op log only.
- Migrations are forward-only, in one transaction. A downgrade across a schema version is refused; `oboete restore` from the pre-migration backup is the way back.
- After an update, `oboete setup --refresh` rewrites adapter entries whose format changed. Other devices on an older version park ops they do not understand (section 5, M17); doctor names the version needed.
- New versions reach the owner's machines only after the `oboete-dogfood` user ran them (project rule).

## 4. Moving existing data in

- **The current oboete store** (~/.oboete/oboete.db on the owner's machines: 14,826 raw events, 141 observations, 17 summaries, 14 prompts, 20 sessions on WSL today). The new version reads it and never writes it.
  - events become raw records in raw.db, labelled `source = oboete-v1`, in timestamp order with new seq numbers, redacted again with the current rules. They can be curated like any raw (`oboete recurate --source oboete-v1`, with a cost estimate first).
  - observations, summaries and prompts become imported documents: search and timeline only, never injected, status unknown (section 4).
  - The old file stays untouched until `oboete migrate --finish`, which lists the old files (oboete.db, pre-*.db snapshots, spool) and asks before deleting them. Until then, doctor lists them, and forget prints them as a limit (they are the old system's files; section 6 §3).
- **claude-mem history**: read-only import (owner decision), opened with `mode=ro`, never written; labelled imported; keyed by source id so a re-import skips tombstoned items (section 5 deny-list).
- **Transcript backfill** (new, optional): at setup the user may build raw records from the agents' own transcripts (Claude Code ~/.claude/projects, Codex and others where a transcript exists). setup shows a count and size first; records are redacted, labelled `source = transcript`, deduplicated against hook-captured raw by (agent, session, turn), and curated only when the user asks. Gives a new user day-one memory. Default off.
- **Now, before the redesign** (owner question 1): today's code deletes a session's raw events as soon as they are summarized (src/db.rs:848, `DELETE FROM events WHERE session_id=?1 AND id<=?2`), so every summary loses raw that the new design treats as the source of truth. A small PR can stop the delete now.

## 5. Cut-over on the owner's machines

- Order: the dogfood user, then WSL, then Windows native, then the M1 iMac. Each machine: install, run migration (§4), run doctor, compare search results on the 112 questions against the old store, then switch the hooks (setup rewrites the entries).
- Rollback: reinstall the previous binary; the old store was never written, so it is intact.
- claude-mem keeps running unchanged next to oboete; both inject at SessionStart as they do today. The public docs say running both is supported and that each injects its own block.

## 6. Running

- Worker lifecycle: hook-started, exits when idle (default until M5 decides).
- doctor lines: pending windows with reasons and next attempt; coverage; provider budget left; last sync, last pull and last backup; curator isolation status; capture failures; unfinished forget jobs; disk use and growth; version skew; adapter status; old files awaiting `migrate --finish`.
- Logs hold no payload (S7), rotate and are size-capped. `oboete doctor --bundle` writes metadata only, for bug reports (LATER r2-oncall-4, pulled forward for public support).
- No telemetry. oboete makes no network call of its own except the providers, embedder and hub the user configured, and `oboete update` when run.

## 7. Public release

- Apache-2.0 (LICENSE exists), NOTICE crediting the claude-mem donor code.
- Docs: a Japanese README first, then English; MUST-M23's four docs (what is stored and redacted; what leaves the machine per tier; what forget purges and its limits; agent setup with M10 status); docs/hub-protocol.md (section 5); SECURITY.md with a private reporting address; a CHANGELOG and semver.
- CI: `cargo audit` now, non-blocking; cargo-deny and an SBOM at release (LATER r2-security-6).
- Release gate: every feature finished (owner rule) and section 8's evaluation lines met.

## Owner questions

1. Stop today's raw deletion now with a small PR (recommended), or leave it until the redesign lands (decision 9's second half)?
