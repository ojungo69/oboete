# Third-party notices

## codemem

- Upstream: [`kunickiaj/codemem`](https://github.com/kunickiaj/codemem)
- Snapshot: `26438e75ce1d0fec6be34981f15045a15c89658b` (upstream v0.40.2 equivalent)
- Location: `vendor/codemem/`
- License: MIT
- Copyright: 2026 Adam Kunicki

The upstream license text is retained at
[`vendor/codemem/LICENSE`](vendor/codemem/LICENSE). Local provenance and update
policy are recorded in [`vendor/codemem/VENDOR.md`](vendor/codemem/VENDOR.md).

This snapshot stays under MIT. The repository's own Apache-2.0 grant does not extend
to it, and free-mem headers are not added to files under `vendor/codemem/`.

## Dependency licenses

The dependency tree is not vendored — it is resolved from the lockfile at install time.
The inventory below was measured on 2026-08-18 with pnpm 11.8.0 against the committed
`vendor/codemem/pnpm-lock.yaml`:

```bash
cd vendor/codemem
corepack pnpm install --frozen-lockfile --ignore-scripts
corepack pnpm licenses list --json          # all
corepack pnpm licenses list --json --prod   # what ships
```

| Scope | Packages | Breakdown |
|---|---|---|
| All (incl. dev) | 471 | MIT 378 / Apache-2.0 22 / BSD-3-Clause 20 / ISC 20 / BSD-2-Clause 9 / other 22 |
| Production only | 261 | MIT 208 / Apache-2.0 17 / BSD-3-Clause 14 / ISC 12 / other 10 |

Generating the notices added `rollup-plugin-license` and its dependencies to the dev tree, which is
why the "all" count moved from 455 on 2026-08-16. The production count was re-measured, not carried
over: it is unchanged, because every added package is build-time only.

Findings that need a recorded decision or correction:

| Package | Reported | Actual / choice |
|---|---|---|
| `flatbuffers@1.12.0` | `Unknown` | **Apache-2.0.** `package.json` says `SEE LICENSE IN LICENSE.txt`; the bundled `LICENSE.txt` is Apache-2.0 (upstream `google/flatbuffers`). Reached via `@codemem/core → @xenova/transformers → onnxruntime-web`. |
| `dompurify` | `(MPL-2.0 OR Apache-2.0)` | **Apache-2.0 branch chosen.** |
| `sqlite-vec`, `sqlite-vec-linux-x64` | `MIT OR Apache` (non-SPDX string) | **MIT branch chosen.** |
| `lightningcss`, `lightningcss-linux-x64-gnu` | `MPL-2.0` | Build-time only (viewer CSS). Not present in the production tree, not redistributed. |
| `caniuse-lite` | `CC-BY-4.0` | Dev-only dataset. |
| `mdn-data` | `CC0-1.0` | Dev-only dataset. |

No copyleft-only package is present in the production dependency tree.

## Notices for code bundled into the publishable packages

Four of the five publishable packages are built with Vite, and whatever their
`rollupOptions.external` does not cover is inlined into the shipped output. Every such build now
emits a `THIRD_PARTY_NOTICES.md` next to that output, generated from the **bundler's module graph**
rather than from the build artifact, so it stays correct for outputs that carry no source map.
Generation lives in one place, `vendor/codemem/scripts/license-notice-plugin.mjs`.

Measured on 2026-08-18 against `origin/main`:

| Package | Notice location | Bundled third-party code |
|---|---|---|
| `codemem` (cli) | `dist/THIRD_PARTY_NOTICES.md` | none — the `--ssr` build keeps dependencies external |
| `codemem` (cli) | `dist/THIRD_PARTY_NOTICES.hook-runtime.md` | `commander`, `@codemem/core` |
| `@codemem/core` | `dist/THIRD_PARTY_NOTICES.md` | `hono` |
| `@codemem/mcp` | `dist/THIRD_PARTY_NOTICES.md` | none |
| `@codemem/server` | `dist/THIRD_PARTY_NOTICES.md` | none |
| `@codemem/server` | `static/THIRD_PARTY_NOTICES.md` | 47 packages — 30 × `@radix-ui/*`, 4 × `@floating-ui/*`, and 13 others (`preact`, `@preact/signals`, `@preact/signals-core`, `dompurify`, `marked`, `tslib`, `aria-hidden`, `get-nonce`, `react-remove-scroll`, `react-remove-scroll-bar`, `react-style-singleton`, `use-callback-ref`, `use-sidecar`) |
| `@codemem/opencode-plugin` | `THIRD_PARTY_NOTICES.md` | none |

A package that bundles nothing still ships a notice file saying so. The absence of a file is treated
as a failure, not as "zero dependencies" — otherwise a broken generator would be indistinguishable
from an artifact that genuinely bundles nothing.

`@codemem/opencode-plugin` is the one package the module-graph approach cannot generate a notice
for: the JavaScript it ships is committed source, not a rollup artifact. Measured on 2026-08-18 it
bundles no third-party code — every import is external — so it carries a hand-written
`THIRD_PARTY_NOTICES.md` saying exactly that, and its tarball is inspected like every other
package's. Because its shipped files are committed source rather than build output, a change that
started inlining third-party code would show up in review.

Eleven of the 47 packages in `static/THIRD_PARTY_NOTICES.md` ship no license file of their own
(ten `@radix-ui/*` sub-packages and `react-remove-scroll-bar`). Their entries record the SPDX identifier declared in
`package.json` and state explicitly that upstream ships no license file, rather than omitting the
entry. Whether recording the SPDX identifier is sufficient where upstream ships neither a license file
nor a copyright line — as opposed to supplying the canonical license text under a copyright holder we
would have to infer — is an open question, tracked in
[#81](https://github.com/ojungo69/free-mem/issues/81).

`harness/notice-inclusion-check.mjs` enforces this. It runs the install itself, cleans the build
output directories, rebuilds, packs each publishable package with `pnpm pack`, extracts the tarball,
and checks the notices inside it. Cleaning first matters because several builds use
`emptyOutDir: false`: without it, a stale bundle from an earlier build survives into the tarball
without appearing in the newly generated notice.

The expected `name@version` entries — and a SHA-256 digest of each license body — are pinned as a
**complete set** in `harness/notice-baseline.json`, and the set of notice files is compared too, so a
missing dependency, a missing file, an unexpected addition, and a license text that no longer matches
all fail. Versions are part of the key because the generator runs with `multipleVersions: true`;
keying on the bare name would collapse two versions of one package into a single entry and hide the
omission. A package with no notice file at all is a failure rather than a silent pass. Regenerate
with `--write-baseline` when dependencies legitimately change; the diff is then part of the commit
under review, the same arrangement as `harness/contract-hashes.json`.

Two paths enforce it automatically: the `notices` CI job (on every pull request and push), and each
publishable package's `prepublishOnly` script (on `npm publish` / `pnpm publish`). A third path is
available but optional — `vendor/codemem/scripts/release-tag-preflight.sh` calls the gate, and that
script only runs when someone invokes `pnpm run release:preflight-tag`. The release workflow that
would call it lives at `vendor/codemem/.github/workflows/release.yml`, which is not where GitHub
Actions looks, so it does not run in this repository. `npm publish --ignore-scripts` is not covered.
Restricting publish rights to a protected workflow is tracked in
[#83](https://github.com/ojungo69/free-mem/issues/83).

`harness/license-inclusion-check.mjs` remains separate and still does not look at build
output: it checks package-level `LICENSE` files.

### Bundled code outside the npm packages

`vendor/codemem/plugins/claude/scripts/hook-runtime.mjs` and its `plugins/codex/` counterpart are
committed copies of the `hook-runtime` bundle, produced by `packages/cli/scripts/sync-hook-runtime.mjs`.
They contain `commander` (MIT) and carry no copyright text of their own, so the same script copies the
generated `THIRD_PARTY_NOTICES.hook-runtime.md` next to each of them — MIT requires the copyright line
and the permission notice to accompany copies, and naming the license here is not the same as shipping
its terms alongside the copy. These files are not part of any npm package's `files`, so the tarball
gate does not cover them; they are redistributed through the GitHub source archive, and the copied
notice travels with them. Because the notice is copied from the build rather than written by hand, it
follows whatever the bundle actually contains.

Re-run the scan whenever the lockfile changes; `evidence/adr-004-licensing.md` records how these
findings feed the license decision.
