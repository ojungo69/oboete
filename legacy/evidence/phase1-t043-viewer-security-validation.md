# T043 viewer security validation

日付: 2026-08-14

対象: `52f2f03` + T043 working tree

## 結論

viewer は loopback 上の静的 UI、秘密を返さない public health、認証済み read-only RPC relay に限定された。browser process と viewer process は DB handle を持たず、全 data read と authoritative session registry は daemon が所有する。browser は origin-scoped `sessionStorage` に opaque credential だけを保持する。未認証、別 Origin、nonce 再利用、期限切れ session、daemon 再起動、旧 mutation/ingest route、daemon 不在をそれぞれ fail-closed または typed failure で確認した。

## 攻撃者と到達性

| 攻撃者 | 到達可能面 | 防御・結果 |
|---|---|---|
| 未認証の local process | loopback HTTP data API | Bearer/session なしは 401。誤 credential も 401。public health は liveness metadata のみ |
| 悪意ある web origin | loopback HTTP、browser session | exact viewer Origin 以外は 403。session は origin-scoped `sessionStorage` に保持し、相対 `/api/` request の `Authorization: Session` だけに付与。CSP は `script-src 'self'` |
| URL/history/referrer の観測者 | CLI が開く viewer URL | 60秒 one-use nonce だけを fragment に置き、network 前に `history.replaceState` で除去。`Referrer-Policy: no-referrer` |
| viewer process の侵害/誤実装 | daemon RPC | viewer は DB を開かず、allowlisted read/auth/context RPC だけを使用。旧 mutation/ingest/transport route は未登録で 404 |
| 別 UID の local process | `control/` と daemon socket | control dir 0700、socket/token/identity/lock 0600。RPC peer credential gate は T037 の same-user 検査を継続使用 |

## 検証結果

| 境界 | 実装確認 | runnable check | 判定 |
|---|---|---|---|
| 256-bit bearer | daemon が `control/token` に 32 byte base64url を生成。regular file、owner、0600 を再読込時も検査。比較は constant-time | `P1-T043-09-bearer-file` | pass |
| nonce | daemon memory 内、60秒、交換時に先に削除 | `P1-T043-03-nonce-single-use-race` | pass |
| browser session | HMAC署名、12時間、daemon instance binding、上限8、logout、再起動失効 | `P1-T043-04-session-expiry-restart`, `05-session-eviction`, `13-daemon-restart-session` | pass |
| browser request | exact loopback Origin、bounded auth JSON、`sessionStorage` + `Authorization: Session`、`credentials: "omit"`、no-store | `P1-T043-01-browser-auth-401`, `02-origin-403`, `11-loopback-cookie-csp` | pass |
| URL/CSP | nonce を fetch 前に history から除去。remote script/font を削除し `script-src 'self'` | `P1-T043-06-browser-url-privacy`, `11-loopback-cookie-csp` | pass |
| credential non-disclosure | health は bearer 不要の非秘密 liveness response。CLI/core/plugin probe は未検証 listener に Authorization を送らない | existing core/plugin probe tests + `P1-T043-01-browser-auth-401` | pass |
| daemon ownership | `GET /v1/view` が daemon-owned store/observer/sweeper を使用。config secret header は daemon 側で `[redacted]` | `P1-T043-10-daemon-auth-rpc`, `12-daemon-view-collections` | pass |
| read-only HTTP | legacy viewer mutation、hook ingest、pack transport、config write は route 未登録 | `P1-T043-07-viewer-read-only` | pass |
| daemon failure | DB fallback せず安定した `{error:{code,message}}` 503 | `P1-T043-08-viewer-daemon-unavailable` | pass |
| loopback variants | `127.0.0.1/8` と `[::1]` を shared helper で統一 | actual HTTP exchange tests | pass |

## 実行証跡

cwd: `vendor/codemem`

```bash
corepack pnpm exec vitest run packages/cli/src/commands/serve.test.ts packages/cli/src/viewer-runtime.test.ts packages/core/src/claude-hooks.test.ts packages/core/src/daemon-rpc.test.ts packages/core/src/mutation-dispatcher.test.ts packages/core/src/spool-importer.test.ts packages/core/src/viewer-auth.test.ts packages/core/src/viewer-probe.test.ts packages/ui/src/lib/api/internal.test.ts packages/ui/src/lib/api/runtime.test.ts packages/ui/src/tabs/feed/data/sanitize.test.ts packages/ui/src/components/primitives/primitives.test.tsx packages/viewer-server/src/auth.test.ts packages/viewer-server/src/index.test.ts
corepack pnpm run tsc
corepack pnpm run lint
corepack pnpm run build
corepack pnpm exec vitest run --no-file-parallelism --maxWorkers=1 --reporter=json --outputFile=/tmp/free-mem-phase1-post-t043-serial-final.json
sha256sum /tmp/free-mem-phase1-post-t043-serial-final.json
```

full suite は 396 suites / total 1,894 / passed 1,891 / todo 3 / failed 0。report SHA-256 は `d2e58af19a2b84bf7fc005ec931fcb9dcacb27445b05a7e293d29d405512bdfa`。

focused suite は 14 files / 185 tests、OpenCode probe parity は 2 files / 25 tests で成功した。

## 実ブラウザ確認

初回 T043 候補は headed Chromium で一時 daemon + viewer を起動し、`#auth=<nonce>` の history 除去、data API、read-only Settings、Health/Feed、自前glyphを確認した。その後、旧 host-scoped cookie が別 loopback port に送信されることを実 Chromium で再現した。現行の cookie 非発行、cookie-only 401、origin-scoped `Session` header、相対 `/api/` 限定は regression tests と built viewer smoke で確認し、現行実装を headed Chromium で再確認したとは主張しない。確認 artifact は checkout から削除した。

通常の並列 full run では、互いに異なる既存 DB/spool concurrency test が各試行で1件ずつ失敗した。両 test は単独再実行で成功し、serial full suite も成功したため、T043 の機能 failure ではなく resource/concurrency flake と分類した。

## 制約・残リスク

- 実行 UID は 1000 で root 権限がないため、別 UID からの live connection は作れなかった。設計で許容された代替として、T037 の peer credential test と control/socket/token の owner-only permission assertion を使用した。
- browser credential は cookie に保存しない。別 loopback port へ cookie が送られる旧挙動を実 Chromium で再現し、現行の cookie 非発行・cookie-only 401・origin-scoped Session header で回帰を固定した。
- 既存 SPA の inline CSS を保持するため CSP の `style-src` は `'unsafe-inline'`。script は self-only で、第三者 runtime code は読み込まない。
