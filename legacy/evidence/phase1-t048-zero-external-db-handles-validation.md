# T048 daemon-only DB handle validation

日付: 2026-08-14

対象: `2ebf9df` + T048 working tree

## 結論

test-only exact allowlist を除き、DB connection を開く production code は daemon 所有モジュールと audited SQLite 実装内部だけになった。CLI、MCP stdio、viewer、hook、export/import client、maintenance wrapper には DB fallback がなく、`@codemem/core` から runtime DB opener を取得することもできない。

## closure

- `MemoryStore` は既 open `WriterActor` を必須引数とし、default path、migration、connection 自己生成を廃止した。
- `connect` / `connectReadOnly`、`MemoryStore`、`WriterActor` / `ReadOnlyActor` は package runtime export から除外した。actor/store の型だけは type-only export とした。
- path を受け取って自己 open していた export/import、maintenance init/report/relink/reliability/status、migration wrapper を削除した。daemon handler は `*WithDb`、既 open store、既存 pass 関数を使う。
- 独立 connection を所有していた dedup/ref/scope/session-context/summary/vector runner class を削除し、daemon job registry が pass 関数を直接呼ぶ。
- test は package public surface を使わず、exact allowlist `packages/core/src/test-utils.ts` の `openTestMemoryStore` または test file の direct actor import を使う。

## production opener allowlist

| opener | allowed source |
|---|---|
| `connect` / `connectReadOnly` | `core/db.ts`、`core/daemon-canonical.ts`、`core/daemon-jobs.ts` |
| `new MemoryStore` | `core/daemon-canonical.ts`、`core/daemon-jobs.ts` |
| `WriterActor.open` / `ReadOnlyActor.open` | `core/db.ts`、`core/legacy-cutover.ts`、`core/online-backup.ts`、`core/storage.ts` |
| raw `BetterSqlite3` | `core/daemon-lifecycle.ts`、`core/writer-actor.ts` |

`connectReadOnly` は audited internal definition のみ残り、production caller は 0。MCP SDK の `server.connect(transport)` は SQLite opener ではないため scan の negative-lookbehind で除外する。

## blocking test

`packages/core/src/sole-writer-boundary.test.ts` の `P1-T048-01-zero-external-db-handles` は次を一つの判定にする。

1. comments/test/generated path を除いた全 production TypeScript を走査し、opener hit が上記 exact source allowlist だけであること。
2. public runtime surface に `connect`、`connectReadOnly`、store/actor、direct export/import/maintenance wrapper、旧 runner class が存在しないこと。

## runnable checks

cwd: `vendor/codemem`

```bash
pnpm exec vitest run packages/core/src/sole-writer-boundary.test.ts packages/core/src/daemon-foundation.test.ts packages/core/src/daemon-rpc.test.ts packages/core/src/online-backup.test.ts packages/core/src/mutation-dispatcher.test.ts packages/cli/src/commands/db.test.ts packages/mcp-server/src/rpc-client.test.ts --reporter=dot
pnpm exec vitest run packages/core/src/store.test.ts packages/core/src/sole-writer-boundary.test.ts packages/core/src/daemon-jobs.test.ts --reporter=dot
pnpm exec tsc --build --force --pretty false
pnpm run lint
pnpm run build
pnpm exec vitest run --no-file-parallelism --maxWorkers=1 --reporter=json --outputFile=/tmp/free-mem-phase1-post-t048-serial.json
sha256sum /tmp/free-mem-phase1-post-t048-serial.json
```

- focused: 46 + 110 tests passed
- forced tsc / lint / workspace build: success
- serial full: 393 suites / total 1,828 / passed 1,825 / todo 3 / failed 0
- report SHA-256: `e7367709111ae546c770b5dac1c4654c9a878341d4a699a7ae13450c3c089f4c`
- test identity: post-T047 `1,835 - 10 retired direct-opener tests + 2 handle-injection replacements + 1 registered = 1,828`

## review coverage

- Ponytail review: self-opening wrapper と単独 runner class を残さず削除し、daemon の既存 handle、`*WithDb`、pass 関数を再利用した。新 dependency / factory / compatibility shim は追加していない。
- trust-boundary review: package exports、deep relative production import、全 opener caller、connection close ownership、daemon report comparison、test-only exception を確認した。
- Codex Security diff scan は既知の preflight 設定競合（`agents.max_threads` と `multi_agent_v2`）のため実行せず、user config は変更していない。手動で runtime export と opener callsite を全数確認した。
- GitNexus の linked-worktree diff gap のため、`rg` 全 caller、実 source、compiler、focused/full tests を正本とした。
