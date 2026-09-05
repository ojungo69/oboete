# T057 Phase 1 backup restore smoke validation

日付: 2026-08-15

対象: `de9a872` + T057 working tree

## 結論

実配布 core / CLI / daemon を別 process で起動し、未使用 data dir への restore、FTS 再構築、vector artifact hash 不一致時の FTS-only 成功、破損 journal の fail-closed、legacy writer の排他と tombstone を固定した。test-local fault injection では storage journal、staging、pointer、reopen、integrity、rollback、committed 直後の21境界を個別に通し、各 recovery target を OLD / NEW の一方に固定した。

## gate coverage

- source daemonへ2件を実 RPC 記録し、derived FTSを意図的に空にした状態で built CLI `backup create|verify --json` を実行。backup artifact 内の FTS 0件、manifest/artifact hash、全 canonical table count/checksum を確認した。
- artifact + sidecarを未使用の別 data dirへcopyし、`sqlite_vec.artifact_sha256` を不一致にしてmanifest hashを再計算。built CLI restore後にdaemon自動停止・再起動、直接 FTSと実 RPC searchの双方で2件を取得した。backup後にsourceへ追加した行はtargetへ入らず、source pointer/hash/3件は不変だった。
- restored current artifact と legacy cutover current artifactを別inodeへcopyし、元manifestのcanonical fieldsをproduction `verifyCanonicalBackup`へ再入力。両方で全 canonical table count/checksum、FTS schema、watermark一致を確認した。
- journalの空、部分JSON、不正state、committed artifact hash不一致、currentがold/newのどちらでもない曖昧pointerを実daemonへ投入。全caseで起動非ゼロ、pointer/current artifact hash不変、journal保持を確認した。
- prepared/switched/committed各journalのtmp write・flush・rename・parent fsync、staging artifact/versions fsync、pointer rename/parent fsync、reopen/integrity、rollback pointer rename/parent fsync、committed cleanupをtest-local `node:fs` faultで個別検証。committed rename前はOLD、committed rename可視後のparent fsyncとcommitted cleanupはNEW、その他の未完了境界はOLDへ収束した。
- legacy cutover前のuncooperative idle RW handleは開始前に拒否。EXCLUSIVE取得後のonline backup `.tmp` 中にdaemonを停止して旧writerを起動し、`SQLITE_BUSY`を確認。commit後の旧binary相当再起動も非ゼロ、旧path=tombstone、WAL/SHMなし、current artifact 1本、split-brain tableなしを確認した。

## gate が検出した regression

rollback pointerのrename完了後にparent directory fsyncが失敗すると、次回recoveryは見かけ上old pointerと一致しているためfsyncを再試行せずjournalを削除していた。`restorePointer()` は期待pointerと既に一致する場合（nullを含む）も `db/` をfsyncしてからjournal cleanupへ進むよう修正した。修正前testは `rollback-pointer-parent-fsync: expected 0 to be 1` でRED、修正後21境界がgreenになった。

## runnable checks

```bash
# vendor/codemem
pnpm run phase1:backup-restore-smoke
pnpm exec tsc --build --force --pretty false
pnpm run lint
pnpm run phase1:static-scan

# repo root
node --check harness/phase1-backup-restore-smoke.mjs
git diff --check
```

- workspace build: pass
- real-process harness: fresh-dir / corrupt journal / legacy process の全gate pass
- focused regression set: 4 files / 19 tests passed
- TypeScript: pass
- Biome: 393 files / 0 violations
- static scan: 279 production files / 0 violations

## review coverage

- GitNexus impact: `restorePointer` のdirect callerは `recoverStorageJournal` 1本。daemon startup、restore activation、legacy migration、canonical writer openの4 flowへ到達するためrisk HIGHとして既存3回帰とbuilt process gateを通した。
- correctness/security review: unexpected daemon start時にnegative test自身がexitを無期限待ちする欠陥と、legacy成功系のchecksum不足を検出。stop後即failとproduction verifier再利用で解消した。
- Ponytail: production変更は既存 `fsyncPath` の一致経路再利用だけ。新dependency、production fault hook、第二journal、checksum再実装は追加していない。未参照stdout getterだけを削除した。
- `checksum mismatch` は現行journal schemaの `artifactSha256` 不一致として検証した。committed rename後・parent fsync faultは可視journal stateに従いNEWへ固定し、別のpower-loss modelまでは主張しない。
- CodeGraphはrepoに未導入で、GitNexus + 実sourceが共有flowを解決した。Graphifyはdocs/DB/infra横断の追加根拠を与えないため導入していない。
