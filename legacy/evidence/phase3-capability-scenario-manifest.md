# Phase 3 capability scenario manifest — v1 の由来

`harness/schema/capability-scenarios.v1.json` の根拠を記録する。addendum §13 は
「Adding or removing a required scenario requires a manifest version bump recorded in the
evidence file」と定めており、本ファイルがその evidence file にあたる。

正本: `specs/001-agent-memory-core/resume-continuity-addendum-v6.2.md`

## manifestVersion 1 — 収録した 7 個の scenario

すべて addendum §8 の次の 1 文を出典とする。

> Tier A requires exact-version proof of hint delivery, claimed prompt-gate delivery, compact
> persistence/fallback, exactly-one compact restore, retry dedupe, crash/restart semantics, and
> size/malformed behavior.

| `scenarioId` | 出典の語句 | `requiredFor` |
|---|---|---|
| `claimed-prompt-gate-delivery` | claimed prompt-gate delivery | `tier_a` |
| `compact-persistence-fallback` | compact persistence/fallback | `tier_a` |
| `crash-restart-semantics` | crash/restart semantics | `tier_a` |
| `exactly-one-compact-restore` | exactly-one compact restore | `tier_a` |
| `hint-delivery` | hint delivery | `tier_a` |
| `retry-dedupe` | retry dedupe | `tier_a` |
| `size-malformed-behavior` | size/malformed behavior | `tier_a` |

manifestHash: `86c3e4a42bc9cb160a83fdacbf7db933a7622b0c3c4ec43854565c5308f69a4d`

`appliesToAgents` は 2 つとも `["claude", "codex"]`。Phase 3 の対象 CLI がこの 2 つだけであるため。

上の表とこの hash が、この版の scenario 集合そのものである。新しい版は節を追加して記録し、
過去の版の節は書き換えない。

これを機械的に強制しているのは 2 つ。`harness/continuity/capability-manifest.test.ts` が
表・hash・manifest の三者を照合し、CI の **Published manifest versions are immutable** が
`origin/main` に既に載っている `capability-scenarios.v*.json` の変更を拒否する。後者が無いと、
manifest と hash と evidence を同じ変更の中で一緒に書き換えれば版を据え置いたまま通ってしまう。
集合を変えるときは新しい版のファイルを追加すること。

### `scenarioId` は導出値である

addendum は scenario の ID を与えていない。上表の ID は出典の語句から機械的に kebab-case 化した
**導出値**であり、正本に文字列として存在するものではない。§13 の manifest check は exact-set
equality なので、生成側（report / matrix）もこの ID 集合に揃える必要がある。ID を変えるなら
manifest version を上げ、本ファイルに記録する。

### `requiredFor` を `tier_a` だけにした理由

出典が「Tier A requires …」であり、addendum はこれらを `generic_phase3` や `automatic_strategy` の
要件としては挙げていない。§13 の union には 3 つの値があるが、正本に根拠のない分類を足すのは
transcription ではなく設計判断になるため、書かれているものだけを採った。

## `manifestHash` の正規化

§13 は `manifestHash` の不一致を preflight 失敗としているが、**算出方法を定義していない**。
再計算できない hash は「一致を確認したつもり」を作るだけなので、ここで規則を固定する。

```
manifestHash = SHA-256hex( JCS( { …manifest, scenarios: scenarioId 昇順 } ) )
```

- 符号化は **RFC 8785 JCS**。正本 `agent-memory-final-spec-v6.md` §22.6 が
  「hash/signature 対象 JSON は RFC 8785 JCS 等の標準 canonicalization を使用し、
  独自 canonical JSON を実装しない」と定めているため、キー順や空白を自前で決めない
- 値の側で決めるのは 2 点だけ。`scenarios` を `scenarioId` の昇順に並べること
  （JCS は配列を並べ替えないので、順序の正規化は値の側の仕事）と、
  `manifestHash` 自身を入力から除くこと
- その「昇順」は **UTF-16 code unit 順**（JCS §3.2.3 が object のキーに使うのと同じ順序）。
  `scenarioId` の形は正本も schema も `string` としか言っていないので、非 BMP の ID を混ぜると
  Unicode scalar 順（Rust の `str` の既定）と食い違い、同じ manifest から別の hash が出る。
  現行の v1 は ASCII のみだが、規則を決めないまま増えると後から直せないのでここで固定する
- scenario のキーは列挙しない。手で並べた列挙は欄が増えたときに黙って入力から漏れる
- 入力の manifest は **I-JSON**（RFC 7493）であること。RFC 8785 §3.1 が canonicalize の
  入力をそこに限っており、§2.3 は object の重複 property 名を禁じている。`JSON.parse` は
  重複を後勝ちで潰すため、潰れた値からは hash が出てしまい、同じファイルを拒否する準拠実装と
  食い違う。読み込みは `harness/schema/jcs.ts` の `readIJsonFile` を通す（UTF-8 の復号も
  fatal にする。RFC 7493 §2.1 が UTF-8 を必須にしており、置換文字で読み替えると
  壊れた bytes から hash が出る）

この規則は `harness/continuity/capability-manifest.test.ts` が強制する。同 test は
再計算して `manifestHash` と照合するほか、scenario の各欄を書き換えると hash が必ず変わること
（＝入力から落ちている欄が無いこと）と、並べ替えでは変わらないことも確認する。

§13 の残りの半分——生成された report / matrix の disposition 集合が manifest と exact-set で
一致すること——はまだゲートが無い。Task 5 の preflight predicate で実装する。

## disposition はここに書かない

§13 の `RequiredCapabilityScenarioV1` に `disposition` フィールドは無く、JSON Schema も
`additionalProperties: false` の closed schema である。§13 は disposition を
「the generated report/matrix」側に置いている。manifest は **何が証明されるべきか** だけを
宣言し、**何が証明されたか** は生成物が持つ。
