# ADR 0002: Backup manifest authenticity

**Status:** Accepted for Phase 1

## Context

Phase 1 backups are local, owner-only SQLite snapshots. Restore needs a deterministic manifest for corruption checks and compatibility decisions, but Phase 1 has no signing-key lifecycle or encrypted off-device backup surface.

## Decision

Phase 1 stores a SHA-256 hash of the canonical JSON manifest and the SHA-256 of the completed SQLite artifact. The manifest is generated only after the online backup is finalized, by reopening that artifact read-only. Backup artifacts and sidecars are mode `0600`; their directory is mode `0700`.

The sidecar explicitly records `authenticity: "hash-only"` and `signature: null`. This detects accidental corruption and one-sided modification. It does not claim authenticity against an attacker who can replace the artifact, manifest, and hashes together. Owner-only permissions reduce cross-user exposure but do not protect a compromised owner account.

Phase 1 does not generate a long-lived local signing key. Adding a key without defined storage, rotation, recovery, and device-transfer behavior would turn an availability failure into an unrecoverable backup failure. Off-device backup/export is not provided in Phase 1; when introduced, encryption is required by the product specification.

## Phase 8 blocking follow-up

Core 1.0 release work must define signing-key storage, rotation, recovery, revocation, and multi-device trust; sign the canonical manifest; verify the signature before restore; and specify migration behavior for Phase 1 hash-only backups. The release gate must cover artifact replacement, sidecar replacement, wrong-key, rotated-key, and recovery-key fixtures. Signed verification is blocking before backup authenticity or off-device protection is claimed.
