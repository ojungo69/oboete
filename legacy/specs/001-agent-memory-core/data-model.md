# Data Model: Agent Memory Continuity Platform — Core 1.0

**Date**: 2026-08-12 | **Plan**: [plan.md](plan.md)

正本は v6.1 のスキーマ定義。本ファイルはエンティティ → v6.1 該当章の索引と、
実装フェーズへの割り付けのみ（定義の複製はしない。二重管理防止）。

| Entity | v6.1 正本 | 実装 Phase | 要点（正本の同一性・不変条件） |
|---|---|---|---|
| Project / Workspace / Scope identity | §6 | 2 | opaque UUID 正準。Git 信号は link-candidate 証拠。verified/provisional/conflicted、衝突は fail-closed |
| NormalizedEvent | §8 | 2 | adapterDeliveryId 優先冪等（fallback = schema-versioned fingerprint、揮発 field 除外）。reorder window、late correction invalidation |
| Session（host/process identity, heartbeat） | §10 | 2 | hostId / processStartIdentity / heartbeatSeq による liveness・abandoned 判定 |
| SessionWorkState / SessionLineage | §10–11 | 3 | turn 進行と系譜。mechanical evidence と semantic note の分離 |
| ContinuationCheckpoint | §11 | 3 | revision + claimFence + claimHeartbeatUntil、CAS claim（at-most-one）。taskLineageId + taskBoundaryReason。cross-device 重複は fork lineage |
| DurableMemory | §12 | 3/5 | origin 5 値 + provenanceQuality 3 値。sensitivity は送信除外に直結 |
| CapabilityEvidence / Tier | §7.2 | 0B/4 | native/synthesized/unsupported/unknown + coverage/evidence。Tier は capability_hash + native_version + E2E artifact に付与 |
| Generation contract / run ledger | §13–14 | 6 | 単一 job runner + role/prompt/schema data。80 req/day hard cap（リトライ込み） |
| EmbeddingRequest / per-item ledger | §15 | 7 | item-addressable。unique key = (generation_id, memory_id, memory_revision, input_hash)。catch-up watermark + serialized atomic switch |
| FTS 派生テーブル / RRF | §16 | 5 | contentful dual（unicode61 remove_diacritics 2 + trigram）。score(d)=Σ 1/(k+rank_s(d)) 固定 k/caps/tie-break |
| MCP surface | §18 | 5 | local stdio 5 tools + user-authority CAS。2026-07-28 profile |
| SyncOperation | §22 | — | **Core 1.0 範囲外**（Phase 10）。credentialEpoch/keyId/signature/parentRevisionIds は schema freeze 対象として定義のみ既存 |

## Validation / State transitions

- 状態遷移の正本: checkpoint lifecycle（open → delivered → accepted/dismissed/superseded、
  reopen 含む）= §11、turn state（open tool set / close reason）= §8、identity states = §6。
- Property test 対象（Phase 2–3 Exit）: claim/fence CAS（§27.10）、duplicate x10 / parallel /
  late event（§8）、identity collision matrix（§6）。
