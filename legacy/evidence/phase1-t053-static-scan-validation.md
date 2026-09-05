# T053 Phase 1 static exit gate validation

日付: 2026-08-14

対象: `355f19f` + T053 working tree

## 結論

production source 276 files を TypeScript AST で走査し、DB opener、DDL、deep/alias import、旧 direct path、未認定 sidecar、public runtime bypass を T029 disposition の exact allowlist と照合する非ゼロ終了 gate を追加した。readonly の例外はない。

Biome の `noRestrictedImports` は `better-sqlite3` の default import、`@codemem/core` の runtime DB capability、`@codemem/core/*` deep import を拒否する。例外は daemon 内の raw SQLite 実装 2 files、`test-utils.ts`、test suffix だけに限定した。

## machine-enforced closure

- `connect` / `connectReadOnly` call: `daemon-canonical.ts`, `daemon-jobs.ts` の 2 files と exact match。
- `new MemoryStore`: 同じ 2 files と exact match。
- `WriterActor.open` / `ReadOnlyActor.open`: `db.ts`, `legacy-cutover.ts`, `online-backup.ts`, `storage.ts` の 4 files と exact match。
- raw `better-sqlite3` import / constructor: `daemon-lifecycle.ts`, `writer-actor.ts` の 2 files と exact match。
- DDL: `daemon-jobs-schema.ts`, `db.ts`, `maintenance-jobs.ts`, `maintenance/init-vacuum.ts`, `mutation-dispatcher.ts`, `schema-bootstrap.ts` の 6 files と exact match。
- `@codemem/core` package export から daemon-owned module への subpath / alias target を拒否。
- T048 disposition の opener 5 rows / exact paths、`RPC_METHODS` 27件と disposition、旧 `/v1/operations/backup/*` 不在を照合。
- test-only exception は `packages/*/src/**/*.test.ts`, `packages/core/src/test-utils.ts`, `packages/core/src/test-schema.generated.ts`, `packages/core/scripts/generate-test-schema.ts` に固定。`scripts/eval/` は disposition で production 外 tooling と明示。

## runnable checks

```bash
# repo root
node --experimental-strip-types harness/phase1-static-scan.ts --self-test

# vendor/codemem
pnpm run phase1:static-scan
pnpm run tsc
pnpm run lint
pnpm exec vitest run packages/core/src/sole-writer-boundary.test.ts --reporter=verbose
```

- static scan: 276 production source files / 0 violations / exit 0
- injected self-test: static/dynamic deep import、sidecar、alias connect、MemoryStore、raw SQLite import/constructor/require、DDL、package export bypass を検出 / exit 0
- Biome negative fixture: daemon 外の `import Database from "better-sqlite3"` を `lint/style/noRestrictedImports` で拒否 / exit 1。fixture は検証後に削除
- tsc: success
- lint: 388 files / 0 errors
- focused boundary test: 1 file / 1 test passed

## review coverage

- Ponytail: 既存 TypeScript compiler API、Biome、T048 boundary test を再利用し、新 dependency と二重の regex scanner を追加していない。
- trust boundary: alias import、dynamic import / require、namespace actor open、runtime/type-only import、public exports、legacy DB path、sidecar identifier/string を確認した。
- Codex Security 専用 scan は既知の preflight 設定競合で実行不能のため再試行せず、正式 scan 完了は主張しない。static negative fixtures、compiler、runtime test、manual source review で補完した。
- GitNexus は linked-worktree index gap のため最終 diff の既知 symbol impact を補助利用し、source / compiler / tests を正本とする。
