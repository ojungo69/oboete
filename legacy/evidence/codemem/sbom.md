# Phase 0A license and binary-asset inventory

Evidence snapshot: commit `26438e75ce1d0fec6be34981f15045a15c89658b`.

## Scope and method

- Direct dependencies are the union of `dependencies` and `devDependencies` in the root and every tracked package manifest, including the two nested `.opencode/package.json` manifests. Workspace links are listed separately.
- Exact versions come from `pnpm-lock.yaml:7-343`. The nested OpenCode runtime manifests pin `@opencode-ai/plugin` `1.2.27` directly but are not pnpm-workspace importers, so that version is identified as manifest-pinned rather than lockfile-resolved.
- License identifiers were read from locally available package registry manifests where present and checked against the corresponding upstream package/repository license where the registry manifest omitted the field. `Declared source` records that distinction; no dependency install succeeded during this freeze.
- Native/binary assets were found by inspecting tracked file types, `pnpm-lock.yaml`, `pnpm-workspace.yaml`, and runtime download/load code.

## Repository license

The repository license is MIT (`LICENSE:1-20`). Published workspace packages declare MIT in `packages/cli/package.json:5`, `packages/core/package.json:5`, `packages/mcp-server/package.json:5`, `packages/viewer-server/package.json:5`, `packages/opencode-plugin/package.json:5`, and `packages/cloudflare-coordinator-worker/package.json:5`. The private UI manifest has no `license` field (`packages/ui/package.json:1-4`).

## Direct workspace dependencies

| Name | Version | License | Evidence |
|---|---:|---|---|
| `@codemem/core` | `0.40.2` | MIT | `packages/core/package.json:2-5`; workspace links at `pnpm-lock.yaml:59-61`, `92-94`, `135-137`, `203-205`, `307-309` |
| `@codemem/mcp` | `0.40.2` | MIT | `packages/mcp-server/package.json:2-5`; `pnpm-lock.yaml:95-97` |
| `@codemem/server` | `0.40.2` | MIT | `packages/viewer-server/package.json:2-5`; `pnpm-lock.yaml:98-100` |

## Direct external dependencies

`Registry manifest` means the exact-version npm metadata available locally declared the license. `Upstream license` means the exact manifest omitted a license field or its cached metadata was unavailable, and the package's upstream license was used. These are direct dependencies only, not a transitive-license inventory.

| Name | Exact version(s) | License | Declared source |
|---|---:|---|---|
| `@biomejs/biome` | `2.4.16` | MIT OR Apache-2.0 | Upstream license; exact registry manifest did not declare it |
| `@clack/prompts` | `1.5.0` | MIT | Registry manifest |
| `@cloudflare/vitest-pool-workers` | `0.13.5` | MIT | Registry manifest |
| `@cloudflare/workers-types` | `4.20260529.1` | MIT OR Apache-2.0 | Registry manifest |
| `@hono/node-server` | `2.0.4` | MIT | Registry manifest |
| `@modelcontextprotocol/sdk` | `1.29.0` | MIT | Registry manifest |
| `@opencode-ai/plugin` | `1.18.15`; nested manifests `1.2.27` | MIT | Upstream license; `1.18.15` is lockfile-resolved and `1.2.27` is manifest-pinned |
| `@preact/preset-vite` | `2.10.5` | MIT | Registry manifest |
| `@preact/signals` | `2.9.1` | MIT | Upstream license |
| `@radix-ui/react-collapsible` | `1.1.12` | MIT | Upstream package license |
| `@radix-ui/react-dialog` | `1.1.15` | MIT | Upstream package license |
| `@radix-ui/react-dropdown-menu` | `2.1.16` | MIT | Upstream package license |
| `@radix-ui/react-popover` | `1.1.15` | MIT | Upstream package license |
| `@radix-ui/react-radio-group` | `1.3.8` | MIT | Upstream package license |
| `@radix-ui/react-select` | `2.2.6` | MIT | Upstream package license |
| `@radix-ui/react-switch` | `1.2.6` | MIT | Upstream package license |
| `@radix-ui/react-tabs` | `1.1.13` | MIT | Upstream package license |
| `@radix-ui/react-toast` | `1.2.15` | MIT | Upstream package license |
| `@radix-ui/react-toggle-group` | `1.1.11` | MIT | Upstream package license |
| `@radix-ui/react-tooltip` | `1.2.8` | MIT | Upstream package license |
| `@types/better-sqlite3` | `7.6.13` | MIT | Registry manifest |
| `@types/express` | `5.0.6` | MIT | Registry manifest |
| `@types/node` | `24.12.4`; root `24.13.2` | MIT | Registry manifest |
| `@types/omelette` | `0.4.5` | MIT | Upstream package license |
| `@xenova/transformers` | `2.17.2` | Apache-2.0 | Upstream package license |
| `better-sqlite3` | `12.8.0` | MIT | Registry manifest |
| `bonjour-service` | `1.4.0` | MIT | Upstream package license |
| `commander` | `14.0.3` | MIT | Registry manifest |
| `dompurify` | `3.4.11` | MPL-2.0 OR Apache-2.0 | Registry manifest |
| `drizzle-kit` | `0.31.10` | Apache-2.0 | Upstream license; exact registry manifest did not declare it |
| `drizzle-orm` | `0.45.2` | Apache-2.0 | Registry manifest |
| `express` | `5.2.1` | MIT | Registry manifest |
| `express-rate-limit` | `8.5.2` | MIT | Registry manifest |
| `hono` | `4.12.26` | MIT | Registry manifest |
| `husky` | `9.1.7` | MIT | Upstream license; exact registry manifest did not declare it |
| `jsdom` | `27.4.0` | MIT | Registry manifest |
| `limiter` | `3.0.0` | MIT | Upstream package license |
| `lint-staged` | `16.4.0` | MIT | Registry manifest |
| `marked` | `18.0.5` | MIT | Registry manifest |
| `omelette` | `0.4.17` | MIT | Upstream package license |
| `preact` | `10.29.2` | MIT | Registry manifest |
| `sqlite-vec` | `0.1.9` | MIT OR Apache-2.0 | Upstream package license |
| `tslib` | `2.8.1` | 0BSD | Registry manifest |
| `tsx` | `4.22.3` | MIT | Registry manifest |
| `typescript` | `6.0.3` | Apache-2.0 | Registry manifest |
| `vite` | `8.0.16` | MIT | Registry manifest |
| `vitest` | `4.1.7` | MIT | Upstream license; exact registry manifest did not declare it |
| `wrangler` | `4.95.0` | MIT OR Apache-2.0 | Registry manifest |
| `zod` | `4.4.3` | MIT | Registry manifest |

Exact resolution evidence is `pnpm-lock.yaml:7-38`, `pnpm-lock.yaml:52-343`; the nested manifest pins are `packages/cli/.opencode/package.json:2-4` and `packages/opencode-plugin/.opencode/package.json:2-4`.

## Native, binary, WASM, and downloaded assets

### Tracked repository assets

The pinned tree contains no tracked `.node`, `.so`, `.dylib`, `.dll`, `.exe`, `.wasm`, archive, or vendored prebuilt-binary file. The tracked binary media are the five documentation PNG screenshots under `docs/images/`; they are not executable assets. `packages/viewer-server/static/` is generated and absent from the pinned tree.

### Dependency-provided native and binary families

| Family | Locked version(s) | Asset/load behavior | Evidence |
|---|---:|---|---|
| `better-sqlite3` | `12.8.0` | Native Node SQLite addon; install/build is explicitly allowed | `pnpm-lock.yaml:2380`, `pnpm-workspace.yaml:4-6` |
| `sqlite-vec` | `0.1.9` | Prebuilt SQLite extensions for Darwin arm64/x64, Linux arm64/x64, Windows x64 | `pnpm-lock.yaml:3549-3574` |
| `@xenova/transformers` / ONNX Runtime | `2.17.2`; `onnxruntime-node` and `onnxruntime-web` `1.14.0` | Native Node runtime and browser WASM/runtime assets used by the embedding backend | `pnpm-lock.yaml:2276`, `pnpm-lock.yaml:3253-3260`, `packages/core/src/embeddings.ts:166-174` |
| Biome CLI | `2.4.16` | Platform CLI packages for Darwin, Linux glibc/musl, and Windows | `pnpm-lock.yaml:460-511` |
| esbuild | `0.18.20`, `0.25.12`, `0.27.3`, `0.28.1` | Platform executable packages; install/build is explicitly allowed | `pnpm-lock.yaml:681-1275`, `pnpm-lock.yaml:2725-2740`, `pnpm-workspace.yaml:6` |
| workerd | `1.20260317.1`, `1.20260526.1` | Cloudflare runtime binaries for Darwin, Linux, and Windows; install/build is explicitly allowed | `pnpm-lock.yaml:558-612`, `pnpm-lock.yaml:3884-3889`, `pnpm-workspace.yaml:10` |
| `msgpackr-extract` | `3.0.4` | Optional native extractor packages for Darwin, Linux, and Windows; install/build is explicitly allowed | `pnpm-lock.yaml:1506-1531`, `pnpm-lock.yaml:3177`, `pnpm-workspace.yaml:7` |
| sharp / libvips | `0.32.6`, `0.34.5`; libvips `1.2.4` | Native image addon plus platform libvips payloads; install/build is explicitly allowed | `pnpm-lock.yaml:1325-1468`, `pnpm-lock.yaml:3472-3476`, `pnpm-workspace.yaml:9` |
| lightningcss | `1.32.0` | Platform native bindings for Android, Darwin, FreeBSD, Linux, and Windows | `pnpm-lock.yaml:3016-3086` |
| rolldown | `1.0.3` | Platform native bindings plus `wasm32-wasi`; uses `@napi-rs/wasm-runtime` `1.1.4` | `pnpm-lock.yaml:1536`, `pnpm-lock.yaml:2066-2155`, `pnpm-lock.yaml:3405` |
| `fsevents` | `2.3.3` | Optional macOS native filesystem-events addon | `pnpm-lock.yaml:2849` |
| `rosie-skills` | `0.6.4` | Package ships/selects platform command binaries | `pnpm-lock.yaml:3425` |
| `blake3-wasm` | `2.1.5` | WASM hashing payload | `pnpm-lock.yaml:2393` |

`protobufjs` is explicitly build-disabled (`pnpm-workspace.yaml:8`); that setting is recorded because it is adjacent to the native build allowlist, not because the repository contains a protobuf binary.

### Runtime-downloaded artifacts

- The embedding backend defaults to model id `Xenova/bge-small-en-v1.5` (`packages/core/src/embeddings.ts:132-134`). Its lazy `@xenova/transformers` pipeline creation (`packages/core/src/embeddings.ts:140-152`, `packages/core/src/embeddings.ts:169-174`) can fetch/cache model/config/tokenizer/ONNX artifacts on first use when they are not already cached. No model artifact is committed in this repository.
- Hook, MCP, setup, and OpenCode runner fallbacks invoke `npx`, which can download/cache the `codemem` npm package when no installed CLI is selected. Version-pinned examples are `plugins/codex/scripts/ingest-hook.mjs:99`, `plugins/codex/scripts/user-prompt-hook.mjs:123`, `plugins/claude/scripts/ingest-hook.sh:44-47`, `plugins/claude/scripts/ingest-hook.sh:131-134`, and `packages/opencode-plugin/.opencode/plugins/codemem.js:1518-1572`; unpinned `npx codemem` entries are `plugins/codex/.mcp.json:1-8`, `plugins/claude/.claude-plugin/plugin.json:8-15`, `plugins/claude/scripts/inject-context-hook.sh:28-32`, `plugins/claude/scripts/pre-read-hook.sh:28-32`, and the setup-generated MCP/hook configurations at `packages/cli/src/commands/setup.ts:70-119`, `packages/cli/src/commands/setup.ts:201-273`, `packages/cli/src/commands/setup.ts:335-351`.
- The shipped viewer HTML requests a Google Fonts stylesheet/font payload and an unversioned `lucide@latest` browser script from external CDNs at runtime (`packages/ui/static/index.html:8-11`). Neither downloaded web asset is committed in the repository.
