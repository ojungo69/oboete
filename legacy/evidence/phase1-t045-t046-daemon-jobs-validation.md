# T045/T046 daemon jobs・maintenance mode validation

日付: 2026-08-14

対象: `8663aa1` + T045/T046 working tree

## 結論

maintenance / backfill / report の class C surface を daemon 所有の durable job registry へ移した。CLI は job trigger を一度だけ送信し、受領した job ID の GET 照会だけを行う。別 process maintenance worker、worker command、PID file 管理は削除した。

破壊的 job は daemon maintenance mode で直列化し、通常 write、raw-event sweeper、spool importer を停止する。T050 の online backup を作成・再検証できた場合だけ mutation を開始し、backup path が利用不能な fixture では job が failed となり DB row が未変更であることを確認した。

## durable job contract

- schema v20 の `daemon_jobs` に args、dry-run、state、attempt、result、timestamp を永続化する。`max_attempts` は DB constraint と API projection の双方で 1 固定。
- daemon 再起動時の `queued` / `running` は `daemon_restarted` で failed にする。user-triggered Class-C job は自動再送しない。未完了の内部 backfill だけは、その `daemon_restarted` failure を確認後に fresh job として再投入する（各 job の `max_attempts=1` は維持）。
- `POST /v1/jobs` は `{jobId,state}`、`GET /v1/jobs/:id` は durable result、一覧は kind/state/submittedAfter の allowlist filter のみ。
- CLI は POST を一度だけ実行する。lost response、failed、timeout 後に POST を再送せず、job ID 取得後は GET polling のみ。
- job args は kind ごとの allowlist・型・長さ・件数上限を検証する。dry-run を実装していない mutating kind は `dryRun:true` を投入前に拒否する。
- result は 512 KiB 上限。内部 backfill は daemon の同じ queue で完了まで bounded pass を回す。

## maintenance / backup ordering

```text
queued -> running -> maintenanceMode=true -> sweeper stop
       -> destructive kind: online backup -> hash/SQLite verify
       -> mutation -> result persist -> sweeper resume -> maintenanceMode=false
```

- maintenance 中の通常 DB-writing RPC は `{code:"maintenance_mode",retryable:true}`。spoolable client は redaction 済み shared spool へ保存する。
- spool importer 自身も maintenance 中は走らない。
- backup artifact は `maintenance-<jobId>.sqlite` と `.json`。検証失敗時は mutation を開始しない。
- dry-run job と read-only report は maintenance mode / backup を要求しない。

## test mapping

| token | 証明 |
|---|---|
| `P1-T045-01-job-id-result` | 全登録 kind が job ID を返し、durable result を GET できる |
| `P1-T045-02-job-no-auto-retry` | 再起動時 queued/running が attempt を増やさず failed になる。user job は再送せず、pending な internal backfill だけを fresh job へ再投入する |
| `P1-T045-03-worker-absorbed` | 旧 worker source/command/PID 管理がなく、内部 backfill が daemon job になる |
| `P1-T046-01-maintenance-mode` | mode 中 write 拒否、backup artifact、backup failure 時 DB unchanged、unsupported dry-run 拒否 |
| `P1-T046-02-maintenance-spool` | synthetic retryable maintenance response を redacted spool へ保全 |

T044 の `P1-T044-03-cli-no-db-fallback` も maintenance / memory report command 全体へ拡張し、daemon 不在時に CLI が SQLite を開かないことを固定した。

## runnable checks

cwd: `vendor/codemem`

```bash
pnpm lint
pnpm exec tsc --build --force
pnpm build
pnpm exec vitest run packages/core/src/daemon-jobs.test.ts packages/core/src/daemon-rpc.test.ts packages/mcp-server/src/rpc-client.test.ts packages/cli/src/commands/cli-rpc.test.ts --no-file-parallelism --maxWorkers 1
pnpm exec vitest run --no-file-parallelism --maxWorkers 1 --reporter json --outputFile /tmp/free-mem-phase1-post-t046-serial-final3.json
sha256sum /tmp/free-mem-phase1-post-t046-serial-final3.json
```

- focused safety boundary: 4 files / 20 tests / failed 0
- workspace lint / tsc / build: success
- serial full: 390 suites / total 1,833 / passed 1,830 / todo 3 / failed 0
- report SHA-256: `297ebd3719add24b02bdd6882d172180926960e4f3d7cfcb45d881f3849e2388`
- test identity: post-T044 `1,836 - 8 retired + 5 registered = 1,833`

serial run は、旧 spool-importer test 2 件が起動直後の内部 maintenance queue 完了を待たない race を検出した。health の瞬間値だけでは job 間の隙間を拾うため、両 test を `GET /v1/jobs` の全 job terminal 待機へ合わせた。対象 4 tests を3連続実行し、続く serial full も green を確認した。

## review coverage

- Ponytail review: 旧 worker 357 行と serve PID 管理を削除し、既存 maintenance handler / online-backup / MCP RPC client を再利用した。新 dependency と speculative fallback は追加していない。
- trust-boundary review で unsupported `dryRun:true` が mutation を迂回し得る候補を検出し、kind allowlist による投入前拒否と回帰 assertion を追加した。
- Codex Security diff scan は preflight が `agents.max_threads cannot be set when multi_agent_v2 is enabled` を返したため formal report 未生成。user config は変更していない。
- GitNexus `detect-changes` は linked worktree の index が差分を検出できず `No changes detected`。impact の正本は実 source、compiler、focused/full tests とした。
