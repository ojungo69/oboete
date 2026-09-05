# Vendor Snapshot: codemem

- 上流: `kunickiaj/codemem`（GitHub）
- 取り込み commit: `26438e75ce1d0fec6be34981f15045a15c89658b`（upstream `v0.40.2` 相当、2026-08 時点 main）
- 取り込み方法: ローカル clone（`~/projects/free-mem-vendor/codemem`）から `git archive <commit>` で tracked tree のみ展開（`.git` / 生成物 / node_modules を含まない）
- 取り込み日: 2026-08-12
- License: MIT（`LICENSE` 参照。SBOM は `evidence/codemem/sbom.md`）
- 方針: **pinned vendor**。上流追随はしない。取り込みたい上流変更は commit 単位で特定し、write-handle / auth 面の再監査付きで個別 cherry-pick する（ADR-001 / v6.1 §4.3）
- 改変予定: Phase 1 で `evidence/unsafe-path-action-plan.md` の A1–A7（fatal 経路 8 の物理削除 + carve-out 無効化）から着手。以降の改変はすべて本リポジトリのコミット履歴が正
- GitHub fork は作成しない。公開時も `free-mem` 内の pinned snapshot として扱い、上流追随や upstream release を意味しない
