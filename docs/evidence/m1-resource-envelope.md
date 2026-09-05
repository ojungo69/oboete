# M1 resource envelope

## Measurement setup

- Date: 2026-09-04 JST (cold-start script timestamp: 2026-09-03T21:23:19.083Z UTC).
- Machine: `Linux DESKTOP-PNCJSEO 6.18.33.2-microsoft-standard-WSL2 #1 SMP PREEMPT_DYNAMIC Thu Jun 18 21:54:43 UTC 2026 x86_64 x86_64 x86_64 GNU/Linux`.
- CPU: `AMD Ryzen 9 5950X 16-Core Processor`.
- Node: default `$HOME/.nvm/versions/node/v24.16.0/bin/node` (`v24.16.0`) and `$HOME/.nvm/versions/node/v22.23.1/bin/node` (`v22.23.1`). The latter is the local proxy for the CI matrix's pinned `22.16.0`, not an exact 22.16.0 run.
- Commit: `a9b51fb`.
- Bundle: `dist/oboete.mjs`, 1,237,178 bytes after the build below.

Commands executed for the bundle and cold-start measurement:

```bash
npm run build
node scripts/measure-cold-start.mjs --markdown \
  --node "$(command -v node)" \
  --node "$HOME/.nvm/versions/node/v22.23.1/bin/node" \
  --bundle dist/oboete.mjs
```

Commands executed for the empty-prefix size measurement:

```bash
measure_tmp=$(mktemp -d /tmp/oboete-pack-offline-XXXXXX)
mkdir -p "$measure_tmp/npm-cache/_cacache"
cp -as "$HOME/.npm/_cacache/content-v2" "$measure_tmp/npm-cache/_cacache/content-v2"
cp -a "$HOME/.npm/_cacache/index-v5" "$measure_tmp/npm-cache/_cacache/index-v5"
mkdir -p "$measure_tmp/npm-cache/_cacache/tmp"
export npm_config_cache="$measure_tmp/npm-cache"
npm pack --pack-destination "$measure_tmp"
mkdir "$measure_tmp/prefix-requested"
npm_config_prefer_offline=true npm install --omit=dev \
  --prefix "$measure_tmp/prefix-requested" \
  "$measure_tmp/oboete-0.1.0-alpha.0.tgz"
du -sk "$measure_tmp/prefix-requested/node_modules"
du -sk "$measure_tmp/prefix-requested/node_modules/oboete"
du -sk "$measure_tmp"/prefix-requested/node_modules/* \
  "$measure_tmp"/prefix-requested/node_modules/@*/* | sort -n | tail
```

The temporary cache was needed because this sandbox makes `~/.npm` read-only and blocks registry access. Its content-addressed blobs were exposed read-only by symlink, its 29 MB index was copied, and npm installed 26 production packages into a newly created empty prefix. The successful install command itself has the requested `npm install --omit=dev --prefix <empty prefix> <tarball>` form; `npm_config_prefer_offline` only selects the seeded cache.

## Cold start

The 200 KB cases contain 204,792 bytes of response content; the table reports the complete serialized stdin size. Every secret-dense content line is exactly 160 characters and contains one synthetic `api_key=<48 hex>` value. The `Landed` counts include all 3 warm-ups plus all 30 measured invocations. Status uses the strictest reported statistic (`max`): 100 ms for `--version`, and the capture contract's 300 ms process deadline for hook paths.

<!-- measure:start -->
- Date: 2026-09-03T21:44:35.543Z
- Node versions: `$HOME/.nvm/versions/node/v24.16.0/bin/node` (v24.16.0); `$HOME/.nvm/versions/node/v22.23.1/bin/node` (v22.23.1)
- Commit: `a9b51fb`
- Bundle: `dist/oboete.mjs` (1237178 bytes)
- Samples: 30 measured runs after 3 warm-up runs per scenario
- Measurement attempts: run 1 load `1.40 2.43 2.28 2/4249 14`; run 2 load `1.86 2.42 2.28 1/4191 4282`; kept run 1 (lower 1-minute load average)
- Percentiles: linear interpolation over the 30 measured runs; status is `max <= budget`

Load average next to this table (kept run 1, before the measurement set): `1.40 2.43 2.28 2/4249 14`

| Node | Scenario | stdin bytes | p50 ms | p95 ms | max ms | hook.log wall p50 | Landed | Budget | Status |
|---|---|---:|---:|---:|---:|---|---|---:|---|
| v24.16.0 | `--version` | 0 | 51.2 | 57.4 | 59.7 | n/a | n/a | 100 ms | pass |
| v24.16.0 | hook small, DB present | 730 | 169.2 | 194.1 | 208.6 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook clean 200 KB, DB present | 206768 | 175.0 | 199.3 | 200.0 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook secret-dense 200 KB, DB present | 206772 | 185.6 | 238.1 | 252.1 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v24.16.0 | hook small, DB absent (spool) | 742 | 171.1 | 201.4 | 206.5 | not recorded by hook.log | spool files=33; memory.db absent=yes | 300 ms | pass |
| v22.23.1 | `--version` | 0 | 49.0 | 51.7 | 52.7 | n/a | n/a | 100 ms | pass |
| v22.23.1 | hook small, DB present | 730 | 161.6 | 169.4 | 173.8 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v22.23.1 | hook clean 200 KB, DB present | 206768 | 166.2 | 172.5 | 174.6 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v22.23.1 | hook secret-dense 200 KB, DB present | 206772 | 178.0 | 182.7 | 183.0 | not recorded by hook.log | raw_events=33 | 300 ms | pass |
| v22.23.1 | hook small, DB absent (spool) | 742 | 159.2 | 163.0 | 170.8 | not recorded by hook.log | spool files=33; memory.db absent=yes | 300 ms | pass |
<!-- measure:end -->

Both Node versions pass every cold-start row. The largest `--version` result is 59.7 ms against 100 ms; the largest hook result is 252.1 ms against 300 ms. `hook.log` records capture outcome and row count but no hook-owned wall-time field, so no log-derived p50 exists.

## Installed size

- Entire production `node_modules`: 33,480 `du -k` blocks = 34,283,520 bytes = 32.695 MB.
- Installed `node_modules/oboete`: 1,244 `du -k` blocks = 1,273,856 bytes = 1.215 MB.
- Unit convention: GNU `du -k` reports allocated 1,024-byte blocks; here 1 MB means 1,048,576 bytes, as required by the gate.
- Target: 30 MB = 31,457,280 bytes = 30,720 `du -k` blocks.
- Result: **fail**, over by 2,760 `du -k` blocks = 2,826,240 bytes = 2.695 MB.

Load average next to this table (at the successful requested-form install): `1.24 1.68 1.77 1/3752 16`

| Top-level entry | `du -k` blocks | MB |
|---|---:|---:|
| `ai` | 8,624 | 8.422 |
| `zod` | 7,988 | 7.801 |
| `@ai-sdk` (scope aggregate) | 4,560 | 4.453 |
| `hono` | 3,688 | 3.602 |
| `undici` | 2,180 | 2.129 |
| `preact` | 1,956 | 1.910 |
| `@secretlint` (scope aggregate) | 1,356 | 1.324 |
| `oboete` | 1,244 | 1.215 |
| `workers-ai-provider` | 800 | 0.781 |
| `@hono` (scope aggregate) | 236 | 0.230 |

The scoped rows are directory aggregates produced by the required glob and overlap their child package rows; the table is a ranking, not an additive breakdown.

Written reason to bring to the owner, not an approved exception: the 2.695 MB excess is dominated by `ai`, `zod`, `@ai-sdk/*`, `hono`, `undici`, and `preact`. Plan Complexity Tracking row 1 keeps the heavy observer/viewer packages external and lazily imported because bundling their CommonJS transitives into ESM throws `Dynamic require` and loading them in the engine's hook path would add megabytes to every capture. The owner must either approve that reason as the over-30-MB exception or require a dependency/packaging reduction; this task makes neither decision.

### Re-measurement after reclassifying the bundled packages (commit after d2b275c)

`preact`, `@secretlint/core`, `@secretlint/secretlint-rule-preset-recommend` and `smol-toml` ride
inside `dist/oboete.mjs` (scripts/build.mjs bundles the hook-path packages) or only exist at
viewer build time (`preact` is compiled into the viewer assets by vite, T079), so they are
devDependencies, not runtime dependencies. `zod` stays: it is a peer dependency of `ai` and the
provider path needs it at runtime. Same commands as above, run online against the registry
(no cache seeding), 16 production packages installed:

- Entire production `node_modules`: 29,852 `du -k` blocks = 30,568,448 bytes = 29.152 MB.
- Installed `node_modules/oboete`: 1,244 `du -k` blocks (unchanged).
- Result: **pass**, 868 `du -k` blocks = 0.848 MB under the 30 MB target (30,720 blocks).

| Top-level entry | `du -k` blocks | MB |
|---|---:|---:|
| `ai` | 8,624 | 8.422 |
| `zod` | 7,988 | 7.801 |
| `@ai-sdk` (scope aggregate) | 4,560 | 4.453 |
| `hono` | 3,688 | 3.602 |
| `undici` | 2,180 | 2.129 |
| `oboete` | 1,244 | 1.215 |
| `workers-ai-provider` | 800 | 0.781 |
| `@hono` (scope aggregate) | 236 | 0.230 |

The margin is small (0.85 MB); a dependency bump of `ai`, `zod` or `hono` can cross the line,
which is what the pack-check step of T087 is for.

## R13 rows

Load averages for the source tables: cold start `1.40 2.43 2.28 2/4249 14`; installed size `1.24 1.68 1.77 1/3752 16`.

| R13 row | Status | Evidence | Consequence from research.md |
|---|---|---|---|
| Real bundle cold start on 22.16 and 24.x | pass | v22.23.1 local proxy: 52.7 ms max for `--version`, 183.0 ms max hook; v24.16.0: 59.7 ms max for `--version`, 252.1 ms max hook | Not triggered: “blocked; a split entry point needs a constitution amendment first” |
| Installed size with dependencies | pass (after reclassification; the first measurement at a9b51fb failed at 32.695 MB) | 29.152 MB / 29,852 `du -k` blocks, 0.848 MB under target once the bundled packages became devDependencies | Not triggered; the first measurement's written reason is kept above for the record |

## Fixture replay (T068)
