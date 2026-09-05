# T055 Phase 1 fault-injection exit gate validation

日付: 2026-08-15

対象: `30d79a8` + T055 working tree

## 結論

Linux の隔離 rig で実 build artifact と別 PID daemon を使い、surface fallback、Class A/B replay、spool durability、daemon lifecycle、privacy non-persistence を機械検証する非ゼロ終了 gate を追加した。production fault hook と追加 dependency は使わない。

## gate coverage

- daemon を `SIGKILL` して stale socket を残した状態で、実 hook ingest、hook inject event、MCP remember、CLI raw event を個別実行。ready 4件を exact method/session/event で照合し、再起動後に receipt 4、raw event 3、memory 1、quarantine 0を確認。
- Class A 9 method を同一 key で各10回 replay。実 hook/MCP client の committed response を blackhole し、fallback spool 後に daemon を kill、再取込後の receipt、event、memory、delete revision、retrieval exposure/delivery を各1件で確認。
- backup は実 SQLite copy 中、export は `writing` journal + plaintext temp 存在中に `SIGSTOP`/`SIGKILL`。同一 ID replay、新 ID retry、artifact/manifest hash、orphan temp 0件、missing output parent でも daemon 起動継続を確認。
- spool は tmp write、native flush、既存 tmp file fsync、tmp/ready directory fsync、ready unlink、delete 後 directory fsync を fault injection。startup recovery と実 dispatcher/DB で loss 0、receipt/side effect 各1件を確認。既存 quota/counter/quarantine/concurrent-writer tests も同じ gate で実行。
- 同一 data dir の daemon 同時起動を20回反復し毎回1 owner/1 loserを確認。crash直後再起動、stale socket、child FD非継承、force-kill stale/PID reuse/identity mismatch拒否、lock取得後・identity公開前 kill を実 process で確認。
- 生成した fake GitHub PAT、private marker、`<private>` が spool raw bytes、canonical DB/WAL、fixture tree、surface/daemon stdout・stderr に存在しないことを確認。

## gate が検出した regressions

1. `BEGIN EXCLUSIVE` の同時実行では両 contender が即時 `SQLITE_BUSY` となり owner 0件になる実プロセス競合を再現した。reader を持たない専用 `lock.db` を `BEGIN IMMEDIATE` writer reservation に変更し、busy_timeout 0 / journal DELETE / fail-closed を維持した。
2. flush failure 後の valid tmp は、同一 key 再送と daemon startup recovery の両方で source file fsync 前に rename され得た。共有 recovery を含む両 rename 前に file fsync を置き、fault 中は tmp を保持する。
3. rename 後の directory fsync failure は、同一 key 再送が ready file の存在だけで `duplicate` を返し、未完了の durable boundary を再試行しなかった。tmp/ready directory を再 fsync してから duplicate を返す。
4. backup/export の mid-flight crash は private SQLite/plaintext export temp を retention 外に残した。operation ID から一意に決まる exact temp path を使い、backup replay / export journal recovery で regular-file・no-symlink を確認してその1件だけ削除する。外部 export parent の欠落・権限変更は daemon 起動を妨げず diagnostic に固定する。

## runnable checks

```bash
# vendor/codemem
pnpm run phase1:fault-injection
pnpm run tsc
pnpm run lint
pnpm run phase1:static-scan
semgrep scan --config p/default --error --metrics=off --disable-version-check \
  packages/core/src/daemon-lifecycle.ts packages/core/src/daemon-operations.ts \
  packages/core/src/online-backup.ts packages/core/src/spool.ts

# repo root
node --check harness/phase1-fault-injection.mjs
git diff --check
```

- self-contained fault gate: build success、surface/Class A/lifecycle/Class B 全 pass、exit 0
- focused regression set: 5 files / 32 tests passed
- TypeScript: success
- Biome: 389 files / 0 errors
- static scan: 277 production source files / 0 violations
- Semgrep `p/default`: 378 rules / 4 changed production files / 0 findings
- built hook artifact: CLI dist / Claude plugin / Codex plugin の SHA-256 と bytes が一致

## review coverage

- Ponytail: stdlib process/fs control、既存 daemon/RPC/spool/dispatcher/backup/export、既存 fs mock を再利用。wildcard temp scan、production pause hook、追加 dependency、第二の journal/lock 実装は追加していない。
- trust boundary: credential を継承しない隔離 env、output cap、PID > 1、operation ID/path validation、owner-only temp、regular-file/no-symlink、exact-path cleanup、cleanup failure 時の daemon 継続を確認した。
- Codex Security 専用 scan は既知の preflight 設定競合で実行不能のため再試行せず、正式 scan 完了は主張しない。Semgrep、runtime fault controls、manual source review で補完した。
- GitNexus は変更前の `acquireExclusiveLock` と `recoverTmpEntriesLocked` の caller/affected process を exact に解決した。outer worktree の最終 unstaged mapping は docs のみを解決したため、最終判断は source、compiler、実 process gate、DB assertion を正本とする。
