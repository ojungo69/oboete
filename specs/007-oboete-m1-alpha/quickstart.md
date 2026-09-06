# Quickstart: validating oboete M1

## Prerequisites

- Node.js 22.16 or newer for the engine; Node.js 24.x for the four-agent E2E (Pi 0.84.4 requires
  >= 22.19).
- For every probe and E2E run: a separate Linux user (`oboete-dogfood`) on this WSL host with its
  own home and its own logins for all four agents. The maintainer's own agent environment is never
  used (FR-041); a temporary `--home` is not isolation because Grok Build reads Claude-compat hooks
  from `$HOME`.

## Build, lint, and tests

```bash
npm ci
npm run typecheck          # tsc --noEmit
npm run lint               # eslint
npm run build              # esbuild: dist/oboete.mjs, build/test/*.mjs, embedded viewer assets
npm test                   # node --test build/test with coverage (unit, migration smoke, contracts)
npm run pack-check         # npm pack, install the tarball into an empty prefix, measure unpacked size (<= 30 MB)
```

Expected: green on 22.16 and 24.x; `pack-check` prints the installed size including dependencies.

## Verification gate (research R13)

```bash
sudo -u oboete-dogfood -H env PATH="$PATH" node scripts/e2e/probe-contracts.mjs   # appends results to docs/research/
```

Expected: a dated section per probe item, one per row of the R13 table (tool payload fixtures,
Codex and Grok PostCompact payload and Grok Stop lastAssistantMessage, Grok parallel-batch
delivery and PermissionDenied payload, detector 1 MB inside the cutoff, Codex SessionStart
sources and rollout flush, Grok MCP registration, Pi compaction event and tool registration,
provider transport/auth/model and structured output, Grok context on a failed call, Pi and Grok
resume continuity, runner behaviour on unread stdin above 1 MB, Pi durable error surface, MCP
legacy server against each client with raw frames, per-model context windows, bundle cold start,
installed size). A failed probe
prints the R13 row's consequence; dependent implementation tasks stay blocked until the owner
approves an amendment or the row passes.

## Fixture replay (SC-002, SC-003, SC-005, SC-009, SC-010)

```bash
node scripts/fixtures/generate-1000-events.mjs
OBOETE_HOME=$(mktemp -d) node dist/oboete.mjs fixture replay test/fixtures/events-1000.jsonl
```

Expected (`docs/evidence/m1-resource-envelope.md`): capture-hook p99 under 300 ms with at least
99% under budget, including payloads at and above the 1 MB stdin cap; session-start injection
measured on both the ready path (under 300 ms) and the pending path (under 1 s); worker peak RSS
under 150 MB; database growth per 1,000 events; zero secret corpus items in the database, spool,
logs, packs, or outbound bodies; zero directive corpus phrases in stored memories, accepted
observer output, or packs (they legitimately remain in raw events and the spool); at least 90% of
seeded Japanese and English facts retrieved; zero duplicate injections per (conversation, context
epoch); on Grok, `why` shows one confirmed delivery per pack (or, only under an approved A15, the
counted per-call deliveries of a parallel batch).

## Failure injection (User Story 2)

```bash
for f in db-missing busy corrupt readonly enospc worker-kill provider-unreachable provider-hang \
         provider-429-3036 provider-403-5035 provider-length provider-malformed cap-boundary \
         lease-lost-after-3036 detector-throw config-malformed pi-throw pi-child-hang pi-spawn-failure clock-jump \
         mixed-sensitivity resume compact fork clear setup-repeat setup-remove lease-steal pause \
         oversized-payload detector-never-returns consent-changed provider-401 provider-wrong-language \
         update-sensitivity tombstone-identical-content worker-kill-after-response agent-swap remote-no-duplicate \
         grok-success grok-exec-failure grok-oboete-deny grok-other-handler-deny grok-parallel-batch grok-no-tool; do
  NODE_ENV=test OBOETE_TEST_FAULT=$f node --test build/test/fault-*.test.mjs
done
```

Expected: every hook exits 0 within its absolute deadline (process wall time asserted per event
kind, including a 1 MB input and a detector that never returns), spooled events are recovered
when the fault clears, no batch is applied twice and a worker killed after the provider responded
but before apply causes exactly one extra call and one apply (HTTP count and apply count asserted
separately), a changed consent tuple makes no call, a 401 is reported as `auth_failed` not
`provider_paid`, English output for Japanese input is retried once then replaced by the fallback,
an eligible update never relaxes a local-only target, identical deleted content is never
re-created on either path, a remote preset never yields the same observation from the fallback
batch, a lost lease stops the old worker's writes but the 3036 signal persists,
attempt 150 is allowed and attempt 151 is refused across presets, a detector failure stores metadata only, the mixed-sensitivity outbound
body contains only eligible content and an opaque repository id, Grok cases deliver exactly the
expected number of packs, an oversized payload leaves one partial row that holds only the
redacted prefix inside the read bound (A7, A14), and doctor names the degraded component with `reason`,
`consequence`, and `recovery`.

## Setup and doctor on the isolated account (User Story 4, SC-008)

```bash
sudo -u oboete-dogfood -H env PATH="$PATH" bash -lc '
  npm install -g ./oboete-<version>.tgz &&
  time oboete setup --agents claude,codex,grok,pi --provider workers-ai --accept-egress &&
  oboete doctor --probe-provider --json'
```

Expected: setup completes in under 2 minutes (probes run in parallel) and reports each agent as
wired with a passed probe and its trust state; re-running setup leaves the foreign configuration
files byte-identical outside the managed blocks with mode and owner preserved; `--remove` restores
them; `--yes` is refused when the consent tuple changed; doctor reports every item healthy; break
one item at a time (remove a hook entry, chmod the database, corrupt the database header, kill the
worker, point the provider at an unreachable host, set the allowance counter to exhausted, leave a
stale Pi `.started` file, break the Pi extension so it cannot spawn) and confirm doctor names it
with a `reason`, the user-facing `consequence`, and a `recovery` step that, when followed, turns
the item green again.

## Cross-agent memory (User Story 1, SC-001, SC-004)

```bash
sudo -u oboete-dogfood -H env PATH="$PATH" node scripts/e2e/isolated-user.mjs --pairs all
```

The harness seeds three facts in a synthetic repository with agent A (headless: `claude -p`,
`codex exec`, `grok -p`, `pi -p`), then starts agent B in the same repository with a prompt that
forces one tool call and asks for the facts. Expected: 12 of 12 ordered pairs recall all three facts
on the first turn (Grok Build receiving: by the first tool result); the same run with credentials
removed still passes with `Degraded:` lines in every pack.

## Privacy (User Story 3, SC-005, SC-006)

```bash
npm test -- --test-name-pattern "privacy"
```

Expected: fail-closed tests block secret and local-only rows from the remote observer (events,
nearby candidates, citations, repository metadata) and cross-repository injection; fail-open tests
deliver eligible rows; eligibility decisions are identical when only the producing agent changes;
credential values never appear in logs, spool, doctor output, or packs.

## Viewer and MCP (User Story 6, SC-011)

```bash
oboete view      # prints http://127.0.0.1:<port>/?token=...
sudo -u oboete-dogfood -H env PATH="$PATH" node scripts/e2e/mcp-clients.mjs   # tools/list + tools/call through each agent's MCP client, raw frames recorded
```

Expected: a memory created by the worker appears in the viewer within 2 seconds as unreviewed;
review, pin, delete, and search work; the URL without the token and any non-loopback bind are
refused; each supported client lists and calls `search`, `timeline`, `get`; a `repo` argument is
rejected.

## Export / import (User Story 7)

```bash
oboete export > memories.jsonl
OBOETE_HOME=$(mktemp -d) oboete import memories.jsonl --dry-run
```

Expected: counts match, tombstones are preserved with their original hash, an imported row never
lowers sensitivity and lands as `local_only` / `imported`, a body whose hash does not match, an
oversized line, or a malformed line is rejected with exit `2`; imported rows are absent from
`search` and packs until the worker has classified them, and a tombstone imported under
`--map-repo` still suppresses the same content.

## Dogfood gate (SC-007)

Run `scripts/e2e/isolated-user.mjs --daily` on the isolated account once a day for 7 consecutive
days; each run records doctor output, provider usage, spool backlog, duplicate count, and viewer
latency under `docs/evidence/m1-dogfood.md`. M1 is a "done" candidate only after 7 green days;
installation into the maintainer's environment is a separate approval afterwards.
