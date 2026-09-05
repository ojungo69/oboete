# Phase 3 参照実装: event intake・冪等な状態還元・operation correlation

対象: `harness/continuity/reference-model.ts` / `harness/continuity/reference-model.test.ts` /
`harness/continuity/mutate.sh` / `harness/fixtures/continuity/**`。

正本: `specs/001-agent-memory-core/resume-continuity-addendum-v6.2.md` §3.1 / §4.2 / §4.3、
`agent-memory-final-spec-v6.md` §8.2（idempotency key 導出）/ §22.6（decimal string・JCS）。

この文書は「正本にそう書いてあるので実装した規則」と「正本に無いので harness 側で決めた導出」を
分けて記録する。後者は Rust 側が同じ値を出すために必要な情報であり、同時に正本へ戻すべき宿題でもある。

## 1. 正本どおり実装した規則

| 規則 | 正本 | 実装 |
|---|---|---|
| `evidenceKind` / `ingestAttestation` は intake が割り当て、caller の値を信頼しない | §3.1 | `stampIntakeEvidence` は caller の `ingestAttestation` を読まずに捨て、認証済み経路の受領証（`IntakeContextV1.attestation`）で置き換える |
| native は attestation・active capability hash 一致・`(scenarioId, captureMethod, channel)` が proven の 4 条件 | §3.1 | 1 つでも欠ければ `synthesized`。channel は受領証側の値を使う（caller の主張は使わない）。**authority 側の値が空なら一致は成立させない**（`activeCapabilityHash` / `expectedSourceAgent` / `exactAgentVersion`）: capability matrix が未整備の daemon で caller が同じ空値を名乗ると native が成立してしまう。「未設定」の表し方は空文字とは限らないので、identity 材料と同じ `isBlank`（空白・タブ・U+200B・U+FEFF まで）で落とす |
| 受領証は「その認証済み取り込みの receipt」であって、在ることだけでは認証の証拠にならない | §3.1 | `ingestReceiptId` / `peerIdentityId` が空白だけの受領証は `authenticatedPeer` を成立させず、その上に乗る `authenticatedVersion` も成立しない（**経路の認証**と **version の authority** は述語を分けてある。前者は `turn_identity_unauthenticated` の診断も直接ゲートする）。認証できない経路を `undefined` ではなく**欄が空の受領証**で表す daemon では、存在だけを見ると誰も名乗っていない受領証が native authority の根拠になる。§3.1 は kind を「認証済み peer identity」から導けと言うので、peer を指していない受領証は根拠にならない。`channel` は閉じた union（`rpc` / `spool`）なので空白形が存在せず、検査を足していない。**検査を足す欄と足さない欄は schema で決まる**: 凍結 schema で `ingestReceiptId` / `peerIdentityId` / `scenarioId` は `type: string` + `maxLength` だけ = 空白が schema 妥当なので検査が要り、`channel` / `captureMethod` は enum なので閉じている（下記「還元器は event の schema 妥当性を前提にする」と同じ原則）。理由は欄ごとに違う: `channel` は authority 対 authority で caller が触れず、`captureMethod` は caller 側の値だが enum で閉じている |
| `IsoTimestamp` の暦検査は**書く層にも置く** | §22.6 | `attestedAt` の値は caller 由来ではない。intake は caller の受領証を destructure で捨て、daemon 自身の `context.attestation` を刻印する。読む層（`assertIdentityMaterial`）にしか検査が無いと、**受領証の時刻を 1 つ間違えた daemon は全 event を落としながら、エラーは受領証ではなく event を名指しする**（実測: `2026-02-30T00:00:00Z` の受領証は intake を診断ゼロで通り、直後に 3 つの入口すべてが `§22.6 違反: provenance.ingestAttestation.attestedAt が暦として実在しない` を投げた）。intake は台帳より前なので、throw しても配送鍵は消費されない。**ただし空白は暦違反にしない**（外部のコードレビューが指摘し、実測で再現した）: この関数自身が「認証できない経路を `undefined` ではなく欄が空の受領証で表す daemon」を前提にしているので、空白を落とすと**その daemon の event が 100% 消え、しかもエラーは認証の欠落ではなく timestamp を名指しする**。空白は「時刻を名乗っていない」であって暦違反ではないので、`declared()` で不在として読み、`authenticatedVersion` が降格で扱う——ただし通すようにした以上、**時刻を名乗らない受領証を authority にしない**条件（`!isBlank(attestation.attestedAt)`）を対で足す必要があり、足さないと緩めた側から native authority が漏れる。読む層も同じ判断にする（intake で降格した event はその受領証を持ったまま還元器へ来るので、ここで落とすと降格が意味を失う）。必須欄の `occurredAt` は逆で、空白は綴り違反として落とす。締めすぎていないことも同じ test で見る。**negative fixture の節導出も直した**: `intake-reject` の分岐だけ `/§3.1 違反/` を固定で書いていたので、§22.6 を名乗る intake の case が原理的に書けず、書く層の検査が parity の被覆から外れていた（`runtime` の分岐と同じく `reason` から節を取る形へ統一し、`daemon-receipt-attested-at-not-real` を追加） |
| `provenance` 不在は節を名乗って落とす | §3.1 | 暦検査のループが `event.provenance.ingestAttestation?.attestedAt` を読むので、optional chain は `ingestAttestation` からしか効かない。`provenance` を持たない event は**節を名乗らない `TypeError`** になり、`rejected-events.json` が case ごとに 1 つの節を宣言して TS/Rust パリティの基準にしている分類契約から外れる（外部のコードレビューが指摘し、実測で確認した）。`?.` で黙って通すのは §3.1「Every adapter MUST populate `provenance`」と逆向きなので、`assertIdentityMaterial` の入口で §3.1 として落とす。**intake にも同じガードを置く**: `stampIntakeEvidence` は 1 行目で `event.provenance` を素で destructure するので、生の adapter 出力に最初に触る層が節を名乗らない `TypeError` で死んでいた（還元器側のガードは intake を通らない caller にしか効かない）。`attestedAt` の暦検査を「書く層にも置く」と決めたのと同じ形。**暦検査自体は helper（`assertRealInstant`）に寄せた**: 2 層で同じ規則を手書きすると 2 通りの文面が 1 つの規則を主張することになり、Rust 側はどちらに合わせても片方と食い違う。比較は型を外して行う（型の上では常に定義済みなので、素の比較は「不要な条件」として静的解析に落ちる） |
| `scenarioId` は proven な scenario を **naming** していなければならない | §3.1 | 空白だけの `scenarioId` は proven の根拠にならない。matrix 側にも空白 entry を持つ daemon で caller が同じ空白を名乗ると等値で proven が成立する。caller 側を非空白に固定すれば matrix 側の空白 entry とは等値にならないので、検査は片側で足りる。`captureMethod` は閉じた union なので同様に検査不要 |
| exact version でない `sourceAgentVersion` は native authority を失う | §3.1 | `IntakeContextV1.exactAgentVersion` との一致を要求 |
| kind は「認証済み peer identity・channel・captureMethod・capability matrix」から導く | §3.1 | **`event.sourceAgent` が受領証の Agent（`expectedSourceAgent`）と食い違う event は、降格ではなく intake が受け取らない**（throw）。§3.1 は検査に落ちた event を `synthesized` へ降格せよと言うが、それは**証跡の質**の規定で、`sourceAgent` は質ではなく **scope selector** として使われる: `assertSameScope` が「どの状態を書き換えてよいか」をこの値の等値だけで決める。降格しても値そのものは残るので、外部のセキュリティレビューが指摘したとおり、peer=codex として認証された event が `sourceAgent: "claude"` を名乗ると `synthesized` の札を貼られたまま **claude の operation を診断ゼロで `succeeded` にできた**（実測）。correlation は `evidenceKind` を一度も読まないので、札は何も束縛しない——**wire が運ぶ値を authority にしない**という同じ原則の適用漏れだった。空白の `sourceAgent` を還元器が落とすのと同じ形で、他人の名前はそれより悪い（空白は「誰も名乗っていない」、他人の名前は「scope が矛盾する」）。認証できない経路（`expectedSourceAgent` が空 = 受領証が peer を名乗っていない）は「違う」と言える相手が居ないので従来どおり降格で扱い、締めすぎていないことも test と変異の両方向で固定した。intake は台帳より前なので throw しても配送鍵は消費されず、訂正版の再送はそのまま効く。**この棄却形は fixture 側にも持たせた**: `rejected-events.json` の `rejectedBy` に `intake-reject` を足し、降格（`intake`）と区別する。区別しないと、降格しか実装しない移植でも negative fixture が緑になり、TS/Rust parity の基準がこの規則を守らせない。この結果 `authenticatedVersion` の中の `event.sourceAgent === context.expectedSourceAgent` は到達時点で必ず true になるため削除した（証明済みに死んだ条件を authority 述語に残すと、検査が行われているように読める）。**境界**: intake を経由せず `reduceTaskWorkState` を直接呼ぶ経路では、状態と一致する `sourceAgent` を詐称した event を止められない。参照模型は認証済み peer を知らないので、そこは daemon の trust boundary であり、還元器側に検査を二重化はしない |
| intake は `sessionId` も束縛する | §3.1 | `IntakeContextV1.expectedSessionId` を足し、event が違う session を名乗ったら**降格ではなく受け取らない**（`sourceAgent` と同じ理由: session は証跡の質ではなく scope selector で、§4.3 の候補選びと放棄がこの値で絞る。降格しても値は残るので他人の operation を選べる）。状態が持つ session は `PendingOperation.correlation.sessionId` だけで、それは**以前の event の自称をそのまま写した値**なので照合の基準にならない——start を偽った相手が比較対象も書いている。event の外を見られるのは intake だけなので、束縛できる層は intake しか無い。session を名乗れない経路（`expectedSessionId` が空白）は従来どおり素通しで、`sourceAgent` と違い evidence の質は落とさない。intake は台帳より前なので throw しても配送鍵は消費されない。negative fixture は `foreign-session` |
| `turnIdSource="native"` は exact version について proven な native turn identifier を要求する | §3.1 | `IntakeContextV1.nativeTurnIdentityProven` が false なら caller の native 主張を `unavailable` へ降格し `turnId` を落とす。証明は version に紐づくので、受領証・`sourceAgent`・`sourceAgentVersion` の束縛（`authenticatedVersion`）が成り立たない event にも適用しない。`capabilityHash` は capability matrix にまだ turn identity の cell が無い（#40）ため turn の判定には使わない。`synthesized_monotonic` は adapter 由来なので触らない。**この帰結として `activeCapabilityHash` または `scenarioId` だけが空白の場合、`evidenceKind` は `synthesized` に落ちるが `turnIdSource` は `native` のまま残る**（降格は `authenticatedVersion` だけに依存する）。turn identity の cell が matrix に入る（#40）まではこの非対称が正しい振る舞いで、回帰ではない |
| どちらかの turn が unavailable なら rule 2 は適用されず operation は `unknown` になる | §4.3 | 同じ match key の open な候補を `unresolved`（候補の参照）として返し、還元側で `unknown` にする。閉じられるのは rule 1 だけ。**turn 種別（`turnIdSource`）の一致は候補の絞り込みで見る**: 種別は start 側の材料（`PendingOperation.startTurnIdSource`・#35）にしかないので、以前は候補を 1 件に絞ってから最後に比べていた。それだと「同じ matchKey・同じ turnId で種別だけ違う」候補が 2 件並んだとき、種別で 1 件に決まるはずのものが `terminal_ambiguous` になって**両方 `unknown` に倒れ、配送鍵も消費される**。絞り込み時に見れば rule 2 の「exactly one open candidate」が成立して閉じられる。ただし**材料がある候補だけ**種別で絞る（復元直後は `startTurnIdSource` を持たない要素がありうるので、材料が無いことを「種別が違う」と読むと理由を取り違える。§3.1 は降格の理由を doctor が報告することを求めている）。候補ゼロのときの診断も「turn 同一性が無い」と「種別が違う」で書き分け、`unknown` に倒す相手は種別違いならその候補だけにする。**open / 確定済みの切り分けもこの絞り込みの後で行う**: 先に切り分けると、turn が両立しない open な兄弟が「open が居る」と数えられて確定済み経路が飛ばされ、確定済み候補への健全な再配送が `terminal_unmatched` に化けて、その兄弟を `unknown` に倒し台帳まで消費する（兄弟はこの terminal では閉じえないので巻き込む理由が無い）。ただしその結果、確定済み経路には turn が両立しない **open な**候補が残りうるので、成否の矛盾判定は**確定済みの候補だけ**を見る（`started` は成否を主張していないので、それを矛盾として隔離すると健全な terminal が台帳に入らず無限再送になる）。**§4.3 の順序要件も同じく絞り込みで見る**。start より後の terminal だけがその operation を閉じられるという要件は、これまで候補を 1 件に決めたあとの検査だった。それだと start が 10 と 20 の候補が並ぶとき、`ingestSeq` 15 の terminal は「10 の側しか閉じえない」のに両方が候補として数えられ、`terminal_ambiguous` で**両方 `unknown` に倒れて台帳まで消費される**（実測。隔離と違って訂正版が重複 no-op になるので隔離より悪い）。`turnIdSource` と同じく材料がある候補だけを対象にし、rule 1 を名乗った terminal は絞らない。**全件が順序不適合なら絞らない**——そこで空にすると、候補 1 件が順序違反というだけの場合に `terminal_out_of_order`（何が起きたかを名指しする診断）が `terminal_unmatched` に化ける。通す側も test で固定した。**同じ絞り込みは、確定済みの候補で再配送を説明するときにも要る**（open な候補を選ぶときだけではない）: これは「この terminal が閉じえた候補」の定義なので、素の候補集合で説明を許すと、`native` の failed と `synthesized_monotonic` の succeeded が同じ matchKey・同じ turnId で並ぶとき、succeeded を名乗る `native` の 2 通目が**兄弟に説明されて** `terminal_already_applied` になる。隔離（台帳を消費しない）を回避して台帳を消費するので、後から届く訂正版が重複 no-op として捨てられる。絞り込みは `sameTurnOf` / `eligibleOf` の 2 つに切り出して、確定済みの説明・open な候補選びの両方で同じものを使う（rule 1 を名乗った terminal は turn 両立を要求しないので、どちらも `byNativeId` があれば素通しする）。**ただし絞るのは「説明がつくか」だけで、「成否が矛盾しているか」は絞らない**: この分岐は性質の違う 2 つの問いを続けて解いていて、(i)「この terminal は候補の再配送として説明がつくか」は閉じえた候補だけが説明役になれるので turn 両立が要るが、(ii)「確定済みの候補と成否が矛盾していないか」は**閉じる権限ではなく壊れた証跡かの判定**なので turn 両立は要らない。matchKey・kind・input hash まで同じ terminal が確定済みの status と逆を主張しているなら、turn の導出が §4.3 どおりでない adapter だとしても矛盾は矛盾。(ii) まで絞ると候補が全部落ちた場合に `find` が undefined を返し、隔離（台帳を消費しない）が `terminal_already_applied`（台帳を消費する）に化けて訂正版が重複 no-op になる。**判定の順序は原則として隔離が先**（台帳を消費しない分岐を、消費する分岐より先に置く）。**ただし「記録できる open な候補が居るか」がそれより優先する**。§4.3:368 は「zero か複数の open にマッチした terminal は何も閉じず、unmatched な証跡として保存し candidates を `unknown` にする」と終状態まで名指ししているので、記録できる候補が 1 件でも居るなら台帳を消費してでもそちらに従う。この優先順位は最初から在ったものではなく、隔離ゲートの優先度を `terminal_already_applied` に対してだけ決めていたところへ、open / 確定済みの切り分けを turn 絞り込みの後ろへ移して `open.length === 0` の到達範囲を広げたときに、`terminal_unmatched` に対して決め直していなかった穴を塞いだもの（**門を足すと隣に順序依存が生まれる**の実例で、外部レビューが実測で指摘した）。塞がないと「確定済みの兄弟が居て、turn が両立しない open な候補も居る」形が隔離に落ち、open な候補は `started` のまま残って状態が嘘をつく。しかも `turnIdSource` の食い違いは adapter の捕捉経路という**定常的な性質**なので「訂正版」が存在せず、還元器は純関数なので再送は毎回同じ隔離になる——`started` を矛盾集合から外した理由（無限再送）と同型の失敗が、確定済みの兄弟経由で残っていた。逆に、その terminal が**閉じえた**確定済み候補（turn 両立する候補）と成否が矛盾している場合は、記録できる open が居ても隔離のままにする（訂正版が存在しうる形なので、台帳を消費しないほうが回復に効く）。矛盾もしておらず記録できる候補も居ない場合だけ `unresolved` が空の `terminal_unmatched` になる。**照合不能（`terminal_identity_unverifiable`）で `unknown` に倒す相手も turn 両立する候補だけ**にする: 照合不能は「どの候補を指すか決められない」であって、rule 2 で閉じえない候補まで巻き込んでよい話ではない。rule 1 では `sameTurnOf` / `eligibleOf` が素通しなので `settled` が空になるのは rule 2 だけ |
| `turnIdSource` は凍結 schema の語彙（`native` / `synthesized_monotonic` / `unavailable`）だけを受ける | §3.1 | `assertTurnIdentity` の先頭で `TURN_ID_SOURCES` と突き合わせて落とす（語彙は手で並べず schema 側の定数から引く）。参照模型は event が schema 検証を通ってから届くとは限らない（intake も還元器も生の値を読む）ので、**語彙外の綴りは `unavailable` の分岐にも intake の `native` 証明要求にも当たらず、降格を丸ごと迂回して自称 `turnId` を保持できた**（実測: `"Native"` / `"NATIVE"` / 末尾空白の `"native "` / キリル а の同形異字 / `"bogus"` がいずれも診断ゼロで通り、`turnId` は `"turn-FORGED"` のまま残った。外部のコードレビューが指摘した）。そのまま §4.3 rule 2 に入ると `sameTurnOf` は `turnId` の等値、`eligibleOf` は記録と event の**自己一致**で通るので、捏造した turn 同一性で turn scope がまるごと成立する。空白の identity 材料に実行時ガードを置きながら、identity 上いちばん効くこの欄だけ素通しだった。還元器・correlate・放棄の 3 入口すべてで落ちることを test で固定した |
| turn scoping を要求する規則は unavailable に fail closed になり、downgrade の理由は doctor が報告する | §3.1 | intake の降格は `turn_identity_downgraded` 診断を返す。`stampIntakeEvidence` の戻り値は `{ event, diagnostics }` |
| `turnId` は native / synthesized_monotonic のとき必須、unavailable のとき不在 | §3.1 | `assertTurnIdentity`（schema 側にも if/then があり二重に守る） |
| operation event は `operation` envelope 必須。correlation 値を `payload` から読まない | §3.1 | `assertOperationEnvelope`。correlation 関数は `payload` を参照しない。公開している `correlateTerminalEvent` も入口で同じ検査を行う（還元器を経由しない呼び出しで飛ばすと、既知の terminal kind が envelope 無しで届いたとき §3.1 違反が「照合できなかっただけ」の `terminal_unmatched` に化けて、壊れた adapter の証跡がそのまま保存される）。同じ理由で `assertSameScope` も入口で行う: 候補の絞り込みは session と lineage しか見ず、状態は Agent を 1 つしか持たないので、ここで比べないと別 Agent の terminal が「権威ある一致」として返り、consumer がそれを適用する。**§22.6 の `ingestSeq` decimal string 制約も同じ理由で入口に置く**: `compareIngestSeq` は start を選んだ後の順序比較でしか走らないので、候補ゼロ・適用済み・曖昧・照合不能で早期 return する経路では検査されず、還元器が入口で落とす入力を直接呼びだけが `terminal_orphaned` として返していた。同じ突き合わせで `assertIdentityMaterial` の欠落も見つけた: `assertSameScope` は lineage と Agent しか束縛せず、候補の絞り込みは `sessionId` の等値だけを見るので、**空白の `sessionId` を持つ terminal が、同じく空白の `sessionId` を持つ pending（復元した checkpoint や別実装が書いた状態。凍結 schema に minLength は無い）を診断ゼロで閉じられた**。空白同士は「同じ session」ではなく「どちらも名乗っていない」。**同じことが `sourceAgent` にもある**: `assertSameScope` は `event.sourceAgent === state.sourceAgent` の等値しか見ず、凍結 schema は event 側にも状態側にも `maxLength` しか課さないので、Agent 同一性を「不明」として空白で表す adapter が 2 つあると互いの状態を同じ scope として書き換えられる（intake の降格は `evidenceKind` を落とすだけで scope は縛らないので、ここで落とさないと誰も落とさない）。`assertIdentityMaterial` で `canonicalFingerprint` / `eventId` / `sessionId` / `sourceAgent` の 4 欄を落とす。**`occurredAt` も同じ関数で落とす**（#27・§22.6）: 凍結 `IsoTimestamp` の綴りに合わない値と、綴りは合うが暦として実在しない値の 2 段で拒否する。綴りを先に当てないと `slice(0, 19)` の後ろが見られず、数値 offset・offset 無し・末尾ゴミ・切り詰めた値が通って**還元器が凍結 schema に適合しない状態を出す**。公開入口で守る不変条件は envelope・turn 同一性・identity 材料（`occurredAt` を含む）・`ingestSeq`・scope の 5 つになり、還元器および `finalizeAbandonedState` と同じ集合になった。**この「公開 API と還元器で守る不変条件の集合がずれる」形は round 12・14 と合わせて 3 回出ているので、export を増やすときは還元器入口の集合と突き合わせること**。**`correlateTerminalEvent` の引数型も `IntakeStampedEventV1` に揃えた**。当初は素の `NormalizedContinuityEvent` を受けていて、evidence にも「これは意図どおりで、correlate は `provenance` を一度も読まない = authority label を消費しないので、intake を経由しない event を受けても authority 判定の迂回にはならない」と書いていたが、**これは誤りだったので撤回する**（外部のセキュリティレビューが指摘し、実測で確認した）。correlate は `provenance` こそ読まないが、rule 2 の候補選びで `turnIdSource` を、`assertSameScope` で `sourceAgent` を見ており、**どちらも intake が認証結果に応じて書き換える欄**。つまり「読んでいる authority label が別の名前で存在していた」ので、intake を飛ばせば証明の無い native turn 主張がそのまま照合権限になる（実測: 未証明 native の rule 2 terminal は直接呼びだと `matched` になり、`stampIntakeEvidence` を通すと `unavailable` へ降格して `terminal_unmatched` になる）。型で intake の通過を要求すれば、還元器・放棄・公開入口の 3 つで前提が揃う。**この境界は実行時 test では固定できない**（test helper が `IntakeStampedEventV1` へキャストして返すので、引数型を戻しても実行結果は変わらない——外部レビューの advisory）。素の `NormalizedContinuityEvent` を渡す到達しない呼び出しに `@ts-expect-error` を置いて tsc で固定した。引数型が広がると「未使用の抑止」で tsc が落ちるので、締めた側と緩めた側の両方向で発火する（実測で確認）。**判断を誤った理由**は、authority を `provenance`（label の置き場）だけで探して、label が**書き換える対象の欄**を探さなかったこと |
| dedupe authority は `adapterDeliveryId`、無ければ canonical fingerprint | v6 §8.2 | `idempotencyKeyOf` は fallback（union ではない。正本の導出式が `??` で書かれている）。schema が `adapterDeliveryId` に minLength を持たないので、空白だけの値は「無い」として fingerprint へ落とす（`isBlank` なので空文字だけでなく空白・タブ・U+200B・U+FEFF も含む） |
| dedupe は revision 採番の**前** | §4.2 | `reduceTaskWorkState` は ledger 照合を最初に行い、重複なら何も採番しない |
| 重複した論理 event は no-op（同じ state bytes・content hash・revision・history） | §4.2 | 重複経路は入力の snapshot をそのまま返す。ledger も同一参照 |
| 遅れて届いた event も後続 revision を作り、証跡を書き換えない | §4.2 | 適用は常に新しい revision を作る。既存 `sourceEventIds` は追記のみ |
| terminal 照合は 1) `nativeOperationId` 一致 2) `operationMatchKey` + turn/kind 一致かつ open な候補が 1 件 3) それ以外は不一致 | §4.3 | `correlateTerminalEvent`。`nativeOperationId` を名乗った terminal は rule 1 だけで判定する（一致しないときに rule 2 へ落とすと、matchKey の導出が §4.3 どおりでない adapter 相手に別 operation を診断なしで閉じてしまう。wire 越しに導出は検証できない） |
| terminal は start より後（権威順序）・未適用・payload/source hash 非衝突 | §4.3 | ingestSeq 比較 / status 判定 / `canonicalInputHash` 比較。**「未適用」と「非衝突」は配送 ID が違う 2 通目で同時に問題になる**: dedupe は内容を比べられず、identity 衝突検査は kind と input hash しか見ないので、成否だけが逆の terminal が「適用済み」として黙って通っていた。受理済み terminal の指紋は `PendingOperation.terminalFingerprint` として状態に載せた（#44）ので、指紋の食い違いはその欄で直接見る。ただし欄は任意で、この版より前に書かれた状態には無い——そこでも確定した status は持っているので、**成否の矛盾は `terminal_conflict` で隔離する**（どちらかが `unknown` = 成否を主張していない場合は矛盾ではない）。rule 2 の候補は同じ matchKey の兄弟をまとめて拾う（同じ turn で同じ tool を同じ入力で 2 回動かした場合など）ので、矛盾の判定は候補集合全体に対して行う: **成否が一致する候補が 1 件でもあれば再配送として説明がつく**ので隔離しない。兄弟の成否だけを見て隔離すると、健全な再配送が台帳に入らないまま無限に再送される。source hash（`canonicalFingerprint`）の衝突は correlation より前の dedupe で見る。冪等台帳が eventId だけを持つと、同じ配送 ID で内容が違う event が `duplicate` として捨てられて衝突検査が到達不能になるので、台帳は適用時の source hash も保持する（`LedgerEntryV1`）。衝突は `delivery_conflict` で隔離 |
| 台帳の鍵は `ledgerKeyOf`（`d:` / `f:` で keyspace を分けたもの）で、caller にも公開する | v6 §8.2 | `IdempotencyLedger` は caller が渡して caller に返るので、構築・永続化・復元は caller の責務。それなのに公開していたのは接頭辞の無い `idempotencyKeyOf` だけだった（実測: 還元器が引くのは `"d:delivery-start"`、公開関数が返すのは `"delivery-start"`）。公開関数で台帳を組み立て直した daemon は全 entry で還元器と食い違い、重複判定が一度も発火しないまま再配送が新規 event として適用される。`d:` / `f:` の分割は wire にも hash にも出ない内部詳細なので、caller が自力で再現しようと思う類の知識ではない（外部のコードレビューが指摘）。両方の鍵が違うこと自体も test で固定した |
| 状態を変えた経路はすべて原因 event を operation に残す | §3.1 | `withSourceEvent`。照合できた terminal・`unknown` に倒した候補・放棄はどれも呼んでいたのに、**再配送 start の経路だけ呼んでいなかった**（外部のコードレビューが指摘）。この経路は配送鍵を消費して revision も進め、さらに記録に欠けている識別材料を埋めるので、呼ばないと**状態が変わった理由が状態からも辿れない**（`history` は `CanonicalWorkStateV1` の欄ではないので永続的な provenance にならない）。`withSourceEvent` が早期 return する条件は **2 つあり、意味が違う**: (a) 既に記録済み（同じ eventId の再配送＝台帳だけ失った復元。何も失っていない）、(b) `sourceEventIds` が上限 256 件（**記録できなかった**）。増えるのは eventId が変わる本来の再配送契約で、かつ上限に達していないときだけ。**再配送 start の経路でも (b) は起こる**ので、この経路は `duplicate_operation_start` に加えて `source_events_truncated` を出す（片方だけだと「記録できなかった」ことが状態にも診断にも残らない）。**同じ「片方だけ揃っていない」は `source_events_truncated` の判定にもあった**: `withSourceEvent` は「既に記録済みなら何もしない」を先に見るのに、診断側は配列長しか見ていなかったので、**何も失っていないのに truncation を報告する**（診断文は「この event を記録できない」と event について断言する形なので、本当に記録できなかった場合と区別がつかない）。両方に同じ `includes` を置いた |
| 隔離するのは「状態に記録できる相手が居ない」場合だけ | §4.3・§3.1 | 判定は**診断コードの列挙ではなく結果**から導く: `unresolved` が空なら、その commit は候補を 1 件も `unknown` にしないので、**operation の側には**何も残らない。ただし記録先は #43 で state に出来た: 候補を 1 件も retain できなかった terminal は `droppedEvidence` に `orphaned_terminal` として記録し、その revision は**隔離のまま**進める（`quarantineWithRecord`。配送鍵は消費しないので、後から start が届いてからの再配送で閉じられる）。したがって現在の分岐は「記録して隔離する」と「event 自身の corruption なので記録もしない隔離」の 2 つで、以前あった「記録先が無いので鍵を焼くかどうかしか違わない」という状況は残っていない。当初はコード名で `terminal_conflict` / `terminal_orphaned` を並べていたが、**同じ状態に別のコードで到達する経路が漏れていた**（外部のコードレビューが指摘し、実測で確認した: 確定済みで同じ matchKey の兄弟が 1 件居るだけで、start より先に届いた terminal が `terminal_orphaned`（隔離・鍵を残す）ではなく `terminal_unmatched`（commit・鍵を消費）に落ちた。start より先に terminal が届くのは正常運用——hook と transcript scan の取り込み順、再起動後の catch-up——なので、鍵を焼くと後から start が届いてからの再配送が重複 no-op になり operation は永久に `started`。この失敗はこの分岐のコメントが `terminal_orphaned` について既に書いていたもので、**規則を名前で並べたせいで規則自身の対象が漏れた**）。例外は `terminal_already_applied` で、これは「既に閉じた operation の再配送」= 本当に重複なので鍵を消費する（隔離すると adapter が無限に再送する）。両方向を test と変異で固定した |
| terminal は照合された 1 件だけを閉じる | §4.3 | 適用は `operationId` の等値ではなく**照合結果の参照**で当てる。`operationId` は `eventId` + matchKey からの導出なので還元器は重複を作らないが、凍結 schema は `maxLength` しか課さず一意性も要求しないため、復元した checkpoint や別実装が書いた状態では schema 妥当なまま重複しうる。等値で当てると terminal 1 通が複数の operation を**診断ゼロで**閉じる（空文字の重複でも非空の重複でも同じ）。候補の絞り込みは `.filter` だけなので照合結果は状態の配列の要素そのもので、参照で当てれば重複があっても 1 件に限定できる。**同じ当て方は放棄経路にもある**: `finalizeAbandonedState` は「自 session の operation だけを `unknown` にする」ために session で絞ってから id の集合を作っていたので、id が重複していると絞り込みが無意味になり、**旧 session の `session_ended` が resume 先の live な operation まで `unknown` にする**（このコード自身のコメントが防ごうとしている事象そのもの）。共有している当て方ごと直し、`sourceEventsFull` も対象を id ではなく参照で受け取る。**照合結果が返す「閉じられなかった候補」も id ではなく参照で返す**（`unresolved: readonly PendingOperation[]`）。id で当てると、状態側で id が重複しているとき**候補ですらない operation**——別 session のもの——まで `unknown` になる。§4.3 が集合単位で指示しているのは「candidates を `unknown` のままにする」であって、候補の外へ広げてよいとは言っていない。**巻き込みが可逆でないことも記録しておく**: `unknown` に倒された候補は status としては回復できる（`open` は `started` だけでなく `unknown` も拾うので、rule 1 だけでなく rule 2 の terminal でも閉じられる）が、`sourceEventIds` は append-only なので、巻き込んだ terminal の `eventId` は残る。だから「回復できるから広くてよい」ではなく、倒す相手は候補に限る。**`nativeOperationId` にも同じことが起きる**: 凍結 schema は native id にも一意性を課さないので、復元した checkpoint には同じ native id の確定済みと live が並びうる。§4.3 の rule 1 は「exact `nativeOperationId` + 同じ session/lineage」で operation を**一意に**指す規則なので、2 件当たった時点で指せていない。件数を見ずに open だけで選ぶと**確定済み operation 宛ての再配送が live な兄弟を閉じる**（実測）。rule 1 を名乗った terminal については `compatible.length > 1` を `terminal_ambiguous` にして、候補は `unknown` までに留める。**数えるのは `byNativeId` ではなく `compatible`**——最初は絞り込み前の集合を数えていたが、それだと native id は同じでも input hash や kind が違う（= 既に候補から外れている）兄弟まで数に入り、健全な terminal が `terminal_ambiguous` で `unknown` に倒れて台帳まで消費される（隔離と違って訂正版が重複 no-op になるので隔離より悪い）。門を足すときに母数だけ他の判断と違うものを使っていた（この関数の他の判断はすべて `compatible` を見る）。通す側の 3 形（hash 違い・kind 違い・native id 違い）を test と変異で固定した（全件が確定済みなら手前の `open.length === 0` で `terminal_already_applied` に落ちるので、再配送の扱いは変わらない——通す側も test で固定した）。**選ぶ側の非対称も残っていた**: 同じ native id の兄弟から互換な候補を優先する修正を入れたとき、**derived `operationId` の枝は先頭 1 件のまま**だったので、`operationId` が衝突する状態では配列順で結果が割れた（実測: 衝突する兄弟が先頭なら `start_conflict`、互換な兄弟が先頭なら `duplicate_operation_start`。隔離は鍵を消費せず還元器は純関数なので、前者は同じ再配送が永久に収束しない）。選び方を 1 箇所に括り出して両方の枝が同じものを使う形にした——**片方の枝を直したらもう片方の枝が残る**のも同じ「軸を変えて確かめる」の対象。**互換な兄弟が複数居るときの選び方にも同じ非対称が残っていた**（外部レビューが指摘し、実測で確認した）: derived id で当たった兄弟が空白の `nativeOperationId` を持ち、別の兄弟が届いた native id を既に名乗っている状態では、連結順のせいで空白の側が選ばれる。下の復元がそこへ native id を書くので、**同じ native id の pending が 2 件**になり、rule 1 は候補 2 件を曖昧と見て後続の terminal がどちらも閉じられない（空白を「主張していない」と読む修正そのものが作った経路——欄を埋める修正は、埋めた値が一意性を要求される欄なら**重複を作りうる**）。互換な候補のうち **native id を既に名乗っている側**を優先する形を一度書いたが、**それでは塞がらないので撤回し、埋める側で塞いだ**（外部のコードレビューが 2 ラウンド続けて指摘し、どちらも実測で再現した）。優先は `compatible` の中しか見られない。(1) 届いた start が native ID を持たないとき、優先の根拠（互換なら値が一致している）は成立しない——`startConflictsWith` の native ID 比較は**両方が declared のときだけ**走るので一致を一度も確かめていない。(2) より重いのは、**名乗り手が非互換なとき優先が空振りする**こと: `operationMatchKey` だけ違う `op-native-claimer` が `toolu_1` を名乗る状態に `toolu_1` の start を再配送すると、互換な derived id 兄弟が選ばれてそこへ `toolu_1` が書かれ、**同じ native ID の pending が 2 件**になる。後続の terminal は rule 1 で候補 2 件 = `terminal_ambiguous` になり、どちらも `unknown` のまま閉じられないのに配送鍵だけ消費される。塞ぎ方は**埋める直前に「その native ID を名乗る兄弟が他に居るか」を見る**（`nativeIdTaken`）。**走査する集合は rule 1 の候補集合と同じ絞り方でなければならない**: 最初 `siblings` を見ていたが、そこに含まれる `idMatches` は session で絞っていないので、**別 session の名乗り手が埋め戻しを止めた**（外部のコードレビューが指摘し、実測で再現した）。止まると自 session の terminal は候補ゼロ = `terminal_orphaned` で隔離され、隔離は鍵を消費せず還元器は純関数なので**同じ terminal が永久に隔離され続ける**——この module が最重要視している収束しない形そのもの。lineage と session で絞り済みの `nativeMatches` を走査する形に直し、走査集合の広さを変異で固定した。埋めない側の代償は「その pending が native ID を持たないまま」だが、その ID を名乗る terminal は名乗り手のほうで一意に閉じられるので、照合が成立しない形にはならない。**優先は残す**。一度「`nativeIdTaken` が同じ不変条件をより広く守るので不要」と判断して削除したが、これは誤りだった（外部のコードレビューが 3 ラウンド目で指摘し、実測で確認した）。`nativeIdTaken` が守るのは「2 件目を作らない」だけで、**帰属は動かさない**: 相手を配列順で決めると `recovered` も `withSourceEvent` も truncation 判定も**その native ID を持たないほうの operation**に当たり、再配送の provenance が本来の operation に残らず、無関係な兄弟が上限に達していれば `source_events_truncated` を偽って出す。削除したとき変異が生存したのは分岐が検証不能だからではなく、**test が native ID の位置しか見ておらず帰属（`sourceEventIds`）を見ていなかった**から——生存した変異を「意味の無い分岐」と読むのが誤りで、検証が狭い可能性を先に疑う。比較は**値の一致**で行い、届いた start が native ID を持つときだけ働かせる（「何かを名乗っていれば優先」だと、互換の定義上まだ一致を確かめていない相手を選ぶ）。両方向を変異で固定した。連結順そのものは変えない——両集合とも全件衝突のときの衝突の証拠が変わってしまうため。**同じ形は 4 箇所目、上限退避（`retainPendingOperations`）にも残っていた**（外部のセキュリティレビューが指摘し、実測で確認した。上の 3 箇所を直した時点で「この形は潰した」と書いていたのは誤りで、探し方が呼び出し側に寄っていて保持側を見ていなかった）。落とす相手を id の集合で持つと、(a) 1 件分の枠を空けるつもりで**同名の兄弟までまとめて消え**（実測: 上限 256 件・同名 2 件の状態に start を 1 通入れると 256 件のはずが 255 件になり、生きている `started` が消えた）、(b) `dropped.size` が件数でなく**異なり数**になるので退避件数の判定自体がずれ、(c) 診断の `evicted` も id 経由なので**落とした件数を過少に報告する**。保持判定を参照に変えた。（当時の側索引は鍵が `operationId` そのものだったので、そちらは id で消すしかなかった。材料を要素に載せた今は退避で要素ごと落ちるので、鍵で消す経路自体が無い。)ここで一度「**同名の兄弟が残っているなら消さない**」という条件を足したが、**これは fail open だったので撤回した**（外部のセキュリティレビューが次のラウンドで指摘し、実測で確認した）。id が衝突していると、その表は**どちらの兄弟の材料かを原理的に判別できない**。退避した側の材料を残すと、生き残った側の順序検査が他人の材料で通り、**順序違反の terminal が診断ゼロで適用され台帳まで消費される**（実測: 退避側 start=`10`・生存側の実 start=`100` の状態に `ingestSeq` 50 の terminal を当てると `succeeded` になった）。「生きている operation の材料を消したくない」は動機としては正しいが、**消さないことの代償は fail open で、消すことの代償は fail closed な降格**（`terminal_order_unverifiable` → `unknown`）なので、比較する対象を取り違えていた。無条件に消す。材料の復旧は #35（状態に持たせる）が本筋。状態側 identity 欄そのものの検査は Task 6 `reconcileWorkspace` 待ち。**同じ「凍結 schema が課さない一致」は `taskLineageId` にもある**（外部レビューが start 側を指摘し、実測で確認した）。schema は `correlation.taskLineageId` が `state.taskLineageId` と一致することを要求しないので、復元した checkpoint や別実装が書いた状態には別 lineage の pending が schema 妥当なまま並ぶ。`assertSameScope` が束縛するのは **event** のlineage なので、**状態の中身**は誰も絞っていなかった。`operationId` は eventId + matchKey からの導出で lineage を含まないため、同じ derived id の別 lineage の pending が居ると **現 lineage 宛ての start が「自分の再配送」と見なされて重複になり、鍵を消費したまま現 lineage には operation が 1 件も残らない**（実測: 状態 `lineage-1` / pending `lineage-OTHER` で `applied` + `duplicate_operation_start`）。ここも隔離ではなく**絞り込み**で直す（衝突扱いにすると、その checkpoint がある限り毎回同じ隔離になる決定論的な永久隔離になる）。軸で洗い直した結果、`pendingOperations` を触る 4 箇所の内訳はこうなった: (1) start の再配送相手 = 絞る（今回修正）、(2) terminal の候補 = 元から絞っていたが、母数の lineage を `terminalEvent.taskLineageId ?? state...` で取っていた。`assertSameScope` を入口で通しているので値は同じ（死んだ `??`）だが、wire が運ぶ値を候補選びの権威に見せる書き方なので state 固定にした、(3) 放棄の対象 = **絞っていなかった**（実測で別 lineage の operation が `unknown` に倒れた。`sourceEventIds` は append-only なので巻き込んだ `session_ended` の eventId が別 lineage の記録に永久に残る）ので session と同じ理由で絞る、(4) 上限退避 = **lineage 外を status 順より先に落とす**。ここは一度「別 lineage の pending も 256 件の枠を占めるので、退避の対象から外すと自 lineage の live な operation が代わりに落ちる。scope の判断ではなく容量の判断」として**絞らない**と書いたが、**これは誤りだったので撤回する**（外部のセキュリティレビューが指摘し、実測で確認した）。「絞らない」と「lineage 外を優先して落とす」は別の選択肢で、前者を選ぶと**別 lineage の要素で自 lineage の証跡を押し出せる**（実測: 自 lineage の `succeeded` 1 件 + 別 lineage の `started` 255 件で満杯の状態に自 lineage の start を入れると、自 lineage の `succeeded` が退避されて別 lineage 255 件は全て残った）。lineage 外の要素は照合・放棄のどの経路からも候補にならないので、状態に残しておく価値が自 lineage の確定済み証跡より低い——**容量の判断だからこそ価値の低いものから落とす**のであって、容量の判断であることは「順序を決めない」理由にならなかった。**絞り込みにした帰結として、別 lineage の双子が居ると現 lineage 側に新しい pending が積まれ、`operationId` は lineage を含まないので状態に同じ id が 2 件並ぶ**。ここで一度「どちらの材料か判別できないので材料なしに倒す」としたが、**これは誤りだったので撤回する**（外部のコードレビューが指摘し、実測で確認した）。当時の側索引は**凍結 schema の外**にあるので checkpoint から復元されることが無く、entry を書けるのは `assertSameScope` を通った**自 lineage の start だけ**（書く側も読む側も 1 箇所）だった。よって別 lineage の双子は entry の帰属を曖昧にしない。材料を要素に載せた今は帰属を決める表そのものが無いので、この問い自体が消えている。数に入れると「曖昧でないものを曖昧と読む」ことになり、しかもその代償は fail closed 一方向ではなかった: `startConflictsWith` の `recordedSource !== undefined` 節が常に false になって **`turnIdSource` のすり替え検査が無効化される**（実測: 別 lineage の双子を 1 件置くと、native → synthesized_monotonic にすり替えた再配送が `start_conflict` から `duplicate_operation_start` に変わり、配送鍵まで消費した = **fail open**）。`startFactsFor` の同名判定を自 lineage に絞った。自 lineage で id が衝突する場合だけは帰属を判別できないので従来どおり材料なしに倒す（両方向を test で固定した）。**この誤りの教訓**は、「fail closed 側を選んだ」と思った判断でも、その材料を**別の検査が使っていれば**そちらでは fail open になりうること。材料を落とす判断は、その材料の consumer を全部数えてから決める |
| 0 件または複数一致の terminal は何も閉じず、診断を出す | §4.3 | `terminal_orphaned`（候補ゼロ）/ `terminal_unmatched` / `terminal_ambiguous` を返す。候補が居る場合は open のまま `unknown` にする |
| correlation / hash の衝突は隔離する | §4.3・v6「same op ID + different hash: quarantine corruption」 | **指紋の衝突は open な候補についても見る**: 閉じる経路は `terminalFingerprint` を無条件に上書きするので、確定済みの候補にしか検査が無いと、復元状態にだけありうる「open なまま指紋を持つ候補」で規則が破れる（実測: `started` + 指紋 F1 の状態に F2 の terminal を渡すと `applied` / 診断ゼロ / `succeeded` になり、F1 の証跡が状態から消えた）。候補が 1 件に決まっている場面なので、確定済み側のような「兄弟のどれとも一致しないこと」は要らず、上書きする相手そのものだけを見る。**台帳を消費する 2 つの順序分岐より先に判定する**（後ろに置くと指紋が食い違う terminal が順序の穴を通って `unknown` に化け、訂正版の再配送が重複 no-op で消える）。入ってくる側が `unknown` に倒れる経路では発動しない——指紋を書かないので上書きが起きず、その判定は event 自身の性質なので隔離すると永久に閉じられない。 `outcome: "quarantined"`。状態にも台帳にも入れない（入れると訂正版の再配送が重複 no-op になる）。判定材料は `operationKind`（= 保持側の `toolName`）と `canonicalInputHash` の直接比較で、terminal 側では `operationMatchKey` を比べない（§4.3 の matchKey は入力に「turn when present」を含むので、turn をまたいだ terminal が start と違う matchKey を持つのは仕様どおり。rule 1 は turn を要求しない = turn 両立は rule 2 の要件なので、ここで一致を求めると背景実行の完了や prompt 境界をまたいだ tool が永久に閉じない）。kind は matchKey の入力に含まれる identity の一部だが turn と違って start から terminal の間に変わらないので、rule 1 で選んだ候補にも要求できる（**§4.3 の rule 1 の字義は native ID + session/lineage だけなので、kind で絞るのは harness 判断**。identity の一部であること自体は §4.3 の matchKey 導出が担保している）。ただし `toolName` は凍結 schema の `required` に無いので、checkpoint から復元した状態や別実装が書いた状態では schema 妥当なまま欠けうる。素で比べると健全な terminal が永久に隔離され台帳にも入らない（= adapter が無限再送）ため、兄弟の `canonicalInputHash` と同じく**両方 present のときだけ**比べる。start の再配送側は `operationMatchKey` / `operationKind` / `nativeOperationId` / `canonicalInputHash` / `sessionId` / `turnId` を見る（同じ native ID は同じ呼び出しなので turn も同じはず）。`sessionId` を含めるのは、`operationId` が `eventId` + `matchKey` からの導出で session を含まず、`assertSameScope` も lineage と Agent しか束縛せず、状態が session を持たない（lineage は session をまたぐ）ため、ここで比べないと誰も比べないから。`OperationCorrelationV1` の `required` なので任意欄と違って両方 present ガードは要らない。**両方 present ガードには裏側の責務がある**: 記録に欠けている任意欄を再配送が持っているなら、**鍵を消費する前に埋める**。凍結 schema は `nativeOperationId` / `canonicalInputHash` / `toolName` を required にしていないので復元した状態では欠けうる。欠けたまま重複として鍵だけ消費すると、その material を使う照合が永久に成立しない（実測: native id を持たない pending に native id 付きの start を再配送すると重複になり、その native id を名乗る terminal は rule 1 で候補ゼロ = `terminal_orphaned` の隔離を繰り返し、operation は `started` のまま止まる）。埋めて安全なのは、この分岐に来た時点で `startConflictsWith` が false = **両方が持つ欄はすべて一致済み**だから。欠けた欄を埋めるだけなら矛盾は作れない。**ただし `turnId` と `turnIdSource` は埋めない**: どちらも turn scoped で、再配送は元の start と違う turn で届きうる。記録が turn 同一性を持たないときに再配送側の turn を書くと、rule 2 の照合権限を「元の start に無かった turn」で与えることになる（欠落を埋めるのと違い、記録の**意味**を変える）。外部レビューは降格された turn の回復もここで行うことを提案したが、実測すると隔離に倒しても結果は変わらない（記録は `unavailable` のまま残り、native の terminal は同じく `terminal_unmatched` になる）ので、**塞ぐ側にも守る対象が無い**。降格された turn identity の回復は #35 の本筋で扱う。埋めた欄の判定は `recovered` object から導く（欄を別の場所に手で並べると、欄が増えたとき片方だけ更新されて緑のまま守らなくなる）。`turnId` も同じく誰も比べていなかった（§4.3 は matchKey の入力に「turn when present」を含むので、正しく導出された matchKey なら turn が違えば matchKey も違うが、導出は wire 越しに検証できない）。記録された turn は rule 2 の候補選び（`eligible`）が使うので、古い turn のまま重複として台帳に入れると、その operation は本来の turn の terminal で閉じられず `terminal_unmatched` で `unknown` に倒れる。`turnId` は `required` に無く `turnIdSource: unavailable` では正当に不在なので、こちらは両方 present のときだけ比べる。**`turnIdSource` も識別材料に含める**（当初は「凍結 schema の外（側索引・#35）にあり復元直後は空でちょうど必要なときに比べられない」として外していたが、これは誤りだったので撤回する）。材料が無いことを「比べない」で済ませると、`turnId` の文字列だけ同じで種別をすり替えた再配送が `duplicate_operation_start` として台帳に入り、記録は元の種別のまま残る（再配送された start では start 材料の 2 欄を埋めない）ので、**再配送側の種別で来た terminal は rule 2 の候補選び（`eligibleOf`）で落ちて `terminal_unmatched` になり、健全な証跡が `unknown` に倒れたうえに配送鍵まで消費済み**になる（実測）。復元直後の空は `eligibleOf` と同じ扱い——材料があるときだけ比べる——で足りる。ただし**矛盾と言えるのは双方が具体的な turn 同一性を主張している場合だけ**にする: `unavailable` は「turn 同一性を主張していない」という表明で、§4.3 もどちらかが `unavailable` なら rule 2 を適用しないと言うだけで矛盾とは言わない。片側でも `unavailable` を衝突にすると、intake が降格した start（proven でない version の native 主張はここへ落ちる）と、証明が回復した後の同じ start の再配送とが噛み合わず、還元器は純関数なので**毎回同じ隔離 = 決定論的な永久隔離と無限再送**になる（`started` を矛盾集合から外した理由と同型）。ここは 2 人のレビュアーが逆向きの結論を出した箇所で（外部のセキュリティレビューは「再配送側の `unavailable` 免除は caller が指定できるので塞げ」、コードレビューは「免除が片側だけなのが非対称なので対称にせよ」）、決め手は**再配送は start 材料の 2 欄を書かない**という実測だった: 記録された種別はどちらの経路でも元のまま残るので、免除しても記録は汚れず、「訂正版の再配送で記録を直す」経路もそもそも存在しない（塞いでも守るものが無い）。caller が `unavailable` を名乗って得られるのは自分の配送鍵の消費だけで、状態も台帳の他の鍵も動かせない。塞ぐのは種別の**すり替え**（native ⇄ synthesized_monotonic）だけで、そこは 2 つの具体的な主張が食い違っている。**隔離するのは候補が全件衝突する場合だけ**にする: §4.3 どおりに matchKey を導出しない adapter では同じ matchKey で input hash が違う pending が並びうるので、identity が衝突する候補は「この terminal のものである可能性」から外すだけにして、他の候補の照合を妨げない。兄弟の identity を根拠に隔離すると live な operation が永久に閉じない。**免除ではなく絞り込みにする**のが要点で、「互換な候補が 1 件でもあれば全体を免除する」形にすると、確定済みの互換候補が囮になって衝突する open な候補に terminal が付く（確定済み A（hash A）＋ open な B（hash B）に A の terminal を再配送すると、B が診断ゼロで `succeeded` になる）。以降の open 選択・確定済み成否の照合はすべて互換な候補だけを見る。**terminal が識別材料を省いた場合は「衝突しない」ではなく「照合できない」**: 両方 present ガードは復元耐性のためにあるが、そのままだと `canonicalInputHash` を省くだけで検査を無効化でき、同じ matchKey の別 operation を閉じられる（省略は wire 側の自由なので攻撃者が選べる経路）。記録側が持つ欄を terminal が省いていたら適用せず、`terminal_identity_unverifiable` で候補を `unknown` に倒す（隔離ではないので台帳には入り、後から届いた本物の terminal がそのまま閉じられる）。`toolName` 側に対称のものが要らないのは、`operationKind` が envelope の必須欄で空も許さないため terminal から省けないから。**ただし候補の絞り込み側には両方 present ガードが要る**（外部のコードレビューが指摘し、実測で確認した）: rule 2 の候補を選ぶ `.filter` だけ `toolName` を素で比べていたので、`toolName` を持たない状態（凍結 schema の required に無いので復元した checkpoint では欠けうる）では `undefined === "Bash"` が false になり候補ゼロ = `terminal_orphaned` の隔離になる。隔離は台帳を消費せず還元器は純関数なので、**同じ terminal が毎回同じ隔離で収束しない**（実測: `toolName` を消すだけで `succeeded` が `terminal_orphaned` に変わった）。兄弟の 2 箇所（`startConflictsWith` / `identityConflicts`）は最初から両方 present ガードを持っていたので、**同じ形の 3 箇所目でここだけ落ちていた**。§4.3 の matchKey は tool 名を入力に含むので、仕様どおりに導出する adapter では matchKey 一致が既に kind を束縛している。**発火の母数は `plausible`（turn と順序が両立する候補。open / 確定済みの切り分け前）**にする。当初は `compatible` で数えていたが、それだと**この terminal の付け替え先になりえない兄弟が健全な照合を潰す**（実測: open な候補（hash あり）に、確定済みで**別 turn**の兄弟が 1 件並ぶだけで、診断ゼロの `succeeded` が `terminal_identity_unverifiable` に変わり、open な候補が `unknown` に倒れて台帳まで消費された。外部のコードレビューが指摘）。別 turn の兄弟は `sameTurnOf` で落ちるので、この terminal の再配送先ですらない。**母数は `open` でもない**: 確定済みでも**同じ turn** の兄弟は「この terminal はそちらの再配送だった」がありうるので、hash の省略で付け替えが起きる。`plausible` がちょうどその集合。rule 1 の候補数を `byNativeId` から `compatible` へ直したのと同じ「母数の取り違え」で、**この形はこの関数で 3 回目**（rule 1 の件数・矛盾判定・照合不能）。母数を `plausible` にすると `unresolved: open` は `open ⊆ plausible ⊆ sameTurn` から自動的に turn 両立になるが、`compatible` を混ぜる変異が観測できるよう「発火する形のうえで turn 非両立の open な兄弟が巻き込まれない」fixture を別に置いた。**なお発火は互換な候補が 2 件以上あるときに限る**: `canonicalInputHash` は凍結 envelope の任意欄（§3.1）なので省略自体は schema 妥当で、§4.3 が terminal に課すのは「non-conflicting な payload/source hash」＝衝突しないことであって不在は衝突ではない。§4.3 の matchKey は canonical input hash を入力に含むので、仕様どおりに導出する adapter では hash 違いの兄弟はそもそも候補に並ばず、候補が 1 件なら省略で付け替えられる相手が居ない（照合権限は rule 1 = `nativeOperationId`、rule 2 = matchKey + 互換な turn/kind が既に一意に決めている）。素で発火させると「terminal は入力ではなく結果なので hash を載せない」adapter の terminal が 1 通残らず閉じなくなる。候補が 2 件以上並ぶのは matchKey を仕様どおりに導出しない adapter だけで、そこでは hash が唯一の弁別子なので省略された時点で倒す。**判定の位置は成否矛盾検査より後**にする: 前に置くと、確定済みの候補に矛盾する terminal が hash を省くだけで隔離（台帳を消費しないので訂正版が後から効く）を回避して照合不能（台帳を消費する）に化け、訂正版の再配送が重複 no-op として捨てられる |
| 成否が曖昧な terminal は `unknown` を確定する | §4.3 | `successful` が無い場合に加え、kind が失敗を宣言しているのに `successful: true` を名乗る自己矛盾も `unknown` に倒し `terminal_evidence_contradicts` を出す（schema はどちらの欄も valid なので通るが、`succeeded` にすると壊れた adapter が失敗を握り潰せる）。矛盾は照合の成否と無関係な event 自身の性質なので、**照合前に判定して全経路（隔離・unmatched・適用）で出す**。照合できた場合しか出さないと、同じ壊れた adapter でも operation が既に閉じているときだけ `terminal_already_applied` に埋もれて見えなくなる |
| rule 2 は双方が同じ `turnIdSource` 種別の turn 同一性を持つことを要求する | §4.3 | start 側の種別を要素の `PendingOperation.startTurnIdSource` に保持して照合する（当初は凍結 schema の外の側索引に置いていた。やめた理由は §3 の「索引方式をやめた理由」）。**照合は候補の絞り込み時に行い、材料がある候補だけを対象にする**（上の行に詳細）。材料が無い候補を落とさないのは、欄が無い要素が復元直後だけに現れるものではないため: この版より前に書かれた checkpoint も、2 欄を書かない別実装の状態も同じ形になる。どちらの経路でも「材料が無いものは種別違いとして落とさない」で一貫している。**ただし落とさないことと合格にすることは別**で、絞り込みを抜けた候補には「種別が一致した」と「確認できなかった」が混ざる。ここを分けていなかった間は、`startIngestSeq` だけを持つ復元状態（順序は確認できる）に rule 2 の terminal が来ると、状態側に照合材料が無いまま**診断ゼロで**`succeeded` が確定した（実測。独立レビューの指摘）。当時の説明は「材料が無い候補は rule 1 経由なら `terminal_order_unverifiable` に落ちる」だったが、順序材料だけを持つ状態がその前提を外す。閉じる直前に `terminal_turn_unverifiable` を置いて `unknown` に倒す形にした——順序側の`terminal_order_unverifiable` と対称で、**順序の 2 分岐より後**なので 2 欄とも欠く状態はこれまでどおり順序側の診断で観測される。turn 両立を要求しない rule 1 は素通しのまま（通す側も test で固定した） |
| 放棄・復帰時に証跡が無い operation は `unknown` | §4.3 | `finalizeAbandonedState`。§4.2 の重複 no-op はこの経路にも掛かるので、台帳を受け取り、同じ放棄 event の再配送では revision を採番し直さない。配送 ID の衝突判定も還元器と同じで、同じ配送 ID で source hash が違う放棄 event は `outcome: "quarantined"` にする（重複として黙って捨てると放棄が落ちて operation が `started` のまま残る）。放棄の kind は還元器の入口でも弾く（`reduceTaskWorkState` に渡すと operation envelope を持たないので汎用 commit に落ち、状態を変えないまま台帳の鍵だけ消費する。その台帳を渡された `finalizeAbandonedState` は重複として捨てるので、放棄が永久に適用されず operation が `started` のまま残る）。放棄するのはその event の session の operation だけ（lineage は session をまたいで続く。§5 の checkpoint は `sourceSessionId` と `taskLineageId` を別に持つので、絞らないと遅れて届いた旧 session の `session_ended` が resume 先の live な operation を潰す）。還元器の terminal 経路と同じく**黙って間引かない**: `sourceEventIds` が上限の operation は status だけ `unknown` に変わって、そう変えた理由の event が状態から落ちるので、`AbandonmentResultV1.diagnostics` に `source_events_truncated` を出す。**隔離するときの診断も還元器側と同じ `delivery_conflict` を出す**（同じ配送 ID で source hash が違う放棄）。以前は outcome だけで区別できると考えて空で返していたが、§3.1 が求めるのは doctor が理由を報告できることで、doctor が受け取るのは診断の側。空で返すと「なぜ放棄が落ちたか」が経路ごとに違う形でしか分からない |
| 閉じられなかった terminal は unmatched evidence として保つ | §4.3 | `unknown` にした候補の `sourceEventIds` にその terminal を足す。状態が変わった理由を状態から辿れるようにする（放棄経路と扱いを揃える） |
| 状態は lineage ごとに 1 つ | §4 | `assertSameScope` は lineage に加えて `sourceAgent` も束縛する。`OperationCorrelationV1` は Agent を持たず scope が sessionId + taskLineageId だけなので、束縛しないと別 Agent の terminal が同じ session に居る他 Agent の operation を閉じられる |
| seq は safe integer を超える decimal string | v6 §22.6 | `compareIngestSeq` は桁数 → 辞書順の 2 段比較。`Number()` を使わない |

## 2. 正本に無いので harness 側で決めたこと

いずれも hash に効くので、Rust 側は同じ式でなければ parity が取れない。

### 2.1 content hash と revision

```text
contentHash   = SHA-256( JCS( state から stateRevision を除いたもの ) )
stateRevision = SHA-256( JCS({ schema: "free-mem/work-state-revision/v1",
                               previousRevision, eventId, contentHash }) )
operationId   = SHA-256( JCS({ schema: "free-mem/operation-id/v1",
                               startEventId, operationMatchKey }) )
```

- JCS は §22.6 の要求（RFC 8785）。`harness/schema/jcs.ts` の `canonicalizeJson` を使う。
- `stateRevision` を hash 対象から外すのは循環を避けるため。除外は列挙ではなく
  `Omit<CanonicalWorkStateV1, "stateRevision">` と「その形で組み立てる」ことで担保している
  （手で除外リストを持つと欄が増えたとき守られなくなる）。
- 時刻・乱数・連番を使わないので、同じ入力からは常に同じ値になる。

### 2.2 `lastIngestSeq` は単調な watermark

addendum は `CanonicalWorkStateV1.lastIngestSeq` の意味を定義していない。同じ addendum が
event store の watermark を「highest applied `ingestSeq`」と定義している（§6.4 の acceptance）ので、
状態側も **max（後退しない）** とした。遅れて届いた event は revision を作るが watermark は動かさない。

### 2.3 fail-closed に倒した既定（#36）

`operationKind` は自由文字列で、どれが shell / 破壊的 / 外部 / 資格情報かの表が正本にも
capability matrix にも無い。表が来るまでは:

- `PendingOperation.kind` = `tool` 固定
- `replayPolicy` = `never_auto` 固定（§4.3 の `verify_first` は誰にも付かない）
- `sensitivity` = `private` 固定（内容を見ずに `normal` と申告しない）

`CanonicalWorkStateV1.sensitivity` は「構成要素の最大」なので、pending operation が 1 件でもあれば
`private` 以上になる。remote routing を実装する前に #36 を閉じる必要がある。

集約は状態の内容を走査して見つけた `sensitivity` の最大を取る（欄を手で並べると、状態に欄が
増えたとき集約から漏れるため）。**直前の revision の集約値を下限に含める**ので、集約は revision を
またいで単調非減少になる。§10 が集約値を実体に持たせる理由は「raw event の TTL 後には遡って判定
できない」ことなので、構成要素が状態から消えても機密度は下げない（`retainPendingOperations` の退避は
保管上の都合であって「機密ではなくなった」という証跡ではない）。含めないと、唯一の `secret` な
構成要素が退避された次の revision で `private` / `normal` に落ち、§9.2 の remote 送信ゲートが開く。
この模型に格下げの event は無いので、単調にしても失うものは無い。§10 の語彙（`normal` / `private` / `secret`）に無い値を見つけた
ときは最上位に倒す。`indexOf` の `-1` をそのまま順位に使うと最下位（`normal`）に落ちて fail open
になるので、「機密度不明」は自動 resume を止める側へ倒す。

語彙外の値は schema 不正とは限らない。宣言されている `sensitivity` 欄はすべて `$ref: Sensitivity`
（`enum: ["normal","private","secret"]`）だが、集約は状態の内容を再帰して「`sensitivity` という名前の
string キー」を全部拾うので、`Observed.value`（`$ref: JsonValue` = 任意ネストの自由形式）の中に
入った `sensitivity` キーは enum の制約を受けない = **schema 妥当なまま語彙外が入りうる**。
`nativeTodoState` がその経路。

**単調化の代償**（どちらも fail closed 方向なので穴ではないが、#36 が実分類器を入れるときに
移行経路が要る）:

- 語彙外を 1 回混ぜると lineage が恒久的に `secret` になる（原因の payload を直しても戻らない）。
  単調化前は、その構成要素が状態から消えれば戻っていた。
- `sensitivity: private` 固定のプレースホルダが恒久化する。`tool_started` が 1 本でも通れば
  lineage は永久に `private` 以上になり、v6:1001 が `private` 以上を remote refinement / extraction の
  ゲート対象にするので、**tool を 1 度でも使った lineage は §9.2 の送信から恒久的に外れる**。
  #36 で実分類器を入れるときは、集約値の再導出（または移行）の経路を用意する必要がある。

同じ向きの既定が 2 つある:

- **capability matrix が空の daemon には native を与えない**。`activeCapabilityHash` が空文字のとき、
  caller も空文字を名乗れば「一致」してしまう。§3.1 の proven は「active exact-version capability
  matrix hash と等しいこと」なので、matrix が無いなら proven も無い（`sourceAgent` /
  `sourceAgentVersion` の空文字ガードと同じ扱い）。
- **冪等台帳の内部鍵は authority ごとに分ける**。v6 §8.2 の導出式は `adapterDeliveryId` と
  canonical fingerprint を同じ keyspace に置くが、`adapterDeliveryId` は adapter が自由に採番する
  値なので、他 event の `canonicalFingerprint` を写した event を先に送ると本物が診断ゼロの重複と
  して消える。wire に出る導出式（`idempotencyKeyOf`）は正本のままにして、台帳の中だけ
  `d:` / `f:` で分ける。分離は wire にも hash にも出ないので契約に影響しない。

### 2.4 `updatedAt` は単調でない

`updatedAt` は revision を作った event の `occurredAt` にする。`lastIngestSeq`（単調な
watermark）と違って遅れて届いた event では後退し得るが、これは「この revision の証跡が
いつ観測されたか」を指す値なので、最大値へ丸めると嘘になる。順序が要る比較には
`lastIngestSeq` を使う。

### 2.5 還元の結果は 3 値

`applied` / `duplicate` / `quarantined` を `outcome` として返す。`quarantined` が言っているのは
**配送鍵を消費しない**ことだけで、だから訂正した event を後から入れ直せる。**状態を変えない
という意味ではない**: `quarantineWithRecord` を通る隔離（孤児 terminal の記録）は
`droppedEvidence` を足し、revision と history を進める（§2.10 の分類を参照）。呼び出し側は
`outcome` によらず**返った状態を取る**——`quarantined` を見て前の状態を使い回すと記録が落ちる。

### 2.6 intake が付ける kind は native / synthesized の 2 値

§3.1 は intake の導出元（peer identity・channel・captureMethod・capability matrix）を挙げるが、
`derived` は AI 由来の派生物に付く種別で、event intake の出力ではない。よって
`stampIntakeEvidence` は `native` か `synthesized` しか返さない。

### 2.7 別 lineage の event は適用しない

状態は lineage ごとに 1 つ（§4）。`taskLineageId` が状態と食い違う event を還元すると、
境界の確定（§2.2）を経ずに前の task の状態が書き換わるため拒否する。`taskLineageId` を持たない
event は、還元先の状態の lineage に属するものとして扱う。

### 2.8 成否を主張しない terminal

`successful` が無い terminal は成功とも失敗とも言えないので `unknown` にする
（§4.3「Missing or ambiguous terminal evidence establishes `unknown`」の同じ扱い）。

### 2.9 配列上限の保持方針（#39）

frozen schema は `pendingOperations` も `sourceEventIds` も 256 件（§10 の `arrayItems`）に制限して
いるが、当初は addendum に保持・退避の規則が無かった。tool 呼び出しの多い session では上限を
超えるので、harness 側で下記を決めた。**この規則はその後 addendum §4.3「Retention and eviction
(#39)」として正本に採用された**ので、現在は harness 独自の判断ではなく正本の写しである（残って
いる harness 側の判断は `unknown` の失効規則が無いことだけ）。移植する実装は正本に従うこと:

- 上限に達した状態へ新しい start が来たら、**`correlation.taskLineageId` が状態の lineage と違う
  もの** → `succeeded` → `failed` → `unknown` → `started` の **5 群**の順に落として場所を空ける。
  lineage 外を先頭に置くのは、その要素が照合・放棄のどの経路からも候補にならないから——容量の
  判断だからこそ価値の低いものから落とす（絞らずに落とすと、別 lineage の要素で自 lineage の
  確定済み証跡を押し出せる）。**群内の順序は `pendingOperations` の配列位置だけ**で、先頭側から
  落とす。`startedAt` で並べ替え**ない**: 時刻は event を出す側が寄越す値なので、並べ替えると誰を
  残すか adapter が選べる（`droppedEvidence` の並びと同じ規則・同じ理由）。落とした
  `operationId` は `pending_operations_evicted` に並べる
- **落とせるものが無いから取り込まない、にはしない**。`unknown` を状態から消す経路が他に無いので、
  枠が open な operation で埋まると以後すべての start が入らなくなる。訂正版の存在しない隔離を
  adapter が再送し続けるだけで、回復経路が無い（詰まった session は以後どの tool 呼び出しも
  記録できない）。失って影響の小さい順に落として、落とした事実を診断に出すほうを選んだ
- `sourceEventIds` は上限で頭打ちにし、`source_events_truncated` を出す。`unknown` は open のままなので
  同じ operation に terminal が何度でも再照合され、上限を見ないと還元器自身が schema 違反の
  状態を出す（test で `CanonicalWorkStateV1` として検証している）

状態は projection なので、退避した operation の event 自体は daemon の event store 側に残りうる
（§6.4 は acceptance transaction の中で event store を再照会する）。ただし **event の保持期間は
addendum に無い**ので、「必ず残る」とは書けない。**退避を状態に記録する場所は #43 で出来た**:
落とした要素は `droppedEvidence` に `reason: "evicted"` として積まれるので、`started` を落としても
その operation が存在したことは状態に残る（`operationId`・確定 status・start の eventId・時刻。
以前ここには「痕跡ごと消える」と書いてあったが、それは記録先ができる前の話）。**残る限界は
2 つ**——記録は operation の中身全体ではなく識別材料だけで、記録自体も同じ 256 件で有界なので
先頭から落ちる。完了した operation の永続的な置き場は per-kind projection（§3 の未実装項目）で、
そこが入るまでは上限に達した長い session で相関の履歴が短くなる。

start の材料は要素の 2 欄（`startIngestSeq` / `startTurnIdSource`）にあるので、退避で要素ごと
落ちる。**別の表を削り忘れる余地が無い**のがこの置き場を選んだ理由の 1 つで、側索引方式では
`pendingOperations` が 256 件で頭打ちの一方でその表だけが単調増加しないよう、退避のたびに
`operationId` を消す必要があった（やめた理由は §3 の「索引方式をやめた理由」）。

### 2.10 閉じられない terminal を「記録できる」と「記録できない」で分ける

§4.3 は「zero or multiple にマッチした terminal は何も閉じず、unmatched evidence として保ち、
候補を `unknown` のままにし、診断を出す」と要求するが、`outcome` と冪等台帳の扱いは書いていない。
**状態に記録できる相手が居るかどうか**で分けた。

**隔離する（= 配送鍵を消費しない。台帳には入れない）**。ただし**状態を変えるかどうかは分かれる**:

*状態も台帳も動かさない（event 自身の corruption）*:

- `terminal_conflict`（v6「same op ID + different hash: quarantine corruption」）。台帳に入れると
  訂正版の再配送が重複 no-op として黙って捨てられる。live な集合から落ちた証跡は無いので記録もしない

*`droppedEvidence` に記録し、revision と history は進める（`quarantineWithRecord`）*:

- `terminal_orphaned`（候補が 1 件も無い）。start より先に terminal が届く順序前後は正常運用で
  起きる（hook と transcript scan の取り込み順、再起動後の catch-up）。台帳に入れると、後から
  start が届いても同じ terminal は重複 no-op になり二度と閉じられない。隔離しておけば再配送で
  拾い直せる
- `terminal_unmatched` のうち**開いた候補が 1 件も無いもの**（候補は matchKey で拾えるが全員
  確定済みで、turn が両立するものも無い）。`unknown` に倒す相手が居ないので台帳へ入れる形は
  取れない

後者 2 つは隔離でありながら**返る状態は前の状態ではない**（記録・revision・history が進む）。
状態を捨てて「隔離だから不変」と読む移植は記録を落とす。

`terminal_orphaned` と、開いた候補ゼロの `terminal_unmatched` は、隔離に加えて
`droppedEvidence` へ `orphaned_terminal` として記録する。前者は候補ゼロ、後者は候補はあるが
開いているものがゼロで、**どちらも「この terminal を保持する相手が居ない」**——診断だけで
流すと状態から消える。逆に開いた候補が 1 件でもあれば記録しない（その候補が `unknown` として
event の効果を持つので、live な集合から落ちていない）。

**重複判定の鍵は §8.2 の順**（`adapterDeliveryId` → `canonicalFingerprint`。keyspace は台帳と
同じく `d:` / `f:` で分ける）。片側だけの鍵はどちらかの向きを必ず落とすことを両方測った:
指紋だけにすると、配送鍵も `operationMatchKey` も違う 300 件の孤児が同じ指紋を名乗るだけで
記録 1 件に潰れた（history 1 / 記録 1 / 台帳 0、299 件が診断も無く消える）。`eventId` だけに
すると、同一配送の再送が毎回別物として記録された（1 → 300 revision）。前者は adapter が
算出して wire で運ぶ値を単独の権威にしたための穴、後者は再配送契約（同じ配送鍵・違う
`eventId`）を無視したための穴で、**正直な adapter でも起きるのは後者**。

鍵が一致していても**指紋が食い違うなら重複ではない**。同じ配送 ID で内容が違うのは §4.3 の
corruption で、適用済み経路は同じ条件を `delivery_conflict` にしている。配送鍵を第一 authority に
した分、ここを黙って重複に倒すと孤児経路だけがその検査を失うので、記録も状態も鍵も動かさずに
`delivery_conflict` だけを出す。どちらかが指紋を名乗っていない場合は「違う」と言えないので
発動しない（FR-012 と同じ扱い）。

**この検査は還元器の入口に置く**（台帳の衝突検査と同じ位置）。記録側だけに置くと、**start が
届いた後の再送を取りこぼす**: 隔離は配送鍵を消費しないので、孤児を記録した後に start が来ると
同じ配送鍵の再送は照合経路へ進み、記録側の検査に一度も当たらない（実測: 孤児 F1 を記録 →
start → 同じ配送鍵で F2 を再送すると `applied` / 診断ゼロ / operation は succeeded / 指紋は F2、
記録は F1 のまま）。corruption の検出が **start の到着順しだい**になるのが問題で、入口に移すと
順序に依らない。移した結果、記録側の同じ検査は到達不能になったので畳んだ——同じ規則を 2 か所に
置くと片方だけが直る。

**候補を `unknown` に倒して台帳へ入れる**:

- `terminal_unmatched`（**開いた候補が居て**、turn 両立などで 1 件に絞れない）/ `terminal_ambiguous`
- `terminal_out_of_order`（start の `ingestSeq` が状態にあり、terminal がそれ以前）。terminal の
  証跡が来ている以上「まだ走っている」とは言えない。`unknown` にするのは一致した 1 件だけで、
  同じ match key の無関係な open は巻き込まない
- `terminal_order_unverifiable`（checkpoint から復元して `startIngestSeq` を持たない: #35）。**ここを
  隔離にしてはいけない**。復元直後は全 terminal がこの分岐に落ちるため、隔離すると operation が
  `started` のまま二度と閉じられず、resume capsule が「まだ実行中」と偽る（`unknown` より悪い）。
  §3.1 の fail closed（自動経路を降格し、理由を doctor に出す）どおり `unknown` へ倒す

`terminal_orphaned` を `terminal_unmatched` と別 code にしたのは、同じ code で `outcome` が
分かれると doctor が「start 待ちの孤児」と「候補は居るが絞れない」を区別できないため。

**（この段落は #43 で解消済み。経緯として残す）** かつてこの隔離は §4.3 の文面からの明示的な
乖離だった。§4.3 は「zero **or multiple** にマッチした terminal は … unmatched evidence として
保つ」と書き、隔離は「correlation/hash conflict」に限っているのに、当時の `CanonicalWorkStateV1`
には unmatched evidence の置き場が無く（候補が 0 件なので `sourceEventIds` を足す相手が居ない）、
frozen schema のままでは正本どおりに実装できなかった。台帳へ入れる側に倒すと上記のとおり
順序前後の孤児を永久に殺すので、当時は隔離を選んで schema 側の穴として #43 に起票した。
**現在は `droppedEvidence` がその置き場**で、孤児 terminal は `orphaned_terminal` として状態に
記録される（隔離という判断はそのままで、記録が伴うようになった）。移植する実装はこの段落では
なく §2.10 と正本 §4.3 に従うこと——記録を省くと状態も revision hash も一致しない。

**復元後に再配送された start で start 材料の 2 欄を埋めない。** §6.4 は `ingestSeq` を
「authoritative event-store watermark（適用済みの最大 `ingestSeq`）」と定義しており、採番するのは
daemon の event store である。したがって再配送 event が運ぶ `ingestSeq` は再配送時の取り込み位置で
あって、元の start の権威順序ではない。埋めると 2 つの意味で壊れる:

1. **正しさ**: 再配送の位置を元の start の位置として順序検査に使うことになる
2. **信頼境界**: 復元直後（start 材料の 2 欄が無い）に、被害者の `startEventId` か `nativeOperationId`
   と `operationMatchKey` を写した start を**小さい `ingestSeq`** で送ると、正規の terminal が順序検査を
   通って `unknown` ではなく `succeeded` になる。§14 の zero-tolerance カウンタ
   `unsafe unknown replay` に直結する（材料は §10 の `ResumeCapsuleV1.workState` がそのまま運ぶ）

よって復元後は `terminal_order_unverifiable` で `unknown` に倒れる。これが §3.1 の fail closed どおり
であり、順序材料の復旧は #35（start の `ingestSeq` を `PendingOperation` に持たせる）が本筋。

**台帳だけを失った復元**では、再配送された start が還元器に届く。再送契約では再配送の
`eventId` が変わるので、`operationId = SHA-256(JCS({schema, startEventId, operationMatchKey}))` は
一致しない。そのままだと同じ operation が 2 件積まれ、以後 rule 1 の terminal が候補 2 件で
何も閉じられなくなる。`nativeOperationId` は本物の呼び出しごとに一意なので、それが一致する
pending があれば再配送として扱う。`nativeOperationId` を出さない adapter では 2 回目の本物の
呼び出しと区別できないので、この経路は使わない（§4.3 が rule 1 だけに閉じる権限を与えているのと
同じ非対称）。

同じ `nativeOperationId` を名乗りながら `operationMatchKey` か `operationKind` か `canonicalInputHash`
が違う start は再配送ではなく corruption なので `start_conflict` で隔離する（再配送として台帳に
入れると、訂正版が同じ配送 ID で来ても重複 no-op になって戻せない）。terminal 側と違って matchKey も
比べるのは、同じ native ID は同じ呼び出し = 同じ turn のはずで、turn 差で matchKey が変わる余地が
無いため。再配送の検索は「導出した `operationId` 一致」→「`nativeOperationId` 一致」の順に当たるので、
前者で当たった場合は native ID を一度も比べていない。そのため衝突検査は `nativeOperationId` の
直接比較も持つ（後者で当たった場合は常に等しいので効かない）。

**`terminal_orphaned` の隔離には期限が無い**。start が後から来る孤児（順序前後）と、start が二度と
来ない孤児（退避で消えた operation、daemon が実行途中で attach して terminal だけ捕まえた場合）は
還元器の中では区別できない。台帳に入れる側に倒すと前者を永久に殺すので隔離を選んだが、後者は
adapter が再送を続けることになる。

**打ち切りは呼び出し側の責務**とする。`quarantined` + `terminal_orphaned` を受けた delivery 層は、
同じ冪等キーの再送を「その session の取り込み連番が孤児 terminal の `ingestSeq` を十分に
追い越すまで」に限り、それを過ぎたら unmatched evidence として doctor に出して捨てる。ここで見る
のは delivery 層が持つ session 側の連番であって、状態の `lastIngestSeq` ではない。状態の watermark
は**この lineage に適用した event の最大値**なので、同じ session の別 lineage の event ぶんだけ
session の連番より遅れる。しかも watermark は連続被覆を主張しない（addendum §4.1）ので、これを
「start はもう来ない」の根拠にはできない。還元器側に
期限を持たせないのは、時刻も試行回数も状態に入れられない（決定的でなくなる・frozen schema に
置き場が無い）ため。退避された operation は `droppedEvidence` として状態に残るようになった（#43）ので、
この分岐の後者——「退避されて live set から消えた start の terminal」——は証跡としては失われなくなった。
還元器の分岐そのものは残る: 記録は identification のみで、live set に戻す材料ではない。

### 2.11 event kind の分類（#29）

`operation` envelope を要求する kind の集合は正本に無い。harness の正規化語彙
（`harness/schema/capability.ts` の `EventKind`）に対する分類を
`OPERATION_EVENT_PHASES` / `NON_OPERATION_EVENT_KINDS` として `harness/schema/continuity.ts` に置いた。
test は「2 つの和が `EVENT_KINDS` に過不足なく一致する」ことを `EVENT_KINDS` から導いて検査するので、
kind を足すと分類を決めるまで CI が落ちる。語彙に無い adapter 固有の kind は未分類として
envelope を要求しない（既知の非 operation kind が envelope を持つ場合だけ拒否する）。

同じ理由で、**どの kind が放棄を確定するかも正本に無い**。§4.3 は「放棄・復帰時に証跡が無い
operation は `unknown`」とだけ書く。`finalizeAbandonedState` は export された関数なので、限らないと
routing の取り違えで届いた `user_prompted` 等が同 session の実行中 operation を全部 `unknown` に
したうえで冪等キーを消費する。語彙のうち「その session がもう進まない」と言える
`session_ended` / `session_interrupted` の 2 つに限り、他の kind は throw する。復帰側（resume）は
event ではなく checkpoint 経路なのでここには含めない。

`canonicalFingerprint` は schema の `required` に入っているが `maxLength` しか制約が無いので空文字が届きうる。v6 §8.2 の dedupe authority は「`adapterDeliveryId`、無ければ canonical fingerprint」なので、空文字を「値がある」と読むと
2 通りに壊れる: 配送 ID を持たない event が全部 1 つの鍵に潰れて最初の 1 件以外が診断ゼロで消える／配送 ID を持つ event は
台帳に source hash 無しで載り、同じ配送 ID の訂正版が衝突検査を素通りして重複として捨てられる。空文字の
`adapterDeliveryId` は fingerprint へ落とせるがこちらは落とし先が無いので、`assertIdentityMaterial` で schema violation にする。

同じ関数で `eventId` の空文字も落とす。`deriveOperationId(eventId, matchKey)` の材料なので、空文字だと同じ turn の
rule 2 の start が 2 件とも同じ operationId になり、2 件目が `duplicate_operation_start` として消える
（以後その operation の terminal は照合できない）。

`sessionId` も同じ族。§4.3 の候補選びと放棄はどちらも session で絞るので、空文字だと session を
特定できない adapter の event が全部同じ scope に入り、別 session の terminal が診断ゼロで operation を
閉じ、別 session の `session_ended` がそれを放棄する（実 ID なら `terminal_orphaned` で隔離される）。
`turnId` と違って「不在」を表す語彙が無いので落とすしかない。これは bot 指摘ではなく、同じ族を
schema の `required` 側から棚卸しして見つけたもの。

`turnId` も同じ形の穴を持つ。schema は `maxLength` しか課さないので、`turnIdSource` が native /
synthesized_monotonic のまま空文字が届きうる。空文字を「turn がある」と読むと §4.3 rule 2 の turn 同一性が
空文字同士で成立し、無関係な turn の operation を閉じる。unavailable な turn を全部空文字で表す adapter では
全 operation が 1 つの turn に潰れるので、`assertTurnIdentity` で schema violation にする（`unavailable` の
ときに `turnId` を持てない不変条件は従来どおり）。

envelope の任意欄（`nativeOperationId` / `canonicalInputHash`）は schema が `maxLength` しか持たない
ので空文字が届きうる。空文字を「値がある」と読むと rule 1 が「native ID を持たない operation」
同士を全部同じものとして照合するため、`assertOperationEnvelope` で schema violation に落とす
（正規化を照合側の 5 箇所に散らさない）。この欄の検査は **adapter 固有の kind にも掛ける**。
既知の phase を持たない kind は phase の照合こそできないが、envelope を持つなら還元器の
operation 経路（start / terminal）にそのまま入るので、検査を飛ばすと同じ穴が custom kind 経由で
開いたままになる。

**空判定は「空文字」ではなく「空白・書式制御文字だけ」で行う**（`isBlank`）。schema が課すのは
`maxLength` だけなので、上に並べた実害はどれも空白 1 文字・タブ・改行・U+FEFF・U+200B でそのまま
再現する（`unavailable` を空文字で表す adapter は、同じ理由で空白でも表す）。JS の `\s` は U+FEFF まで
含むが U+200B などの書式制御文字は含まないので `\p{Cf}` を足す。判定は 1 箇所に置いて
`assertIdentityMaterial` / `assertTurnIdentity` / `assertOperationFields` / `idempotencyKeyOf` /
`ledgerKeyOf` / `sourceHashOf` の全部で使う（ゲートを足すたびに同じ穴を作り直さないため）。
空白を**含む**だけの値（`"session 1"`）と `"0"` は identity として妥当なので落とさない。

**残るリスクは state 側に移った**。`CanonicalWorkStateV1` の
`{taskLineageId, sourceAgent, projectId, workspaceId, stateRevision}` はどれも `required` + `maxLength`
だけで、還元器はどれも検査しない。event 側に同じガードを足しても意味は無い（`assertSameScope` が
event の `taskLineageId` / `sourceAgent` を state の値と突き合わせるので、event 側の検査は state 側の
anchor があるぶん構造的に冗長で、唯一残る失敗形「state 側が空」では一致する正しい event まで落として
全停止になるだけ）。**Task 6 の `reconcileWorkspace` が event から state を生む時点で、これらの非空を
保証すること**が本 PR からの前提条件。

## 3. 限界

- **per-kind の状態投影は未実装**。prompt / file / command / test を `Observed*` へ写す規則が addendum に
  無いため、還元は bookkeeping（watermark・revision・pendingOperations）だけを行う。
- **§4.3 の 3 つの検査は状態だけで実施できる（#35 / #44 で解消済み。以前はここが穴だった）**。
  権威順序（terminal は start より後）・rule 2 の turn 種別・payload/source hash の衝突は、いずれも
  `PendingOperation` に載せた任意欄（`startIngestSeq` / `startTurnIdSource` / `terminalFingerprint`）
  から読む。**索引方式をやめた理由**: 最初は `TaskWorkStateSnapshotV1.operationStarts` を
  凍結 schema の外に置いていたが、鍵が `operationId` である以上、**凍結 schema は `operationId` に
  一意性を課さない**（`maxLength` だけ）ので、同じ id の pending が並ぶ状態では**どちらの兄弟の
  材料か原理的に判別できない**。判別せずに引くと片方の材料でもう片方が検査され、実測で兄弟 B の
  `ingestSeq` 100 を使って A 宛ての terminal が診断ゼロで通った（逆向きの値では健全な terminal が
  弾かれた）。当時は「曖昧なら材料なし」に倒して塞いだが、それは**生き残った側の証跡まで
  `unknown` に落とす**代償を払っていた（退避で同名の材料をまとめて消す規則も同じ理由で必要だった）。
  要素に載せると鍵が要らず、材料は operation と同じ寿命になり、退避・復元でも取り違えが起きない。
  索引・`OperationStartFactsV1`・`startFactsFor`・退避時の索引同期はすべて削除した（実装は 29 行純減）。
  **残る境界**: 3 欄はいずれも任意なので、この版より前に書かれた checkpoint と、欄を書かない別実装の
  状態には無い。**材料が無いことは検査合格ではない**——順序は `terminal_order_unverifiable` で
  `unknown` に倒し、turn 種別は**候補選びでは**落とさず免除するが、免除は「検査した」ではないので閉じる直前に `terminal_turn_unverifiable` で `unknown` に倒す（rule 1 は turn 両立を要求しないので素通し）。指紋は従来どおり適用済み扱いにする。
  空白も「無い」として読む（素で見ると空文字が `compareIngestSeq` に渡り、確認できていない順序が
  確認できたことになる）。
- **状態から消えた証跡は状態に残る（#43 / #39 で解消済み）**。上限で退避された operation と、
  live set のどの operation も retain できなかった terminal を `CanonicalWorkStateV1.droppedEvidence` に有界に記録する。後者は**綴りが 2 つある**——候補が 1 件も一致しなかった場合と、一致した候補が全件確定済みで turn 両立する相手が居なかった場合——で、状態から見ればどちらも同じ「証跡が live set から消えた」なので、どちらも `orphaned_terminal` として記録する（open な候補が 1 件でも残る場合は記録しない: その候補が `unknown` になって event の効果を担う）。
  以前は「記録先が凍結 schema に無いので、隔離と commit の差は配送鍵を焼くかどうかしかない」と
  書いていた箇所で、状態を受け取った側には「静かに消えた」と「そもそも来なかった」の区別が
  届いていなかった。**孤児の記録は隔離の代わりではない**: 配送鍵は消費しないまま記録だけ足すので、
  後から start が届けば同じ terminal の再配送で閉じられる。記録が状態を変える以上 revision を
  採番し history にも積む（片方だけ動かすと状態の revision が自分の履歴の末尾に無くなる）。
  同じ terminal は 2 度足さない——隔離は鍵を消費せず還元器は純関数なので、素で足すと記録が再送の
  たびに伸びて 256 件の枠を食い潰す。**同一性は §8.2 の順で見る**（`adapterDeliveryId`、無ければ
  `canonicalFingerprint`。keyspace は台帳と同じく分ける）。`eventId` は封筒の値なので再配送で
  変わり、それを鍵にすると再送のたびに足してしまう。**残る境界**: 記録が 256 件を超えたら先頭から落ちるので、
  古い孤児の行が落ちた後に同じ terminal が再送されると 1 行だけ積み直される。打ち切りは
  delivery 層の責務（evidence §2.10）で、還元器の側では止められない。
- **boundary authority（§2.2）は未実装**。§3.1 の必須 negative の後半
  「forged native event は task boundary を confirm できない」は、confirm 側が入るまで
  「synthesized へ降格する」ところまでしか検証していない。
- **`finalizeAbandonedState` は history を作らない**。戻り値が `CanonicalWorkStateV1` であって
  snapshot ではないため、revision の履歴を積むのは呼び出し側の責務。冪等台帳は受け取るので、
  同じ放棄 event の再配送で revision を採番し直すことは無い。
- **intake の診断は呼び出し側が集める**。`stampIntakeEvidence` は `{ event, diagnostics }` を返すが、
  それを還元結果の診断と併せて doctor へ渡すのは daemon 側の仕事で、参照実装は連結しない。
- **`lastIngestSeq` の意味は正本に無い**（#38）。ここでは単調な watermark として実装している。
- **event kind が成否を主張するかは語彙に書かれていない**。`tool_completed` / `tool_failed` は
  `harness/schema/capability.ts` の `EventKind` に列挙があるだけで、意味の定義が正本にも harness にも
  無い。`tool_failed` は名前が失敗を宣言しているので `successful: true` を矛盾として扱うが、
  `tool_completed` が成功まで主張するかは決められないので `successful: false` は矛盾扱いしない
  （`failed` のまま記録する）。語彙側で決めるべき宿題。
- **turn identity の降格は intake が行う（#41 は addendum §3.1 で決着済み。ここは限界ではなく
  規定の記録）**。証明されていない `native` 主張は intake が `unavailable` へ倒し、
  `turn_identity_downgraded` を出す。**delivery 層では降格しない**。
  `synthesized_monotonic` は認証条件を持たないので**未認証経路でも降格せず**、代わりに
  `turn_identity_unauthenticated` を出す（判定は**経路の認証** = `authenticatedPeer` だけを見る。
  version の authority と一緒にすると、CLI を上げた直後の version drift が「未認証」として
  観測に混ざる）。残る露出——未認証経路の event が rule 2 の turn 両立を満たしうる——も正本に明記した。
  parity fixture は `unauthenticated-synthesized-turn`（`rejectedBy: intake-diagnostic`）。
  経緯: 正本は §3.1 で intake に `evidenceKind` と `ingestAttestation` の権限しか与えておらず、
  `turnIdSource` の書き換えは明示されていなかった（§14 は未証明時の措置を
  `turnIdentityDisposition` による delivery 層の downgrade として書く）。参照実装が fail closed の
  向きで intake に倒していたのを、正本の側を書いて追認した形。§6.3 の
  `turnIdentityDisposition` は destination 側を見る補完で、**そちらは未実装**（schema の欄はあるが
  読む実装が無い）なので、この complementarity は今日は片側しか動かせない。
- **`assertSameScope` の不一致は throw する**。別 Agent / 別 lineage の event を状態に渡すのは
  router のバグなので隔離ではなく例外に倒しているが、`sourceAgent` は event が名乗る値なので、
  scope を event 由来で選ぶ実装だと 1 件の取り違えで stream が止まりうる。daemon 側で
  「state は認証済み peer identity で選ぶ」を守る前提。
- **`sessionId` は intake が束縛する（#42 で解消済み。以前はここが穴だった）**。§4.3 の correlation
  scope は「same session/task lineage」なのに、§3.1 が intake に導出させるものとして挙げていたのは
  「認証済み peer identity・ingest channel・`captureMethod`・capability matrix」だけで **session が
  入っていなかった**。`IntakeContextV1` は `expectedSourceAgent` しか束縛せず session は素通しで、
  `assertSameScope` は状態側に session が無いので照合できない——結果として、同じ Agent・同じ
  lineage の別 session を名乗る event が rule 1 で他人の operation を閉じられ、安価な start を `CONTINUITY_LIMITS.arrayItems` 件だけ送って
  他人の実行中 operation を退避させられた（§10 の上限。件数を doc に写すと定数の変更で黙って古くなる）。
  現在は `IntakeContextV1.expectedSessionId` を足し、**降格ではなく intake が受け取らない**
  （`sourceAgent` と同じ理由: session は証跡の質ではなく scope selector なので、降格しても値は残り
  他人の operation を選べてしまう）。正本にも §3.1 の一文として書いた。**残る境界**は `sourceAgent`
  と同じで、intake を経由せず還元器を直接呼ぶ経路と、`expectedSessionId` を空で渡す
  （= 権限のある session を名乗れない）ingest 経路。後者は従来どおり素通しで、締めると認証できない
  経路が全滅するため。
- **還元器は event の schema 妥当性を前提にする**。`ingestSeq` は decimal string として検査するが、
  `occurredAt` の形式・文字列長（§10）・JSON 深さは見ない。壊れた値をそのまま状態に写すので、
  daemon 側で `reduceTaskWorkState` を呼ぶ前に `NormalizedContinuityEvent` schema での検証を
  済ませておく前提。同様に `IntakeStampedEventV1` の目印は型だけのもの（`JSON.parse` や spool を
  跨げば消える）なので、信頼境界ではなく呼び順の lint として扱う。
- **状態は権威として扱い、還元器は検証しない**（#35 / #43 で欄が増えたので明示する）。
  `startIngestSeq` のように**比較に使う**値だけは綴りを見て、合わなければ「名乗っていない」に
  倒す（合わない値をそのまま `compareIngestSeq` に渡すと throw し、壊れた要素を狙っていない
  terminal まで巻き添えで落ちるため）。それ以外——低い `startIngestSeq` を持つ状態、`evicted` に
  `eventId` を載せた記録、上限を超えた `droppedEvidence`——は素通しする。**同じ状態を書ける相手は
  `status: "succeeded"` を直接書ける**ので、ここで検査を足しても守れるものが増えない。
  checkpoint の出どころを保証するのは daemon 側の責任。以下は実測で確かめた具体的な境界:
  - **上限超えの `droppedEvidence` を repair するのは、記録を足す経路だけ**。`recordDroppedEvidence`
    を通る経路——start の退避と、**記録を生む terminal**（`terminal_orphaned` /
    `terminal_unmatched`）——では刈った事実が `dropped_evidence_overflowed` に出る。記録が重複でも
    刈りは行う（重複を素の隔離に倒すと、刈った結果ごと捨てて上限超えの状態を返し続ける）。
    一方、**operation を閉じるだけの terminal**と放棄の経路は `nextContent` を通るだけで、
    診断の出口が無い。**黙って間引かない**ほうを優先し、記録を生む event が次に来るまで上限超えの
    まま運ぶ（凍結 schema の `maxItems` に反する状態をその間だけ出しうる）。
  - **`DroppedEvidenceEntryV1` の欄の組み合わせは誰も検査しない**。`reason` ごとに書く欄は
    還元器が決めているが、schema は `oneOf` を持たず（別言語の validator で挙動差が出るため）、
    runtime にも検査は無い。読む側は `reason` で分岐すること。
  - **孤児 terminal の記録は上限をまたぐと再び足されうる**。重複判定は live な
    `droppedEvidence` だけを見るので、その記録が 256 件の枠から押し出された後に同じ terminal が
    再送されると、もう一度記録して `stateRevision` が動く。隔離は配送鍵を消費しない設計
    （後から start が届けば閉じられる）なので再送自体は止まらない。**枠の中では収束する**——
    churn には 1 つの lineage で 1 件の孤児と 256 件以上の脱落が要る。`history` を見れば
    厳密に判定できるが、それは上限の無い走査になるので採らなかった。
  - **退避された operation の `terminalFingerprint` は記録に残らない**（`DroppedEvidenceEntryV1` は
    識別と分類だけを持つ）。ただし退避された operation 宛ての terminal は #44 の前から
    候補ゼロで孤児隔離になっていたので、この欄が消えることによる差は無い。
  - **`history` の `eventId` は一意でない**（実測）。配送鍵は `adapterDeliveryId` を含むので、
    同じ `eventId` を違う配送 ID で 2 回送ると 2 回とも適用され、行が 2 つ積まれる。
    `history` を `eventId` で引く消費者は書けない。
  - **記録が上限で押し出されたことは、状態だけを読む側には分からない**。脱落は
    `dropped_evidence_overflowed` に出るが、診断は状態の外にある。退避と孤児が同じ FIFO を
    共有しているので、**片方を 256 件流せばもう片方の狙った記録を消せる**（消えた後の状態は
    「最初から無かった」と区別が付かない = #39 が閉じた穴が 1 段下で開く）。閉じるには
    脱落の累計や reason 別の件数を状態側の scalar として持つ必要があり、それ自体が canonical
    hash の面を変える変更なので、この cluster では入れずに #61 として切り出した
  - **識別子と指紋は生値のまま状態に入り、機密度は `private` 固定**。凍結 schema は `eventId` /
    `operationId` / `canonicalFingerprint` を `maxLength` だけの文字列として許し、digest 形式を
    強制しない。識別子に資格情報を混ぜる adapter が居れば、その値が `secret` ではなく
    `private` として checkpoint と診断に残る。**この露出は #43/#44 が作ったものではない**：
    `Observed*.sourceEventIds` が同じ値を同じ機密度で以前から保持している。閉じるには intake で
    識別子の不透明性を強制する必要があり、event 表面全体に及ぶので #62 として切り出した
  - **上限超えの復元配列を刈るのは記録経路だけ**。刈りは `recordDroppedEvidence` の中にあるので、
    start と孤児 terminal は通る。**何も足さない event でも刈る**——退避ゼロの start も、記録が
    重複だった再送も刈る（上限は append の性質ではなく状態の性質で、運ぶと自分の凍結 schema に
    反する状態を revision ごとに出し続けることになる）。刈りは 1 度で上限に収まるが、**刈りで落ちるのが
    その terminal 自身の記録だった場合は次の再送で積み直される**ので、収束は最大 2 revision
    かかる（実測: 記録が配列の先頭にある 257 件の状態で 1 回目が孤児ごと刈り、2 回目が再記録、
    3 回目で差分ゼロ）。これは下の「上限をまたぐと再び足されうる」と同じ境界。**operation を閉じる terminal と放棄は記録経路を通らない**ので、
    上限超えの配列をそのまま次の revision へ運ぶ。全経路で揃えるには刈りを revision の構築側へ
    移す必要があり、FR-015 が求めているのは schema 側の上限なのでここでは移していない
  - **復元状態が `startEventId` を空白で書いていると、退避の記録は兄弟を判別できない**。
    凍結 schema は `OperationCorrelationV1.startEventId` を必須にしているが `minLength` は
    持たないので、空文字が届きうる。`declared()` はそれを「名乗っていない」として扱うため、
    記録は `eventId` を持たず、同名・同 status の兄弟が並ぶ状態では落ちた側を特定できない
    （欄を持つ状態に戻る）。**代わりの識別子を捏造しない**: `sourceEventIds[0]` は順序が
    保証されない append-only の配列で、そこから start を推測するのは 1 度やって差し戻した
    誤りそのもの。`nativeOperationId` は任意欄で、記録側に置き場も無い。**復元状態を検査して
    弾くこともしない**——「状態は権威であって検査しない」がこの還元器の立場で、`startIngestSeq`
    の綴り検査を例外として明示しているのと同じ理由（`startEventId` を空白で書ける実装は
    `status` も直接書ける）。材料が無いことは「特定できない」であって、記録側の欠陥ではない。
  - **revision ごとの複製は配列 1 段だけ**。`pendingOperations` も `droppedEvidence` も
    `[...arr]` で配列だけを分け、要素 object は過去の revision と共有している。凍結 schema の
    欄は `readonly` ではないので、要素を書き換える caller が現れれば `contentHash` を採った後の
    過去 revision も一緒に変わる。**還元器自身は要素を書き換えない**（どの経路も新しい object を
    作る）ので現時点で観測できる不整合は無く、これは `droppedEvidence` に固有でもない
    （`pendingOperations` が以前から同じ形）。閉じるには状態グラフ全体を `readonly` にするか
    revision ごとに deep clone する必要があり、どちらもこの cluster の範囲外
- **保持先が満杯だと terminal の identity が状態から消える**（#68）。`droppedEvidence` に落とす
  判定は「この terminal を保持できる**相手が居るか**」で、「実際に保持**できたか**」ではない。
  open な候補の `sourceEventIds` が既に 256 件だと `unresolved` は空でないので記録の分岐に
  入らず、後段の append は上限で拒まれる。実測: 候補は `unknown` に倒れ、診断は
  `[terminal_out_of_order, source_events_truncated]`、`droppedEvidence` は空、**台帳は 1**
  （鍵は消費されるので adapter は再送しない）。status は残るが `eventId` も指紋も配送 ID も
  残らない。#43 が塞いだ穴と同じ形が別の軸に残っている——判定を「保持できたか」へ移す必要が
  あり、それは correlate と apply の間で truncation の結果を戻す構造変更になるのでここでは
  やらない。
- **event 由来の文字列の長さを検査していない**（#69）。凍結 schema は `maxLength: 8192` を
  課すが、還元器は event の文字列長を見ないまま状態へ写す。実測: 8193 文字の
  `canonicalFingerprint` を持つ孤児 terminal で状態が schema 違反 2 件、8193 文字の `eventId`
  を持つ start で 4 件。**後者はこの変更の前から同じ**（`sourceEventIds` は前から `eventId` を
  そのまま写している）ので、新しい欄が穴を作ったのではなく既存の穴が広がった形。識別子は
  配列と違って**切り詰められない**（切り詰めると別の値になり `orphanKeyOf` と台帳が誤って
  一致する）ので、入口で fail closed に倒す設計判断が要る。
- **session 全体を 1 回の fold で流す用途には向かない**。`commit` は event ごとに冪等台帳と
  history を複製するので、fold の長さに対して二乗で伸びる（実測: 1,000 event 61ms、
  5,000 event 1,024ms、20,000 event 23,332ms）。参照実装は「同じ fixture から TS と Rust が
  同じ値を出すか」を確かめるためのもので、常駐 daemon の還元器はこの複製をしない実装にする。
  台帳と history を呼び出し側が持つ append-only 構造にすれば線形になるが、それをやると
  「直前の snapshot は書き換わらない」という比較の前提が崩れるので、ここでは複製のままにした。

## 4. 再現方法

```bash
node --experimental-strip-types --test harness/continuity/reference-model.test.ts
node harness/contract-hashes.mjs > harness/contract-hashes.json   # fixture を足したら再生成
bash harness/continuity/mutate.sh                                 # §5 の変異テスト
node harness/continuity/old-shape-baseline.mjs                    # §4.1 の baseline を再生成
```

`harness/fixtures/continuity/tool-lifecycle-reduction.json` と `restored-state-reduction.json` の
`expected` は参照実装の出力そのもの。導出（2.1）を変えたら test が期待値との差分で落ちるので、
意図した変更であることを確認してから `expected` を書き直す。

**2 本ある理由**: 前者は**空の状態**から始め、後者は**新しい欄を持つ状態を読む**（#35 の
`startIngestSeq` / `startTurnIdSource`、#43 の `droppedEvidence`、#44 の `terminalFingerprint`）。
新しい欄はすべて任意なので、1 本にまとめると「欄を書きはするが読まない」移植でも hash が合う。
旧い形の fixture を消さないのも同じ理由で、欄を持たない状態が読めること自体が要件（FR-013/FR-014）。

### 4.1 旧形入力の差分ゲート（SC-003）

「新しい欄を持たない状態への挙動は、4 件の欠陥是正以外変わらない」を、**変更前の実装との
突き合わせ**で判定する。`harness/fixtures/continuity/old-shape-parity.json` は旧形の corpus
（20 case / 29 step）と、それを分岐点 `d517a8b` の実装に通した結果を両方持つ committed artifact で、
`old-shape-parity.test.ts` が同じ corpus をこの実装に通して食い違う JSON path を許可表と照合する。

決め方の要点:

- **基準は sha で固定する**。実行時に `origin/main` を引くと、この変更が main に入った時点で
  比較対象が変更後の実装になり、門が自分で自分を無効化する。
- **比較面は還元結果を素通しする**。形を変えるのは台帳（`Map`）を鍵で整列した配列に直す 1 か所
  だけで、欄を選び直さない。最初は診断を `code` だけ、history と台帳を件数だけに縮約していたが、
  それだと `detail` の作り替えも台帳の中身の入れ替えも差分に出ない（隔離環境で
  `terminal_order_unverifiable` の `detail` を壊しても全 test が緑のままだった）。
- **hash も比較面に残す**。`stateRevision` と `history[].contentHash` は状態が動いた step では
  当然食い違い、許可表に実測値で載る。値の意味は薄いが、**状態が動かない step では一致しな
  ければならない**ので、「旧形入力では何も変わっていない」の一番強い証拠がここに出る。
- **許可表は path だけでなく値まで固定する**。path だけを許すと「ここは何が起きてもよい」に
  なり、記録が 2 件に増える退行（#39 が塞いだ形）を素通しする。表に無い差分が出ても、表に
  あるのに差分が出なくなっても落ちる。
- **corpus は件数で縛る**。case 数・step 数を test 側の定数と突き合わせ、case が黙って
  減っても落ちるようにする。
- **baseline が生成物であることを CI が確かめる**。fixture はただの JSON なので、手で書き換えて
  contract hash を作り直せば test は緑のまま通る（test が見るのは基準 sha の自己申告と件数だけ）。
  CI は基準 commit から `--output` で作業ツリー外へ生成し直し、committed fixture と `diff` する
  （`fetch-depth: 0` が要る）。残る穴は「比較面そのものを狭める改変」で、再生成も同じ狭い比較面を
  使うので byte 一致で通る。そこは §5 の変異（比較面を縮める 7 件）とレビューが見る。

実測: 差分が出たのは 29 step 中 **13 step**、**16 step は完全一致**。差分の内訳は #35 / #39 /
#43 / #44 と、それに従属する hash。1 件だけ**比較面を広げて初めて見えた差分**がある——
`restored-start-redelivery-ledger-miss` の重複判定で `diagnostics[0].detail` の文面だけが変わり、
判断・状態・台帳・hash はすべて一致していた。

**corpus が届いていない経路**（黙って間引かない）: 同じ session 内で `operationId` が衝突する
2 件を作る経路（start の再配送判定が先に倒す）、`assertSameScope` が投げる入力（例外は還元結果を
返さない）。上限 256 件の退避（§2.9）は generator が組み立てる corpus に入ったので、ここからは
外れた。`operation.phase === "progress"` も**到達不能ではなかった**——`kind` は開いた文字列で、
既知の phase 表にも非 operation 表にも無い adapter 固有 kind は envelope の欄検査だけを通って
還元器に入る。`restored-adapter-progress` として corpus に入れた（旧新は完全一致）。

## 5. 変異テスト（2026-08-18）

スクリプトは `harness/continuity/mutate.sh`（`bash harness/continuity/mutate.sh` で再現できる）。
各ゲートをわざと壊し、対応する test が落ちることを確認した。**218 件すべてで 1 件以上が失敗**し、
生存はゼロ、実行件数も期待どおり 218 件（黙って飛ばされた変異ゼロ）、復元後は 220/220 green
（`mutate.sh` が回すのは `reference-model.test.ts` と `old-shape-parity.test.ts` の 2 本。
`harness/continuity/*.test.ts` 全体は 330/330）。

**壊す対象は還元器だけではない**。§4.1 の差分ゲートは、還元器ではなく**比較面（`old-shape-projection.ts`）
と corpus（`old-shape-parity.json`）**を壊して検証する。門そのものの assert を壊す変異は、その門でしか
検出できないので kill できない——つまり変異の対象にならない。緩める方向（比較面から診断・状態を落とす）と、
corpus を実際より広く見せる方向（case を許可表から切り離す・再送を別の配送にすり替える・退避 case から
同名の兄弟を消す）の両方を入れてある。緩める方向は**面ごとに 1 件ずつ**置いた: 診断を落とす / 診断を
`code` だけに縮める / 状態を落とす / `stateRevision` を外す / 履歴を落とす / 台帳を鍵だけに縮める。
2 番目は codex のレビューが隔離環境で実証した退行そのものである（縮めた比較面では `detail` を壊しても
全 test が緑のままだった）。

**下の表は `mutate.sh` の出力から作る**（同スクリプトの header がそう宣言している）。ラベルを足し引き
したら、次で突き合わせてから doc を直す。CI は `mutate.sh` を走らせるが doc は見ないので、
この乖離を検出するのはこの手順だけ:

```sh
bash harness/continuity/mutate.sh > /tmp/mut.txt 2>&1
grep "ℹ fail" /tmp/mut.txt | sed 's/[[:space:]]*ℹ fail /|/; s/[[:space:]]*$//' \
  | grep -v '^|' | sort > /tmp/got.txt          # 実行の label|件数（末尾の復元行は label 無しなので落ちる）
grep -E '^\| .* \| [0-9]+ \|$' evidence/phase3-reference-model.md \
  | sed 's/^| //; s/ | /|/; s/ |$//' | sort > /tmp/want.txt
comm -3 /tmp/want.txt /tmp/got.txt   # 空でなければ乖離している
```

**突き合わせは実行の出力に対して行う**。以前はここに `mutate.sh` の `run` ラベルと doc のラベルを
比べる手順を書いていたが、それは**スクリプトと doc の 2 つの静的なテキストしか見ていない**ので、
件数の列がどれだけずれても一致と出る。実測: 上の形に直した初回で **45 行の件数が古かった**
（test を足すたびに各変異の fail 件数は増えるが、ラベルが変わらないので前の手順には映らない）。
ゲート自体は「生存 0」で守られていたが、**表の数字は測っていない値**になっていた。

**この harness 自身が 3 つの穴を持っていた**（外部のコードレビューが指摘し、実測で全部再現した）。どれも「壊していないゲートを kill として計上する」形で、**kill 率も実行件数も緑のまま嘘をつく**:

1. **置換後の文字列に書いた `\n` が改行にならない**。bash の二重引用符は `\n` を展開しないので、リテラルのバックスラッシュ n が TS に埋まり module が parse できなくなる。それでも node:test は「読み込みに失敗した 1 件」を fail として数えるので、`fail 1` だけを見ている `run()` は kill と判定した（**5 件が空証明**。うち 1 件は外部レビューで撤回した fail open の回帰ガードそのもので、そこが無検証だった）
2. **アンカーがソース中で一意でない**。`replace(old, new, 1)` は必ず先頭を書き換えるので、2 つ目の site を狙ったラベルは 1 つ目を二重に壊すだけになる（**3 件**。還元器と放棄で同じ形の guard を持つ箇所）。ラベル数しか数えない実行件数の突き合わせには映らない
3. **「変異で test が 1 件も走らなかった」を生存扱いにする safeguard が死んでいた**。node:text は読み込み失敗を fail 1 件として数えるので `[ -z "$n" ] || [ "$n" -eq 0 ]` が成立しない。1 と 2 がすり抜けたのはこれが原因

塞ぎ方は**個別のエントリ修正ではなく構造側**にした: `mutate()` は `s.count(old) == 1` を要求し（一意でないアンカーは即エラー）、`run()` は baseline の test 件数を測っておいて「変異が baseline と同じ件数の test を走らせたか」を突き合わせる。`\n` は `mutate()` 側で改行に解釈する。**さらに CI から呼ぶようにした**——人が手で叩いたときしか走らないと、アンカー外れも空証明も次のラウンドまで見つからない。8 件を実際に走る形へ直した結果、いずれも正しく kill された（ゲートは正しく、空だったのは証明のほうだった）。

kill 率より先に**実行件数**を見ること。変異はソース中の文字列アンカーで当てるので、実装を直すと
`assert old in s` が落ちて `&&` が短絡し、その変異は**出力に何も出ないまま黙って飛ばされる**
（round 12 で 3 件、round 13 で 1 件、round 15 で 2 件、round 16 で 1 件、round 17 で 9 件、round 18 で 1 + 4 + 1 件、round 19 で 11 + 3 + 1 件が外れ、いずれもこの自己検査が検出した。round 17 では**再構成で無意味化した変異**（`open` が `[matched]` と同一になり差が出なくなったもの）も生存として検出できた。round 21 でも 1 件出た: 再配送で `toolName` を無条件に上書きする変異は、`startConflictsWith` が「両方 present なら一致」を既に保証しているので上書きしても同じ値しか書けず観測できない。空虚な変異は差し替えでなく削除した）。この突き合わせはスクリプト自身が行うようにした: 末尾で
`実行 N / 期待 M、生存 K` を出し、**M ≠ N（黙って飛ばされた）か K > 0（生存した）なら非ゼロで
終わる**ので、kill 率を人が読んで判断する必要がない。期待件数はスクリプト自身の `run` ラベル数
から数える。変異でソースが壊れて test が 1 つも走らなかった場合も、そのゲートを検証できていない
点は生存と同じなので生存に数える。

この自己検査自体が発火することを確かめてある（先頭 1 変異だけに切り詰めた写しで実測）:

| 写し | exit | 出力 |
|---|---:|---|
| 変異 1 件、正常 | 0 | 実行 1 / 期待 1、生存 0 |
| アンカーが存在しない変異を 1 件追加 | 1 | 実行 1 / 期待 2、生存 0 |
| 何も壊さない変異を 1 件追加 | 1 | 実行 2 / 期待 2、生存 1 |
| 復元後の baseline を赤くする（BAK に壊れたソースを入れる） | 1 | `変異テスト失敗: 復元後の baseline が green でない` |

| 壊した箇所 | 落ちた test 数 |
|---|---:|
| dedupe 判定を外す | 6 |
| lastIngestSeq の max を外す | 2 |
| ingestSeq を数値比較にする | 2 |
| envelope 必須を外す | 3 |
| intake の attestation 必須を外す | 5 |
| caller の attestation を信じる | 1 |
| sourceAgent の束縛を外す | 2 |
| 認証できない経路でも Agent 名で落とす | 4 |
| session の束縛を外す | 3 |
| 空白の session 束縛を実在する名前として扱う | 1 |
| session を名乗れない経路でも session 名で落とす | 3 |
| 未認証の synthesized_monotonic を診断に出さない | 2 |
| 未認証の判定に version 一致まで求める | 1 |
| version authority が経路の認証を前提にしない | 10 |
| 空の Agent 名を素通しする | 5 |
| native turn の証明要求を外す | 7 |
| turn 証明の version 束縛を外す | 4 |
| turn 降格を黙って行う | 2 |
| turn 同一性の不変条件を外す | 5 |
| state への Agent 束縛を外す | 3 |
| 空 adapterDeliveryId の fallback を外す | 2 |
| rule 1 の排他を外す | 2 |
| rule 2 の turn 同一性要求を外す | 2 |
| 候補が複数のときの拒否を外す | 2 |
| terminal 側に matchKey 一致を要求し直す | 4 |
| identity 衝突を候補 1 件で判定する | 5 |
| terminal の canonicalInputHash 衝突検査を外す | 7 |
| identity 衝突の隔離を外す | 6 |
| kind と successful の矛盾を素通しする | 2 |
| 矛盾診断を照合済み経路だけに戻す | 1 |
| 矛盾した terminal を succeeded にする | 1 |
| start 不在の分岐を外す | 9 |
| terminal の権威順序検査を外す | 6 |
| 綴りの合わない順序材料を値として読む（空白・語彙外） | 2 |
| 空白の turn 種別を値として読む | 1 |
| 順序違反で候補を巻き込む | 1 |
| 候補ゼロの terminal を台帳に入れる | 34 |
| 順序不明で候補を unknown にしない | 7 |
| start の取り込み連番を記録しない | 63 |
| start の turn 種別を記録しない | 37 |
| 再配送 start でも順序材料を書く | 7 |
| 再配送 start を nativeOperationId で拾わない | 13 |
| 再配送の判定を matchKey にする | 18 |
| start の identity 衝突検査を外す | 10 |
| start の matchKey 衝突検査を外す | 2 |
| start の canonicalInputHash 衝突検査を外す | 3 |
| 放棄を session で絞らない | 2 |
| 候補の unknown 化を外す | 20 |
| unknown 化で証跡を残さない | 3 |
| sourceEventIds の上限を外す | 1 |
| pendingOperations の上限を外す | 13 |
| 退避対象から open を外す（詰まる） | 1 |
| 退避件数の上限を外す | 11 |
| 退避を黙って行う | 4 |
| revision ごとの配列分離を外す | 1 |
| 放棄経路の dedupe を外す | 2 |
| 台帳の keyspace 分離を外す | 4 |
| 空の capabilityHash を素通しする | 2 |
| 未知の sensitivity で fail open する | 1 |
| 空文字の任意欄を素通しする | 2 |
| start の operationKind 比較を外す | 1 |
| start の toolName 存在ガードを外す | 3 |
| 放棄 kind の制限を外す | 1 |
| 配送 ID 衝突の隔離を外す | 2 |
| sensitivity 集約を normal 固定にする | 7 |
| adapter 固有 kind の欄検査を外す | 1 |
| start の nativeOperationId 比較を外す | 1 |
| terminal の operationKind 比較を外す | 2 |
| terminal の toolName 存在ガードを外す | 4 |
| 放棄経路の配送 ID 衝突検査を外す | 1 |
| 空 canonicalFingerprint を素通しする | 2 |
| 確定済み成否との矛盾検査を外す | 4 |
| 成否を主張しない terminal も矛盾扱いにする | 4 |
| 成否が一致する兄弟の検査を外す | 2 |
| 放棄 kind を還元器に通す | 1 |
| 空文字の turnId を素通しする | 2 |
| 空文字の eventId を素通しする | 2 |
| sensitivity の下限に直前の集約値を使わない | 1 |
| 空文字の sessionId を素通しする | 3 |
| 空白文字を identity 材料として通す | 15 |
| 書式制御文字だけの identity 材料を通す | 8 |
| 空の operationMatchKey / operationKind を素通しする | 2 |
| open の選択を identity 互換に絞らない | 2 |
| canonicalInputHash の省略を照合可能として扱う | 2 |
| 再配送 start の session 検査を外す | 3 |
| 放棄で落とした証跡を報告しない | 1 |
| 直接呼びの envelope 検査を外す | 1 |
| 再配送 start の turn 検査を外す | 1 |
| 再配送 start の turn 存在ガードを外す | 5 |
| 候補 1 件でも照合不能ゲートを発火させる | 4 |
| 照合不能ゲートの候補数を 1 件ずらす | 2 |
| 照合不能を成否矛盾検査より先に判定する | 1 |
| 空白だけの capability hash を authority にする | 1 |
| 空白だけの Agent 名を authority にする | 1 |
| 空白だけの exact version を authority にする | 1 |
| 直接呼びの Agent 検査を外す | 3 |
| rule 2 の turn 種別の絞り込みを外す | 9 |
| turn 種別の材料が無い候補も落とす | 5 |
| 種別違いの巻き込み範囲を広げる | 1 |
| turn 種別が無いまま rule 2 を閉じさせる（FR-004） | 2 |
| turn を要求しない rule 1 まで種別で止める | 1 |
| 空白だけの turn 種別を材料として通す | 1 |
| turn 種別が無い候補を unknown に倒さず据え置く | 2 |
| 受領証 ID が空でも認証済みとする | 1 |
| peer identity が空でも認証済みとする | 1 |
| 空白だけの受領証 ID を authority にする | 1 |
| 空白の scenarioId で proven を成立させる | 1 |
| 直接呼びの ingestSeq 検査を外す | 2 |
| 直接呼びだけ scope を ingestSeq より先に見る | 1 |
| 直接呼びの identity 材料検査を外す | 3 |
| 空白の sourceAgent を素通しする | 1 |
| turn 両立ゼロの確定済みを適用済みにする | 9 |
| 矛盾判定の母数まで turn で絞る | 2 |
| 候補の unknown 化を operationId の等値で当てる | 3 |
| 放棄の適用先を operationId の等値で当てる | 2 |
| 確定済みの説明に turn 両立を求めない | 1 |
| 確定済みの説明で turn 種別だけ見ない | 1 |
| open の切り分けを turn 絞り込みより前にする | 16 |
| 矛盾判定に open な候補も混ぜる | 2 |
| 退避の保持判定を operationId の一致に戻す | 3 |
| 記録できる候補が居ても隔離を優先する | 1 |
| 抑止した矛盾を報告に残さない | 1 |
| 照合不能で turn 非両立の候補も巻き込む | 1 |
| rule 1 の候補が複数でも 1 件選ぶ | 2 |
| rule 1 の候補数を identity 絞り込み前で数える | 1 |
| 再配送 start の turn 種別を見ない | 2 |
| 降格した再配送 start も隔離する | 1 |
| 記録が降格されていても再配送を隔離する | 2 |
| 別 lineage の pending も再配送の相手にする | 2 |
| 放棄が別 lineage の operation も倒す | 1 |
| 退避で lineage 外を優先しない | 1 |
| 群の中を配列位置でなく startedAt で退避する | 1 |
| 再配送が持つ native id を記録に埋めない | 4 |
| 名乗っている兄弟が非互換でも空白へ埋める | 1 |
| 抑止の走査集合を session で絞らない | 1 |
| 候補を start の順序で絞らない | 2 |
| 全件順序不適合でも空に絞る | 2 |
| turnIdSource の語彙検査を外す | 2 |
| 候補の toolName を素で比べる | 3 |
| 適用済みの再配送も隔離する | 8 |
| 記録済みの event でも truncation を出す | 1 |
| 再配送 start の原因 event を残さない | 5 |
| 照合不能ゲートの母数を compatible に戻す | 3 |
| 放棄の配送衝突を診断に出さない | 1 |
| correlate の入口で terminal 相を要求しない | 1 |
| 兄弟から互換な候補を選ばない（derived id / native id 両方） | 25 |
| 再配送の相手を集合ごとに選ぶ | 8 |
| 全件衝突のとき衝突の証拠を持たない | 10 |
| native id が一致する兄弟へ帰属させない | 1 |
| 届いた start が native id を持たなくても帰属を動かす | 1 |
| 兄弟の連結順を入れ替える | 1 |
| truncation の対象を照合相手の外へ広げる | 1 |
| 再配送 start の truncation 対象を全 pending にする | 21 |
| IsoTimestamp の暦検査を外す | 5 |
| 受領証の時刻を暦検査から外す | 2 |
| provenance 不在を節で落とさない | 1 |
| 書く層で provenance 不在を落とさない | 1 |
| 書く層で受領証の時刻を検査しない | 2 |
| 空白の受領証時刻を暦違反として落とす | 1 |
| 時刻を名乗らない受領証を authority にする | 1 |
| 読む層で空白の受領証時刻を暦違反にする | 1 |
| 暦検査の前に綴りを当てない | 1 |
| offset の Z 固定を外す | 1 |
| 小数部の綴りを見ない | 1 |
| 任意欄の空白を present として読む | 16 |
| 空白だけ弾いて語彙外は比較へ渡す（#35 FR-004） | 1 |
| 状態側の空白 lineage を通す | 1 |
| event 側の空白 lineage を通す | 2 |
| 再配送 start の truncation 診断を落とす | 1 |
| 飛ばした衝突兄弟を報告しない | 4 |
| 記録を末尾から落とす（FR-008） | 4 |
| 記録の上限検査を外す（FR-015） | 5 |
| 記録の追加を別の診断で報告する（FR-009） | 13 |
| 記録の脱落を診断に出さない（FR-009） | 4 |
| 退避の記録で機密度を引き継がない | 1 |
| 孤児の記録を normal で残す | 4 |
| 孤児の記録を再送のたびに足す | 7 |
| 孤児の重複判定を eventId で行う（再送 DoS） | 4 |
| 重複判定で配送鍵を見ず指紋だけにする（§8.2 の順を崩す） | 1 |
| 配送鍵の無い記録を同一性なしにする | 1 |
| 孤児の記録に配送鍵を残さない | 8 |
| 退避の記録に兄弟を判別できる識別子を残さない | 4 |
| start を provenance 配列の先頭から取る | 1 |
| 刈っただけの修復を捨てる（FR-015） | 2 |
| 足せていなくても状態を進める | 7 |
| 同じ配送鍵の指紋食い違いを黙って重複にする | 1 |
| 材料が欠けていても指紋の食い違いにする | 168 |
| 孤児の記録に同一性の鍵を残さない | 7 |
| 候補ゼロの terminal を状態に記録しない | 17 |
| 開いた候補ゼロの unmatched を状態に記録しない | 2 |
| 退避を状態に記録しない | 7 |
| 上限超えの復元状態を刈った事実を黙る（FR-015） | 5 |
| 記録に触らない経路で記録を落とす | 3 |
| 復元状態の空配列をそのまま残す（FR-013） | 4 |
| 記録の配列を revision 間で共有する（§4.2） | 1 |
| 記録だけの隔離で watermark を進める（§4.1） | 4 |
| 呼び出し側が渡した watermark を無視する | 4 |
| 受理した terminal の指紋を残さない（FR-010） | 6 |
| unknown に倒した operation にも指紋を残す | 3 |
| 指紋の衝突検査を外す（FR-011） | 2 |
| 指紋が一致しても再配送として説明しない | 10 |
| 指紋を持たない旧い状態も衝突にする（FR-012） | 6 |
| 兄弟の 1 件が名乗っていれば全員分の衝突にする（FR-012 混在） | 1 |
| open な候補の指紋の食い違いを見ない（FR-011） | 2 |
| open な候補の空白の指紋を「違う指紋」と読む（FR-012） | 1 |
| unknown に倒れる terminal でも指紋の食い違いで隔離する | 1 |
| 指紋の衝突判定を順序材料がある場合だけにする | 1 |
| 指紋の衝突判定を rule 1 の terminal だけにする | 1 |
| 旧形 parity の比較面から還元結果の hash を落とす | 1 |
| 旧形 parity の比較面から診断を落とす | 1 |
| 旧形 parity の診断を code だけに縮める | 1 |
| 旧形 parity の比較面から状態を落とす | 1 |
| 旧形 parity の比較面から stateRevision を外す | 1 |
| 旧形 parity の比較面から履歴を落とす | 1 |
| 旧形 parity の台帳を鍵だけに縮める | 1 |
| 旧形 corpus の case 名を許可表から外す | 1 |
| 旧形 corpus の再送を別の配送にすり替える | 1 |
| 旧形 corpus の退避 case から同名の兄弟を消す | 1 |

「通るべきものが通る」側も対で置いている: 語彙外 kind の envelope、非 operation kind の envelope 無し、
turn 同一性の 3 通りの正しい組み合わせ、optional が全部無い状態の hash、turn が unavailable でも
rule 1 なら閉じること、上限に達していても start は必ず取り込むこと、上限に余裕があれば退避の
診断を出さないこと、proven な version の native turn と adapter の `synthesized_monotonic` は
降格しないこと、`capabilityHash` の不一致は evidence だけを降格させて turn には触れないこと、
同じ Agent の terminal は通ること、隔離した terminal も start を入れ直せば閉じられること、
巻き込まれなかった候補には証跡が付かないこと、negative fixture の `intakeContext` に欠落があれば
（何をしても synthesized になって intake の case が素通りするので）落ちること。

還元後の状態は `validateContractValue("CanonicalWorkStateV1", ...)` で凍結 schema に対して検証する
（terminal を 300 回投げても違反しないこと）。ゲートの test だけだと、還元器が schema 違反の状態を
出しても緑のままになる。
