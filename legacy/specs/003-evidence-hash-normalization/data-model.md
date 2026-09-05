# Phase 1 Data Model: 証拠 digest による real-cli-e2e 昇格の裏付け

---

## 0. この digest が証明すること・しないこと（信頼境界）

先に境界を書く。ここを曖昧にしたまま「証拠に裏付けられた」と言うと、
`real-cli-e2e` の意味が実態より強く読まれる。

**証明する**

- 名指しされた観測記録が、fixture が申告した **生 byte の digest**（`captureRawHash`）と
  **正規化抜粋の digest**（`evidenceHash`）の両方のとおりであること。
  生 byte 側があるので、正規化が伏せる差（時刻・空行）を含めて 1 byte の差し替えも検出する
- その記録の event 列・欄の構成・識別子の等値関係（どの event が同じ session / turn /
  tool 呼び出しに属するか）が、申告時点から変わっていないこと
- 記録が隔離 rig の 1 回の run から出たものであること（§2.5 の manifest による）

**証明しない**

- 記録の**本文**（モデルの出力、tool の標準出力）が特定の内容だったこと。
  これらは伏せ字になるため digest には現れない
- したがって「textual outcome に依存する主張」（例: 注入した token が応答へ echo された、
  tool が特定の文言で失敗した）は digest だけでは裏付けられない

**この境界から出る帰結（§4.3 で機械化する）**: 裏付けられない主張を持つ cell は
`real-cli-e2e` を名乗らない。「穴があると説明文に書いておく」ことで昇格を許さない。

---

## 1. 正規化の canonical 形（`normalizationVersion: 1`）

### 1.1 入力の読み取りと境界

- 入力は 1 件の観測記録ファイル。1 行 1 JSON の NDJSON
- 抽出範囲は **ファイル全体**。rig が scenario 1 件につき 1 ファイルを書くため、
  ファイル境界がそのまま scenario 境界になる
- **公開 API の入力は `Uint8Array`（byte 列）にする。** `string` を受け取る形にすると
  呼び出し側が `readFileSync(..., "utf8")` で先に復号でき、不正 UTF-8 の検出を迂回できる。
  復号は関数の内側で 1 箇所に閉じる
- **読み取りは既存の `harness/schema/jcs.ts` を再利用する**
  - byte 列 → 文字列は `decodeUtf8`。Node の既定 UTF-8 読み取りは不正 byte を U+FFFD へ
    黙って置換するため、そのままでは壊れた記録が「正常な記録」として digest を得る
  - 行の解釈は `parseIJson`。`JSON.parse` は重複 property を後勝ちで潰すため、
    `{"a":1,"a":2}` と `{"a":2}` が同じ digest になる。`parseIJson` は重複を拒否する
- 空行は読み飛ばす。空行以外で解釈できない行が 1 つでもあれば **失敗**
- 解釈できた行が 0 件なら **失敗**
- 各行は `event`（文字列）と `payload`（オブジェクト）を持たなければならない。欠ければ失敗
- **`payload.unparsed` を持つ行があれば失敗**。`harness/rig/capture-hook.sh:15` は hook の
  stdin を解釈できなかったとき `payload = {unparsed: raw}` に包む。これは「取れなかった観測」
  なので、正常な payload として digest に混ぜない
- 正規化の中間オブジェクトは `Object.create(null)` で作る。通常の `{}` へ `__proto__` を
  代入すると欄が消え、`__proto__` の有無が digest に現れなくなる

### 1.2 各行の変換

```
{ "event": <verbatim>, "payload": <正規化した payload> }
```

top-level の `at` は落とす（時刻）。`event` / `at` / `payload` 以外の top-level キーが
現れた場合は伏せ字側で処理して残す（未知の欄の出現自体を digest に反映するため）。

### 1.3 値の変換

値の扱いは **キー名と深さの組**で決まる。3 系統ある。

#### (a) verbatim — 位置を限定する

| 位置 | キー |
|---|---|
| top-level | `event` |
| `payload` の**直下** | `hook_event_name` / `tool_name` / `source` / `reason` / `permission_mode` / `agent_type` / `prompt` |

**それより深い階層に同じ名前が現れても verbatim にしない。** 実測で
`payload.tool_input.prompt`（2 件）と `payload.tool_response.prompt`（1 件）が存在し、
これは Agent tool へモデルが組み立てた引数とその echo にあたる。キー名だけで一致させると
`claude-subagent` の digest が再取得で不安定になる。

verbatim 値には `RIG_INJECT_[A-Za-z0-9_]+` → `RIG_INJECT_<marker>` の置換を適用する。

#### (b) 相関 token — 値は捨てるが等値関係は残す

`payload` 直下の次のキーは、値そのものは出さず**初出順の局所 token** へ置き換える。

| キー | token |
|---|---|
| `session_id` / `prompt_id` / `turn_id` / `tool_use_id` / `agent_id` | `<id:1>`, `<id:2>`, … |
| `transcript_path` / `agent_transcript_path` / `cwd` | `<path:1>`, `<path:2>`, … |

token 表は**ファイル単位**で、`id` と `path` で別の番号空間を持つ。同じ値が再び現れたら
同じ token を返す。

**走査順を canonical 側に固定する。** 番号は「初出順」だが、その「順」を入力の property
挿入順で決めてはいけない。同じ JSON 値でも property の書き順が違えば番号がずれ、
digest が変わる（§1.4 でキーを整列するので M10 が破れる）。走査順は次で固定する。

1. 行は**ファイル内の出現順**
2. 各行の中は `event` → `payload` の順
3. object は**キーを UTF-16 コードユニット昇順に整列した後の順**
4. array は要素の順

つまり token の割り当ては §1.4 の直列化と同じ順序で走る。

**この扱いが必要な理由**: すべて同じ `<string>` にすると等値関係が消える。
`stableNativeSessionId` は「session 識別子が run を通して同一か」という主張であり、
等値関係が消えると原理的に裏付けられなくなる（証拠を要求したのに、証拠から導けない cell が
生まれる）。token は値の中身を持たないので、絶対 path も識別子も漏れない。

#### (c) 伏せ字 — それ以外すべて

| 入力 | 出力 |
|---|---|
| `null` | `null` |
| boolean | そのまま（低エントロピーで安定。`interrupted` / `isImage` などの意味が残る） |
| number | `"<number>"` |
| string、空 | `"<string:empty>"` |
| string、その他 | `"<string>"` |
| array | 要素ごとに再帰。**長さを保持する** |
| object | キーを全保持し、キーごとに再帰 |

**配列の中と、`payload` 直下より深い階層は、(a) も (b) も適用しない。**
位置で verbatim を決める規則を深い階層へ広げると不安定になる。

### 1.4 直列化

- 各行を JSON へ直列化する。オブジェクトのキーは UTF-16 コードユニット昇順
- 区切りは詰める（`,` と `:` の後に空白を入れない）
- 非 ASCII はエスケープせず UTF-8 のまま出す
- 行区切りは LF。**最終行の後にも LF を 1 つ置く**
- 全体を UTF-8 の byte 列にし、その SHA-256 の小文字 hex が `evidenceHash`

### 1.5 版番号

- `normalizationVersion` は整数。現在の値は `1`
- §1.1〜§1.4 のいずれかを変えたら値を上げる
- harness は自分が実装している版と一致しない申告を **失敗**として扱う

---

## 2. schema と型の変更

### 2.1 `CaptureFixture` に足す欄

```ts
/**
 * この capture の根拠。`harness/fixtures/<cli>/raw/` からの相対 path で名指しする。
 * 複数 run を 1 つの fixture でまとめている場合があるため配列（最低 1 件）。
 */
evidence?: EvidenceRef[];

interface EvidenceRef {
  /** 証拠置き場からの相対 path。絶対 path と `..` は拒否 */
  path: string;
  /** 正規化抜粋の SHA-256（小文字 hex 64 桁）。再取得しても変わらない側 */
  evidenceHash: string;
  /**
   * 観測記録ファイルの生 byte の SHA-256。
   * `evidenceHash` だけだと、正規化が伏せる差（時刻・空行・伏せ字対象の値）を変えても
   * 同じ digest になる。「この記録そのもの」への結び付けはこちらが担う。
   * legacy 証拠にも必須。
   */
  captureRawHash: string;
  /** 生成に用いた正規化規則の版 */
  normalizationVersion: number;
  /**
   * 同じ run の manifest（§2.5）。置き場からの相対 path。
   * **無い ref は `real-cli-e2e` の根拠にならない**（§2.6 の legacy 証拠）。
   * digest の照合は manifest の有無に関わらず必ず行う。
   */
  manifest?: string;
  /** manifest ファイルの生 byte の SHA-256。`manifest` があるとき必須 */
  manifestHash?: string;
}
```

### 2.1a legacy 証拠（manifest を持たない ref）

既存の観測記録 16 件は 2026-08-12 に取得されたもので、当時の rig 一時領域
（`/tmp/free-mem-rig-jura/capture/`）は既に存在しない。`.version` も `.errors` も stderr も
残っていないため、**真正な manifest を後から作ることはできない**。
手で書いた manifest は fixture の自己申告そのもので、FR-003b を満たさない。

再取得も同 PR では成立しない。claude CLI は現在 2.1.234 で、fixture の pin は 2.1.228。
取り直すと 5 つの claude fixture すべての `nativeVersion` が変わり、
version-pin（`assemble.ts` の `version-pin violation`）に引っかかる。これは別の変更になる。

**Decision**: manifest を持たない ref を「legacy 証拠」として受け入れる。ただし

- digest の照合は行う（記録を差し替えたら組み立てが落ちる。今より厳しくなる）
- **その ref だけを根拠とする cell は `source-test` のまま**。`real-cli-e2e` へは上げない
- `real-cli-e2e` を名乗るには manifest 付きの ref が要る

結果として、この変更で昇格する cell は **0 件**になる。塞ぐのが目的であって、
昇格させるのが目的ではない（issue 本文「実 CLI capture rig を有効化する前に塞ぐ」）。

**owner の不採用事項との関係**: issue #20 は「生 transcript ファイル全体の SHA-256 を
`evidenceHash` にする」を不採用としている。`captureRawHash` は生ファイルの SHA-256 だが、
不採用の対象ではない。不採用だったのは**再現性を担う digest として生ファイルの hash を使う**
ことで、それだと同じ scenario を取り直すたびに不一致になる。役割を 2 つに分けている。

| 欄 | 役割 | 再取得で |
|---|---|---|
| `evidenceHash`（正規化抜粋） | 「同じ観測か」を判定する。owner が採用した形 | 変わらない |
| `captureRawHash`（生 byte） | 「この記録そのものか」を結び付ける | 変わる |

`evidenceHash` の役割は owner 決定のまま。`captureRawHash` はそこへ足した別の役割で、
置き換えではない。

**単数欄にしない理由**: `claude/interrupt-and-hook-timeout` が 5 本の観測記録を根拠にしている。
単数欄だと 4 本が検査対象から外れ、この fixture の主張の中心である中断の証跡が裏取り無しで残る。

### 2.2 既存 `evidenceHash` 欄の扱い

`CaptureFixture` 直下の `evidenceHash?: string` は **廃止**する。本 issue が塞ぐ対象そのもの。
申告している fixture は 0 件なので、廃止で既存 fixture の書き換えは発生しない。

`CapabilityEvidence`（matrix 出力側）は §5 の形へ変える。

### 2.3 `capability.schema.json`

- `properties.evidence` を追加。`type: "array"`、**`minItems: 1`**、`additionalProperties: false`
  - 要素の `required` は `path` / `evidenceHash` / `captureRawHash` / `normalizationVersion` の 4 つ
  - `manifest` と `manifestHash` は **両方あるか両方無いか**。対応済み keyword だけで書く。

    ```jsonc
    "allOf": [
      { "if": { "required": ["manifest"] },     "then": { "required": ["manifestHash"] } },
      { "if": { "required": ["manifestHash"] }, "then": { "required": ["manifest"] } }
    ]
    ```

    **`dependentRequired` も `maxProperties` も `not` も使えない**:
    `harness/schema/validate.ts` の `SUPPORTED_KEYWORDS` に無く、
    `validate.ts:327` が `unsupported schema keyword` で throw する。
    使えるのは `$ref` / `$defs` / `type` / `enum` / `const` / `required` / `properties` /
    `additionalProperties` / `items` / `minItems` / `maxItems` / `minLength` / `maxLength` /
    `pattern` / `minimum` / `maximum` / `oneOf` / `anyOf` / `allOf` / `if` / `then` / `else`
- `properties.scenarioId` を追加。`type: "string"`、`pattern: "^[a-z0-9]+(?:[.-][a-z0-9]+)*$"`。
  `required` に加える（すべての capture fixture が持つ。matrix へ出る唯一の識別子のため）
- **成果物へ出る既存の string にも制約を掛ける。** 自由文を止めても、制約の無い string が
  残っていればそこから漏れる。現状は次が無制約:

  | 欄 | 現状 | 変更後 |
  |---|---|---|
  | `fixtureId` | `{type: string, minLength: 1}` | `pattern: "^(claude\|codex)/[a-z0-9]+(?:-[a-z0-9]+)*$"`。matrix の `fixtureIds` / `sourceFixtureId` / `evidenceSources` / 生成 limitations に出る |
  | `observedEvents[].sourceEvents[]` | `{type: string}` | 既知の hook 名の `enum` に閉じる。matrix の cell へそのまま載る。**値は手で並べず、既存 fixture と観測記録から機械的に導いた**（T002 で実測。raw に現れる event は `PostToolUse` / `PreToolUse` / `SessionEnd` / `SessionStart` / `Stop` / `SubagentStop` / `UserPromptSubmit` の 7 種。fixture の `sourceEvents` はこのうち 6 種で、`SubagentStop` は raw にのみ現れる。`PreCompact` は 1 件も無く、手で並べたときに書いていた） |
  | `nativeVersion` | `{type: string, minLength: 1}` | 制御文字を禁じる `pattern` を足す（`^[\x20-\x7e]+$`） |
  | `limitationCodes[]` | 新設 | closed enum（§5.3）。値は `limitation-codes.md` の 22 種。`limitations` と**同じ場所に同じ長さで位置対応**させる |

  `fixtureId` へ prompt をコピーすれば matrix の 4 経路へ漏れる。
  「生成側の文字列は fixture id と enum しか含まないので安全」という前提は、
  fixture id 自体が無制約なら成立しない
- `properties.evidenceHash`（top-level）を削除
- `evidence` は `required` に加えない。`official-doc` / `source-test` 由来の fixture は
  観測記録を持たない

**`manifest` を optional にしつつ schema で required にしない理由**: legacy 証拠 8 fixture が
全件落ちる。型と schema の食い違いを残すと、片方だけ直したときに気づけない。

**`minItems: 1` が要る理由**: 「全 ref が一致」を `every` で実装すると空配列は空虚真になり、
`evidence: []` を書くだけで観測記録を 1 件も読まずに昇格できる。schema と assemble の両方で
非空を検査する（片方だけだと、schema を通さない経路が残ったときに穴が開く）。

**`required` に入れない判断の理由**: 入れると `official-doc` 由来の fixture が書けなくなり、
`evidenceKind` の語彙 3 種のうち 2 種が到達不能になる。条件付きの要求（実 CLI 観測を
名乗るときだけ必要）は assemble 側の検査で表す。

### 2.4 `harness/fixtures/continuity/*.json`

別種の fixture（`cases` / `intakeContext` を持つ）で `capability.schema.json` の検証対象外。
schema 変更の影響を受けないことを回帰で確認する。

### 2.5 run manifest

digest はファイルの整合性しか証明しない。「これは実 CLI を隔離 rig で動かした記録だ」は
別に裏付けが要る。rig は既に材料を出している。

| 材料 | 現状 |
|---|---|
| CLI の exact version | `$RIG_BASE/capture/<cli>-<label>.version`（`rig.sh:76,88`） |
| recorder の失敗 | `<capture>.errors`（`capture-hook.sh:23`。**無い**ことが正常） |
| run の終了状態 | `<cli>-<label>.exit`（`rig.sh` が `printf '%s\n' "$rc"` で書き、manifest の `exitStatus` はここから読む。`<cli>-<label>.stderr` の `exit=N (recorded)` は人向けの控え） |
| 隔離設定 | `rig.sh` の `run_env`（`HOME` / `CLAUDE_CONFIG_DIR` / `CODEX_HOME` の差し替え） |

rig は run のたびに manifest を 1 件書き、証拠置き場へ observation と一緒に持ち込む。

```jsonc
{
  "manifestVersion": 1,
  "cli": "claude",                              // "claude" | "codex" の enum
  "cliVersion": "2.1.228 (Claude Code)",        // .version を単一行として読み末尾 LF を除いた値
  "scenarioId": "claude.lifecycle-basic",       // §5.3 の制約付き識別子。自由文ではない
  "capturedAt": "2026-08-12T00:00:00.000Z",     // 観測記録の 1 行目の at。rig が別に持つ時刻ではない
  "isolated": true,                             // rig が書く。fixture の自己申告ではない
  "internalRunMarker": true,
  "exitStatus": 0,                              // 非負整数
  "recorderErrors": 0,                          // .errors が無い / 空なら 0。非負整数
  "capture": "claude-lifecycle-basic.jsonl",    // 置き場からの相対 path
  "captureRawHash": "<観測記録ファイルの生 byte の SHA-256>",
  "captureHash": "<正規化抜粋の SHA-256>",
  "normalizationVersion": 1
}
```

**`captureRawHash` を別に持つ理由**: `captureHash` は正規化後の digest なので、
既知の正規化衝突（例: `hook-timeout` と `lifecycle-basic`）を持つ別の記録と manifest を
組み合わせられる。生 byte の digest を併記すると、manifest が指す記録は 1 つに定まる。

**manifest の型と検証**

- TypeScript の `RunManifest` 型と、`additionalProperties: false` の JSON Schema を置く
- `manifestVersion` が harness の実装版と違えば **失敗**（未知の版を推測で読まない）
- `exitStatus` / `recorderErrors` は非負整数。負数・非整数・欠落はすべて失敗
- `manifestHash` は **manifest ファイルの生 byte の SHA-256**。正規化は掛けない
- `capturedAt` は取り込み側も検証側も `captureCapturedAt`（`normalize.ts`）1 本を通す。
  綴りは schema の pattern が当てるが、暦に実在しない日付（`2026-02-30`）は pattern を
  通るので、同じ関数の中で `isRealInstant` にも掛ける。ここを緩めると、公開する
  `verifiedAt` に実在しない瞬間が載る

**照合する項目（§4.1 の一覧に入る）**

| manifest 側 | 突き合わせる相手 |
|---|---|
| `manifestVersion` | harness の実装版 |
| `cli` | fixture の `cli` |
| `cliVersion` | fixture の `nativeVersion`（下の canonical 規則を適用してから比較） |
| `scenarioId` | fixture の `scenarioId`（§5.3 で fixture 側にも足す） |
| `capturedAt` | 観測記録の 1 行目の `at`（`captureCapturedAt` が導く）。**fixture の `capturedAt` とは比べない**——1 つの fixture が複数の run を束ねるので、fixture 単位で縛ると 2 本目以降の manifest が構造的に通らなくなる |
| `capture` | `EvidenceRef.path` |
| `captureRawHash` | 観測記録の生 byte から再計算した値 |
| `captureHash` | 観測記録の正規化抜粋から再計算した値 |
| `normalizationVersion` | `EvidenceRef.normalizationVersion` と harness の実装版 |
| `isolated` | `true` であること |
| `internalRunMarker` | **`true` であること**。fixture との一致だけを見ると、双方を `false` にすれば通る |
| `recorderErrors` | `0` であること |

**`cliVersion` の canonical 規則**: rig は `{ "$CLAUDE_BIN" --version; } > <label>.version` で
保存する。`--version` の出力は末尾に LF が付く（実測: `... Code)\x0a`）。fixture の
`nativeVersion` は改行を含まないので、そのまま比較すると正当な manifest が不一致になる。
manifest へ書く時点で **UTF-8 の単一行として読み、末尾の CRLF か LF を 1 つだけ取り除く**。
複数行が返る CLI が現れたら失敗させる（黙って 1 行目を採らない）。

**scope の限界**: manifest は rig が書くので、rig を動かす人を信頼する境界は残る。
これは repository の checkout を信頼するのと同じ水準で、`real-cli-e2e` の定義に
「隔離 rig の運用者を信頼する」ことを明記して閉じる。

---

## 3. path 解決の許可範囲

```
resolveEvidencePath(cli, relPath):
  1. cli が既知の値（"claude" / "codex"）でなければ失敗
  2. relPath が絶対 path なら失敗
  3. relPath を path 区切りで分解して ".." を含むなら失敗
  4. root = realpath(harness/fixtures/<cli>/raw)
  5. candidate = realpath(join(root, relPath))  — 存在しなければ失敗
  6. candidate が root + 区切り で始まらなければ失敗（symlink 脱出の遮断）
  7. candidate が通常ファイルでなければ失敗（lstat で判定）
  8. candidate を読む
```

**root の差し替え口**: `resolveEvidencePath` は module 位置から root を導くので、
一時ディレクトリへ置いた synthetic な証拠を end-to-end で組み立てられない
（positive control が成立しない）。内部関数へ **root を明示的に渡せる引数**を設け、
production の CLI 経路は固定 root を渡す。fixture の値や環境変数から root を変えられない
ことも test で固定する（差し替え口が production 入力から届いたら、置き場の制約が消える）。

- 4 と 5 の両方で realpath を取る。root 側を解決しないと、root 自体が symlink のときに
  6 の前方一致が常に失敗する
- 6 は「root で始まる」ではなく「root + 区切り で始まる」で判定する。
  `raw` と `raw-evil` のような兄弟ディレクトリを通さないため
- **失敗はすべて安全な理由コードへ変換する。** `realpath` / `open` / `stat` の例外を
  そのまま伝播させると repository の絶対 path が診断へ出る

**残る境界**: hard link は置き場内の通常ファイルとして通る。検査から読み取りまでの間に
ディレクトリを差し替える TOCTOU も残る。どちらも「checkout を書き換えられる相手」を
前提とするので、checkout を信頼済みとする境界の内側に置く。境界は §0 に明記する。

---

## 4. 判定の流れ（assemble 側）

### 4.1 fixture ごと

```
evidence が無い          → この fixture の cell は real-cli-e2e にしない（source-test 止まり）
evidence が空配列        → 失敗（空虚真の遮断）
evidence がある          → 各 ref について:
    normalizationVersion が harness の実装版と違う      → 失敗
    path 解決が拒否された                                → 失敗
    観測記録の byte が読めない                           → 失敗
    digestRaw(captureBytes) !== ref.captureRawHash       → 失敗
    normalizeCapture(captureBytes) が解釈できない        → 失敗
    digestCapture(captureBytes) !== ref.evidenceHash     → 失敗
    manifest がある場合:
      manifest の byte が読めない                        → 失敗
      digestRaw(manifestBytes) !== ref.manifestHash      → 失敗   ← parse の前に照合する
      closed schema に合わない                           → 失敗
      §2.5 の照合表 12 項目のいずれかが違う              → 失敗
  全 ref が通ったとき refVerified = true
```

`manifestBacked` は **ref ごとの属性**であって fixture の属性ではない（§4.2）。

「失敗」は組み立て全体の失敗。当該 cell だけ黙って `source-test` へ落として続行しない。
落として続行すると、証拠の差し替えを忘れた状態が緑のまま残る。

### 4.2 昇格箇所

`assemble.ts` の 3 箇所——capture cell・highLevel cell・prompt 対の再刻印——すべてが
`promoteCell` の結果を見る。`Boolean(f.evidenceHash)` を見る経路を 1 つも残さない。
（行番号では書かない。実装が動くと文書だけが古い場所を指したまま残る）

**昇格は cell ごとに、その cell を支持する ref を見て決める**

**判定の順序が効く。** 種別の判定を先に置く。

```
cell ごと:
  主張が §4.3 で「導けない」種類  → source-test（supporting は求めない）
  導ける種類:
    supporting = fixture の ref のうち、§4.3 の導出値が申告値と一致するもの
    supporting が空                          → 失敗（申告を支持する記録が 1 件も無い）
    backed = supporting のうち、manifest 付きで、かつ申告した sourceEvents を
             **その値の導出に実際に使った** hook として全部持つもの
    backed が空                               → source-test（legacy 証拠だけ、または出どころの申告が合わない）
    それ以外                                  → real-cli-e2e。evidenceRefs に backed を載せ、
                                                verifiedAt は backed の capturedAt のうち最も遅い瞬間
```

**`backed` を fixture の和集合で決めない。** 1 つの fixture は複数の run を束ねるので、
和集合で足りるとすると「値を導いた run」と「申告した hook を持つ run」が別々でも通る。

**「記録に在る」ではなく「導出に使った」で見る。** §4.3 の導出は cell ごとに使う hook が
決まっている（`assistant_completed` は `Stop`、`session_started` は `SessionStart`）。
在るかどうかだけを見ると、`assistant_completed` の出どころを `SessionStart` と申告しても
通る——記録も digest も manifest も本物のまま、公開する provenance だけが偽になる。
`deriveClaims` は値と一緒に `captureSources` を返し、照合はそちらに対して行う。

**cell の統合でも同じ規則を保つ。** 複数 fixture が同じ cell を主張したとき、
`sourceEvents` は「自分の昇格が `real-cli-e2e` だった側」からしか取らない。証拠を持たない
fixture は `sourceEvents` の実在検査そのものを飛ばされる（ref が 0 件）ので、統合で足すと
実測済み cell がどの記録にも無い hook 名を主張する。

順序を逆にすると（supporting を先に求めて空なら失敗）、導出値が存在しない「導けない」主張は
supporting が必ず空になり、`sessionStartInjection` や `tool_failed` を持つ既存 fixture が
`source-test` に留まらず**組み立て全体を失敗させる**。

**fixture 単位の `manifestBacked` を使わない理由**: legacy ref A がその cell を支持し、
同じ fixture に無関係な manifest 付き ref B があるとき、fixture 単位の boolean だと
B の manifest で A の主張が昇格してしまう。混在 fixture を使った回帰 test を置く。

`source-test` へ留めるのは異常ではない（証拠が弱いという正当な状態）。**失敗**させるのは
digest 不一致・manifest 不整合・導出値と申告値の食い違い・支持 ref ゼロ・空配列・未知の版。

**prompt 対の同一 run 拘束**: prompt 対の再刻印は「1 つの実測が対を同時に証明した」ことを要求する。
1 つの fixture が複数 run を束ねられるようになった以上、fixture が同じことは同一 run の
証明にならない。**両 cell を支持する ref 集合に同じ 1 件が含まれるときだけ**対を成立させる。
なお両 cell は §4.3 の表で「導けない」側なので、現状この経路は到達しない。

### 4.3 claim を観測記録から導く

digest 一致は「記録が改竄されていない」ことしか言わない。正しい記録と digest を流用して
根拠のない cell 主張を書き足す経路は、**主張の値そのものを記録から導く**ことでしか塞げない。
「穴があると説明文に書く」では塞がらない。

#### VerifiedClaims — 検証器の出力

検証器は各 `EvidenceRef` から、その記録が支持する主張を機械的に生成する。

```ts
interface VerifiedClaims {
  /** この ref の由来。manifest が無ければ real-cli-e2e の根拠にならない */
  refIndex: number;
  manifestBacked: boolean;
  /** この記録から導けた capture cell の値 */
  capture: Partial<Record<EventKind, ObservedCapability>>;
  /** この記録から導けた highLevel cell の値 */
  highLevel: Partial<Record<HighLevelKey, ObservedCapability>>;
}
```

#### 複数 ref の集約

既存 fixture は複数 run の**和集合**を表す。`claude/interrupt-and-hook-timeout` の 5 本は
それぞれ別の観測で、どの 1 本も fixture の全主張を支持しない。したがって

- fixture の 1 つの主張は、**いずれか 1 件以上の ref が同じ値を導出**すれば成立する
- 成立させた ref だけを、その cell の `evidenceRefs` へ載せる
- どの ref も導出しない主張は **失敗**（黙って値を差し替えない）

「全 ref が全主張を支持する」ことは要求しない。要求すると既存 fixture が全件落ちる。

#### 導ける主張と、導けない主張

正規化後に残る情報は「event の種類と並び」「欄の有無」「識別子の等値関係」「boolean 値」。
主張がこの範囲の関数として書けるかどうかで分ける。

| 主張 | 導出 | 判定 |
|---|---|---|
| `session_started` / `user_prompted` / `session_ended` = native | 対応 hook（`SessionStart` / `UserPromptSubmit` / `SessionEnd`）が実在する | **導ける** |
| `tool_started` / `tool_completed` = native | `PreToolUse` / `PostToolUse` が実在する | **導ける** |
| `assistant_completed` = synthesized | `Stop` の payload に `last_assistant_message` **欄**が存在する（値は伏せ字でよい） | **導ける** |
| `turn_completed`（Claude）= synthesized | `UserPromptSubmit` と `Stop` が同じ `prompt_id` token を共有し、`turn_id` 欄が無い | **導ける**（相関 token のおかげ） |
| `turn_completed`（Codex）= native | `UserPromptSubmit` と `Stop` が同じ `turn_id` token を共有する | **導ける**。Codex fixture 3 件は実際に `native` を申告しており、Claude と同じ規則にすると全件落ちる |
| `session_interrupted` = synthesized | `Stop` が無く `SessionEnd` がある | **導ける** |
| `subagentCapture` = native | `SubagentStop` が実在する | **導ける** |
| `stableNativeSessionId` = native | 全 event の `session_id` が同じ token | **導ける**（相関 token のおかげ） |
| `tool_failed` = native / `toolFailurePhasesObserved` | 失敗か成功かは `tool_response` の**中身**にしかない | **導けない** |
| `sessionStartInjection` / `promptAwareInjection` / `promptDeliveryBeforeModel` | 注入が効いたことは応答**本文**への echo でしか分からない | **導けない** |
| `compactSingleDelivery` / `trueSessionEnd` / `pre_compact` / `post_compact` | 現行 16 件に該当 event が無く、導出規則を実測で決められない | **導けない** |

#### 導けない主張の扱い

**`real-cli-e2e` を名乗らせない。** 該当 cell は `source-test` に留める。
これらの cell については、fixture の申告値を導出値と照合しない（導出値が無いため）。
申告のままの値を載せるが、証拠強度は上げない。


これらを裏付けるには「注入 token が応答に現れたか」のような本文由来の述語が要り、
それは正規化が伏せる情報なので、**digest を再取得安定にする設計と本質的に両立しない**。

別の証拠形式（例: rig が「注入 token が応答へ含まれていたか」を boolean として manifest へ
記録する）で扱うのが筋で、本 issue の範囲を超える。別 issue へ切り出す。

---

## 5. 出力の provenance

### 5.1 `CapabilityEvidence` の形

複数 ref を持つ fixture が cell の根拠になるため、単数の `evidenceHash` では表せない。

```ts
interface CapabilityEvidence {
  // ... 既存の欄 ...
  /** matrix 直下の evidenceSources への添字。cell がどの記録に裏付けられたか */
  evidenceRefs?: number[];
}
```

matrix 直下に表を 1 つ置き、cell からは添字で参照する。

```ts
/** fixtureId + path の昇順で一意化した配列。cell からは添字で参照する */
evidenceSources: Array<{
  fixtureId: string;
  path: string;                  // 置き場からの相対 path。絶対 path は入らない
  evidenceHash: string;
  normalizationVersion: number;
  manifestHash: string | null;   // legacy 証拠は null
  cliVersion: string;
  scenarioId: string;            // 制約付き識別子。自由文の scenario は出さない
}>;
```

これで SC-006（どの記録を・どの版で・どの digest で・どの CLI 版で判定したかを成果物だけで追える）
が複数 ref でも成り立つ。並びは `fixtureId` → `path` の昇順で一意に決める。

**`scenario` の自由文を成果物へ出さない。** 現行の `scenario` 欄は
「Bash tool 成功実行（allowedTools 明示）で PreToolUse/PostToolUse の対を観測」のような
自由文で、投入した指示や command 由来の文言が混ざり得る。`scenarioId` を fixture へ
新設し、`^[a-z0-9]+(?:[.-][a-z0-9]+)*$` に制約する（例: `claude.tool-lifecycle`）。
自由文の `scenario` は fixture 内の説明として残すが、matrix へは出さない。

### 5.2 `capabilityHashInputs`

現在は `fixture:<id>@<evidenceHash>` という ad-hoc 文字列を並べている（`assemble.ts` の contract hash 入力）。
欄が増えると連結の曖昧さで別の入力が同じ文字列になり得るので、**構造化して JCS で
canonical 化する**。`harness/schema/jcs.ts` の `canonicalizeJson` を使う。

```ts
capabilityHashInputs = [
  canonicalizeJson({ cli, nativeVersion }),
  // evidenceSources を**丸ごと**。欄を手で並べると、後から足した欄が黙って hash の外に残る
  canonicalizeJson(evidenceSources),
  canonicalizeJson(folded),   // 畳んだ capabilities（capabilityHashInputs 自身を除く）
];
```

`evidenceSources` の欄を数え上げる形は採らない。実装の途中で実際に `cliVersion` と
`scenarioId` が抜け、公開する provenance を書き換えても capability hash が動かない状態に
なった。manifest hash を含む全欄が hash の入力に入るのは、この「丸ごと canonical 化」の
結果であって、列挙を維持した結果ではない。
入力の列挙は欄を数え上げるのではなく畳んだ結果から導く（既存の `folded` の扱いを踏襲）。
exact な byte 列と並び順を contract test で固定する。

**`harness/contract-hashes.json` の再生成が必要になる。** `capability.schema.json` と
fixture がその入力で、CI が `node harness/contract-hashes.mjs` の出力との差分を見ている
（`.github/workflows/ci.yml:151-152`）。再生成を実装順序へ入れる。

### 5.3 秘密を出さない（FR-015）

**現状は既に破れている。** `harness/matrix/{claude,codex}.json` に `RIG_INJECT_5f3a9` が
そのまま載っている。fixture の `limitations` 自由文が matrix へ逐語転記されるため、
normalizer を伏せ字にしても直らない。

対処:

**自由文を matrix へコピーしない。** これが唯一の完全な閉じ方になる。
部分文字列の照合は、下限より短い秘密（token の断片、短い prompt）を原理的に取りこぼす。
取りこぼす検査を FR-015 の MUST の担保にはできない。

**設計**

**配置**: 散文 `limitations` は fixture の **top-level** と **`observedEvents[]` ごと**の
2 箇所にある（現行 27 種の内訳: top-level 側と event 側の両方）。cell 固有の caveat との
対応を失わないよう、`limitationCodes` も**同じ 2 箇所**に置く。

| 場所 | 散文（残す） | コード（新設・matrix へ出る） |
|---|---|---|
| fixture top-level | `limitations: string[]` | `limitationCodes: string[]` |
| `observedEvents[]` | `limitations: string[]` | `limitationCodes: string[]` |
| `highLevel` の cell | （現状なし） | （現状なし） |

**matrix 側は既存契約の `CapabilityEvidence.limitations: string[]` をそのまま使い、
中身をコードにする。** 新しい欄を作ると凍結済みの型と食い違う。
assemble が生成する文字列（`observed <value> in <fixtureId>` など）は、
`fixtureId` と cell 名と enum 値しか含まない（E4 の制約により fixtureId も安全な形になる）。

**enum の全値**: 現行 27 種の散文（top-level 11 / event ごと 16。実測）へ 1 つずつ割り当てた
**22 種**。対応表は `specs/003-evidence-hash-normalization/limitation-codes.md`（T003 の成果物。
散文の SHA-256 先頭 8 桁を key にしてあるので、散文を書き換えると対応が崩れたことが分かる）。
同じ観測事実を述べた散文には同じコードを当てている。

**配置は `limitations` と同じ場所・同じ長さ・位置対応**にする。散文を足してコードを足し忘れた
状態を検査で落とすため（コードを dedupe すると対応が消えるので dedupe しない）。
コード名は「観測できなかったこと」を表す kebab-case にする
（例: `stop-not-fired-on-sigint` / `session-end-reason-always-other` /
`post-tool-use-absent-on-failure` / `failure-phase-not-directly-observable` /
`headless-only-no-tty` / `subagent-internals-not-visible-to-parent`）。

- 既存の散文 `limitations` は fixture 内に残す（repo 内の読み手向け）。**matrix へは出さない**
- `scenario` も §5.1 のとおり `scenarioId` へ置き換えて matrix から外す

**補助検査として部分文字列の照合を残す**

閉じ方は上の設計側にある。そのうえで、将来 assemble が新しい経路で文字列を出したときに
気づけるよう、成果物へ出る全文字列を対象に、参照 raw の秘密欄（`prompt` /
`last_assistant_message` / `cwd` / `transcript_path` / 入れ子の `tool_input`・`tool_response`）
から取った 16 文字以上の部分文字列を含んだら組み立てを失敗させる。
**これは信頼境界ではなく、設計が破れたときの警報。**

**漏洩 test は canary で行う**

1. 一意な canary 文字列を、fixture の散文 `limitations` と raw の秘密欄の各経路へ 1 つずつ仕込む
2. 組み立てを子プロセスで走らせ、matrix・stdout・stderr のいずれにも canary が出ないことを見る
3. 散文 `limitations` へ入れた canary が出ないことが、「自由文を matrix へ出さない」の確認になる

- backfill の際に 8 fixture へ `limitationCodes` を付ける
- 成果物へ出す path は置き場からの相対 path のみで、
  `^[A-Za-z0-9][A-Za-z0-9._-]*\.jsonl$` に制約する

## 6. 先に決めた変異（実装が満たすべき kill 条件）

| # | 変異 | 落ちるべき test |
|---|---|---|
| M0 | `improvesEvidence`（`assemble.ts`）を廃止した欄のまま残す | 証跡の優劣で cell が入れ替わる経路。廃止欄を読むと常に false になり優劣が黙って消える |
| M1 | 昇格条件を `Boolean(fixture.evidence)` へ戻す（再計算しない） | 実在しない path を指す fixture が棄却されること |
| M2 | digest 不一致を「失敗」から「降格して続行」へ変える | 不一致 fixture で組み立てが失敗すること |
| M3 | `normalizationVersion` の照合を消す | 未知の版を申告した fixture で失敗すること |
| M4 | 3 箇所のうち 1 箇所だけ検査を外す（prompt 対の再刻印を素通し） | 3 経路それぞれの棄却 test |
| M5 | path 解決の `..` 検査を消す | `../../../etc/passwd` 形の ref が拒否されること |
| M6 | path 解決の realpath 前方一致を消す | 置き場内から外へ出る symlink が拒否されること |
| M7 | verbatim キーから `prompt` を落とす | `claude-tool-denied` と `claude-tool-ok` の digest が異なること |
| M8 | 伏せ字をやめて値を verbatim にする | 再取得 `claude-interrupt3` / `claude-interrupt4` の digest が一致すること |
| M8b | verbatim 判定を深さ無視のキー名一致へ変える | `payload.tool_input.prompt` だけが違う 2 記録の digest が一致すること |
| M9 | 正規化でキーを落とす（値が伏せ字なら欄ごと省く） | 欄の有無だけが違う 2 記録の digest が異なること |
| M10 | 直列化のキー整列を消す | 内容が同じでキー順が違う 2 記録の digest が一致すること |
| M11 | array の長さを保持しない | 長さだけが違う 2 記録の digest が異なること |
| M12 | 空ファイル・全行壊れを成功にする | 空の観測記録で失敗すること |
| M13 | schema から `evidence` を消す（fixture だけ先に足した状態） | `unknown top-level key` で弾かれること |
| M14 | 失敗メッセージへ観測記録の中身や絶対 path を入れる | 失敗出力に禁止文字列が現れないこと |
| **M15** | schema の `minItems: 1` と assemble の非空検査を外す | `evidence: []` の fixture が棄却されること |
| **M16** | 相関 token をやめて `<string>` へ戻す | 同じ `session_id` が継続する記録と途中で変わる記録の digest が異なること |
| **M17** | 相関 token をファイル単位でなく全体で共有する | 別ファイル間で token 番号が影響し合わないこと |
| **M18** | `sourceEvents` の実在検査を消す | 正しい raw と digest のまま、実在しない hook を根拠に挙げた fixture が棄却されること |
| **M19** | prompt 対の同一 run 拘束を「同一 fixture」へ緩める | 2 つの別 run を束ねた fixture で対が成立しないこと |
| **M20** | manifest の照合（`isolated` / `recorderErrors` / `captureHash` / `cliVersion`）を外す | 隔離外・recorder 失敗・version 不一致の manifest が棄却されること |
| **M21** | `parseIJson` を `JSON.parse` へ、`decodeUtf8` を既定読み取りへ戻す | 重複キー・不正 UTF-8 の記録が棄却されること |
| **M22** | `payload.unparsed` を持つ行を正常扱いにする | 取れなかった観測を含む記録が棄却されること |
| **M23** | 正規化の中間オブジェクトを `{}` にする | `__proto__` 欄の有無が digest に現れること |
| **M24** | fixture の `limitations` を無害化せずに backfill する | matrix・stdout・stderr に raw の実値が現れないこと |
| **M25** | `contract-hashes.json` を再生成しない | CI の contract hash 差分検査が落ちること |
| **M26** | claim 導出をやめて fixture の申告値をそのまま使う | 正しい raw と digest のまま cell 値を書き換えた fixture が棄却されること |
| **M27** | 導けない主張（`sessionStartInjection` 等）も昇格対象にする | 該当 cell が `source-test` に留まること |
| **M28** | manifest 無しの ref でも昇格させる | legacy 証拠だけの cell が `source-test` に留まること |
| **M29** | `captureRawHash` の照合を外す | 正規化衝突する別の記録と manifest を組み合わせた fixture が棄却されること |
| **M30** | 相関 token の走査順を入力の property 順にする | キー順だけを入れ替えた 2 記録の digest が一致すること |
| **M31** | 公開 API の入力を `string` にする | 不正 UTF-8 の記録が呼び出し側の復号で素通りしないこと |
| **M32** | `capabilityHashInputs` を ad-hoc 連結へ戻す | 欄の境界が曖昧な 2 つの入力が同じ hash にならないこと |
| **M33** | `scenarioId` をやめて自由文 `scenario` を matrix へ出す | canary が matrix へ出ないこと |
| **M34** | legacy ref から `captureRawHash` の照合を外す | 正規化が伏せる差（空行追加・時刻変更）だけを変えた記録が棄却されること |
| **M35** | `manifestBacked` を fixture 単位にする | legacy ref が支持する cell と manifest 付き ref が同居する fixture で、その cell が昇格しないこと |
| **M36** | 複数 ref の集約を「全 ref が支持」にする | 5 本を参照する `claude/interrupt-and-hook-timeout` が通ること |
| **M37** | `turn_completed` の導出を Claude 規則へ統一する | Codex fixture 3 件の `native` 申告が通ること |
| **M38** | `manifest.internalRunMarker` の検査を fixture との一致だけにする | manifest と fixture の双方が `false` の組み合わせが棄却されること |
| **M39** | `cliVersion` の末尾改行を取り除かない | 正当な manifest が `nativeVersion` と一致すること |
| **M40** | schema の `manifest` / `manifestHash` を片方だけ必須にする | legacy fixture 8 件が通り、片方だけの ref が弾かれること |
| **M41** | 自由文の runtime 検査を外す | 参照 raw の秘密欄由来の 16 文字以上を含む自由文が棄却されること |
| **M42** | 散文 `limitations` を matrix へコピーする | 散文へ仕込んだ canary が matrix へ出ないこと |
| **M43** | `limitationCodes` の enum を開いた文字列にする | enum 外のコードを持つ fixture が棄却されること |
| **M44** | schema の `oneOf` を `dependentRequired` にする | fixture 検証が `unsupported schema keyword` で throw せずに走ること |
| **M45** | `captureRawHash` の再計算を消す | 正規化が伏せる差だけを変えた記録が棄却されること |
| **M46** | `manifestHash` の照合を parse の後に回す | 壊れた manifest を parse する前に棄却されること |
| **M47** | 判定順を逆にして supporting を先に求める | 導けない主張を持つ既存 fixture が `source-test` に留まり、組み立てが失敗しないこと |
| **M48** | evidence root を test から差し替えられなくする | positive control が end-to-end で走ること |
| **M49** | production 入力から evidence root を変えられるようにする | fixture の値で root が動かないこと |
