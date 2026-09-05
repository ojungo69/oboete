# Feature Specification: 証拠 digest による real-cli-e2e 昇格の裏付け

**Feature Branch**: `feat/evidence-hash-normalization`

**Created**: 2026-08-18

**Status**: Draft

**Input**: GitHub issue #20「harness: evidenceHash を正規化抜粋 SHA-256 で生成・照合し、自己申告 tier 昇格を防ぐ」（priority: p0 / area: capability / target: core 1.0）

---

## 背景（origin/main 5bbf292 の実測）

issue #20 の状況記述は正確だった。実測で確認した内訳は次のとおり。

| 事実 | 出典 |
|---|---|
| `evidenceHash` は capture fixture の**任意**欄として既に存在する（`^[a-f0-9]{64}$`）。値の裏取りは無い | `harness/schema/capability.schema.json:145-148` |
| 証拠強度は `Boolean(fixture.evidenceHash)` だけで決まる。64 桁 hex を書けば `real-cli-e2e`、書かなければ `source-test` | `harness/assemble.ts:280`, `:349` |
| 昇格箇所は **3 箇所**ある。capture cell、highLevel cell、prompt 対を証明した fixture で provenance を差し替える再刻印 | `harness/assemble.ts:280`, `:349`, `:383` |
| committed fixture で `evidenceHash` を申告しているものは 1 件も無い | `grep '"evidenceHash"' harness/fixtures/` が 0 件 |
| 現在の matrix は `real-cli-e2e` 0 件・`source-test` 21 件（Claude 12 / Codex 9）。既存成果物に偽の証跡は無い | `harness/matrix/claude.json`, `harness/matrix/codex.json` |
| `harness/fixtures/{claude,codex}/raw/*.jsonl` に隔離 rig で取得した観測記録が 16 件ある。これを読むコードは 1 行も無い | `harness/assemble.ts:270` のコメントが自認している |
| 隔離の自己申告 `rig.isolated !== true` は既に弾いている。塞がっていないのは digest の裏取りだけ | `harness/assemble.ts:140` |

つまり穴は「hash を書けば昇格する」1 点で、既に汚染された成果物は無い。実 CLI capture rig を
有効化する前の今が、証拠を伴わない昇格経路を閉じる最後の機会になる。

16 件の観測記録は本物だが、取得当時の rig 一時領域が既に存在せず、run の素性を示す記録を
後から真正に作ることはできない。したがって**この作業で昇格する cell は 0 件**になる。
目的は経路を塞ぐことであって昇格させることではない。既存の記録も digest で結び付くので、
差し替えれば組み立てが落ちるようになる。

## User Scenarios & Testing *(mandatory)*

### User Story 1 - 証拠のない tier 昇格を止める (Priority: P1)

capability matrix を読む実装者は、ある cell が `real-cli-e2e` と書いてあれば「隔離環境で実 CLI を
動かして観測した」と理解する。現在はその記載に裏付けが無く、fixture を 1 枚書き足すだけで最高位の
証拠強度を名乗れる。fixture が指す実物の観測記録から digest を計算し直して一致した場合にのみ
`real-cli-e2e` を刻むようにする。

**Why this priority**: 証拠強度は harness の存在理由そのもの。ここが自己申告で通ると、matrix 全体が
「誰かがそう書いた」以上の意味を持たなくなり、後続の実装判断がすべて根拠を失う。

**Independent Test**: 実在しない観測記録を指す fixture、および観測記録はあるが digest が食い違う
fixture を用意し、いずれも `real-cli-e2e` にならず組み立てが失敗することを確認する。

**Acceptance Scenarios**:

1. **Given** 形式は正しい 64 桁 hex を自己申告した fixture があり、対応する観測記録が存在しない、
   **When** matrix を組み立てる、**Then** 組み立ては失敗し、その fixture の cell はどの成果物にも
   `real-cli-e2e` として現れない
2. **Given** 観測記録は実在するが fixture が申告した digest と一致しない、**When** matrix を
   組み立てる、**Then** 組み立ては失敗し、不一致であることが理由として示される
3. **Given** 観測記録が実在し digest も一致し、run の素性を示す記録を伴い、cell の主張が記録から
   導けて申告値と一致する、**When** matrix を組み立てる、**Then** その cell は `real-cli-e2e` として
   記録される。再計算が一致した記録が run の素性を示す記録を伴わない、または cell の主張が
   記録から**機械的に導けない種類**である場合は `source-test` に留まる。導ける種類なのに記録が
   申告値を支持しない場合は組み立てを失敗させる（記録の不在・読取失敗・digest 不一致・未知の版
   も 1・2・4 のとおり失敗。FR-005）
4. **Given** fixture が申告した正規化版番号を harness が知らない、**When** matrix を組み立てる、
   **Then** 組み立ては失敗する（未知の版を「たぶん同じ」と見なして続行しない）

---

### User Story 2 - 別環境で取り直しても同じ digest になる (Priority: P2)

rig を動かす人は、同じ scenario を別のマシン・別の日に取り直す。session 識別子・時刻・作業ディレクトリの
絶対 path・モデルが書く文面は毎回変わるので、記録全体をそのまま指紋にすると再取得のたびに不一致になり、
検査が「毎回 digest を書き換える儀式」に堕ちる。環境差だけを取り除いた抜粋から digest を作る。

**Why this priority**: これが成立しないと US1 の検査は運用で必ず無効化される。ただし US1 の
「証拠が無ければ昇格しない」だけでも単独で価値があるため P2。

**Independent Test**: 同じ scenario の 2 回の取得記録が同じ digest になり、別 scenario の記録が
別の digest になることを、committed 済みの観測記録だけで確認できる。

**Acceptance Scenarios**:

1. **Given** 同一 scenario を 2 回取得した記録があり、両者は session 識別子・時刻・transcript path・
   作業ディレクトリだけが異なる、**When** それぞれの digest を求める、**Then** 2 つは一致する
2. **Given** 投入した指示が異なる 2 つの記録がある、**When** それぞれの digest を求める、**Then**
   2 つは一致しない
3. **Given** 観測された event の並びが異なる 2 つの記録がある、**When** それぞれの digest を求める、
   **Then** 2 つは一致しない
4. **Given** 2 つの記録が、観測できる範囲では完全に同一である（差が観測に現れない）、**When**
   それぞれの digest を求める、**Then** 一致してよい。ただしその一致は「取り違えても通る」ことを
   意味しないよう、fixture 側が自分の観測記録を名指しし、その記録から digest を計算する

---

### User Story 3 - 出所を追え、秘密は漏れない (Priority: P3)

証拠を後から検証する人は「どの CLI の正確な版で、どの scenario を、どの観測記録から、どの正規化規則で
判定したか」を成果物だけで辿れる必要がある。同時に、観測記録には作業ディレクトリの絶対 path・
投入した指示・モデルの出力が含まれるため、成果物や診断出力・CI ログへそのまま出してはならない。

**Why this priority**: 追跡可能性と秘密の非漏洩は必須だが、US1/US2 が無いと記録する対象そのものが
存在しない。

**Acceptance Scenarios**:

1. **Given** 組み立てが成功した、**When** 成果物を読む、**Then** 証拠の所在・正規化の版・digest・
   scenario の識別子・CLI の正確な版が判別できる
2. **Given** 観測記録に絶対 path・投入指示・モデル出力が含まれる、**When** 成果物・診断出力・CI ログを
   検索する、**Then** それらの本文は現れない
3. **Given** 観測記録を差し替えたのに fixture 側の digest を更新していない、**When** 組み立てる、
   **Then** 明示的な失敗になる（古い digest が黙って残らない）

---

### Edge Cases

- 観測記録の path に `..` や絶対 path を書いた fixture → 許可された証拠置き場の外を読ませない。解決を拒否して失敗する
- 証拠置き場の中を指しているが symlink で外へ出る → 実体解決後も置き場の内側であることを確認し、外なら失敗する
- 観測記録が空、または 1 行も解釈できない → 失敗する（「対象 0 件だから成功」にしない）
- 観測記録の 1 行が壊れた JSON → 失敗する（壊れた行を読み飛ばして残りで digest を作らない）
- 観測記録に harness が知らない新しい欄が増えた → 欄の存在自体が digest に反映される。値は既定で伏せ字になるため、新しい欄の中身が成果物へ漏れることはない
- 同じ観測記録を複数の fixture が指す → 許容する。digest はそれぞれの記録から計算されるため、取り違えは fixture の名指しで防ぐ
- 証拠強度が `real-cli-e2e` 以外（`official-doc` / `source-test`）の cell → digest を要求しない。要求するのは実 CLI 観測を名乗る cell のみ

## Requirements *(mandatory)*

### Functional Requirements

**証拠の結び付け**

- **FR-001**: 実 CLI 観測を根拠とする fixture は、対応する観測記録を機械可読な形で名指ししなければならない（MUST）
- **FR-002**: 名指しされた場所は、あらかじめ決めた証拠置き場の内側に解決されなければならない（MUST）。絶対 path、置き場の外へ出る相対 path、実体解決後に置き場の外へ出る参照は拒否する（MUST）
- **FR-003**: 実 CLI 観測を根拠とする fixture は、観測記録から算出した digest と、算出に用いた正規化規則の版を申告しなければならない（MUST）
- **FR-003a**: 証拠の名指しは 1 件以上でなければならない（MUST）。空の名指しを「全件一致」として受理してはならない（MUST NOT）
- **FR-003b**: `real-cli-e2e` への昇格根拠として用いる証拠（FR-003a の名指し 1 件）は、その観測を生んだ run の素性（CLI 種別と正確な版、隔離設定、記録の失敗有無、終了状態）を、fixture の自己申告ではなく取得側が生成した記録によって示さなければならない（MUST）。この記録を伴わない名指しは棄却せず、`real-cli-e2e` の根拠にしないだけとする（FR-006d / FR-017a）

**判定**

- **FR-004**: 成果物を組み立てる側は、fixture の申告値を信用せず、名指しされた観測記録から digest を計算し直さなければならない（MUST）
- **FR-005**: 観測記録が存在しない・読めない・digest が一致しない・正規化規則の版が未知、のいずれでも組み立ては失敗しなければならない（MUST）。当該 cell を黙って下位の証拠強度へ格下げして続行してはならない（MUST NOT）
- **FR-006**: `real-cli-e2e` は、FR-004 の再計算が一致した cell にのみ刻まれなければならない（MUST）。実 CLI 観測を名乗る cell を生成する経路が複数ある場合、そのすべてが同じ検査を通らなければならない（MUST）
- **FR-006a**: 「1 つの実測が 2 つの主張を同時に証明した」ことを要求する判定は、同じ観測記録から導かれたときにのみ成立させなければならない（MUST）。1 つの fixture が複数の記録を束ねられる以上、fixture が同じことを同一実測の根拠にしてはならない（MUST NOT）
- **FR-006b**: cell の主張が観測記録から機械的に導ける種類である場合、その値は記録から導出して申告値と照合しなければならない（MUST）。導出値と申告値が食い違う fixture は棄却しなければならない（MUST）
- **FR-006c**: 主張が記録から機械的に導けない種類である場合、その cell に `real-cli-e2e` を刻んではならない（MUST NOT）。導けないことを説明文で断ったうえで昇格させる扱いは許されない（MUST NOT）
- **FR-006d**: run の素性を示す記録（FR-003b）を伴わない証拠だけを根拠とする cell に、`real-cli-e2e` を刻んではならない（MUST NOT）

**正規化**

- **FR-007**: 正規化規則は repository のコードを正本とし、fixture 側の記述で上書きできてはならない（MUST NOT）
- **FR-008**: 観測記録を取得する側と検証する側は、同一の正規化実装を用いなければならない（MUST）。同じ規則を二重に実装してはならない（MUST NOT）
- **FR-009**: 正規化は、取得のたびに変わる値（時刻、session 識別子、tool 呼び出し識別子、プロセス識別子、作業ディレクトリや transcript の絶対 path、所要時間、モデル名、モデルが書く自由文）を digest から除かなければならない（MUST）
- **FR-010**: 正規化は、観測に現れた欄の**有無**と、event および配列要素の**順序**を保存しなければならない（MUST）。値を伏せる場合も欄そのものを落としてはならない（MUST NOT）。欄の並び順は canonical な順序へ正規化してよい（同じ内容が書き順の違いで別物にならないため）
- **FR-010a**: 正規化は、取得のたびに変わる識別子について、値を出さずに**等値関係**を保存しなければならない（MUST）。識別子が run を通して同一かどうかを主張する cell が、証拠から導けなくなってはならない（MUST NOT）
- **FR-011**: 正規化は、投入した指示・event の種類・hook の名前・tool の名前・起動理由・終了理由・権限モード・subagent の種別を digest に反映しなければならない（MUST）
- **FR-012**: 正規化規則には版番号を持たせ、規則を変えたら版番号を上げなければならない（MUST）
- **FR-013**: 正規化の出力形式（欄の並び順、配列の扱い、改行、文字符号化、伏せ字の表現）は一意に定まらなければならない（MUST）
- **FR-013a**: 観測記録の読み取りは、不正な文字符号化と重複した欄名を検出して拒否しなければならない（MUST）。黙って置換・上書きして続行してはならない（MUST NOT）
- **FR-013b**: 取得側が「解釈できなかった」と記した部分を含む記録は、正常な観測として扱ってはならない（MUST NOT）

**追跡と非漏洩**

- **FR-014**: 成果物は、証拠の所在・正規化規則の版・digest・scenario の識別子・CLI の正確な版を記録しなければならない（MUST）
- **FR-015**: 成果物・診断出力・CI ログは、観測記録の本文、投入した指示、モデルの出力、実行環境の絶対 path を含んではならない（MUST NOT）。この制約は正規化の出力だけでなく、fixture の自由文が成果物へ転記される経路にも及ぶ（MUST）
- **FR-016**: 失敗時の説明は、何が一致しなかったかを示しつつ FR-015 を破ってはならない（MUST）

**限界の明示**

- **FR-018**: 証拠強度の定義は、digest が実際に裏付ける範囲（記録の同一性、event 構造、識別子の等値関係、および取得側の記録による run の素性）を述べなければならない（MUST）。記録の本文に依存する主張を digest が裏付けると読める書き方をしてはならない（MUST NOT）

**移行**

- **FR-017**: 既存の観測記録 16 件に対応する既存 fixture は、その記録から算出した digest を結び付けなければならない（MUST）。結び付けた記録を差し替えたまま digest を更新しない場合、組み立てが失敗しなければならない（MUST）
- **FR-017a**: 既存の記録は run の素性を示す記録を伴わないため、この作業で `real-cli-e2e` へ昇格する cell があってはならない（MUST NOT）。同時に、証拠強度が下がる cell があってもならない（MUST NOT）

### Key Entities

- **観測記録（evidence artifact）**: 隔離 rig の下で 1 回の scenario 実行中に届いた hook event を、届いた順に 1 行 1 件で書き出したもの。証拠置き場の中に置かれる
- **正規化抜粋（normalized excerpt）**: 観測記録から、取得ごとに変わる値を取り除き、欄の有無と並びと substantive な値だけを残した一意な表現
- **証拠 digest（evidenceHash）**: 正規化抜粋の SHA-256
- **正規化規則の版（normalizationVersion）**: 正規化の定義に付ける版番号。fixture 側の申告と harness 側の実装が一致しない限り判定しない
- **capture fixture**: ある CLI・ある scenario の観測結果を、harness が読む形にまとめたもの。ここに証拠の所在・digest・正規化規則の版が載る
- **capability cell**: matrix 上の 1 マス。どの EventKind をどう観測できたかと、その証拠強度を持つ

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 対応する観測記録を持たない fixture は、形式的に正しい digest を申告していても `real-cli-e2e` になる経路が 0 件になる
- **SC-002**: 観測記録の不在・digest 不一致・未知の正規化版という 3 つの異常のうち、成功として素通りするものが 0 件になる
- **SC-003**: 同じ scenario を 2 回取得した既存の記録の組で、digest の一致率が 100% になる
- **SC-004**: 別々の scenario を取得した既存の記録どうしで digest が一致する組は、観測できる差が実際に 0 である組だけになる
- **SC-005**: 成果物・診断出力・CI ログを、既存 16 件の観測記録に現れる絶対 path・投入指示・モデル出力の文字列で検索したとき、ヒットが 0 件になる（基点 commit `5bbf292` では 3 件ヒットする。数え方は baseline.md (b) の表と同じ「文字列と出現ファイルの組」単位で、異なる文字列としては 2 種類）
- **SC-005a**: 名指しが空、記録の本文が壊れている、取得側が解釈できなかった部分を含む、のいずれでも受理されるものが 0 件になる
- **SC-009**: 識別子が run を通して同一かどうかを主張する cell が、証拠から導けないまま `real-cli-e2e` を名乗ることが 0 件になる
- **SC-006**: 成果物だけを見て、任意の `real-cli-e2e` cell について「どの観測記録を、どの正規化版で、どの digest として、どの CLI の版で判定したか」を追える割合が 100% になる
- **SC-007**: 証拠置き場の外を指す参照（絶対 path・`..` を含む相対 path・置き場外へ出る symlink）が、いずれも解決を拒否される
- **SC-008**: 移行後、証拠強度が変化した cell が 0 件になる（昇格 0・降格 0）。かつ、いずれかの観測記録を 1 byte 変えると組み立てが失敗する
- **SC-010**: `real-cli-e2e` を名乗る cell のうち、(a) 記録の再計算を通っていない、(b) run の素性を示す記録を伴わない、(c) 主張を記録から導けていない、のいずれかに当たるものが 0 件になる

## Assumptions

- 証拠置き場は `harness/fixtures/<cli>/raw/` とする。rig が別の場所へ書き出す場合、その成果物をこの置き場へ同一内容で持ち込む工程が必要になる
- scenario の識別子は制約付きの `scenarioId` を fixture へ新設して用いる。既存の自由文 `scenario` 欄は投入した指示や command 由来の文言を含み得るため成果物へは出さない。scenario 定義そのものを別ファイルへ切り出す作業は本仕様の範囲外
- `official-doc` / `source-test` を根拠とする cell には証拠 digest を要求しない。本仕様が扱うのは実 CLI 観測を名乗る cell のみ
- 既存 16 件の観測記録は隔離 rig 下で実際に取得されたものであり、記録そのものは真正として扱う。ただし取得当時の一時領域が既に存在せず、run の素性を示す記録を後から真正に作ることはできない。したがって既存の証拠は `source-test` に留まる
- 現在 `real-cli-e2e` を名乗っている cell は 0 件なので、この作業で既存成果物の証拠強度が下がることは無い。この変更の目的は経路を塞ぐことであり、昇格させることではない
- モデル名を digest から除くのは、既定モデルの更新で digest が変わることを避けるため。モデル名は追跡情報としては別途記録する
- 証拠強度の語彙（`official-doc` / `source-test` / `real-cli-e2e`）は正典仕様 `agent-memory-final-spec-v6.md` §521 の定義を変えない
- 本件は完全性 digest・path traversal 拒否・入力検証を含むためセキュリティ関連として扱い、issue #20 の実装順序が指定する外部 CLI への委譲は行わず Claude Code が実装する。constitution III の「セキュリティ関連コードは外部 CLI へ委譲せず Claude Code が直接実装する（MUST）」に従った判断であり、issue 記載の手順からの意図的な逸脱として記録する
- 昇格を刻む経路は現在 3 箇所ある。仕様は「実 CLI 観測を名乗る cell を生む経路すべて」を対象とし、箇所数が将来変わっても FR-006 の要求は変わらない
