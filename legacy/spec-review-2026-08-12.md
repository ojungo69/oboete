# 暫定仕様書レビュー報告 (2026-08-12)

対象: `agent-memory-final-spec-v5.md` / `agent-memory-implementation-spec.md` / `agent-memory-codex-preimplementation-review-v5.md`

レビュー体制: Claude Code による通読・差分分析 + 多レンズ並列監査(6視点、指摘ごとに独立反証2票以上で採用) + 外部事実検証エージェント3体(CLI hook実在性 / Cloudflare無料枠の算術 / codemem内部実態)。

> 状態: 完成(多レンズ監査・外部検証すべて反映済み)

---

## 1. 最重要: v5.0 仕様書が2つ併存し、内容が食い違っている

`agent-memory-final-spec-v5.md`(3202行)と `agent-memory-implementation-spec.md`(3330行)は、どちらも「v5.0」「実装前完成版」を名乗り、どちらも「旧文書(v3/v4)に優先する」と宣言していますが、**互いへの参照が一切なく、数十箇所で実質的な設計が食い違っています**。

これが問題になる直接の理由:

- Codexレビュー依頼文書(`agent-memory-codex-preimplementation-review-v5.md`)は **final-spec-v5 だけを読ませる**構成です。このままCodex壁打ちを実行すると、implementation-spec にしか無い改善(smart resume、MCP権限分離、subagent追跡など)が正典から漏れたままレビューされます。
- 両ファイルとも「致命的な未決定事項なし」「Blocking user decisionは残さない」と宣言していますが、下の差分表のとおり、**どちらの設計を採るか決まっていない項目が多数残っています**。この宣言は現状では成立していません。

**推奨**: Codex壁打ちの前に1本へ統合する。統合の土台は final-spec-v5(レビュー依頼が参照している側)とし、implementation-spec 側の優位な設計(§2の「impl優位」列)を取り込む。統合後、implementation-spec は旧文書リストへ移して削除またはアーカイブする。

## 2. 両ファイルの実質差分表(どちらを採るか決定が必要)

「推奨」は本レビューの意見です。◀ が付いている側の設計を推奨します。

| # | 項目 | final-spec-v5 | implementation-spec | 推奨と理由 |
|---|---|---|---|---|
| 1 | 優先順位の1位 | 作業再開(compact/crash復旧) | Agent間の記憶共有 | どちらでも設計は成立するが、全invariantの根拠になるため明文で1つに固定する。両文書の本文はどちらも「作業継続が最重要価値」と述べており、final側◀が本文と整合 |
| 2 | turn単位checkpoint | 作らない(SessionWorkState更新のみ。§11.3で乱造回避を明記)◀ | TurnCompletedごとに`kind=turn`のimmutable checkpoint作成+7日GC(§17.4) | final◀。mutableなSessionWorkStateがturn毎に更新され、crash時はそこからcrash_recovery checkpointを作れるので、turn毎のimmutable行は冗長で書き込み量だけ増える |
| 3 | resume方式 | SessionStartで未完了checkpointを無条件注入(§17.1) | `resume_mode=smart`: 新sessionはhintのみ→最初のprompt関連度で全文注入(§0.6, §17.1)◀ | impl◀。無条件注入は「別の作業を始めたのに前の作業の続きが挿入される」誤爆があり、smartの方がUXが正しい。same-session compactは両者とも全文復元で一致 |
| 4 | truth_state | 5値(`confirmed_wrong`あり)◀ | 4値。ただしUI/MCPに`mark_wrong`操作があり遷移先stateが未定義(内部矛盾) | final◀。implは`mark_wrong`の行き先が無いままなので5値に統一 |
| 5 | session durability保持 | 30日(§12.3) | 90日(§14.2) | どちらでも可。決めが必要なだけ。checkpoint accepted 90日と揃えるならimpl |
| 6 | 注入token予算 | 550/150-300/300-600、合計1400、上限1800(§17.2) | 500/700/700、合計1600(§17.8) | どちらでも可。1本化必須。checkpoint 550はやや小さく、implの700が現実的 |
| 7 | MCP書き込み権限 | `confirm`/`pin`/`mark_wrong`含む全actionを列挙、権限分離が弱い(§18.5) | Agentは`create`/`propose_*`のみ。confirm/pin/unpin/retractはUI/CLI限定+optimistic concurrency(`expected_revision`)(§19.3-19.4)◀ | impl◀。「memoryはinstruction authorityを持たない」原則(両文書共通)を実際に守るのはimpl側の設計。finalのままだとprompt injectionでAgentにpin/confirmさせる攻撃面が残る |
| 8 | subagent追跡 | capability宣言のみ。eventに親子情報なし | `agentInstanceId`/`parentSessionId`をeventへ付与+親turn完了前の昇格禁止invariant(§0.8, §10.4)◀ | impl◀。finalにはsubagentの混入防止規定が実質無い |
| 9 | raw event削除後の証拠 | 「metadata hashは残せる」のみ(§12.10) | `MemoryEvidenceSnapshot`正式entity+raw削除前のsnapshot生成をretention jobの前提条件化(§0.5, §14.7)◀ | impl◀。finalの書き方ではTTL後にmemoryの根拠が消え、provenance要件と矛盾する |
| 10 | session異常終了(abandoned)判定 | 「daemon起動時にstale active検出」のみ(§11.5) | host PID/boot ID/lease_until+5段階判定。liveness不明なら`idle`維持(§15.2)◀ | impl◀。finalの判定では「PCを数日離れただけ」をabandoned誤判定してcrash_recovery checkpointを乱造し得る |
| 11 | free認定の失効 | 失効概念なし(§13.8) | `certification_expires_at`+30日+価格/quota変更で再検証(§12.11)◀ | impl◀。無料枠は変動するので期限付き認定が誠実 |
| 12 | SessionLineage | 正式entity+table(§10.2)◀ | 相当entityなし(checkpointのparent参照のみ) | final◀。Agent切替・resume・compactの系譜はcheckpoint参照だけでは再構成できない |
| 13 | Generation role | 6role(`rolling_summary`あり) | 5role | どちらでも可。rolling_summaryを本当に使うか次第(§7の削減候補も参照) |
| 14 | EventKind | `session_idle`/`session_interrupted`あり | `interrupted`のみ | 統一が必要。idleをeventにするかscheduler判定にするかの決め |
| 15 | checkpoint schema | `goal`等のsemantic fieldがrequiredトップレベル(§11.2)。§10.4の「semanticはoptional」と自文書内で緊張 | `canonical_state_json`(観測のみ)と`semantic_resume_note_json`(AI派生・任意)を分離(§13.4-13.5, §15.6)◀ | impl◀。「復旧をLLMに依存させない」という両文書共通の根本原則を型で保証するのはimpl側。finalは`goal: string`(required)がLLM無しで埋められない |
| 16 | compact戦略enum | `session_compaction_event`含む5値 | 4値 | 統一が必要。OpenCode系のsession.compactedを使うならfinalの5値 |
| 17 | Local API | `/v1/resume/select`等(§19.5) | `/v1/resume/resolve`+`/v1/resume/accept`、`/v1/backup/verify`等(§9.5) | endpoint集合が別物。統一必須(どちらでも可、implの方が対称性あり) |
| 18 | 自己再取り込み防止 | `injection_id`/`generation`属性付きmarkerでID基点の除外(§14.8, §17.4)◀ | 属性なしmarker2種のみ(§17.9) | final◀。IDが無いとinjection ledgerと突き合わせできず、echo検出テストも書きにくい |
| 19 | Hard Invariants | 25項目 | 34項目 | 集合が実質不一致。統合時に和集合を取り、番号を振り直す必要 |
| 20 | 実装Phase順序 | 信頼性→**continuity(P2)**→provider(P3)→検索→adapter | provider/embedding(P1)→信頼性(P2)→**continuity(P3)**→adapter | **プロジェクトの主目的が「作業再開」なら continuity を先に作るfinal◀が一貫**。implはprovider契約を先に固める順。PR系列も両者で別物なので、採った側に完全に揃える |
| 21 | benchmark corpus | 120 sessions(45/45/30) | 100 sessions(40/40/20) | 規模自体を見直し推奨(§7)。数字の1本化必須 |
| 22 | sync対象 | checkpointを無条件sync、「SessionWorkStateのvolatile詳細」のみlocal-only(曖昧) | SessionWorkState全体local-only+「task lineageごとのlatest open/delivered checkpoint」のみsync◀ | impl◀。同期対象が明確で転送量も小さい |
| 23 | config schema | `generation.roles`等 | `model_roles`等 | 統一必須(実装前に決めないとadapter/インストーラが割れる) |

このほか injection envelope のtag名、`injectedContextIds`フィールド有無、Codexレビュー依頼の出力形式が参照する章立てなど、細部の不一致が多数あります(統合作業時に機械的に潰せるレベル)。

## 3. 単体としての仕様欠陥(統合後の正本で直すべきもの)

自前分析で確定した分。多レンズ監査の追加分は§4に追記。

### 3.1 契約の穴

- **idempotencyKey の生成規則が未定義**。「spool前に確定する」とだけあり、何から導出するか(agent + nativeSessionId + nativeToolUseId + kind + payload hash 等)の規則が無い。at-least-once配送+冪等処理という信頼性設計の根幹なので、導出式を仕様に明記しないとadapter実装ごとに割れて重複memoryが発生する。
- **spool上限到達時の新規イベントの扱いが未定義**。「古いeventを無言削除しない」「警告する」とあるが、上限到達後に来た新イベントをdropするのか、blockするのか、上限を超えて書くのかが書かれていない。「acceptedまたはspooledされたeventを失わない」invariantと衝突する箇所。
- **daemon local API(/v1/events等)の認証が未定義**。loopback/UDS限定とだけある。同一マシンの他プロセスが偽イベントをPOSTしてmemoryを汚染できる(=将来の全sessionへの間接prompt injection)。UDS/named pipeはファイル権限で守れるが、loopback TCPを使う場合(WSL bridge等)はWeb UIと同様のtoken必須と明記すべき。
- **daemonの単位が不明記**。global 1台(data_dir単一)想定に読めるが明記なし。複数プロジェクト並行時の挙動・多重起動防止はここが前提になる。

### 3.2 プライバシー/セキュリティ

- **`<private>`タグのecho漏れ**: user promptの`<private>`本文はadapterが落とすが、assistantがその内容を引用して回答すると`assistant_completed`のcaptureに同じ内容が載る。多層防御(secret detector)で拾える保証はない。「private指定された文字列のn-gramをsession内suppressリストに載せる」等の対策を仕様に一言入れる価値あり。
- **secret検出器の具体が未指定**。「deterministic secret detection」と繰り返すのみ。既存OSS(gitleaks系ruleset等)を使うのか、自作regexなのか、日本語混在テキストでの誤検出方針は何か。ここが弱いと「secret leak 0」というrelease gateの判定自体が定義できない。
- **sidecar(Claude/Codex subscription)の規約リスクの明示不足**。「documented interfaceのみ使用」は書かれているが、ヘッドレス自動呼び出しを大量に行うこと自体がprovider規約と衝突し得るリスクを、Zero Incremental Cost profileのユーザー向け表示に含めるべき。

### 3.3 性能目標の実現性

- **adapter delivery p95 < 50ms は hook をNodeで書くと未達がほぼ確定**。Nodeプロセスの起動だけで50〜100ms掛かる。hookあたりの起動コストを誰が払うか(常駐daemonへ送る軽量バイナリ/シェル、bun compile、等)の実装戦略が仕様に無い。全tool callに毎回100ms載ると体感が悪化し、「memory障害はcoding Agentを止めない」の精神に反する。目標値を維持するならhook実装言語の制約を仕様へ、緩めるなら目標値を現実に合わせる。

### 3.4 release条件の構造リスク

- **5 Agent同時Tier AがCore 1.0の条件**になっており、1つのCLIのnative制約(hookが無い等)でリリース全体がblockされる。例外条項はKimiのみ。後述の外部検証(§5)の結果次第では、この条件自体を「Claude+Codex(またはClaude+OpenCode)でCore 1.0、残りは1.x」へ緩める判断を推奨。
- **評価体制が研究プロジェクト級**: 100-120 session corpus、複数annotator、hidden holdout、CMEM black-box比較、をすべて個人で維持するのは工数的に最重量級。しかもn=100前後で「-2%以内の非劣性」を統計的に主張するのは検出力不足。品質claim(「claude-mem/CMEM同等以上」)を初期リリースで諦めれば、この体制ごと大幅に縮小できる(§7の判断事項)。

## 4. 多レンズ監査の確定指摘(反証2票を通過したもの)

6レンズ(整合性/状態機械/実現可能性/セキュリティ/スコープ/評価設計)の並列監査で候補52件を検出し、独立した反証エージェント2視点(事実正確性・重要性)の両方を通過した**7件のみ**を確定として採用しました(45件は「knownと重複」「仕様の別所で手当て済み」「実装時に自明」等で却下)。全て行番号付きの根拠があります。

### 4.1 [HIGH] checkpoint重複配送: `delivered`が除外されず、取得のclaim機構も無い(final-spec §11.6)

Claude CodeとCodexを同時に起動すると、両方が同じopen checkpointを取得・注入できてしまう。§11.6の除外リストは accepted/superseded/expired のみで **delivered が入っておらず**、`/v1/resume/select` にclaim/lease機構が無い。両sessionがそれぞれ最初のturnを完了すると `acceptedBySessionId`(単一値)が後勝ちで上書きされる。
**修正案**: resume/selectをDBトランザクション内の条件付きclaim(open→delivered + deliveredToSessionId + lease期限)にし、lease有効なdeliveredを他sessionの候補から除外する。

### 4.2 [HIGH] checkpoint statusをsyncのrevisionとして流すと、2台同時resumeが「ユーザー解決待ちconflict」になる(final-spec §22.4/§22.8)

checkpointはdefault sync対象で、syncは「same parentから複数revision = concurrent conflict、explicit resolutionで解決」という設計。すると laptop と desktop で同時にresumeしただけで、双方の open→delivered 遷移が同一parentからの2 revisionとなり、**日常操作が毎回conflictを生む**。conflict状態のcheckpointをresume候補としてどう扱うかも未定義。
**修正案**: statusをbody revisionから分離してdeviceローカルの配送状態にする(syncは本文とaccepted終端のみ)か、statusに限り単調merge(open<delivered<accepted)を§22.8の例外として定義する。

### 4.3 [HIGH] snapshotにtombstoneが含まれるか未規定 — 削除済みmemoryが復活する経路(final-spec §22.11)

長期offline端末の復帰シナリオ: 削除op発行→log compaction→復帰端末がtombstone無しのsnapshotをimport→ローカルに残る旧entityを「未sync」としてpush→**削除したはずのmemory(secretを含み得る)が全端末へ復活**。snapshot内容は「canonical materialized rows」としか規定されておらず、tombstoneのretention期間も未規定。
**修正案**: snapshotへのtombstone行の必須含有を明記し、tombstone retentionをcompaction retention windowより長く固定する。

### 4.4 [HIGH] SessionWorkState/ContinuationCheckpointに`sensitivity`が無く、remoteルーティングのゲートが構造的に効かない(final-spec §10.3/§11.2、implも同様)

provider routingの判定基準1位は「sensitivity / remote eligibility」だが、observation extractionやcheckpoint refinementでremoteへ渡される **SessionWorkState/checkpoint自体がsensitivityフィールドを持たない**。private指定のprompt由来の内容がwork state経由でremote providerへ流れることを、仕様上のゲートで止められない。raw eventのTTL(14日)はcheckpoint保持(30-90日)より短いため、後から遡って導出することも不可。
**修正案**: 両entityに集約sensitivity値(構成要素の最大機密度)を必須で持たせる。

### 4.5 [MEDIUM] supersede条件「同じwork lineage」が未定義で、checkpointにlineage識別子が無い(final-spec §11.7)

Codexでtask Xのcheckpoint C1(open)、Claude Codeで同一branchの別task YのC2を作った時、C2がC1をsupersedeするか判定する材料がschemaに無い。実装が広く解釈すると**別タスクの未完了作業が無言で失われる**。
**修正案**: checkpointに`taskLineageId`(初回採番、resume acceptで継承)を追加し、supersedeを同一lineage内に限定(impl-spec §18.4の並行checkpoint規則の取り込みとセット)。

### 4.6 [HIGH] impl-spec内部でpinnedのデータモデルが三様(impl §13.6/§14.2/§15.3/§19.3)

durability enumは3値(pinned無し)、retention表はpinnedをdurability段階扱い、§19.3は独立field `pin_state` を要求、DB schemaにはその列が無い。**pin をenum値にするか独立fieldにするかはmigration級の決定**なので実装前に一本化が必要(final-specは4値enum方式で、これも非互換)。

### 4.7 [MEDIUM] impl-specには`<private>`/`<local-only>`タグ機構が存在しない(impl全文)

final-spec §9.3のuser markup機構に相当する記述がimpl側にゼロ。impl-specだけを読んで実装するとprivate markupが仕様から消える。§1の統合必要性を裏付ける追加証拠。

## 5. 外部事実検証の結果

### 5.1 Cloudflare無料枠の算術検証(完了)

公式docs(2026-08-07更新のpricingページ等)からの検証結果。仕様書の「Cloudflare Workers AI = 無料observer第一候補」という想定に**実質的な制約**が見つかりました。

**(a) Workers AI Free 10,000 Neurons/日 vs observer負荷**

observer 1リクエスト = input 12,000 + output 1,000 tokens(仕様書のbudget)で計算:

| モデル | Neurons/リクエスト | 処理可能リクエスト数/日 |
|---|---|---|
| llama-3.2-1b-instruct | 47.7 | 約209 |
| llama-3.2-3b-instruct | 86.0 | 約116 |
| llama-3.1-8b-instruct-fp8-fast | 84.3 | 約119 |
| gemma-4-26b-a4b-it | 136.4 | 約73 |
| llama-3-8b-instruct(full precision) | 382.4 | 約26 |

→ **コーディング実働日を1日100〜300 turnとすると、300は全モデル未達。100でも8b級でマージンほぼゼロ**。出力token単価が入力の6〜8倍で、出力コストが支配的。

**(b) json_schema(構造化出力)対応モデルの制約**: json_schemaモードは限定モデルのみで、**安価な1b/3bは非対応**。対応リストは8b級以上が中心。つまり「安いモデルでNeuronを節約しつつスキーマ強制」は両立しない可能性が高い。pricingページの`llama-3.1-8b-instruct-fp8-fast`とjson-modeページの`llama-3.1-8b-instruct-fast`が同一SKUかはdocsから確定できず(ここが一致すれば約119リクエスト/日で構造化出力が可能、不一致なら最悪26まで低下)。実装時に実機確認が必要。

**(c) 成立が確認できたもの**: OpenAI互換endpoint(chat/embeddings)、embedding(bge-m3が多言語100+言語対応・約1,075 Neurons/M tokensで実質無視できるコスト)、SQLite Durable Objects Free枠(100k requests/日、5M rows read/日、100k rows written/日、PITR 30日、FTS5サポート)、FTS5書き込み増幅(1件あたり目安3〜6行の増幅でも100k rows written/日に対し余裕。ただし倍率は公式非公開のため構造からの見積り)。

**(d) 仕様書が参照する「1GB write失敗 vs 10GB/object」の矛盾は現行docsにも実在**(GA changelogは10GB、概念ページの脚注が1GBのまま)。ただしFree planは**アカウント全体で5GB total**が先に効くため、per-object 800MB保守運用に加えてaccount totalの監視が実務上の本命(仕様書には account total 監視の記述あり — 妥当)。

**仕様への影響**:

1. 「Cloudflare Workers AI Freeをfree-certified第一候補」は、**observer budget 12k input のままだと成立圏が狭い**。budgetを6k程度へ縮小すればリクエスト/日は概ね倍増し、また仕様のトリガー設計(turn completed / idle / 20 events)を活かして複数turnを1 digestへbatchingすれば、300 turn日でもリクエスト数を100未満に圧縮できる。**「turn≒1リクエスト」前提を見直し、free profileでは積極batching+縮小budgetを既定にする**旨を仕様に足すべき。
2. free-certified gateの「typical daily workload」を**数値で定義**(例: 200 turn/日、observer 80リクエスト/日相当)しないと、この合否判定は実行できない。
3. モデル選定の探索空間は「json_schema対応 × Neuron単価 × 品質」で事前に絞れる。benchmark前にモデルを固定しない方針は正しいが、候補は実質llama-3.1-8b系(fp8-fast)+テキストparse許容なら3b、という狭さをbenchmark計画に織り込むべき。

### 5.2 CLI hook実在性(完了)

5 CLI全てについて、現行source(GitHub main / ローカル実機バイナリ)・公式docsで capture / **注入surface** / compact検知 / session ID を検証。要点:

**(a) Tier A×5 の成否 — 崩れるのは Kimi Code の1セルだけ**:

| CLI | capture | session開始時注入 | prompt毎注入 | compact前後 | 判定 |
|---|---|---|---|---|---|
| Claude Code | ◎(31 events) | native | native | PreCompact + **PostCompact両方実在**(SessionStart matcher=compactも別途あり) | Tier A |
| Codex CLI (v0.147, source一致確認) | ◎(11 events) | **native**(`additionalContext`) | **native** | Pre/PostCompact native | Tier A |
| Pi | ◎ | 実質native(`before_agent_start`がsystemPrompt+message注入可) | native | session_before_compact / session_compact | Tier A(注入面は5本中最強) |
| OpenCode | ◎ | **合成必要**(初回メッセージ判定。本番plugin `opencode-supermemory`が実証済み) | native(`chat.message` parts注入) | experimental.session.compacting + session.compacted | Tier A(合成前提+experimental版pin必須) |
| Kimi Code | ◎(20 events、唯一`Interrupt`あり) | **unsupported**(ソース確定: 注入経路は`UserPromptSubmit`のstdout 1本にハードコード) | native | Pre/PostCompact(captureのみ) | **nativeではTier A不成立** |

**(b) 仕様への最小修正案**: Tier Aの定義を「session開始時に*nativeで*注入」ではなく「**session開始時点で確実に注入が届くこと(初回プロンプトでの合成を許容)**」へ緩める。この定義なら5本全てTier Aになり、Kimi例外条項の発動も不要。逆にnative縛り維持ならKimiは明示Tier B。

**(c) Codexの過小評価を修正すべき**: 仕様§10.5の記述は正しいが、CodexはClaude Code以外で唯一**両方の注入面をnativeに持つ**(+PermissionRequest/SubagentStart/SubagentStop hookもあり)。「Claude+もう1つ」で先行リリースする場合の相方はCodexが最有力、という設計判断の根拠になる。

**(d) 仕様書に無いギャップ**: Piはevent payloadにsession IDが無く`ctx.sessionManager.getSessionId()`経由(仕様の「stable session ID」能力表に未記載の注意点)。Piの「session resume lifecycle」はresume専用イベント未確認でpartial。OpenCodeのsession_end専用イベントも未確認(`session.deleted`/`dispose`で代替)。未確定はPi/OpenCodeのinterrupt、OpenCodeのsession_endの3セルのみ(いずれもmemory adapterには重要度低)。

### 5.3 codemem内部実態(完了)

GitHub上の実ソースを直接読んで仕様書の前提8項目を検証した結果、**全て事実と確認**。仕様書のcodemem認識は正確です。fork判断に効く発見:

**(a) 仕様が「削除必須」とする2経路は実在し、場所も特定済み**:
- **direct DB fallback**: `packages/cli/src/commands/claude-hook-ingest.ts`(および`codex-hook-ingest.ts`)に「HTTP first, direct DB fallback」というコマンド説明文ごと実装されている。daemon(viewer)が落ちていると、hookが直接better-sqlite3でraw_eventsへINSERTする。仕様の「atomic spool統一」はこのfallbackをspool書き込みへ置換する改修として素直に位置づく。
- **unsafe auth path**: `packages/core/src/observer-auth.ts`が**OpenCodeの認証キャッシュ(~/.local/share/opencode/auth.json)からOAuth tokenを抽出**し、`observer-client.ts`が**`chatgpt.com/backend-api/codex/responses`(非公開consumer backend)**へSSEで直接リクエストする実装。Anthropic側にもOAuth token転用(`anthropic-beta: oauth-2025-04-20`)がある。仕様書§5.3の禁止事項リストは、このコードを名指ししたものとして正確。
- **注意点**: この経路は`observer-client.ts`(91KB)と密結合しており、隔離は「ファイル削除」ではなく相応の設計作業になる。Phase 0の見積りで「fork継続条件: unsafe pathが分離不能なら再審議」とあるが、実態は「分離可能だが軽くない」。

**(b) sole-writer化の実作業が判明**: 現状は最低3系統が同一SQLiteへ直接接続する — viewer-server(daemon本体)、**MCP serverが独自接続を持つ**(`packages/mcp-server/src/stdio.ts`が「Owns its own better-sqlite3 connection」と明言)、CLI hookのfallback。仕様は「adapterはDBへ直接writeしない」と書くが、**MCP serverの接続をどうするか(write専用でdaemon経由化 / readは直接許可)の言及が薄い**。sole-writer移行の実体は「CLI fallback→spool置換」+「MCP server write経路のdaemon化」の2作業。仕様§4.2の必須変更リストへ明記推奨。

**(c) fork baseとしての成熟度は想定以上**: pnpm monorepo 8パッケージ、**テスト187本**、CI 7ジョブ(型・lint・単体・Cloudflare Worker統合・pluginスモーク・E2E×3)、設計ドキュメント100本超。npm公開済み(codemem 0.40.2ほか3パッケージ、MIT)。EmbeddingClient(@xenova/transformers + bge-small-en-v1.5、FTS fallback付き)、sqlite-vec 0.1.9、FTS5、tier routingも仕様書の記述どおり実在。「codemem forkが本当にgreenfieldより小さいか」というCodex壁打ち論点に対しては、**fork優位を支持する材料**。

## 6. Codexレビュー依頼文書への指摘

- 依頼書の必須出力「2. Hard Invariant matrix」は「仕様書の全Hard Invariant」を評価させるが、2ファイルでinvariant集合が違う(25 vs 34)。統合後の正本を前提にしないと成立しない。
- 「最重要レビュー項目 B」の表項目に`session_idle`系が入っているが、implementation-specにはそのeventが無い。統合とセットで依頼書も更新が必要。
- 依頼書自体の構成(反証原則、inference/unknown区別、禁止事項)は良くできており、統合後はそのまま使える。

## 7. ユーザーに決めてほしいこと(最終版)

いずれも「どちらでも実装可能だが、決めないと先へ進めない/後から変えると高くつく」判断です。各項目に推奨を付けています。

1. **仕様の統合**(§1・§2): final-spec-v5を土台に、§2で「impl◀」を付けた項目(smart resume、MCP権限分離、subagent追跡、証拠snapshot、lease付きsession判定、free認定期限、sync対象の明確化)を移植し、§4の確定7件の修正案を反映した統合版v6を1本作る。implementation-specは旧文書としてアーカイブ。**この統合作業を私(Claude Code)が行ってよいか**(その後にCodex壁打ちを実行するのが正しい順序)。
2. **Core 1.0のAgent数**(§3.4・§5.2): 推奨は**段階release**: Claude Code + Codex でCore 1.0(検証の結果、CodexはClaude以外で唯一両注入面nativeの最有力相方)、OpenCode/Pi/Kimiは1.xで追加し25-path gateはPlatform 1.0側へ移す。5同時を維持する場合はTier A定義を「session開始時点で確実に注入が届く(初回プロンプト合成を許容)」へ緩和する必要あり(§5.2(b)、Kimiがnativeでは不成立のため)。
3. **品質claim**(§3.4): 「claude-mem/CMEM同等以上」を初期リリースの必須gateにするか。推奨は**v1では claimを外し内部評価(小規模corpus+自動指標)に留める** — 100-120 session+複数annotator+hidden holdoutの維持は個人開発で最重量のコスト項目であり、統計的にもn=100前後で-2%非劣性は主張しづらい。claimは後から積める(認定は期限付き設計なので整合的)。
4. **無料observer戦略**(§5.1): Cloudflare Workers AI freeを第一候補に保つ場合、(a)observer入力budgetを12k→6k程度へ縮小、(b)turn毎でなくbatching前提、(c)typical daily workloadの数値定義(例: 200 turn/日)、の3点を仕様へ反映することを推奨。json_schema対応×単価の制約からモデル候補は実質llama-3.1-8b系に絞られる点も評価計画へ。
5. **Web UI / WSL bridgeのスコープ**: Web UI 15 viewsのうちdebug/ops系はCLI(doctor)で代替してviewer最小構成に、Windows/WSL bridgeはCore 1.0から外しseparate-device+syncで代替(bridgeは需要が実証されてから)、を推奨。
6. **仕様内の残る決め**(§2の「どちらでも可」項目): session保持30日vs90日、token予算、EventKind集合、Local API endpoint名、config keys — 統合作業時に一括で確定する(私の推奨値は§2の表に記載済み)。

## 8. 検証プロセスの記録

- 多レンズ監査: 6レンズ並列 → 候補52件 → 独立反証2視点(opus)で7件確定/45件却下。総計110エージェント、エラー0。
- 外部事実検証: CLI hook(5本、現行source+実機binary確認)、Cloudflare公式docs(算術付き)、codemem実source(8 claim全verified)。
- 本レビューはファイルを一切編集していません(この報告書の新規作成のみ)。
