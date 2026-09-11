# Claude Code 引き継ぎ — 009 memory core

2026-09-11 更新。US5 の security review を閉じて commit した。009 全体は未完了。
前回の引き継ぎ (2026-09-10、Codex から) はこの文書で置き換える。

## 最初に行うこと

1. `/home/jura/projects/free-mem-wt/009-memory-core` で作業する。branch は `009-memory-core`。
   直近の commit: `04bd7ac6` (sync 契約)、`b2e9e931` (E5 docs)、`97bbe882` (perf)、`4257eeee`
   (前回 handoff)、`0e92d4b4` (macOS runbook)、`849fde57` (US5 security 修正)、`7f37376c`、
   `943660a4`、`590c0a2f`。
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

## 完了: E5 (RSS / wall time) と T033 (sync 契約)

- E5 (`97bbe882` + `b2e9e931`): scratch merge を 1 transaction + prepared statement cache
  (`src/db/statements.ts`)。near-limit apply 3,228 s / 304 MiB → 108 s / 181 MiB。全 run が
  import/export CLI 予算 512 MiB 以下。receipt は `us5-rss3/`、切り分けは
  `us5-rss2/experiments/README.md`。gate `us5-perf1-*` 緑。
- T033 (`04bd7ac6`): `contracts/sync.md` + research R8。Codex 契約レビュー 8 巡を反映、第 8 巡の
  4 件は未確認のまま折り込み (契約の "Review status")。**owner の確認待ち** (この checkpoint で
  scope を決める: 契約どおり T034–T036 を実装するか、US6 を 009 から外して follow-up spec にするか)。
  T034 は着手前に契約レビューを再実行する。
- PR は `008-qd-d` (#185) を base に作成済み (branch は #185 から切ったため。#185 merge 後に main へ
  retarget される)。マージは `pr-merge-gate` に従う。

## 完了: US6 (T034–T036、2026-09-11)

- owner が「契約どおり実装」を選択。`src/sync/` 10 モジュール + 0008 + `oboete sync` CLI + MCP
  `sync_status` (read-only) + doctor `sync` 項目 + `sync_approvals` (承認時に記録)。
- テストは `test/unit/sync*.test.ts` 7 ファイル 141 ケース (`test/helpers/sync.ts` 共有 fixture、
  `sync-review.test.ts` はレビュー指摘の pin)。
  重い 2 ケース (256 MiB push、180,000 行 graph) は `OBOETE_SYNC_HEAVY=1` でのみ実行
  (receipt: `/var/tmp/oboete-009-20260909.jJ5grc/us6/heavy-bounds.log`)。
- 検証一覧を書く過程で apply/publish の欠陥 7 件 + CLI 入力 1 件を修正 (tasks.md の T034–T036
  注記に列挙)。契約は "Implementation notes" に amend 4 点を記録。
- 単一ファイルのテスト実行は `/var/tmp/oboete-009-20260909.jJ5grc/scratch/test-one.mjs`
  (`TEST_ONE_SLOT=<name> node test-one.mjs test/unit/x.test.ts`、build/ を触らない)。
- レビュー: `/code-review high` 3 巡 + finder 1 角度、Codex correctness pass (`us6/secrev1..10`。security
  枠のプロンプトは OpenAI の cyber classifier に切られるので correctness 枠で出す) で 10 + 6 + 9 + 9 + 14 + 11 + 17 + 16 + 20 + 12 + 4 件。
  round 11 は 4 件中 3 件(move の parent、bound tombstone の後着 sweep、未解決 dep の raw-event member)を修正・pin。
  4 件目(相互 cross-memory context cycle を単一デバイスが物理保持した場合の source 行 loss)は未解決。
  cycle-break を「既適用 edge に対して head 1 本ずつ増分判定」する形なので、どの edge を落とすかが処理順(=random
  replica id)依存で、adverse fixture の実測で 6 割の run が device 間 divergence。content-key 順の処理と lossless-restore は
  入れたが cycle-break 自体は決定的にならない。完全修正は「cycle の break edge を content で正準選択する pass」か
  「capture/apply の cycle 対称化(capture-model 変更、round8 pin の挙動が変わる)」で、いずれも設計変更。owner に go/no-go 確認中。
  0008 は branch 内で in-place 編集した (未リリース)。旧 0008 を適用済みの DB は作り直す。
  全件 security-owned code を Claude Code が修正し RED→GREEN で pin。round 4 で source の同一性を
  content-derived hash から保存 key (`memory_sources.sync_key`) に設計変更 (3 巡続けて同じ箇所が
  指摘されたため)。round 7 で key を乱数にし (device 間の同一性は UNIQUE tuple のみ)、tuple 待ちを
  pass 前の row 退避 (parking) に置き換えた。round 8 で key を内容由来に戻し (tombstone は source では
  復活可能な状態)、tuple と lineage を同一性として運び、parking の穴を塞いだ。round 9 でその閉包を閉じた
  (writer でも key は memory ごと、retirement は frontier 全部、tuple は revision ごと、pass 外の held 行、
  lineage 結合は全行 store 後、source の head 選択は revision id で決定的、tuple を奪われた退避行は
  merge しない)。round 10 でその閉包を閉じた (withheld payload の fill で tuple を運ぶ、lineage 結合は
  同一 local memory の中だけ、terminal 削除は pass 後の sweep で全 holder に届く、resolution は残す head の
  tuple を持つ、retirement は union 前に rebind、pass の退避は pass 内だけで永続化しない = round 9 で入れた
  `sync_parked` テーブルは撤去)。semgrep 0、ponytail。契約の "Implementation notes" に round ごとの規則を記録。
  gate は `scratch/gate-us6.sh` (`P=us6-gate1`)。

## 再開順序と未完了事項

1. **US5 を閉じる**: PR の bot/CI 指摘を `pr-merge-gate` で処理 → T029–T032/T043 をチェック。
   gate の再実行は `sec-gate.sh` (`/var/tmp/oboete-009-20260909.jJ5grc/scratch/`、`P=` を変えて
   setsid で起動。並行 build/review 禁止、serial の 300 ms seed miss は単独再実行)。
2. **macOS (T040 / SC-006)**: `docs/evidence/memory-core-2026-09/macos-runbook.md` を M1 iMac
   (remote desktop) で実行し、receipt を `/var/tmp/oboete-009-20260909.jJ5grc/macos/` に戻して
   quickstart に記録する。platform probe のみ、agent pair は対象外。
3. **US6**: 実装済み (上記)。残りは PR #190 の bot/CI 指摘処理と merge (#185 の後)。
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
PR の状態と「完了: E5 と T033」の owner 確認結果から続けてください。
commit/push/PR/merge はグローバルルールに従えば可。日常用インストール、実モデルやクラウドの
activation は未許可です。
```
