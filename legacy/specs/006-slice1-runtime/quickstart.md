# Quickstart: Validate Slice 1 Automatic Memory Runtime

## Prerequisites

- Linux or WSL on a local Linux filesystem.
- Node.js 24.16.0 and pnpm 11.8.0.
- Synthetic isolated home/config/data/artifact/evidence directories. Never use real memory content or
  credentials.
- The exact Claude Code/Codex versions pinned by the corrected Product Reset fixture for the final
  real-hook gate.

## 1. Planning artifact and task syntax gate

```bash
specify init --here --force --non-interactive --integration codex
install -m 0644 CONSTITUTION.md .specify/memory/constitution.md
cmp CONSTITUTION.md .specify/memory/constitution.md
specify self check
specify integration status
SPECIFY_FEATURE_DIRECTORY=specs/006-slice1-runtime \
  .specify/scripts/bash/check-prerequisites.sh --json --require-tasks --include-tasks
node --input-type=module <<'NODE'
import { readFileSync } from "node:fs";
const path = "specs/006-slice1-runtime/tasks.md";
const lines = readFileSync(path, "utf8").split("\n");
const taskLines = lines.filter((line) => /^- \[[ x]\] T/.test(line));
const ids = taskLines.map((line) => {
  const match = /^- \[[ x]\] T([0-9]{3})(?: \[P\])?(?: \[US[1-4]\])? .+/.exec(line);
  if (!match) throw new Error(`invalid task syntax: ${line}`);
  return Number(match[1]);
});
if (ids.length !== 60) throw new Error(`expected 60 tasks, found ${ids.length}`);
for (let index = 0; index < ids.length; index += 1) {
  if (ids[index] !== index + 1) throw new Error(`task sequence breaks at ${ids[index]}`);
}
console.log("PASS tasks=60 range=T001-T060");
NODE
```

Then run the non-destructive `$speckit-analyze` skill with these exact inputs:
`spec.md`, `plan.md`, `tasks.md`, `research.md`, `data-model.md`, `contracts/*.md`,
`quickstart.md`, and `checklists/requirements.md` under `specs/006-slice1-runtime/`. Resolve every
CRITICAL/HIGH finding before implementation and report any remaining MEDIUM/LOW.

Expected: Spec Kit resolves `specs/006-slice1-runtime`; task syntax reports exactly T001-T060 with
no gaps/duplicates; no unresolved placeholder, clarification marker, unchecked checklist item, or
CRITICAL/HIGH analysis finding remains.

## 2. Contract/fixture correction checkpoint (PR 0)

```bash
node --experimental-strip-types specs/005-product-reset/fixtures/validate-slice1-fixture.mjs
node --test specs/005-product-reset/contracts/validate-slice1-fixture.test.mjs
node --test specs/005-product-reset/contracts/validate-alpha-result-input.test.mjs
node --test specs/005-product-reset/contracts/validate-alpha-result-render.test.mjs
node --test specs/005-product-reset/contracts/validate-alpha-runner-evidence.test.mjs
node --test specs/005-product-reset/contracts/validate-alpha-result.test.mjs
node --test specs/005-product-reset/contracts/validate-alpha-result-failure.test.mjs
```

Expected after the co-delivered mechanical correction: the fixture/schema/semantic validator and bound
success/failure/suite evidence use closed wire protocols, complete canonical endpoint URLs,
CredentialRefV1, computed provider/manifest fingerprints, fixed resource fields, and normal
provider-proposal metadata for later harness materialization. Local-derivation and output-limit cases
are complete successor manifests rather than partial overlays. Result/runner schemas represent and
fingerprint the 12 resource windows for executed results, use a null plateau object/fingerprint for
canonical no-activity results, and bind CA proof, identity conflict, sensitivity-byte totals no
greater than observed payload bytes, zero runner-owned restricted/sentinel observations, and the
exact 16+1 suite; recovery
signals enforce exact producer kind, sequence, and 0→1→0 grant CAS. Every command exits 0.

This checkpoint proves static shape, successor/signal fingerprint binding, and bound examples only.
It does not prove a live stub, TLS, socket/request bytes, or runtime materialization; those belong to
section 9.

PR 0 co-delivers these 006 planning artifacts with the scoped 005 correction. Do not begin runtime
work until that checkpoint merges.

## 3. Vertical manifest gate (PR 1)

```bash
cd "$(git rev-parse --show-toplevel)/vendor/codemem"
corepack pnpm install --frozen-lockfile
corepack pnpm exec vitest run \
  packages/core/src/capability-manifest.test.ts \
  packages/core/src/setup-internal.test.ts \
  packages/core/src/storage.test.ts \
  packages/core/src/observer-auth.test.ts \
  packages/core/src/observer-client.test.ts \
  packages/core/src/observer-config.test.ts \
  packages/core/src/extraction-replay.test.ts \
  packages/core/src/distill.test.ts \
  packages/core/src/ingest-pipeline.test.ts \
  packages/core/src/index.test.ts \
  packages/core/src/maintenance/ai-structured.test.ts \
  packages/core/src/viewer-routes/config.test.ts \
  packages/core/src/viewer-routes/observer-status.test.ts \
  packages/core/src/daemon-lifecycle.test.ts \
  packages/core/src/daemon-rpc.test.ts \
  packages/core/src/operational-status.test.ts \
  packages/core/src/raw-event-flush.test.ts \
  packages/core/src/raw-event-sweeper.test.ts \
  packages/core/src/vectors.test.ts \
  packages/cli/src/commands/setup-codex.test.ts \
  packages/cli/src/commands/setup-config.test.ts \
  packages/cli/src/commands/status.test.ts \
  packages/ui/src/tabs/settings/components/ObserverStatusBanner.test.tsx \
  --maxWorkers=1 --no-file-parallelism
```

Required assertions:

- only the two wire protocols and complete canonical endpoint URLs compile;
- local HTTP credential-none/eligible-only, literal-loopback HTTPS restricted-peer, remote HTTPS,
  TLS/redirect/CredentialRef decisions, and both fingerprints are exact;
- Anthropic/OpenAI headers, request/response shapes, credential-none behavior, 60 s timeout,
  exact UTF-16 12,000-unit system/user allocation, 4,000-token output, 1 MiB response, and
  temperature 0.2 are frozen;
- setup displays safe fields and confirms before one Claude+Codex+pointer transaction;
- shared lifecycle-lock interleaving cannot start a daemon between health preflight and activation;
  invalid TLS chain/hostname fails the 5 s credential/payload-free handshake with zero mutation/
  request/credential bytes, and daemon start revalidates it;
  legacy conflict and running daemon mutate nothing; rollback restores every touched file/pointer;
  prepared/applied/committed journal interruptions recover, while any target matching neither
  recorded prestate nor journal poststate causes zero mutation across all targets and retains the
  journal; unresolved recovery blocks provider startup without exposing prestate bytes;
- absent current starts capture-only; malformed current fails startup;
- daemon-start TLS outage/rejection still starts writer/RPC/capture/spool-import/lexical, disables
  provider/AI only with a bounded reason and retains work;
- valid current freezes one snapshot but remains `pending_privacy_boundary` with no executable
  provider, AI maintenance, or Sweeper until PR 3;
- legacy provider/resource env mutation after startup changes no effective behavior;
- scheduler uses 30 s/120 s/1 s/5 min/100/retention-off profile fields and does not claim v21/pack
  readiness early.
- the only resource successor is version 2 with derivation limit 17; every other field/profile
  override is rejected.

## 4. Schema v21 and durable job gate (PR 2)

```bash
cd "$(git rev-parse --show-toplevel)/vendor/codemem"
corepack pnpm --filter @codemem/core run generate:test-schema
corepack pnpm exec vitest run \
  packages/core/src/db.test.ts \
  packages/core/src/test-schema.generated.test.ts \
  packages/core/src/store.test.ts \
  packages/core/src/raw-event-flush.test.ts \
  packages/core/src/raw-event-sweeper.test.ts \
  packages/core/src/daemon-rpc.test.ts \
  packages/core/src/operational-status.test.ts \
  packages/cli/src/commands/db.test.ts \
  packages/cli/src/commands/status.test.ts \
  --maxWorkers=1 --no-file-parallelism
```

Expected:

- fresh v21 and verified v20→v21 migration have final DDL parity; failed migration leaves v20;
- legacy source/stream/event uniqueness is removed and repository/source/stream/event uniqueness uses
  index-only `COALESCE(repository_identity,'repo-v1:unknown')`; NULL stays stored and unauthorized,
  repeated unknown collisions fail closed rather than bypassing SQLite uniqueness; every collision
  is durably retained as a secret redacted-payload/digest quarantine receipt with no normal ACK,
  canonical admission, job, or memory;
- same event identity/digest is idempotent; a different digest preserves the canonical event,
  creates/reuses one durable non-success conflict receipt, sends no normal ACK, and creates no memory;
- one newly admitted v21 job has at most 100 source events; a wider migrated legacy recovery range
  remains intact; capacity 25 includes retry-exhausted and evicts nothing;
- claim generation rejects stale completion; successful claims alone increment lifetime attempt count;
- new jobs start at attempt 0; automatic claims are exactly attempts 1-3, and a failed attempt 3 is
  retry-exhausted until a one-shot valid grant;
- admission manifest/provider fingerprints never change; changed config creates a new attempt
  fingerprint and one exact-job grant creates one claim;
- invalid signals create no claim/attempt; timer never resumes exhausted work;
- setup activation and provider unhealthy→healthy receipts fan out crash-idempotently to at most 25
  currently matching exhausted jobs, while explicit doctor retry targets exactly the displayed job;
  each signal binds job+producer receipt with unique pairs and the healthy edge resumes retained
  provider work only after this PR2 state exists;
- privacy skip and memory+batch+frontier each commit atomically; failure moves no frontier;
- retention is disabled/0 and future purge exempts all uncompleted job ranges;
- exact complete legacy `gave_up` may recover without frontier change; missing/ambiguous becomes
  terminal `legacy_unrecoverable`, consumes no capacity, never reports success, and never rewinds.

## 5. Complete privacy gate (PR 3 / #130)

```bash
cd "$(git rev-parse --show-toplevel)/vendor/codemem"
corepack pnpm exec vitest run \
  packages/core/src/project.test.ts \
  packages/core/src/normalized-event.test.ts \
  packages/core/src/raw-event-flush.test.ts \
  packages/core/src/ingest-pipeline.test.ts \
  packages/core/src/ingest-prompts.test.ts \
  packages/core/src/ingest-xml-parser.test.ts \
  packages/core/src/store.test.ts \
  packages/core/src/search.test.ts \
  packages/core/src/ref-queries.test.ts \
  packages/core/src/pack.test.ts \
  packages/core/src/prompt-pack-ledger.test.ts \
  packages/core/src/vectors.test.ts \
  packages/core/src/daemon-rpc.test.ts \
  packages/core/src/observer-client.test.ts \
  packages/core/src/maintenance/ai-structured.test.ts \
  packages/core/src/maintenance/memory-role-report.test.ts \
  packages/core/src/viewer-routes/raw-events.test.ts \
  packages/core/src/viewer-routes/stats.test.ts \
  packages/core/src/viewer-routes/memory.test.ts \
  packages/core/src/export-import.test.ts \
  packages/core/src/mutation-dispatcher.test.ts \
  packages/core/src/index.test.ts \
  packages/core/src/backup-restore-smoke.test.ts \
  packages/core/src/capability-manifest.test.ts \
  packages/core/src/daemon-lifecycle.test.ts \
  packages/core/src/raw-event-sweeper.test.ts \
  packages/mcp-server/src/rpc-client.test.ts \
  packages/mcp-server/src/server.test.ts \
  packages/cli/src/commands/cli-rpc.test.ts \
  packages/cli/src/commands/memory.test.ts \
  packages/cli/src/commands/pack.test.ts \
  --maxWorkers=1 --no-file-parallelism
```

Required matrix:

- eligible/local-only/private/secret/degraded/legacy-unknown × remote/local/unknown × same/cross/
  unknown repository;
- provider peer trust `verified|unverified|not_applicable`, proving local HTTP restricted-ineligible
  and verified local HTTPS same-repository restricted-eligible without fingerprint lookup;
- bounded Git-probed transport-preserving HTTPS/SSH origin with exact SSH-user non-collision,
  realpathed common-dir fallback, linked worktree,
  basename collision, forged remote/project/workspace claims, and origin A→B revalidation that
  prevents A's restricted content from matching B;
- provider, structured maintenance, search/recent/timeline/explain, findByFile/findByConcept, daemon
  get/search/pack, MCP body/index/recent/timeline/explain/pack, viewer, lexical/semantic pack/trace,
  export/import, dedup/supersession;
- Claude/Codex/MCP caller location/model claims remain remote/unknown; only runner-bound loopback
  evidence selects a local destination;
- viewer gates raw-event/status/usage, memory, prompt, legacy-summary, artifact, and safe-session;
- export v2 gates memory items, prompts, and legacy summaries; session shells omit cwd/Git remote/
  branch/user/free-form metadata; legacy v1 import content becomes secret/unknown;
- all-restricted provider requests/bytes 0; mixed provider eligible-only in order;
- every new claimed provider item has exactly one direct non-empty `citations` child; canonical
  ordinal whole-event cites and optional half-open UTF-8 canonical-payload spans resolve through the
  Store-private claim projection, survive parse/repair, and persist exact IDs/spans;
- missing/empty/duplicate/noncanonical/out-of-range ordinals, forbidden provider ID/repository/digest
  authority, one-sided/malformed/out-of-bounds/code-point-split spans, caller-forged projection,
  claim/source/boundary drift, stale claim, out-of-set/mixed-repo citations, and provider-backed
  no-claim ingest commit nothing; same-response duplicate anchors reject atomically; later retries
  deduplicate exact active anchors, quarantine active overlap, suppress tombstoned overlap, and allow
  only disjoint spans to become distinct sibling provenance;
- output above the active attempt's `maxMemoryItemsPerDerivation` commits nothing; version 1 rejects
  17 outputs, test-only version 2 accepts 17, and both reject 18 or more;
- restricted content is absent before render/measure/serialize and from logs/diagnostics/traces;
- semantic-disabled vector rows are unchanged;
- public core barrel exports neither unrestricted extraction replay nor distill corpus/report APIs.
- only after the full matrix is installed does daemon lifecycle enable the manifest-derived
  Observer/AI maintenance/Sweeper; no prerequisite PR can enable #130 early.

Issue #130 is not ready to close unless this entire matrix and the full/packed gates below pass.

## 6. Triggered bidirectional lifecycle gate (PR 4)

```bash
cd "$(git rev-parse --show-toplevel)/vendor/codemem"
corepack pnpm exec vitest run \
  packages/core/src/ingest-xml-parser.test.ts \
  packages/core/src/ingest-pipeline.test.ts \
  packages/core/src/store.test.ts \
  packages/core/src/pack.test.ts \
  packages/core/src/prompt-pack-ledger.test.ts \
  packages/core/src/raw-event-sweeper.test.ts \
  packages/core/src/daemon-rpc.test.ts \
  packages/core/src/claude-hooks.test.ts \
  packages/core/src/normalized-event.test.ts \
  packages/cli/src/commands/claude-hook-inject.test.ts \
  packages/cli/src/commands/codex-hook-inject.test.ts \
  --maxWorkers=1 --no-file-parallelism
```

Expected: fixed kinds/provenance/no-op and final pack limits pass; newly committed events nudge once,
duplicates do not; debounce/immediate/request drains are bounded; PreCompact is captured; stop waits
active work and no pending `finally`/timer starts post-stop work; explicit restart works; source and
packed Claude⇄Codex scenarios contain exact required and zero forbidden facts.

## 7. Managed setup and doctor gate (PR 5)

```bash
cd "$(git rev-parse --show-toplevel)/vendor/codemem"
corepack pnpm exec vitest run \
  packages/cli/src/commands/setup-codex.test.ts \
  packages/cli/src/commands/setup-config.test.ts \
  packages/cli/src/commands/serve.test.ts \
  packages/cli/src/commands/status.test.ts \
  packages/core/src/daemon-lifecycle.test.ts \
  packages/core/src/daemon-rpc.test.ts \
  --maxWorkers=1 --no-file-parallelism
corepack pnpm --filter codemem test:packed-artifact
```

Expected: unsupported storage mutates nothing; setup installs Claude+Codex, safely coordinates
stop/activate/start or matching attach, rejects mismatch, and verifies version/fingerprint/doctor;
restart/stop/uninstall creates no duplicate writer; doctor reports manifest/provider, writer,
mutation, spool, capacity/jobs, summary, lexical, semantic-disabled, hook, and pack readiness from
runtime facts.

## 8. Full workspace and generated-artifact gates

```bash
cd "$(git rev-parse --show-toplevel)/vendor/codemem"
corepack pnpm run build
corepack pnpm run tsc
corepack pnpm run lint
CI=true corepack pnpm run test:coverage -- --maxWorkers=1 --no-file-parallelism
corepack pnpm run phase1:no-agent-blockage
corepack pnpm run phase1:backup-restore-smoke
corepack pnpm --filter codemem test:packed-artifact
cmp plugins/claude/scripts/hook-runtime.mjs plugins/codex/scripts/hook-runtime.mjs
cd "$(git rev-parse --show-toplevel)"
CODEMEM_INDEX="$PWD/vendor/codemem"
gitnexus analyze --index-only --skip-skills --skip-git "$CODEMEM_INDEX"
gitnexus impact startDaemon --direction upstream \
  --repo "$CODEMEM_INDEX" --file packages/core/src/daemon-lifecycle.ts \
  --kind Function --depth 3 --include-tests --summary-only
gitnexus impact flushRawEvents --direction upstream \
  --repo "$CODEMEM_INDEX" --file packages/core/src/raw-event-flush.ts \
  --kind Function --depth 3 --include-tests --summary-only
gitnexus impact buildMemoryPack --direction upstream \
  --repo "$CODEMEM_INDEX" --file packages/core/src/pack.ts \
  --kind Function --depth 3 --include-tests --summary-only
for store_read in get recent recentByKinds search timeline findByFile findByConcept; do
  gitnexus impact "$store_read" --direction upstream \
    --repo "$CODEMEM_INDEX" --file packages/core/src/store.ts \
    --kind Method --depth 3 --include-tests --summary-only
done
git diff --check
```

Expected: all commands exit 0; generated hook runtimes are byte-identical; no restricted sentinel is
present in generated artifacts/output/logs/diagnostics; no spool, redaction, sole-writer, backup,
semantic-retention, or packed-artifact regression exists.

## 9. Pinned real-hook runner (PR 6)

```bash
node --test harness/slice1-runtime.test.mjs
node harness/slice1-runtime.mjs \
  --fixture specs/005-product-reset/fixtures/slice1-bidirectional-en-v1.json \
  --candidate vendor/codemem
```

Expected: the corrected fixture's stub metadata materializes only normal provider proposals. Both
Agent directions, manifest absence/malformed state, outage/replay, failure/grant, stale claim/
capacity, all privacy consumers, duplicate/no-op, linked worktree/path-with-spaces, unsupported
environment, packed setup/runtime, and semantic-disabled retention emit validator-accepted evidence.

The runner emits exactly 16 positive observations plus the late-injection negative and repeated
pair-bound same-event-ID/different-payload-digest conflict attempts without overwrite. The closed result and
runner-evidence schema carries all 12 plateau windows, drain/checkpoint receipts, item/token/
concurrency samples, public CA proof, and six raw base/local/repaired setup/start TLS receipts with exact
SNI/timeout/timing/verified/trust-anchor/phase-stable-peer-cert/zero-request/credential/payload
evidence, with setup completion strictly before daemon-start beginning and the network object plus
all receipts bound to the bundle invocation. The plateau object binds candidate/artifact/environment/
invocation and a fresh bundle-unique process root. Every plateau window has a unique workload receipt,
the same positive duplicate-attempt count, no-op outcome, and
zero memory/job delta, plus a strict non-overlapping workload-start/workload-receipt/drain-receipt/
checkpoint-receipt/sample timestamp chain. Results carry separate network/plateau fingerprints and derived aggregates;
all raw fields remain in the candidate-inaccessible runner bundle.

Every real scenario also has one runner-owned provider-egress observation. The monitor is armed
before candidate start and remains through process-tree termination; its network gate opens only
after a direct durable-event-set authorization. It binds active provider/location, first/last
request times, explicit canonical-order committed event IDs/count/fingerprint (including non-prefix
sets), exact wire aggregates, source bytes by sensitivity, and zero pre-authorization or
non-loopback attempts. The late-injection negative projects the base receipt.
Sensitivity-byte counts come from the runner stub's actual received request bytes matched to fixed
synthetic markers/spans; policy-derived or candidate-reported counts fail the self-test.
Every fixed retry/redirect recovery subcase also has one sorted, case/manifest-bound full
observation under the same rules. No-op subcases carry full zero-egress observations, and all initial
and recovery receipt/process-tree IDs are bundle-global unique. Each receipt binds the bundle
invocation, its owning case ID, and its fresh per-execution/process-generation root; reusable PID
labels fail. The sum of each receipt's sensitivity-byte buckets never exceeds its observed payload;
its runner-owned restricted-payload bytes and forbidden-sentinel count are zero; canonical
unsupported/not-run output carries no plateau workload or plateau fingerprint.

Restricted local proposals use the fixture-pinned literal-loopback HTTPS URL. Remote proposals use the fixture-pinned
base/repaired HTTPS hostnames mapped only inside the runner namespace to its loopback stub, with a per-run
hostname/IP-valid public test CA installed into isolated system trust before candidate start.
Production rejects added CA path/environment configuration. Evidence binds the public CA fingerprint;
no private key is committed. Bind, CA, hostname, or fingerprint drift fails.
Base remote uses environment name `FREE_MEM_SUMMARY_API_KEY` with derived `external_metered`; local
uses `https://127.0.0.1:1234/v1/chat/completions` and credential `none`. The runner records actual stub
cost 0 independently of cost class and uses the production pure compiler, not a harness compiler.

External-egress-disabled cases record zero non-loopback socket attempts and zero remote restricted
bytes. Base/repaired remote HTTPS alone may match environment-credential and eligible-payload bytes;
verified local HTTPS is credential-none with zero credential bytes and may match expected request plus
eligible/private/local-only payload bytes. Latency
uses ordinals 1-22, discards 1-2, and applies nearest-rank p95 separately. The resource gate runs 12
identical duplicate/no-op windows with strict non-overlapping workload/drain/checkpoint/sample order,
discards 1-2, enforces absolute ceilings
on 3-12, and requires windows 8-12 to have constant process count, zero drained queue, identical
item/token counts, RSS span at most 16 MiB, storage span at most 65,536 bytes, concurrency at most 2,
and zero post-teardown orphan process. Missing pins/samples, inaccessible roots, interrupted
lifecycle, or incomplete evidence emits no success.
