# Implementation Plan: Agent Memory Continuity Platform — Core 1.0（Phase 0A〜8）

**Branch**: `001-agent-memory-core` | **Date**: 2026-08-12 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/001-agent-memory-core/spec.md`

> **正本**: 要件・スキーマ・ゲートの正本は `agent-memory-final-spec-v6.md`（v6.1）。
> 本 plan は v6.1 §29–30 を実行計画（順序・体制・検証・分岐）へ落とすことに徹し、設計を再発明しない。

## Summary

codemem pinned vendor snapshot（commit `26438e75`）をベースに、v6.1 §29 の Phase 0A→8 を
§30 の PR 1–10（1:1 対応）として phase branch 単位で実装し、各 Phase の Exit gate
（決定論的検証のみ）を通過させて Core 1.0（Claude Code + Codex 対応）に到達する。
Phase 0A の base bake-off が不合格の場合のみ、MIT 資産の選択移植へ分岐（v6.1 §4.3 / Phase 0A Exit）。

## Technical Context

すべて v6.1 で決定済み。NEEDS CLARIFICATION なし。

**Language/Version**: TypeScript / Node.js（codemem ベースに準拠。exact toolchain は Phase 0A で凍結）

**Primary Dependencies**: SQLite（FTS5 / unicode61 + trigram）、任意で sqlite-vec v0.1.9（SHA-256/platform pin、既定 off）、MCP SDK（2026-07-28 profile）

**Storage**: ローカル SQLite（daemon 単一 writer、§19）。クラウドは範囲外（Phase 10）

**Testing**: upstream 既存テスト基盤（Phase 0A で確認）+ property tests（claim/fence §27.10、event ordering §8）+ golden matrix fixtures（Phase 0B）+ Track 1 決定論ゲート（§27）

**Target Platform**: Linux / WSL2 ローカル（Windows bridge は Phase 11 で判断）

**Project Type**: ローカル daemon + CLI/hook/MCP アダプタ群（single project）

**Performance Goals**: v6.1 §28（検索 100k 件で FTS-only p95 目標内、ほか §28 の目標値）

**Constraints**: ゼロ増分コスト（80 req/day hard cap）/ 平文外部送信禁止 / fail-closed / runtime data はローカル完結 / source 公開は publication gate 後のみ / tag・package・release は Phase 8 まで禁止

**Scale/Scope**: 記憶 100k 件スケール、Claude + Codex の 4 directed routes（Core 1.0 時点）

## Constitution Check

*GATE: Phase 0 research 前に通過必須。Phase 1 設計後に再評価。*

| Principle | 判定 | 根拠 |
|---|---|---|
| I. ローカルファースト | PASS | 本フィーチャーはローカルのみ（cloud は Phase 10 で範囲外） |
| II. ゼロ増分コスト | PASS | Phase 6 free-certified profile + hard cap。sidecar は certification-gated opt-in、Claude サブスクは恒久除外（付録B.3） |
| III. プライバシー境界 | PASS | redaction/secret detector/daemon auth/credential storage は Claude Code 自ら実装（委譲しない）。Phase 1/5/6 に配置 |
| IV. 安全境界 | PASS | Phase 1 の sole writer 検証が Hard Invariant 4 の blocking gate |
| V. 決定論的ゲート | PASS | 全 Exit gate は Track 1 機械判定のみ（§27/§29） |
| VI. ローカル完結 | PASS（2026-08-14 source visibility 改訂） | runtime/data はローカルのまま。ユーザー決定により publication gate 後の GitHub source/PR だけ許可し、release gate は変更しない |

**Phase 1 設計後の再評価**: PASS（設計成果物は v6.1 への索引であり新規違反なし。2026-08-12）

## Project Structure

### Documentation (this feature)

```text
specs/001-agent-memory-core/
├── plan.md              # This file
├── research.md          # Phase 0 output（v6.1 決定記録への索引）
├── data-model.md        # Phase 1 output（v6.1 スキーマ索引）
├── quickstart.md        # Phase 1 output（Phase 別検証ガイド）
├── contracts/           # Phase 1 output（外部契約の索引）
└── tasks.md             # Phase 2 output（/speckit-tasks が生成）
```

### Source Code (repository root)

```text
vendor/codemem/          # pinned snapshot 26438e75 = product コードの正本（Phase 1 以降ここに直接改変）
evidence/                # Phase 0A 成果物: inventory / SBOM / delta 比較 / base ADR
harness/                 # Phase 0B: hook golden matrix fixtures / sidecar hostile harness
                         # src/ は新設しない（ADR-001: vendor 内 packages/* が実体）
```

**Structure Decision**（2026-08-12 確定 — ADR-001）: base = codemem pinned vendor snapshot
（`26438e75`）。レイアウト: `vendor/codemem/` が product コードの正本（pnpm workspace 構造ごと保持、
Phase 1 以降の改変はここに直接コミット）、`evidence/` = Phase 0A 監査成果物、`harness/` = Phase 0B で
追加する contract harness。`src/` 新設はしない（vendor 内 packages/* が実体のため）。上流追随なし・
cherry-pick は §4.3 手続きのみ（`vendor/codemem/VENDOR.md`）。Phase 0A〜0B は product コードに触れない
（v6.1 の「機能変更なし」「product DB 変更なし」制約）。

## 実行計画（v6.1 §29–30 の実行順序と体制）

各 Phase = 1 branch（`phase-0a-evidence` 等）+ worktree 隔離。GitHub 公開後もマージ条件 = 該当 Exit gate
（spec.md SC-0A〜SC-8）+ 2 本立てレビュー（正しさ → ponytail-review）+ speckit-verify-tasks（委譲回は必須）。

| Phase | ブランチ | 主実行者（委譲ルーティング） | セキュリティ関連（Claude Code 自ら） |
|---|---|---|---|
| 0A Evidence Freeze | phase-0a-evidence | Codex（探索主体: inventory/delta 比較） | auth path 調査の判定・action plan 策定 |
| 0B Contract Harness | phase-0b-harness | Codex（fixture 化は探索主体） | sidecar hostile harness 設計 |
| 1 Safety Boundary | phase-1-safety | **Claude Code 自ら**（全体がセキュリティ関連: auth loader 削除・peer auth・redaction） | 全部 |
| 2 Identity/Event | phase-2-identity | Grok（§6/§8 で変更範囲宣言可）/ 補助 Codex | なし（識別子・状態機械のみ） |
| 3 Continuity | phase-3-continuity | Grok（§11 で範囲宣言可） | claim fence の CAS 検証レビュー |
| 4 Vertical Routes | phase-4-routes | Codex（adapter 完成は探索含む） | なし |
| 5 Retrieval/MCP | phase-5-retrieval | Grok（§16/§18 で範囲宣言可） | MCP peer auth / provenance stripping |
| 6 Generation/Free | phase-6-generation | Codex | provider 資格情報処理・budget enforcement・redaction 経路 |
| 7 Embeddings | phase-7-embeddings | Grok（§15 で範囲宣言可） | sqlite-vec SHA 検証ロジック |
| 8 Core 1.0 Gates | phase-8-release | Claude Code 司令塔 + Codex（gate runner） | signed artifacts / backup 検証 |

補足:

- 委譲プロンプトの正本 = v6.1 該当章 + 当該 Phase の tasks.md。仕様の再説明を委譲文に書かない。
- Codex は `codex-companion.mjs task --write --fresh --background`、Grok は範囲強制が要るとき
  `grok-delegate.sh --allowed-file`。並列委譲はジョブごとに別 worktree。
- 各 Phase 完了時に次 Phase の前提（Exit 成果物）を確認してから進む（フェーズ境界で一時停止・報告）。
- Phase 0A で bake-off 不合格の場合: 本 plan の Structure Decision と Phase 1 以降の実行者配分を
  改訂してから続行（plan 改訂も speckit-plan の再実行ではなく本ファイルの追記で行う）。

## Complexity Tracking

Constitution 違反なし。記載事項なし。
