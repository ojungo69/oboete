# 変更前の baseline（T004 の成果物）

SC-005（秘密が成果物へ出ない）と SC-008（証拠強度が変化した cell が 0 件）は、
変更後にこの記録と突き合わせて判定する。計測日は commit 日時、基点は `origin/main` 5bbf292。

## (a) 現在の evidenceKind の内訳

| matrix | evidenceKind | 件数 |
|---|---|---|
| `harness/matrix/claude.json` | `source-test` | 12 |
| `harness/matrix/codex.json` | `source-test` | 9 |

**`real-cli-e2e` を名乗る cell は 0 件**。したがってこの変更で降格する cell も 0 件になる。
issue #20 本文の「既存成果物に偽の証跡はない」はこの点で正しい。

## (b) 成果物に出ている秘密（変更後は 0 件になる）

| 文字列 | 出現箇所 | 件数 |
|---|---|---|
| `RIG_INJECT_5f3a9` | `harness/matrix/claude.json` | 1 |
| `RIG_INJECT_5f3a9` | `harness/matrix/codex.json` | 1 |
| `aa16b2026df287771` | `harness/matrix/claude.json` | 1 |

どちらも fixture の散文 `limitations` が `harness/assemble.ts:295` で逐語転記された結果で、
normalizer を伏せ字にしても消えない（data-model.md §5.3）。

数え方: 上の表は「文字列と出現ファイルの組」単位で **合計 3 件**（`RIG_INJECT_5f3a9` が 2 ファイル、
`aa16b2026df287771` が 1 ファイル）。異なる文字列としては 2 種類。spec.md の SC-005 は前者で数える。
なお quickstart.md §6 の実際の検査は `assert_no_match` で grep の終了コード 1 だけを「一致なし」とし、
一致ありと対象欠落・読み取り失敗をともに失敗させる。件数ではなく有無だけを見ており、この数値に
依存する自動ゲートは無い。

## (c) 観測記録 16 件の inventory

`captureRawHash` の backfill（T031 / T032）はこの表の値を使う。

| 置き場 | ファイル | bytes | 行数 | 生 byte の SHA-256 |
|---|---|---|---|---|
| `harness/fixtures/claude/raw` | `claude-hook-timeout.jsonl` | 1711 | 4 | `c09c0fea350ce06b0ecd7aa98db4b42459def6164130d7a3d84502781d8a9d87` |
| `harness/fixtures/claude/raw` | `claude-inject.jsonl` | 1819 | 4 | `00c71abe7e54d2560026cce5722cf39e203cf30e21576f1b370022b1840e7213` |
| `harness/fixtures/claude/raw` | `claude-interrupt.jsonl` | 2640 | 4 | `5cade0369306b5bbbf15c679e777ddf1211621ff9dca8dee131f5020ce04d1cf` |
| `harness/fixtures/claude/raw` | `claude-interrupt2.jsonl` | 1264 | 3 | `acf7f52855aa7cea9fa42d88c30b08a7983f8fb187e201b91c79682c11f402be` |
| `harness/fixtures/claude/raw` | `claude-interrupt3.jsonl` | 1245 | 3 | `35cf5d6cd90e5347f074249d3a5df8f8f39f1a2fb6fcb150cba75a2aeb1da417` |
| `harness/fixtures/claude/raw` | `claude-interrupt4.jsonl` | 1245 | 3 | `0b6e936b7fc0ff05549be53e990451e677a07ab94408e1e190a73ca280259ae5` |
| `harness/fixtures/claude/raw` | `claude-lifecycle-basic.jsonl` | 1711 | 4 | `7f3a47b1e27e54bbf2157b8facfcc050a1dbd6d77439337760ed8471c8c5202e` |
| `harness/fixtures/claude/raw` | `claude-subagent.jsonl` | 5995 | 9 | `6f2b48ae1654b1f85644862df739e2d15db3cde7c85d3275cf1b430753cf8fcc` |
| `harness/fixtures/claude/raw` | `claude-tool-denied.jsonl` | 3035 | 6 | `f00e7cc19e6b97af361741276bbfd4b03cc319d9f0e9b6b4c7cb4c377b1416c4` |
| `harness/fixtures/claude/raw` | `claude-tool-fail.jsonl` | 2487 | 5 | `99dc2d63ede38499e16369978666153754e9a9ae0d1ef6a17347989392d08e16` |
| `harness/fixtures/claude/raw` | `claude-tool-fail2.jsonl` | 2439 | 5 | `5f168b009d0a109861725af13e95ecff32ce52f8444ffd61f6700920b4b9809b` |
| `harness/fixtures/claude/raw` | `claude-tool-ok.jsonl` | 2992 | 6 | `ea8485c3cb9586930570c5e4bafc2214065a4a6733e01a75082db002189d4cdc` |
| `harness/fixtures/codex/raw` | `codex-inject.jsonl` | 1840 | 4 | `e48bc3310c295a865b14a424cc7892a63b775d3afa0f9524124bd6a26b13ca80` |
| `harness/fixtures/codex/raw` | `codex-lifecycle-basic.jsonl` | 1751 | 4 | `09a006b93306daae568e2ba77b8660b9ac6f385077bf9bd789007b1219fe5ef0` |
| `harness/fixtures/codex/raw` | `codex-tool-fail.jsonl` | 2938 | 6 | `067269f7de6d1bc31eb7f909dec71380426229211095e534a973e7c2653a4b55` |
| `harness/fixtures/codex/raw` | `codex-tool-ok.jsonl` | 2938 | 6 | `1aaefa4f701901b10fd8ecadcdc4112ef45179e21693a116e4134bd3dd589cd4` |

合計 16 件 / 38050 bytes。

## (d) 現在の `evidenceHash` 参照

`grep -rn evidenceHash harness/` は **95 件**（うち `harness/matrix/` の生成物が 44 件）。
T019 の完了時に、残るのは意図した形だけになる。
