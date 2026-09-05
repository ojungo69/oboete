# T056 Phase 1 no-Agent-blockage validation

日付: 2026-08-15

対象: `0ad4075` + T056 working tree

## 結論

実配布 hook runtime を別 process で起動する Linux rig を追加し、Claude 2,000ms / Codex 1,500ms の hard cap 内で全 fault を exit 0・exact fail-open 応答に固定した。timeout 直前の `SessionEnd` は 500ms 予約 spool へ保全し、daemon 再起動後も degraded delivery を doctor で可視化する。

## gate coverage

- daemon 不在、socket peer denied、protocol mismatch、disk full、256KiB 超入力、hung daemon、partial close/hold、newline 無し32KiB超応答、large context-pack、100ms lock holder を Claude/Codex の built hook で個別検証。
- worker spawn failure、whole runtime 同期 stall、stdout consumer stall を fault injection。外側 supervisor が Agent 別 hard cap 前に exact fallback を1回だけ返し、exit 0、descendant/process 残留なしを確認。
- safe custom secret/private regex と pinned Gitleaks rule は本文を redaction して `degraded=false`。catastrophic user regex と redaction worker stall は100ms総期限で本文を metadata-only に封じ、予約 spool へ `redaction_degraded=true` を保持。
- timeout edge、slow stdin、timestamp 無し `SessionEnd` 20回 replay を両 Agent で検証。各 hard cap 内、ready spool 1件、安定 event/idempotency key、本文・raw path/ID の非漏えいを確認。
- global scanner の write と代表 maintenance job を catastrophic rule 下で実行。daemon continuity、durable `redaction_degraded` job、再起動後 doctor warning / `degradedDeliveries=1` を確認。
- healthy built hook を反復し p95 を記録。最終 gate は Claude 152.6ms / Codex 160.1ms で150ms目標を外れ、仕様の performance policy に従い非blocking warning とした。直前の同一 gate は144.3ms / 146.2msで、環境変動を隠さず両方記録する。

## gate が検出した regressions

1. hook の RPC timer は preprocessing、config/transcript read、redaction、spool fsync を総期限に含めず、破局的 regex が Agent hard cap を越えた。actual runtime 全体を worker supervisor 内へ置き、入力前からの絶対期限と spool reserve を共有した。
2. user/private regex だけを worker 化すると built-in/global scanner と maintenance が main thread を停止できた。既存 `SecretScanner` を再利用する常駐 worker に全 rule を集約し、100ms event deadline と metadata-only degraded fallback を共有した。
3. bundle 内 `toString()+eval` worker は書換え済み `require` を解決できず、正常 custom rule も常時 degraded になった。実 module URL worker + ready handshake に置換し、built positive control を gate に追加した。
4. hook RPC response は無制限 chunk 蓄積だった。通常 method を32KiB、context pack を256KiBに固定し、newline 前から累積上限を強制した。
5. timestamp 無し `SessionEnd` が retry ごとに別 IDとなり、Codex 設定にも当該 event が無かった。両 Agent の content-anchored identity と5本目の Codex hook を追加し、explicit timestamp の正当な Stop は区別した。
6. malformed global scanner config、無効 capture group/kind/flags、壊れた config file が user rule を黙って無効化して本文を保存できた。missing と invalid を区別し、invalid は store/maintenance とも metadata-only degraded に固定した。
7. spool 再処理で current scan が degraded でも healthy な adapter metadata が上書きし、doctor が false negative になった。各層の ruleset fingerprint を canonical mergeし、degraded/private/local/sensitivity は最強値を保持した。
8. scanner の cycle guard が共有参照も処理済みとして original object を返し、2本目の参照に plaintext を残した。`WeakMap<object, sanitizedClone>` へ変更し、空 clone を再帰前に登録して cycle と alias を同時に安全化した。
9. MCP remember は adapter の degraded metadata を event と違って daemon へ渡さず、project rule failure が doctor に残らなかった。memory RPC/spool の required fields を safe placeholder で維持し、validated metadata を daemon merge・DB・再起動後 doctor まで伝播した。

## runnable checks

```bash
# vendor/codemem
pnpm run phase1:no-agent-blockage
pnpm run tsc
pnpm run lint
pnpm run phase1:static-scan

# repo root
node --check harness/phase1-no-agent-blockage.mjs
git diff --check
```

- self-contained fault gate: 全 fault / replay / doctor / p95 計測 pass、exit 0
- focused regression set: 13 files / 314 tests passed
- packed artifact smoke: pass
- TypeScript / Biome: pass
- built hook artifact: CLI dist / Claude plugin / Codex plugin の bytes と SHA-256 が一致
- spool degraded merge: RED で `redaction_degraded=false` を再現後、関連18 testsを3連続、concurrent writerを5連続 pass

## review coverage

- Ponytail:既存 runtime bundle、worker_threads、SecretScanner、spool、doctor、setup/sync helperを再利用。追加 dependency、第二 scanner、別 journal、wildcard cleanup は追加していない。
- trust boundary: bounded stdin/stdout/RPC response、worker spawn/crash/stall、timeout metadata seal、config syntax/capture/kind validation、owner-only spool、plaintext非永続化を確認した。
- Codex Security 専用 scan は既知の preflight 設定競合で実行不能のため再試行せず、正式 scan 完了は主張しない。Semgrep、runtime fault gate、manual source reviewで補完する。
- GitNexus の現 index は `vendor/codemem` symbols を解決できず影響判定は unknown。caller は `rg` と実 source、compiler、built-process gate を正本として確認した。
- CodeGraph は repo に未導入、Graphify は bounded な runtime/security flow に追加根拠を与えないため、T056 では導入していない。
