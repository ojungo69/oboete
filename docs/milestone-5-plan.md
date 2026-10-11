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
  record segment は触らない。
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
- [ ] Slice 2: uid の物理 purge（D5。2a 登録と隠す・送らない、2b op 本文と knowledge.db、2c backup）
- [ ] Slice 3: raw scope、rescan、retention
- [ ] Slice 4: 可逆な管理と訂正
- [ ] Slice 5: WebUI
- [ ] Slice 6: 安全性・測定・独立レビュー

未チェックの slice がある間、この段階は完了ではない。

### 第一 slice の実装・検証（2026-10-03、設計 v2）

設計 v1（journal）の実装を v2（D1）に置き換えた。`forget_jobs.step` は 1 のままで、物理 purge
完了を返す経路はない。

- 公開 CLI の主な回帰シナリオ（`tests/forget_cli.rs`）: import origin のない record の登録前の拒否、
  本文なしのログと届かない範囲の表示、失われた・古い・他 home の・壊れたログが hook を止めないこと、
  古い backup と capture rule の変更で v1 の event が戻らないこと、redaction rule が変わっても
  コピーした transcript が忘れた identity を保つこと、壊れた raw.db がログから忘却を回復すること、
  backup ディレクトリに届かなかった忘却を home のログだけで戻すこと、巻き戻った raw.db で新しい
  record が残り忘れた record は戻らないこと、raw.db と両方のログを失うと forget の表示どおり本文が
  戻ること、破れたログ行の後の忘却も restore で残ること。
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
