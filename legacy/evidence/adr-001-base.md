# ADR-001: 実装ベース = codemem pinned vendor snapshot（26438e75）

日付: 2026-08-12 / 状態: **Accepted** / 判定枠: v6.1 §4.3 base gate

## 決定

Core 1.0 の実装ベースとして **codemem `26438e75ce1d0fec6be34981f15045a15c89658b` の pinned vendor snapshot** を採用する。fork（上流追随）はしない。上流の後続変更は §4.3 の手続き（個別 cherry-pick + 再監査）でのみ取り込む。

## §4.3 gate 判定

| gate 条件 | 結果 | 根拠 |
|---|---|---|
| write-handle inventory 完了 | **PASS** | `codemem/write-handle-inventory.md`（T007、file:line 全数、Appendix 含む） |
| fatal / non-fatal 分類完了 | **PASS** | `codemem/write-handle-classification.md`（T010: fatal 計 10 経路 = auth 系 5 + sole-writer 系 5（F1–F8 + 補遺 F5'/F6'）、いずれも局在・除去可能） |
| delta 比較記録 | **PASS** | `delta-comparison.md`（T011: vendor が 4 指標すべてで fork / greenfield に優位） |
| unsafe path 除去可能性 | **PASS（見込み）** | fatal 計 10 経路は observer-auth.ts / observer-client.ts（auth 5）+ hook 内 open ×4 + template ×1（sole-writer 5）に局在。除去後も BYOK HTTP 経路が独立残存することを dispatch 構造で確認。実除去と blocking 検証（runtime DB-open trace + static scan）は Phase 1 Exit — 完了判定は 10 経路 → 0 を基準にする |
| upstream 実行可能性 | **PASS** | ホスト実測 4028/4037 pass（fail 6 は環境依存と確認済み）。tsc / biome green。toolchain: node v24.16.0 / pnpm 11.8.0（corepack） |

## 理由（要約）

1. **スタック一致**: v6.1 決定の TS/Node/SQLite と一致する唯一の候補。Rust 2 候補（ai-memory / remem）はコード移植不可＝実質 greenfield。
2. **資産量**: DB/schema bootstrap・FTS・Claude/Codex/OpenCode hook 配線・MCP stdio・viewer・sqlite-vec **0.1.9**（v6.1 pin と同一版）を直接再利用可能。
3. **安全性の到達可能性**: sole-writer 違反と unsafe auth 経路（計 10）はすべて特定可能・局在（T010）で、Phase 1 の物理削除計画（T012）が立つ。
4. **fork でなく vendor**: carve-out（sync/coordinator/sharing 全面）が上流と恒常競合するため、追随は rebase 地獄になる。pin 固定 + 選択 cherry-pick が唯一維持可能な形。

## 却下した代替案

- **ai-memory ベース**: WriterHandle actor は Hard Invariant 4 の理想実装だが Rust 全面のためスタック不一致。**設計参照元として採用**（Phase 1 daemon writer は actor パターンで実装）。
- **remem ベース**: write 経路 101 箇所・単一 writer 境界なしで、要求境界から最遠。SQLCipher 運用と pack 形式は将来参考。
- **greenfield（MIT 資産選択移植）**: gate 不合格時の予備案だった。gate 合格につき不発動。4,037 件級（HOST RETRY2 実測 4028 pass）のテスト資産をゼロから再構築するコストが決定的。

## 帰結

- `vendor/codemem/` に `.git` を含まない snapshot を取り込み（T014）、出所を `VENDOR.md` に記録。
- Phase 1 で T012 の A1–A7 を実施（fatal 計 10 経路の除去 + carve-out 無効化）。Exit は Hard Invariant 4 の blocking 検証。
- 上流 codemem の脆弱性修正など取り込みたい変更が出た場合: 対象 commit を特定し、write-handle / auth 面の再監査付きで個別 cherry-pick（§4.3）。
