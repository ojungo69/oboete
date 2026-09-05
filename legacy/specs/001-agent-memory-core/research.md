# Research: Agent Memory Continuity Platform — Core 1.0

**Date**: 2026-08-12 | **Plan**: [plan.md](plan.md)

本フィーチャーの research は完了済みであり、成果物は v6.1 仕様書本体に統合されている。
Technical Context に NEEDS CLARIFICATION は残っていない。本ファイルは決定記録の索引。

## 決定の出所（時系列）

1. **v5→v6 統合 + ユーザー決定 6 件** — v6.1 付録A（2026-08-12）。
   スコープ・課金方針（ゼロ増分コスト）・プライバシー既定などの製品判断。
2. **Codex 壁打ち**（2026-08-12、report: `codex-review-report-2026-08-12.md`）—
   verdict: proceed-with-blockers。blocking findings 13 件 + capability matrix + 実装順提案。
3. **23 エージェント反証検証** — refuted 0 / confirmed 6 / partially_confirmed 7。
   全 13 件を採用（7 件は縮小採用）。採否と縮小理由は v6.1 付録B.2、報告書側の誤りは付録B.4。
4. **実装 gate 判定**（2026-08-12）— PASS。確定 blocker は仕様反映済み、実装レベル検証は
   Phase 0A/0B/1 の Exit gate に符号化。

## 主要決定（Decision / Rationale / Alternatives）

| Decision | Rationale | Alternatives considered |
|---|---|---|
| ベース = codemem pinned vendor snapshot `26438e75` + §4.3 base gate | 既存資産（DB/検索/hook 基盤）再利用が最速。gate で unsafe path を機械判定 | ai-memory `a9e9a24d` / remem `cde8bc05` / greenfield（Phase 0A bake-off で最終確定。全面 greenfield は不採用と決定済み） |
| Claude サブスク sidecar 恒久除外、`profile_resolution_order = ["local","cloudflare-free"]` | Anthropic 規約が Free/Pro/Max 資格情報の第三者ルーティングを明示禁止（一次ソース検証済み、v6.1 付録B.3） | サブスク優先案（ユーザー決定④原案）— 法的に実装不可のため除外。**要ユーザー確認事項として記録済み** |
| Codex sidecar は certification-gated opt-in（§13.6、ToS 確認含む） | 隔離保証の文書化された契約が Codex に無い + provider ToS 未検証 | 既定有効化（hostile config 経由の事故リスクで却下） |
| 埋め込み既定 off / sqlite-vec v0.1.9 SHA pin / LanceDB 削除 | FTS-only で 1.0 品質を先に確定。ネイティブ拡張の供給リスクを SHA pin で封じる | LanceDB（供給・サイズ・複雑性で削除）、既定 on（コスト・障害面で却下） |
| FTS = contentful dual（unicode61 + trigram）+ fixed RRF + CJK routing | JP/EN/mixed の正しさを決定論的に検証可能にする | 外部形態素解析（依存増で却下）、embedding 前提検索（既定 off と矛盾） |
| Track 1 = 機械判定のみ blocking / Track 2 = 事前登録・封印 holdout | 主観評価の blocking 化は再現性がない | LLM judge を blocking に使う案（却下） |

## 未解決事項の扱い

真に E2E でしか確定しない論点（v6.1 §33 に列挙: 実バイナリの hook 挙動・timeout 実測・
sidecar 隔離の実挙動など）は「research の残課題」ではなく **Phase 0A/0B の成果物**として
計画に組み込み済み。ここで追加調査はしない。
