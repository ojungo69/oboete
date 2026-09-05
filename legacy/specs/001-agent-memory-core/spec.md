# Feature Specification: Agent Memory Continuity Platform — Core 1.0（Phase 0A〜8）

**Feature Branch**: `001-agent-memory-core`

**Created**: 2026-08-12

**Status**: Ready for planning with Phase 3 preflight

**Input**: User description: "正本仕様は agent-memory-final-spec-v6.md (v6.1) 全体。今回のフィーチャー範囲は Phase 0A〜8 (Core 1.0, PR 1-10) の実装。v6.1 §29-30 のフェーズ定義と exit gate を要件として採用し、Phase 3 continuityは2026-08-16のresume preflight addendumで安全性・品質契約を補強する。"

> **正本と優先関係**: 要件・スキーマ・ゲートの基礎正本は
> `agent-memory-final-spec-v6.md`（v6.1、リポジトリルート）である。
> `resume-continuity-addendum-v6.2.md` は §7 / §10 / §11 / §17 / §27 のcontinuity詳細だけを
> 限定的に上書きし、その範囲ではaddendumが優先する。その他はv6.1が優先する。
> ユーザー決定6件はv6.1付録A、Codex壁打ち反映（B-01〜B-13）は付録B、
> Phase 3のOSS比較・採否は `evidence/phase3-resume-oss-comparison.md` に記録する。
> 本ファイルは各フェーズの受け入れ基準を索引化し、正本の型や状態遷移を重複定義しない。

## User Scenarios & Testing *(mandatory)*

### User Story 1 - 作業の中断と再開（継続性） (Priority: P1)

コーディングCLI（Claude Code / Codex）のユーザーが、セッション中断（クラッシュ・compact・
明示終了）の後に新しいセッションを開始すると、直前の作業状態（チェックポイント）が提示され、
現在のprompt・workspaceと互換性があり、かつ**そのexact Agent / native CLI version / capability hashが
要求されたdelivery strategyを実機E2Eで証明している場合だけ**自動復元される。未証明・unsupported・
`unknown_after_test`の経路は、hint・candidate list・manual recoveryへ安全にdowngradeする。
同じcheckpointは並行sessionへ二重配布されず、誤resumeは最初の正常turnだけでacceptedにならない。

**Why this priority**: プラットフォームの中核価値。これが成立しない限り他の全機能は意味を持たない。

**Independent Test**: observer / embedding / syncをすべて無効にした状態で、Claude CodeとCodexの
same-agent continuationが、それぞれの証明済みstrategyまたは明示downgrade経路で成功すること。
typed task state、pending operation、workspace reconciliation、initial claim、delivery engagement、
atomic acceptance、safe capsule、selection decisionの全deterministic gateを通ること（Phase 3 Exit）。

**Acceptance Scenarios**:

1. **Given** 作業途中のsessionがcrash、**When** 同一Agentで新sessionを開始、**Then** crash-recovery checkpointが提示され、exact capabilityとworkspace照合に応じてfull / verification / hint / manualへ決定論的に分岐する
2. **Given** open checkpoint 1件と並行session 2本、**When** 両方が同時claim、**Then** checkpoint projectionとattempt作成のatomic CASにより一方だけがactive fenceを取得する
3. **Given** checkpoint配送後に別タスクを開始、**When** 最初のturnが正常終了、**Then** 関連engagement evidenceがないため旧checkpointはacceptedにならない
4. **Given** command開始後にterminal eventなしでcrash、**When** resume capsule生成、**Then** operationは`unknown / verify_first`として表示され、自動再実行されない
5. **Given** checkpoint時点からHEAD・dirty tree・対象fileが非互換、**When** smart resume評価、**Then** full checkpointは自動注入されず、照合理由と確認候補だけが提示される
6. **Given** heuristicが新task boundaryを提案、**When** user/runtime confirmationなし、**Then** 旧task lineageは削除・supersedeされない
7. **Given** Agent textが「続きとして完了した」と主張、**When** 関連runtime evidenceなし、**Then** explicit acceptとして扱われない
8. **Given** capsule markerを模したmalicious historical text、**When** capture、**Then** owned ledger/hash/bytes/schema検証に失敗して自動strip・memory promotionされない
9. **Given** private/secret work-state field、**When** automatic capsule生成、**Then** policy未許可privateと全secret値がwrapper bytes・logs・reportsへ出ない

### User Story 2 - 記憶の蓄積と検索（メモリ） (Priority: P2)

ユーザーの作業から抽出・手動登録された記憶（DurableMemory）がローカルに永続化され、
新しいsession開始時・明示検索時に、日本語・英語・混在queryで正しく検索・注入される。
Derived observationはsource evidenceを保持し、古い事実は無言上書きせずrevision・validity・失効履歴を持つ。

**Why this priority**: 継続性（P1）の次に価値が高い。検索正しさはPhase 5で独立検証可能。

**Independent Test**: vector無効（FTSのみ）のまま100k件スケールでJP/EN/mixed retrieval gateと
echo-loop testが通り、presentation-level dedupeでもsource evidenceが失われないこと（Phase 5 Exit）。

**Acceptance Scenarios**:

1. **Given** 日本語2文字query、**When** search、**Then** exact/n-gram routingでtrigramの3文字未満不一致を回避して結果が返る
2. **Given** 注入済みcontextを含むconversation、**When** event intake、**Then** self-ingestion preventionで注入内容が再抽出されない
3. **Given** consolidated observationとsupporting factが同時候補、**When** context pack生成、**Then** packは重複抑制できるが保存済みsource evidenceを削除しない
4. **Given** 更新・訂正されたfact、**When** current search、**Then** invalidated revisionはdefaultから除外/down-rankされるがhistoryでは追跡できる

### User Story 3 - エージェント間の作業引き継ぎ（クロスエージェント） (Priority: P3)

Claude Codeで中断した作業をCodexで再開できる（逆方向も）。4つのdirected route
（self含む）すべてでmemory + typed checkpointが引き継がれる。

**Why this priority**: 差別化価値だが、P1成立が前提。Phase 4で検証。

**Independent Test**: 4/4 route scenarioがpassし、source/destination Agentのcapability差が
strategyとTierへ正しく反映されること（Phase 4 Exit）。

**Acceptance Scenarios**:

1. **Given** Claude Code作成checkpoint、**When** Codex sessionが受け入れ、**Then** capability差分はnative/synthesized/unsupported/unknown + exact evidence付きで扱われる
2. **Given** destination Agentがprompt-aware injectionを証明できない、**When** smart resume要求、**Then** source inspectionだけでTier A扱いせず、証明済み`session_start_full` / `next_prompt_synthesized` / `manual_only`へ落とす
3. **Given** synthesized prompt deliveryの片側証拠だけ存在、**When** capability matrix生成、**Then** profileはinvalidとして拒否される

### User Story 4 - ゼロコスト運用（無料枠生成） (Priority: P4)

記憶抽出・要約などの生成処理は、認定済み無料profile（hard cap 80 req/day、retry込み）で
コスト0円で動作する。cap超過時は抽出をqueue退避し、core continuation/searchは劣化しない。

**Why this priority**: generationなしでもtyped checkpoint・searchは機能する。

**Independent Test**: probe manifest + dense/sparse traceで少なくとも1つのfree-certified profileが認定され、provider swapでdata lossがない（Phase 6 Exit）。

**Acceptance Scenarios**:

1. **Given** 日次80 req消費済み、**When** 追加抽出要求、**Then** 外部requestなしでqueue退避、翌日catch-up
2. **Given** Claude subscription credentialが端末に存在、**When** 生成経路、**Then** 利用しない（Codex sidecarはcertification合格 + explicit opt-inのみ）
3. **Given** generation provider停止、**When** compact/crash recovery、**Then** canonical observed stateのみでresume成立

### Edge Cases

- v6.1 §8（重複x10 / 並行 / late event）、§6（fork/rename/shallow/worktree/no-remote/monorepo/WSL identity）、§22.11（tombstone/復活）
- v6.2: duplicate event no-op、task-boundary proposal/confirmation、unknown pending operation、incomplete reconciliation、initial claim race、engagement threshold/contradiction、mode × capability、selection ambiguity、safe renderer/capture、sensitivity、atomic acceptance、temporal memory history

## Requirements *(mandatory)*

### Functional Requirements

- **FR-0A**: Evidence Freeze / Base Bake-off — v6.1 Phase 0A + §4.3。
- **FR-0B**: Adapter / Sidecar Contract Harness — version-pinned matrix、capability schema、sidecar hostile harness。未観測cellは`unknown`。
- **FR-1**: Safety Boundary / Sole Writer — daemon single writer、thin RPC、atomic spool、peer auth、ownership manifest、backup。
- **FR-2**: Canonical Identity / Event State Machine — opaque UUID、adapterDeliveryId、turn state、late correction、session liveness。
- **FR-3P**: Phase 3 Preflight — v6.2 addendum §2〜§14。exact capability disposition、typed task state、event no-op、PendingOperation、task boundary、fail-closed reconciliation、initial claim、engagement/atomic acceptance、selection wire、sensitivity、capsule lifecycle、memory history、single quality-report schema、doctorをfreezeする。
- **FR-3**: Continuity State Machine — task lineage/binding、CanonicalWorkStateV1、checkpoint history、claim/fence/lease/engagement、crash recovery、capability-driven resume、memory_resume。
- **FR-4**: Claude/Codex Vertical Routes — 4 directed routes、exact evidence profile公開。
- **FR-5**: Retrieval / Injection / MCP — dual FTS、fixed RRF、CJK routing、safe capsule、self-ingestion prevention、local stdio MCP、user-authority CAS、evidence-preserving dedupe。
- **FR-6**: Generation Roles / Free Candidate — job runner、run ledger/cache、hard budget、claude-mem importer。
- **FR-7**: Optional Embeddings — item-addressable contract、per-item ledger、sqlite-vec pin、atomic switch、FTS fallback。
- **FR-8**: Core 1.0 Gates / Release — deterministic gates、resume non-inferiority、backup/restore、install matrix、72h soak、signed artifacts。

### Key Entities

NormalizedEvent / Session / SessionTaskBinding / CanonicalWorkStateV1 / PendingOperation /
ContinuationCheckpointV2 / CheckpointDispositionEvent+Projection / CheckpointDeliveryAttempt /
WorkspaceReconciliationReport / ResumeThresholdProfileV1 / ResumeSelectionDecisionV1 /
ResumeCapsuleV1 / DurableMemory revision history / CapabilityEvidence / EmbeddingRequest /
SyncOperation（Core 1.0では未実装）。

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-0A**: base ADR、inventory/classification/delta、clean install、unsafe path plan。
- **SC-0B**: exact capability matrix、sidecar certification。source/READMEだけでTierを上げない。
- **SC-1**: daemon-only write handle、fault injection、no Agent blockage、backup restore smoke。
- **SC-2**: identity collision、duplicate x10、parallel/late-event property tests。
- **SC-3P**: `ContractPreflightState=complete`（addendum §13 と同一 predicate: manifest exact-set 一致 — missing / extra / duplicate / manifest hash mismatch / artifact 欠落は全て blocking — + matrix 再生成 + runtime-neutral fixture green）。全required capability scenarioが`not_run`以外のdispositionを持ち、typed schema/hash、duplicate-event no-op、task-boundary proposal/confirmation、pending-operation、fail-closed reconciliation、initial-claim race、engagement threshold/contradiction、atomic acceptance、mode × capability、selection ambiguity、sensitivity、renderer/capture negative test、memory-history、quality-report reproducibility、doctor fixtureがpassする。未証明capabilityは適切にdowngradeする。
- **SC-3**: providerなしでsame-agent continuation成功。duplicate full injection、wrong scope、incompatible auto-resume、unsafe unknown replay、early acceptance、accepted-attempt/open-checkpoint、stale fence、capsule escape、malformed capsule trust、source evidence deletion、stale derived artifact use（直接依存・推移依存の両方）が全て0。
- **SC-4**: 4/4 route pass、destination Agentの証明済みstrategyのみ使用。
- **SC-5**: retrieval + echo-loop + temporal/source preservation + 100k FTS-only performance pass。
- **SC-6**: provider swap、free-certified profile ≥1、generation停止時continuation pass。
- **SC-7**: embedding switch/fallback/gate pass。
- **SC-8**: Core 1.0 release。主要resume scenarioが#8のfrozen claude-mem non-inferiority gateをpassし、未達はreviewed exception ADRなしにrelease不可。

### Normative Behavioral Report

`benchmarks/behavioral/contract.schema.json`の`ResumeQualityReportV1`を単一machine-readable authorityとする。

必須項目:

- wrongResumeRate
- unnecessaryHintRate
- candidateSelectionAccuracy
- criticalStateRecall
- fabricatedStateRate
- staleFieldRate
- reExplanationTurns / Tokens
- firstUsefulActionMs
- taskCompletionSuccessRate
- hintTokens / fullCapsuleTokens
- claudeMemBaselineDelta

Behavioral metricは`number | "unsupported"`。ただし該当Phase/Releaseが要求するmetricで`unsupported`はgate failure。zero-tolerance counterは常にnumber。

## Assumptions

- Phase順序はv6.1 §30を基礎とするが、FR-3P / SC-3PをPhase 3 product実装前のblocking barrierとして挿入する。
- Contract preflightの`complete`はaddendum §13と同一predicate: required scenario manifestとのexact-set一致（missing / extra / duplicate / manifest hash mismatch は fail）+ 全scenario disposition済み + evidence artifact存在 + matrix再生成 + runtime-neutral contract green。capabilityがunsupported/unknown_after_testでもgeneric/manual implementationは可能だが、automatic strategy/Tierはproven時のみ。
- 大規模なPhase 2以降のTS product codeは**#1 Stage 0の完了まで**増やさない（Stage 0完了 = ADR-003 + 凍結した4 contract + cutover gate定義。ADR-003「帰結」、tasks.md「Runtime barrier」）。schema/fixture/harnessはruntime-neutralなので並行可能。
- Stage 1が決めるのはroadmapを進めてよいかと候補scopeの評価までである。実際のCore 1.0のcutover scopeと時期は、Cutover gate 1–10と#84の完了後に確定する。**Core 1.0の現行runtime authorityはTypeScript/Nodeのまま**（v6.1 / ADR-001）で、Rustを標準実行基盤とする戦略目標自体はADR-005で決着済み。
- Phase 9〜11は後続feature。
- public sourceはCore 1.0 releaseを意味せず、Phase 8までtag/package/release禁止。
- `resume_mode=off`はcompactを含む全automatic contextを無効化する。
- Cline型shadow GitはCore 1.0必須にしない。workspace restoreは別ADR/security/evidence後に追加可能。
