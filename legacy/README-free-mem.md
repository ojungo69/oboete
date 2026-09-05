# free-mem

> **Pre-release source.** free-mem does not yet publish a supported package or compatibility
> promise. Do not use this checkout with production secrets or irreplaceable memory data.

**Automatic local memory for Claude Code and Codex.**

free-mem is being reset around one product outcome: work normally in Claude Code or Codex, then
switch to the other Agent and receive the relevant decisions, discoveries, failed approaches,
changes, and next actions without writing a handoff document.

The target experience is inspired by claude-mem's automatic capture and context injection. The
implementation goal is a smaller, more predictable product with no Chroma/Python sidecar, bounded
background work, simple profiles, independent summary and embedding choices, and diagnostics that
state when retrieval is degraded.

## Current status

**Slice 1 implementation** ([#137](https://github.com/ojungo69/free-mem/issues/137)) is the active
work, building on the Product Reset M0 authority. The frozen capability manifest, durable schema
v21 processing jobs, and the privacy closure ([#130](https://github.com/ojungo69/free-mem/issues/130):
a single destination boundary over every content read, and restricted content never reaching a
remote summary provider) have landed; the bidirectional automatic memory flow, managed setup
lifecycle, and the real-hook artifact gate are next.

- Slice 1 runtime specification: [`specs/006-slice1-runtime/spec.md`](specs/006-slice1-runtime/spec.md)
- Product Reset specification: [`specs/005-product-reset/spec.md`](specs/005-product-reset/spec.md)
- Foundation decision: [`evidence/adr-006-product-reset.md`](evidence/adr-006-product-reset.md)
- Implementation plan: [`specs/005-product-reset/plan.md`](specs/005-product-reset/plan.md)
- GitHub routing ledger:
  [`specs/005-product-reset/issue-routing.md`](specs/005-product-reset/issue-routing.md)

The existing TypeScript workspace contains a tested safety kernel—daemon-owned writes, bounded
spool, redaction, local search, backup/restore, Claude/Codex hooks, CLI/MCP, and viewer assets—but
it is not yet a supported automatic-memory product. In particular, clean setup, automatic runtime
startup, model profiles, prompt-timed flushing, end-to-end diagnostics, packaging, and resource
evidence still need focused implementation.

## Technical Alpha boundary

Technical Alpha is deliberately narrow:

- Linux and WSL on a local Linux filesystem
- one local user alternating between Claude Code and Codex
- automatic capture, asynchronous summary, local storage, retrieval, and context injection
- lexical retrieval plus semantic retrieval when enabled and healthy
- a small resource profile plus independent summary and embedding provider choices
- explicit provider destination, credential source, cost class, and data-egress behavior
- truthful lexical fallback when a model or semantic index is unavailable
- doctor, minimal inspection/deletion, backup/restore, and a clean lifecycle

The Alpha does **not** promise safe operation replay, workspace reconciliation, checkpoint leases,
Rust Core, macOS/native Windows, additional Agents, broad remote MCP, Cloud sync, teams, or RBAC.
Those are reconsidered only after the local automatic-memory flow is useful to external users.

## Product shape

```text
Claude Code / Codex hooks
          │  bounded, fail-open capture
          ▼
local daemon ── durable queue/spool ── local memory store
          │
          ├─ asynchronous summary provider
          ├─ independent embedding provider
          └─ lexical + semantic candidate lanes
                            │
                 bounded InjectionPack
                            ▼
                    Claude Code / Codex
```

The durable store and lexical retrieval remain local and usable without Cloud. Remote providers
are opt-in and receive only the allowed redacted projection. Semantic failure never fabricates a
healthy empty result; the product falls back to the strongest available local result and reports
the reason.

## Delivery order

After M0, the Alpha is split into three focused specifications and pull requests:

1. **Automatic runtime path** — setup/start/doctor, one permanent minimal capability manifest,
   Claude-to-Codex and Codex-to-Claude capture, prompt-triggered flush, summary, lexical retrieval,
   inject, daemon/provider failure, and spool recovery.
2. **Profiles and explainable retrieval** — extend the Slice 1 manifest to multiple resource
   profiles and independent embedding providers, then add semantic lifecycle and bounded
   InjectionPack.
3. **Doctor and Alpha release** — inspection/deletion, full lifecycle, package, backup/restore,
   resource soak, and five-user external validation.

Only the first slice becomes implementation-ready after the Product Reset. Rust, Cloud, and extra
platform/Agent work do not run in parallel with it.

## Repository layout

- `vendor/codemem/`: pinned, modified Codemem workspace and current implementation base
- `specs/005-product-reset/`: active Product Reset specification, contracts, and tasks
- `evidence/`: foundation, safety, and decision evidence
- `specs/001-*` through `specs/004-*`: completed or historical pre-reset specifications
- `harness/`: existing capability and continuity evidence, historical unless a new slice explicitly
  reuses a bounded part

The v6 continuity documents, Rust-first ADR, continuity reference model, and broad capability rig
remain available as historical evidence. They are not active Product Alpha authority and must not
silently re-enter a new implementation slice.

## Development baseline

The current workspace uses Node.js 24.16.0 and pnpm 11.8.0.

```sh
cd vendor/codemem
corepack pnpm install --frozen-lockfile
corepack pnpm run build
CI=true corepack pnpm run check
```

At the Product Reset branch point, build, typecheck, and lint passed; Vitest passed 124 files and
1,895 tests with three existing todo tests.

The built CLI can be inspected from this checkout, but it is not a supported installation path:

```sh
node vendor/codemem/packages/cli/dist/index.js --help
```

Do not run editor setup against a real profile merely to evaluate this repository. Focused runtime
slices use isolated configuration and data directories until the Technical Alpha package exists.

## Release standard

The Alpha requires evidence for the complete user path, not feature presence alone:

- Claude-to-Codex and Codex-to-Claude automatic memory with no manual handoff
- zero Agent blockage, accepted-event loss, duplicate durable memory, secret egress, or
  incompatible-scope injection in the required failure set
- bounded capture, retry, queue, process, storage-growth, and injection behavior
- lexical fallback and visible degradation when summary or semantic providers fail
- clean install, update, backup, restore, rollback, doctor, and uninstall
- packed artifact, notices, checksums, SBOM, and protected publishing path
- external Alpha users completing first value without manual configuration-file editing

Broad superiority claims are prohibited until a pinned, reproducible comparison report supports
them.

## Future Pro lane

After the local Alpha is validated, free-mem may add encrypted Cloud sync, multi-device backup,
and a hosted viewer. These services must consume the local contracts and must never become a
dependency of local capture, search, or injection.

## Security

Report vulnerabilities privately as described in [`SECURITY.md`](../SECURITY.md). Never include real
credentials, private memory content, or local artifact paths in an issue or fixture.

## Licensing

free-mem is licensed under Apache License 2.0; see [`LICENSE`](../LICENSE) and [`NOTICE`](../NOTICE).
Third-party material keeps its original license. `vendor/codemem/` is a pinned MIT snapshot and is
not relicensed by this repository; see [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) and
[`vendor/codemem/VENDOR.md`](vendor/codemem/VENDOR.md).

Contributions use the same license and require DCO sign-off; see
[`CONTRIBUTING.md`](../CONTRIBUTING.md).
