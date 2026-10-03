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
`preview / start / status` を置き、CLI と WebUI が同じ処理を呼ぶ。

### D1. 削除要求の権威は raw.db、本文なしの要求ログを 2 部（設計 v2）

2026-10-03 に設計 v1（raw.db の外の `privacy.db` と `privacy.head` を open のたびに比べる）を
置き換えた。v1 では制御ファイルが欠けている・古い・他 home のもの・壊れているだけで全 hook の
open が失敗して記録が止まり、戻すには手でファイルを消すしかなかった。restore を使う普通の理由で
ある home の喪失も守れなかった。spec 6.2・5.8・2.6 も tombstone、deny-list、job 行を raw.db に
置き、別の journal を持たない。

- **raw.db が唯一の権威。** `forget` の登録は raw の書込 transaction 1 つで、tombstone（type
  `tombstone`、source `forget`）、import origin で鍵を付けた `denied_records`、本文なしの要求 JSON
  全体を持つ `forget_jobs` を書く。hook は制御状態を読み書きしないので、制御状態が記録を止めない。
- **raw の書込ロック中は raw 以外を読み書きしない。** raw の commit の後にログへ追記し、各コピーの
  本当の状態（home と backup の横の両方に記録済み、またはどちらが書けず restore で何を意味するか）
  を表示する。restore は swap のロックを取る前に両方のログを読み、ロック下で live raw.db を見直す。
- **本文なしの要求ログ 2 部。** `<home>/forget.log` と `<backup dir>/forget.log`。1 要求 1 行の
  JSON で、登録した home の安定した id、ランダムな 128 bit の job id、record ごとの device・seq・
  origin hash・session hash を持つ。時刻は transcript の切れ目に数える record だけが持つ。本文、
  パス、本文の fingerprint は持たない。1 要求 500 record、1 行 256 KiB まで（大きい対象は command
  が複数の要求に分ける）。追記は排他ロック下で 1 回ずつ、末尾が改行でなければ先に改行を書き、
  `fsync` し、作ったときはディレクトリも同期する。各行に payload の checksum を付け、checksum
  や形が合わない行、他 home の行は、その行だけ飛ばして報告する。
  home の id は raw.db の metadata に置き、最初の device id から作る。ファイルのコピーで
  将来の記録用の device id が変わっても保持し、record の backup にも運ぶ。別の権威は作らない。
- **reconcile は双方向で、副本の障害では拒否しない。** worker の起動と stores を開き直すたび（`backup::check`
  の後、consumer と export の前）、`oboete restore`、forget command、import command の取り込み前に、
  raw にあってコピーにない要求をコピーへ書き、コピーにあって raw にない要求を raw へ適用する。
  読めない・書けないコピーは報告して飛ばす（command の出力、`forget --status`）。`raw::open` と
  hook は reconcile しない。読めた要求を raw.db に適用できなかった場合は、deny のないまま
  import・consumer・provider が進まないよう caller へ失敗を返す。
- **適用は seq ではなく identity で。** identity は import origin だけで、`(device, seq)` は出所の
  記録にとどめる。要求の適用は、保存された import origin が一致する record をすべて（再 import 後の
  重複も）tombstone で隠し、deny 行は origin で `INSERT OR IGNORE` する。Claim と Correction の op
  は anchor record の origin が deny されていれば拒否する。再利用された seq を古い要求で隠さない。
- **restore は live raw.db の要求も運ぶ。** 読める raw.db の上の restore は、その `forget_jobs` を
  identity で再構築後の raw に適用してから swap し、ログにも書く。job のない deny 行（segment から
  再構築したもの）はそのまま残す。backup の tombstone 行は deny 行を運ぶ。
- **派生書込の fence。** `curate::Reading` は window の入力を読む前に `denied_records` の件数を読み、
  Window と Turn の全 writer が raw の追記 transaction の中でそれと比べ、違えば何も記録せずに
  切り直す（`append_ops_fenced`）。knowledge.db が raw の tombstone に遅れている間（Anchors の
  checkpoint より後に tombstone がある間）、window・turn・embedding の各 phase は待つ。preview
  token も同じ件数を含む。
- **第一 slice で忘れられるのは import origin のある record だけ。** live と回復した record に
  native identity を与えるのは slice 3a。

障害の範囲（実装前に決めた打ち切り基準）: 第一 slice は F0（障害なし、通常の並行）、F1（raw.db の
喪失・破損、backup と片方のログは残る）、F2（raw.db だけが古いコピーに置き換わる）で正しい。F3
（ログと既定の backup を含む home 全体が古いコピーに置き換わる）と二重障害は、forget が表示し
spec 6.3 に書く限界とする。forget が表示する限界は次のとおり。

- raw.db が古いコピーに置き換わった後、worker がログを適用するまでの間、最初の hook が忘れた
  record を SessionStart に注入しうる。登録の時点で hook が作り終えた packet と送信済みの provider
  呼出しは取り消さない。
- raw の commit と最初のログ追記の間の crash の後、次の worker 起動の前に raw.db を失うと要求は
  失われる。raw.db と両方のログを失った場合も同じ。
- ログの行が 1 つも残らない segment からの restore は deny 行を再構築するが job 行は戻らない。
- ログと backup ディレクトリは origin と session の hash を持つ（本文はない）。v1 と transcript
  から取り込んだ record の origin は保存された payload の hash を含むので、ログを持つ人は保存された
  文面の推測を確かめられる。別の identity は slice 3a の課題。
- 第一 slice は物理 purge をしない。古い backup segment には本文が残り、replay のときに tombstone
  で隠れる。エージェント自身の transcript も本文を持ち、forget はそのパスを表示する。

### D2. 再 import の identity

imported document は既存の uid/source/source_id を使う。raw record には版付きの
source identity と fingerprint を別表で持たせる。v1 は source device と元 event id、
transcript は agent/session と安定した event ordinal を使い、内容の fingerprint で
同じ id の差替えを見分ける。コピー元のファイルパスは identity にしない。
要求ログにはこれらの hash と record id だけを残す。検索文・本文・引用は保存しない。
native identity がある import はその identity を比較し、同じ文字列の別 event まで消さない。
importer は capture 前の agent/session から session hash を作り、origin と同じ transaction で
保存する。伏せ字後の label からは作らず、本文を持たない要求と record backup にも運ぶ。
その hash がない旧 record の forget は、対応を推測せず登録前に拒否する。
チェックは append の transaction 内。fingerprint だけでは parser/redaction の変更をまたぐ
再 import 防止を証明できないため、第一 slice は native identity のない legacy/hook raw の
登録を具体的な理由付きで拒否する。既存の忘却がある状態で provenance を渡さない raw import
も明示的に拒否する（本文を運ばない touch metadata を除く）。import command は取り込みの前に
要求ログを reconcile するが、読めないログで import を止めない（D1）。単なる文字列 sentinel は本番の
deny にしない。
最終 M5 には live hook の source identity と legacy の対応付けを含める。保存済み provenance
から確実に対応付けられないものを「再 import 安全」と呼ばず、無関係な session 全体の deny
で隠さない。この bridge の完成前はオーナー切替をしない。

### D3. preview と確認

preview は対象 identity、件数、必要なサンプルと制限を返すだけで job を作らない。
target、解決した record 集合、`denied_records` の件数から preview token を作る。
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
`forget_jobs` は本文なしの要求を保持するので、後に接続した hub へ control-first で送れる。
設定済みなら waiting/acked を区別し、acked を hub purge 完了と表示しない。
第一 slice は hub を調べず `hub=not_connected` と表示し、M6 の接続後にその状態を拡張する。

## 実装順序と完了条件

| Slice | 対象と再利用する処理 | 完了条件 |
|---|---|---|
| 1 削除要求 | `forget.rs`, `raw.rs`, `backup.rs`, `worker.rs`, `main.rs`, `migrate.rs`, `transcript.rs`, `curate.rs`, `turns.rs`, `embed_phase.rs`, `claims.rs`。既存 SQLite transaction、raw.lock、import checkpoint。 | import origin のある record/device span（1 要求 500 件まで）の preview/start/status、raw.db の deny と本文なしの要求ログ 2 部、古い backup の restore・raw.db の巻き戻し・再 import による復活防止、派生書込の fence。物理 purge は未完了と表示する。native identity のない raw、uid/session/repo/time の start は後続まで拒否する。 |
| 2 uid の完全削除 | Slice 1 に `claims.rs`, `consumer/*`, `embed_phase.rs`, `search/b.rs`。既存 no-AI rebuild を内部で共有。 | claim/document uid の read backstop、raw op 本文・全 derivation・correction・digest・FTS・vector cache と backup の purge、失敗から再開。claim-only は raw 0 件。 |
| 3 raw scope の完全削除 | 同じ pipeline を session/repo/time/range に拡張。live/legacy provenance bridge、大きい selection のページ処理、`Span::minus`, recurate queue。 | 第一 slice の provenance/500件制限を解消する。重なる window summary と派生物も消える。実行中の curator/digest/embed の返答は commit しない。#167 の支援 evidence を持ち、失われた場合は proposed に戻る。#186 の preference はマスク済み raw から deterministic に再導出する。 |
| 4 可逆な管理 | `CorrectionOp`, `Pending`, 既存 exclusion op/clock、capture 共通経路。 | mute/unmute、capture exclusion、withdraw、訂正。#157 anchor 継承、#162 clock 後退、#160 restore 後の失敗報告を含む。 |
| 5 WebUI | `view.rs` の `save_gate`、`assets/viewer/`、同じ型付き backend。 | 日英で preview/確認/進捗/制限を表示。correction/mute/capture/withdraw/forget/global preference を操作でき、CLI 編集を要求しない。#94 の coverage matrix に実装証拠を残す。 |
| 6 安全性と測定 | doctor、provider scratch sweep、OS の secret file helper、合成 failure harness。 | 下記の削除 canary、M22、A104、security review。#281/#285 を保持して解決し、権限未対応を成功扱いにしない。 |

最初の writer fence は Slice 1 のファイル、この計画書、`tests/forget_cli.rs`。
公開境界は CLI と `forget` の型付き API。内部 table 名を正解とするテストではなく、
preview が変わる、復元後も get/import から戻らない、ログが失われても hook が記録を止めない等を確認する。

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
登録前の disk full と permission failure は fail closed。要求ログの欠損・破損・古さ・他 home の行は
記録を止めず、報告して飛ばす（D1）。read/derived commit と登録の race は barrier で再現する。
バックアップの圧縮前後と DB の物理 byte grep の両方を確認する。

通常の必須チェックは `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、
`cargo test`。privacy、削除、restore、viewer write は security review の対象。
CI/security gate は弱めない。slice ごとに PR を開き、repository の通常のレビュー経路を通す。

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

- [ ] Slice 1: 削除要求と raw record/span の安全な受理
- [ ] Slice 2: uid の物理 purge
- [ ] Slice 3: raw scope、rescan、retention
- [ ] Slice 4: 可逆な管理と訂正
- [ ] Slice 5: WebUI
- [ ] Slice 6: 安全性・測定・独立レビュー

未チェックの slice がある間、この段階は完了ではない。

### 第一 slice の実装・検証（2026-10-03、設計 v2）

設計 v1（journal）の実装を v2（D1）に置き換えた。`forget_jobs.step` は 1 のままで、物理 purge
完了を返す経路はない。

- 公開 CLI の試験 10 件（`tests/forget_cli.rs`）: import origin のない record の登録前の拒否、
  本文なしのログと届かない範囲の表示、失われた・古い・他 home の・壊れたログが hook を止めないこと、
  古い backup と capture rule の変更で v1 の event が戻らないこと、redaction rule が変わっても
  コピーした transcript が忘れた identity を保つこと、壊れた raw.db がログから忘却を回復すること、
  backup ディレクトリに届かなかった忘却を home のログだけで戻すこと、巻き戻った raw.db で新しい
  record が残り忘れた record は戻らないこと、raw.db と両方のログを失うと forget の表示どおり本文が
  戻ること、破れたログ行の後の忘却も restore で残ること。
- 型付き API と fence の試験: `forget.rs` の 5 件と、provider の呼出し中に登録された忘却が
  その答えを何も書かせないこと（`curate.rs`）。障害試験にオーナーのデータ・モデル・サービスは
  使っていない。
- `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`: 成功。

常駐 worker は、保持している raw.db と同じパスのファイルが別のものになったとき（古いコピーを手で
戻したときなど）、次の確認で stores を開き直す（`worker::serve`、テスト
`a_resident_worker_opens_raw_db_again_when_another_file_takes_its_place`）。置き換わったファイルは
別の端末の記録として読まれる（docs/cards.md S3）。

引き継ぎレビューで確認した 2 件の復活経路を修正した。
raw.db を別ファイルに置き換えても home の id を保持し、古いコピーへの再置換やコピー後の backup
からの復元で削除要求を適用する。忘れた v1 record の session hash と時刻から transcript の
取り込み境界も回復し、準備済みの batch は追記 transaction 内でもその境界を確認する。
伏せ字ルールを外した後も取り込み境界が一致し、raw への要求適用がロックで失敗した import は
続行しない。CLI の追加試験 5 件と、準備後の忘却が追記を止める型付き API の試験で、先に失敗することと
修正後の通過を確認した。削除対象より前の transcript と無関係な新規記録は残る。

残り: 独立レビュー。この結果は全 M5 の削除 canary 成功ではない。物理
purge、live/legacy identity、大量 selection、uid/session/repo/time、可逆な管理、WebUI、各 OS の
安全性と M22 は後続。
