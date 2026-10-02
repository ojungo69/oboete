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

## 6. レビューで残す決定と追跡

製品採用、runtime/API/versionの確認、話者保証の検証方法、stable IDとrevisionのwire表現、scope作成・対応UI、配送確認点の具体化は後続の設計・実機検証で決める。未確認製品には能力を付与しない。#320のowner判断は別途必要。

identity修正は#321、provenance/claim保持は#320/#167、correctionは#157/#162、UIは#94/#338、remote readは§5.13とmilestone 6で追跡する。外部bot runtimeの実装を開始するとき、既存追跡で覆えない具体的契約だけをissue化する。現時点では重複する新規issueを作らず、このドラフトPRを仕様レビューの入口とする。
