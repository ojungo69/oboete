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

### Setup

- Date: 2026-09-06T03:10:26.194Z
- Machine: `Linux DESKTOP-PNCJSEO 6.18.33.2-microsoft-standard-WSL2 x64`.
- CPU: `AMD Ryzen 9 5950X 16-Core Processor`.
- Node: `/home/jura/.nvm/versions/node/v24.16.0/bin/node` (v24.16.0).
- Commit: `16303cac`.
- Bundle: `/home/jura/projects/free-mem-p5-t068/dist/oboete.mjs`, 1460612 bytes.
- Fixture: `/home/jura/projects/free-mem-p5-t068/test/fixtures/events-1000.jsonl` (1051 lines).
- `OBOETE_HOME`: `/tmp/oboete-t068-home-q9XUB5`. Config file absent (schema default preset `workers-ai`); child environment has no provider credentials, so summaries are rule-based (`no_provider`).
- Temporary git repository with one empty commit so `HEAD` exists. `NODE_ENV=test`.
- Worker RSS: Linux `/proc/<pid>/status` `VmHWM`, polled every 50 ms on replay's `observe` children and, from before the first hook through the final flush, hook-spawned workers found via `worker_lease.pid` using one read connection. The lease poll excludes replay's own pid and observe children; a busy read is skipped. A replay-owned `worker_lease` token is held across every `SessionEnd`/`session_shutdown` and the pending windows; hooks can still spawn workers while the lease is free.

Commands executed:

```bash
npm run build
node dist/oboete.mjs fixture replay /home/jura/projects/free-mem-p5-t068/test/fixtures/events-1000.jsonl
```

Load average at the start of the run: `0.38 0.42 1.40 1/2683 3446234`

### SC-002 capture time

Capture-only hooks (`hookDeadlineMs` ≠ `INJECTION_DEADLINE_MS`). Bound 300 ms. Row status is informational: p99 ≤ bound and ≥99% of samples ≤ bound. SC-002 is judged on the pooled capture sample.

| Agent | Event | n | p50 ms | p95 ms | p99 ms | max ms | Bound | Status |
|---|---|---:|---:|---:|---:|---:|---|---|
| claude | PostCompact | 1 | 157.2 | 157.2 | 157.2 | 157.2 | 300 ms | pass |
| claude | PostToolUse | 56 | 159.6 | 168.3 | 174.7 | 181.3 | 300 ms | pass |
| claude | PostToolUseFailure | 1 | 157.4 | 157.4 | 157.4 | 157.4 | 300 ms | pass |
| claude | PreToolUse | 57 | 157.7 | 169.2 | 173.2 | 174.9 | 300 ms | pass |
| claude | SessionEnd | 11 | 159.7 | 162.2 | 162.6 | 162.8 | 300 ms | pass |
| claude | Stop | 58 | 165.2 | 176.2 | 178.4 | 179.3 | 300 ms | pass |
| codex | PostCompact | 1 | 150.4 | 150.4 | 150.4 | 150.4 | 300 ms | pass |
| codex | PostToolUse | 60 | 159.3 | 165.3 | 180.8 | 189.7 | 300 ms | pass |
| codex | PostToolUseFailure | 1 | 160.9 | 160.9 | 160.9 | 160.9 | 300 ms | pass |
| codex | PreToolUse | 61 | 159.3 | 164.4 | 170.6 | 173.0 | 300 ms | pass |
| codex | SessionEnd | 11 | 158.4 | 164.7 | 165.1 | 165.2 | 300 ms | pass |
| codex | Stop | 62 | 158.9 | 165.5 | 174.5 | 180.5 | 300 ms | pass |
| grok | PermissionDenied | 1 | 169.3 | 169.3 | 169.3 | 169.3 | 300 ms | pass |
| grok | PostCompact | 2 | 151.1 | 151.2 | 151.2 | 151.2 | 300 ms | pass |
| grok | PostToolUseFailure | 1 | 166.9 | 166.9 | 166.9 | 166.9 | 300 ms | pass |
| grok | SessionEnd | 11 | 160.8 | 170.7 | 171.3 | 171.5 | 300 ms | pass |
| grok | Stop | 60 | 176.8 | 183.0 | 190.0 | 194.6 | 300 ms | pass |
| pi | agent_settled | 77 | 166.6 | 171.2 | 174.2 | 179.5 | 300 ms | pass |
| pi | input | 77 | 163.2 | 168.2 | 169.8 | 170.6 | 300 ms | pass |
| pi | session_compact | 1 | 168.4 | 168.4 | 168.4 | 168.4 | 300 ms | pass |
| pi | session_shutdown | 15 | 160.4 | 165.8 | 168.8 | 169.6 | 300 ms | pass |
| pi | session_start | 16 | 155.5 | 167.0 | 168.8 | 169.3 | 300 ms | pass |
| pi | tool_result | 76 | 163.2 | 170.3 | 173.6 | 177.9 | 300 ms | pass |
| all | * | 717 | 162.0 | 177.0 | 181.2 | 194.6 | 300 ms | pass |

### Injection hooks

Classified by `hookDeadlineMs(agent, event) === INJECTION_DEADLINE_MS` (Claude/Codex `SessionStart`/`UserPromptSubmit`, Grok `SessionStart`/`UserPromptSubmit`/`PreToolUse`/`PostToolUse`, Pi `inject` for `session_start`/`input`). Every (agent, event) group must pass: every sample ≤ 300 ms, with no 99% allowance. The per-agent pending session-start sample is excluded here and reported only in the session-start table at 1300 ms. Session-start events that ran while the lease was held for another agent's pending window are also omitted (they are not the ready path). Pi capture of those events stays in the capture table; the inject child is measured here.

| Agent | Event | n | p50 ms | p95 ms | p99 ms | max ms | Bound | Status |
|---|---|---:|---:|---:|---:|---:|---|---|
| claude | SessionStart | 12 | 178.4 | 190.0 | 196.7 | 198.3 | 300 ms | pass |
| claude | UserPromptSubmit | 58 | 190.2 | 201.0 | 203.9 | 204.6 | 300 ms | pass |
| codex | SessionStart | 10 | 177.2 | 186.9 | 189.6 | 190.3 | 300 ms | pass |
| codex | UserPromptSubmit | 62 | 190.0 | 201.0 | 207.1 | 208.8 | 300 ms | pass |
| grok | PostToolUse | 57 | 173.1 | 183.1 | 188.1 | 191.1 | 300 ms | pass |
| grok | PreToolUse | 59 | 170.7 | 177.6 | 181.6 | 183.2 | 300 ms | pass |
| grok | SessionStart | 10 | 177.3 | 182.1 | 182.9 | 183.0 | 300 ms | pass |
| grok | UserPromptSubmit | 60 | 189.3 | 198.9 | 203.5 | 208.2 | 300 ms | pass |
| pi | input | 76 | 122.1 | 128.0 | 130.2 | 132.1 | 300 ms | pass |
| pi | session_start | 14 | 116.2 | 124.0 | 125.7 | 126.2 | 300 ms | pass |
| all | * | 418 | 176.1 | 197.1 | 203.1 | 208.8 | 300 ms | pass |

Size-tagged events (FILL-only JSON byte length, then ROOT substituted). Stdin above the 256 KiB read bound is stored as `partial` / `truncated = 1`.

| seq | Agent | Event | tag | FILL JSON bytes | wall ms | classification_state | truncated |
|---:|---|---|---|---:|---:|---|---:|
| 761 | claude | UserPromptSubmit | at_bound | 1048576 | 163.3 | partial | 1 |
| 777 | grok | UserPromptSubmit | at_bound | 1048576 | 163.9 | partial | 1 |
| 793 | codex | UserPromptSubmit | above_bound | 1048577 | 160.3 | partial | 1 |
| 809 | pi | input | above_bound | 2097152 | 164.3 | partial | 1 |

### Session-start wait

Ready path: previous session summarized (bound 300 ms). Pending path: one sample per agent whose hold window opens, with the lease kept held from that agent's last session end preceding its own last session start through that start (and Pi `inject --kind start`) so the hook cannot spawn a worker and the pack must take the pending path (bound 1300 ms = the engine's INJECTION_DEADLINE_MS: the 300 ms ready budget plus the 1 s summary wait of FR-024; inject.ts caps the wait at the remaining budget). Passing requires at least one sample and the summary-pending sentence in every sample's pack. The lease hold is what makes the pending path deterministic.

| Agent | Path | n | p50 ms | p95 ms | max ms | Bound | summary_pending | Status |
|---|---|---:|---:|---:|---:|---|---|---|
| all | ready | 45 | 175.2 | 183.2 | 198.3 | 300 ms | 0/45 packs carry summary_pending | pass |
| all | pending | 4 | 1180.6 | 1185.1 | 1185.7 | 1300 ms | 4/4 packs carry summary_pending | pass |

Ready max 198.3 ms (n=45, pass). Pending max 1185.7 ms (n=4, pass).

### SC-003 worker memory and database growth

- observe runs: 43 spawned by replay, 42 hook-spawned (polled via worker_lease.pid).
- Max VmHWM: 113864 kB = 111.195 MB (bound 150 MB, pass).
- `memory.db` + `-wal` before: 221184 bytes; after: 3985488 bytes; delta 3764304 bytes; 3581640 bytes per 1,000 events.
- Rows: raw_events=1322, memories=87, injections=292, injection_items=5464.

### SC-005 secret scan

All 32 non-null corpus secrets are absent from memory.db, memory.db-wal, spool/, logs/, and packs.

Detector precision on the 5 `secret = null` negatives: 0 of their `text` values survived into `memories` unredacted (a redacted negative is a false positive, not a failure of this bound).

### Directive scan

All 32 directive phrases are absent from memories.title, memories.body, and packs.

64 raw_events.content rows still carry a directive phrase (allowed; they may remain in raw events and the spool).

### SC-010 duplicate injections

Zero `injection_items` rows with `decision = included` share the same `(conversation_id, context_epoch, memory_id)`.

raw_events.id count 1322 vs lines piped 1051. Pi `tool_result` stores two kinds per line, so the id count can exceed the line count; a re-delivery would collapse onto an existing id.

### SC-009 fact recall

Japanese 20.0% (4/20); English 15.0% (3/20); overall 17.5% (7/40). Bound ≥ 90%. Summaries are rule-based (`preset` default with no credentials).

Misses:

| fact id | lang | query |
|---|---|---|
| f-ja-08 | ja | ビューア本文の色トークン名は？ |
| f-ja-12 | ja | 日本語パックのバケット名は？ |
| f-ja-16 | ja | 形態素フォールバックのライブラリと版は？ |
| f-ja-20 | ja | 日本語ターン予算のキー名は？ |
| f-en-01 | en | Which port does the sidecar listen on? |
| f-en-05 | en | Where is the compaction epoch key computed? |
| f-en-09 | en | What is the pack cache hostname? |
| f-en-13 | en | Which bucket holds English packs? |
| f-en-17 | en | Which library version does fallback packing use? |
| f-ja-05 | ja | 検出器タイムアウトのログコードは？ |
| f-ja-09 | ja | 日本語 dogfood のログイン名は？ |
| f-ja-13 | ja | 正規化修正のコミットはどれ？ |
| f-ja-17 | ja | セッション要約の最短間隔は何秒？ |
| f-en-06 | en | What error code is a stalled Pi child? |
| f-en-10 | en | What is the viewer accent token name? |
| f-en-14 | en | Which commit is the injection ledger fix? |
| f-en-18 | en | What is the next English-side migration file? |
| f-ja-06 | ja | 次に入れるマイグレーションのファイル名は？ |
| f-ja-10 | ja | 全角英数の正規化はどのファイル？ |
| f-ja-14 | ja | 日本語ヒントの環境変数名は？ |
| f-ja-18 | ja | replay 作業ブランチ名は？ |
| f-en-07 | en | Which branch should the replay harness land on? |
| f-en-11 | en | What is the English dogfood login name? |
| f-en-15 | en | Which env var enables English retrieval traces? |
| f-en-19 | en | What is the English turn budget key? |
| f-ja-07 | ja | 社内 memory ホストの名前は？ |
| f-ja-11 | ja | 無料枠のリセット日はいつ？ |
| f-ja-15 | ja | 日本語 compact のキュー名は？ |
| f-ja-19 | ja | 鍵ローテは月のいつ？ |
| f-en-08 | en | How many seconds is the spool reclaim interval? |
| f-en-12 | en | When is the next catalog freeze? |
| f-en-16 | en | What is the English observer queue name? |
| f-en-20 | en | Which model alias do Grok fixture sessions pin? |

### Lifecycle

Each `tags.lifecycle` sequence checked against contracts/agents.md. `fork`: the forked session's `conversation_id` differs from the preceding session of that agent. `resume`: that SessionStart created no `injections` row and printed no pack. `compact`: at least one detector-clean (`classification_state = done`) `compaction_summary` row exists, and the conversation's `context_epoch` equals that row count (Claude's PostCompact + SessionStart(compact) pair counts once, A16). `clear`: a new session id and a new conversation. A compaction hook that misses the detector deadline stores a `failed` row and by A16 opens no epoch; 0/0 fails this check.

| check | n | pass/fail | offending sessions |
|---|---:|---|---|
| fork | 3 | pass | — |
| resume | 4 | pass | — |
| compact | 4 | pass | — |
| clear | 4 | pass | — |

`compaction_summary` rows:

| agent | native_session_id | classification_state |
|---|---|---|
| claude | 217d4204-49e7-4e5a-8911-79e20872f3dc | done |
| codex | 8699654e-8ae4-4da7-90e6-3620fecce2a7 | done |
| grok | 12612e89-11b9-4a27-9067-4fb7f58668a7 | done |
| grok | 12612e89-11b9-4a27-9067-4fb7f58668a7 | done |
| pi | 443b6e30-2d75-4a6b-9a0c-0745524c61b6 | done |

### Hook exits

Capture and injection hooks (including Pi `inject`). Bound: every process exits 0; a non-zero status, a kill signal, or a spawn timeout is a contract violation (contracts/cli.md, FR-002). Wall time of these hooks stays in the timing tables above.

All 1143 capture and injection hooks exited 0 (none killed, none timed out).

### Bounds

| SC | Measured | Bound | Status |
|---|---|---|---|
| SC-002 | p99 181.2 ms; 100.0% ≤ 300 ms (n=717) | p99 ≤ 300 ms and ≥99% of capture events ≤ 300 ms | pass |
| injection | p99 203.1 ms; 100.0% ≤ 300 ms (n=418); worst codex/UserPromptSubmit p99 207.1 ms | every (agent, event) group passes: every injection hook ≤ 300 ms (previous summary ready) | pass |
| session start | ready max 198.3 ms (n=45); pending max 1185.7 ms (n=4), 4/4 packs carry summary_pending | ready ≤ 300 ms; pending n > 0, every pack carries summary_pending and every sample ≤ 1300 ms (INJECTION_DEADLINE_MS: 300 ms budget + 1 s summary wait) | pass |
| SC-003 | max VmHWM 113864 kB (111.2 MB); observe runs: 43 spawned by replay, 42 hook-spawned (polled via worker_lease.pid); growth 3581640 bytes / 1,000 events | < 150 MB worker peak RSS; growth recorded | pass |
| SC-005 | 0 secret ids in db/wal/spool/logs/packs | zero secret corpus values in db, wal, spool, logs, packs | pass |
| SC-009 | ja 20.0% (4/20); en 15.0% (3/20); overall 17.5% (7/40) | ≥ 90% ja, en, and overall | fail |
| SC-010 | 0 duplicate included (conversation_id, context_epoch, memory_id) groups; raw_events.id=1322 vs lines piped=1051 | zero duplicate included memories per (conversation, epoch) | pass |
| lifecycle | fork/resume/compact/clear all pass | every tagged sequence matches contracts/agents.md | pass |
| directives | 0 directive phrases in memories/packs | zero corpus directive phrases in memories and packs (FR-021) | pass |
| hooks | all 1143 capture/injection hooks exited 0 | all hooks exit 0 | pass |

One or more measured bounds failed. The numbers above are the run, not a softened reading.
