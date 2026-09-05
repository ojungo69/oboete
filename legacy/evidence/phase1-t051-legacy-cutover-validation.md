# T051 legacy layout cutover validation

日付: 2026-08-15

対象: product candidate `c4feb833da8c652bdd1d3efa70e8a6e96d33d015`

## 結論

旧配置 DB の owner set を Linux `/proc/*/fd` と trusted absolute `lsof` の和集合で検査し、旧 owner が残る場合は canonical publish を開始しない。唯一の cutover daemon が WAL 上の `BEGIN EXCLUSIVE` を保持したまま read-only handle から online backup を生成・検証し、private recovery hardlink を保持する。

旧 path を private tombstone directory への symlink へ atomic rename + parent fsync した後、recovery hardlink 経由で旧 inode の owner set と setup ownership manifest を再検証し、その後にだけ既存 storage journal で canonical artifact を publishする。transaction/handle/control lock は publish 完了後まで解放しない。

tombstone 設置後・pointer publish 前に停止しても、次回起動は exact tombstone target と同一 dev/inode の recovery hardlink を検証して旧 path を復元し、同じ cutover を最初から再実行する。hardlink の欠損・異なる inode の混在は変更前に fail-closed とする。

## closure

- `packages/core/src/legacy-cutover.ts`: 旧 process の bounded clean-stop request、2点 owner-set 検査、online backup/verify、rollback hardlink、tombstone、journal publish、起動時 interruption recovery、split-brain fence。
- `packages/core/src/daemon-lifecycle.ts`: storage recovery 直後・canonical open 前に one-time cutover。旧 Claude/Codex spool は canonical daemon 起動ごとに共通 redaction spool への durable handoffを再試行し、変換不能・quota失敗のfileを保持したまま既存 dispatcher で成功分だけimport。
- `packages/core/src/install-manifest.ts` / CLI `setup.ts`: managed block に加え、setup が実際に管理した config/runtime file の hash target を owner-only manifestへ記録。部分 setup は別 integration の記録を保持し、選択 integration の消滅 target は除外する。`O_NOFOLLOW` fd と inode 再照合により、欠落・symlink・hash drift は fail-closed。
- 旧 path の WAL/SHM は成功後に除去。旧 binary が再起動しても tombstone directory を SQLite file として開けず、canonical DBにも別DBにも書けない。

## blocking tests

`packages/core/src/legacy-cutover.test.ts`:

- `P1-T051-01-legacy-owner-set`
- `P1-T051-02-cutover-fail-closed`
- `P1-T051-03-tombstone-before-unlock`（tombstone 後・publish 前の起動時復旧と canonical row 保持を含む）
- `P1-T051-04-old-binary-split-brain`

## runnable checks

cwd: `vendor/codemem`

```bash
pnpm exec vitest run packages/core/src/legacy-cutover.test.ts packages/core/src/daemon-lifecycle.test.ts packages/core/src/spool.test.ts packages/core/src/install-manifest.test.ts packages/cli/src/commands/setup-codex.test.ts
pnpm exec tsc --build --force --pretty false
pnpm run lint
pnpm run build
pnpm exec vitest run --no-file-parallelism --maxWorkers=1 --reporter=json --outputFile=/tmp/free-mem-phase1-post-t051-serial.json
sha256sum /tmp/free-mem-phase1-post-t051-serial.json
```

- focused: T051 4 + setup/manifest/lifecycle/spool/sole-writer 回帰が成功
- forced tsc / lint / workspace build: success
- serial full: 395 suites / total 1,833 / passed 1,830 / todo 3 / failed 0
- report SHA-256: `228469c040aee239c4dc68e5f0ab289d49fcd1b90d68d8d623ce226c7825adce`
- test identity: post-T048 `1,828 + 4 registered cutover tests + 1 setup-manifest integration test = 1,833`

## review coverage

- Ponytail: 新 dependency、汎用 migration framework、別 backup 実装を追加せず、T049/T050 storage journal/spool dispatcher を再利用。
- trust boundary: PID reuse 再検査、trusted absolute `lsof`、同一UID `/proc` 不可時のfallback、fd/inode-based symlink検査、private manifest、unlock直前照合、tombstone durable order、早期rollback hardlink、旧spool retain-on-failureを手動差分レビューで確認。
- Codex Security 専用scanは preflight が `agents.max_threads cannot be set when multi_agent_v2 is enabled` で停止したため、正式scan完了は主張しない。ユーザーconfigは変更せず、上記manual review・compiler・runtime testで補完した。
- GitNexus は linked-worktree の新 symbols を解決できず全件 `UNKNOWN`; `rg` caller 全数、実 source、compiler、runtime testsを正本とした。

2026-08-15 closure: T051 4/4、daemon lifecycle 4/4、forced TypeScript build、targeted Biome が成功。独立 correctness/Ponytail review は blocker なし。GitNexus staged diff は linked-worktree index の制限で `No changes detected` のため、起動前 caller と実 test を正本とした。
