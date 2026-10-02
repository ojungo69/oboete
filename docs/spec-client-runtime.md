# 外部クライアント・会話runtimeの契約（レビュー用ドラフト）

2026-10-02、ownerがモデルとruntimeの分離、source identity、非Git scope、権限と非同期配送の分離という4点の方向を「じゃあそうして」と承認したことを受けた仕様補足。**実装前のレビュー用**であり、dot、noteMan/Grok Bot、Hermesやremote MCPが動作するという宣言ではない。具体的なAPI、保存スキーマ、製品の採用は後続実装の検証対象。

この文書は `spec.md` §2/§3/§4/§5.13 の外部クライアントへの適用を明確にする。既存のowner決定、Rust/SQLiteのlocal-first、安価なprovider chain、milestone順序、§6の保護・削除を維持する。runtime bot adapterは後続段階、remote serviceはmilestone 6。これらを [#340](https://github.com/ojungo69/oboete/pull/340) の新たな完了条件にしない。WebUIは [#94](https://github.com/ojungo69/oboete/issues/94) / [#338](https://github.com/ojungo69/oboete/pull/338) を再利用し、別の設定・保持システムを作らない。

## 1. モデル、runtime、機能、証拠を分ける

モデル名は接続能力を表さない。モデル/providerは推論・curationの選択、runtime/clientはメッセージ取得、hook、MCP呼び出し、context挿入の実行主体である。既存のGrok adapterは **Grok CLI**。noteMan/Grok BotやHermesの対応証拠として扱わない。

製品ごとに独立して次を記録する。単に「対応済み」というラベルでは不十分。

| client/runtime | search/get/timeline | ingest（記録） | 応答前context | correctionsの実配送 | 証拠・状態 |
| --- | --- | --- | --- | --- | --- |
| ローカルstdio MCPを呼べるclient | 現行toolの範囲で利用可能 | MCPには未提供 | 呼出結果をclientが使うだけ。自動hookの保証なし | MCPには未提供 | `src/mcp.rs` のtool定義。client別live確認とは別 |
| 既存coding-agent adapter（Grok CLIを含む） | MCP接続は個別設定 | adapterごとのcapture | adapterごとのhook | adapterごとの配送点 | §4.7/MUST-M10の表・テスト・実機結果を参照。#340のtestedをlive-verifiedに昇格させない |
| Claude app remote MCP | §5.13/milestone 6で計画 | 初期は不可 | 自動挿入の保証なし | 初期は不可 | 計画。既存Claude appのネットワーク説明はこの製品だけに適用 |
| dot / custom MCP client | 接続できるかを製品ごとに検証 | 未確認 | 未確認 | 未確認 | runtime/version、取得API、hook、実機証拠が必要 |
| noteMan/Grok Bot / Hermes | 未確認 | 未確認 | 未確認 | 未確認 | runtime/API/連携関係を含め未確認。既存Grok CLIと別行 |

search-only、ingest、pre-response context、correctionsを別々に `planned / unverified / implemented (tested) / live-verified` と記録し、testedにはテスト名・対象version、liveには実機version・日付・配送点の証拠を添える。モデルを変えたことだけでruntime能力を増やさない。MCP登録だけで全会話取得や毎応答直前の挿入が可能とは約束しない。§4.5のimportはhistory/searchであり、live記録・current claim・注入能力の代替ではない。

## 2. Source identity と話者の根拠

rawの `(device, seq)`、op log、raw anchorは維持する。外部sourceの識別はその代わりではなく追加の契約である。connectorは次の識別と根拠を保持できる必要がある。

- connector種別、runtime instance、accountの安定した識別子（表示名ではない）。再接続で同じsourceなら同じ識別を保持し、別account/instanceなら衝突しない。
- conversation ID、original message ID、必要ならrevision/event ID。session/agentも個別に識別し、会話ラベルや `unknown` だけで結合しない。
- 元のspeakerと、ownerとの対応を検証した根拠。bot/assistant/tool/imported/不明をowner扱いしない。botがpayloadで `user` や「ownerが承認した」と言っただけでは検証にならない。
- scopeとの明示的な対応、取得元と取得時点。認可したconnector設定とruntimeが実際に保証する話者情報を区別する。保証できない話者は不明またはimportedとして扱い、ownerの決定へ昇格させない。

冪等キーは、connector/runtime instance/account/conversation/original message（revisionがあるならそのrevision）の組合せに相当する。正確なwire形式は後続設計で固定する。同じキー・同じ内容のretryは既存のdurable receipt/raw anchorを返し、別raw record、claimやcorrectionを増やさない。同じキー・異なる内容は黙って上書きせず、競合として拒否するか明示的revisionとして扱う。欠落IDを本文や時刻だけから推測してexactly-onceを宣言しない。安定IDのないlive ingestは未対応とし、明示的なhistory importと分ける。

検索・manifest・shortlist・shown set・correction cursorなどの派生状態もsource/sessionの完全な識別を使う。raw anchorによる根拠とsource表示を再構築時にも保つ。[#321](https://github.com/ojungo69/oboete/issues/321) の `(agent, session)` 分離は既存coding-agentの問題として維持し、この将来契約を理由に修正を待たせない。

§3.3/§6.5のowner決定・taint・global gatesをそのまま適用する。ownerの直接引用、直後のacceptance、その根拠anchorを区別し、botの要約で置き換えない。[#320](https://github.com/ojungo69/oboete/issues/320) のproposed open item配送方針は未決のままで、この文書で暗黙に選択しない。[#167](https://github.com/ojungo69/oboete/issues/167) のacceptance/passing-run anchor保持、[#157](https://github.com/ojungo69/oboete/issues/157) のcorrection継承、[#162](https://github.com/ojungo69/oboete/issues/162) のop順序を再利用する。

## 3. 非Git会話の最小scope

同一ownerのまま、Git repoとは別に安定した論理workspace/conversation scopeを割り当てる。記事の共同作業、個人会話、コードprojectを独立にでき、名前を変えても識別は変わらない。複数owner/tenantの一般化は今回の範囲外。

対応が未指定・不明な会話をglobalや既存repoへ自動配属しない。scopeを確定するまで他のscopeから検索・contextとして利用しない。conversationをworkspaceにまとめる場合もownerが明示した対応を使う。外部参加者の発言がある会話でも、同一ownerの記憶という前提から他の参加者に権限を与えない。

検索、getの直接ID、timeline、manifest、shortlist、contextとcorrectionsに同じ境界を適用する。`all` は認可されたscopeの集合であり、全DBではない。共有会話にownerの個人scopeや他projectを持ち込まない。global preferenceを含め、remoteへ返せるデータ集合はgrantを越えない。global化は従来の `pref add` / viewerの明示操作だけで、会話内容やbotの推論では行わない。

§5.13のper-repo grantsを維持し、非Git対応を実装する時だけ明示的なlogical-scope grantとして拡張する。非Git grantがない間はアクセスを拒否し、repo ID偽装や無制限grantで代用しない。capture exclusion、redaction、egress、withdrawal、forget、retention、backupの既存規則を同じscopeへ適用する。scope変更や削除後もraw/sourceの識別と安全な非表示を維持し、別scopeへ自動コピーしない。

## 4. 権限と非同期状態

| 権限 | できること | 他の権限へ波及させない境界 |
| --- | --- | --- |
| read | grant内のsearch/get/timeline | append、correction、forget、global操作、device enrollmentを不可にする |
| append（将来local adapter等） | 認可source/scopeのraw受付 | ownerとして決定・訂正する権限やsync control opを含まない |
| correction | 認可されたowner操作を記録 | append/readから推定しない。botがownerを名乗っても不可 |
| forget | §6の確認・削除pipeline | correctionやappendに含めない。replica/backupへの影響を明示する |
| global preference | 既存の明示的owner channel | conversation ingest、remote search、scope設定から付与しない |

remote clientの初期権限は **read-only**。§5.13のOAuth、device-issued approval、per-repo grantsとreturn-time再検査を維持する。append等のremote書込APIは今回要求しない。**device-sync tokenをbotへ渡さない**。§5.12/§6.3のとおり、それは全replicaとbackupを消去できる信頼境界であり、OAuthのread grantとは別物である。

記録のdurable受付、非同期curation、context準備、runtimeへの実配送を別の状態として報告する。受付成功はcuration完了でも「AIが覚えた」でもない。receiptには同じretryを識別できるanchor/受付状態を対応させ、書込みに失敗したら成功を返さない。workerの遅延・停止・再試行は既存§2.5/§3/§4の経路を使い、リクエストのたびにproviderを同期実行しない。

将来adapterのshown/correction状態は、scopeと完全なsession identityに結び付ける。応答前の取得、queue入り、timeout、cancelだけではshown/consumedにしない。runtimeが対象応答へ渡したことを確認できる配送点で、実際に残った行だけを記録する。安定したdelivery identityで重複通知を冪等に扱い、不明な配送結果は未確認として報告する（exactly-once注入を保証しない）。再配送前に権限・tombstone・withdrawal・未適用owner変更を再検査し、未配送correctionをretryで失わない。配送確認点がないruntimeはcorrectionsをlive-verifiedにできない。既存adapterの配送点をこの将来契約で書き換えず、#340のテストと実機確認を継続する。

## 5. 後続実装の受け入れ条件

以下は仕様の受け入れ例であり、今回実行済みのテストではない。実装PRで失敗時も含むfixture/harnessと必要なlive証拠を添える。

| ID | 入力・失敗条件 | 合格条件 |
| --- | --- | --- |
| C1 identity collision | 二agent、二runtime instance、二accountが同じsession/conversation/messageラベルを使う | raw/sourceの帰属、manifest、検索、shownとcorrectionsが混ざらない。#321の既存agent回帰も維持 |
| C2 speaker provenance | botがowner/userを名乗る。assistantがtoolの「承認」を引用する | owner decided/globalへ昇格しない。verified owner acceptanceのみ既存gateを通り、根拠anchor削除で#167の状態へ戻る |
| C3 cross-scope | 個人・記事・repoのうち記事だけgrant。`all`、直接ID、timeline、context、corrections、途中のgrant取消を試す | 他scopeの内容・source本文・snippetを返さない。return-time検査で取消とwithdrawalを反映。未指定scopeもglobalへ漏れない |
| C4 retries | 同じmessageを並列再送、durable commit直後の応答喪失、再起動後再送、異なるpayloadで同じキー | raw/claim増殖なし、同じanchor/receiptへ収束。異なる内容は検出。revisionは明示的に区別 |
| C5 async delivery | worker停止、context未準備、queue、timeout、cancel、切詰め、重複配送通知 | durable受付とcuration/配送を区別。未配送correctionを消費しない。実配送行だけshown、通知retryは冪等。遅延中に削除された内容は再配送しない |
| C6 correction lifecycle | recurationでuid変更、acceptance削除、時計巻戻り、rebuild | #157/#167/#162の既存期待結果を維持。別source/sessionへcorrectionを送らない |
| C7 privileges | read-only OAuthでappend/correct/forget/pref/global化/device操作を要求。appendでcontrol opを送る | 全て拒否し副作用なし。device-sync credentialをclientへ露出しない。未grant非Git scopeは拒否 |
| C8 capability evidence | MCPだけ登録したclient、history import、mockで成功したhook | search/ingest/pre-response/correctionsを別表示。historyはunknown/search-only。mockをliveと表示せず、毎応答/全会話対応を宣言しない |

## 6. Live capture と host recall adapter

2026-10-02、ownerが追加の改善と常時HTTPサービスの方向を「それらも追加してほしい」と承認した。以下も実装前の仕様レビューであり、インストールや起動の承認・実施ではない。

比較したclaude-memは `039c6160f0ff26e9fab37cae7f50b994ba68f7ff`。[Grok Bot資料](https://github.com/thedotmack/claude-mem/blob/039c6160f0ff26e9fab37cae7f50b994ba68f7ff/docs/public/grok-bot/index.mdx)、[Installer](https://github.com/thedotmack/claude-mem/blob/039c6160f0ff26e9fab37cae7f50b994ba68f7ff/src/services/integrations/GrokBotInstaller.ts)、[IndexWriter](https://github.com/thedotmack/claude-mem/blob/039c6160f0ff26e9fab37cae7f50b994ba68f7ff/src/services/integrations/GrokBotIndexWriter.ts) に、JSONL watcher → worker/observer → MCP search と、hostが読むrolling INDEXへの経路がある。Grok Build CLIとは別で、allowlist付きawareness-push pilotもINDEXとは別である。これは比較元の確認であり、ownerのnoteMan実機の対応証拠ではない。

oboeteにも **captureとrecall deliveryを別々のadapter能力** として設計する。host hooksがない製品は、認可されたtranscript/APIの監視とhostが実際に再読込するcontext file/APIを候補にする。`agent-transcripts/*/*.jsonl` や `memory/log/zz-claude-mem-inject.md` は比較元の例に留め、noteManのパス・format・権限・再読込時点は実機で確認してから指定する。既存importをlive adapterと呼び替えない。

- watcherのcheckpointはsource identity、schema version、file generation/安定file identity、確定したbyte位置、message/revision IDに対応させ、**durable受付後だけ**進める。未完成JSONL行/分割UTF-8は持ち越し、改行で確定したbounded recordだけparseする。不正・過大行は隔離と理由を記録し、無限bufferや無言の取りこぼしを避ける。
- truncation、rotation、rename、copy、新規file、再起動、poll重複・通知喪失を区別し、§2の冪等キーで再走査を安全にする。inode/path/offsetだけをmessage identityにしない。host IDがない場合は、永続connector ID方式をrotation/replayまで実証できる時だけ採用し、できなければ未対応と報告する。revisionは明示し、古いrevisionのreplayで最新状態・forget deny-listを復活させない。
- 同じraw/worker/op logを使い、bot用の第2記憶engineを作らない。watch targetはownerが許可した範囲だけ。sourceのroleを自己申告のowner権限と混同せず、§2/§3のscope・provenanceを維持する。
- output file、DB、backup、export、curator/observerの内部記録をwatch inputから除外する。host transcriptに再掲載されたgenerated contextは識別して記録可能な引用と新規owner発言を分離し、memoryの再取込みで自分のclaimを増殖・owner昇格させない。識別できなければlive対応を未確認とする。
- rolling contextは認可scopeのdelivered claim/IDと根拠・日付・as-ofを、hostのbyte/token/行上限内で生成する。他projectの最新記憶で空きを埋めるcross-project fallbackを採用しない。current扱いしないimportやproposed itemを混ぜて上限を満たさない（#320は別判断）。
- fileを使う場合はownerが許可した専用managed fileだけを単一writerが更新し、同directoryのstagingとatomic replace等、そのOS/hostで確認した方法を使う。読み手が半書込みを見ないこと、並行refreshが新世代を古い内容へ戻さないこと、symlink/permission変更や手動編集競合時に安全に止まることを確認する。月別diaryへの無条件追記や任意パスへの書込みは要求しない。
- correction/forget/withdrawal時にmanaged contextを再生成・削除し、再読込後の実配送を確認する。file更新成功をshown/consumedにしない。hostが既に読んだcontextや外部保存コピーは消去保証の外側であり、§6.3に従い限界を説明する。古いrefresh taskはreturn/write時の再検査で削除済み内容を戻さない。
- setup/doctorはruntime/version、read/write target、話者保証、cursor状態、queue lag、最終生成・実配送の別時刻、未確認能力を秘密本文なしで表示する。外部modelを使うroundtripは、後続の隔離環境で明示的に許可されたtestのみ。

実装PRの受け入れ：分割行、UTF-8境界、重複event、commit後crash、rotation/copy/truncation、revision逆順、過大行、watcher停止・再開で欠落/重複を数える。二scope・二instanceを同じラベルで動かし、他scopeの内容とgenerated contextを再取込みしない。並行reader/writerとforget中のrefreshを競合させ、半file・旧内容復活を0にする。実hostで「test入力 → durable記録 → curation → scoped search → host context取得 → 次のtaskで利用 → correction → 再利用 → forget後に返らない」を確認し、read可能・配送済み・回答での実利用を別々に記録する。実利用が観測できなければ未確認として残し、harnessだけをlive-verifiedにしない。

## 7. 任意の常駐LOCAL HTTPサービス

ownerが希望した常時利用の選択肢として追加する。既定のon-demand workerとstdio MCPは維持し、常駐モードの設定を明示的に選ぶ。§1.8/§6.6/M5の従来の「常駐は測定結果でのみ採用」は、**任意のlocal HTTPモードを提供すること**に限り置き換える。default workerの選択と既存M5性能lineは変えない。

| 境界 | 将来サービスの契約 |
| --- | --- |
| 所有権 | 一つのoboete homeに一つのlifecycle owner。既存worker lockを使い、既存workerが動く場合は確認したhandoverか明示的busyで待つ。別curator/checkpointを並走させない。PIDだけで別processをkillせず、実体・home・lock ownershipを検証する |
| engine/DB | Rust/SQLiteと既存raw/derived/op logを共有。HTTPは既存操作の入口。SQLiteのlock/retry、migration、rebuild、update、backup、forgetの排他を迂回しない。hook/CLI/stdio MCPの独立利用も保持 |
| background | watcherとboundedなwake-up/scheduled retryを維持し、idleでもlistenerは選択中残る。durable queue/checkpointから再開し、polling/retry stormや無制限thread・jobを作らない。provider chain、費用上限、egress gateを共用する |
| API | versionを明示したAPI（例えば `/api/v1`。最終route/schemaは実装設計で固定）。authenticated health/status、scoped search/get/timeline、明示的append/correction grantによるingest/correctionを分ける。statusにdurable receipt、consumer lag、配送未確認、費用/資源の概況を区別し、秘密や他scopeの本文を含めない |
| 書込み | read credentialはingest/correction不可。appendは認可source/scopeのbounded schemaだけで任意SQL/path/shell/control opを拒否。correctionはverified owner channelだけ。forget/global設定は既存のowner確認・pipelineを使い、readやbot ingestから権限を推定しない |
| transport認証 | loopback（既定127.0.0.1、IPv6は明示設定・検証）にbind。全endpointを認証し、healthも例外にしない。Hostの明示allowlist、browser Origin検証、wildcard CORS禁止、tokenはheader/安全なowner保存。Originなしのlocal machine clientもauth/Host必須。browserのnull/foreign Originは拒否。device-sync tokenを再利用しない |
| lifecycle | graceful stopは新規受付を止め、受理済みrecord/queue/cursorをdurableにし、boundedなdrain後にlockを解放。crash/restartで重複worker・二重記録・cursor先行を起こさない。port使用中は自動公開bindや勝手なprocess停止で解決しない。設定変更は安全な保存と実効値/必要restartを表示 |
| 資源 | #53の接続・header/body/framing・期限の境界を再利用。検索/SQLite処理のdeadlineとHTTP受信/送信のdeadlineを分離。接続数・queue長・record size・response size・watcher scan量・retry頻度・provider同時実行・idle RSS/CPU/diskを有界にする。数値budgetは対象OSの同workload計測で固定し、未計測値を達成済みにしない |
| WebUI/OS | #94/#338でenable/disable、bind/port、最小grant、watch target/scope、health/lag、restart/recoveryと資源を表示。OS自動起動/service登録は各OSで内容と変更範囲をレビューした後の明示的install action。今回の仕様承認でlistener/startup/securityを変更しない |

既存viewerを同サービスへ統合するかは後続設計だが、認証・設定・DB ownershipを重複させず、別listenerの場合も同じownerとgrant/purge状態に従う。既存のviewerだけを「常駐memory service実装済み」と表示しない。

milestone 6のremote MCPはhub OAuth/read grants/cloud syncの別境界。local HTTPをpublicにbindして代用しない。常駐HTTPはcloud-only curationへの移行ではなく、none/free/local/subscription/paidの既存chainを使う。CMEM Proのhosted機能全体、無人cloud observerやmulti-tenant server productとの同等性を約束しない。

受け入れ：同homeでservice/worker/hookを同時起動し一つのconsumer ownerに収束。restart/kill/crash/port競合/migration/forget/update/restore時に失ったreceipt・先行cursor・旧削除内容の再公開を0にする。認証なし、foreign/null Origin、偽Host、readでappend/correct/forget/global、over-size、slow-client、飽和queueを拒否し、通常負荷へ復帰。stdio/on-demandをHTTP停止中も利用できる。Windows native/WSL/macOSでlifecycle、権限、資源budgetを計測し、service installの確認は別途明示的に実施する。

## 8. 検索状態の可視化とportable export

### MCPのlexical fallback理由

現行main `74c40c075b024e125d976175346bd5df31308688` の `src/mcp.rs:165–171` は `Answer.hits` の行だけ返し、`Answer.vector` を落とす。§4.10/A93の「full-text fallbackと理由を返す」をMCPでも満たす必要がある。これは新しい検索engineではなく既存契約の不足。最新main `6225150661472d2be9c54aa62802bcaab648f772` でも同じコードを再確認した。

受け入れ：Vector::Usedと全VectorSkip（off / excluded / no-vectors / building / waiting / timeout / error）をMCPの実tool出力に通し、検索方式とsafeな理由が **no hitsの場合にも**callerに分かる。metadataの表現は既存clientとの互換を保つ形で決め、本文・認証情報・providerの生errorを理由として返さない。hits/順位/ID/根拠・scopeを変えず、failure時もlexical検索が継続し、excludedでqueryが送信されないことを確認する。CLI/viewer/HTTPも同じ状態を表現する。

### 選択的portable export（低優先のplanned要件）

ownerが選んだscope・期間・kindのデータを、version/schemaとstable source/record/claim identity、provenance/raw evidence、revision、correction/supersession/deletion状態を伴って持ち出せるようにする。既存backup/rebuildやsyncの代替ではなく、#340の完了条件にしない。実装は後続に置き、release対象は後続owner判断とする。既存milestone gateへ自動追加しない。

previewは対象・件数・推定size・含む個人情報と削除状態を示し、redaction、capture/egress exclusion、grantを尊重する。secret/tokenや内部configを含めず、必要なevidenceを範囲外から黙って追加しない。範囲外evidenceは欠落と明示し、断片をcurrent decisionとして偽装しない。imported/historyをcurrentへ昇格しない。write先はownerが選択し、atomic completion/中断復旧と権限を確認する。

削除済みpayloadはexportしない。必要なtombstone/withdrawalや非内容metadataの扱いをversion contractに明示し、古いexportの再importでforget/correctionを消さない。既にownerが外へコピーしたexportは自動削除できないという§6.3の限界を表示する。後続にimportを作るなら同ID/revisionのretry冪等、scope対応の明示、deny-list優先、rebuild同等のprovenance保持が受け入れ条件。未知version/欠落根拠/途中fileは安全に拒否またはhistory-onlyで扱い、黙って権限やcurrent statusを復元しない。

## 9. 同workloadで比較し、優越を先に宣言しない

比較開始時に最新claude-mem SHA/versionとoboete SHA/configを固定する。今回のsource確認は039c6160で、実行benchmarkではない。既に[Japanese/CJK substring検索の回帰](https://github.com/thedotmack/claude-mem/blob/039c6160f0ff26e9fab37cae7f50b994ba68f7ff/tests/services/sqlite/search-cjk-fallback.test.ts) と[quota fallback](https://github.com/thedotmack/claude-mem/blob/039c6160f0ff26e9fab37cae7f50b994ba68f7ff/docs/public/usage/quota-fallback.mdx) がある。「日本語やfallbackがない」を差別化の根拠にしない。Rustという言語選択だけで省メモリ・高品質を主張しない。

§8.1/§8.2の凍結fixture/held-out split/既存lineを維持し、live bot/local serviceの比較は別の拡張tableにする。同じsource入力・scope・質問/task、同じ期間/稼働時間/失敗注入、provider/model/budgetとfallback条件を開示する。異なるproviderの比較はその差を明示し、言語/記憶engineの差と混同しない。実データや外部model使用は後続で明示的に許可された隔離runのみ。

| 観点 | 比較で記録する結果 |
| --- | --- |
| 品質 | retrieval、owner decision precision/recall、accepted案の保持（#255/#244）、false task promotion（#320）、cross-session mixing（#321）、correction/forget。日本語/英語と少量scopeを分ける |
| 実task handoff | 会話・runtime・端末を替えた次taskで何が取得・配送・実利用されたか。検索hitだけで引継ぎ成功としない。unsupported capabilityは未対応と記録 |
| latency | cold/warm/idle/負荷時のcapture receipt、curation、context生成/実配送、search、resumeのp50/p95と失敗/未測定率 |
| 総資源/費用 | raw/index/vector/queue/backup/log/exportを含むdiskと月間増分（#317）、全常駐processとmodelのidle/peak RSS・CPU、model/embedding/provider retry/fallbackを含む実費・subscription使用量 |
| recovery | partial/rotation/restart/provider quota/DB busy/port競合/forget/update/restoreからの回復時間、欠落・重複・stale配送件数 |

常駐modeはidle負担も計上する。速さのために安全性・品質・scopeを落としたrunを同等比較としない。既存milestoneの合格と拡張能力の合格を別に示し、CMEM Pro全hosted機能の同等性や未測定の勝利を主張しない。

## 10. 決定・追跡・実装順序

新規追跡は既存課題が覆わない次の4契約だけ。このPRを中心に実装前レビューを行う。

| 追跡 | 範囲 |
| --- | --- |
| [#343](https://github.com/ojungo69/oboete/issues/343) | MCP searchのAnswer.vector/fallback理由。既存§4.10/A93の不足 |
| [#344](https://github.com/ojungo69/oboete/issues/344) | live captureとscoped host recall adapter。M5後に検証 |
| [#345](https://github.com/ojungo69/oboete/issues/345) | 任意persistent LOCAL HTTP lifecycleとgrant別API。M5後に検証 |
| [#346](https://github.com/ojungo69/oboete/issues/346) | 選択的portable export。低優先planned |

- 既存：identity #321、false task/provenance #320/#167、accepted decision #255/#244、correction #157/#162、総resource #317、HTTP境界 #53、WebUI #94/#338、M5 forget、M6 remote read/sync。scopeを広げるissue本文変更や重複issueを作らない。
- 順序：既存M3/M4の品質・注入を進め、M5のforget/securityを満たしてからlive bot/local serviceを隔離dogfoodで実装・検証する。M6 remote/syncは既存順序。local serviceはremote必須依存ではなく、remote書込を前倒ししない。MCP fallbackは既存§4.10のfollow-up、exportは低優先。#340は作業中にmain `6225150661472d2be9c54aa62802bcaab648f772` へmergeされたが、どれもその既存範囲や後続作業の追加blockerにしない。既存adapterのtested/live-unverified区別を維持する。
- 残る判断：noteManの正確なruntime/API/version/path/権限と話者保証、stable revision/cursor、host配送確認点、API schema/token保管、viewerとのlistener統合、OS service方式、計測budgetとexport format。未確認製品に能力を付与しない。#320のowner判断は別途必要。
- 共有：ownerの依頼に従い、作業中Codex向けの中央handoffをPR342へ投稿する。直接のsupported message channel/target sessionを確認できない限りterminalへ入力・割込みをしない。GitHubへの「投稿済み」と相手の「確認済み」を別々に報告する。
