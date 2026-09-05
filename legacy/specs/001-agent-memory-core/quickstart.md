# Quickstart / Validation Guide: Core 1.0（Phase 0A〜8）

**Date**: 2026-08-12 | **Plan**: [plan.md](plan.md)

Phase ごとの「これが通れば次へ進める」検証手順。期待結果 = spec.md の SC-0A〜SC-8
（= v6.1 §29 Exit gate）。コマンドの具体形は各 Phase の tasks.md で確定する。

## 前提

- runtime data はローカル完結。source の push / PR は publication gate 通過後のみ許可し、tag / package / release は Phase 8 まで行わない。
- 候補リポはリポジトリ外（`~/projects/free-mem-vendor/` 等の sibling）にローカル clone し、
  pin commit を checkout してから `vendor/` へ取り込む。
- 実装は worktree 隔離ブランチ上で行い、Exit gate 通過 + 2 本立てレビュー後に main へマージ。

## Phase 0A — Evidence Freeze / Base Bake-off

```bash
# 候補3種を pin して upstream テスト実行（exact toolchain を記録）
# codemem 26438e75 / ai-memory a9e9a24d / remem cde8bc05
```

検証: `evidence/` に inventory（全 DB open / write-capable handle / provider auth / sync・sharing
import の静的一覧）、license/SBOM、fatal/non-fatal 分類、delta 比較、base ADR が揃い、
upstream check/test の実行ログが保存されていること。機能変更ゼロ（product コード差分なし）。

## Phase 0B — Contract Harness

検証: `harness/` の golden matrix（Claude/Codex の exact stable binary で hook lifecycle /
timeout / first injection / compact / tool failure phase / interrupt / subagent を fixture 化、
version-pinned）が再実行で安定 pass。sidecar hostile harness の合否判定が記録され、
不合格なら default disabled 確定。product DB 変更なし。

## Phase 1 — Safety Boundary

検証: runtime DB-open trace + static scan で write-capable handle = daemon のみ（blocking）。
daemon kill / replay / duplicate / spool fault injection tests pass。エージェント動作を阻害しない
（no Agent blockage）。backup restore smoke pass。

## Phase 2 — Identity / Event

検証: identity collision matrix（fork/rename/shallow/worktree/no-remote/monorepo/WSL）pass。
duplicate x10 / parallel・late event property tests pass。

## Phase 3 — Continuity

検証: observer/embedding/sync 全 off で Claude・Codex 各 same-agent continuation 成功。
claim/fence property tests（§27.10）pass — 並行 claim で at-most-one。

## Phase 4 — Vertical Routes

検証: 4 directed routes（Claude ⇄ Codex、self 含む）が memory + checkpoint 両方で pass。
response loss / long turn / simultaneous sessions / managed hook limitation シナリオ含む。
capability profile が evidence hash 付きで出力される。

## Phase 5 — Retrieval / MCP

検証: retrieval gate（§27.6）+ echo-loop test pass。100k 件 scale で FTS-only p95 目標内。
JP 2 文字クエリが exact/n-gram routing で解決。MCP 5 tools の user-authority CAS が競合時に正しく拒否。

## Phase 6 — Generation / Free Candidate

検証: probe manifest + dense/sparse 両 trace で free-certified profile ≥ 1。80 req/day hard cap が
リトライ込みで enforce され、超過時は外部リクエストゼロ + キュー退避。provider swap で data loss なし。
claude-mem importer が canonical rows のみ one-way 取り込み。

## Phase 7 — Embeddings（optional）

検証: generation switch test（build set → catch-up → serialized atomic switch）pass。
extension 不在/SHA 不一致で FTS-only fallback。embedding gate（§27.8）pass。

## Phase 8 — Core 1.0 Release

検証: Track 1 回帰 corpus（20〜30 session）で deterministic gates 全 pass。backup/restore
（manifest 検証・degraded restore 含む）。install/update/uninstall matrix + clean-room install。
72h soak。signed artifacts。以後 schema freeze。
