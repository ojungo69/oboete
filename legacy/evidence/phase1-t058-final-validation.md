# T058 Phase 1 final candidate validation

日付: 2026-08-16

初回対象: product candidate `546edd346a4b1c35b72850377512108f742f55b8`（`fix: restore MCP compatibility contracts`）

## 結論

SC-1 の初回候補検証は完了した。後続の closure refresh は下記に追記し、`main` へのマージと外部設定は引き続き最終外部ステップとして分離する。

## Final gates

- toolchain: Node `v24.16.0` / Corepack pnpm `11.8.0`。
- clean checkout `check`: 114 files、`1,854 = 1,851 passed + 3 todo`、failure 0。
- test-set comparator: `4,037`（事前）`- 2,376`（retire）`+ 193`（登録済み追加）`= 1,854`（最終）。
- T055 fault injection: built surface / Class A / lifecycle / Class B 全 pass、focused 5 files / 34 tests。T056 no-Agent-blockage: pass、最終 p95 は Claude `148.3ms` / Codex `139.3ms` で 150ms 目標内、13 files / 314 tests と packed artifact も pass。T057 backup/restore smoke: real-process gate と focused 4 files / 19 tests が pass。
- T029 の disposition は T053 static scan で再照合済み（279 production files / 0 violations）。
- clean checkout は frozen lockfile install → build → check → CLI help → 実 viewer 起動 / health / 正常停止の順で pass。
- full coverage は statements `76.86%`、branches `69.10%`、functions `81.48%`、lines `79.18%`。LCOV と `origin/main...a8ff359` の追加 executable line / branch outcome を照合した Sonar new-code 近似は `5,595 / 6,929 = 80.75%`。Sonar の SCM 判定と Quality Gate は push 後の解析を正本とする。
- PR `#5` の `61acb3d` 解析では Sonar new-code coverage `81.551%`、security / maintainability `A`、duplication `0.884%`、hotspots reviewed `100%`。ASCII operation ID の locale 非依存 sort を誤って BUG とした1件は根拠付き false positive とし、Quality Gate は pass に再評価された。
- Vitest JSON SHA-256: `5b7d261b88a42da32d1c094205170b4ece4a5dd35fc5f090cf0289f77d1911ba`。LCOV SHA-256: `149e5691ae96496053d09aef38c1e160f75857528e50d0f03e793ced999c1746`。
- tsc、Biome 393 files、workspace build、3 standalone hook bundle の byte 一致（SHA-256 `9cd97306668ba5a159a2bed1591b11e2ec78bf725b6ac7c2d137f0d220a82ac8`）も pass。
- final analysis closure は focused 6 files / 176 tests、viewer API 2 files / 2 tests、UI production build、tsc、targeted Biome、GitNexus caller impact、independent correctness/security review、Ponytail review が pass。
- PR closure は deadline-sensitive test の production同等 prewarm（`51ee76e`）と、MCP request scope / public search shape / `--db-path` 互換復元（`546edd3`）を追加した。初回候補で workspace build、tsc、targeted Biome、focused 6 files / 40 tests、全114 files / `1,854` tests、test-set comparator、CLI help、GitNexus caller impact、independent correctness/security review、Ponytail review が pass。

## 2026-08-16 closure refresh

closure refresh は `01440d5` のfull gate bundleと、その後の product-code candidate `cfbcd6f`、historical validated head `87195f5`、test-only coverage closure `55323dd` を分けて記録する。

- toolchain は Node `v24.16.0` / Corepack pnpm `11.8.0`。

`01440d5` gate bundle:

- `01440d5` の serial full suite は 401 suites / `1,854 = 1,851 passed + 3 todo` / failed 0。Vitest JSON SHA-256 は `32f3ceea08f11a9939300dd60ded15dcd26f0d45952caa7ab1f3ba2e6a821b25`。
- test-set comparator は `4,037 - 2,376 + 193 = 1,854`、registered tokens 83、unexpected 0。
- T053 static scan は 279 production files / 0 violations。
- T054 は 7 runtime states × 6 independent process surfaces が pass。T055 は built surface / Class A / lifecycle / Class B と focused 5 files / 34 tests が pass。
- T056 は fail-open gate、focused 13 files / 314 tests、packed artifact が pass。healthy p95 は Claude `132.5ms` / Codex `131.9ms` で 150ms 目標内。
- T057 は real-process fresh restore / journal fail-closed / legacy fencing と focused 4 files / 19 tests が pass。
- CLI dist / Claude / Codex の standalone hook bundle は byte 一致、SHA-256 `c9ee97876394fba32eb8e7e8adc4327db7f4bc7e0aa315eec222e35440a6cc47`。
- setup は source checkout の共通 built CLI/hook を事前検査し、built CLI と選択 lane の config/installed runtimeをmanifestへ記録する。OpenCode は絶対Node/checkout pathを持つwrapperとplugin sourceも記録し、plugin不在をmutation前に拒否する。packed `--codex-only` は OpenCode package 非同梱でも2回成功した。lane config と install manifest は失敗時 rollback する。
- T037 は shared socket transport の実 `EACCES` を `peer_denied` / non-retryable にし、`ECONNREFUSED|ENOENT` だけを retryable unavailable にする。その他の socket error は reject のままなので spool/fail-open 契約を弱めない。

`1a42c65` delta は setup snapshotを既存coreと同じ `O_NOFOLLOW -> fstat/read(fd) -> post-read lstat` へ変更した。inode swapを注入した既存title内の回帰testは旧実装で意図したassertion failure、修正後 setup/config 2 files / 28 tests、workspace build、`tsc --build`、targeted Biome、packed artifact が pass。

その後の product-code delta は `0ca5e4e`（redaction worker readiness）、`216866a`（MCP retrieval の SDK handoff finalization）、`a478665`（Claude user-scope MCP と setup transaction）、`89384c3`（custom MCP package spec の no-force ownership境界）。`89384c3` では focused 4 files / 30 tests、CLI build、`tsc --build`、targeted Biome、independent correctness/security review、Ponytail review が pass。custom scoped package、npm alias、file/git/URL spec、Codex TOML comment injection は unmanaged として拒否し、既知 launcher/argv だけを明示 allowlist する。

test-only `55323dd` は既存title内で direct / transient npx / Node checkout / uvx / uv run / uv tool run / npx `-p` / npx `--package=` の8形を追加した。focused setup test 1 file / 23 tests、Biome、`tsc --build`、CLI build が pass。PR run `31910079592` と push run `31910076790` は full coverage を含む `check` が passし、PR #7 の Sonar new-code coverage は `81.2327506899724%`（lines `472 / 567`、conditions `411 / 520`）へ上昇した。Reliability / Security / Maintainability は A、duplication `0.0%`、hotspots reviewed `100%`。CodeQL、Codacy、Semgrep、GitGuardian、Socket、secrets も passした。

`87195f5` は single-quoted Codex MCP table と、commented managed assignment が active custom commandを隠す2件を修正した。table quoteは対称一致し、current/legacy 判定はactive `command` / `args` parserを共有する。既存title内の2 regressionは旧実装で意図したassertion failure、修正後 focused setup 1 file / 23 tests、Biome、`tsc --build`、CLI build、Semgrep 22 rules / 0 findings、independent correctness/security review、Ponytail review が pass。

`87195f5` headの push run `31910863263` と PR run `31910865507` は full coverage を含む `check` が passした。PR #7 の Sonar new-code coverage は `81.3528336380256%`（lines `477 / 572`、conditions `413 / 522`）。Reliability / Security / Maintainability は A、duplication `0.0%`、hotspots reviewed `100%`。CodeQL、Codacy、Semgrep、GitGuardian、Socket、secretsもpassし、review threadは12件中unresolved 0。

`9177d61` は setup config transaction の最終 hardening である。snapshot は `O_NOFOLLOW` FD と再照合を使い、既存 target の publish直前変更を拒否する。新規 OpenCode wrapper は hard-link no-clobber で公開し、install manifest の capture/read/merge/write は既存 spool lock で直列化する。Codex TOML の dotted / inline-parent / descendant の `mcp_servers.codemem` 表現は `--force` を含め fail-closed とし、既知の table 形式だけを更新する。

`a263513` は JSON/JSONC root を共通 object parser で検証し、Codex hooks の preflight/install と OpenCode/Claude config の全 caller で `null`・array・primitive を mutation 前に拒否する。旧実装は `null` root で `TypeError`、OpenCode loader は非objectを受理するREDを確認し、既存title内の回帰は拒否後の元bytes保持を固定した。最終deltaは setup/config 2 files / 28 tests、関連 6 files / 38 tests、変更4 filesのBiome、`tsc --build`、workspace build、packed artifact、T053 static scan 279 production files / 0 violations、Semgrep `p/security-audit` 22 rules / 0 findings、3 hook bundle SHA-256 `c9ee97876394fba32eb8e7e8adc4327db7f4bc7e0aa315eec222e35440a6cc47`、independent correctness/security/Ponytail review が passした。

同headの push run `31915346516` と PR run `31915349208` は full coverage を含む `check` が passした。PR #7 の Sonar new-code coverage は `85.83270535041447%`（lines `617 / 700`、conditions `522 / 627`）。Reliability / Security / Maintainability は A、duplication `0.0%`、hotspots reviewed `100%`。CodeQL、Codacy、Semgrep、GitGuardian、Socket、secretsもpassし、review threadはunresolved 0。

`c1b25e3` は Codex MCP の現行判定を canonical table 全体のexact matchへ変更し、同じ command/argsでも `enabled = false` 等の追加・変更された assignmentを残さずcanonical形へ正常化する。既存title内の回帰testは旧実装で disabled assignment が残るREDを確認し、修正後 setup/config 2 files / 28 tests、関連 6 files / 38 tests、変更2 filesのBiome、`tsc --build`、workspace build、packed artifact、T053 static scan 279 production files / 0 violations、Semgrep `p/security-audit` 22 rules / 0 findings、3 hook bundle SHA-256 `c9ee97876394fba32eb8e7e8adc4327db7f4bc7e0aa315eec222e35440a6cc47`、independent correctness/security/Ponytail review が passした。

同headの push run `31916103054` と PR run `31916105227` は full coverage を含む `check` が passした。PR #7 の Sonar new-code coverage は `85.77912254160363%`（lines `614 / 697`、conditions `520 / 625`）。Reliability / Security / Maintainability は A、duplication `0.0%`、hotspots reviewed `100%`。CodeQL、Codacy、Semgrep、GitGuardian、Socket、secretsもpassし、review threadはunresolved 0。

`721e3e0` は Codex TOML の offset-preserving structure scanner を共有し、comment、single/multiline string、nested array/inline table、CRLFをtable境界から除外した。ambiguous/unsupported layout、Unicode escape key、root/descendant assignment、array table、duplicate table、header/RHS trailing tokenは `--force` を含めmutation前に拒否する。既存title内の回帰testを含む setup 23 / 23、関連 6 files / 38 tests、変更2 filesのBiome、`tsc --build`、workspace build、packed artifact、T053 static scan 279 / 0、Semgrep 22 rules / 0 findings、independent correctness/security/Ponytail review が passした。

`22dfd34` は OpenCode wrapper の import先を re-export `index.js` から実装 `.opencode/plugins/codemem.js` へ直結し、実装本体と唯一のlocal transitive import `.opencode/lib/compat.js` を install manifestへ記録する。旧実装でactual source path期待が失敗するREDを確認し、修正後 setup 23 / 23、manifest/cutoverを含む focused 5 files / 34 tests、変更2 filesのBiome、`tsc --build`、workspace build、packed artifact、T053 static scan 279 / 0、Semgrep 22 rules / 0 findings、3 hook bundle SHA-256 `c9ee97876394fba32eb8e7e8adc4327db7f4bc7e0aa315eec222e35440a6cc47`、independent correctness/security/Ponytail review が passした。Claude `.claude.json` は higher-precedence local MCP entryも同居するため、one-time cutover前のwhole-file fingerprintを意図的なfail-closed境界として維持した。

`22dfd34` headの push run `31918904433` と PR run `31918905724` は full coverage を含む `check` が passした。PR #7 の Sonar new-code coverage は `86.63594470046083%`（lines `778 / 877`、conditions `726 / 859`）。Reliability / Security / Maintainability は A、duplication `0.0%`、hotspots reviewed `100%`。CodeQL、Codacy、Semgrep、GitGuardian、Socket、secretsもpassし、review threadはunresolved 0。

`cfbcd6f` は Codex MCP table の置換範囲を最後の非comment・非blank行までに限定し、次tableの説明commentと空行を保持する。既存title内の回帰testは旧実装でcomment消失のREDを確認し、修正後 setup 23 / 23、変更2 filesのBiome、`tsc --build`、workspace build、Semgrep `p/security-audit` 22 rules / 0 findings、independent correctness/security/Ponytail review が passした。

code/test headの push run `31920200434` と PR run `31920202238` は full coverage を含む `check` が passした。PR #7 の Sonar new-code coverage は `86.70487106017193%`（lines `783 / 882`、conditions `730 / 863`）。Reliability / Security / Maintainability は A、duplication `0.0%`、hotspots reviewed `100%`。CodeQL、Codacy、Semgrep、GitGuardian、Socket、secretsもpassし、review threadはunresolved 0。

- closure production deltaへの Semgrep `p/security-audit` は初回 changed 4 filesと最終 setup targetの各runで22 rules / 0 findings。independent correctness、manual security、T037、selected-lane setup、CodeQL #57 review は blocker-free。Ponytail reviewでは既存 typed-error helperと既存 safe file-descriptor patternを再利用し、追加 abstraction/dependency は作らなかった。
- immutable machine verifier は evidence-aware matcher の **25 / 31** を維持する。現候補は source/caller/test/evidence/exit-gate の手動 five-layer auditで T027–T057 **31 / 31**。T037 は production socket callerまで再確認済み。

PR #7 の code/test head `cfbcd6f` は上記local delta gateとlive checksで再検証済み。`87195f5`、`a263513`、`c1b25e3`、`22dfd34` のlive coverage/checkはhistorical headの記録として保持する。`main` merge と Sonar long-lived branch移行は、この repository evidence commit 後に実施する external stepとして分離する。

## Machine verifier と手動照合

`verify-tasks-report.md` の machine verdict は意図的に **25 / 31** のまま保持する。これは evidence-aware verifier の branch/diff/presence matcher の到達範囲であり、未完了数ではない。

手動照合は T027–T057 の **31 / 31** を candidate、実装、test/evidence、exit gate で突合して完了した。machine score と一致しない6件の理由は次のとおり。

Fresh-session verifier の immutable report は `61acb3d` で生成した。その後の `3da6826` は Codacy / Sonar の最終解析対応、`51ee76e` は test-only timing安定化、`546edd3` は既存MCP契約の復元に限定され、上記 gate と全 caller review で別途検証した。T027–T057 の手動照合結果は変わらない。

| Task | machine verdict | scope-limit reason | manual reconciliation |
|---|---|---|---|
| T027 | PARTIAL | worktree/branch は repository diff artifact ではない | 現在の isolated worktree と branch を確認 |
| T028 | PARTIAL | frozen baseline は candidate diff に再出現しない | baseline、retire manifest、final comparator を照合 |
| T030 | PARTIAL | deletion は presence matcher では陽性化できない | consumer/credential path の不在を source と static scan で確認 |
| T031 | NOT_FOUND | task が deletion target の source path を指定していない | sidecar dispatch の不在、T053 denylist、build/test を確認 |
| T032 | SKIPPED | bootstrap deletion に path/symbol/acceptance artifact がない | template 参照なしを source/CLI help で確認 |
| T037 | PARTIAL | referenced test は candidate diff の変更対象外 | peer error mapping と production caller/export を確認 |

## Security review matrix

| Finding | Disposition | candidate evidence |
|---|---|---|
| degraded redaction worker failure could admit a persisted control identifier | fixed | `isSafePersistedText` now rejects `intake.degraded`; regression test covers injected worker failure |
| legacy cutover could publish the current pointer before tombstone/final owner checks | fixed | migration/publication moved after identity, tombstone, owner-scan, and manifest checks; ordering regression test added |
| process loss after tombstone but before pointer publication required manual recovery | fixed | startup verifies the exact tombstone and matching recovery hardlink, restores the legacy path, and reruns cutover; preserved-row regression added |
| unauthenticated viewer JSON handling buffered a chunked body before enforcing its size limit | fixed | installed Hono `bodyLimit` now rejects both JSON POST routes before `readBoundedJson`; the existing auth security case proves 413 before the full stream or RPC dispatch |
| loopback-port cookie scope exposed viewer sessions to another local listener | fixed | cookie auth was retired; the browser holds an origin-scoped `Authorization: Session` value, and cookie-only requests are rejected |
| existing-viewer login and custom DB viewer ownership were not bound to one runtime endpoint | fixed | PID ownership now lives under the resolved private runtime root; exact host family / port and daemon-issued nonce exchange are required before reuse or stop |
| custom `--db-path` runtimes shared parent control paths and callers dropped the explicit DB path | fixed | fixed-length path-hash runtime roots are resolved once and propagated through CLI hooks, MCP, viewer, setup, and legacy cutover recovery |
| backup create / restore could exceed the shared 2-second RPC deadline after committing | fixed | backup methods use durable operation journals and GET recovery; same-ID replay is stable and restore stop runs after late completion |
| interrupted restore recovery trusted a result sidecar without rechecking the activated SQLite artifact | fixed | before writer open, noncommitted restore journals require the saved main-file SHA-256 and absence of WAL/SHM; mismatch preserves pointer, journal, artifact, and sidecars and fails closed |
| viewer API helper could attach its session header to a future absolute-URL caller | fixed | `viewerFetch` accepts only relative `/api/` paths before adding authorization; the existing browser-auth test proves a cross-origin URL is rejected before `fetch` |
| viewer config projection exposed `observer_api_key` | fixed | the browser-facing projection now redacts the supported plaintext credential and has a literal-key regression |
| daemon-job POST/GET lifecycle was reported as a retry concern | rejected by contract | user-triggered T045 Class-C work is one POST plus GET polling and is never auto-retried; only pending internal backfills resume as fresh jobs after a durable `daemon_restarted` failure |
| MCP request IDs were reused after a stdio server restart | fixed | request identity now includes the transport session or one per-server scope; all ten read/write callers share the same root helper |
| `memory_search` exposed internal `body_text` and omitted public `body` | fixed | only the public `memory_search` selector restores the pre-RPC eight-field response shape |
| `codemem mcp` no longer accepted the shared `--db-path` option | fixed | the existing shared option resolver sets `CODEMEM_DB` before the stdio server import; daemon-only ownership remains unchanged |
| setup rollback snapshots checked and read editor config by path in separate operations | fixed | snapshots use one non-following file descriptor for `fstat` and bytes, then verify post-read path identity; deterministic inode-swap regression leaves the replacement untouched |

Valid initial findings were fixed through `546edd3`; the daemon-job report is not a defect under the explicit Class-C contract. Independent correctness, manual security, and Ponytail re-reviews returned blocker-free. The dedicated Codex Security runner could not start because the active global config combines `multi_agent_v2` with `agents.max_threads`; persistent user configuration was not changed, and the initial candidate diff received the manual security review instead. Semgrep findings were reviewed with no remaining valid High/Critical issue. CodeQL alerts `#29` / `#30` were dismissed as false positives because their test-only `/tmp` sources reach non-creating `r` / `r+` opens on existing private paths; the aggregate check then passed. The later deltas through `87195f5` have their own closure-refresh correctness/security/Ponytail reviews, and `55323dd` received the test-contract and Ponytail reviews above.

`.codacy.yml` uses documented `include_paths` to include the vendored product source and engine-specific Lizard exclusions only; this is a scope override, not an allowlist. Live Codacy status remains an external push-time check. See [Codacy configuration file](https://docs.codacy.com/repositories-configure/codacy-configuration-file/).

## Upgrade and rollback commands

Run from the target installation after selecting the canonical SQLite path with `--db-path`; backup IDs are returned by `backup create|list`.

```bash
# pre-upgrade: create and verify a local recovery point
codemem backup create --reason pre-upgrade --json --db-path <path-to-mem.sqlite>
codemem backup list --json --db-path <path-to-mem.sqlite>
codemem backup verify <backup-id> --json --db-path <path-to-mem.sqlite>

# candidate checkout validation
corepack pnpm install --frozen-lockfile
pnpm run build
pnpm run check
pnpm run codemem --help

# rollback: restore only a verified local backup, then restart the daemon
codemem backup restore <backup-id> --json --db-path <path-to-mem.sqlite>
codemem status --json --db-path <path-to-mem.sqlite>
```

Backups can contain private/local-only data; Phase 1 supports local backup only.

## Terminal external step

Product-code commit `cfbcd6f` includes the later setup hardening and test-only closure `55323dd` and is the verified PR #7 code/test head. The repository evidence commit records its green checks; merge `main`, recheck the resulting `main` CI, then migrate the Sonar long-lived branch only after confirming the expected branch IDs and clean Quality Gate. These external mutations occur after this evidence commit rather than being presented as local validation.
