# T052 backup / restore baseline validation

日付: 2026-08-14

対象: `4c6b32f` + T052 working tree

## 結論

SQLite online backup の完成 artifact を read-only で再オープンし、schema / SQLite source / FTS / sqlite-vec artifact / active generation / canonical row checksum / watermark を含む canonical manifest と hash を生成する。継続 writer と並行した snapshot の row checksum 一致を blocking test で確認した。

backup は migration、user-authority maintenance/repair、destructive import、daily から共通実装へ到達する。automatic retention は直近 7 日 + それより古い 4 週を保持し、manual backup は自動削除しない。canonical DB と backup directory/artifact/sidecar は owner-only。

restore は verified backup から新 artifact を構築し、FTS を rebuild、vector 不在・artifact 不一致時も FTS-only を維持してから既存 storage journal で pointer を atomic switch する。旧 artifact は削除せず、成功 response 後に daemon を停止する。

## closure

- `vendor/codemem/packages/core/src/online-backup.ts`: manifest v1 / sidecar v2、hash、list/verify、retention、daily、staging restore、compatibility check。
- `vendor/codemem/packages/core/src/daemon-{canonical,jobs,lifecycle,rpc}.ts`: DB 0600、trigger wiring、maintenance/restore exclusion、RPC、restore 後 shutdown。
- `vendor/codemem/packages/cli/src/commands/backup.ts`: `codemem backup create|list|verify|restore` と privacy 表示。
- `vendor/codemem/packages/mcp-server/src/rpc-client.ts`: backup reason に project custom secret rule を適用し、redacted reason から payload hash を再計算。ID は制御値として保持。
- `vendor/codemem/docs/adr/0002-backup-manifest-authenticity.md`: Phase 1 hash-only の脅威境界と Phase 8 signing-key lifecycle / negative fixture gate。

## blocking tests

- `P1-T052-01-backup-manifest-hash`
- `P1-T052-02-backup-retention-permissions`
- `P1-T052-03-restore-journal-order`
- `P1-T052-04-backup-privacy-copy`

## runnable checks

cwd: `vendor/codemem`

```bash
pnpm exec tsc --build --force --pretty false
pnpm exec vitest run packages/core/src/online-backup.test.ts packages/core/src/daemon-lifecycle.test.ts packages/core/src/daemon-rpc.test.ts packages/core/src/daemon-jobs.test.ts packages/core/src/daemon-operations.test.ts packages/cli/src/commands/backup.test.ts packages/mcp-server/src/rpc-client.test.ts --maxWorkers=1 --no-file-parallelism
pnpm run tsc
pnpm run lint
pnpm run build
pnpm exec vitest run --maxWorkers=1 --no-file-parallelism --reporter=json --outputFile=/tmp/free-mem-phase1-post-t052-serial-v2.json
pnpm run codemem backup --help
sha256sum /tmp/free-mem-phase1-post-t052-serial-v2.json
```

- focused: 7 files / 36 tests passed。secret-regression subset は 3 files / 16 tests passed
- tsc / lint（388 files）/ 6 workspace build: success
- serial full: 397 suites / total 1,838 / passed 1,835 / todo 3 / failed 0
- report SHA-256: `0145ab951141e83198f07d239cd1a5d896754368cfe05db741de2f940f57aff8`
- test identity: T051 `1,833 + T052 4 registered tests + MCP custom-secret regression 1 = 1,838`
- 初回 serial run は既存 `P1-T040-03-periodic-sweeper-uses-shared-dispatcher` の 3 秒待機が 1 回だけ失敗。対象 test 単独 pass、同一 full serial 再実行 pass のため負荷時 timing flake と判定し、初回 JSON `/tmp/free-mem-phase1-post-t052-serial.json` も保持

## review coverage

- Ponytail: T050 online backup と判断 #16 storage journal を再利用し、未公開の中間 sidecar compatibility は削除。新 dependency / backup framework / signing-key scaffold は追加していない。
- trust boundary: operation ID allowlist、payload hash、regular-file/symlink/WAL拒否、live inode拒否、owner-only mode、manifest/artifact/canonical row再検証、backup/restore排他、staging fail-closed、旧 artifact保持、secret reason非永続化を手動差分レビューで確認。
- Codex Security 専用scanは preflight が `agents.max_threads cannot be set when multi_agent_v2 is enabled` で停止したため、正式scan完了は主張しない。configは変更せず、manual review・compiler・runtime testsで補完。
- GitNexus は linked-worktree の新 symbols を解決できず impact は `UNKNOWN`、taint/PDG layer も不在。`rg` caller、実 source、compiler、runtime testsを正本とした。
