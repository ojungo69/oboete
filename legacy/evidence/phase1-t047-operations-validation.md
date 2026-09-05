# T047 export/import daemon operation validation

日付: 2026-08-14

対象: `d813339` + T047 working tree

## 結論

export/import の production CLI は SQLite を直接開かず、daemon の class B operation RPC だけを使用する。各 request は client 生成 UUID と、`operationId` / `payloadHash` を除いた allowlisted body の canonical JSON SHA-256 を持つ。同一 ID・同一 hash は現在または最終 state を返し、異なる hash は filesystem / DB 副作用前に `idempotency_conflict` で拒否する。

operation journal は `control/operations/<operationId>.json` に 0600、各 state 更新を atomic replace + file/parent fsync で永続化する。daemon 再起動時の未完了 operation は自動再実行せず `daemon_restarted` で failed に固定し、`GET /v1/operations/:id` から完了済み result と同様に再取得できる。

## export / import ordering

```text
export: prepared -> writing -> verified -> committed
import(destructive): prepared -> maintenanceMode=true -> sweeper stop
                   -> online backup -> SQLite/hash re-verify -> backup_verified
                   -> applying(transaction) -> committed -> sweeper resume
```

- export は daemon 所有 connection で payload を生成し、同一 directory の 0600 temp file を flush 後に rename、parent fsync、SHA-256 再読出しまで成功してから committed にする。
- output parent の symlink と、intermediate symlink を介して canonical data directory 内へ到達する path を投入前と書込み直前の双方で拒否する。
- destructive import は dry-run validation 後に online backup を作成・再検証し、成功後だけ既存 transaction import を実行する。backup directory を file で塞いだ fixture では failed、memory row 0 のまま。復旧後の別 operation は backup ID 付きで committed、memory row 1 を確認した。
- dry-run import は DB mutation、maintenance mode、backup を行わない。
- CLI の `-` stdin/stdout 互換は 0700 temp directory + 0600 file を介して維持する。terminal result を確認した場合だけ削除し、lost response / timeout では daemon が参照中の可能性があるため artifact を残す。
- operation POST は一度だけ送信し、その後は GET polling のみ。daemon 不在・lost response・failed 後の POST 自動再試行と client DB fallback はない。

## test mapping

| token | 証明 |
|---|---|
| `P1-T047-01-operation-id-conflict` | 同一 ID・異 hash を拒否し、第二 output を生成しない。symlink 経由 data-dir output も拒否 |
| `P1-T047-02-operation-result-retrieval` | 同一 request の再送と daemon 再起動後 GET が同じ committed result / output hash を返す |
| `P1-T047-03-import-backup-precondition` | backup failure は DB unchanged、backup 復旧後だけ import committed |

## runnable checks

cwd: `vendor/codemem`

```bash
pnpm lint
pnpm exec tsc --build --force
pnpm build
pnpm exec vitest run packages/core/src/daemon-operations.test.ts packages/core/src/daemon-jobs.test.ts packages/mcp-server/src/rpc-client.test.ts packages/cli/src/commands/alias-deprecation.test.ts --no-file-parallelism --maxWorkers 1
pnpm exec vitest run --no-file-parallelism --maxWorkers 1 --reporter json --outputFile /tmp/free-mem-phase1-post-t047-serial.json
sha256sum /tmp/free-mem-phase1-post-t047-serial.json
```

- workspace lint / forced tsc / build: success
- serial full: 392 suites / total 1,835 / passed 1,832 / todo 3 / failed 0
- report SHA-256: `dce42b151bd40e1e49ed7fc67948e95f3904a75a5f68fac30f9fe17433158b2d`
- test identity: post-T046 `1,833 - 1 retired stub + 3 registered = 1,835`

## review coverage

- Ponytail review: 既存 canonical hash、online backup、transaction import、daemon job queue、共有 RPC transport を再利用。単一用途 snapshot helper と重複 payload 比較を削除し、新 dependency / retry framework / operation DB table は追加していない。
- trust-boundary review: path type/length/NUL、filter allowlist/type/date、operation ID/hash、data-dir escape、atomic output、backup-before-import、restart/lost-response の非再実行を確認した。
- Codex Security diff scan は T045/T046 と同じ preflight 設定エラー（`agents.max_threads` と `multi_agent_v2` の競合）が未解消のため再実行せず、user config は変更していない。
- GitNexus は linked worktree 差分を検出できない既知 gap のため、全 caller の `rg`、実 source、compiler、focused/full tests を正本とした。
