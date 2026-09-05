# T044 CLI RPC cutover validation

日付: 2026-08-14

対象: `e191d5a` + T044 working tree

## 結論

T044 対象の production CLI は daemon RPC client に切り替わり、daemon 不在時も SQLite を直接開かない。spoolable mutation は共有 redacted spool、user-authority mutation と read は typed error、後続 Phase 6/7 の機能は typed stub に限定した。旧 prompt-pack ledger command と、stub から到達不能になった local distill/embed/replay/benchmark 実装は削除した。

## command と daemon surface

| CLI | daemon surface / 不在時 |
|---|---|
| `enqueue-raw-event` | `POST /v1/events`。RPC 失敗時は同じ idempotency key と redaction metadata を共有 spool へ保存 |
| `memory remember` | `POST /v1/memories/record`。RPC 失敗時は共有 spool |
| `memory show` / `forget` | `GET` / `DELETE /v1/memories/:id`。typed error、DB fallback なし。`forget` は spool/自動 retry なし |
| `memory inject` / `pack` / `pack trace` | `POST /v1/context/pack`。typed error、DB fallback なし |
| `search` / `recent` | `POST /v1/search`。typed error、DB fallback なし |
| `stats` | `GET /v1/view` の `stats` collection |
| `status` | `GET /v1/health` + `GET /v1/doctor`。daemon 停止は `not_running` / DB `unknown` として exit 0 |
| `serve` | resolved data directory の同一 socket で daemon と viewer を起動・停止 |
| `distill` / extraction replay・benchmark | `{code:"feature_unavailable",phase:6}` |
| `embed` | `{code:"feature_unavailable",phase:7}` |

memory report、DB maintenance、export/import は T045–T047 が所有し、T044 では既存 local path を変更していない。

## privacy と ownership

- CLI と MCP は既存の `createMcpRpcClient` を共有し、`.agent-memory.toml` policy を RPC 前に適用する。
- direct `POST /v1/events` は adapter redaction metadata を envelope に保持する。実 daemon + read-only actor の回帰 test で secret、private、local-only 本文が `raw_events.payload_json` に存在せず、sensitivity/private/local-only metadata が残ることを確認した。
- daemon 不在 matrix は search/recent/stats/pack/show/forget/remember/inject/status を実行し、DB-open trace と SQLite file が生成されないことを確認した。spoolable remember だけが `control/spool/ready` に保全される。
- `--db-path` の legacy override は DB を開くためではなく canonical data directory を導出するためだけに使う。

## runnable checks

cwd: `vendor/codemem`

```bash
pnpm lint
pnpm tsc
pnpm build
pnpm exec vitest run packages/cli/src/commands/distill.test.ts packages/cli/src/commands/embed.test.ts packages/cli/src/commands/memory-inject.test.ts packages/cli/src/commands/memory.test.ts packages/cli/src/commands/pack.test.ts packages/cli/src/commands/status.test.ts packages/cli/src/commands/cli-rpc.test.ts packages/mcp-server/src/rpc-client.test.ts
pnpm exec vitest run --no-file-parallelism --maxWorkers 1 --reporter json --outputFile /tmp/free-mem-phase1-post-t044-serial-final3.json
sha256sum /tmp/free-mem-phase1-post-t044-serial-final3.json
node packages/cli/dist/index.js --help
node packages/cli/dist/index.js memory --help
node packages/cli/dist/index.js pack --help
node packages/cli/dist/index.js status --help
node packages/cli/dist/index.js serve --help
```

- focused: 8 files / 29 tests / failed 0
- serial full: 390 suites / total 1,836 / passed 1,833 / todo 3 / failed 0
- report SHA-256: `f570ca29bb73d8987e3ad8e55af963bd1979bfd5d358f89de85f7ef313c91d17`
- test identity: post-T043 `1,894 - 61 retired + 3 registered = 1,836`。post-only は `P1-T044-01..03` の3件だけ

build 済み CLI を `CODEMEM_DATA_DIR` / `CODEMEM_DB` 未設定、explicit `--db-path`、loopback port 43989 で起動した。viewer health は HTTP 200、`status --json` は daemon running / database ready / viewer running / attention empty、`serve stop` 後は daemon socket 消滅を確認した。help に廃止済み `prompt-pack-ledger` は存在しない。

## Ponytail review

共有 RPC client と既存 core response type を再利用した。typed stub の背後に残っていた local distill/embed/replay/benchmark 本体、独立 prompt-pack ledger transport と重複 test は削除し、T045 以降が必要とする DB maintenance/report/export path だけを残した。
