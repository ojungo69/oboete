# Contracts: Agent Memory Continuity Platform — Core 1.0

本ディレクトリには 2 種類の文書がある。

1. **設計契約の索引**（下表）— 正本は v6.1。ここでは索引のみを持ち、複製しない。
   契約の実体ファイル（JSON Schema / fixture）は実装 Phase で `harness/` と `src/` 側に生成し、
   Exit gate の検証対象にする。
2. **凍結された実装契約**（`*-v1.md`）— Phase 1 の TypeScript 実装が**現に何をしているか**の記述。
   設計意図ではなく実装挙動が対象で、[ADR-003](../../../evidence/adr-003-rust-local-core.md)（#1 Stage 0）に基づき
   Rust 再実装が観測可能な挙動を一致させるための正本として凍結する。

## 凍結された実装契約（Stage 0）

| 契約面 | ファイル | 凍結元 |
|---|---|---|
| local RPC wire contract（socket / handshake / method 一覧 / size bound / deadline / typed error） | [rpc-v1.md](rpc-v1.md) | vendor/codemem Phase 1 |
| sole-writer 不変条件（audited entry point / writer lock / identity / maintenance / migration / cutover / static scan） | [writer-boundary-v1.md](writer-boundary-v1.md) | 同上 |
| spool on-disk format（layout / 命名 / encoding / idempotency / quota / quarantine / sweeper） | [spool-format-v1.md](spool-format-v1.md) | 同上 |
| error taxonomy と fail-open 契約（typed code / hook 挙動 / exit code / doctor 表示） | [error-taxonomy-v1.md](error-taxonomy-v1.md) | 同上 |

これらは「現状の記述」であり改善提案を含まない。実装のギャップは各文書の Known gaps に記録し、
再実装が再現すべき挙動か否かを明示する。TypeScript 側の挙動を変える場合は、対応する `*-v1.md` を
同じ PR で更新する（更新は version を上げるか、v1 の該当行を diff 付きで訂正する）。

## 設計契約の索引（v6.1 正本）

| 契約面 | v6.1 正本 | 消費者 | 実装 Phase |
|---|---|---|---|
| Agent Adapter Contract（hook lifecycle / capability 申告 / adapterDeliveryId） | §7 | Claude Code / Codex adapter（0B で fixture 化、4 で完成） | 0B, 4 |
| Event Intake API（daemon RPC。schema allowlist / size bound / version handshake） | §8, §19 | hook / CLI / MCP クライアント | 1, 2 |
| Checkpoint claim API（CAS: id+revision+fence+destination、heartbeat lease） | §11 | resume 経路（memory_resume / viewer） | 3 |
| MCP surface（local stdio 5 tools + user-authority CAS、2026-07-28 profile: POST-only Streamable HTTP / Origin / header-body cross-validation は remote 時） | §18 | エージェント（MCP クライアント） | 5 |
| Generation contract（role/prompt/schema data、run ledger、client-side schema validation） | §13–14 | job runner ↔ provider adapters | 6 |
| Embedding contract（EmbeddingRequest / EmbeddingItemResult、per-item ledger） | §15.2 | embedding pipeline | 7 |
| Sidecar certification manifest（ToS 確認・effective_config・hostile fixture・process-tree/FD・JSON 耐性） | §13.6 | sidecar 有効化判定 | 0B（判定）, 6（optional PR） |
| claude-mem importer（tag-pinned one-way、canonical rows のみ） | §14 | importer CLI | 6 |
| Configuration surface（profile_resolution_order / free_profile_batching / embedding backend） | §24 | インストーラ / ユーザー設定 | 1–6 で段階導入 |

範囲外（Core 1.0 では契約定義のみ・実装なし）: Personal Cloud sync protocol（§22）、remote MCP（§18/§22）。
