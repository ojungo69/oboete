# Specification Quality Checklist: 証拠 digest による real-cli-e2e 昇格の裏付け

**Purpose**: Validate specification completeness and quality before proceeding to planning
**Created**: 2026-08-18
**Feature**: [spec.md](../spec.md)

## Content Quality

- [x] No implementation details (languages, frameworks, APIs)
- [x] Focused on user value and business needs
- [x] Written for non-technical stakeholders
- [x] All mandatory sections completed

## Requirement Completeness

- [x] No [NEEDS CLARIFICATION] markers remain
- [x] Requirements are testable and unambiguous
- [x] Success criteria are measurable
- [x] Success criteria are technology-agnostic (no implementation details)
- [x] All acceptance scenarios are defined
- [x] Edge cases are identified
- [x] Scope is clearly bounded
- [x] Dependencies and assumptions identified

## Feature Readiness

- [x] All functional requirements have clear acceptance criteria
- [x] User scenarios cover primary flows
- [x] Feature meets measurable outcomes defined in Success Criteria
- [x] No implementation details leak into specification

## Notes

- **検証 2 巡目で背景節を全面的に書き直した。** 1 巡目の実測を shared checkout の
  ローカル main（origin/main とは unrelated histories の別物）へ当てていたため、「`evidenceHash` は存在しない」「無条件で `real-cli-e2e` を刻む」「既に 21 cell が
  `real-cli-e2e`」がすべて誤りだった。origin/main 5bbf292 を基点とする worktree で測り直した結果、
  issue #20 の状況記述は正確であり、昇格箇所は 2 箇所ではなく 3 箇所（`assemble.ts:280`/`:349`/`:383`）、
  現在の matrix は `real-cli-e2e` 0 件・`source-test` 21 件だった。FR-017 と SC-008 も
  「維持」から「初めての正当な昇格」へ書き直した。観測記録 16 件は両 tree で byte 一致（`cmp` 全件）
  だったので、正規化規則の実測結果は影響を受けない。
- 検証 1 巡目で 3 件を直した。
  1. FR-009 に「モデルが書く自由文」を具体名（`last_assistant_message` 等）で書いていたため、
     観測欄の実装名に依存しない表現へ改めた
  2. 「正規化案」を FR に直接埋めていた箇所（欄の伏せ字表現の具体形）を FR-013「一意に定まること」
     へ後退させ、具体形は plan / data-model が決める配分にした
  3. SC を「実装が通ること」ではなく件数・割合で測れる形へ書き直した（SC-001〜SC-008）
- 実測に基づく正規化の候補規則は仕様ではなく設計判断なので、`/speckit-plan` の research.md へ回す。
  ただし「どの規則を選んでも FR-009〜FR-011 と SC-003 / SC-004 を同時に満たすこと」は spec 側の制約。
- constitution VI（ローカル完結・push / PR 禁止）は本 repo の現況（GitHub 上で PR 運用中）と
  食い違っているが、その解消は issue #74 の担当であり本仕様では扱わない。
