# Phase 0 Research: 証拠 digest による real-cli-e2e 昇格の裏付け

すべて `origin/main` 5bbf292 を基点とする worktree（`feat/evidence-hash-normalization`）
での実測に基づく。推測で書いた項目は無い。

---

## R0. 現状の穴（測定）

`harness/assemble.ts` は 3 箇所で `real-cli-e2e` を刻む。3 箇所とも判断材料は
`Boolean(fixture.evidenceHash)` だけで、値の裏取りは無い。

| 行 | 対象 | 判定式 |
|---|---|---|
| 280 | capture cell | `evidenceKind: hashed ? "real-cli-e2e" : "source-test"` |
| 349 | highLevel cell | 同上 |
| 383 | prompt 対の再刻印 | `evidenceKind: "real-cli-e2e"`（到達条件 `pairFixture` が `f.evidenceHash` を要求） |

`harness/fixtures/*/raw/` を読むコードは 1 行も無い（`assemble.ts:270` のコメントが自認）。
committed fixture で `evidenceHash` を申告しているものは 0 件、現在の matrix は
`real-cli-e2e` 0 件・`source-test` 21 件。**塞ぐべき穴はあるが、汚染された成果物はまだ無い。**

**Decision**: 3 箇所すべてを同じ検査に通す。1 箇所でも残すと別識別子で同じ欠陥が生き残る。

---

## R1. 何が揮発し、何が安定か（16 件の観測記録から測定）

観測記録は 1 行 1 JSON の NDJSON。top-level は `event` / `at` / `payload` の 3 キーのみ。
`payload` に現れるキーは 23 種。

**揮発する（取得のたびに変わる）**

| キー | 変わる理由 |
|---|---|
| `at`（top-level） | 実行時刻 |
| `session_id`, `prompt_id`, `turn_id`, `tool_use_id`, `agent_id` | run ごとに新規発番 |
| `transcript_path`, `agent_transcript_path`, `cwd` | 実行環境の絶対 path。`/tmp/free-mem-rig-$USER/...` を含む |
| `duration_ms` | 実行時間 |
| `model` | 既定モデルの更新で変わる |
| `last_assistant_message` | モデルが書く自由文。同一 scenario の 2 回で「Exit code: \`1\`」と「The exit code is **1**. ...」のように別物になる（`claude-tool-fail` / `claude-tool-fail2` で実測） |
| `tool_input.*`, `tool_response.*` | モデルが組み立てた引数と実行結果。`description` が「Run the false command」と「Run the false command to return a non-zero exit code」で揺れることを実測 |

**安定かつ substantive**

`event` / `hook_event_name` / `tool_name` / `source` / `reason` / `permission_mode` / `agent_type` / `prompt`

`prompt` は scenario が投入する指示そのもので、モデルの出力ではないため安定する。
唯一の例外は注入 scenario の `RIG_INJECT_<token>` で、これは rig の呼び出し側が `INJECT_MARKER`
環境変数で渡す値（`harness/rig/rig.sh:65`）。

**verbatim は位置で決まる。キー名だけでは決まらない。** 実測すると `prompt` は
`payload` の直下（16 件）だけでなく `payload.tool_input.prompt`（2 件）と
`payload.tool_response.prompt`（1 件）にも現れる。後者は Agent tool へモデルが組み立てた
引数とその echo で、モデルが書く自由文にあたる。キー名だけで一致させると `claude-subagent` の
digest が再取得で不安定になり、SC-003 を静かに破る。**verbatim にするのは top-level の `event` と
`payload` 直下の 7 キーのみ**とし、それより深い同名キーは伏せ字にする。

**Decision**:
- 除外は「キー名を列挙して落とす」ではなく **「値を伏せ字に置き換え、キーは必ず残す」** とする。
  キーを落とすと、新しい欄が増えたり消えたりしたことが digest に現れなくなる（FR-010）。
- 既定は伏せ字。verbatim にするのは上記 8 キーだけ。これにより、将来 CLI が追加する未知の欄の
  中身が成果物へ漏れる経路が原理的に無くなる（FR-015 を既定動作で満たす）。
- `RIG_INJECT_[A-Za-z0-9_]+` は `RIG_INJECT_<marker>` へ置換する。marker を verbatim に
  残すと注入 scenario だけ再取得で digest が変わる。

**Alternatives considered**:
- *生ファイル全体の SHA-256*: owner が明示的に不採用。時刻と session id が入るので再取得のたびに不一致になる
- *揮発キーの denylist を作り、残りは verbatim*: 新しい欄が増えたときに既定で verbatim になるため、
  未知の欄経由で絶対 path や会話内容が digest 経由で成果物へ出る。privacy 境界（constitution III）を
  既定で破る向きなので不採用
- *event 名の並びだけを digest にする*: 下の R2 で衝突が多発する（`tool-denied` と `tool-ok` が同じになる）

---

## R2. 提案規則を 16 件へ当てた結果（測定）

規則: 各行を `{event, payload}` にし `at` を落とす / object はキーを全保持して UTF-16 code unit 順に整列 /
`null` と boolean は verbatim / number は `<number>` / string は top-level `event` と `payload` 直下の
7 キーのみ verbatim（marker 置換あり）でそれ以外は `<string>` か `<string:empty>` /
array は要素ごとに再帰し長さを保持 / LF 区切りの NDJSON。

結果は **16 件で 14 種の digest**。一致したのは 2 組だけ。

| 組 | 判定 | 根拠 |
|---|---|---|
| `claude-interrupt3` ≡ `claude-interrupt4` | **正しい一致** | 同一 prompt「Write a 600 word essay about the ocean. Be thorough.」・同一 event 列 `SessionStart,UserPromptSubmit,SessionEnd`。異なるのは session id・時刻・transcript path だけ。**同一 scenario の再取得が同じ digest になることの実データによる証明**（SC-003） |
| `claude-hook-timeout` ≡ `claude-lifecycle-basic` | **正しい一致** | 揮発欄を除くと 2 ファイルの差分が 0 行。hook を 15 秒ブロックしても hook event 列には何も現れない、という観測結果そのものが同一。過剰除外ではない |

一致しなかった重要な組:

| 組 | 意味 |
|---|---|
| `claude-tool-denied` vs `claude-tool-ok` | 別 digest。投入した指示が違う |
| `claude-inject` vs `claude-lifecycle-basic` | 別 digest。注入 scenario と最小 run を取り違えない |
| `claude-tool-fail` vs `claude-tool-fail2` | 別 digest。prompt が実際に違う（「the single command: false」と「exactly: false 」）ので別物として正しい |
| `codex-tool-ok` vs `codex-tool-fail` | 別 digest。`tool_response` が非空文字列と空文字列で分かれる |

digest の実値は実装時に生成して fixture へ埋める。ここに書き写すと、規則を 1 つ調整するたびに
文書側の値が古くなる（実際に本 research の推敲中だけで 3 回変わった）。
検査対象は「何と何が一致し、何と何が一致しないか」であって値そのものではない。

**規則を調整したときの測定履歴**（distinct と衝突の組は 3 回とも同じ）

| 版 | 変更点 | distinct | 衝突 |
|---|---|---|---|
| 初版 | キー名だけで verbatim を判定 | 14 | `hook-timeout`≡`lifecycle-basic`、`interrupt3`≡`interrupt4` |
| 深さ限定 | verbatim を `payload` 直下に限定 | 14 | 同じ（変わったのは `claude-subagent` の値だけ） |
| 相関 token | 識別子を `<id:N>` / `<path:N>` へ | 14 | 同じ（全件の値が変わる） |

**`claude-tool-denied` は実際には拒否を観測していない**。`stdout` が `blocked`、
`last_assistant_message` が `blocked` なので、Bash tool は実行されている。
`permission_denied` を記録している fixture が 1 件も無いのはそのため。
この scenario は「拒否を観測しようとして観測できなかった」記録として扱う。

**`prompt` を verbatim から外した場合の測定**: 衝突が 5 組へ増え、`claude-tool-denied` と
`claude-tool-ok` が同一 digest になった。許可拒否と成功実行の取り違えは substantive な誤りなので、
`prompt` は verbatim 側に必須。

### 識別子の等値関係を残す（レビュー指摘を受けた修正）

上記の規則は揮発する識別子をすべて同じ `<string>` にしていた。これだと等値関係が消え、
`stableNativeSessionId`（session 識別子が run を通して同一か）が原理的に裏付けられなくなる。
証拠を要求したのに証拠から導けない cell が生まれるのは設計の誤り。

`session_id` / `prompt_id` / `turn_id` / `tool_use_id` / `agent_id` は `<id:N>`、
`transcript_path` / `agent_transcript_path` / `cwd` は `<path:N>` へ、**ファイル単位・
初出順**で置き換える。値の中身は出さないので絶対 path も識別子も漏れない。

**測定**: 16 件へ当て直した結果、**distinct 14 種・衝突 2 組**は変わらない。
2 組の内訳（`hook-timeout` ≡ `lifecycle-basic`、`interrupt3` ≡ `interrupt4`）も同じ。
等値関係を残しても区別能力は落ちず、逆に消していた情報が戻る。

**Decision**: 相関 token を含めた規則を `normalizationVersion: 1` として凍結する。

### digest が証明しないこと

同じ event 列・同じ欄構成なら、`tool_response` の中身が成功文でも拒否文でも同じ
`<string>` になる。注入 token が応答へ echo されたかも digest には現れない。
**記録の本文に依存する主張は digest では裏付けられない。** これは正規化の欠陥ではなく、
「再取得で同じ digest になる」ことと本質的にトレードオフの関係にある
（本文を含めれば再取得で必ず変わる）。

対処は 2 つに分ける。

1. **run の素性**（実 CLI か、どの版か、隔離されていたか、記録が失敗していないか）は
   digest ではなく rig が書く manifest で裏付ける（R7）
2. **cell ごとの本文依存の主張**は、raw から値を導く述語が要る。本 issue の範囲を超えるので
   残る穴として明記し、`real-cli-e2e` の定義を実態へ合わせる（data-model.md §0・§4.3）

---

## R3. 共有 normalizer の置き方

取得側は `harness/rig/rig.sh`（POSIX shell）、検証側は `harness/assemble.ts`（TypeScript）。
shell から TypeScript の関数を直接呼ぶ手段は無い。

**Decision**: normalizer を TypeScript のモジュール 1 本にし、
- `assemble.ts` は `import` する
- `rig.sh` は同じファイルを `node` で CLI として起動する（`harness/dco-check.mjs` が既に使っている
  「モジュールとしても CLI としても動く」形と同じ）

**Alternatives considered**:
- *shell 側にも同じ規則を書く*: 二重実装は必ず drift する。「片方だけ直して緑のまま」は
  この repo で既に起きている失敗の形
- *rig を TypeScript へ書き換える*: 本 issue の範囲を超える。rig の隔離設定は shell の
  環境変数注入に強く依存している

---

## R4. 証拠の置き場と path 解決

rig は `$RIG_BASE/capture/{claude,codex}-<label>.jsonl` へ書く（`harness/rig/rig.sh:73,88`）。
`$RIG_BASE` の既定は `/tmp/free-mem-rig-$USER` で、repo の外。
一方 committed 済みの証拠は `harness/fixtures/<cli>/raw/` にある。

**Decision**:
- 証拠置き場は `harness/fixtures/<cli>/raw/` のみとする
- fixture が名指しする値は「置き場からの相対 path」に限る。絶対 path・`..` を含む path は拒否
- 実体解決（realpath）後も置き場の内側であることを確認する。symlink で外へ出る参照を拒否
- rig は capture を置き場へ byte 同一で持ち込む工程を持つ。持ち込んだ後のファイルに対して
  digest を出す（持ち込み前に出すと、持ち込みで内容が変わっても気づけない）

---

## R5. 移行（backfill）

観測記録 16 件と fixture 8 件の対応は scenario 記述から一意に決まる。

| fixture | 観測記録 |
|---|---|
| `claude/lifecycle-basic` | `claude-lifecycle-basic.jsonl` |
| `claude/tool-lifecycle` | `claude-tool-ok.jsonl`, `claude-tool-denied.jsonl` |
| `claude/tool-failed-executed` | `claude-tool-fail.jsonl`, `claude-tool-fail2.jsonl` |
| `claude/injection-and-subagent` | `claude-inject.jsonl`, `claude-subagent.jsonl` |
| `claude/interrupt-and-hook-timeout` | `claude-interrupt.jsonl`, `claude-interrupt2.jsonl`, `claude-interrupt3.jsonl`, `claude-interrupt4.jsonl`, `claude-hook-timeout.jsonl` |
| `codex/lifecycle-basic` | `codex-lifecycle-basic.jsonl` |
| `codex/injection` | `codex-inject.jsonl` |
| `codex/tool-lifecycle-and-failure` | `codex-tool-ok.jsonl`, `codex-tool-fail.jsonl` |

`claude/interrupt-and-hook-timeout` が 5 本を参照する。**証拠参照は配列でなければならない**
（単数欄だと最初の 1 本しか結び付かず、残り 4 本は検査対象から外れる）。

`claude-tool-denied.jsonl` はどの fixture の散文からも参照されていない。`toolFailurePhasesObserved`
に `permission_denied` を記録している fixture も無い。取得はされたが cell の根拠として使われていない
記録なので、`claude/tool-lifecycle` の証拠配列へ含める（同一 scenario 系列の観測であり、
含めても cell の値は変わらない）。

**Decision**: 8 fixture すべてに証拠配列と digest（正規化抜粋と生 byte の両方）を埋める。
**昇格は 0 件**になる（R7 のとおり真正な manifest を作れないため）。降格も 0 件。
埋める意味は、記録を差し替えたら組み立てが落ちるようになること。

---

## R7. run の素性は manifest で裏付ける（レビュー指摘）

SHA-256 の一致はファイル内容の整合性しか証明しない。「実 CLI を隔離 rig で動かした記録だ」は
別の裏付けが要る。現状 `nativeVersion` と `rig.isolated` は fixture の自己申告のまま。

rig は既に材料を出している。

| 材料 | 出どころ |
|---|---|
| CLI の exact version | `$RIG_BASE/capture/<cli>-<label>.version`（`rig.sh:76`, `:88`） |
| recorder の失敗 | `<capture>.errors`（`capture-hook.sh:23`。**無い**ことが正常） |
| run の終了状態 | `<cli>-<label>.exit`（`rig.sh:219`, `:277`。0 以外なら `.stderr` にも `exit=N (recorded)` が残るが、manifest が読むのは `.exit`） |
| 隔離設定 | `rig.sh` の `run_env`（`HOME` / `CLAUDE_CONFIG_DIR` / `CODEX_HOME` の差し替え） |

**Decision**: rig が run ごとに manifest を 1 件書き、observation と一緒に置き場へ持ち込む。
assemble は manifest の**生 byte**から digest を再計算して `ref.manifestHash` と照合し
（parse の前に行う）、closed schema で検証したうえで §2.5 の照合表 **12 項目**を全件見る。
`internalRunMarker` は fixture との一致ではなく `true` そのものを要求する
（双方 `false` にすれば通ってしまうため）。`cliVersion` は `--version` の出力に末尾 LF が
付く（実測）ので、末尾の CRLF か LF を 1 つだけ取り除いてから比較する。
形式と照合表は data-model.md §2.5。

`assemble.ts:402` のコメントが「§13 の manifest hash はまだ無い（Task 5 で入る）。
入る場所はこの配列」と書いているとおり、`capabilityHashInputs` に入れる場所が既にある。

**残る境界**: manifest は rig が書くので、rig を動かす人を信頼する境界は残る。
checkout を信頼するのと同じ水準として §0 に明記する。署名や外部 attestation は
ローカル完結（constitution I / VI）と釣り合わない。

---

## R8. 読み取りの頑健性は既存実装を使う（レビュー指摘）

`harness/schema/jcs.ts` に `decodeUtf8`（不正 UTF-8 を拒否）と `parseIJson`（重複 property を
拒否）が既にある。`harness/continuity/jcs.test.ts:91` が重複キー拒否を確認している。

自前で `readFileSync(..., "utf8")` + `JSON.parse` を書くと 2 つ穴が開く。

- Node の既定 UTF-8 読み取りは不正 byte を U+FFFD へ黙って置換する。壊れた記録が
  正常な記録として digest を得る
- `JSON.parse` は重複 property を後勝ちで潰す。`{"a":1,"a":2}` と `{"a":2}` が同じ digest になる

さらに `harness/rig/capture-hook.sh:15` は hook の stdin を解釈できなかったとき
`payload = {unparsed: raw}` に包む。これは「取れなかった観測」なので正常な payload として
digest に混ぜてはいけない。

**Decision**: 読み取りは `decodeUtf8` + `parseIJson` を再利用する。`payload.unparsed` を
持つ行は失敗にする。正規化の中間オブジェクトは `Object.create(null)` で作る
（通常の `{}` へ `__proto__` を代入すると欄が消え、`__proto__` の有無が digest に現れない）。

---

## R9. 秘密は既に成果物へ出ている（レビュー指摘・実測）

`harness/matrix/{claude,codex}.json` に `RIG_INJECT_5f3a9` がそのまま載っている。
fixture の `limitations` 自由文が matrix へ逐語転記されるため、normalizer を伏せ字にしても
消えない。quickstart の `grep -r "RIG_INJECT_" harness/matrix/` は現時点で失敗する。

**Decision**: **自由文を matrix へコピーしない**。部分文字列の照合は下限より短い秘密を
原理的に取りこぼすので、取りこぼす検査を FR-015 の MUST の担保にはできない。

- fixture へ `limitationCodes: string[]`（schema の closed enum）を新設し、matrix へはコードだけ出す
- 散文 `limitations` は fixture 内に残す（repo 内の読み手向け）
- 自由文の `scenario` も `scenarioId` へ置き換えて matrix から外す
- 部分文字列の照合（参照 raw の秘密欄由来の 16 文字以上）は**設計が破れたときの警報**として残す。
  信頼境界は「自由文を出さない」設計側
- 漏洩 test は canary 方式。散文 `limitations` と raw の秘密欄の各経路へ仕込み、
  子プロセスで組み立てて matrix・stdout・stderr に出ないことを見る

現行の散文は 27 種で、`RIG_INJECT_5f3a9` や subagent の `agent_id`（`aa16b2026df287771`）を
含んでいる。コード化は 1 対 1 の機械的な作業になる。

---

## R10. ビルドと CI の前提（レビュー指摘・実測）

| 前提 | 実測 |
|---|---|
| `harness/tsconfig.json` は `include: ["**/*.ts"]`、`allowJs` / `checkJs` 無し | `.mjs` を import すると implicit any になる。新規モジュールは `.ts` にする |
| `erasableSyntaxOnly` / `verbatimModuleSyntax` が有効 | enum など実行時に効く構文は使えない |
| `harness/package.json` に `scripts` が無い | `npm run harness:assemble` は存在しない。実コマンドを使う |
| CI が `node harness/contract-hashes.mjs` の出力と `harness/contract-hashes.json` を diff（`ci.yml:151-152`） | `capability.schema.json` と fixture を変えたら再生成が必要 |

---

## R6. 退役させる既存記述

`new-decision-doc-must-retire-old-statements` に従い、新しい定義と食い違う既存記述を先に洗い出した。

| 場所 | 現在の記述 | 扱い |
|---|---|---|
| `harness/schema/capability.ts:44` | 「その capture の raw transcript の SHA-256」 | **書き換える**。owner が不採用とした「生ファイル全体の SHA-256」そのものなので、正規化抜粋の digest である旨へ改める |
| `harness/assemble.ts:270-273` | 「evidenceHash が付くまでは弱い証跡種別に落とす」 | **書き換える**。hash の存在ではなく再計算の一致が条件になる |
| `harness/assemble.ts:341-344` | 「Task 2/3 の実 CLI rig が hash を記録したら昇格する」 | **書き換える**。記録するだけでは昇格しない。digest の一致・run 素性の記録・主張の導出一致の 3 つが揃って初めて上がる |
| `harness/matrix/README.md:13` | 「evidenceKind: 全 cell `real-cli-e2e`（隔離 rig 下の実 CLI 実行）」 | **書き換える**。現在の matrix は `source-test` 21 件で、この行は既に事実と違う。移行後の実際の値へ合わせる |
| `agent-memory-final-spec-v6.md:521` | 証拠強度の語彙定義 | **変えない**。語彙は据え置き、判定条件だけを足す |

---

## R7. schema と型と fixture の同時変更

`harness/assemble.ts:147` の `unknown top-level key` 検査により、JSON Schema が知らないキーを
fixture が持つと弾かれる。したがって schema・TypeScript 型・fixture は同一 commit で動かす必要がある。

`harness/fixtures/continuity/*.json` は別種の fixture（`cases` / `intakeContext` を持つ）で、
CaptureFixture の schema 変更の影響を受けない。回帰で確認する。

**Decision**: 「fixture だけ先に新しい欄を足すと落ちる」方向も test で固定する。片方向だけの test は
偽陽性を仕様として守ってしまう（`declarative-constraint-needs-firing-test`）。
