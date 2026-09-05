# ADR-004: free-mem のライセンスと inbound contribution 方針

- Status: **Accepted**（2026-08-17 に repository owner が決定。ライセンス付与は事後に取り消せない）
- Date: 2026-08-16（決定: 2026-08-17）
- 関連: issue #10（Governance）、#9（namespace 移行と upstream attribution）、#12（Personal Cloud BYOC）
- 前提: ADR-001（vendored codemem を base にする決定）、`THIRD_PARTY_NOTICES.md`

## 背景

repository は public だが、README が明示するとおり **repository 全体への license grant が無い**。
この状態は source-available ではあっても、第三者が利用・改変・再配布・contribute できる状態ではない。
Core 1.0 の package / tag / release を作る前、または外部 contribution を本格的に受け入れる前に確定する必要がある。

本 ADR は候補を証拠付きで比較し、1 つを推奨した。owner は 2026-08-17 に推奨どおり採用を決定した。

## 決定

1. free-mem 独自コードの outbound license を **Apache-2.0** とする。
2. `vendor/codemem/` の MIT 表示は維持し、上書きしない。
3. inbound = outbound とし、**DCO sign-off を必須、CLA は求めない**。
4. AI 生成・AI 支援コードは PR で provenance を申告させる。

## 候補比較

| 観点 | MIT | **Apache-2.0（推奨）** | MPL-2.0 | AGPL-3.0 |
|---|---|---|---|---|
| 商用利用・再配布・改変 | 可 | 可 | 可 | 可（ただし network 配布で source 開示義務） |
| 特許許諾 | **無し**（黙示のみ） | **明示的に有り**（§3。特許訴訟時の終了条項付き） | 有り（§2.1(b)） | 有り |
| notice 義務 | license 文の同梱 | license + NOTICE の同梱（§4） | 改変ファイルの source 開示 | 改変全体の source 開示 |
| vendored MIT との両立 | そのまま | 可（MIT → Apache-2.0 の一方向で取り込み可能。ただし本件は取り込まず vendor 配下を MIT のまま維持する） | 可 | 可 |
| 採用側の摩擦 | 最小 | 小（npm / crates で標準的） | 中（file-level copyleft を嫌う組織がある） | 大（社内利用でも敬遠されやすい） |
| local-first という製品性質との整合 | 中立 | 中立 | 低（competitor 対策としての効果が薄い。ユーザは自分で動かすため） | 低（同左。かつ BYOC 構成のユーザに義務が及びうる） |

### なぜ Apache-2.0 を推すか

- **特許条項が唯一の実質的な差**である。free-mem は checkpoint の claim / fence / CAS といった機構を実装する。
  MIT には特許許諾が無く、利用者は黙示のライセンスに頼ることになる。Apache-2.0 は §3 で明示的に許諾し、
  かつ特許訴訟を起こした側の許諾を終了させる。infra 系 OSS で標準的に選ばれる理由がここにある。
- **copyleft は本製品では効果が薄い**。free-mem は local-first で、ユーザが自分の機械で動かす。
  MPL / AGPL が守ろうとする「改変版を SaaS として提供する第三者」というシナリオは、
  #12 の Personal Cloud（ユーザ自身の Cloudflare アカウントに載せる BYOC）とは異なる。
  義務を課す相手がユーザ自身になりやすく、採用障壁だけが残る。
- **依存関係と衝突しない**（下記スキャン結果）。prod 依存に copyleft-only は無い。
- MIT との差は「特許条項と NOTICE の有無」だけであり、採用側の摩擦は実務上ほぼ変わらない。

MIT を選ぶ合理性があるとすれば、upstream codemem と表記を揃えたい場合と、NOTICE 運用のコストを避けたい場合。
その場合は本 ADR の決定 1 を差し替えるだけでよく、他の決定（DCO・provenance・第三者表示）は変わらない。

## 依存関係ライセンスの実測

```bash
cd vendor/codemem && corepack pnpm install --frozen-lockfile --ignore-scripts
corepack pnpm licenses list --json          # 全体
corepack pnpm licenses list --json --prod   # 配布に載る範囲
```

実測日 2026-08-18、pnpm 11.8.0、lockfile 固定。notice 生成のために `rollup-plugin-license` と
その依存を dev 側へ追加したので、2026-08-16 の実測（全体 455）から dev 側だけが増えている。

| 範囲 | package 数 | 内訳 |
|---|---|---|
| 全体（dev 含む） | 471 | MIT 378 / Apache-2.0 22 / BSD-3-Clause 20 / ISC 20 / BSD-2-Clause 9 / その他 22 |
| prod のみ | 261 | MIT 208 / Apache-2.0 17 / BSD-3-Clause 14 / ISC 12 / その他 10 |

prod の 261 件は 2026-08-16 と同一（増えた分はすべて build 時のみ）。据え置きではなく再実測した値。

判断に関わる個別の事実:

- **copyleft-only は prod に無い**。MPL-2.0 のみの package は `lightningcss` / `lightningcss-linux-x64-gnu` の 2 つで、
  いずれも build 時の依存（viewer の CSS 処理）であり配布物に載らない。
- `dompurify` は `(MPL-2.0 OR Apache-2.0)` のデュアル。**Apache-2.0 側を選択する**と記録する。
- `flatbuffers@1.12.0` は `pnpm licenses list` が `Unknown` と報告するが、これは package.json が
  `"license": "SEE LICENSE IN LICENSE.txt"` を使っているため。同梱の `LICENSE.txt` は Apache-2.0 で、
  upstream は `google/flatbuffers`。経路は `@codemem/core → @xenova/transformers → onnxruntime-web → flatbuffers`。
  **Unknown ではなく Apache-2.0 として扱う**。
- `sqlite-vec` / `sqlite-vec-linux-x64` は `MIT OR Apache` という非 SPDX 表記。**MIT 側を選択する**と記録する。
- `caniuse-lite` は CC-BY-4.0、`mdn-data` は CC0-1.0。いずれも dev 依存のデータセット。
- `json-schema` は `(AFL-2.1 OR BSD-3-Clause)`、`expand-template` は `(MIT OR WTFPL)`、
  `lru-cache` は BlueOak-1.0.0、`tslib` は 0BSD。いずれも permissive で、Apache-2.0 の配布と衝突しない。

## repository 内 material の分類

| 分類 | 場所 | license | copyright |
|---|---|---|---|
| free-mem 独自コード | `harness/`、将来の `src/`・Rust crate | Apache-2.0（本 ADR） | The free-mem Authors |
| upstream codemem 由来 | `vendor/codemem/` | MIT（変更しない） | 2026 Adam Kunicki |
| 仕様・設計文書 | `specs/`、`docs/`、`agent-memory-*.md` | Apache-2.0 | The free-mem Authors |
| evidence / 検証レポート | `evidence/` | Apache-2.0 | The free-mem Authors |
| capability fixture・golden matrix | `harness/fixtures/`、`harness/matrix/` | Apache-2.0 | The free-mem Authors |
| 第三者 dependency（install 時に解決される分） | `node_modules/` | 各 package の license | 各権利者 |
| 第三者 dependency（build 出力に inline される分） | `packages/*/dist/`、`packages/viewer-server/static/`、`plugins/*/scripts/hook-runtime.mjs` | 各 package の license | 各権利者 |
| creative asset（logo 等） | 現時点で無し | 追加時に本表へ追記する | — |

`plugins/*/scripts/hook-runtime.mjs` は npm package の `files` に入らず GitHub の source archive
でだけ配布される。名前と権利者を `THIRD_PARTY_NOTICES.md` に書くだけでは MIT の要求（複製に
copyright 行と許諾文を同梱する）を満たさないので、`sync-hook-runtime.mjs` が bundle と一緒に
生成済みの `THIRD_PARTY_NOTICES.hook-runtime.md` を複製先へ置く。手書きではなく build 出力の
複製なので、bundle の中身が変われば notice も追従する。commit 済みの複製が build 出力と
一致していることは CI の `check` job が build 直後に検査し、`release-tag-preflight.sh` も
ゲートの直後に同じ検査を行う——runner や作業ツリー上では作り直せてしまう以上、HEAD 側が
古いままでも他のゲートは全て通り、source archive にだけ誤りが残る。

`vendor/codemem/` 配下のファイルに free-mem の header を付けない。分類の境界はディレクトリで判定できる状態を保つ。

以前この表には「第三者 dependency は `node_modules/`（配布しない）」の 1 行しか無かったが、
それは install 時に解決される分だけを見た記述だった。bundler が build 出力に inline する分は
**配布物そのものに入る**（実測は下の「配布面のチェック」を参照）。2 行に分けたのはそのため。

## inbound contribution 方針

- **inbound = outbound**。contribution は outbound と同じ license で受け入れる。著作権の譲渡は求めない。
- **DCO sign-off を必須**（`Signed-off-by:` 行）。CLA は求めない。理由: 個人 repository で CLA の管理コストに
  見合う便益が無く、DCO で出所の表明としては足りる。
- **AI 生成・AI 支援コードは PR で申告する**。本 repository 自体が AI agent による実装を含むため、
  禁止ではなく申告を求める。申告内容は「どのツールを使ったか」と「第三者コードの丸写しが無いことの確認」。
- **第三者コードの持ち込みは出所・commit・license の記録を必須**とし、`THIRD_PARTY_NOTICES.md` を同じ PR で更新する。
- **fixture に実データを入れない**（実 credential・私的な memory 内容・ローカル固有パス）。既存の repo 規則と同じ。
- license 確定前に来た外部 PR は、確定後に改めて DCO 付きで出し直してもらう。

DCO の**検査機構**は issue #59 の変更（2026-08-18、`main` へ squash された commit）から稼働する。
検査する集合は、`main` を base とする PR の merge-base より後から head までの commit であり、
稼働時点までに `main` にある既存 history は遡って不合格にしない。

**merge を止める意味での「強制」は 2026-08-18 に開始した。** 開始条件として置いた 3 つは
次のとおり満たした。

1. 下記の残存経路を実測で否定した（PR #86）
2. `dco` を `main-protection`（ruleset 20850916）の required status check へ登録した
3. 未署名 PR が実際に merge を止められることを確認した（PR #87）

強制の機構は `dco` という単一の check である。workflow（`.github/workflows/dco.yml`）も checker
（`harness/dco-check.mjs`）も `main` 側から読み、PR の head は git history としてしか読まない。
この形を選んだ理由は、PR が自分を検査するコードを書き換えられないことにある。あわせて `dco` という
名前の check を出す経路は 1 つに保つ。下記のとおり required status check は同じ名前の check run を
全件見るので、生産者が増えると、そちらが失敗・取り消しになるだけで merge が止まり、どの run が判定
したのかも辿れなくなる。

**免除は置かない。bot も同じく検査する。** author email は commit する側が `git commit --author` で
自由に名乗れるので、email 一致による免除は誰でも騙れる。免除が必要になった場合の拡張先は、email では
なく GitHub が認証した actor login に紐付ける形である。

**同名 check による迂回は成立しない。** 「PR が自分の `ci.yml` に `name: dco` の job を足せば、
trusted な `dco` と区別のつかない成功 check を作れるのではないか」という指摘があったので実測した
（PR #86、2026-08-18）。

required status check の `notices` を対象に、本物の job を落としたうえで、`name: notices` の偽 job を
7 分後に成功させた。他の required check はすべて成功、未解決の conversation は 0 件。この状態で
merge state は BLOCKED だった。偽 job を残したまま本物だけ直すと CLEAN になったので、BLOCKED の原因は
`notices` である。

つまり GitHub は required の名前を持つ check run を**全件**見ており、後から成功した同名 check が
先の失敗を上書きすることはない。

この性質は迂回を塞ぐ一方で、`dco` workflow に `concurrency` を置けないことも意味する。取り消された
run は required check を満たさないので、run が 1 つ取り消されるだけで merge が止まる（PR #88 で
`dco` が `[success, cancelled]` になり BLOCKED を実測した）。`cancel-in-progress: false` にしても、
3 つめの run が来たときに待機中の run が取り消されるため同じことが起きる。PR は base branch 側の `dco` を止められないので、未署名 PR に対する
その失敗は必ず残る。専用 App へ移して `integration_id` で source を固定する案は、この実測により
不要と判断した。

## 配布面のチェック

現時点で package / tag / release を作っていないため、ここは「作るときに満たすべき条件」として定義する。

- GitHub source archive: `LICENSE` / `NOTICE` / `THIRD_PARTY_NOTICES.md` が root にあること（満たしている）
- npm package: `license` フィールドと `LICENSE` の同梱。`vendor/codemem` 由来 package は MIT のまま。
  **加えて、build 出力に inline された第三者コードの notice を同梱する**（下記）
- Rust crate / binary archive: `license` フィールド + `LICENSE` + `NOTICE` の同梱
- SBOM / checksum bundle: release evidence に license scan の結果を含める
- documentation site: footer に license 表記

### bundle された依存の notice（issue #50、2026-08-18 に解消）

各 package の vite build に `rollup-plugin-license` を入れ、**bundler の module graph**を入力に
`THIRD_PARTY_NOTICES.md` を成果物へ出す。生成は `vendor/codemem/scripts/license-notice-plugin.mjs`
に 1 箇所化してある。

| 公開 package | notice の場所 | 収録数（2026-08-18 実測） |
|---|---|---|
| `codemem` | `dist/THIRD_PARTY_NOTICES.md` | 0（`--ssr` build で依存は external） |
| `codemem` | `dist/THIRD_PARTY_NOTICES.hook-runtime.md` | 2（`commander`、`@codemem/core`） |
| `@codemem/core` | `dist/THIRD_PARTY_NOTICES.md` | 1（`hono`） |
| `@codemem/mcp` | `dist/THIRD_PARTY_NOTICES.md` | 0 |
| `@codemem/server` | `dist/THIRD_PARTY_NOTICES.md` | 0 |
| `@codemem/server` | `static/THIRD_PARTY_NOTICES.md` | 47（`preact` / `@radix-ui/*` / `dompurify` ほか） |

`package.json` の `files` は変更していない。`dist/` と `static/` のディレクトリ指定が配下を含むため。

**方式の選択理由**: 成果物を後から測る方式（sourcemap の `sources` を集計する、識別子を grep する）は
採らなかった。sourcemap を出さない成果物を黙って 0 件として通すためで、実際 issue #50 の当初の実測表は
この穴で `hook-runtime.js`（`commander`）を対象から落とし、`static/app.js` については
`@floating-ui/*`・`@preact/signals-core`・`marked`・`tslib`・`aria-hidden`・`get-nonce`・
`react-remove-scroll` 系・`react-style-singleton`・`use-callback-ref`・`use-sidecar` の 14 件を
取り落としていた。

**「bundle せず external にする」を選ばなかった理由**: `static/app.js` はブラウザ向けで external 化が
原理的に不可能なため、notice 生成の工程は結局必要になる。node 側の 2 件（`hono`・`commander`）だけを
external 化しても工程は減らず、cli は `sync-hook-runtime.mjs` と `test:packed-artifact` が bundle 形状に
依存している疑いがあるぶんリスクだけ増える。`@codemem/mcp` は結果的にこの形になっているが、
意図して選んだ設計ではない。

**module graph 方式が届かない 1 件**: `@codemem/opencode-plugin` は rollup build を通らない
（出荷する JS が生成物ではなく commit 済みの原本）ため、module graph からは notice を作れない。
2026-08-18 の実測では第三者コードを含まず、import も外部のみ。生成の代わりに「何も bundle していない」と
述べる notice を package 内に置いて同梱し、tarball 検査そのものは他の 4 package と同じように通す。
原本が変われば diff がレビューに載るので、bundle され始めた場合はそこで気付ける。
当初はこの package を検査対象から外していたが、独立レビューが「`prepublishOnly` を持つのに自分自身は
一度も検査されない」ことを指摘したため、対象へ戻した。

CI ゲート:

- `harness/license-inclusion-check.mjs` — `LICENSE` / `NOTICE` / `THIRD_PARTY_NOTICES.md` /
  `vendor/codemem/LICENSE` の存在、README の SPDX 表記と `LICENSE` の一致、`vendor/codemem` 配下
  package.json の `license` 維持を検査する。**build 出力は見ない**
- `harness/notice-inclusion-check.mjs` — 自分で install と build を行い、公開 package を `pnpm pack` して
  展開し、tarball の中身を検査する。「build 済みなら検査する」形にしていないのは、build 順序に依存して
  黙って素通りするのを避けるため。build の前に `pnpm run clean` で生成先を空にするのは、
  `emptyOutDir: false` の成果物で古いファイルが残り、生成が止まっても受理されるのを防ぐため。
  notice だけを消す形では、notice に載らない古い JS が tarball に残る経路が閉じない。

  **自動的に強制される経路は 2 つ**: CI の `notices` job（PR と push の両方）と、各公開 package の
  `prepublishOnly`（`npm publish` / `pnpm publish` で起動）。3 つ目として任意実行の
  `scripts/release-tag-preflight.sh` もゲートを呼ぶが、これは `pnpm run release:preflight-tag` を
  人が実行したときにだけ走るので、強制力は無い。
  `vendor/codemem/.github/workflows/release.yml` は GitHub Actions が読む位置（repo 直下の
  `.github/workflows/`）に無いため、この repo では起動しない。**`npm publish --ignore-scripts` は
  覆えない**。publish 権限を保護された workflow に限定する件は issue #83

  期待する `name@version` と、その license 本文の SHA-256 digest は `harness/notice-baseline.json` に
  **完全な集合として**固定する。本文を digest で固定するのは、非空かどうかしか見ないと本文が
  別物に差し替わっても通るため。鍵に version を入れるのは、生成側の `multipleVersions: true` に
  合わせるため——名前だけを鍵にすると同名・異 version の 2 件が 1 件へ潰れ、片方の欠落が見えない。
  当初は代表的な数件を名指しする形だったが、独立レビューが「名指ししていない `marked` を 1 件落としても
  通る」ことを実測したため切り替えた。notice ファイルの集合そのものも baseline と突き合わせるので、
  増えた場合も消えた場合も落ちる。両側が空だと比較が 1 件も回らないので、公開 package は notice を
  1 件以上同梱すること自体を条件にしている。依存が正当に増減したときは `--write-baseline` で再生成し、
  その差分を commit に載せてレビューする（`harness/contract-hashes.json` と同じ運用）

## owner の決定（2026-08-17）

1. **outbound license は Apache-2.0**。MIT との比較は上記のとおりで、特許条項を理由に採用。
2. **著作権者の表記は `The free-mem Authors`**。個人名・法人名は使わない。
3. **CLA は求めず DCO のみ**。inbound = outbound を維持する。

license 付与は取り消せない。一度公開した version に対する grant は撤回できず、
後から変更する場合は以降の version にのみ効く。この前提を踏まえた上での決定である。

## 残る未決事項

- release 前の専門家確認（本 ADR は法的助言の代替ではない）。
- ~~DCO は文書に規定しているが CI では強制していない（issue #59）~~ → **2026-08-18 に解消**。
  上の「inbound contribution 方針」の開始点・検査集合・機構・同名 check の実測を参照。
- ~~公開 package の tarball に bundle された依存の notice が載らない（issue #50）~~ →
  **2026-08-18 に解消**。上の「bundle された依存の notice」節を参照。当初この項に書いていた実測
  （`codemem` が `@clack/prompts` ほかを bundle する / `@codemem/server` が `@hono/node-server` を
  bundle する）は再実測で否定された。どちらも `--ssr` build のため依存は external のままで、
  代わりに `hook-runtime.js` と `static/app.js` に漏れがあった。
- **license ファイルを同梱していない依存の扱い**。`static/` の 47 件中 11 件（`@radix-ui/*` の一部など）は
  npm tarball に license ファイルを持たない。現在の notice はその旨と package.json の SPDX 宣言を記録する
  だけで、MIT の許諾条項本文と copyright 表示は載らない。upstream が出していないものを代わりに書くと
  権利者の表示を創作することになるため、現状は「無いことを明示する」に留めている。これで MIT の
  条件を満たすと言い切れるかは**専門家確認の対象**（下の 1 項目目に含む）。issue #81 で追跡する。
- `plugins/{claude,codex}/scripts/hook-runtime.mjs` は git に commit された bundle で、`commander` の
  コードを含むが copyright 表示を持たない。npm package には載らないので tarball ゲートの対象外だが、
  **GitHub source archive では再配布される**。当初は root の `THIRD_PARTY_NOTICES.md` に名前と
  権利者を書くだけで足りると判断していたが、これは誤り。MIT が求めるのは複製に copyright 行と
  許諾文そのものを同梱することで、名前の列挙はそれに当たらない。`sync-hook-runtime.mjs` が bundle と
  一緒に生成済みの `THIRD_PARTY_NOTICES.hook-runtime.md` を複製先へ置く形へ改めた。生成物を追跡対象に
  増やすことにはなるが、build 出力の複製なので bundle の中身が変われば notice も追従し、
  commit 済みの複製が build 出力と一致していることは CI と preflight が検査する。
  **この 2 ファイルを消さないこと**——commander の許諾文を実際に配布側へ届けているのはこれで、
  root の `THIRD_PARTY_NOTICES.md` は経路の説明を担うに留まる。

## 帰結

- 第三者は Core 1.0 を明確な条件で利用・改変・再配布できる。
- vendored codemem の MIT 表示と権利関係は分離されたまま維持される。
- 外部 contribution は DCO を通り、provenance が PR に残る。
