# Claude Code 引き継ぎ — 009 memory core

2026-09-11 更新。US5 の security review を閉じて commit した。009 全体は未完了。
前回の引き継ぎ (2026-09-10、Codex から) はこの文書で置き換える。

## 最初に行うこと

1. `/home/jura/projects/free-mem-wt/009-memory-core` で作業する。branch は `009-memory-core`。
   この文書の commit の直下が `0e92d4b4` (macOS runbook)、`849fde57` (US5 security 修正)、
   `7f37376c` (E3 matrix)、`943660a4` (import promote)、`590c0a2f` (A–E2 checkpoint)。
   origin/main に対して 21 commit 先行、遅れ 0。branch は未 push、PR は未作成。
2. `git status --short` で作業ツリーが clean か確認する。
3. [spec](spec.md)、[plan](plan.md)、[tasks](tasks.md)、[migration contract](contracts/migration.md)、
   [quickstart](quickstart.md) (E1〜E4 が US5 の証拠) を読む。owner 判断はメモリ
   `oboete-009-owner-decisions-2026-09-11` に固定してあり、聞き直さない。

許可の範囲 (2026-09-11 owner 判断): グローバルルール (`pr-merge-gate` 等) に従う限り commit /
push / PR / merge まで可。日常用インストール、実エージェント起動、実モデル・有料 API、クラウド転送、
アカウント変更は別の activation。security 関連のコードは外部 CLI に委譲せず Claude Code が書く。
Codex を起動する shell からは API key 類を `env -u` で外す。

## 現在の実装

- T001–T019、T021–T022、T025–T028、T046 は証拠付きでチェック済み (変更なし)。
- US5 (T029–T032、未チェック): native V1/V2 streaming reader、private SQLite plan、preview/apply、
  origin receipts、claude-mem query export adapter、`import promote --work/--list`、
  10 ケースの matrix、そして今回の security 修正。
- `849fde57` の内容 (quickstart E4 の表が正本):
  redacted personal hash は既知の personal projection 行しか選べない (held、tombstone 化しない)、
  sensitivity は全経路で raise-only (`UPDATE` の rank guard、dependency source は edge のみで親より
  低い rank を拒否、`raiseToParents` の identity-keyed worklist + trigger 子孫再同期、unverified は
  最後に merge)、receipt は最終 merged 状態と live 行の厳しい方 (`identity_elsewhere` は hash-only、
  proposal は projection の最終状態も継承)、`orphan_origin_payload` validator、`promote --list` は
  payload を読まない。0007 は変更なし。
- レビュー: Codex read-only 9 巡 (最終 `secrev13`、0 finding)、`/code-review high` ok:true
  (repro 168/168)、`ponytail-review`、semgrep 0。最終 gate `us5-sec12-*` (両 Node 1,177 + 202、pack)。
- 受容した残件は contract の Known validator limits と quickstart E4 末尾に書いてある
  (cache 基準の件数、`historical_held` は unchanged 扱い、proposal receipt は origin memory 追従、
  Codex 第 9 巡が提案した #15/#16 の追加ケース)。

## 進行中: E5 (RSS / wall time)

- Codex job `task-mtvzonzs-xg4byp` を `/home/jura/projects/free-mem-wt/009-rss` (849fde57 を
  detached checkout) で起動済み (2026-09-11 05:38 JST)。内容: `mergeTransferPlan` の plan.db 書き込みを
  1 transaction で囲む、scratch に `PRAGMA synchronous = OFF`、既存テスト全緑、
  `/var/tmp/oboete-009-20260909.jJ5grc/us5-rss/` と同じ手順で再計測 (出力は
  `/tmp/oboete-009-20260909.jJ5grc/us5-rss2/`。**/tmp は再起動で消えるので読んだら即 /var/tmp に
  コピーする**)。プロンプトは `/var/tmp/oboete-009-20260909.jJ5grc/scratch/us5-perf-task.txt`。
- 回収手順: `cd ~/projects/free-mem-wt/009-rss` で `codex-companion.mjs status --json` →
  `result`。diff を 009-memory-core に持ち込む前に `/code-review` → `ponytail-review`、
  perf 変更は security 対象外だが scratch 以外の PRAGMA を触っていないことを確認する。
- その後 `contracts/migration.md` の RSS 予算文を「import/export CLI 512 MiB、hook/worker 150 MiB」に
  変え、実測 peak を appendix に載せ、quickstart に E5 節を書く。

## 再開順序と未完了事項

1. **E5 を閉じて US5 を閉じる**: 上記の perf 回収 → gate (`sec-gate.sh` は
   `/var/tmp/oboete-009-20260909.jJ5grc/scratch/`、`P=` を変えて setsid で起動。並行 build/review 禁止、
   serial の 300 ms seed miss は単独再実行) → T029–T032 をチェック → PR を `pr-merge-gate` で開く。
   T029 の wire appendix と `research.md` の一次資料は E4 までで contract に反映済みか再確認する。
2. **macOS (T040 / SC-006)**: `docs/evidence/memory-core-2026-09/macos-runbook.md` を M1 iMac
   (remote desktop) で実行し、receipt を `/var/tmp/oboete-009-20260909.jJ5grc/macos/` に戻して
   quickstart に記録する。platform probe のみ、agent pair は対象外。
3. **US6**: T033 まで (contracts/sync.md + research、依存追加なし) で止めて確認。transport は
   ユーザー指定ディレクトリの暗号化 bundle file、Node `crypto` のみ。migration merger は sync の
   現行 work 更新に流用しない。
4. **US7 + amendment**: T037–T039、T047 (session スコープ常駐、hook 起動、lease 所有、idle exit)、
   T048 (detected local + consented free presets、有料は自動選択しない)。
5. **実測と最終 gate**: T020、T023–T024、T041–T045。実 agent pair・実モデル・100k events・7 日運用は
   merge 後の follow-up issue + dogfood cron で追う (owner 判断)。

## 検証の作法

- 生ログは `/var/tmp/oboete-009-20260909.jJ5grc/` (prefix `us5-*`、review は `us5-sec-reviews/`)。
  `/tmp` に置いた Codex 出力・script・prompt は 2026-09-11 05:18 の再起動で消えたので、長寿命の
  ものは `/var/tmp` に置く。
- gate は typecheck → lint → build → 両 Node の unit glob → serial glob (`--test-concurrency=1`) →
  pack-check を順番に。並行して build や重い python を走らせると hook seed の 300 ms deadline を
  外して worker/lease 系が落ちる (単独再実行で 202/202)。
- Codex security review は `codex exec --sandbox read-only -C <wt> -` の fresh session、prompt は
  `us5-sec-reviews/secrev*/fix*.txt` の形式 (対象 = staged 差分、脅威モデル、severity 基準、JSON
  schema)。

## Claude Code への開始指示

```text
/home/jura/projects/free-mem-wt/009-memory-core で作業してください。
specs/009-memory-core/HANDOFF-claude-code.md と、そこで指定された spec/plan/tasks を読み、
「進行中: E5」の Codex job を回収するところから続けてください。
commit/push/PR/merge はグローバルルールに従えば可。日常用インストール、実モデルやクラウドの
activation は未許可です。
```
