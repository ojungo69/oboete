# Implementation Plan: 継続状態に証跡の置き場を作る（Cluster C）

**Branch**: `feat/continuity-state-schema-v2` | **Date**: 2026-08-17 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `specs/002-continuity-state-evidence/spec.md`

## Summary

凍結された `CanonicalWorkStateV1` / `PendingOperation` に**任意欄だけ**を足し、addendum v6.2 §4.3 が
課す 3 つの検査と、live な集合から外れた証跡を、**状態だけを渡された実装**でも扱えるようにする。

技術的な核は 1 つ: いまの回避策（`operationId` を鍵にした凍結 schema 外の写像）は、schema が
`operationId` に一意性を課していないという事実と正面から衝突しており、そのために「同名の兄弟が
居たら材料なしに倒す」という専用の関数まで持っている。材料を `PendingOperation` の**要素そのもの**
に載せれば鍵が不要になり、この衝突と関数と同期処理がまとめて消える。schema を増やしながら
実装が減る、という形にできる。

## Technical Context

**Language/Version**: TypeScript（`--experimental-strip-types` で Node から直接実行）。
`tsc` は `vendor/codemem/node_modules/.bin/tsc`、設定は `harness/tsconfig.json`

**Primary Dependencies**: 追加なし。標準の `node:test` / `node:assert` のみ

**Storage**: N/A（この feature が触るのは契約とその参照実装だけ。永続化層は未着手）

**Testing**: `node --experimental-strip-types --test harness/continuity/*.test.ts`（現行 259 件）+
`harness/continuity/mutate.sh`（変異ゲート、実行数と生存数を自己検査する）

**Target Platform**: N/A（契約とその参照実装）

**Project Type**: 契約 + 参照実装（single project）

**Performance Goals**: N/A。状態のサイズだけが制約で、SC-005 が上限を要求する

**Constraints**:
- 凍結 schema。生バイトの hash が `harness/contract-hashes.json` に固定され、CI が再生成と突き合わせる
- 追加する配列は §10 `arrayItems`（256）を守る
- 参照実装は純関数（時刻・乱数・連番を使わない）。同じ入力から常に同じ出力

**Scale/Scope**: 変更ファイル 9（schema 2・実装 1・test 2・変異 1・fixture 群・hash・正本 2）

## Constitution Check

*GATE: Phase 0 research の前に通す。Phase 1 design の後に再確認する。*

| 原則 | 適合 | 根拠 |
|---|---|---|
| I ローカルファースト | ✅ | 契約の変更のみ。外部送信経路に触れない |
| II ゼロ増分コスト | ✅ | 生成経路に触れない。依存を足さない |
| III プライバシー境界 | ✅ | 記録に payload を入れない（R-005 / FR-007）。`sensitivity` は退避元から引き継ぎ、孤児は fail-closed の `private`。**入力検証を含むのでセキュリティ関連として扱い、外部 CLI へ委譲せず Claude Code が実装する** |
| IV 安全境界（fail-closed） | ✅ | FR-004 / FR-012 が中心要件。欄が無いことを「合格」と読み替えない。曖昧な入力は上書きでなく衝突 |
| V 決定論的ゲート | ✅ | 追加する検証はすべて機械比較（型・hash 差分・テスト・変異）。LLM 判定を混ぜない |
| VI ローカル完結 | ⚠️ | 下記 |

**VI の逸脱**: constitution は「push・PR 作成を一切行わずローカルで完結する（MUST NOT リモート
公開操作）」と書いているが、この repo は既に GitHub 上で運用され、PR #17 以降のレビューゲートが
プロセスの中核になっている。**constitution の記述が実態に追い付いていない**。この feature の
判断ではなく governance 文書の問題なので、ここでは是正せず、利用者へ報告して別途改定する
（改定は Sync Impact Report + semver バンプが要る）。

## Project Structure

### Documentation (this feature)

```text
specs/002-continuity-state-evidence/
├── spec.md              # 完了
├── checklists/
│   └── requirements.md  # 完了
├── plan.md              # このファイル
├── research.md          # Phase 0 完了（R-001〜R-005）
├── data-model.md        # Phase 1 完了
├── quickstart.md        # Phase 1 完了
└── tasks.md             # /speckit-tasks が作る（このコマンドでは作らない）
```

`contracts/` は作らない。この feature の契約は既に
`harness/schema/continuity.schema.json` が正本として存在しており、そこへの差分を
data-model.md §1〜§3 が定義している。別ディレクトリに写しを置くと正本が 2 つになる。

### Source Code (repository root)

```text
harness/
├── schema/
│   ├── continuity.schema.json      # 正本（生バイトが contract hash の入力）
│   └── continuity.ts               # 同じ形の TypeScript 表現
├── continuity/
│   ├── reference-model.ts          # 参照実装（書き込み・読み取り・記録・退避）
│   ├── reference-model.test.ts     # FR ごとの回帰
│   ├── schema-freeze.test.ts       # 新しい欄・$def の凍結
│   ├── old-shape-projection.ts     # 旧形入力の比較面（SC-003）
│   ├── old-shape-baseline.mjs      # 分岐点の実装で baseline を生成する
│   ├── old-shape-parity.test.ts    # 旧形入力の差分ゲート
│   └── mutate.sh                   # 規則ごとの変異ゲート
├── fixtures/continuity/            # 新旧どちらの形の状態も残す
└── contract-hashes.json            # 再生成する（手で書かない）

specs/001-agent-memory-core/
└── resume-continuity-addendum-v6.2.md   # §4.3 を実装と一致させる（FR-017）

evidence/
└── phase3-reference-model.md       # 索引方式をやめた理由を記録
```

**Structure Decision**: 既存の `harness/` 構成をそのまま使う。新しいディレクトリを作らない。
この feature は既存の契約 1 つと参照実装 1 つへの差分であり、切り出す単位が無い。

## 実装順序（tasks はこの順に割る）

依存関係の都合で、この順序は入れ替えられない。

1. **schema 差分**（`continuity.schema.json` + `continuity.ts` + `schema-freeze.test.ts`）
   — 先に契約を固定しないと、実装が「何を書いてよいか」を決められない
2. **順序材料の移設**（#35）— `PendingOperation` へ書く / 読む、`operationStarts` と
   `startFactsFor` を削除。**この段階で実装が減る**
3. **記録の追加**（#43 / #39）— `droppedEvidence` と 2 つの診断コード
4. **指紋の比較**（#44）— `terminalFingerprint` の書き込みと衝突検出
5. **fixture と contract hash の再生成** — 新しい欄を持つ状態と持たない状態の両方
6. **変異ゲートの付け替え** — 消えた規則の変異を削除、新しい規則の変異を追加
7. **正本の追従**（addendum §4.3 + §0.1 改定行、evidence）— FR-017

2 と 4 は独立なので並べられるが、どちらも同じファイルの同じ関数群を触るので、直列のほうが
衝突の解消コストが低い。

## リスクと先回り

| リスク | 兆候 | 先回り |
|---|---|---|
| 変異のアンカーが実装変更で黙って外れる | `mutate.sh` の 実行 < 期待 | 2 と 6 を同じ commit にせず、6 の直前で必ず 7 本のゲートを通す。ずれたら `comm -23` で特定 |
| fixture を変えて hash を再生成し忘れる | ローカル緑・CI だけ赤 | quickstart の順序（hash 差分をテストより先）を守る |
| 「欄が無い」経路の回帰が漏れる | 旧 fixture が緑のまま意味が変わる | FR-004 / FR-012 / FR-014 を**変更前の実装との突き合わせ**で示す（SC-003 / T049 の差分ゲート）。書き換えた期待値の行数は判定に使わない |
| 差分ゲートの比較面が実際より狭い | 門は緑だが振る舞いの一部が比較されていない | 比較面は還元結果を素通しし、欄を選び直さない。狭める方向の変異を面ごとに 1 件ずつ置く |
| baseline を手で書き換えて門を通す | fixture と contract hash を両方直せばローカルは緑 | CI が基準 commit から再生成して committed fixture と `diff` する（quickstart のゲート 7） |
| 記録の追加で状態が単調増加する | `droppedEvidence` が 256 を超える | FR-015 の変異（上限検査を外す）を必ず入れる |
| 索引削除で fail-open を作る | 曖昧な状態が「材料あり」になる | 削除するのは鍵の曖昧さ由来の分岐だけ。`terminal_order_unverifiable` へ倒す経路は残す |

## 実装の担当

`rules/coding.md` の委譲ルーティングを当てると **Claude Code が自ら実装**する。理由は
Constitution III と `rules/security.md` の対象範囲（入力検証・fail-closed の境界）に当たるため。
外部 CLI（Codex / Grok）へは出さない。レビューは 2 本立て（`/code-review` → `ponytail-review`）。

## 前提条件（実装開始の条件）

`main` に PR #51・#52・#55 が入っていること。3 本とも `harness/continuity/` と
`contract-hashes.json` を触るので、先に入れないと衝突の解消が本題より高くつく。
この plan までは先行して構わない（`specs/` しか触らないため）。

## Complexity Tracking

Constitution 違反は無いが、R-002 で「版を上げない」という判断をしたので、**次に schema を触る回の
判断材料**として条件をここに転記する。以下のどれか 1 つでも満たしたら、次の変更は任意欄の追加では
済ませず、本物の版（`CanonicalWorkStateV2`）を作る。

| # | 条件 | なぜそこが境目か |
|---|---|---|
| 1 | `CanonicalWorkStateV1` を永続化した状態が repo の外に出た | 次の版が「前の版が書いたファイル」を読む必要が生じる。任意欄の有無では移行を表せない |
| 2 | 2 つ目の実装が `contract-hashes.json` に固定された | 片方だけが新しい欄を書く期間が必ず生じ、版で区別できないと調整できない |
| 3 | 既存欄の意味・必須性・値域を変える | 任意欄の追加は既存の読み手の判断を変えないが、制約強化は変える |
| 4 | 欄を削除・改名する | 同上。`additionalProperties: false` の下では読み手が壊れる |
