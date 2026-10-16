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
  パス、本文の fingerprint は持たない。1 要求 500 record、1 行 256 KiB まで（第一 slice は
  500 件を超える選択を登録前に拒否し、利用者が範囲を分ける。自動分割は slice 3）。追記は
  排他ロック下で 1 回ずつ、末尾が改行でなければ先に改行を書き、
  `fsync` し、作ったときはディレクトリも同期する。各行に payload の checksum を付け、checksum
  や形が合わない行、他 home の行は、その行だけ飛ばして報告する。
  読込みも 1 行の上限で区切り、過大な行は固定サイズの buffer で末尾まで読み捨てる。
  壊れた行全体をメモリへ載せず、後続の有効な要求を読む。
  新規 home の id は最初の device id とし、この schema が raw を最初から作ったことの proof、
  device id、file identity と共に、最初の schema transaction で raw の metadata に保存する。ファイルのコピーで
  将来の記録用の device id が変わっても保持し、record の backup にも運ぶ。別の権威は作らない。
  旧 raw の device から付けた id は表示用の未検証値であり、新規 home の proof を持たない。
  record backup は id と proof を運び、restore は明示された一致する値だけを戻す。
  旧形式の backup と、restore のために作った staging DB は新規 home の proof にならない。
- **reconcile は双方向で、ログのコピーの障害では拒否しない。** worker の起動と stores を開き直すたび（`backup::check`
  の後、consumer と export の前）、`oboete restore`、forget command、import command の取り込み前に、
  raw にあってコピーにない要求をコピーへ書き、コピーにあって raw にない要求を raw へ適用する。
  読めない・書けないコピーは報告して飛ばす（command の出力、`forget --status`）。`raw::open` と
  hook は reconcile しない。読めた要求を raw.db に適用できなかった場合は、deny のないまま
  import・consumer・provider が進まないよう caller へ失敗を返す。
  `start` が raw に登録した後の失敗は、登録済みの Status とログの問題を返し、登録失敗と表示しない。
- **適用は seq ではなく identity で。** identity は import origin だけで、`(device, seq)` は出所の
  記録にとどめる。要求の適用は、保存された import origin が一致する record をすべて（再 import 後の
  重複も）tombstone で隠し、deny 行は origin で `INSERT OR IGNORE` する。Claim と Correction の op
  は anchor record の origin が deny されていれば拒否する。再利用された seq を古い要求で隠さない。
- **restore は live raw.db の要求も運ぶ。** 読める raw.db の上の restore は、その `forget_jobs` を
  identity で再構築後の raw に適用してから swap し、ログにも書く。job のない deny 行（segment から
  再構築したもの）はそのまま残す。backup の tombstone 行は deny 行を運ぶ。
  本文を持たない Removed 行も native hash を運び、restore と再 backup を繰り返しても deny との
  対応を失わない。rescan 等で既に隠れた対象にも、現端末がまだ持たない forget control を新しい seq
  で記録し、次の incremental backup に deny を運ぶ。deny を検証して復元した control はその印を
  保ち、同じ要求の retry で control を増やさない。Removed 行の本文と sample は復元しない。
  control 自身も deny の origin を保持し、対象 record の端末を含まない backup の restore と再 backup
  でも対応を失わない。要求の `(device, seq)` は出所の記録だけで、deny との対応付けには使わない。
- **派生書込の fence。** `curate::Reading` は window の入力を読む前に `denied_records` の件数を読み、
  Window と Turn の全 writer が raw の追記 transaction の中でそれと比べ、違えば何も記録せずに
  切り直す（`append_ops_fenced`）。knowledge.db が raw の tombstone に遅れている間（Anchors の
  checkpoint より後に tombstone がある間）、window・turn・embedding の各 phase は待つ。preview
  token も同じ件数を含む。
- **登録と送信を順序付ける。** provider は最後の raw の確認より前に home ごとの共有ロックを取り、
  forget は raw の書込 transaction より前に同じロックを排他で取る。`dispatch.lock` は順序付け用で、
  削除状態を持たない。ロック下で、保持している raw の接続が現在のファイルを指すことも確認する。
  同一性を確認できなければ送信・登録を拒否し、古い接続の deny 件数を権威にしない。
  hook の open・append はこのロックを取らない。HTTP は実際の TCP への
  本文送信が終わってから解放し、モデルの応答待ちは登録を止めない。TLS・CONNECT の handshake や
  body reader の読み終わりだけでは解放しない。CLI は stdin の引き渡し完了、または prompt file を
  渡したプロセスの起動後に解放する。送信失敗もロックを残さず、retry と fallback は改めて確認する。
  新しい shortlist query の embedding も knowledge.db の追随を待つ。送信済みの結果の回収と
  本文検索は続ける。
- **第一 slice で忘れられるのは import origin のある record だけ。** live と回復した record に
  native identity を与えるのは slice 3a（「Slice 3 の決定」の 3a: 保存せず device・seq・ts から計算する）。

障害の範囲（実装前に決めた打ち切り基準）: 第一 slice は F0（障害なし、通常の並行）、F1（raw.db の
喪失・破損、backup と片方のログは残る）、F2（raw.db だけが古いコピーに置き換わる）で正しい。F3
（ログと既定の backup を含む home 全体が古いコピーに置き換わる）と二重障害は、forget が表示し
spec 6.3 に書く限界とする。forget が表示する限界は次のとおり。

- raw.db が古いコピーに置き換わった後、worker がログを適用するまでの間、最初の hook が忘れた
  record を SessionStart に注入しうる。登録の時点で hook が作り終えた packet と送信済みの provider
  呼出しは取り消さない。
- raw の commit と最初のログ追記の間の crash の後、次の worker 起動の前に raw.db を失うと要求は
  失われる。backup の segment が要求を持つ前に（purge の rebuild か次の worker の終了が書く）
  raw.db と両方のログを失った場合も同じ。
- ログの行が 1 つも残らない segment からの restore は deny 行を再構築するが job 行は戻らない。
- ログと backup ディレクトリは origin と session の hash を持つ（本文はない）。v1 と transcript
  から取り込んだ record の origin は保存された payload の hash を含むので、ログを持つ人は保存された
  文面の推測を確かめられる。slice 3a の live record の origin は device・seq・ts だけから作り、本文を
  含まない。v1 と transcript の origin はそのまま（本文を含まない native id が保存済み provenance にない）。
- 第一 slice は物理 purge をしない。古い backup segment には本文が残り、replay のときに tombstone
  で隠れる。エージェント自身の transcript も本文を持ち、forget はそのパスを表示する。

### D2. 再 import の identity

imported document は既存の uid/source/source_id を使う。raw record には版付きの
source identity と fingerprint を別表で持たせる。v1 は source device と元 event id、
transcript は agent/session と event の fingerprint 内の出現順を使い、内容の fingerprint で
同じ id の差替えを見分ける。コピー元のファイルパスは identity にしない。
別の event の追加で変わる全体の行番号は identity にしない。同じ fingerprint が複数ある
event と、native session id のない event は検索用に取り込むが、第一 slice の forget では
登録前に拒否する。後の import が一致する重複を認識したら、既存の最初の出現も同じ transaction
で曖昧にする。既存の forget と重なる曖昧な batch の取り込みは拒否し、復活も推測による削除もしない。
先頭の namespace が native session id から作られていない場合は、後の行に id が現れても
未検証のままとする。既存の forget がある home では、未検証の raw 本文を取り込まない。
要求ログにはこれらの hash と record id だけを残す。検索文・本文・引用は保存しない。
native identity がある import はその identity を比較し、同じ文字列の別 event まで消さない。
importer は capture 前の agent/session から session hash を作り、origin と同じ transaction で
保存する。伏せ字後の label からは作らず、本文を持たない要求と record backup にも運ぶ。
旧 v1 の hook 時刻は transcript の同じ出来事より遅れうる。保存された provenance に共通の
event id がないため、第一 slice は `oboete-v1` の record を、Removed の元 metadata も含め
登録前に具体的な理由付きで拒否する。既に受理した v1 の要求と deny は回復時にも保持し、
同じ native session の切れ目を持つ deny に重なる transcript batch は、checkpoint を進めず
明示的に拒否する。準備済みの batch も追記 transaction 内で再検証する。既存の先行 raw は
消さず、時刻の前倒し・本文照合・session 全体の tombstone で対応を推測しない。
逆方向も、同じ native session に deny がある状態で未対応の v1 本文を取り込むことは拒否する。
exact origin が既に deny された record は従来どおり取り込まない。transcript の新規 forget も、
同じ native hash の v1 等、または native hash のない live/legacy record がある home では
対応を証明できないため登録前に拒否する。異なる native hash は別の namespace として残す。
live の表示用 session label を照合キーにしない。この暫定制限でも hook・search は継続する。
その hash がない旧 record の forget は、対応を推測せず登録前に拒否する。
whole-record tombstone で隠れた import も、残る native identity で選ぶ。隠れた record の本文
サンプルは返さず、元の source・kind・時刻だけを読み transcript の切れ目を判断する。
その metadata も失われた record は推測せず拒否する。既に origin が deny された record は再登録しない。
Removed の backup も元の source・kind・時刻だけを運び、restore と再 backup 後も登録できる。
旧形式等でこの metadata が完全に残らない場合は、本文なしのまま登録を拒否する。
home lineage にも同じ安全条件を適用する。旧 raw の現在の device だけでは、それ以前の
コピーとの家系を証明できないため、proof のない home は現行 importer の native record が
追加されても第一 slice の forget を登録前に拒否する。hook・import・search は継続する。
第一 slice の F2 保証は、この登録条件を満たす home が保存した id と proof を持つコピーを対象とする。
チェックは append の transaction 内。fingerprint だけでは parser/redaction の変更をまたぐ
再 import 防止を証明できないため、第一 slice は native identity のない legacy/hook raw の
登録を具体的な理由付きで拒否する。既存の忘却がある状態で provenance を渡さない raw import
も明示的に拒否する（本文を運ばない touch metadata を除く）。import command は取り込みの前に
要求ログを reconcile するが、読めないログで import を止めない（D1）。単なる文字列 sentinel は本番の
deny にしない。
最終 M5 には live hook の source identity と legacy の対応付けを含める。保存済み provenance
から確実に対応付けられないものを「再 import 安全」と呼ばず、無関係な session 全体の deny
で隠さない。この bridge の完成前はオーナー切替をしない。
（2026-10-11、「Slice 3 の決定」の 3a で改訂）bridge は session 単位で作る。live hook の identity は
device・seq・ts から計算し、v1 と transcript とは agent と native session id で対応付け、同じ session の
別の出所の record を写しとして選択に加える。record 単位の対応は保存済み provenance にないので作らない。
オーナー切替の条件はこの session 単位の bridge とする。

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

### D5. uid の forget（Slice 2）

spec 6.2 の claim-uid target。claim の uid（`claims::uid`、64 桁の hex）と imported document の
uid（`<source>:<key>`）だけを受ける。raw の record は 1 件も消さない（preview は「raw records: 0」と、
本文自体を消すときに forget すべき span を示す）。

- **target の見分け方。** 64 桁の hex は claim の uid。それ以外は knowledge.db の `imported` に
  あれば document の uid。`<device>:<seq>` の形の record id は `--record` を使うよう案内して拒否し、
  card（`<device>.<op seq>.<n>`）と summary（`S<device>.<op seq>`）の id も拒否する（それらは record
  から派生し、record を消す Slice 3 で K4・T7 のとおり隠れる）。knowledge.db にも raw の op にも
  見つからない uid は登録前に拒否する。
- **uid の op の探し方。** claim op の本文は uid を持たず、全 op（import が 17 万件を超えうる）を
  計算し直すと遅い。knowledge.db を索引にする: `derivations` と `claims` が uid ごとの
  `(op_device, op_seq)` を、`imported` が import op の op_seq を与える。見つけた op の本文から
  consumer と同じ導出（`claims::normalize` の後の `claims::uid`）で uid を計算し直し、一致するものだけを
  対象にする。その uid の Correction op は本文の `uid` で選ぶ（`Pending::read` と同じ）。
- **登録は raw の書込 transaction 1 つ（D1 と同じ）。** 本文を持たない `forget` op（`{uid, job}`）を
  op log に追記し、`forget_jobs` に要求を書く。要求は版 2（target `Uid`、records なし）で、要求ログ
  2 部にも D1 と同じ規則で書く。古い binary は版 2 の行を読まずに飛ばす。別の表は作らない: op が
  権威で、その uid への式索引（`ops_forget_uid`）が索引。ops segment で backup に運ばれ、restore した
  raw.db はそれだけで忘れている。restore と reconcile が要求を適用するときは、同じ uid の op が
  すでにあれば足さない。spec 5.8 の tombstone として、M6 はこの op を exclusion と同じ control op と
  して先に送る。op の種類は `OpKind` に加え、すべての op consumer が知っている種類にする（知らない
  種類で worker は止まる）。
- **隠す・送らない（backstop）。** 登録の commit の時点から、claim と imported document の本文が
  外へ出るすべての所で、その uid を返さず送らない: search の各 leg・get・cite・timeline・viewer・
  MCP・hook の注入と packet・`oboete claims`（読む側）、embedding phase・curator の prompt
  （shortlist、前の window から持ち込む決定と提案、recuration の `anchored_in`）（送る側）。どこも同じ
  索引（`Raw::forgotten`）を引く。claim の読み手の多くはすでに `claims::Pending`（worker がまだ
  適用していない owner の変更と tombstone）で隠すので、`Pending::read` が忘れた uid も読み、
  `touches` がそれに真を返す。embedding phase は忘れた claim を Pending の保留にせず `forgotten` の
  印を付けて飛ばす（保留のままだと、印のない文書が列の先頭に残り続ける）。knowledge.db に行が残って
  いる間（step 3 の前）もこれで隠れる。忘れた claim の訂正と mute は、claim が無いものとして断る。
- **再導出の拒否。** 同じ span を curate し直すと同じ uid が導かれる（uid は kind と最初の引用の文で
  決まる）。curation は忘れた uid の claim を append の前に落とし、window op の `dropped` に
  `its uid is forgotten` として載せる。import は忘れた uid の文書を飛ばす。`append_ops` は忘れた uid の
  Claim・Correction・Import op を含む batch を拒否する（最後の砦。ここで拒否されると window や
  500 件の import batch が止まるので、前段で落とすのが本来の経路）。D1 規則 12 の fence の件数に
  forget op も数える: curation の呼出し中の uid の forget は、どの window もいったん切り直させる
  （uid の forget は稀なので、広すぎる fence を受け入れる）。
- **raw の op 本文を消す（2b）。** 見つけた Claim op（同じ uid の再導出と recuration を全部）、その
  uid の Correction op、Import op の body を、本文を持たない `{"forgotten": "<job>"}` に書き換える。
  op_seq・type・batch は変えない（checkpoint は op_seq で数える）。すべての op consumer は一つの
  helper でこの body を何も導かない op として読み、エラーにしない。書換えの transaction は
  `secure_delete` を有効にし、commit 後に `wal_checkpoint(TRUNCATE)` する。
  消した claim が supersede していた claim は、何にも supersede されなくなり current に戻る。preview は
  その claim を名指しし、利用者は mute か forget を選べる（Claude; overrulable: 本文のない
  supersede を残して古い claim を隠し続けるより、何も導かない op の方が単純で、理由を示せない隠れ方を
  作らない）。消した claim を supersede していた claim は、行のない uid を指す supersede を持つだけで
  そのまま残る。
- **knowledge.db は AI を呼ばない rebuild（`worker::rebuild`）で作り直す（2b）。** 書き換えた op からは
  何も導かれないので、claim・derivation・correction・edge・FTS・packet・imported の行は新しい
  ファイルに入らない。rebuild の後に uid の derivation と imported の行が無いことを確かめ、残って
  いれば索引が op を取りこぼしたとして job を止め、done と報告しない。`carry` は、古いファイルの
  `vector_keys` で忘れた uid の鍵だけが指す `src_sha` の vector を持ち越さない（同じ本文を持つ別の
  文書が指す vector は、消す本文を新たに漏らさないので残す）。古い knowledge.db、
  `knowledge.db.rebuilding-*`、`*.quarantined-*` は rebuild の後に消す。
- **backup（2c）。** 書き換えた op を含む ops segment を書き直す（新しい segment と seal を横に書いて
  から入れ替え、中断しても restore がその segment の他の op を失わない。既存の seal を上書きしない）。
  record segment は触らない。（下の「2c の決定」で改める）
- **進み方と再開。** `forget_jobs.step` は 1 登録・2 op 本文・3 knowledge.db・4 backup・5 完了。
  各 step は冪等で、終わるごとに step を進める。worker の起動時と forget command が未完了の job を
  続ける。`local` は step 5 まで「hidden; physical purge pending」、5 で「done」。
- **preview。** uid の種類、一行の見出し（claim は本文の最初の行、document は title）、raw records 0、
  消える claim op・correction op・import op の数、vector の数、書き直す ops segment の数、current に
  戻る claim。

PR は 3 つに分ける。2a: target・登録・要求ログ・restore と reconcile・隠す・送らない・再導出の拒否
（step 1 から進まない）。2b: op 本文・rebuild と carry・ファイルの後始末・完了の確認と再開。
2c: backup の書直し・中断試験・byte grep の canary。

試験（2a の分）:

1. claim の uid と document の uid の preview は raw records 0 と各数を返す。知らない uid、
   record・card・summary の id は登録前に拒否し、record の形の id には `--record` を案内する。
2. 登録の commit の直後から search・get・cite・timeline・viewer・MCP・hook の注入がその uid を
   返さない（knowledge.db に行が残っている間も）。
3. 登録の後、embedding phase とその window の recuration を回しても、stub の provider はその本文の
   どの byte も受け取らない（claim と document の両方）。
4. 同じ window の recuration が同じ uid を導いても claim op は書かれず、`dropped` に
   `its uid is forgotten` と載る。同じ claude-mem DB の再 import はその文書を戻さない。忘れた uid の op を含む batch を
   `append_ops` は拒否する。curation の呼出し中に登録された uid の forget は、その答えを何も書かせない。
5. 要求ログ 2 部に版 2 の行が書かれ、版 1 の行と混ざったログを読める。raw.db を失った restore の後も
   忘れた uid が戻り、reconcile を何度回しても forget op は 1 つ。
6. forget op は ops segment に入り、restore した raw.db はその op だけで uid を忘れている（別の表は
   作らない）。記録の segment が壊れて切り詰められても forget op は付け直される。

#### 2b の決定（2026-10-11。D5 の 3 か所を改める）

上の D5 のうち、uid の op の探し方（knowledge.db を索引にしない）、carry の規則（どの鍵も指さない
vector も持ち越さない）、進み方と再開（step の数字ではなく状態で判定し、purge は `oboete forget`
の process だけが worker の lock の下で走らせる）を、次のとおり改める。残りは D5 のまま。

1. **op は op log を走査して探す。** knowledge.db は worker が読むまで遅れる（import の直後、worker の
   停止中、rebuild の途中）。2a の preview はすでに `Raw::uid_ops` で op log を数えている。purge も同じ
   走査で op を選ぶ: Claim op はすべての本文から consumer と同じ導出（`claims::op_uid`）で uid を
   計算し、Correction と Import は本文の `uid` で選ぶ。登録の後は `append_ops` がその uid の op を拒否
   するので、選ぶ op は登録の時点から増えない（restore で古い本文が戻ったときは 7 の確認でやり直す）。
   knowledge.db は 7 の完了の確認にだけ使う。走査の費用は Claim op の数に比例する（評価用の home の
   写しの 242 件で数 ms）。Import と Correction は SQL の一回の走査（raw.rs の `ops_windows` の注記の
   とおり、178,370 件の import op の走査で 136 ms）。
2. **本文の書換え。** 選んだ op の body を `{"forgotten":"<job>"}` に書き換え、type・op_seq・ts・batch
   は変えない。transaction は登録と同じ順で取る（`dispatch::exclusive` の後に raw の書込
   transaction）。raw.db の書込接続は開くときに `secure_delete` を有効にする: この UPDATE で空く領域
   だけでなく、restore の op の切り詰めなど以後の削除で空く領域も 0 で埋まるので、古い断片を消す
   ための VACUUM は要らない（この変更より前に書かれた store、つまり dogfood と評価の写しは対象外）。
   commit の後、transaction を持たない接続で `wal_checkpoint(TRUNCATE)` をし、結果の行の busy が 0 に
   なるまで 100 ms おきに 5 秒までやり直す。truncate できなければ step を進めない（WAL に書換え前の
   page が残るため。次の継続でやり直す）。`secure_delete` は raw.db の接続の pragma を置く一か所
   （`raw::open`）で設定し、`Restore::finish` を含むすべての書込に効かせる。
3. **読み手。** 本文が `{"forgotten": ...}` だけの op は何も導かない。判定は一つの helper
   （`raw::Op::forgotten`）。読み手は次のとおりで、2b の試験でそれぞれ一度は書き換えた op を通す:
   `op_rows_in`（すべての op consumer）、`ops_of`（`Pending::read` と claim の訂正）、
   `previous_window_ops`（前の window から持ち込む claim）、`window_of`、`source_ids` と
   `import_repos`（import の重複の判定。書き換えた Import op は `source_id` を失うので、同じ文書の
   再 import を止めるのは 2a の忘れた uid の判定だけになる）、`uid_ops` と `forgotten_op`（forget
   自身）、`Restore::finish`（op の切り詰め）。backup の ops segment は 2c。
4. **knowledge.db は 1 回の rebuild で作り直す。** step 2 を終えた job が何件あっても rebuild は 1 回に
   まとめる（forget を続けて登録しても rebuild は増えない）。既存の no-AI rebuild（set aside →
   consumers → 古いファイルの削除）を使う。作り直しの間に始まる session は記憶の一部しか、または何も
   受け取らない。評価用の home の写し（記録 71,063 件、op 15,736 件、埋め込みなし）で、release の
   build の rebuild は 83 秒、最大 RSS 90 MB だった（vector の carry はこの home に vector がない
   ので含まない）。preview はそのことを言い、`oboete forget --yes` はその間待つ。
5. **carry の規則（D5 の carry を改める）。** carry は、古いファイルの `vector_keys` で忘れていない鍵が
   指す `(embedder, src_sha)` の vector だけを持ち越す。どの鍵も指さない vector（変わる前の本文の
   vector）も持ち越さない: 忘れた claim の前の版の vector がその中にあっても見分けられないため（その
   分、rewind で古い本文が戻ると埋め込み直す）。`vector_keys` のない古いファイルからは何も持ち越さ
   ない。`src_sha` が空の行（埋め込みを飛ばした印）は何も指さない。worker の起動時に quarantined と
   rebuilding のファイルから持ち越す `carry_set_aside` も同じ規則で、raw の忘れた uid の集合を渡す。
   どちらの carry も、要求ログにだけある忘れた uid を足す（raw.db を古い写しに戻すと、ログを raw.db に
   戻すのは rebuild の後の pass。Codex on #444）。足すのはそのホームの要求だけで、backup の置き場を
   共有する別のホームの要求は数えない（Codex on #444）。ログの写しが読めない、または場所が分からないときは
   carry を止め、vector が読めない写しのときと同じく rebuild は何も変えずに止まる（CodeRabbit on
   #444）。
   rebuild の後に `knowledge.db.rebuilding-*` と `knowledge.db.quarantined-*` を消す。この規則は
   forget のない `oboete rebuild` と restore の carry も変える（持ち越す数が減る）。
6. **hook の状態。** hookstate の値で本文を持つのは `shown` だけ（見せた claim の `{uid: {fp, body}}`
   を 7 日。OpenCode の受領も同じ値）。ほかの値は本文を持たない: `compacted`・`injected`・
   `checkpoint-*`・`step-*` は空、`failed` は seq、`file-*` は card の id、`turn` は uid の並び。
   purge は各 session の `shown` からその uid の項目を `hookstate::update` で消す（ファイルは消さず、
   hook と同じ lock の約束で書く）。
7. **完了は状態で判定する（D5 の「進み方と再開」を改める）。** `forget_jobs.step` は進んだところの
   記録で、完了の証拠にしない。継続のたびに step 2 から確かめ直し、確認に通らない step をやり直す。
   step 2 の完了: その uid の op に本文が一つもなく、その後の checkpoint が truncate できた。step 3 の
   前の job があれば、継続のたびに rebuild する。knowledge.db にその uid の行が残っていなくても:
   rewind（古い写しに戻した raw.db）が行だけを消しても、その本文は vector の cache と全文索引の
   segment に残り、新しいファイルだけがそれを残さない（Greptile on #444）。rebuild が終わったことは
   どこにも記録されないので、rebuild の後、step 3 の前に止まった purge も、続けるときにもう一度
   rebuild する（止まったときだけの費用。Codex on #444）。aside のファイルは、その vector をまだ
   持ち越していなければ（`carried` の時刻がファイルより前）5 の規則で持ち越してから消す。
   `shown` に残れば 6 の掃除。rebuild の後にも knowledge.db に行が残るときは、op を取りこぼした
   として job を止め、done と報告しない（status と doctor が言う）。restore が古い本文を戻しても、
   次の継続で step 2 からやり直す。
   #439 との関係: 登録の前に manifest を読んだ hook が、掃除の後に `shown` を書くと本文が戻る。
   step 5 の完了（2c）でも `shown` の確認をもう一度通し、#439 の読取りの柵でこの窓を閉じる。
8. **purge を走らせるのは `oboete forget` の process だけ。** `oboete forget --yes` は登録の後、同じ
   process で purge を続ける。purge は `state/purge.lock` を待たずに取り（取れなければ「purge は
   実行中」と言って終わる）、step 2 の前に `oboete rebuild` と同じ形（`lock_asking`）で worker の
   lock を取る。worker は自分の lock の中で rebuild を呼べない（同じ process でも lock は二重に
   取れない）ので、自分では purge しない: pass の始めに未完了の uid の job があり purge.lock が空いて
   いれば、hook が worker を起動するのと同じ helper（`hook::spawn_detached`: 切り離し、stdin なし。
   worker は viewer と同じくその子を回収する）で `oboete forget --continue` を起動する。起動した
   時刻は `state/` のファイルに置き、10 分に 1 回まで。`--continue` は続ける job がなければ何も
   言わずに 0 で終わる。worker の lock は `lock_asking` の
   待ち時間までしか待たず、provider の呼出し中の worker が譲らなければ purge は失敗して、次の起動で
   やり直す。同じ job を 2 つの actor が同時に進めない。
9. **status。** `local` は step 3 の前は「hidden; physical purge pending」、step 3 を終えると
   「purged here; backups pending」（2c まで）、step 5 で「done」。
10. **2c の canary を先に決める。** byte grep で探す文字列は、claim op にだけある文字列にする: 試験の
    claim の draft の本文を record の文と違えて（record は "We deploy on Fridays."、draft は
    "Deploys happen on Friday, canary-7f3a."）、stub の curator は summary と card に canary を入れ
    ない。uid と record の本文は意図して残る（forget op は uid を持ち、claim の引用元の record は D5 で
    消さない）。2b の試験も raw.db・raw.db-wal・knowledge.db とその WAL・`state/` をこの文字列で
    探す（backup は 2c）。

2b は PR を 2 つに分ける。2b-1: 1・2・3（op 本文と読み手）、8 のうち purge の process と lock
（`--yes` の後の継続と `--continue`）、step 2 までの status。2b-2: 4〜7（rebuild・carry・hook の状態・
完了の判定）と 8 の worker からの起動。下の試験は 1・3・4・8 が 2b-1、2・5・6・7 が 2b-2。

2b-2 の実装で決めたこと（2026-10-11）:

- 止めた job（7）は列を足さず状態で判定する。rebuild の後は行が残っても step 3 を記録し、op に本文が
  なく knowledge.db がその uid を持つ job を status は「stopped: knowledge.db still holds it after its
  rebuild」と言い、doctor は不健全とする。worker も `--continue` も、step 3 で本文のない job は続けない
  ので、rebuild を繰り返さない。
- worker が未完了の job を見るのは 10 分に 1 回まで（8）。見た時刻を見る前に `state/forget-continue`
  に書き、起動しなかった回も書く。rebuild と restore の中の pass からは見ない。
- aside の写し（5）は、purge が step 3 に進む回に、5 の規則で持ち越してから sidecar ごと全部消す。
- `raw.db.quarantined-*` と `raw.db.restored.quarantined-*` も書き換える前の op の本文を持つ。backup の
  segment と同じく 2c で扱う。

試験（2b の分）:

1. claim の uid の forget の後に purge を回すと、その uid の Claim op（再導出と recuration を含む）と
   Correction op の本文は `{"forgotten": "<job>"}` になり、type・op_seq・batch は変わらない。document
   の uid では Import op。他の op は 1 byte も変わらない。
2. purge の後、raw.db・raw.db-wal・knowledge.db とその WAL・`state/` のどのファイルにも canary が
   ない。
3. 書き換えた op を含む op log から rebuild しても worker は止まらず、その uid の claim・derivation・
   correction・imported・vector_keys の行はできない。消した claim が supersede していた claim は
   current に戻る。3 の読み手を一つずつ、書き換えた op が通る。
4. 同じ claude-mem DB の再 import は書き換えた文書を戻さない。
5. forget を 2 件続けて登録しても rebuild は 1 回。
6. rebuild の途中で止めた purge を続けると、`knowledge.db.rebuilding-*` から忘れた uid の vector が
   戻らない。rebuild の後、aside を消す前に止めた purge は、続けるともう一度 rebuild してから aside を消す（決定 7: rebuild が終わったことはどこにも記録されない）。
   step 3 と記録された job でも、restore で本文が戻れば step 2 からやり直す。
7. purge が 2 つ同時に始まっても（`--yes` と、worker が起動した `--continue`）、進めるのは一つで、
   rebuild は 1 回。worker は未完了の job があると `--continue` を起動し、10 分のうちに二度は起動しない。
8. checkpoint が truncate できない（読み手が古い snapshot を持つ）とき、step は 2 のまま進まない。

#### #439 の決定（読取りの柵。2026-10-11）

2a は、登録の commit の後に始まる読取りを `claims::Pending` と `Raw::forgotten` で、commit の時に
進行中の読取りを出口ごとの確かめ直し（search・get・cite・timeline・viewer の claim・manifest・hook の
packet）で、忘れた uid を返さないようにした。#439 は、別の process で進行中の読取りの出口がさらに
四つ（`get_many`、`oboete claims`、uid の preview の sample、`trec_run`）あることを示した。出口を
一つずつ塞ぐ代わりに、一つの仕組みにする。

1. **柵は `dispatch.lock`。** 読み手は、読み始めてから答えを組み立て終えるまで `dispatch.lock` を
   共有で持つ（送り手と同じ `dispatch::Admission::shared`）。登録・op 本文の書換え・要求の適用・
   exclusion・restore の入替えは今までどおり排他で取るので、登録はどの読取りも送信も進行中でない
   ときにだけ commit する。送信と読取りは共有どうしで互いを待たない。読取りの途中の送信（search の
   query の埋め込み）は同じ lock をもう一度共有で取るが、共有どうしは衝突しない。別の lock file に
   しないのは、読取りの途中で送る読み手が二つの lock を登録と逆の順で取って行き詰まるため。
   **順序は raw.lock が先。** restore は raw.lock を排他で持ったまま入替えで `dispatch.lock` を排他で
   取り、登録は raw を開いた（raw.lock を共有で持つ）後に取る。読み手も raw を開いてから柵を取る:
   柵を先に取って raw を開くと、restore と互いを待って 10 秒の後に失敗する。
2. **柵を取るのは読取りの関数の入口、raw を開いた直後。** `search::b` の検索（`query` とその変種が通る `search`）・
   `get`・`get_many`・`cite`・`timeline`・`claim`（viewer の claim）・manifest を組み立てる関数
   （hook の SessionStart、`oboete inject`、viewer の Context が通る）・`oboete claims`・forget の
   preview（uid・record・span のどれも）・`trec_run`（`oboete claims` は main の一覧の関数で取る）。CLI・MCP・viewer の呼び手は変えない。関数が返すとき答えはでき上がって
   いて、その後の整形と書出し（MCP の応答、HTTP の応答、stdout）は読み直さない。書出しは柵の外に
   置く: 詰まった stdout が lock を持ち続けて登録を止めることはない。いくつもの読取りをまとめる
   関数（`get_many`、`trec_run`）は全体で一回取る。中の関数が取る分は共有どうしで重なり、外の柵を
   持つ間は排他が取られていないので待たない。
3. **hook。** 記録（raw への追記）の後、注入の読取り（manifest・プロンプトの claim）の前に柵を取り、
   見せたものの記録（`shown`、OpenCode の受領）を書き終えてから放す（出力を書く前）。これで、登録の
   前に読んだ hook が掃除の後に `shown` を書く窓（2b の 7 の注記）も閉じる。待つのは 1 秒まで。取れ
   なければその呼出しは注入せず、stderr にそう書く（記録は済んでいる。MUST-M16: hook は agent を止め
   ない）。2a の確かめ直しに戻して注入する道は取らない: 排他が 1 秒を超えるのは restore の入替え
   （knowledge.db も入れ替わる）ぐらいで、そのときの注入は元から空に近い。一回限りの注入点（Grok の
   最初の tool call と圧縮の後の最初、agy の PreInvocation、Cursor の SessionStart と圧縮の後の最初の
   prompt）で柵が取れなければ、その呼出しを注入点にした hook の状態（立てた `injected`、取った
   `compacted`）を戻し、次の呼出しが注入する（Codex on #446）。Claude Code の PreToolUse の file note も
   同じ: raw を開いた後に柵を取り、見たカードの記録（`file-*`）を書き終えるまで持つ。取れなければ
   note はなく、read は進む。
4. **出口の確かめ直しは外す。** 柵の下では何も変えないので、2a が出口に足した確かめ直し（`get` の
   文書と claim の一覧、`claim`、`cite`、`timeline`、search の埋め込みの後、hook の
   `forgotten_now`）は外す。読み始めの `Pending::read` と `forgotten_set` は残す（commit の後に
   始まる読取りのため）。
5. **同じ process の中の登録。** 柵を持ったまま同じ process で登録すると、自分の共有の記述子に阻まれて
   排他が取れず、10 秒待って失敗する。`oboete forget` は preview の柵を放してから登録する。
6. **待ち時間。** hook の外の読取りは `OPEN_WRITE_WAIT`（10 秒）まで待つ。排他を持つのは commit の
   短い間と restore の rename の間だけなので、ふつうは待たない。登録は、読取りが続けて重なると
   10 秒で「busy: try again」になる（今の送信と同じ）。

試験:

1. 各読取りの途中（試験用の seam）で別の thread が登録を始めると、登録はその読取りが答えを組み
   立て終えるまで commit しない（試験は登録がその柵で待ったのを見てから確かめる。負荷で登録が
   まだ柵に着いていないだけの thread を、待っていると取り違えないため。Greptile on #446）。その読取りは uid を返してよい（commit の前に読んだ）。commit の
   後の同じ読取りは uid を返さない。柵を外す mutation で落ちる（出口の確かめ直しはもう無い）。
   2a の試験で同じ thread の seam から登録していたもの（`get`・`cite`・`timeline` と hook の packet）は
   この形に書き直す: 同じ thread では自分の柵に阻まれて登録できない。
2. hook: 注入の読取りの後、`shown` を書く前に登録を始めても、hook が `shown` を書き終えるまで
   commit しない。file note も、`file-*` を書き終えるまで commit しない。柵が取れないとき（排他を
   持ったまま）、hook は記録して注入しない。一回限りの注入点なら次の呼出しが注入する。
3. `oboete forget <uid> --yes` は preview の後に登録できる（自分の柵で止まらない）。

#### 2c の決定（2026-10-11。D5 の backup と進み方を改める）

上の D5 の「backup（2c）」と 2b の決定 7〜9 の step 4・5 を、次のとおり決める。2c は #444（2b-2）と
#446（#439 の読取りの柵）の後に作る。

1. **step 4 は purge の中で、worker の lock の下で走る。** step 3 の後、同じ `oboete forget` の process が
   `worker::lock_asking` を取り直してから backup を書き直す。worker の export と restore（CLI の restore は
   `worker::restore` が、worker の自動 restore は `backup::open_raw` を呼ぶ worker が lock を持つ）はこの
   lock の下で走るので、書直しの途中の segment を読む者も、途中で新しい segment を書く者もいない（lock の
   外で `open_raw` を呼ぶのは測定用の `replay` の `worker::backlog` だけ）。
2. **書き直す行は op の中身で選ぶ。** op_seq では選ばない: raw の op_seq は、別の device の segment
   （home を写した後の古い device id の segment）と、raw の終わりより先に退けられた segment
   （MUST-M14）の op を指さない。backup の場所（`backup::dir`）の ops segment をすべて（device を問わず、
   生きている名前のものも、`*.ops.zst.quarantined-*` に退けられたものも）展開し、各行（`{"device",
   "op_seq","type","ts","body","batch"}`）の op がその uid のものなら（Claim は `claims::op_uid`、
   Correction と Import は本文の `uid`。`uid_op_keys` と同じ判定で、`raw::op_uid` の一か所にまとめる）、
   body を `{"forgotten":"<job>"}` にする。raw.db の purge が書くのと同じ byte なので、書き直した segment
   から restore した raw.db の op は purge の後の raw.db と同じになる。ほかの行は 1 byte も変えない。
   record segment は触らない（D5: uid の forget は record を消さない）。step 4 を待つ job が何件あっても、
   走査は 1 回で全部の uid を見る。ops segment は小さい（評価用の home の写しで op 15,736 件、展開して
   数 MB）。
3. **生きている segment の入れ替え（古い hash を新しい中身が置かれる前に seal から外さない）。** checksum が
   合う segment は次の順で入れ替える。各段の後で dir を sync する。
   (a) 新しい中身を `.<name>.purge` に書いて sync する（`segment_name` の文法に合わないので誰も読まない）。
   (b) seal を 2 行にする: 1 行目に今の hash、2 行目に新しい hash（一時名に書いて rename）。
   (c) `.<name>.purge` を `<name>` に rename する。
   (d) seal を新しい hash の 1 行にする。
   `checksum_matches`（`damage` と doctor の backup の確認が使う）は seal のどの行の hash にも合えば合格と
   する。どの段で止まっても、そこにある segment は seal のどれかの行に合う。合わなくなると `select` は
   その segment から後の ops segment を全部捨てる（`bad_ops` が空でなくなると後のものも入らない）ので、
   ここで一つでも合わない瞬間を作ると restore は op log の残りを失う。古い binary は 1 行目しか読まない
   ので、(c) と (d) の間で止まったときだけその segment を壊れたと見る。止まった後の継続は、そこにある
   segment の hash が seal のどの行に合うかで決める: 2 行の seal はまず今の中身の hash の 1 行にし、
   中身に対象の行が残っていれば (a) からやり直す。残った `.<name>.purge` は step 4 の始めに消す。
   中断した export が rename の前に残した `.<name>.tmp`（本文を持ちうる）も同じく消す: worker の lock の
   下では書きかけの export はない。
4. **信頼されない segment。** checksum が合わない segment、seal のない segment と退けられた segment は
   restore が読まない。そこに seal は作らない: 全体を展開できれば対象の行を書き換え（一時名に書いて
   rename。seal はそのままなので合わないまま）、展開できなければ消す（その uid の本文を持つかもしれず、
   oboete のどの読み手も使わない）。生きている名前のものは seal も一緒に消す。消した数は報告する。
5. **raw.db の写し（2c-2）。** `raw.db.quarantined-*`、`raw.db.restored.quarantined-*`、`raw.db.restored`
   （止まった restore が残した build。次の restore が退けるか、raw.db がなければ raw.db にする）を、purge が
   開いた raw.db とは別のファイルであるときだけ（`raw::path` は raw.db がないと `raw.db.restored` を指し、
   そのときそれが生きている store。step 4 は `raw::open` が通った後にだけ走る）、それぞれ `secure_delete`
   を有効にした SQLite の接続で開き、step 2 と同じ判定（`uid_op_keys`）と書換えをして、WAL を truncate
   する。開けない、書き換えられない、truncate できない写しは、-wal と -shm と一緒に消す。
   `raw.db.restoring`（途中の build。restore は毎回作り直す）は消す。knowledge.db の写しは 2b の決定 5 の
   とおり step 3 で消えている。
6. **完了は状態で判定する（2c-2）。** backup の場所のどの ops segment にも対象の行がなく、2 行の seal が
   なく、raw.db の写しのどれにも本文のある対象の op がないこと。継続のたびに確かめ直す。
7. **step 5（2c-2）。** 各 session の `shown` からその uid をもう一度消し（2b の決定 6 と同じ掃除。#439 の
   柵で登録の前に読んだ hook は登録の前に `shown` を書き終えているので、ふつうは何も見つからない）、
   step 5 を記録する。`local` は「done」。
8. **どの job を進めるか。** `due` は step 5 の前か（2c-1 では step 4 の前）、op が本文をまた持つ job。
   rebuild（step 3）に進むのは、今回 step 2 をやった job と step 2 で止まっている job だけ: step 3 を記録
   した job は rebuild に戻らない（knowledge.db がまだ uid を持つ止まった job で、10 分ごとに rebuild
   しないため）。止まった job も backup は書き直す（本文は消えるべき）が、`status` と doctor は止まった
   と言い続ける。
9. **preview と limits。** preview は書き直す ops segment の数（生きているものと退けられたもの）、消す
   segment の数、書き直す raw.db の写しの数を言う（2c-2。写しの数と一緒に足す）。`uid_limits` の
   「backup segments keep it until their purge is built」を、step 4 の説明と、backup の場所を雲のフォルダに
   置いたときの版の履歴（spec 6.3）に替える（2c-1）。

2c は PR を 2 つに分ける。2c-1: 1〜4 と 8（segment の書直しと進め方）、9 の limits。2c-2: 5〜7 と 9 の
preview。下の試験は 1（segment の分）・2・3・5・6 が 2c-1、1（raw.db の写しの分）・4・7 が 2c-2。

2c-1 の実装で決めたこと（2026-10-11）:

- step 4 は segment の書直しだけで記録する。2c-2 は `due` を step 5 の前に広げ、step 4 の job にも raw.db の
  写しの書換えと step 5 を通す。`local` は step 4 の後「purged here and in the backup segments; raw.db
  copies pending」。
- backup の場所が読めない（`[backup] dir` の設定の誤りなど）、segment が読めないときは step 4 を記録
  しない: job は step 3 のまま、`oboete forget` はその誤りで失敗し、worker は 10 分に 1 回 `--continue`
  を起動してやり直す（決定 8 の rebuild はしない）。backup に本文が残るのに step 4 と記録するより、
  やり直し続けるほうを取る。doctor の backup の行がすでにその設定の誤りを言う。
- restore した raw.db の job は step 1 から始まる（2b からの動き。`Rebuild::finish` が要求を step 1 で
  書く）。その後の purge は op に本文がなくても rebuild を 1 回する。restore の後の rebuild と重なる費用で、
  2c では変えない。
- seal が保証しない segment（決定 4）は、判定できない行を 1 行でも持てば消す: op の行でない、または
  uid の本文を持つ種類（claim・correction・import）の op で本文から uid が出ない行。その行が本文を持つか
  分からず、読む者もいない（Codex on #453）。seal が保証する segment のその行はそのまま残す: export が
  書いた行で（新しい版の種類など）、raw.db の purge も同じ判定でその op を残す。purge が書いた本文
  `{"forgotten":…}`、uid の本文を持たない種類の op、空の行は判定できる行に数える。
- 書く途中で止まった名前として消すのは、export と書直しの `.<segment>.tmp`（退けた segment の
  `.<segment>.quarantined-<ms>.tmp` も）、入れ替えの `.<segment>.purge` と、それを書く `write_synced` の
  `..<segment>.purge.tmp`（Greptile on #453）。seal の `.<segment>.sha256.tmp` は hash だけなので残す。
- 退けた segment として取るのは、export がつける名前（`<segment>.quarantined-<ms>`、`<segment>` は ops の
  segment の名前）だけ。ほかの名前のファイルは書き換えも削除もしない（Codex on #453）。
- 消したファイルが電源断で戻らないよう、歩き終えたら step 4 を記録する前に backup の場所の directory を
  sync する（unix。Greptile on #453）。
- backup の場所を 2 つのホームが共有するとき（設定で同じ場所を指したときだけ）、purge は他方のホームの
  segment も書き換え、書く途中の他方のファイルを消しうる。範囲を系統の device に絞る件は #455。
- 生きている segment も、export がつける名前（device は英数字だけ、op の範囲は 0 で埋めた 12 桁以上。
  `name` で作り直して同じになるもの。範囲は 1 から始まり、逆に進まない）だけを取る（Codex on #453）。
- 本文が戻った job（step 3 以上で op が本文を持つ。restore が戻しうる）は step 2 からやり直し、step を
  正確に 2 に戻す。rebuild が途中で止まっても、次の継続がその job を拾う（Codex on #453）。
- purge が書いた本文と数えるのは `{"forgotten":"<job>"}` だけの本文（`raw::forgotten_body`、
  `raw::Op::forgotten` と同じ判定）。`forgotten` の欄があるだけの本文は判定できない行（Codex on #453）。
- 同じ uid の job が 2 件あるとき（ログが別の job id の要求を戻したとき）、本文に書く job id は 1 件
  （先のもの）、step 4 はすべての job に記録する（CodeRabbit on #453）。
- 設定した backup の場所が無いとき（外付けの drive やクラウドのフォルダが mount されていない）、purge は
  空とみなして step 4 を記録し、export はその場所を作り直す。両方を 1 つの設計で直す件は #457。

2c-2 の実装で決めたこと（2026-10-11）:

- raw.db の写しは store としてではなく素の SQLite の接続で開く（identity・device・schema に触れない）。
  `secure_delete` を有効にし、1 つの transaction で書き換え、WAL を truncate する。その uid の op を持た
  ない写し（claim より前の写し）はそのまま。開けない・書き換えられない・truncate できない写しは -wal・
  -shm・-journal と一緒に消す。`raw.db.restoring` も同じく消す。segment と同じ worker の lock の下で走る。
- step 5 は step 4 の直後に同じ purge が行う（`shown` の掃除をもう一度）。`local` は step 5 で「done」。
  step 4 で止まったときだけ「purged here and in the backups; its last check pending」と言い、次の継続は
  step 4 の走査（もう何も書き換えない）と step 5 をする。2c-1 の step 4 の文言はこれに替わる。
- preview は segment を purge と同じ走査で、何も書かずに数える（生きているもの・退けられたもの・消すもの）。
  backup の場所が読めないときは数の代わりにそう言う。raw.db の写しは候補の数だけを言う（その uid を持てば
  書き換え、書き換えられなければ消す）: read-only の接続は -shm のない写しの WAL を戻せないので、preview は
  写しを開かない。
- preview は、seal が保証しない segment で判定できない行を持つものを「消すもの」に数える（purge と同じ判定）。
- 写しとして取るのは、隔離がつける名前（`raw.db.quarantined-<ms>`・`raw.db.restored.quarantined-<ms>`、
  `<ms>` は数字だけ）と `raw.db.restored` だけ。ほかの名前のファイルは書き換えも削除もしない（segment と
  同じ。Codex on #453）。写しを消したら、step 4 を記録する前にホームの directory を sync する（unix。
  Greptile on #453 の backup の場所と同じ）。
- 写しの op は backup の行と同じく 1 つずつ判定する: 種類の名前が分からない op、本文が parse できない
  claim・correction・import、uid を言わず purge の本文でもない claim・correction・import のどれかを持つ
  写しは、その uid の本文を持ちうるので消す（隔離される写しは壊れた store であることが多い。Greptile on
  #458）。
- 写しを消すときは -wal・-shm・-journal を先に、写し自体を最後に消す: 途中で止まっても写しが残り、
  次の purge がそれを見つけて続ける（Greptile と Codex on #458）。sidecar の名前は写しの path の byte に
  接尾辞を足して作る（UTF-8 でない path でも同じファイルを指す。Codex on #458）。
- 隔離の名前でも、link であるものと raw.db 自体の別名（同じ file identity。unix だけが読める）は写しでは
  ない: 開けば別の名前の本物の store で、-wal がそれのものではない（Codex on #458）。preview も数えない。
- sidecar だけが残った写しも消す: 隔離は -wal を写し自体より先に動かし、restore の掃除は `.restoring` を
  journal より先に消すので、その間で止まると -wal か journal だけが残り、本文を持ちうる（Codex on #458）。
  sidecar の名前から写しの名前を求め、写し自体がなくても同じ規則で取る。op log の表（`ops`）のない写し
  （op log より前の schema）はその uid の op を持たないのでそのまま（CodeRabbit on #458）。

試験（2c の分）:

1. purge の後、raw.db・raw.db-wal・knowledge.db とその WAL・`state/`・backup の場所のどのファイルにも、
   展開した segment にも、raw.db の写しにも canary がない（2b の決定 10 の canary）。他の op の行は 1 byte も
   変わらない。
2. (a)〜(d) の各段の後で止めた purge（`backup::fail_after` の段の名前を足す）について、その時点の segment
   から restore すると op の総数が止める前と同じで（その segment より後の segment の op も戻る）、継続すると
   入れ替えが終わる（seal は 1 行、本文なし）。
3. 古い device id の segment と、raw の終わりより先に退けられた segment の対象の行も書き換わる。展開でき
   ない退けられた segment は消える。
4. raw.db の写しは書き換わり、壊れた写しは消える。`raw.db.restoring` は消える。
5. 書き直した後の segment からの restore は、uid を忘れたまま、ほかの op をすべて戻す。その後の export は
   新しい op だけを書く。
6. step 3 を記録した止まった job は rebuild に戻らず（別の forget の rebuild と同じ purge でも）、backup は
   書き直され、status は止まったと言う。
7. status は step 4 の前は「purged here; backups pending」、step 5 の後は「done」。

### Slice 3 の決定（2026-10-11。D1・D2 の第一 slice の制限を改める）

Slice 3 は PR に分け、この順に作る（2026-10-12 に 3b・3c を 2 つずつ、3d と 3e を 3 つずつに分けた）。後の PR はこの節に「3x の決定」を足す。

| 分割 | 対象 | 完了条件 |
|---|---|---|
| 3a identity と出所の橋 | live record の identity、同じ session の別の出所の写し、v1 の forget、#454 | 下記 3a |
| 3b-1 session とページ | session の target、500 件を超える選択、要求の版 4、session tombstone | 下記 3b-1 |
| 3b-2 repo と時間範囲 | repo・時間範囲の target、遅れて届く record、3a の拒否の改訂 | 第一 slice の 500 件制限の解消、D3 の端末ごとの上限 |
| 3c-1 raw の行 | raw.db の record の行、knowledge.db の作り直し、transcript の切れ目 | 下記 3c-1 |
| 3c-2 backup | backup の record segment、raw.db のコピー | 管理対象の物理 byte に 0 hit |
| 3d-1 claim | 忘れた record を引用する claim の uid の tombstone と op の書換え | 下記 3d-1 |
| 3d-2 window と turn | window op と turn op の文、重なる window の recurate、#366 | 表の Slice 3 の完了条件の残り |
| 3d-3 status の支え | #167 | #167 の受入テスト |
| 3e-1 rescan の伏せ字 | rescan の範囲 tombstone を、保存された byte と、伏せる前の文から作った op に及ぼす | 下記 3e-1 |
| 3e-2 directive | #186（伏せた directive の claim を event から決定的に作り直す） | #186 の受入テスト |
| 3e-3 retention | 保存期間の設定と失効 | spec 6.7 の Retention、A31 |

#### 3a: live record の identity と出所の橋

- **live record（source `hook`・`replay`）の identity は保存せず計算する**:
  `forget::origin("live", "<device>:<seq>:<ts>")`。ts は capture の時刻（ミリ秒）。raw.db が古い
  コピーに戻って seq が再利用されても、新しい record は ts が違うので古い要求で隠れない（rule 5 の
  目的）。backup segment・raw.db のコピー・restore はこの 3 つを運ぶので、どこでも同じ identity に
  なる。本文の hash を含まないので、ログを持つ人が文面の推測を確かめることはできない（D1 の限界は
  v1 と transcript の origin にだけ残る）。capture 時のランダムな id は採らない: 3a より前の record・
  コピー・segment は id を持たず、開くたびに別の id が付くと、ログの要求が古いコピーの同じ record に
  当たらない。
- **登録した live record の identity は `import_origins` に置く。** 要求の適用（`apply_request`）は、
  要求の record の `(device, seq)` にある live record の ts から identity を計算し直し、要求の origin と
  一致したときだけその行を書く（seq は手がかり、一致を決めるのは identity）。以後の適用・backup の
  `import_identity`・deny の運搬・anchor の拒否（`forgotten_op`）は import の record と同じ経路を通る。
  表の意味は「record の identity」になる（列は増やさない）。
- **Removed 行は元の source・kind・ts を出所によらず運ぶ。** 現在は v1 と transcript だけ。whole-record
  tombstone の後に初めて backup された live record も、restore 後に identity を計算でき、ログの要求が
  当たる。元の metadata のない古い Removed 行（kind なし）は identity を持たず、本文もないので選択から
  外して数える。restore 後の Removed 行は label を持たないので、session を要する写しの判定にも使わない
  （本文がないので写しの置き場所でもない）。
- **live の session label を照合に使うのは、label が native id のままのときだけ**（D2「live の表示用
  session label を照合キーにしない」の具体化）: その record の ledger に `session` の行がなく、label が
  `unknown` で始まらない（session id のない event は `unknown-<device>`、`hook::own_session`）。agent は
  gate を通らない hook の引数なので、そのまま比べる。v1 の agent も同じ名前（`claude`・`codex`・
  `grok`）。session hash は出所によらず `forget::session(agent, native session)`。
- **出所の橋（D2 最終段落の改訂）。** 同じ出来事を 2 つの出所が持つことがある: transcript の切れ目の
  直前の event（hook の時刻は transcript の時刻より後）と、v1 の hook 時刻の遅れ。record 単位の共通 id
  は保存済み provenance にないので、本文照合や時刻のずらしで推測しない。共通なのは agent と native
  session id。そこで、選択が触れる各 session について、その session の**別の出所**の record（live・
  transcript・v1 のうち選んだ record と違う出所のもの。この store にあればどの device の record も:
  コピーした raw.db は後の record を別の device で記録する。本文のない行（touch、restore で本文を
  持たない Removed 行）は除き、rescan の tombstone で隠れただけの行は本文が残るので含む）を「写しを
  持ちうる record」として選択に加え、preview に別に数えて見せる。session 単位で漏れがなく、推測はしない。
  同じ出所の選ばなかった record は加えない（session 全体を消さない）。session を target にすれば全出所を
  一度に選ぶ（3b）。
  - **session の分からない live record**（上の条件を満たさない label）は、どの session の写しかを示せない。
    そのような record を選ぶ forget、および同じ agent のそのような record がある home での transcript・
    v1 の record の forget は、登録前に理由付きで拒否する（第一 slice の「unverified native copies」の
    拒否を、この場合だけに狭める）。
  - **v1 の record も forget できる**（第一 slice の拒否を外す）。写しは同じ session の transcript の
    record として加わる。
  - **再 import**: `check_cross_source_forget` を保つ。cut に数える deny（live・v1）がある session の
    transcript batch と、deny がある session の v1 batch は取り込まない。transcript の record の deny は
    origin で再 import を止める（従来どおり）。
  - 限界（spec 6.3 に足す）: 同じ出来事が別の session id で記録された場合（agent が resume で id を
    変えた等）は写しに加わらない。
- **#454: 書き戻しの重複をなくす。** `unwritten::write_back` は書き戻したファイルの内容の hash を、
  その record と同じ raw の transaction で `unwritten_written` に書き、次の書き戻しは記録済みの hash の
  ファイルを追記せずに消す（commit と削除の間で止まった call の重複がなくなる）。名前は使わない:
  `unwritten/` が空になると番号は 0 からやり直す（U1）ので、名前で比べると新しいファイルを捨てる。
  内容の hash だけでも足りない: 同じ payload の 2 つの call が同じミリ秒に同じ event を残しうる。そこで
  keep ごとにランダムな id をファイルに入れ、hash がその keep だけを指すようにする。id のない以前の
  ファイルは同じ内容の別の keep がありうるので hash を記録せず、従来どおり書く（残れば重複しうる。
  Greptile・Codex・CodeRabbit on #460）。forget に限らない。
  raw.db が古いコピーに戻った場合は記録も戻るので、そのファイルが残っていれば従来どおり重複しうる
  （U3 の限界のまま）。
- **live record を取り直す経路を閉じる。** live の forget を受け付けると、hook が同じ出来事をもう一度
  記録する経路が復活の道になる。#454 の書き戻し（上）と、Cursor の SessionEnd の取りこぼし回復
  （`hook::adapt` の cursor 分岐。transcript の turn を本文の出現回数で raw と比べ、足りない turn を
  記録する）。後者は忘れた prompt を「足りない」と数えて記録し直すので、deny のある session では回復
  しない（回復は transcript からの取込みであり、`check_cross_source_forget` と同じ扱い）。hook は raw.db
  の `denied_records` を読むだけで、読めなければ回復を飛ばして SessionEnd 自体は記録する（制御状態が
  記録を止めない、D1）。確かめと数えは reader の hold（`dispatch::reading`）の中で行い、forget はその
  前か後に commit する（登録が hold を持つ間は回復しない）。session は agent が送った id で名指す: 後から
  label を伏せる規則が足されても同じ hash になる（Greptile・Codex on #460）。
- **曖昧な transcript の identity**（同じ session に同じ fingerprint が複数ある、native session id が
  先頭にない）: 第一 slice は再 import で identity が変わりうるので拒否した。同じ要求が同じ session の
  live か v1 の record（cut に数える deny）を含むときは、その session の transcript batch がすべて
  拒否されるので、曖昧な record も写しとして・選択として加えてよい。そうでない session では従来どおり
  拒否する。
- **要求の形**: 写しを加えた要求は version 3 とし、record は target の範囲の外にもある（各 record を
  identity で拒否する。範囲は表示用）。古い binary はその行を未知の version として飛ばして報告する
  （rule 9）。preview は範囲の record と写しを分けて持ち、token は両方を含む。live の record を含む要求も
  写しがなくても version 3: 3a より前の binary は live の identity を計算できず、version 1 なら適用済みと
  して record を見せたままにする（Codex on #460）。import の record だけの要求は version 1 のまま。
- 500 件の上限は 3b まで第一 slice のまま。写しを加えた後の件数で数える。
- 受入テスト（3a の PR）:
  1. hook の record を forget でき、ログから restore・古い raw.db への置換・segment だけの restore の
     どれでも戻らない。古い raw.db で同じ seq を再利用した新しい record は隠れない。
  2. whole-record tombstone の後に初めて backup された live record も、segment からの restore 後に
     ログの要求で隠れる（Removed 行の metadata）。
  3. 同じ session の transcript の record がある live record の preview は、その transcript の record を
     写しとして加えて数える。逆向き（transcript の record の選択に同じ session の live record）と、v1 と
     transcript の組も同じ。同じ出所の選ばない record は加わらない。
  4. session label が伏せ字になった live record、`unknown-<device>` の record は選べず、同じ agent の
     そのような record がある home では transcript と v1 の record も選べない（理由付きの拒否）。
  5. live の deny がある session の transcript の再 import は拒否される（既存の経路）。
  6. #454 の受入テスト: 書き戻して commit した後に残ったファイルは、次の書き戻しで追記されずに消え、
     他のファイルの event は書かれる。その間に forget した record の文面は戻らない。
  7. Cursor の session の prompt を forget した後の SessionEnd は、その prompt を回復しない。deny のない
     session の回復は従来どおり。
  8. 同じ session の live の record を含む要求では、曖昧な transcript の record も加わる。live も v1 も
     含まない要求の曖昧な record は拒否される。

#### 3b-1: session の target、ページ、session tombstone

- **target は record の id で session を名指す。** `--session <device>:<seq>`: search・timeline・get が出す
  record の id の、その session（agent と native session id）を選ぶ。native id はどの出力にも出ないので
  受けない。import の record は importer が残した session hash で名指す。live の record は 3a の検証
  （label が native id のまま）を通るときだけ名指す: `unknown-<device>` や伏せ字の label の record は
  「session を名指さない」と拒否する。要求に残すのは agent と session hash（`Target::Session`）で、label も
  id の本文も残さない。
- **選ぶ record**: その session hash を持つ全出所（live・transcript・v1）、store にある全端末の、本文の
  ある record と v1 の touch（`session_repos` の行。ts は持たない、rule 10）。忘れた record は除く。写しは足さない（全出所を直接選ぶ）。曖昧な transcript の
  identity も選ぶ: 下の tombstone がその session の後の取込みを止めるので、再 import で identity が
  変わっても戻らない。
- **preview**（spec 6.2）: agent、最初の record の時刻（local）、出所ごとの件数、端末の数、最初の prompt
  の先頭（outbound の伏せ字）。同じ agent の session id のない live record の件数も示す: その中にこの
  session の写しがあっても見つけられず残る。3a はそのような record がある agent の forget を拒否したが、
  session の target は拒否しない（この session の record は全部選び、見つけられない写しだけが残る。件数で
  示す）。
- **ページ**: 選んだ record を (device, seq) 順に 500 件ずつの要求にし、要求ごとに job を作り、1 つの raw
  transaction で全部を登録する（全部か無し）。token は全体で 1 つ。status は同じ target と started の job を
  1 行にまとめてページ数を示す（要求に欄は足さない）。総数の上限は置かない: 評価用 home の写しで登録の
  所要時間を測る（受入テスト 5）。
- **要求の版 4**（target `Session`）。古い binary は未知の版として飛ばす（rule 9）。この PR からの binary は
  raw.db の `forget_jobs` の読めない要求（後の oboete の target や版）も飛ばし、reconcile が問題として言う
  （status・purge・worker は止まらない。Greptile on #465）。それより前の binary は公開していない。
- **session tombstone（D3）。** 登録の後に届く、自身の ts が登録時刻以前のその session の record は
  書かない。hook の記録は SQLite の書込 lock だけを取り、時刻を取ってから登録の transaction を待ちうるので、
  確かめは raw の append の transaction の中で行う（`unwritten/` の書き戻しも同じ）。transcript と v1 の
  取込みは record ごとに確かめる（登録後に始まった turn は記録する）。tombstone が覆う deny（その session の
  ページの deny と、登録より前の時刻の deny）は 3a の `check_cross_source_forget` の batch 拒否に数えない
  （CodeRabbit・Greptile on #465）。覆わない deny（登録より後の record の forget、時刻のない別の要求の deny）は
  従来どおり batch を拒否する。表は `denied_scopes(job, kind,
  key, before)`（kind は `session`、key は session hash。3b-2 が repo と時間範囲の列を足す）で、要求の適用
  （`apply_request`）が書くので、ログからの restore と古い raw.db への置換の後も戻る。登録より後の ts の
  event は記録する（将来の記録を止めるのは capture exclusion）。live の event は伏せ字の後の label で
  照らすので、伏せ字の規則が session label を隠した event は遅れて届くと記録される（session を名指せない
  record の一つで、preview が数える）。
- **索引**: 3a の `records_sessions` と `import_origins_session` は forget の preview が初めて作る検索用の
  索引で、spec 1.6 が禁じる構造上の鍵（必須の親行、索引の根、curation や checkpoint の区切り）ではない。
  raw.db を開いただけでは作らない（`raw_db_is_full_and_has_no_label_index`）。`denied_scopes` は索引を
  持たない（行は登録したページの数だけ）。
- 受入テスト:
  1. session の target は全出所・全端末のその session の record を選び、他の session の record を選ばない。
     曖昧な transcript の record も選ぶ。
  2. session を名指さない record の id は拒否する。
  3. 1,234 件は 3 つの要求になり、1 つの transaction で登録され、途中の失敗では何も登録されない。status は
     1 行。件数が変われば token は古い。
  4. 登録をまたいだ hook の記録（ts が登録以前）と書き戻しは書かれず、登録後の ts の event は書かれる。
     transcript の取込みは登録以前の turn を書かず、後の turn を書く。ログから適用し直しても同じ。
  5. 評価用 home の写しで、最大の session と 139,417 件（最大の repo 相当）の登録の所要時間を測る。

#### 3b-2: repo と時間範囲の target

- **target の形。**
  - `--repo <dir|key>`: 既存の directory なら capture と同じ key（`repo::key`）、そうでなければ文字列を key と
    して使う（search が示す key）。要求に残すのは key の hash と登録した端末（`Target::Repo`）で、key は
    残さない（session と同じ。推測した repo 名はログで確かめられる: spec 6.3）。preview は record が名乗る
    key、その record の cwd（5 つまで）、出所ごとの件数と端末の数、最初の prompt の先頭を示す（どれも
    outbound の伏せ字）。
  - `--since <time> --until <time>`: この端末の record の ts が `since` 以上 `until` 未満のもの。search の
    `--since`・`--until` と同じ読み方（`search::b::time`: ISO の日付か時刻、offset のない時刻は UTC、
    `--until` の日付はその日の終わりまで）。preview は local と UTC の両方で示す。要求には端末と 2 つの
    時刻（ms）を残す（`Target::Range`）。
- **解決。** repo: key の hash に一致する `repo` label を持つ、本文のある event。store の全端末の、preview の
  時点のもの（要求は record を 1 件ずつ名指すので、D3 の「確認した端末ごとの上限」は preview の集合
  そのもの）。label が伏せ字の record（ledger に `repo` の行）は一致を判定できないので選ばず、件数を示す。
  時間: この端末の、範囲内の本文のある event と、範囲内の Removed 行（restore で本文がない。identity が
  あれば名指す。なければ passed に数える）。読みは record ごとではなく 1 回の join（repo は最大で store の
  大半を占める）、repo の label は `records_repo`（除外の索引と同じ。preview が初めて作る）。どちらにも
  3a の写し（同じ session の別の出所。どの端末でも、範囲や repo の外でも）を足す。v1 の touch も選ぶ
  （ts は持たない、rule 10）。
- **3a の拒否を改める（broad target）。** session の分からない live record（`unknown-<device>`、伏せ字の
  session label）は、record と span の target では拒否のまま。repo と時間の target では選んで登録し、
  preview に「session を名乗らない record N 件: 別の出所の写しは見つけられず残る」と示す（spec 6.3）。
  そのような record がある agent の transcript・v1 の record も拒否しない。曖昧な transcript の identity も
  選ぶ（下の tombstone が同じ turn の再取込みを止める: 同じ repo label、範囲内の同じ ts）。
- **ページと版。** 選んだ record と写しを (device, seq) 順に 500 件ずつの要求にし、1 つの transaction で
  登録する（3b-1 と同じ）。版 5（版 4 は session のページのまま）。3b-1 までの binary は未知の target の行を
  "not a request line" として飛ばして報告する（未知の版と同じく reconcile のたびに problems に出る）。
  session を名乗らない record の要求の session は空。
- **tombstone（D3）。** `denied_scopes` に端末と範囲の列を足す（3b-1 の表は open が列を足す）。repo:
  kind `repo`、key は repo の hash、端末は登録した端末。時間: kind `range`、端末と 2 つの時刻。登録の後に
  届く、自身の ts が登録時刻以前で scope に当たる record は書かない: hook の記録・書き戻し・transcript と
  v1 の取込み（3b-1 と同じ経路）。repo と時間の scope は登録した端末の record にだけ当たる: 他の端末の
  record が後から（M6 の同期で）来ても、その端末の上限の後なので D3 どおり記録する（session だけが端末を
  またぐ）。live の event は伏せ字の後の label で照らすので、伏せ字の repo label の event は当たらない。
- 受入テスト:
  1. repo の target は全端末のその repo の record と写し（別の repo にあるものも）を選び、他の repo の
     record と伏せ字の repo label の record を選ばない（後者は件数を示す）。directory は capture と同じ key。
  2. 時間の target はこの端末の範囲内の record と、範囲外・他の端末の写しを選ぶ。`until` の時刻の record と
     他の端末の範囲内の record は選ばない。
  3. session を名乗らない live record を含む repo・時間の選択は登録され、件数が preview に出る。record と
     span の target は従来どおり拒否する。
  4. 登録をまたいだ hook の記録・書き戻し・取込みのうち、scope に当たり ts が登録以前のものは書かれず、
     他の repo・範囲外・登録後の ts のもの、他の端末の repo の record は書かれる。ログから適用し直しても同じ。
  5. 版 5 のページは端末の違う record と session のない record を持てる。版 4 と 3 の repo・時間の target は
     未知の版。

#### 3c-1: raw.db の行、knowledge.db、transcript の切れ目

3c は 2b・2c と同じく 2 つの PR に分けた（2026-10-12）。3c-1 は raw.db と knowledge.db、3c-2 は backup。

- **対象**: 忘れた record = `import_origins` の origin が `denied_records` にある (device, seq) の行
  （apply_request が tombstone を置く相手と同じ。3a で live の record も入る）。store の全端末。seq の再利用で
  identity が合わない行は別の record で、対象にしない。
- **step 2**: その行を Removed にする。type `removed`、body・original_bytes・label（agent、session、repo、
  branch、head、gitdir、cwd）を NULL、enc `plain`。device・seq・ts・source・kind は残す（live の identity、
  `original_metadata`、export の Removed 行）。その record の ledger 行も消す（field は body の鍵名の JSON
  pointer）。500 行ずつの transaction で、各 batch は dispatch の排他の下で identity を確かめ直す。全体は
  worker lock の下。raw.db は secure_delete（既存）、最後に raw.db-wal を truncate。冪等。
- **knowledge.db**: uid の job と同じ回の rebuild 1 回（2b の決定 4）。carry の規則に record を足す: 忘れた
  record の鍵（vector_keys の kind `r`、`<device>:<seq>`）が指す vector は持ち越さない（Fts consumer が
  tombstone を処理する前に rebuild が走ると、古いファイルにまだ鍵が残る）。ログにだけある要求の record も
  同じ。rebuild の後、行が本文を持たない job に step 3。
- **status（D4）**: record の job は step 3 で本文を持つ行がないとき「purged from raw and its index here;
  the claims, cards and summaries made from it, and the backups, pending」。rebuild は op を読み直すので、
  その record を言い換えた claim・card・summary は 3d まで残る。restore で本文が戻った job は step 2 から
  やり直す（2b の「状態で決める」）。job ごとの本文の有無は 1 回の集約で読む（job ごとの EXISTS は #467 と同じ形）。
  segment だけの restore（両ログを失った後）は deny を戻し job の行を戻さない（spec 6.3）: 本文を持つ忘れた
  record の deny に job の行がなければ、その job を step 0 として状態で purge する（step は記録しない）。restore は
  live の record と deny を identity で結び直す（3a の結びつけ。segment が forget の前のものなら行に無い）。
  Codex on #469。
- **切れ目（rule 10）**: `transcript_cut` は `recorded` と、その session hash の deny の ts の最小の小さい方を
  返す。session の live record が全部書き換わり `earliest_by_session` に行がないときは deny の最小だけ
  （無ければ切れ目が消え、登録前の turn が取り込まれる）。deny の ts は切れ目に数える record にだけある。
- **索引**: `denied_records_session`（session 列）を最初の deny と purge が作る（`check_cross_source_forget`
  と `transcript_cut` の読み。#467）。forget の preview の索引と同じく、raw.db を開いただけでは作らない。
  knowledge.db の `manifest_facts_record`（device, seq, repo, branch）: manifest consumer は tombstone ごとに
  その record の fact を引き直すが、効く索引が device までで端末の fact を全部走査していた（評価用 home の
  写しで tombstone 1 件 約 60 ms、139,417 件の forget の後の rebuild が 2 時間を超える見込み）。(device, seq)
  だけの索引は DELETE にしか使われず（読みは列を覆う既存の索引を選ぶ）、repo と branch まで覆うと 3 つの
  文とも seek になり、同じ rebuild は 318 秒。
- **CLI**: record の forget も登録の後この process で purge する（uid と同じ）。preview の limits は、行の
  書換えと rebuild、3d と 3c-2 まで残るものを言う。
- 受入テスト: plain の短い本文と page をはみ出す長い本文の canary を forget して purge すると、raw.db・
  knowledge.db と WAL に 0 hit。行は device・seq・ts・source・kind を残し、label・本文・ledger がない。もう
  一度の purge は何もしない。要求を適用し直しても tombstone は増えない。restore で本文が戻ると purge し直す。
  live の record を全部忘れた session の切れ目は deny の最小に残る。zstd の本文は圧縮した byte も残らない。
  carry は忘れた record の鍵の vector を持ち越さない。

#### 3c-2: backup の record segment と raw.db のコピー

- **対象**: 忘れた record の identity の集合 = raw.db の `denied_records` の origin 全部（step 4 の時点で要求は
  raw.db に適用済み。3c-1 の purge と同じく、どの job の record かは問わない）。行の identity は、live の
  record（source が hook・replay）なら行の device・seq・ts から `forget::live_origin`（seq の再利用で ts が
  違う行は別の record で、対象にしない）、取り込みなら `import_identity` の origin。identity の無い取り込み
  （v1 の touch など）の行はそのまま残す: 取り込みの identity は記録する transaction でだけ付き後から付かない
  ので（export の `import_identity` と `import_origins` は同じ #389 から）、どの forget もその record を
  名指せない（scope の forget も passed として飛ばす）。
- **segment**: backup 先の record segment（export の名前の `.seg.zst` と、set aside の `.quarantined-<ms>`。
  どの端末のものも）の event 行で identity が集合にあるものを、export が今その行に書く Removed 行にする:
  device・seq・type `removed`・source・kind・ts・home_id・home_id_proven・import_identity（あれば）。
  body・label・original_bytes・ledger は捨てる。tombstone 行と Removed 行はそのまま。JSON でない行、type が
  event・removed・tombstone のどれでもない行、source の無い event 行、device・seq・ts の読めない live の
  event 行は判定できない行（2c-1 の `Line::Unknown`）。手順は
  2c-1 と同じ: seal のある segment は 4 段の置換、seal のない segment と set aside は書き直し、読めないか
  判定できない行のある seal のない segment は削除、書き込みの途中で残った名前（`leftover`、record の segment の
  名前も）は削除。ops と record の walk は 1 つの関数で、行の判定だけが違う。
- **raw.db のコピー**（2c-2 の `copies_set_aside`）: 3c-1 の `purge_records` と同じ書換え（type `removed`、
  本文・label を NULL、ledger 行を削除）を、identity が集合にある行に。取り込みの行の identity はコピーの
  `import_origins`（行の無いもの、表の無い版はそのまま残す）。書き換えられないコピーは 2c-2 と同じく削除。
- **step 4**: uid の job と同じ回に、step 3 を終え step 4 の前にいる record の job があれば segment とコピーを
  書き換え、その job に step 4。行を書き換える前に raw.db の meta に印（`record_backups_owed`）を置き、
  segment とコピーを書き換え終えたら消す: job の行のない deny（segment だけの restore の後。step を持たない）も、
  purge が途中で止まった後の続きで書き換え、worker の `forget --continue` もこの印を見る（Codex on #469・#470）。record の job の step 5 は 3d が決める（3d が派生の uid の job を登録する
  段）。3c-2 では record の job は step 4 で止まり、`due` は本文を持つ行があるか step 4 の前の job。
- **status（D4）**: step 4 の record の job は「purged here and in the backups; the claims, cards and
  summaries made from it pending」。3d まで done にしない。
- **限界の文**: preview の「older backup segments and copies of raw.db set aside still hold its text」を
  「backup 先の segment と set aside の raw.db のコピーからも消す」に。F3（home 全体が古いコピーに戻る）で
  本文が戻るのは、その古いコピーの backup 先（purge の前のもの）から。spec 6.3 の「record backup segments
  ... may contain text」も同じく。uid の preview と違い、segment の件数は preview で数えない（全 segment の
  展開は大きな store で数分かかる）。
- 受入テスト: live の record と transcript の record の canary を export し、segment の 1 つを set aside、raw.db の
  コピーを 1 つ作り、両方を forget して purge すると、backup 先（展開して）とコピーのどれにも 0 hit。seal は
  合い（`backup::verify`）、restore は忘れた record を Removed 行で戻し他の record の本文を戻す。判定できない
  行のある seal のない segment は削除、seal のある segment は残る。identity の無い取り込みの行は segment でも
  コピーでも byte 単位でそのままで、それだけの segment もコピーも消えない。置換の停止点で止めても続きで終わる。
  status は step 4 で上の文。F3 の CLI テストは forget の前の backup 先を戻して本文が戻ることを確かめる。

#### 3d-1: 証拠を持つ claim

3d は 3 つの PR に分けた（2026-10-12）。3d-1 は claim、3d-2 は window と turn の op の文と recuration、
3d-3 は #167。knowledge.db の側はすでに済んでいる: Anchors は tombstone や Removed の行で読めなくなった引用の
derivation を落とし、その window を recurate の列に入れ、読み手は `claims::Pending` で消化前の tombstone の
claim を隠す。本文が残るのは raw.db の op、ops segment、raw.db のコピー。

- **対象**: 忘れた record（denial の origin が `import_origins` で指す (device, seq)。行が Removed でも）を一つでも
  引用する claim op（部分的な anchor も信用しない、spec 6.2 step 3）。その uid の全部の op（その record を
  引用しない derivation と Correction op も、2b と同じ）。
- **uid の tombstone**: spec の「tombstoned by uid」のとおり、その uid に forget op（`{uid, job}`、job は
  その record の job。最初に見つけた忘れた引用の denial の job）を足す。別の job の行は作らない: record の
  job が運び、forget op が派生した uid の一覧の権威（D5 の「op が権威」）。読み手・送り手・再導出の拒否・
  `append_ops` の拒否は D5 の仕組みのまま。残りの record から recurate しても同じ uid（最初の引用が別の
  record にある claim）は作れない（spec どおり）。その uid を tombstone する forget op が uid の job のもの
  （または deny が restore で戻らなかった record の job のもの）しかないときも足す: op を書き換えた後、step 4
  と後の purge がその uid を見つける道は record の job の forget op だけ（segment だけからの restore は job の
  行を戻さない、Codex on #471）。
- **見つけ方**: preview も purge も raw.db の状態から探す（knowledge.db は Anchors がもう行を落としている
  か、consumer がまだ読んでいないかもしれない）: claim op を 1 回だけ読み（本文を持つものだけ）、引用が忘れた record の鍵にあるもの、または
  record の job の forget op がその uid を tombstone しているものを、uid ごとに集める。Correction op は
  `json_extract(body, '$.uid')`。評価用 home の写しで claim op は 5,314 件（Python で 0.03 秒）、uid ごとの
  `uid_has_ops`（毎回 claim op を全部読む）は使わない。
- **本文を持つ job（`records_due`）**: 行が本文を持つか、その job の派生した uid の op が本文を持つ（上の 1 回の
  走査）。古い ops segment からの restore で claim op の本文が戻った job は step 2 からやり直す（2b の「状態で
  決める」）。
- **step**: step 2 で派生した claim の op を書き換え forget op を足してから（dispatch の下で batch ごと）行を
  書き換える（下の「登録から purge までの間」）、step 3 の rebuild はそのまま、step 4 は record segment とコピーに加え ops segment を派生した uid で書き換える
  （2c の `purge_ops`、uid から record の job へ）、step 5 は派生した uid を各 session の shown から除く
  （2c の決定 7）。step 5 を record の job も持つ。派生した uid は rebuild の直後にも shown から除く（step 4
  が backup を読めずに終わらなくても残さない、CodeRabbit on #471）。raw.db の印（`record_backups_owed`）は
  step 5 の掃除まで残す（job の行のない deny は step を持たない、Codex on #471）。
- **status（D4）**: step 3「purged from raw and its index here, with the claims made from it; the cards and
  summaries made from it, and the backups, pending」、step 4 以降「purged here and in the backups, with the
  claims made from it; the cards and summaries made from it pending」。done は 3d-2 で（card と summary の文が
  まだ残る）。
- **preview**: 消える claim の数（選んだ record を引用する uid の数）。purge と同じく raw.db の op log から
  数える（knowledge.db は consumer がまだ読んでいない claim を持たず、ないこともある、CodeRabbit on #471）。limits の文: 引用する claim は uid ごと忘れ、recuration でも同じ claim は作られない。card と
  session summary は後の版まで残る。
- **登録から purge までの間**: 有効な derivation が忘れた record を引用する claim は、読み手では Pending で、
  window・turn・embedding の phase では lag の gate（knowledge.db が tombstone に追いつくまで送らない）で
  隠れ、Anchors がその derivation を落とす（D1 規則 12 の fence が登録の deny で走っている curation の答えを
  切る。step 2 の forget op でもう一度）。同じ uid の別の derivation だけが引用していた claim（有効な
  derivation は別の record を引用する）は Anchors が残すので、forget op が書かれるまで見えうる。そのため
  step 2 は行（大きな forget では数十秒）より先に claim を書き換える。CLI の forget は登録の直後に purge を
  始めるので、隙間はふつう数秒で、別の purge が走っていたときや purge が止まったときは続きが走るまで。
- 受入テスト: 忘れた record を引用する claim（引用 1 つ、別の record も引用する claim、その uid の別の
  derivation と Correction op）が step 2 で本文を失い、forget op が record の job で足され、raw.db・knowledge.db・
  ops segment・コピーに claim の本文と引用の byte が残らない。引用しない claim は残る。同じ window を
  recurate して同じ uid を導いても op は書かれない。ops segment からの restore で claim op の本文が戻ると
  purge し直す。登録の後 purge の前に作った curation の request は claim の本文も引用も持たない。status と
  preview の数。

#### 3d-2: window と turn の op

- **忘れた record の時刻**: 各忘れた record の forget の tombstone の ts（登録の時刻。tombstone は消えず、
  segment からの restore でも戻る）。下の「見せた」はこれと K4 の `removed` で決める。
- **対象の window op**: curated の window op（`refused`・`recurate` のものも）で `forgotten` をまだ持たない
  もののうち、curator に忘れた record か忘れた claim を見せたもの: (a) 自分の device の span（from_seq〜
  to_seq、端で切った record も）か `goals` に忘れた record があり、op の `removed` がそれを丸ごと（offset
  なし）載せておらず、op が tombstone の 1 分後より前に書かれた（K4 の定義: `removed` に載る record は
  window を切った時に消えていたので見せていない。`removed` は 500 件を超えると何も載せないので、その時は
  時刻で決める。登録より後に切った window は fence と `removed` で見せていないと分かる。1 分の余裕は登録の
  request の時刻と transaction の間に書かれた op のため）。これがないと、purge の後に `oboete recurate` が
  同じ span を curate した op がまた当たり、書き換え・再投入・再送を繰り返す。(b) `candidates` か
  `carried` に 3d-1 で tombstone した uid（record の job の forget op の uid）がある（#366 の 1、
  docs/cards.md K4 の「Not covered」）。uid の forget（2b）の claim は元の span が残るので (b) に入れない。
- **`carried`**: curation が prompt に持ち込んだ claim（決定済みと open item、`Request::carried_uids`）は
  今は op に残らない。新しい window op は `candidates` と同じく uid だけの `carried` を持つ。この版より前の
  op は持ち込んだ claim が分からない（限界として書く）。op の上限では gates の一覧と card を先に削り、
  candidates と carried は最後に削る（forget がそれで window を見つける。どちらも上限があるので、card を
  全部削っても上限を超える op だけが失う、CodeRabbit on #472）。
- **window op の書換え**: 構造だけを残す: `outcome`・`reason`、`from_seq`・`from_offset`・`to_seq`・
  `to_offset`・`covers`、`elided`・`shortened`・`excluded`（record の seq）、`goals`・`removed`、
  `refused`・`recurate`。summary・observations・cards_dropped・dropped・lowered・cut・candidates・
  candidates_cut・carried・provider は捨て、`forgotten: <job>`（最初に当たった忘れた record の denial の
  job、または uid の forget op の job）を足す。cards consumer は summary も observations も持たない op から
  card を作らない（今のまま）。
- **対象の turn op**: `forgotten` をまだ持たない turn op（skip も、ラベルを持つので）のうち、(a) T7 の集合
  （自分の device の from〜through、`read` の span、`goals`）に忘れた record があり、window と同じく op の
  `removed` が丸ごと載せておらず tombstone の 1 分後より前に書かれたもの（from〜through は、T7 の reader と
  同じく session を問わない: ラベルを失った Removed の行は session が分からない）。(b) `read` の span が
  `forgotten` を持つ window op の span と同じで、その span の次の `forgotten` を持たない window op より前に
  書かれたもの（読んだ card が忘れた claim を言い直しうる。その後の turn は recuration の新しい card を
  読んでいる）。状態で決めるので、後の purge でも同じ op が当たる。
- **turn op の書換え**: `fitted` が上限を超えた op にするのと同じ「fields のない skip」（`fields` 空、
  `skipped: true`）にし、ラベルを消し（agent と session を空、repo を null。3c の Removed 行がラベルを
  持たないのと同じ）、`forgotten: <job>` を足す。from・through・ts・read・goals・removed・excluded は残す:
  phase は through で「問い済み」を数えるので、summary は作り直さない。読み手は skip を出さない（今のまま）。
- **見つけ方**: 忘れた record の鍵（`denied_records` ⋈ `import_origins`、device ごとに seq で並べ、job と
  tombstone の ts を付ける）と、record の job の forget op の uid を 1 回だけ読み、claim・window・turn の
  判定で共有する（`record_jobs_with_text` は purge ごとに 5 回と、worker で 10 分ごとに走る）。window と turn
  の op は本文を 1 回 parse して（9 欄の `json_extract` より安い）`Judge` の規則にかけ、範囲の判定は Rust で
  二分探索。deny がない store は読まない（3d-1 と同じ）。
- **step**: step 2 は claim（(b) が読む forget op を書く）→ window と turn → 行の順（dispatch の下で batch
  ごと）。step 3 の rebuild はそのまま。step 4 は ops segment とコピーの window・turn op を、(device, op_seq)
  で raw.db の書き換えた本文にする（2c の uid の判定に、鍵から本文への表を足す）。raw.db が持たない op
  （古い写しに戻した raw.db の先の、set aside された segment やコピーにだけある op）は、その op 自身の本文で
  同じ規則（`Judge`）にかける: window・turn op と、忘れた record を引用する claim op（Codex と CodeRabbit on
  #472）。そのため record の job が step 4 を負う間は、uid の job がなくても ops segment を全部読む。判定の
  前に segment とコピーを 1 度読み、raw.db が持たない claim op のうち忘れた record を引用するものの uid
  （window の candidates・carried が名指す。その uid の op はすべて書き換える: Correction も）と、raw.db が
  持たない window op の span（turn の (b) が読む）を `Judge` に足す（Codex on #472 の 2 回目）。コピーの
  window・turn op の本文が parse できないときは、claim と同じくコピーを消す。step 5 はそのまま（shown は
  claim の uid だけを持つ）。
- **recurate**: claims consumer が `forgotten` を持つ curated の window op を読んだとき、その span 全体を
  recurate の列に入れる（op は自分。Removed の行は次の window の文に入らず、その op の `removed` に載るので
  `Span::minus` で切らない）。今は Anchors が、消えた引用を持つ claim op の window を入れるが、3d-1 の書換えの
  後の rebuild では claim op がもう派生しないので入らない。op から入れるので rebuild でも入り、後の
  recuration が覆うと列から出る（今のまま）。列は `oboete recurate`（見積もりを見せてから送る）でだけ送る。
- **状態で決める**: 対象の op がまだ本文を持つ job（restore で戻ったものも）は step 2 から（3d-1 と同じ）。
  #366 の 2（restore が tombstone を失う）は deny から状態で見つけ直すので覆う。
- **status（D4）**: step 3 は「purged from raw and its index here, with what was made from it; the backups
  pending」、step 4 は uid の job と同じ「purged here and in the backups; its last check pending」、step 5 で
  `done`。3d-3（#167）は claim の status の意味の話で、本文は残さないので done を待たせない。
- **preview**: 空にする card（window op）の数、session summary（turn op）の数（spec 6.2 の preview の種類。
  recurate に入る window の数は card の数と同じ）。登録の前なので deny ではなく preview の record の鍵（時刻
  なし）と、preview が数えた claim の uid で、同じ判定で数える。
- **limits と help**: card と session summary も空になり、その window は `oboete recurate` の一覧に入る。
  この版より前の window が持ち込んだ claim は分からないので、そこから作った card は残る。`forget --help`
  から「後の版」を消す。
- 受入テスト: 忘れた record を span に持つ window、goals に持つ window、忘れた claim を candidates に持つ
  window と carried に持つ window（record は残る）、どれにも当たらない window、忘れた record を `removed` に
  丸ごと載せる window、登録の 1 分後より後の window。その turn op（from〜through に持つ、read に持つ、
  candidates の window を読んだ、その span の recuration の後に読んだ、当たらない）。purge の後、当たる op は
  構造だけで本文の byte が raw.db・knowledge.db・ops segment・コピーに残らず、当たらない op はそのまま。
  recurate の列に当たった window の span が入り、rebuild の後も入っている。その span を recuration した op は
  当たらない。restore で window op の本文が戻ると purge し直す。status は done、preview の数。

#### 3d-3: status を支える record（#167）

gates は claim の status を、引用の行のほかの record で上げることがある: assistant の提案を次の user の
turn が受け入れたとき（`accepted`）、user の素の「はい」や選択肢の番号が window の中の reply に答える
とき（`answers_a_reply`、`picks_an_option`）、window の中の通ったテストの実行で done を残すとき
（`passing_run`）。claim op はその record を持たないので、その record が消えても（rescan の tombstone、
forget、後の withdraw）claim は decided や done のまま残り、rebuild でも残る。

- **op の形**: claim op に `support`（その status が立つ record の `{device, seq}` の一覧、どれか 1 つが
  残っていれば立つ）を足す。空のときは書かない（今までの op と同じ形、uid は変わらない）。本文も引用も
  持たない: その record を忘れても claim op はその record の byte を持たないので 3d-1 の対象（引用する
  claim）にならず、claim は残って status だけが下がる（#167 の受入: 「the claim itself stays」）。
- **gates**: status を上げた条件が引用の行でないときだけ、その record を `Draft` に載せる（`tainted` と同じく
  gates だけが書く）。受け入れ: 受け入れた user の turn。答え: 答えた reply（前の window の reply に答える
  とき、つまり `ended` や `offered` で決まったときは window の中にないので持たない。制限）。通った実行:
  window の中の同じ session と repository の通った実行すべて（どれか 1 つで gate を通る）。user 自身の
  言葉で立つ status は持たない。
- **consumer**: claims consumer は derivation を作るとき support の record がどれも残っていなければ（行が
  消えた、または丸ごとの tombstone。一部の mask は残っているとみなす: 通ったテストの出力の token が
  伏せられても実行は通っている）status を `proposed` にし（gates が下げるのと同じ）、support を
  knowledge.db の `support(op_device, op_seq, device, seq)` に入れる。Anchors は tombstone の対象の record を
  support に持つ derivation を探し、どれも残っていなければ `proposed` にする（claim は消さない、recurate
  の列にも入れない: recuration の gates も同じく下げる）。読み手は derivation の status を読むので
  （owner の Correction の status が先）、hook の配達も検索もそれに従う。rebuild は consumer がそのときの raw で
  決めるので同じになる。Anchors がまだ読んでいない tombstone が support の record に当たる claim は、引用に
  当たるときと同じく読みから外す（`claims::Pending`。下がった後に出る。Codex on #473）。
- **owner の Correction**（Codex on #473）: owner が status を決めた claim は、その Correction が support の
  record が消える（最後の 1 つの最初の丸ごとの tombstone）より前に書かれていれば下げない: `active` は
  Correction の status を見せ続けるので、derivation も立ったまま supersedes を保つ（下げると、owner が
  決めたままの claim の陰にあった古い claim が current に戻る）。消えた後に書かれた Correction は status
  だけを決め、下がった derivation を上げ直さない。どちらも時刻で決まるので、live の consumer と rebuild が
  同じになる。Pending も同じ規則で決める: 消える前に決めた claim は読みから外さず（見える status も edge も
  変わらない）、消えた後に決めた claim は Anchors が edge を消すまで外す。segment からの restore で時刻を
  失った（0 の）tombstone は最も遅い時刻とみなす（owner の status が先）。同じミリ秒の Correction は消える
  前とみなす（時計で順が決まらないときは owner の status が先。Codex on #473）。
- **forget**: support の record を忘れると、その claim は残って proposed になる（spec 6.3 の limits に 1 行）。
  window op の文は 3d-2 の規則（span にある）で消える。
- 受入テスト: 受け入れ・答え・通った実行で立つ claim の op が support を持ち、user 自身の言葉で立つ claim は
  持たない。support の record を tombstone すると live の consumer でも rebuild でも proposed、claim は残る。
  通った実行が 2 つあって 1 つ消えても done のまま。引用の record を消すと今までどおり（evidence lost）。
  owner の Correction の status は下がらない。record forget で支えを忘れた claim は proposed で残り、その op は
  書き換わらない。support の record が消える前に owner が status を決めた claim は下がらず supersedes を保ち、
  消えた後に決めた claim は下がったまま（rebuild でも同じ）。

#### 3d-4: window が書いた claim（#476）

3e-1 の測定で見つけた穴: curator は window の 1 つの record の文を、同じ window の**別の** record を引用する
claim の本文に写せる。3d-1 は忘れた record を引用する claim を、3d-2 はその record を span に持つ window を
書き換えるが、その window が書いた他の claim は文を持ったまま残る（評価用 home の写しで、伏せた文を本文に
持つ claim が 1 件残った）。forget の場合をここで閉じる（伏せ字の場合は #478: 今の規則で op の文を走査して
その場で伏せる）。

- **(c) 同じ batch の claim**: 3d-2 の規則 (a)・(b)（と下の (b')）で record の job が当てた window op、または
  record の job がすでに書き換えた window op の batch（`curate::cover` と recuration は window op の後に claim
  op を 1 回の append で書く: 同じ device と batch で window の後に並ぶ）の claim op のうち、まだ文を持ち、
  3d-1（忘れた record を引用する、その uid）と 3e-1（伏せた record を伏せる前に引用した）が書き換えない
  ものを、window の card と同じく `{"forgotten": <window の job>}` にする（forget が書く 1 つの形のまま:
  consumer・segment・コピーはこの形を頼る）。uid は tombstone しない（生きた record を引用している）:
  3d-2 が recurate の列に入れた window の recuration が同じ uid を作り直す（MUST-M18）。window op と同じ
  transaction で書く。伏せ字（job が `rescan`）の window は対象にしない（#478）。
- **写し（`copied`）**: そのうち本文か理由（`why`）が、window の curator が見た忘れた文の 32 バイト以上の
  連続を繰り返すものの uid を、書き換えた window op に `copied`、その時刻を `copied_at` として残す（どちらも
  window op が残す構造）。見た文は、span と goals の忘れた record の保存された文（`consumer::fts::text`、
  隠す前のもの）、window が名指した（candidates・carried）忘れた record を引用する claim の文（本文・理由・
  引用と、owner の Correction が与えた本文。3d-1 が書き換える前に読む）、window が名指した写しの claim の
  文（その uid に window が与えた言い回しのすべて。Codex on #481）。判定は aho-corasick（依存に
  ある）。
- **(b') 写しをたどる**: `copied` の uid を `candidates` か `carried` に名指し、その `copied_at` の 1 分後
  より前に書かれた window op は、3d-2 の (b) と同じく当てる（job はその uid の window のもの）。ただし、その
  uid の claim op が `copied_at` の後にまた書かれた（recuration が record から同じ uid を作り直した）ときは、
  それより後の window は作り直した claim を見たので当てない（forget の直後に `oboete recurate` を走らせても、
  作り直した window を消さない）。その batch は
  (c) で、写しは名指した写しの文と比べる。window は自分より前の op しか名指さないので op の順の 1 回の読みで
  閉じるが、別の device の window は op の順（device, op_seq）で前に来うるので、写しが増えた読みの後に
  もう一度読む（増えなくなるまで）。後の purge は window op の `copied` から同じ連鎖を読む。
- **32 バイトの根拠**（評価用 home の写し、読み取りだけ、数だけ）: 写しで絞らない連鎖（window → その claim
  → それを名指した window → …）は 1 つの record の forget 30 回で window の中央値 54、最大 1,245、claim の
  中央値 155、最大 1,841（curated の window 2,691、claim op 5,314 のうち）。32 バイト以上の写しだけを
  たどると、30 回で window は最大 13、claim は合計 7。repository の forget（6,727 record）でも増えない。
  16 バイトでは window 最大 200。
- **順序**: 判定はすべて書く前に読む（3d-1 の claim が文を持つうちに）。書く順は今のまま: 3d-1 の claim →
  window とその batch の claim（window ごとに同じ transaction）→ turn → 3e-1 の claim → 行。3d-4 より前に
  書き換えた record の job の window で batch の claim が文を持つもの（この版より前の purge）は、(c) で
  `copied` なしに書き換える: `record_jobs_with_text` がその job を数えるので、更新後の最初の purge が step 2
  からやり直す。
- **step 4**: ops segment とコピーの claim op は、raw.db の当たった window（と raw.db が持たず segment か
  コピーにだけある window、`widen`）の batch（device と batch）にあれば (c) で書き換える（segment の行と
  コピーの ops は batch を持つ）。segment かコピーにだけある window からは写しを探さない（連鎖を始めない）。
- **preview**: 当たる window の数に、それらが書いた claim のうちこれで文を失う数を足す（`oboete recurate` が
  書き直すまで）。
- **限界**: 別の window の claim への 32 バイト未満の写しはたどらない（同じ window の claim は中身を問わず
  書き換える）ので、M4 の canary は 32 バイト以上にする。3d-1 の claim と window の間で止まった purge は、
  引用する claim の文がもう無いので、それを名指した window の batch の claim を `copied` なしに書き換え、
  そこから連鎖を始めない。window の途中で止まった purge も、書き換えた window の claim の文を後の window の
  写しの元にできない。復元で戻った window op を書き直すときは、その batch の claim がもう文を持たない
  ので `copied` を失う。(c) で取った claim の uid の Correction op は本文を残す（uid は recuration が作り直し、
  owner の status と mute はそれに引き継ぐ。写しを残した本文は #482）。
- 受入テスト: span に R1（32 バイトの canary）と R2 を持つ window。C1 は R1 を、C2 は R2 を引用して本文に
  canary を写し、C3 は R2 を引用して写さない。R1 の forget の後、canary は raw.db・knowledge.db・ops
  segment・コピーのどこにもない。C2 と C3 は文を失い、`copied` は C2 だけ。uid は deny されない。window は
  recurate の列にある。C2 を candidates に持つ後の window、carried に持つ別の device の window（op の順で
  前）、忘れた record を引用する claim を名指してその文を写した claim を書いた window とそれを名指した
  window は当たる。C3 を名指した window は当たらない。復元で文が戻った window と claim は purge し直す（連鎖は
  `copied` から）。purge の 1 時間後に写しの claim を名指す window、R2 から C2 を書き直した recuration の
  claim、その後の 1 分以内に C2 を名指した window は残る。raw.db が持たず ops segment とコピーにだけある window とその claim も書き換わる。preview の数。

#### 3e-1: rescan の伏せ字を保存された byte に及ぼす

rescan（spec 2.2）は、規則が増えたとき古い record を読み直し、新しい hit を範囲 tombstone（device、seq、
byte の offset と length）にする。今は読むたびに `hide` が `*` にするだけで、raw.db の行、ledger、backup
の segment、raw.db のコピー、knowledge.db（FTS の三文字組、vector）には本文が残り、伏せる前に curator が
読んで書いた claim・window・turn の op にも残る。spec 6.2 はこの hit を preview のない内部の範囲 target と
して同じ pipeline に通すと決めている（6.7 の「Rescan range」も同じ 0 hit）。

- **伏せ方は byte 数を変えない**: 範囲の byte を文字の境界まで広げて 1 byte ずつ `*` にする（`masked`。
  `hide` と同じ）。offset、ledger の位置、evidence の位置、window の `from_offset`・`to_offset` はそのまま
  使える。だから行・segment の行・コピーの行をその場で書き換えてよく、何度走っても同じ（「本文を持つ」は
  `masked(本文, offset, length) != 本文`）。
- **job の行を作らない**: 規則 1 つで数百〜数千の record に当たりうる。hit ごとに `forget_jobs` の行と要求
  ログを作ると status とログが膨らむ。record の job と同じく状態で進める（D5 item 7）: raw.db の meta に
  数で進める: purge は全部の段を終えたとき、読んだときの範囲 tombstone の数を raw.db の meta
  `rescan_masks_paid` に書く。範囲 tombstone がそれより多い間は伏せ字を払う義務がある（purge の間に増えた
  tombstone は次の回。この版より前の store と、segment から作り直した store（meta が新しい）は何も払って
  いない）。範囲 tombstone を書く transaction は何も書き足さない（数えるのは部分索引 `tombstone_targets`）。
  払う前に、開き直した raw.db の伏せた行が伏せ字の下に byte を持たないことを確かめる（purge の途中の restore が
  segment のとおりの行に戻したら払わない、Codex on #480）。
  worker の `forget --continue` もこれを見る。spec 6.2 の「進みは `forget_jobs` の行に」からの逸脱で、再開、
  doctor の表示、done の条件は数が担う（Claude; overrulable）。
- **raw.db の行**: 範囲 tombstone の対象の event 行の本文（zstd は展開して）に、その行の範囲をすべて当て、
  変わった行だけ書く（zstd の行は圧縮し直す）。その行の ledger の field を `~tombstoned` にする（export が
  segment に書くのと同じ: JSON pointer は本文の key を名指すので、伏せた key を残さない）。Removed の行は
  何もしない。
- **伏せる前の文から作った op**: 3d-1・3d-2 の `Judge` に「伏せた record」を足す（record、tombstone の
  時刻）。window op と turn op は 3d-2 の規則 (a) と同じ（span・goals・読んだ span にその record があり、
  tombstone の時刻と `SHOWN_MARGIN_MS` より前に書かれた）。claim op はその record を引用し、同じ時刻より
  前に書かれたもの（引用が範囲に重ならない claim も: window ごと recurate されるので、同じ文から作り直される。
  伏せた record を複数引用する op はそれぞれの伏せ字の時刻で見て、uid の時刻は op が先に来たすべての伏せ字のうち
  最も遅いもの、Codex on #480）。
  その claim を candidates・carried に名指し、同じ時刻より前に書かれた window op も（curator が claim の文を
  見た）。claim op の本文は window と turn の後に書き換える（uid の forget op がないので、それまでは op が
  名指しの手がかり。途中で止まっても次の purge が同じ順で続ける）。
  忘れた record との違いは 2 つ:
  - **uid を tombstone しない**: forget の step 3 の「tombstoned by uid」は record が消えるから。伏せ字では
    record が残り、recurate や #186 が同じ文から同じ uid（MUST-M18）を作る。uid を拒否すると、作り直した
    claim が書かれない（#186 の受入「伏せた文で current のまま」が成り立たない）。RD/constraints-synthesis.md:153
    の「Claims whose quote overlaps the range are curated again」に従う。claim op は op ごとに
    `{"forgotten": "rescan"}` にし、その uid の他の op（伏せた後の recurate の op、owner の Correction）は残す。
  - **record の行は Removed にしない**（上の伏せ方）。
  window op の span は 3d-2 と同じく recurate の列に入る（consumer が op から入れるので rebuild でも入る）。
  recurate は伏せた後の文を読む。
- **呼び出しの途中の伏せ字**: curator・turn の呼び出しの間に範囲 tombstone が書かれたら、forget の deny と
  同じく書く前に切り直す（`Reading` と `append_ops_fenced` が比べる数に範囲 tombstone の数を足す）。だから
  tombstone の後に書かれた op は伏せた文を読んでいる（上の時刻の判定が成り立つ）。
- **復元と時刻**（Codex on #480）: record segment の tombstone の行は時刻（`ts`）を持ち、segment からの
  restore はそれを戻す（今までは持たず、restore で 0 になり、伏せる前や forget の前に作った op を後に
  作ったと誤って判定した）。この版より前の segment から戻った tombstone は時刻を持たないので、purge は
  それを最も遅い時刻とみなす: その record の文から作った op はすべてその前に作ったとして文を失う（多めに
  取る側。forget の tombstone も同じ）。同じ record の伏せ字に時刻の分かるものがあっても同じ（最も遅い伏せ字が
  record の伏せ字の時刻なので、時刻のないものが 1 つでもあればそれ。Codex on #480）。
- **knowledge.db**: 払う伏せ字がある回は rebuild する（3c-1 と同じ: FTS の三文字組、空きページ）。vector の carry は
  伏せた record の鍵を忘れた record の鍵と同じく外す（伏せる前の文の vector を戻さない）。set aside の
  knowledge.db は今と同じく carry してから消す。
- **backup とコピー**: record segment の event 行で (device, seq, ts) が raw.db の伏せた行と同じものに同じ
  範囲を当て、ledger の field を `~tombstoned` にする（export が今書く行と同じ）。ops segment とコピーの op は
  `Judge` で（3d-2 と同じ）。raw.db のコピーの行も同じ（3c-2 の経路）。
- **status**: 払う伏せ字がある間、doctor に 1 行（止まった job ではない。`forget --status` の JSON 行は job だけ）。
- **限界**: 他の端末の record と op（M6 の同期）は伏せ字の tombstone が届いたときその端末で同じことをする。
  今は自分の端末の rescan だけ。伏せる前に curator・埋め込みの provider に送った文は provider の条件の
  下（6.3）。curator が window の中の伏せた record の文を、別の record を引用する claim の body に写した
  もの（同じ batch の claim）と、その claim を見せた後の window は #478（forget は 3d-4）。secure_delete
  （2b）より前に作った raw.db の空きページと page の中の空き領域には、その前に消した byte が残りうる（#477）。
  伏せた record から作ったと辿れない op の文（伏せた uid の Correction の body、work state、import、curator が
  自分で書いた一致）には後から足した規則が届かない（#478）。
- **止まった purge と伏せた claim の uid**（#483）: 伏せた claim の uid は tombstone しないので、claim の op と
  segment・コピーの行を書き換えると、その uid を名指した window を辿る手がかりが消える。purge は uid と
  伏せ字の時刻を、その claim の行のどれかを書き換える前に raw.db の meta `rescan_masked_uids` に残し（step 2
  は raw.db の claim から、step 4 は segment とコピーの claim の行から）、次の purge は step 2 と step 4 の
  `Judge` でそれも読む。伏せ字を払う時に消す（その時には名指した window をすべて判定済み）。step 4 は
  segment を名前の順に書き換える（止まる所が毎回同じ）。uid は文を持たない（MUST-M18）。segment から作り
  直した raw.db（新しい meta）は残した uid を持たない（止まった purge の後の restore）。
- 受入テスト（6.7 の Rescan range）: 古い規則の下で秘密を含む prompt を記録し、それを引用する claim、
  その window op（summary にも秘密）、それを読んだ turn op を書き、backup を書き、raw.db のコピーを set
  aside し、規則を足して rescan、purge。raw.db（展開した本文も）・WAL・knowledge.db・すべての segment
  （展開して）・コピーに秘密の byte が 0 件。record の他の文は検索に出る。claim の window は recurate の列に
  ある。呼び出しの途中に伏せ字が入った window は書かれない。2 回目の purge は何も書かない。restore の後に
  伏せ字はまた払う義務になる。この版より前に書かれた範囲 tombstone も払う義務になる。伏せた後に伏せた文
  から作った claim op は残り、その uid は拒否されない。purge の途中に書かれた伏せ字は義務に残る。

#### 3e-2: 伏せた directive（#186）

`oboete pref add` の directive は window を持たないので、3e-1 で claim op が本文を失うと recurate では
作り直されない（curate は directive を window に出さない。recurate の列に入っても `covered` で終わる）。
preference は伏せた文で残るはずが、消えていた。

- **作り直す場所は purge**: 3e-1 の purge が伏せた record を引用する claim op を書き換えた後、同じ worker
  の lock の下で、伏せ字を持つ record のうち directive（event の kind）ごとに、その event を今読める形
  （伏せ字の後）で読み、`pref_add` と同じ op（`claims::preference_op`: 文全体の引用、offset 0、
  sentence 0、body は引用と同じ、recipe `oboete pref add`）を足す。伏せ字は byte ごとに `*` にして長さを
  変えない（3e-1 の `masked`）ので、引用の offset と length はそのままで、uid は kind と (device, seq,
  sentence) なので同じ（MUST-M18）。owner の Correction（status・body・mute）は uid に付くのでそのまま
  効く。consumer が derive し、preference は伏せた文で current に戻る。rescan から purge までの間は
  Anchors が今どおり落とす（隠れる側）。
- **状態で決める**: 作り直すかは「その uid の op で、引用が今の directive の文のとおりに読めるものがない」
  かどうかで決め、書き換えた op からは読まない。古い op の文を取った後で止まった purge も、次の回で足す
  （伏せ字は払われるまで残る）。uid が forget されたもの（`forget --uid`）と、record ごと消えたもの
  （読めない）は足さない。
- **作り直した op を後の purge が消さない**: 3e-1 の claim op の規則は時刻（伏せ字の時刻と
  `SHOWN_MARGIN_MS` より前に書かれた op）なので、伏せ字の 1 分以内に足した op を、後の purge（別の
  伏せ字で印が立った回）が書き換えてしまう。preference の op は本文が引用そのもので、他に何も読んで
  いないので、引用が今の event のとおりに読める（`consumer::claims::live`）ものは伏せる前の byte を
  持たない。raw.db の `record_claims` はそれを数えず、step 4（segment とコピー）は raw.db でそう読める
  op の行を書き換えない（`Judge` の `now`。raw.db が持たない行は今までどおり時刻で決める: 多めに取る側）。
  同じ directive をさらに伏せる規則が来れば、引用が今の文と違うので取られ、2 回伏せた文から作り直す。
- **限界**: 伏せ字は byte ごとに `*` にするので、多 byte の文字を伏せると文字数が増える。1,000 文字の上限
  （`MAX_BODY_CHARS`）を超えた preference は consumer が skip する（今と同じく消え、`claim_skips` に理由）。
  `pref add` が 2 つの append の間で止まって op のない directive も、伏せ字の purge で preference を得る
  （owner が頼んだもの）。他の端末の directive は M6。
- 受入テスト（#186）: `pref add "Use tabs, key acme-…"` に mute を付け、backup を書き、規則を足して
  rescan、purge: preference は伏せた文を body に持つ current の global claim で、uid と mute は同じ。
  raw.db・knowledge.db・backup に鍵の byte がない。別の record にだけ当たる規則を足して purge しても、
  作り直した op は書き換えられない。directive の別の語を伏せる規則を足し、古い op の文を取った直後で
  purge を止める: 次の purge が 2 回伏せた文から作り直し、文を持つ preference の op はそれだけ。

#### 3e-3: 保存期間（retention）

spec 6.2 の Retention（owner decision 16、既定は無期限）: 設定の鍵（`config.toml` と設定画面）、期限を
過ぎた自分の端末の raw の行・その backup segment を 3e-1 と同じ書換えで消す（tombstone を作らない）、未
curate の raw（checkpoint の上、pending の window、v1 の取り込み）は残す、claim の証拠は `raw expired`、
期限切れの span の `oboete recurate` は拒否。PR で決めることだった 2 点（証拠の record が期限切れの claim の
status を 3d-3 の support が期限切れのとき proposed に戻すか、期限切れを読み手がどう見分けるか）は、下の
とおり決めた。

決めたこと（docs/retention.md、Claude; overrulable）: 期限切れの record は event の行のまま、ラベル・ledger・
出所を残し、本文を空にして `expired` 列に時刻を入れる（目印は「あるもの」で見分ける）。status は支えの
record が期限切れでも保つ。引用は `raw expired` と出して本文は見せない。curate の window（重なる再 curate
の window と合わせたもの）ごとに丸ごと期限切れにし、その中の directive と v1 の touch の行は期限切れに
しない。3 本の PR（読み手、期限切れの処理、設定画面）。

#### 3e-4: 規則が変わった後の op の文（#478）

3e-1 の purge が伏せ字を及ぼすのは、伏せた record から作られた op（時刻と出所で辿れる claim・window・
turn）だけ。それ以外の op の文（owner の Correction の body、作業メモ、取り込んだ文書、curator が自分で
書いた語、別の record を引用して伏せた文を写した claim）は、書いた時の規則でしか伏せていない（読む時は
egress の gate が今の規則で伏せる）。

- **今の規則で op の文を伏せる**: 規則の版（`Rules::version`: `[redaction]` の規則と残す値で決まる）が
  raw.db の meta `ops_text_scanned`（op の文を最後に走査した版）と違う間、purge が step 2 で raw.db の
  op を全部読み、本文の JSON の文字列値ごとに今の規則で探し（`redact::ranges`）、見つかった所の文字を
  1 文字ずつ `*` にする。record の伏せ字（3e-1）は ledger の offset のために byte 数を保つが、op の文に
  offset はなく、文字数も byte 数も増やさない方を取る（訂正や claim が 1,000 文字の上限を超えて読み捨て
  られないように。Codex on #487）。値を解析して書き直すので JSON は壊れず、キーの順も変わらない
  （`preserve_order`）。再 curate の列には入れない。読みと書換えは op の `(device, op_seq)` 順に 500 件
  ずつ、raw.db でも写しでも（広い規則が log の大半に当たっても、持つのは 1 ページ分）。worker の確かめは
  最初の 1 件で止める（Codex on #487）。
- **伏せない値**（`OP_NAMES`）: oboete が付ける名前（識別子と自分の語彙）の値: `uid`、`id`、
  `supersedes`、`candidates`、`carried`、`copied`、`device`、`forgotten`、`job`、`source`、`source_id`、
  `session`、`repo`、`list`、`kind`、`type`、`status`、`speaker`、`scope`、`recipe`、`outcome`、`agent`、
  `provider`、`dropped`、`lowered`、slice 4a の除外の op の `folder` と `level`。window の `reason` は oboete
  が provider に聞かずに外したとき（取り込みの `imported:<source>` と除外の `excluded`、`curate::set_aside`。
  `oboete recurate` が読む）だけ伏せない。provider の失敗の理由は文なので伏せる（Codex on #487）。それに
  record の文をそのまま写した `quote`（その record の
  伏せ字が決める。引用だけを走査すると、行頭に固定した規則が record の欄の途中とは違う所に当たり、
  引用が読めなくなる）。キーも伏せない。`fields`（作業メモと turn の要約）の中は全部が文。forget の op
  （uid と job）と migration の op（checkpoint と v1 の session id の SHA-256）は文を持たないので読まない。
  64 桁の 16 進を探す利用者の規則が uid を書き換えると、訂正・forget・取り込みの対応が切れ、forget した
  claim が戻る（`view::gated_with_ids` と同じ考え）。repo と list を伏せると op の所属が変わる。名前は
  評価用の store 3 つにある op の文字列の欄を全部数えて決めた（値は読まない）。
- **進み方**: 版が違えば走査する（新しい store とこの版より前の store も 1 回。何も見つからなければ何も
  書かない）。走査は書く transaction の外で、書換えは dispatch の下の `RECORD_PURGE_BATCH` ごとの
  transaction（本文が読んだ時のままの op だけ）。op を書き換えた transaction で meta `ops_text_pending`
  を立て、それがある間 knowledge.db を作り直す（止まった purge が残しても、次の回の走査は何も書かないが
  作り直す）。step 4 は版が違う回ごとに backup の ops segment とコピーの op を全部読み、同じ書換えを
  する（raw.db が持たない op にも、uid を判定できない op の行にも。後者は判定できない行のまま。Codex on #487）。raw.db-wal を truncate でき step 4 まで終わった回が、始めに読んだ版を
  記録して pending を消す（途中で版が変わったら次の回がもう一度走査する）。記録は走査を始めた raw.db
  にだけする（始めた印 `ops_text_scanning`）: 途中の restore が segment から作り直した raw.db（新しい
  meta）はその segment の op を持つので、走査を払わない。worker の `continue_due` は版が違う間、backup の
  ops segment か raw.db のコピーがあれば `forget --continue` を起こす（raw.db が持たない op を持ちうる。
  restore が外した op など。それを読むのは purge の step 4 だけ。Codex on #487）。どちらも無ければ raw.db
  の op を自分で読み、規則が何も伏せなければ走査を記録して purge を起こさず（新しい store の最初の回も）、
  伏せるものがあれば `forget --continue` を起こす（設定が読めない間は待つ）。
- **3e-1・3e-2 の後**: 走査は 3e-1 の書換え（伏せた record から作られた op）と 3e-2 の preference の後。
  3e-1 の判定は変えない（伏せる前に書かれ伏せた record を引用する claim は、走査で引用がまた一致しても
  作り直しのために文を失う）。3e-2 が作り直した op は文が伏せた引用そのものなので変わらない。
- **報告**: `Ops whose text the current redaction rules masked: N`。raw.db-wal を truncate できなかった
  回は 3e-1 と同じく未完了（`MASK_JOB`）。
- **限界**: キー（作業メモの fields の名前など）と上の名前の値は伏せない（repo に当たる規則でも）。
  vector の cache は伏せる前の文から作った vector をその sha で持ち続ける（文字ではない。作り直した
  vector_keys は伏せた文の sha を指す）。他の端末の op は M6。規則を変えるたびに op を全部読む（backup とコピーがあればそれらも。測る）。
- 受入テスト（#478）: (1) window の span の record が秘密を持ち、同じ window の claim が別の record を
  引用して body に秘密を写し、後の window がその claim を候補に出して summary に秘密を写す: purge の後、
  どれも秘密を持たず、claim は伏せた body で current のまま、どちらの window もこのためには再 curate の
  列に入らない。(2) Correction の body、作業メモ、取り込み、window の summary が秘密を持ち、規則を足す:
  rescan と purge の後、raw.db・その WAL・knowledge.db・ops segment・raw.db のコピーに 0 件、各 op は
  前と同じに読める（Correction は uid の status を直したまま）。(3) 同じ規則の 2 回目の purge は走査
  しない、3 つ目の規則でまた走査する。(4) 64 桁の 16 進の規則を足しても、Correction・forget・候補の
  uid は変わらない。(5) 大きなコピーで走査の時間と hook の待ちを測る。(6) backup の ops segment か
  raw.db のコピーだけが持つ op（raw.db には無い）も、規則を足した後の purge が伏せ、worker はそれまで
  走査を記録しない（Codex on #487）。

### Slice 4 の決定（2026-10-13）

Slice 3 の残り（3e-3 の保存期間）より先に、切替の前提である「記録しない」を作る（spec 7.5、owner
decision 27・34。保存期間は既定が無期限で、切替の前提にない）。設定画面の行（settings-coverage.md の
「Stop recording a repository or folder」、W3）と setup の質問は 4b、mute のボタンと訂正の残り（#157・
#160・#162）はその後。

#### 4a: 記録しない（capture exclusion）

spec 6.1 の「記録しない」（owner decision 16・19、A28）。

- **一覧は raw.db の op**: repo は section 5 の exclusion op に `level: "capture"` を付けたもの
  （`{repo, undo, clock, level}`。M6 で他の端末にも届く）。folder は `{folder, undo, clock, level:
  "capture"}`（この端末のパス。M6 は送らない）。どちらも `oboete exclude` の op と同じ順序（端末の op
  順、その後に時計）で最新が勝ち、backup から restore しても戻る。
- **送信の除外も兼ねる**（spec 6.1）: level のない op と capture の op を別の集合として計算し、送信の一覧
  （`exclusions_in`）はその和。capture の undo は capture の集合からだけ外し、`oboete exclude` で別に除外
  した repo は送らないまま。逆に `oboete exclude --undo` は capture の repo を送信に戻さず、そう表示する。
  設定画面の送信の除外の undo も capture の repo には効かない（4b で行を分ける）。
  外へ送る経路はどれも一覧を `Reading::now` から取り、それは `Raw::exclusions_now`（その和に、record が持つ
  repo の label のうち今の規則で読むと記録しない repo になるものを足す。hook が event の label を読むのと同じ）
  を読む: 規則を足した後に除外した repo は伏せた label で載り、規則の前の record は伏せていない label を持つ
  ので、その session も送らない（Codex on #489）。記録しない repo があるときだけ設定を読み、読めなければ何も
  送らない。一覧には今の規則で読んだ除外の値も載せ、取り込み文書の project（埋め込み）と検索のクエリを送る
  checkout の repo も今の規則で読んで比べる（record を持たない repo、規則で同じに読める別の repo も。Codex・
  Cursor on #489）。その後に外した規則で伏せた label（`github.com/o/[REDACTED]`）はもう読み直せないので、
  伏せ字をどんな文字列とも読んで記録しない repo のどれかになりうる label も一覧に足し、その session を送らない
  （安全側に多めに止める。Codex's security review of #489）。
- **入力どおり保存し、今の規則でも読んで比べる**（owner 2026-10-13）: repo は与えられた key のまま、folder
  は `capture::folder_path` を通した絶対パスのまま、伏せ字をかけずに保存する（spec 2.2 の明示の例外。
  伏せ字を切ったり規則を足し引きしたりしても除外が効き続けるように）。値が一覧の値 R に合うのは、R と
  等しいとき、R を今の規則で読んだ形（`capture::never_recorded`）と等しいとき、そして hook が gate の前の
  repo と cwd を持つところでは、それが R と等しいか R の中（path の component 単位、Windows と macOS では
  大文字と小文字を区別しない）のとき。記録済みの event の label はその時の規則で読まれているので、規則の
  前の record は R そのものに、後の record は今の規則で読んだ R に合う。どの書き手も同じ問い
  （`CaptureExclusions::refuses`）で確かめる: hook の書込み transaction（`Raw::append_captured`）と MCP
  の作業メモ（`Raw::work_state_under`）は gate の前の値も渡し、doctor と MCP の理由（`not_recorded_why`）
  も同じに読む。止まった hook の書き戻し（spool は label だけを持つ）と取り込みは label どうしで比べる
  （規則が区切りを食う場合はそこで合わないことがある。push の security review on #489）。
  hook の `cwd` が相対か無いときは、hook が動く directory を絶対パスにしてから比べる。CLI の folder と
  hook の `cwd` はどちらも `capture::folder_path` で、`.` と `..` を名前の上で外し、末尾の区切りを落とす
  （link はたどらない。Codex on #489）。取り消しは保存された値を探す（folder は大文字と小文字を区別しない
  platform ではそのように、なければ今の規則で同じに読める 1 つ。`capture::undone`）。claude-mem から取り
  込む一覧と文書は project の名前で比べる（D13）。規則が区切りを食う、名前の外の文脈に当たる場合も、gate
  の前の値と入力どおりの値どうしなら合う（#496）。設定画面が repository を選ぶときは record の label しか
  持たないので、その label を保存する（伏せてあればそのまま）。`folder` と `level` は `repo` と同じく op
  の名前で、#487 が `OP_NAMES` に足す。
- **`.oboete.toml`**: checkout の top（`repo::key` が `.git` を探す一度の歩きで分かる、worktree なら
  その worktree の top）の `.oboete.toml` の `capture` の鍵だけを読む（他の鍵は見ない）。`capture =
  false` なら記録しない。`capture = true` は一覧を外さない。ファイルが読めない、TOML として読めない、
  `capture` が真偽値でないときも記録しない（repo の設定は制限しかできないので閉じる側に倒す。Claude;
  overrulable）。通常のファイルで 64 KiB までしか読まない: repository が持つファイルなので、device を指す
  link や大きなファイルは読まずに記録しない側にする（hook を止めない。push の security review on #489）。
  そのとき hook は stderr に一行、doctor はその checkout で一行出す。読むのは hook の経路
  だけ（取り込みは checkout が動いた後なので op の一覧だけを見る）。
- **どこで断るか**: `Raw::append_with_ledger` と `append_imported_origins` の書込み transaction の中で一覧
  を読み（保存した値のまま比べる）、除外なら書かない（slice 3b の忘れた session と同じ `Ok(None)`）。除外
  の op が commit された後に書く append は必ず断られる。hook は raw.db を開けたら一覧を呼び出しごとに一度
  読んで `built` で先に外し、unwritten の spool にも入れない。開けないとき（docs/unwritten.md U2）は
  spool に置き、書き戻しの append が断る（制限: それまで spool のファイルに残る）。取り込み（transcript、v1 の
  event と `session_repos`）は `append_imported_under` で、その取り込みの settings で一覧を gate し直した値
  とも比べる（hook と同じ）。spool の書き戻し（`append_kept`）も、書き戻す hook の settings で同じく比べる
  （Codex on #489）。
- **hook の時間**: `oboete replay` の hook の p95 を前後で測り、PR に書く（Windows は 25.5 ms の線の上）。
- **work state**: 除外された repo か folder の checkout からの書込み（MCP、docs/work-state.md）は断る。MCP
  の確認の後に除外が commit されても、`append_ops` が書込みの transaction の中で一覧を読み直し、work state の
  op の repo（取り込んだ一覧は claude-mem の project の名前の repo、D13）と書いた場所（MCP の cwd の label）
  を見て断る（`NeverRecorded`）。project の名前で比べるのは取り込んだ一覧（op に `source` がある）だけで、
  ここで書いた一覧は `work_state_in` が読むとおり自分の key だけで比べる（key が `claude-mem:<名前>/x` の形
  でも）。claude-mem から取り込む work state も同じ規則で入れず、`never_recorded` と数える
  （docs/claude-mem-import.md I6）。取り込みの append（`append_imported_ops`）は、取り込みの settings で
  project の名前を gate した形とも transaction の中で比べる（取り込みの途中に、規則が名前の一部を伏せた除外が
  commit されても断る）。claude-mem が持ち続けるので、undo の後の取り込みでは入る（Codex on #489）。
- **CLI**: `oboete capture exclude [<repo>] [--folder <path>] [--undo]`。repo を省くと今の checkout の
  repo（`oboete exclude` と同じ label）。folder は絶対パスにして保存する。結果の一覧を表示する。repo を
  除外したとき、端末なら「すでに記録した分も忘れますか（既定: 残す）」と聞き、はいなら `forget --repo`
  の preview と確認に進む。端末でなければ `oboete forget --repo <repo>` で消せると表示する。folder は
  forget の target にないので、記録済みは残ると表示する。forget の案内と問いは、規則が今の label を伏せて
  いても、与えた名前（記録が持つ名前）で `forget --repo` する（Codex on #489）。undo は保存した値を外す: 与えた名前（入力のまま、次に今の
  label）で保存したもの、なければ今の規則で gate し直すと入力の label と等しくなる 1 件。2 件以上なら何も
  せず断り、それらを並べる（除外した時の名前で選ぶ。Codex on #489）。
- **見ないもの・制限**: claude-mem の取り込み文書（import op。`claude-mem:<name>` との対応は送信の除外の
  D13 の話）。他の repo の session が除外した repo のファイルを読んだ tool の出力は記録される（spec 6.1 の
  制限）。除外していた間に取り込みが飛ばした event は checkpoint の後ろにあるので、undo しても戻らない
  （除外していた間の記録が戻らないのと同じ）。除外の後に規則を外すか狭めると、その規則が伏せた部分を含む
  除外の値は event の label と合わなくなる（除外し直す）。folder の op は端末で分けない: raw.db を別の機械に
  写すと、元の機械で除外した folder は、その機械でも同じパスなら記録しない（写しも restore で置いた新しい
  ファイルも新しい端末 id になるので、端末で分けると自分の除外が外れて記録が戻る。安全側。Codex on
  #489）。除外の前に folder で記録した分の送信は #490（送信の除外は repo 単位で、folder にはまだない）。規則の追加、
  除外の commit、settings を先に読んだ hook の書込みが同じ数ミリ秒に重なると、その hook の event は記録され
  うる（#492）。

受入:
- 除外した repo と folder の canary の event（hook 経由）は raw.db にも spool にも 0 件。undo の後の event
  は記録され、除外していた間の event は戻らない。
- `.oboete.toml` の `capture = false` と壊れた `.oboete.toml` で 0 件（doctor が言う）。`capture = true` は除外を外さない。
- 後から足した規則が除外した folder の path の一部を伏せても、その folder の event は 0 件。
- capture の除外は送信の一覧に入り、その undo は `oboete exclude` の除外を残す。
- 除外の op の commit の後に書く append（書込みを待っていた hook、spool の書き戻し）は断られる。
- transcript の取り込み（一覧だけ）と work state の書込みも除外を見る。
- 後から足した規則は取り込み（transcript、v1）でも除外を外さない。除外の op の commit の後の work state の
  書込み（MCP、claude-mem の取り込み）は書込みの transaction の中で断られる。規則を足した後の undo は保存
  した値を外す。

#### 4b: 設定画面の「記録しない」

settings-coverage.md の「Stop recording a repository or folder」（W3）と、初回の設定の質問（spec 6.1 の
「setup」。spec-webui.md により初回の設定は画面で行う）。

- **画面**: プライバシーの群に「記録しない」の節。一覧にある repo（送信の除外と同じ選択子、サーバで引き直す）
  ごとに「今後は記録しない」と「今後は記録を再開」。この端末の folder は絶対パスを入れて足し、一覧の各 folder
  は自分の選択子（保存した label の hash）で外す。backend は `oboete capture exclude` と同じ
  `Raw::exclude_capture`（`POST /api/privacy/capture`、送信の除外と同じ token・Host・保存の lock）。
- **送信の除外の行**: 記録しない repo はその行で「記録しないため送信もしません」と表示し、送信の undo を出さ
  ない（4a の制限を解く）。
- **初回**: 初回の案内（onboarding）に「記録したくない場所は次のセッションの前に指定」の一文と「記録しない」の
  節への移動のボタン。
- **断る**: 相対パス、4,096 bytes を超える path、NUL、知らない選択子、二つ以上の対象を名指す要求は何も書かずに
  断る。
- **見ないもの・制限**: 画面の repo の一覧は記録か除外のある repo だけ（まだ記録のない repo は、その folder か
  `.oboete.toml` か CLI で止める）。記録済みを消すのは forget（画面の forget は slice 5）。

受入:
- 画面から repo と folder を記録しない一覧に入れ、外せる。一覧の取得はその両方を返し、記録しない repo は送信の
  行で undo を出さない。
- 断る要求は op を書かない。
- 画面の文言は日英で、`TEXT` の鍵は一つにつき一度だけ定義する（4b の途中で既存の「記録」の見出しを上書き
  しかけた）。

## 実装順序と完了条件

| Slice | 対象と再利用する処理 | 完了条件 |
|---|---|---|
| 1 削除要求 | `forget.rs`, `raw.rs`, `backup.rs`, `worker.rs`, `main.rs`, `migrate.rs`, `transcript.rs`, `curate.rs`, `turns.rs`, `provider.rs`, `dispatch.rs`, `embed.rs`, `embed_phase.rs`, `shortlist.rs`, `claims.rs`。既存 SQLite transaction、raw.lock、import checkpoint。 | import origin のある record/device span（1 要求 500 件まで）の preview/start/status、raw.db の deny と本文なしの要求ログ 2 部、古い backup の restore・raw.db の巻き戻し・再 import による復活防止、派生書込と送信の fence。物理 purge は未完了と表示する。native identity のない raw、uid/session/repo/time の start は後続まで拒否する。 |
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
Linux の要求ログのメモリ上限試験には util-linux の `prlimit` が必要。試験の最初に
利用可否を確認し、未導入なら導入してから `cargo test` を実行する。上限の試験は省略しない。
CI/security gate は弱めない。slice ごとに PR を開き、repository の通常のレビュー経路を通す。

- M4 の削除測定のローカル部分: 管理対象内で 0 hit。復元、再 index、再 import、crash、
  in-flight、rescan、retention の各ケースを含む。これは「段階 4」の検索品質測定とは別。
- A104/Transcript: canary を forget 後、同じ v1/claude-mem/transcript を再 import しても戻らない。
- FTS5 は行を消しても三文字組を segment に残す（`raw_fts` の本文の写しの有無によらない。
  docs/spike/raw-index-copy.md）。Slice 2 の物理 purge は `raw_fts` と `imported_fts` に
  `INSERT INTO <表>(<表>) VALUES('optimize')` をしてから VACUUM し、その後に canary を探す。
  `contentless_delete=1` の表は、消した行を bm25 の合計（行数・語数）からも引かない。順位の近い
  ヒットが入れ替わる程度でヒット自体は変わらず、`oboete rebuild` で knowledge.db を作り直すと戻る。
- M22 forget: 段階 4 Task 12b の scale-home 作成器と stub を再利用し、hook writer と並行して
  段階別時間・検索/注入遅延・未完了理由を測る。仕様にない固定秒数を合格線として加えない。
- 合成データによる build/故障試験はラベル・実モデル・held-out corpus から独立して進める。
  cut-over と M5 完了には段階 3・4 の既存品質ゲートも必要。今回 owner cut-over はしない。
- device/hub/R2/Vectorize の purge、ack、stale-device、hub reseed は M6。
  本段階はその bodyless control の適用入口を備え、通信サービスは先行実装しない。

## 実装状況

- [x] Slice 1: 削除要求と raw record/span の安全な受理（#389）
- [x] Slice 2: uid の物理 purge（D5。2a 登録と隠す・送らない、2b op 本文と knowledge.db、2c backup）（#435、#442、#444、#453、#458）
- [ ] Slice 3: raw scope、rescan、retention
- [ ] Slice 4: 可逆な管理と訂正
- [ ] Slice 5: WebUI
- [ ] Slice 6: 安全性・測定・独立レビュー

未チェックの slice がある間、この段階は完了ではない。

2026-10-14（owner decision 45）: 切り替えは Slice 3 の残り（3e-3 の保存期間）、withdraw、Slice 6 を待たない。切り替えの前に作るのは、Slice 4 の mute/unmute と owner の 2 台で起きうる訂正の修正（#157 から）、Slice 5 のうちそれを操作する画面まで。

### 第一 slice の実装・検証（2026-10-03、設計 v2）

設計 v1（journal）の実装を v2（D1）に置き換えた。`forget_jobs.step` は 1 のままで、物理 purge
完了を返す経路はない。

- 公開 CLI の主な回帰シナリオ（`tests/forget_cli.rs`）: import origin のない record の登録前の拒否、
  本文なしのログと届かない範囲の表示、失われた・古い・他 home の・壊れたログが hook を止めないこと、
  古い backup と capture rule の変更で v1 の event が戻らないこと、redaction rule が変わっても
  コピーした transcript が忘れた identity を保つこと、壊れた raw.db がログから忘却を回復すること、
  backup ディレクトリに届かなかった忘却を home のログだけで戻すこと、巻き戻った raw.db で新しい
  record が残り忘れた record は戻らないこと、raw.db と両方のログとその後の backup を失うと（home
  全体が古いコピーに戻ると）forget の表示どおり本文が戻ること、破れたログ行の後の忘却も restore で残ること。
- 型付き API と fence の試験: `forget.rs` の登録・ログ回復と、provider の呼出し中に登録された忘却が
  その答えを何も書かせないこと（`curate.rs`）。障害試験にオーナーのデータ・モデル・サービスは
  使っていない。
- `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`: 成功。

常駐 worker は、保持している raw.db と同じパスのファイルが別のものになったとき（古いコピーを手で
戻したときなど）、次の確認で stores を開き直す（`worker::serve`、テスト
`a_resident_worker_opens_raw_db_again_when_another_file_takes_its_place`）。置き換わったファイルは
別の端末の記録として読まれる（docs/cards.md S3）。

引き継ぎレビューで確認した 2 件の復活経路を修正した。
raw.db を別ファイルに置き換えても home の id を保持し、古いコピーへの再置換やコピー後の backup
からの復元で削除要求を適用する。v1 と transcript の共通 event id がない記録の登録を拒否し、
既存 v1 deny に重なる transcript は、restore 後と準備済み batch の追記時も明示拒否する。
逆方向の v1 import と、対応不能な native copy がある home の transcript 登録も拒否する。
伏せ字ルールを外しても元の session hash を使い、raw への要求適用がロックで失敗した import は
続行しない。前の行の追加、同じ event の重複、native session id のない記録も合成データで試験した。
CLI と、準備後の忘却が追記を止める型付き API の試験で、先に失敗することと
修正後の通過を確認した。保存済みの先行 raw と無関係な新規記録は残る。既存 v1 deny と
対応を証明できない未取り込み transcript は、bridge ができる後続 slice まで保留する。

残り: 独立レビュー。この結果は全 M5 の削除 canary 成功ではない。物理
purge、live/legacy identity、大量 selection、uid/session/repo/time、可逆な管理、WebUI、各 OS の
安全性と M22 は後続。
