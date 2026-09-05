# Verify Tasks Report — Phase 0B (T017–T025)

日付: 2026-08-12 / scope: branch `phase-0b-harness` vs `main` + working tree / 対象: 9 tasks

> ⚠️ FRESH SESSION ADVISORY: 実装セッション内での実行（委譲回の必須ゲート）。判定は bash 実測に接地。

## Scorecard

| Verdict | 件数 |
|---|---|
| ✅ VERIFIED | 9 |
| 🔍 PARTIAL / ❌ NOT_FOUND / ⚠️ WEAK / ⏭️ SKIPPED | 0 |

## 検証中に検出・修正した gap（1 件）

- **T018（Layer 3 = negative）**: `harness/schema/capability.schema.json` が `additionalProperties: false` のまま
  `capability` / `sourceEvents` / `coverage` / `limitations`（T020 で `capability.ts` に追加した §7.2 準拠フィールド）
  を欠いており、**実 fixture 8 件すべてが自分の JSON Schema に適合しない**状態だった。
  **修正**: 4 フィールドを追加し、`synthesized` の場合に `sourceEvents` を必須（minItems 1）とする条件節も入れた。
  再検査で「schema covers all fixture fields」「all 8 fixtures conform to schema keys」を実測確認。

## Verified Items

| Task | Verdict | 根拠 |
|---|---|---|
| T017 | ✅ VERIFIED | branch `phase-0b-harness` + worktree で本検証を実行中 |
| T018 | ✅ VERIFIED | `schema/capability.ts` 3,627B（§7.2 逐語 + CaptureFixture）、`capability.schema.json`（上記修正後）、`assemble.ts` 11,358B・`--self-test` PASS |
| T019 | ✅ VERIFIED | `rig/` 4 ファイル。`env -i` + `CLAUDE_CONFIG_DIR` / `CODEX_HOME` / `AGENT_MEMORY_INTERNAL_RUN` の隔離要素を 6 箇所で確認。実 CLI で 14 本の生 capture を取得済み |
| T020 | ✅ VERIFIED | `fixtures/claude/` 5 fixture + raw JSONL。version pin `2.1.228 (Claude Code)` |
| T021 | ✅ VERIFIED | `fixtures/codex/` 3 fixture + raw JSONL。version pin `codex-cli 0.147.0` |
| T022 | ✅ VERIFIED | `matrix/claude.json` / `matrix/codex.json` を assembler が生成（version 不一致なら exit 1 する設計）。unknown cell を残したまま出力していることを実データで確認（claude 3 / codex 4） |
| T023 | ✅ VERIFIED | `certification-decision.md` の ToS 節に一次ソース 3 系統の逐語引用 + 規約 4 文書の掃引結果 + 実装制約 5 項目 |
| T024 | ✅ VERIFIED | `sidecar/` 5 ファイル。`run-tests.sh` = ALL PASS（7 項目。当初 5 項目 → setsid 脱走 / CAP 到達を追加）、`hostile-e2e.sh` 実行で Codex 不合格を実検出。**注意: 本レポート初版の「hostile_env 未適用バグは検出・修正済み」は codex 分岐のみで、claude 分岐は未修正のままユーザーの実 `~/.claude` を汚染していた**（後続の `/code-review` が検出。汚染ディレクトリ削除 + 両分岐修正 + supervisor の setsid 取りこぼし修正を commit 4871b1b で実施） |
| T025 | ✅ VERIFIED | `certification-decision.md` 12,187B。Claude / Codex 各 verdict（両者 未認定 = default disabled）+ 理由の性質差 + 再認定条件 |

## 本レポートの限界（後続レビューで判明）

この verify-tasks は「成果物が存在し、内容が stub でないか」を機械的に確認するもので、**成果物の主張が
正しいか**は検証していない。実際、本レポートが全 VERIFIED を出した後の `/code-review` が 15 件の欠陥を
検出した — 実環境汚染、supervisor の封じ込め穴（setsid 脱走の取りこぼし）、matrix の provenance 捏造、
fixture の事実誤り、実在しないテストの PASS 記載など。**verify-tasks の VERIFIED は「やった」の確認で
あって「正しい」の確認ではない**。正しさは 2 本立てレビュー側の責務。

Layer 4（dead-code）: `assemble.ts` の export は self-test と CLI 経路から到達。rig / sidecar のスクリプトは互いに呼び合う構成で孤立なし。
Layer 5（意味）: ⚠️ Interpretive — 各成果物は stub でなく実測値・実 CLI 出力・逐語引用を含むことを通読で確認。特に certification は「未認定」という**否の判定**を根拠付きで出しており、形だけの合格ではない。

## SC-0B 照合

| SC-0B 項目 | 結果 | 根拠 |
|---|---|---|
| Claude/Codex capability golden matrix（version-pinned） | ✅ | 2 matrix、pin は fixture 単位で強制（不一致は組立失敗） |
| sidecar certification 可否判定 | ✅ | Claude = 未認定（BYOK 不在で E2E 不能）/ Codex = 未認定（hostile E2E 不合格）。§13.6 の default disabled 規定どおり |
| product DB 変更なし | ✅ | `git diff main...HEAD -- vendor/` = 0 行 |
| Tier A を宣言しない | ✅ | 全 matrix の高位 cell は unknown のまま（HI-23） |
