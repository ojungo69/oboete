# 段階 5: 忘却と安全性

正本は [spec.md](spec.md) §6、§8.4 の段階 5 と、後から承認された
[spec-webui.md](spec-webui.md)。この計画はそれらの実装順序を定める。
段階 3・4 の計測結果を変更せず、段階 5 の完了やオーナー環境への切替を宣言するものではない。
開始点は Design B `902bf4d`。オーナーの v1、claude-mem、実データ、サービス、認証設定には触れない。
実装と障害試験には合成データの一時 home を使う。Spec Kit や別の計画承認は追加しない。

## 操作の意味

| 操作 | 効果と復元性 |
|---|---|
| 記録しない | repo/folder の将来の記録を止める。再許可しても停止中の記録は戻らない。repo の設定は制限のみ。既存内容は別の forget 確認を経て消す。 |
| mute | owner correction の独立した値。検索には残し、digest を含むすべての注入から外す。unmute できる。retracted の代用ではない。 |
| withdraw/exclude | 共有を止め、記録元は原本を保持する。解除後は再公開できる。現在の exclude は外部送信停止であり capture 停止でも forget でもない。 |
| forget | 不可逆。uid 指定はその claim/document のみで raw は残る。raw scope は証拠を持つ claim を uid ごと消し、残った raw から再導出する。 |
| 訂正 | 所有者が本文・状態を訂正する。rebuild/recurate 後も保持するが、forget の tombstone に優先しない。 |

## 保存と同期の境界

新しいサービスや汎用 job framework は作らない。`forget.rs` に型付き target と
`preview / start / resume / status` を置き、CLI と WebUI が同じ処理を呼ぶ。

### D1. 復元で巻き戻らない制御情報

`raw.db` に tombstone、deny、`forget_jobs` を一つの transaction で適用する。
ただし原本だけでは、古い segment からの restore が削除要求まで巻き戻す。
そこで home 直下の `privacy.db` に本文なしの削除要求を先に耐久保存する。
これはローカルの削除制御履歴であり、raw restore/rebuild が交換・削除しない。
SQLite の既存依存を使い、rollback journal と `synchronous=EXTRA` を使う。
EXTRA は commit で rollback journal を消す directory entry の同期も要求する。
WAL の別ファイル管理も、JSONL の破れた末尾・再採番・独自 transaction も増やさない。

`privacy.head` は形式版、履歴の identity、耐久化済みの通番だけを保存する。
既存の fsync と rename による小さいファイルの置換を使う。要求の commit 後に head を進める。
要求だけが先行した中断は再開できる。head より古い履歴、identity 不一致、未知の形式、
要求後の履歴欠損・読取不能はエラーにし、空の deny として扱わない。
初回作成は要求を書き始める前に head を耐久化する。

削除登録は raw の短い `BEGIN IMMEDIATE` 内で preview を再確認し、制御履歴を保存してから
raw の tombstone/deny/job を commit する。ネットワーク、AI、purge、バックアップ圧縮をこの
transaction に入れない。履歴だけ残った場合は raw の次の open/worker/restore が再適用する。
適用済み通番は raw の metadata に保持し、適用は idempotent にする。
restore は交換前の DB に制御履歴を適用し、失敗した場合は交換しない。

最終的な派生書込には deny 確認と commit を直列化する短い fence を設ける。
現在の rescan は knowledge transaction 内で別 raw writer を開くため、consumer 全体を
raw の write transaction で囲まない。第一 slice は raw 書込の境界だけを担当する。

### D2. 再 import の identity

imported document は既存の uid/source/source_id を使う。raw record には版付きの
source identity と fingerprint を別表で持たせる。v1 は source device と元 event id、
transcript は agent/session と安定した event ordinal を使い、内容の fingerprint で
同じ id の差替えを見分ける。コピー元のファイルパスは identity にしない。
制御履歴にはこれらの hash と record id だけを残す。検索文・本文・引用は保存しない。
native identity がある import はその identity を比較し、同じ文字列の別 event まで消さない。
チェックは append の transaction 内。fingerprint だけでは parser/redaction の変更をまたぐ
再 import 防止を証明できないため、第一 slice は native identity のない legacy/hook raw の
登録を具体的な理由付きで拒否する。既存の忘却がある状態で provenance を渡さない raw import
も明示的に拒否する（本文を運ばない touch metadata を除く）。未知の版や読めない制御履歴は
import を止める。単なる文字列 sentinel は本番の deny にしない。
最終 M5 には live hook の source identity と legacy の対応付けを含める。保存済み provenance
から確実に対応付けられないものを「再 import 安全」と呼ばず、無関係な session 全体の deny
で隠さない。この bridge の完成前はオーナー切替をしない。

### D3. preview と確認

preview は対象 identity、件数、必要なサンプルと制限を返すだけで job を作らない。
解決した record 集合と raw/op/control の版から preview token を作る。
start は同じ scope を transaction 内で再解決し、token 不一致なら保存前に拒否する。
大きい集合は上限を明示して範囲を分け、64 KiB の op に大量の uid を詰め込まない。
session tombstone はその session の遅れて届く op も拒否する。repo/time の削除は確認した
端末ごとの上限までを対象にし、将来の新しい記録を止める操作は capture exclusion とする。
`--from-search` の文は hidden prompt/stdin または POST body からだけ受け、永続化しない。
remote embedding は使わず、`--yes` との併用を拒否する。

### D4. 本当の進捗だけ返す

job は local purge と hub delivery を別に持つ。最初の slice は
`local=pending_physical_purge` から進めず、`done` と返さない。
全ローカル段階と index merge が終わって初めて `local=done`。
hub が未設定なら `hub=not_configured` とし、ローカル完了を妨げない。
制御履歴は保持するので、後に接続した hub へ control-first で送れる。
設定済みなら waiting/acked を区別し、acked を hub purge 完了と表示しない。
第一 slice は hub を調べず `hub=not_connected` と表示し、M6 の接続後にその状態を拡張する。

## 実装順序と完了条件

| Slice | 対象と再利用する処理 | 完了条件 |
|---|---|---|
| 1 制御履歴 | `forget.rs`, `raw.rs`, `backup.rs`, `worker.rs`, `main.rs`, `migrate.rs`, `transcript.rs`。既存 SQLite transaction、raw.lock、import checkpoint。 | native provenance のある新しい raw import の record/device span（登録ごと最大500件）の preview/start/resume/status、原本と別の耐久 deny、古い backup restore と再 import による復活防止。物理 purge は未完了と表示する。native identity のない raw、uid/session/repo/time の start は後続接続まで拒否する。 |
| 2 uid の完全削除 | Slice 1 に `claims.rs`, `consumer/*`, `embed_phase.rs`, `search/b.rs`。既存 no-AI rebuild を内部で共有。 | claim/document uid の read backstop、raw op 本文・全 derivation・correction・digest・FTS・vector cache と backup の purge、失敗から再開。claim-only は raw 0 件。 |
| 3 raw scope の完全削除 | 同じ pipeline を session/repo/time/range に拡張。live/legacy provenance bridge、大きい selection のページ処理、`Span::minus`, recurate queue。 | 第一 slice の provenance/500件制限を解消する。重なる window summary と派生物も消える。実行中の curator/digest/embed の返答は commit しない。#167 の支援 evidence を持ち、失われた場合は proposed に戻る。#186 の preference はマスク済み raw から deterministic に再導出する。 |
| 4 可逆な管理 | `CorrectionOp`, `Pending`, 既存 exclusion op/clock、capture 共通経路。 | mute/unmute、capture exclusion、withdraw、訂正。#157 anchor 継承、#162 clock 後退、#160 restore 後の失敗報告を含む。 |
| 5 WebUI | `view.rs` の `save_gate`、`assets/viewer/`、同じ型付き backend。 | 日英で preview/確認/進捗/制限を表示。correction/mute/capture/withdraw/forget/global preference を操作でき、CLI 編集を要求しない。#94 の coverage matrix に実装証拠を残す。 |
| 6 安全性と測定 | doctor、provider scratch sweep、OS の secret file helper、合成 failure harness。 | 下記の削除 canary、M22、A104、security review。#281/#285 を保持して解決し、権限未対応を成功扱いにしない。 |

最初の writer fence は Slice 1 のファイル、この計画書、`tests/forget_cli.rs`。
公開境界は CLI と `forget` の型付き API。内部 table 名を正解とするテストではなく、
preview が変わる、復元後も get/import から戻らない、再開しても二重登録されない等を確認する。

## 完全削除で落としてはいけないもの

- raw の event だけでなく、ops の claim/evidence/correction/digest/import/window summary。
- 全世代の `vectors`、`vector_keys`、bit index、shortlist query vector。
  現在の `touched` は cache 本体を消さず、`carry` は退避 DB の全 cache を戻すため修正する。
- live DB と WAL/SHM、backup の旧世代、quarantine、rebuilding、restore tmp、provider scratch。
  migration snapshots と eval copies は仕様どおり別の limit として grep 件数を報告する。
- backup の既存 checksum/segment pair に新規用 `seal` をそのまま上書きしない。
  世代の入替えを再開可能にし、中断で無関係な record を restore が捨てないことを試験する。
- retention は同じ物理書換えを使うが tombstone を作らない。未処理 raw を残し、claim は
  `raw expired` の証拠表示で保持する。

## 検証と段階の境界

各段階境界、SQLite commit、ファイル rename/fsync の前後で停止し、再開後に対象が戻らず、
無関係データと checkpoint が保たれることを確認する。既存 `crash.rs` は commit の障害試験に
再利用し、ファイル処理と実プロセス kill の試験を加える。disk full、permission failure、
未知/破損/古い制御履歴は fail closed。read/derived commit と登録の race は barrier で再現する。
バックアップの圧縮前後と DB の物理 byte grep の両方を確認する。

通常の必須チェックは `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、
`cargo test`。privacy、削除、restore、viewer write は security review の対象。
CI/security gate は弱めない。実装は段階ごとのローカル commit までとし、今回 push/PR/merge はしない。

- M4 の削除測定のローカル部分: 管理対象内で 0 hit。復元、再 index、再 import、crash、
  in-flight、rescan、retention の各ケースを含む。これは「段階 4」の検索品質測定とは別。
- A104/Transcript: canary を forget 後、同じ v1/claude-mem/transcript を再 import しても戻らない。
- M22 forget: 段階 4 Task 12b の scale-home 作成器と stub を再利用し、hook writer と並行して
  段階別時間・検索/注入遅延・未完了理由を測る。仕様にない固定秒数を合格線として加えない。
- 合成データによる build/故障試験はラベル・実モデル・held-out corpus から独立して進める。
  cut-over と M5 完了には段階 3・4 の既存品質ゲートも必要。今回 owner cut-over はしない。
- device/hub/R2/Vectorize の purge、ack、stale-device、hub reseed は M6。
  本段階はその bodyless control の適用入口を備え、通信サービスは先行実装しない。

## 実装状況

- [ ] Slice 1: 制御履歴と raw record/span の安全な受理
- [ ] Slice 2: uid の物理 purge
- [ ] Slice 3: raw scope、rescan、retention
- [ ] Slice 4: 可逆な管理と訂正
- [ ] Slice 5: WebUI
- [ ] Slice 6: 安全性・測定・独立レビュー

未チェックの slice がある間、この段階は完了ではない。

### 第一 slice の実装・検証（2026-10-02）

Design B `902bf4d` に対するローカル差分。第一 slice の backend と CLI を実装し、
独立レビュー前の状態。`forget_jobs.step` は 1 のままで、物理 purge 完了を返す経路はない。
起動時の再適用は `raw::open`、restore の再適用は既存の swap 前に行う。

- `cargo fmt --all --check` と `git diff --check`: 成功。
- `TMPDIR=/var/tmp/oboete-m5-tmp CARGO_BUILD_JOBS=4 cargo clippy --all-targets -- -D warnings`: 成功。
- 同じ環境の `cargo test`: 918 成功、3 ignored。全 suite は通常環境で実行し、
  `OBOETE_NO_SPAWN` を suite 全体には設定していない。
- 公開 CLI 7 件: 登録と未完了表示、native identity 欠損の登録拒否、制御ファイル欠損、
  原本破損、古い backup、v1/transcript 再 import、redaction 変更、古い/未知/破損した制御履歴。
- 型付き API の試験: stale preview、raw commit rollback、初期化中と登録直後の実 process kill、
  head 書込失敗、SQLite page limit による実 `SQLITE_FULL`、古い window 応答の拒否、
  同文で別の native event の保持。障害試験にオーナーのデータ・モデル・サービスは使っていない。

この結果は全 M5 の削除 canary 成功ではない。物理 purge、legacy/live identity bridge、
大量 selection、uid/session/repo/time、可逆な管理、WebUI、各 OS の安全性と M22 は後続。
