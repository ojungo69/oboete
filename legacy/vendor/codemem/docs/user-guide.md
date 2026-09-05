# User Guide

## Start or restart the viewer
- `codemem serve` runs the viewer in the foreground.
- `codemem serve start` runs it in the background.
- `codemem serve restart` restarts the background viewer.
- `codemem serve --background` still works as a deprecated alias for `codemem serve start`.

## Viewer trust model

- The viewer and its JSON APIs are designed for **localhost-only** use.
- codemem currently relies on loopback-origin checks and local-process assumptions, not a real login/session auth layer.
- Binding the viewer to `0.0.0.0`, putting it behind a reverse proxy, or exposing it through a tunnel can make local APIs reachable in ways the current trust model was not built for.
- Treat the viewer as a local tool. If you must expose it beyond loopback, add your own auth and network restrictions first.
- This warning applies to the viewer HTTP service, not the separate sync/coordinator listeners documented elsewhere.

## Check local operational status

`codemem status` is the local operational roll-up. It reports database readiness,
viewer state, sync, maintenance, semantic indexing, raw-event ingestion, and the
observer without changing configuration or stored data.

```fish
codemem status
codemem status --json
codemem status --db-path ./codemem.sqlite
```

The legacy `--config` status option is retained temporarily for CLI compatibility but no longer
selects runtime capability. Status reports a warning when it is supplied; activate provider state
with `codemem setup` instead.

- `status` answers whether codemem can do useful local work; `stats` reports database inventory and usage.
- Collection is offline and local-only. It does not contact peers, coordinators, registries, update services, or non-loopback viewer hosts.
- A missing database is reported without creating it. Existing databases are opened read-only.
- With no viewer PID record, status probes the configured loopback viewer address; a malformed or non-loopback record reports `unknown` and is not fetched.
- Warnings and errors appear in the bounded `attention` list. A collected report exits `0` even when `ok` is false; collection failures exit `1`, and usage errors exit `2`.
- Terminal raw-event and observer failures affect `ok` for 24 hours; use `codemem db raw-events-gate` for the detailed reliability window.
- Use `codemem sync status` or `codemem sync doctor`, `codemem maintenance status`, and `codemem db raw-events-status` for detailed subsystem diagnostics.
- `codemem status` displays bounded `retry_exhausted` job IDs. To resume one, run `codemem db raw-events-doctor-retry <job-id>` and confirm the exact attempt snapshot. This command is intentionally interactive and has no `--json` mode.

## Seeing UI changes
- The viewer UI is built from `packages/ui/` and served by `packages/viewer-server/`.
- Rebuild UI assets after frontend changes: `pnpm --filter @codemem/ui build`.
- Restart the viewer after updates: `codemem serve restart`.

## Settings modal

The Settings modal is read-only for Slice 1 provider state. It renders the frozen safe capability
snapshot selected at daemon startup, including provider/model identity and pending privacy/schema/
pack reasons. It does not edit provider, auth, headers, tier routing, resource limits, or sweeper
cadence. Run `codemem setup` to compile and activate a replacement manifest.

Legacy JSON/JSONC and `CODEMEM_OBSERVER_*` values are inspected only by setup migration so each
recognized key can be rejected or recorded as translated/ignored/overridden. Daemon, maintenance,
viewer, and status paths never reread those values as effective runtime configuration.

## Observer auth configuration

Setup accepts exactly one complete OpenAI Chat Completions or Anthropic Messages endpoint, an exact
model ID/revision, and either no credential or one named environment-variable reference. Inline
keys, auth files, arbitrary headers, implicit token cascades, redirects, and TLS trust overrides are
not supported. PR1 keeps provider execution disabled as `pending_privacy_boundary`.

## Memory persistence
- A session is created per ingest payload.
- Observations and summaries persist when the observer emits meaningful content.
- Low-signal observations are filtered before writing.

## Reserved markup in memory text

Three tags are reserved. Hooks apply them before an event leaves your machine, so they
work in any text an adapter captures.

| tag | effect |
|---|---|
| `<private>…</private>` | The block is removed and the record is marked private. |
| `<local-only>…</local-only>` | The content is kept and the record is flagged local-only. See [What the local-only flag gates](#what-the-local-only-flag-gates). |
| `<injected-context>…</injected-context>` | The block is removed. Adapters wrap injected context in it so it is not captured back as your own writing. |

Matching is case-insensitive, and the tags nest.

### What the local-only flag gates

`<local-only>` removes nothing. It sets a flag on the record, and each surface decides what
to do with it. Today the flag keeps the content out of the memory packs that get injected
into your session, and blanks the prompt and working-set paths that the Claude and Codex
hooks would otherwise capture.

The flag also gates the raw-event flush that feeds the observer
([#130](https://github.com/ojungo69/free-mem/issues/130)): local-only content is never sent
to a remote or unverified summary provider. It may still be processed on this device by a
verified local provider serving the same repository. Read the flag as "this text does not
leave the device", not as "this text is never summarized".

### Private blocks and secrets

Marking something private does not exempt it from secret scanning. A secret written inside
a `<private>` block is still detected, and the record is then classified as containing a
secret — a stronger outcome than being marked private, because daemon intake drops the
content and keeps only the configured metadata, rather than keeping the surrounding prose.

### Malformed markup

Text you write is not always well-formed, so the rules for one-sided tags are explicit.
For the two tags that remove content, a tag with no partner means the extent of the block
is unknown, so the content around it is removed and a marker is left where it was:

| input | stored |
|---|---|
| `keep <private>secret` | `keep [private]` |
| `See </private> in the spec.` | `[/private] in the spec.` |

The marker is the point of it: a one-sided tag used to take the surrounding text with it
and leave nothing behind, so there was no way to tell a redaction from an empty note. The
text is still removed - `[private]` means "an opening tag with no close ended the text
here", `[/private]` means "a closing tag with no open removed what came before it".

`<local-only>` is different. It removes nothing and only flags the record, so a one-sided
`</local-only>` has nothing to protect: the tag is dropped and your prose is kept.

Secrets are scanned before reserved markup is removed and again after it, so markup cannot
be used to split a secret into halves that individually escape detection.

## Automatic context injection
- The OpenCode plugin injects a memory pack next to the latest user message by default, keeping older prompt prefixes stable for provider prompt caches.
- Controls:
  - `CODEMEM_INJECT_CONTEXT=0` disables injection.
  - `CODEMEM_INJECT_SURFACE=system` uses the legacy OpenCode system-prompt injection surface.
  - `CODEMEM_INJECT_LIMIT` caps memory items (default 8).
  - `CODEMEM_INJECT_TOKEN_BUDGET` caps pack size (default 800).
- Scope revocation affects newly built packs immediately, but already-injected context in the current OpenCode session is not retroactively scrubbed; start a new session after revoking access if you need a clean prompt history.
- Retrieval, skipped injection, current-request cache reuse, and handoff status are recorded in the local evidence ledger. Records contain bounded memory identity, diagnostic codes, and safe repository-relative working-set paths, never prompt text, pack text, memory content, or absolute paths. Reattaching historical cached context does not create attempts, and ledger failures do not block injection. If post-restart identity repair fails, usable fallback context is still injected without assigning its delivery to a stale or failed ledger attempt.
- Reuse savings estimate discovery work versus pack read size.

## Retrieval attribution diagnostics

Use `codemem stats --attribution` to inspect local, bounded, observational retrieval diagnostics:

```fish
codemem stats --attribution
codemem stats --json --attribution
```

- The report covers the 50 most recent retrieval attempts and at most 100 linked assessments; counts are a recent bounded window, not lifetime totals.
- It includes lifecycle completeness: requested, selected, and handed-off attempts and exposures.
- Evidence completeness distinguishes validated assessed attempts with known or unknown results (insufficient evidence) from unassessed attempts (no valid assessment row was inspected).
- Invalid rows failed current fail-closed validation. Omitted-by-limit rows exceeded the assessment cap; affected attempts are reported with indeterminate status or incomplete assessment details instead of inferred from unvalidated raw rows.
- Findings include counts of stale and harmful assessments.
- It contains no raw transcript, per-memory ROI, or composite productivity score. It makes no causal claim absent a preregistered randomized contrast.

## Semantic recall
- Embeddings are stored via sqlite-vec + fastembed.
- Embeddings are written automatically for new memories.
- Backfill existing memories with: `codemem embed --dry-run` then `codemem embed`.
- If sqlite-vec fails to load, semantic recall is skipped and keyword search remains.

## Distill recurring lessons

Use `codemem distill` to find lessons that keep showing up in memory history.

```fish
codemem distill --explain
codemem distill --all-projects --json
codemem distill --no-judge       # skip the observer-model worthiness judgment
codemem distill --draft          # draft an AGENTS.md rule for the top candidate + diff
codemem distill --draft --apply  # write it after confirmation
```

Candidate mining is deterministic and review-first:

- `project` candidates target that repo's `AGENTS.md`; `user` candidates target global/user context.
- Without `--draft`, the command only emits ranked candidates and evidence (`draft_text` is null).
- Candidates are judged by default: one short observer-model call per candidate drops clusters that are recurring *activity* (release/CI status, review passes with no findings, context lookups) rather than recurring *lessons* — recurrence alone cannot tell these apart. Unjudgeable candidates are kept and marked `unjudged`. When no observer model is configured, the command falls back to unjudged output with a warning; `--no-judge` skips the judgment (and its model calls) entirely.
- `--draft` uses your configured observer model to write one concise rule for the top candidate and prints a unified diff; it does not write anything.
- `--apply` (implies `--draft`) writes the rule into a codemem-managed `## Distilled lessons` block, delimited by `<!-- codemem:distilled:begin/end -->` markers so every distilled edit stays in one place. It prompts before writing (except with `--json`, which is non-interactive — there `--apply` itself is the explicit consent and the write happens immediately) and appends only (never deletes your existing notes).

## Projects, Sharing, Devices, and Health

### Choose a sharing flow

The normal flow is **Projects → Sharing → Devices → Health**, not manual pairing. Inside **Sharing**, open **Teams** when you want to manage ongoing Team membership and inherited Project access:

- **Team onboarding** — create or join a Team when people will collaborate over time.
  - Accepting the Team invitation links the recipient's Identity and device and inherits every current and future Project assigned to that Team.
  - The invitation does not create Project-to-Team assignments. Manage those separately, and review the Team's Projects before sending or accepting the invitation.
- **Direct Project sharing** — once Team sharing is configured, use **Share exact Projects** to invite one Identity to exact Projects without adding the recipient to the Team.
- **Add device** — invite another device for an existing Identity and review the Projects it will inherit from that Identity's direct and Team access.

Team membership organizes people and devices, but it is not permission to every Project—only Projects explicitly assigned to that Team. Project access remains explicit and uses canonical Project identity.

### Share exact Projects

For direct sharing, including after Team onboarding:

1. Choose **Create an invitation → Share exact Projects**.
2. Choose an existing **Identity** or enter the teammate's Identity display name.
3. Select the exact projects and review their existing-memory counts.
4. Confirm that the invite shares those existing memories and future activity, then send the one expiring invite.
5. The recipient reviews the invitation, accepts once, and confirms their Identity and device display names. Codemem links the Identity and device, establishes trust and Project access, and starts initial sync.

```text
Brian will receive:
• 436 existing memories and future activity from codemem

No other projects will be shared.
```

Project access uses canonical project identity, not a display name. Selecting `codemem` does not share a similarly named or sibling project in the same Space. **Only me** keeps a memory local, even when its project is shared.

The invite is single-use, expires, and is limited to the reviewed Projects. It names one Identity, not a Team, and the recipient cannot add Projects during acceptance. Existing and future selected-Project memories arrive after setup; unrelated Projects remain absent.

### Add a device for an existing Identity

Create an **Add device** invitation from the existing Identity. Before sending it, review the exact Projects the new device will receive:

- Direct Projects come from access granted to that Identity.
- Team Projects come from the Identity's Team membership.
- Existing Project exclusions remain excluded.
- The invitation cannot silently add unrelated Projects during acceptance.

The recipient accepts on the new device. Codemem links it to the same Identity, establishes the required trust, and starts initial sync for the reviewed Projects.

### Devices, status, and recovery

**Devices** is a read-only view of where Project access can arrive. Each device shows its **Owning Identity**. Projects are labeled **Direct** when shared with that Identity and **Team** when inherited through a Team policy; both are limited to exact canonical Projects selected in Sharing.

**Availability** tells you whether the device can currently receive work. It does not change ownership or Project access:

| Status | Meaning | What to do |
| --- | --- | --- |
| Waiting for acceptance | The invite has not been accepted. | Copy the invite or cancel it. |
| Setting up project access / Starting first sync | Codemem is establishing trust, access, and initial replication. | Wait. |
| Waiting for device | The recipient device is offline. | Wait; sync continues when it reconnects. |
| Up to date | The selected projects are syncing. | Nothing. |
| Needs attention | A setup step reached a terminal failure. | Use **Retry setup**. |

An offline device is a passive waiting state, not a failure or revocation. It keeps its current access and catches up after reconnecting. Retry only when codemem shows **Needs attention**; retry preserves completed setup work and resumes from the failed step.

Disabling a device enrollment for one coordinator group revokes future delivery only for Projects in that group. The global identity device remains active, stays in **Devices**, and can retain access granted through other groups. Use **Advanced → Team administration** to review or re-enable the affected group enrollment. Re-enabling clears the disabled state; the next owner reconciliation pass then restores only the Projects currently authorized through the Identity's direct shares and Team policies for that group. Delivery resumes without a broader re-invite, and unrelated Projects remain absent. A separate global identity-device revocation removes the device from the active list; it is not restored through the group enrollment action. Neither revocation nor disabling can delete memories already copied to a recipient device.

## Advanced operator and compatibility guidance

Use this section for same-person devices, existing integrations, diagnostics, or self-hosted coordination. These controls preserve internal compatibility; they are not required for the normal Projects → Sharing → Devices → Health workflow.

Legacy `#sync` and `#sync/diagnostics` viewer links remain valid Advanced routes. Saved Sync views and coordinator administration remain available through **Advanced**.

### Sync runtime

- `codemem sync enable` generates keys and writes config.
- `codemem serve start|stop|restart` manages the viewer-backed sync runtime.
- `codemem sync status` shows device info and peer health.

### Manual pairing

Use manual pairing for same-person devices, existing integrations, or compatibility—not normal teammate sharing.

1. In **Advanced**, open the Sync panel and scan/copy the QR payload (recommended).
2. Or run `codemem sync pair` and copy the payload.
3. On the other device, run `codemem sync pair --accept '<payload>'`.

Optional legacy filters can narrow an already-authorized peer's data; they cannot grant project access:

- `codemem sync pair --accept '<payload>' --include shared-repo-1,shared-repo-2`
- `codemem sync pair --accept '<payload>' --exclude private-repo`

### Product terms and internal access boundaries

Normal sharing uses product terms:

- A **Team** organizes collaborating people and their devices. Team membership can supply inherited access only to Projects explicitly shared with that Team.
- A **Project** is the exact canonical workspace selected for sharing, not every workspace with a similar display name.
- A **Space** is the user-facing access boundary that groups related Projects.

Advanced screens and diagnostics may call a Space a **Sharing domain**, a coordinator group an administrative container, and the stored boundary a `scope_id`. Those internal terms explain enforcement; users do not need them to share a Project or add a device. Coordinator-group membership alone never grants Project access.

Project filters narrow an already-authorized peer; they never grant Project access.

Use separate Sharing domains for personal, work, client, and OSS data on the
same machine:

| Example project | Recommended Sharing domain | Why |
|---|---|---|
| `personal/finance` | Personal | Private or same-person data should only sync to your own devices. |
| `work/acme-api` | Acme Work | Employer or team data should only sync to devices granted to that domain. |
| `oss/codemem` | OSS codemem | Public/open-source work can be shared with OSS peers without widening work access. |

Safe defaults:

- Unknown projects default to local-only until you map them.
- `Only me` keeps a memory local even if the project normally shares.
- Private same-person sync uses a personal Sharing domain, not a broad work or
  coordinator group grant.
- A peer's project include/exclude list can remove memories from sync, but it
  cannot add memories from a Sharing domain the peer is not authorized for.
- Broad mappings or basename collisions should be reviewed before you rely on
  them. If `codemem` exists under both work and personal paths, map the canonical
  workspace path/remote instead of trusting the basename.

For a mixed personal/work laptop, start conservatively:

1. Create or select one personal Sharing domain and one work/team Sharing
   domain in the Sync settings UI.
2. Map each known project to the smallest correct Sharing domain.
3. Leave unknown projects local-only until reviewed.
4. Pair peers normally, then confirm each peer card shows the expected
   authorized Sharing domains.
5. Use project include/exclude filters only to narrow what an already-authorized
   peer receives.

Do not treat coordinator-group membership as data access. A coordinator group can help discover and administer devices, but a device still needs Project access through a direct recipient or Team policy before it can receive those memories.

### Upgrade maintenance / Sharing-domain backfill

When upgrading an existing database to 0.30, codemem may run a one-time
Sharing-domain backfill. This stamps historical memories and sync bookkeeping
rows with `scope_id` so future sync and retrieval can enforce the new hard
boundary.

The progress total can be larger than the visible memory count because it
includes both `memory_items` and historical `replication_ops`. Large databases
can be CPU-bound while this runs. That is expected upgrade work; successful
completion should make later startups quieter.

Inspect current and completed maintenance jobs with:

```fish
codemem maintenance status
```

### Same-person device recovery

- In **Advanced → Sync**, use `Assigned actor` to map a peer to your local actor when that machine should count as part of your identity.
- Actor assignment preserves provenance and same-person UI continuity. Private sync still requires membership in a personal Sharing domain; actor assignment is not an access grant.
- If a machine is replaced or re-paired, use `Claim old device as mine` to reconnect older synced history to your local actor.

### Advanced actor management

- The Sync panel now has an `Actors` section for creating and renaming non-local actors.
- The same section can merge a duplicate actor into another actor; this immediately moves assigned peers, while already-stamped historical memories keep their current provenance until a later follow-on flow changes them.
- Assign each paired peer below to `Unassigned actor`, your local actor, or a named actor.
- Assigning a peer changes how older synced memories from that peer are attributed.
- Assigning a peer to a non-local actor keeps that peer's history attributed to that actor; assigning it to your local actor keeps provenance tied to you.
- Non-local peers receive memories only after Sharing-domain authorization succeeds. Their include/exclude filters can narrow that set, but cannot grant access.
- Use `Only me` on a memory when it should stay local and not sync to non-local actors.
- The Sync panel also shows a teammate review card with per-peer counts for memories that will share by default versus memories marked `Only me`, plus a one-click jump into `My memories` in the Feed for review.

### Compatibility, Spaces, grants, and reassignment

Legacy pairing and coordinator invitations remain supported, but do not grant selected-project access by themselves. Manual Space grants and project mappings are Advanced administration.

When selected history may already have replicated, all participating owner devices must support `reassign_scope` before codemem moves it into a project-specific boundary. If any required device lacks support, setup fails closed before partial migration; update that device, then use **Retry setup**. Technical capability details and IDs are available only in diagnostics.

### One-off sync

- `codemem sync once` syncs all peers once.
- `codemem sync once --peer <name-or-device-id>` syncs one peer.

### Autostart

- codemem does not ship a `sync install` helper in the TS CLI.
- Use an OS service manager to run `codemem serve start --foreground` at login/boot.
- Example service templates live in `docs/autostart/launchd/` and `docs/autostart/systemd/`.

### Diagnostics

- `codemem sync doctor` diagnoses sync configuration issues (keys, config, peer reachability).
- `codemem sync bootstrap <peer-device-id>` bootstraps sync state from a peer's snapshot.
- `codemem sync attempts` shows recent sync attempt history per peer.
- A restored peer requires its SQLite database and original signing key together.
  If no matching key exists in `device.key` or the configured platform keychain,
  sync fails closed with a `device_identity_*` diagnostic instead of silently
  replacing the enrolled key.
- The daemon records an `identity_error` state and retries without blocking local
  memory capture. Restore the original key, then restart the service if mDNS
  advertisement also needs to be re-established.
- See [Anchor-peer deployment](anchor-peer-deployment.md#storage-and-backups) for
  the complete backup and restore contract.

### Service helpers

- `codemem sync status` shows sync config and peer health.
- `codemem sync start|stop|restart` are deprecated — use `codemem serve start|stop|restart` instead. The viewer process manages the sync runtime; there is no separate sync-only daemon.

### Coordinator-backed discovery

- Use coordinator-backed discovery when peers are reachable but their addresses change frequently or mDNS does not work across network boundaries such as VPNs.
- Set `sync_coordinator_url` and `sync_coordinator_group` to enable it.
- The Settings UI exposes coordinator URL, group, timeout, and presence TTL fields under Device Sync.
- Use **Share** in Projects for normal teammate sharing. Manual project-to-Space assignment, grants, addresses, fingerprints, filters, epochs, and cursors are operator/compatibility details; Device Sync is for runtime configuration.
- The coordinator is self-hosted/operator-run and only helps peers discover fresh addresses; direct peer-to-peer sync remains the data path.
- See [docs/coordinator-discovery.md](coordinator-discovery.md) for setup, config, and current limitations.
- See [docs/anchor-peer-deployment.md](anchor-peer-deployment.md) if you want an always-on peer as a sync backstop for personal or team Sharing domains.
- Do **not** expose the viewer itself just because the coordinator or sync protocol needs cross-network reachability; those are separate surfaces.

### Keychain (optional)

- `CODEMEM_SYNC_KEY_STORE=keychain` stores the private key in Secret Service (Linux) or Keychain (macOS).
- Falls back to file-based storage if the platform tooling is unavailable.
- On macOS, the Keychain storage uses the `security` CLI and may expose the key in process arguments; use `CODEMEM_SYNC_KEY_STORE=file` if that is a concern.
- Keep the protected `device.key` file as the portable restore artifact even in
  keychain mode; codemem can repopulate the keychain from a matching restored
  file. A matching private key that remains in the platform keychain can also
  authenticate a local installation if `device.key` is missing, corrupt, or
  belongs to another identity. That is not a portable migration: moving a
  keychain-only credential requires platform-supported secure tooling. The
  database and public-key file alone cannot authenticate the original identity.

## Troubleshooting
- If sessions are missing, confirm the viewer and plugin share the same DB path.
- Check `~/.codemem/plugin.log` for plugin errors.
- Sync errors: `codemem sync status` shows the last error per peer.

### sqlite-vec / `no such module: vec0`

**Symptom:** API errors with `SqliteError: no such module: vec0`, or the viewer logs `sqlite-vec failed to load; retrying viewer startup with embeddings disabled` at startup.

`memory_vectors` is a sqlite-vec virtual table backed by the `vec0` extension module. The module is shipped as a per-platform npm sub-package (`sqlite-vec-darwin-arm64`, `sqlite-vec-linux-arm64`, `sqlite-vec-linux-x64`, `sqlite-vec-windows-x64`, `sqlite-vec-darwin-x64`) and selected automatically by npm's `optionalDependencies` resolution. It usually just works, but a few install layouts can leave the right binary missing.

Diagnose first:

```fish
# Confirm the architecture and the codemem install path
uname -m
which codemem
ls (npm root -g)/codemem/node_modules/ | grep -i sqlite-vec
```

You should see both `sqlite-vec/` (the wrapper) and `sqlite-vec-<platform>/` (the prebuilt binary). If the platform-specific package is missing, that's the bug.

Fixes, in order of preference:

1. **Reinstall codemem with optional deps explicitly included.** npm sometimes drops `optionalDependencies` for global installs:
   ```fish
   npm install -g --include=optional codemem@latest
   ```

2. **Force-install the platform package alongside.** If reinstalling didn't help (sometimes happens with global installs across major Node upgrades), install the matching platform sub-package separately and link it into codemem's tree:
   ```fish
   # 64-bit Pi OS / generic Linux ARM64
   npm install -g sqlite-vec-linux-arm64
   ln -sfn (npm root -g)/sqlite-vec-linux-arm64 \
           (npm root -g)/codemem/node_modules/sqlite-vec-linux-arm64
   # then restart the viewer
   ```
   Substitute the right platform: `sqlite-vec-linux-arm` for 32-bit Pi OS (`uname -m` reports `armv7l`), `sqlite-vec-linux-x64` for x86_64 Linux.

3. **Run with embeddings disabled.** Codemem degrades gracefully: keyword search via FTS5 keeps working, the viewer keeps loading, and the only feature you lose is semantic recall via vector similarity:
   ```fish
   set -Ux CODEMEM_EMBEDDING_DISABLED 1
   # then restart the viewer
   ```
   Reverse with `set -e CODEMEM_EMBEDDING_DISABLED`.

The viewer's startup retries automatically with embeddings disabled if the initial load fails (`sqlite-vec failed to load; retrying viewer startup with embeddings disabled` in the banner). If you see API errors with `no such module: vec0` AFTER that retry message, please file an issue — `getSemanticIndexDiagnostics` and other vec-touching code paths should be self-healing on a connection without `vec0`.

### Bootstrap grant failures

**Symptom:** worker bootstrap fails with HTTP 401 / `bootstrap_grant_invalid`.

The wire error is intentionally generic. Check the peer serving the bootstrap snapshot for the specific reason, then work through these:

1. **Is the coordinator reachable from the peer serving bootstrap?** That peer, not the worker, calls the coordinator's admin API to verify the grant. If the coordinator is down or unreachable from that peer, the grant cannot be verified and bootstrap will fail. Check network connectivity and `sync_coordinator_url` config on the serving peer.
2. **Is the grant expired or revoked?** List active grants with `codemem coordinator list-bootstrap-grants <group>` and confirm the grant is still valid.
3. **Does the grant's worker device match the bootstrapping device?** The `worker_device_id` on the grant must match the device ID of the worker attempting bootstrap. A mismatch (e.g., using a grant issued for a different worker) will be rejected.

## Retrieval scope
- New memories are stamped with the Sharing domain resolved from their project mapping; unmapped projects stay local-only.
- Owned feed items expose a visibility control so you can explicitly switch a memory between `Only me` and `Share with peers`.
- Choosing `Only me` keeps the memory local; choosing `Share with peers` keeps it eligible only for peers authorized for the memory's Sharing domain.
- The feed supports `All`, `Mine`, and `Theirs` scopes without splitting memories into separate databases.
- For non-local peers, Sharing-domain membership is the access boundary. Project and per-peer sync filters narrow the eligible set, and `Only me` acts as a per-memory override.

## Advanced Sync panel
- The `Actors` section gives actor creation/rename one home, while peer cards keep assignment close to the peer being changed.
- `Assigned actor` replaces the older `Belongs to me` language in the peer cards.
- Feed cards you own include a visibility control so shared/private intent can be changed without editing raw metadata.
- `Redact sensitive details` lives above Recent sync attempts so it is easier to find before you inspect peer addresses and attempt history.
- Recent sync attempts intentionally show only the latest few rows in the viewer; use CLI diagnostics for deeper history if needed.
