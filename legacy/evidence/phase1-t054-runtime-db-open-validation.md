# T054 Phase 1 runtime DB-open exit gate validation

日付: 2026-08-14

対象: `e206280` + T054 working tree

## 結論

Linux の隔離 0B rig 上で、7 runtime states × 6 independent surfaces を実 process として起動し、canonical / legacy DB の open owner が daemon PID だけであることを機械検証する非ゼロ終了 gate を追加した。daemon down では owner set が空で、DB main / WAL / SHM / pointer / legacy path の存在、mtime、size、SHA-256 が全 surface 実行の前後で不変であることを確認する。

## process matrix

states:

- daemon down
- daemon up
- maintenance
- online backup
- schema migration
- legacy cutover
- restore

各 state で hook ingest、hook inject、MCP stdio、CLI read、viewer RPC、jobs CLI を別 PID で実行する。daemon は harness とは別 PID で起動し、active operation は実 trace / job / journal evidence を検出した時点で `SIGSTOP` して観測後に `SIGCONT` する。production fault hook は追加していない。

## owner evidence

- `CODEMEM_DB_OPEN_TRACE`: daemon lifecycle / writer actor / readonly actor の PID、owner、DB path を照合。
- `/proc/<pid>/fd`: `(dev, ino)` を全同一 UID process から走査。
- trusted `/usr/bin/lsof`: path evidence を `/proc` と独立に走査。
- atomic switch / rename 後も旧 inode を追うため、canonical / legacy / staging / version DB は hardlink evidence path と `(dev, ino)` の双方を保持。
- 全7 states で synthetic two-PID owner set を拒否し、canonical と legacy の実 DB を別 rogue PID で readonly open して `/proc` と `lsof` の双方が検出し、state assertion が失敗することを校正。

## gate が検出した regressions

1. standalone hook bundle が DB actor/native SQLite まで取り込み、ESM bundle 内の CommonJS builtin `require` で起動不能だった。storage layout を DB actor から分離し、hook bundle は pure layout を import、Vite banner で Node `createRequire` を提供した。生成済み Claude / Codex hook runtime も同じ build artifact に同期した。
2. active operation 中に client timeout で socket が閉じた後、daemon が response を返すと未処理 `EPIPE` で終了した。RPC connection error を fail-safe に破棄し、restore の restart request は peer disconnect 後も一度だけ実行するよう shared `attachDaemonRpc` で修正した。

## runnable checks

```bash
# vendor/codemem
pnpm run phase1:runtime-db-open-trace
pnpm exec vitest run packages/core/src/daemon-rpc.test.ts packages/core/src/daemon-lifecycle.test.ts packages/core/src/spool.test.ts packages/cli/src/hook-runtime.test.ts packages/cli/src/commands/hook-thin-client.test.ts --reporter=verbose
pnpm run tsc
pnpm run lint
pnpm run phase1:static-scan

# repo root
node --experimental-strip-types harness/phase1-static-scan.ts --self-test
node --check harness/phase1-runtime-db-open-trace.mjs
```

- self-contained runtime gate: build success、7 states × 6 independent surfaces、全 state pass、exit 0
- focused regression set: 5 files / 34 tests passed
- TypeScript: success
- Biome: 389 files / 0 errors
- static scan: 277 production source files / 0 violations
- static scan injected self-test: pass
- standalone built hook: invalid JSON に `{"continue":true}`、DB/native actor 不在、残る dynamic require は Node builtins のみ
- Semgrep `p/default`: changed source / harness は 0 findings。生成 hook 2 files の 12 findings は、再生成前にも同一だった dynamic regex の重複検出で、固定 internal tag は false positive、user regex の killable deadline 境界は予定どおり T056 で blocking 検証する

## review coverage

- Ponytail: production pause hook、追加 dependency、第二の DB owner implementation は追加せず、既存 trace、0B rig、`/proc`、`lsof`、stdlib process control を再利用した。完了済み child timer は明示的に解除する。
- trust boundary: inherited credentials を渡さない隔離 env、trusted absolute `lsof`、output cap、temporary-root prefix guard、owner-only DB artifacts、PID > 1 / independent PID negative controls を確認した。
- Codex Security 専用 scan は既知の preflight 設定競合で実行不能のため再試行せず、正式 scan 完了は主張しない。Semgrep、focused tests、runtime negative controls、manual source review で補完した。
- GitNexus は新規 harness と linked-worktree symbols を解決できず impact は `UNKNOWN`。全 caller の source review と compiler / tests / runtime gate を正本とする。

## 2026-08-16 closure refresh

product-code candidate `01440d5` で self-contained gate を再実行し、7 runtime states × 6 independent surfaces がすべて pass した。stopped daemon の CLI probe は production の `status --json` を外側 timeout 付きで使用し、harness 自身の無期限待機を防ぐ。T037 の socket DAC error は shared `callDaemonRpc` で `EACCES -> peer_denied` へ到達し、unknown/transient socket error は従来どおり reject して hook/MCP の spool fail-over を維持する。

同候補の集約結果は `evidence/phase1-t058-final-validation.md` に記録する。
