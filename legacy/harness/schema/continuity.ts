import type { EventKind } from "./capability.ts";

/** 継続契約のスキーマ版。 */
export const CONTINUITY_SCHEMA_VERSION: 1 = 1;

/** §10 の共有上限。 */
export const CONTINUITY_LIMITS = {
  hintTokens: 120,
  fullCapsuleTokens: 700,
  promptMemoryTokens: 700,
  combinedTokens: 1500,
  absoluteTokens: 1800,
  capsulePayloadBytes: 32768,
  wrapperBytes: 36864,
  jsonDepth: 12,
  stringUtf8Bytes: 8192,
  arrayItems: 256,
  objectKeys: 128,
  rankedCandidates: 5,
} as const;

export type TaskBindingRole = "primary" | "side" | "subagent";

export const TASK_BINDING_ROLES = ["primary", "side", "subagent"] as const satisfies readonly TaskBindingRole[];

export type BoundaryEvidenceKind =
  | "explicit_user"
  | "native_fork"
  | "accepted_resume"
  | "agent_proposal"
  | "deterministic_shift";

export const BOUNDARY_EVIDENCE_KINDS = [
  "explicit_user",
  "native_fork",
  "accepted_resume",
  "agent_proposal",
  "deterministic_shift",
] as const satisfies readonly BoundaryEvidenceKind[];

export type TaskBoundaryProposalState = "proposed" | "confirmed" | "rejected";

export const TASK_BOUNDARY_PROPOSAL_STATES = [
  "proposed",
  "confirmed",
  "rejected",
] as const satisfies readonly TaskBoundaryProposalState[];

export interface BoundaryEvidence {
  kind: BoundaryEvidenceKind;
  sourceEventIds: string[];
  proposedAt: string;
  confidence?: number;
}

export interface SessionTaskBinding {
  sessionId: string;
  taskLineageId: string;
  role: TaskBindingRole;
  boundAt: string;
  unboundAt?: string;
  boundaryEvidence: BoundaryEvidence;
  revision: string;
}

export interface TaskBoundaryProposalV1 {
  proposalId: string;
  sessionId: string;
  currentTaskLineageId: string;
  proposedTaskLineageId: string;
  proposedRole: TaskBindingRole;
  evidence: BoundaryEvidence;
  state: TaskBoundaryProposalState;
  revision: string;
}

export type TaskBoundaryDecisionSource = "user" | "native_runtime";

export const TASK_BOUNDARY_DECISION_SOURCES = [
  "user",
  "native_runtime",
] as const satisfies readonly TaskBoundaryDecisionSource[];

export type TaskBoundaryDecisionV1 =
  | {
      kind: "confirm";
      proposalId: string;
      expectedProposalRevision: string;
      expectedBindingRevision: string;
      source: TaskBoundaryDecisionSource;
      sourceEventIds: string[];
    }
  | {
      kind: "reject";
      proposalId: string;
      expectedProposalRevision: string;
      expectedBindingRevision: string;
      source: TaskBoundaryDecisionSource;
      sourceEventIds: string[];
    };

export interface TaskBoundaryAuthorityContextV1 {
  // §2.2 caller の source 値ではなく、解決済み event から権限を検証する
  sourceEvents: NormalizedContinuityEvent[];
  agent: string;
  exactAgentVersion: string;
  capabilityHash: string;
  provenScenarioIds: string[];
  userSurfaceAuthority?: {
    surface: "cli" | "viewer" | "mcp_user_authority";
    grantedAt: string;
  };
}

export type JsonPrimitive = string | number | boolean | null;
export type JsonValue = JsonPrimitive | JsonValue[] | { [key: string]: JsonValue };
export type EvidenceKind = "native" | "synthesized" | "derived";

export const EVIDENCE_KINDS = ["native", "synthesized", "derived"] as const satisfies readonly EvidenceKind[];

export type Freshness = "current" | "stale" | "unknown";

export const FRESHNESS_VALUES = ["current", "stale", "unknown"] as const satisfies readonly Freshness[];

export type Sensitivity = "normal" | "private" | "secret";

export const SENSITIVITIES = ["normal", "private", "secret"] as const satisfies readonly Sensitivity[];

export interface Observed<T extends JsonValue> {
  value: T;
  sourceEventIds: string[];
  ingestSeq: string;
  observedAt: string;
  evidenceKind: EvidenceKind;
  confidence: number; // 0..1
  freshness: Freshness;
  truncated: boolean;
  sensitivity: Sensitivity;
}

export type ContinuityCaptureMethod =
  | "native_event"
  | "hook"
  | "plugin"
  | "transcript_scan"
  | "user_surface";

export const CONTINUITY_CAPTURE_METHODS = [
  "native_event",
  "hook",
  "plugin",
  "transcript_scan",
  "user_surface",
] as const satisfies readonly ContinuityCaptureMethod[];

export interface ContinuityIngestAttestationV1 {
  ingestReceiptId: string;
  peerIdentityId: string;
  channel: "rpc" | "spool";
  attestedAt: string;
}

export interface ContinuityEventProvenanceV1 {
  sourceAgentVersion: string;
  // §3.1 daemon intake 層が割り当て、caller の値は信頼しない
  evidenceKind: EvidenceKind;
  captureMethod: ContinuityCaptureMethod;
  capabilityHash?: string;
  scenarioId?: string;
  // §3.1 daemon intake 層が割り当て、caller の値は信頼しない
  ingestAttestation?: ContinuityIngestAttestationV1;
}

export type TurnIdSource = "native" | "synthesized_monotonic" | "unavailable";

export const TURN_ID_SOURCES = [
  "native",
  "synthesized_monotonic",
  "unavailable",
] as const satisfies readonly TurnIdSource[];

export type ContinuityOperationPhase = "start" | "progress" | "terminal";

export const CONTINUITY_OPERATION_PHASES = [
  "start",
  "progress",
  "terminal",
] as const satisfies readonly ContinuityOperationPhase[];

/**
 * §3.1「`operation` は kind が operation の start / progress / terminal である event に必須」を
 * 判定するための正本（#29）。addendum は operation 系 kind を列挙していないため、harness の
 * 正規化 event 語彙（`EventKind`）に対する分類をここで固定する。`NormalizedContinuityEvent.kind`
 * は adapter 固有の値も取り得る開いた文字列であり、この表にも `NON_OPERATION_EVENT_KINDS` にも
 * 無い kind は未分類として envelope を要求しない。
 */
export const OPERATION_EVENT_PHASES = {
  tool_started: "start",
  tool_completed: "terminal",
  tool_failed: "terminal",
} as const satisfies Partial<Record<EventKind, ContinuityOperationPhase>>;

/** operation 系でないと確定している kind。`OPERATION_EVENT_PHASES` との和が `EVENT_KINDS` に一致する。 */
export const NON_OPERATION_EVENT_KINDS = [
  "session_started",
  "user_prompted",
  "assistant_completed",
  "turn_completed",
  "pre_compact",
  "post_compact",
  "session_idle",
  "session_interrupted",
  "session_ended",
] as const satisfies readonly EventKind[];

export interface ContinuityOperationRefV1 {
  phase: ContinuityOperationPhase;
  // §4.3 terminal correlation の正本となる schema-versioned hash
  operationMatchKey: string;
  operationKind: string;
  nativeOperationId?: string;
  canonicalInputHash?: string;
}

export interface NormalizedContinuityEvent {
  eventId: string;
  adapterDeliveryId?: string;
  canonicalFingerprint: string;
  kind: string;
  ingestSeq: string;
  occurredAt: string;
  sessionId: string;
  taskLineageId?: string;
  turnId?: string;
  turnIdSource: TurnIdSource;
  sourceAgent: string;
  provenance: ContinuityEventProvenanceV1;
  // §3.1 operation event の correlation 値は payload ではなくこの envelope を使う
  operation?: ContinuityOperationRefV1;
  payload: JsonValue;
  successful?: boolean;
}

export interface ObservedFile {
  path: string;
  role: "active" | "modified" | "read" | "test" | "config" | "unknown";
  contentHash?: string;
  existsAtObservation: boolean;
  sourceEventIds: string[];
  observedAt: string;
  freshness: Freshness;
  sensitivity: Sensitivity;
}

export interface ObservedCommand {
  operationId: string;
  commandDisplay: string;
  cwd?: string;
  exitCode?: number;
  status: "succeeded" | "failed" | "unknown";
  sourceEventIds: string[];
  observedAt: string;
  evidenceKind: EvidenceKind;
  sensitivity: Sensitivity;
}

export interface ObservedTest {
  operationId: string;
  commandDisplay?: string;
  target?: string;
  status: "passed" | "failed" | "partial" | "unknown";
  summary?: string;
  sourceEventIds: string[];
  observedAt: string;
  evidenceKind: EvidenceKind;
  sensitivity: Sensitivity;
}

export interface OperationCorrelationV1 {
  operationId: string;
  startEventId: string;
  nativeOperationId?: string;
  operationMatchKey: string;
  sessionId: string;
  taskLineageId: string;
  turnId?: string;
  toolName?: string;
  canonicalInputHash?: string;
}

export type ReplayPolicy = "never_auto" | "verify_first" | "safe_idempotent";

export const REPLAY_POLICIES = [
  "never_auto",
  "verify_first",
  "safe_idempotent",
] as const satisfies readonly ReplayPolicy[];

export interface PendingOperation {
  operationId: string;
  correlation: OperationCorrelationV1;
  kind:
    | "command"
    | "file_mutation"
    | "test"
    | "tool"
    | "migration"
    | "external_side_effect"
    | "other";
  description: string;
  status: "started" | "succeeded" | "failed" | "unknown";
  // §4.3 unknown side effect の自動再実行可否
  replayPolicy: ReplayPolicy;
  sourceEventIds: string[];
  startedAt: string;
  terminalAt?: string;
  idempotencyKey?: string;
  verificationHint?: string;
  sensitivity: Sensitivity;
  // §4.3 の権威順序・turn 両立を状態だけで判定するための start 側の材料（#35）
  startIngestSeq?: string;
  startTurnIdSource?: TurnIdSource;
  // §4.3 の衝突検査に使う、この operation を閉じた terminal の指紋（#44）
  terminalFingerprint?: string;
}

export interface DroppedEvidenceEntryV1 {
  reason: "evicted" | "orphaned_terminal";
  recordedAt: string;
  sensitivity: Sensitivity;
  eventId?: string;
  operationId?: string;
  status?: "started" | "succeeded" | "failed" | "unknown";
  // 再送で記録が増えないようにするための鍵（#39）。監査用の識別子は eventId のほう。
  // 第一 authority は adapterDeliveryId で、指紋はそれを名乗らない記録の fallback（§8.2 と同じ順）
  terminalFingerprint?: string;
  adapterDeliveryId?: string;
}

export interface RepositoryStateSnapshot {
  repositoryId: string;
  workspaceId: string;
  branchKey?: string;
  worktreeId?: string;
  headSha?: string;
  upstreamSha?: string;
  dirtyTreeFingerprint?: string;
  gitStatusSummary?: string;
  capturedAt: string;
}

export interface SemanticResumeNoteV1 {
  schemaVersion: 1;
  goal?: string;
  completed: string[];
  currentState?: string;
  nextActions: string[];
  blockers: string[];
  unresolvedQuestions: string[];
  providerId: string;
  modelId: string;
  promptHash: string;
  generatedFromIngestSeq: string;
  confidence: number;
  sourceEventIds: string[];
  sensitivity: Sensitivity;
}

export interface CanonicalWorkStateV1 {
  schemaVersion: 1;
  taskLineageId: string;
  projectId: string;
  workspaceId: string;
  branchKey?: string;
  sourceAgent: string;
  latestSubstantivePrompt?: Observed<string>;
  lastAssistantConclusion?: Observed<string>;
  nativeTodoState?: Observed<JsonValue>;
  activeFiles: ObservedFile[];
  modifiedFiles: ObservedFile[];
  recentCommands: ObservedCommand[];
  recentTests: ObservedTest[];
  pendingOperations: PendingOperation[];
  // 退避した operation と相手の見つからなかった terminal の記録（#43 / #39）
  droppedEvidence?: DroppedEvidenceEntryV1[];
  repositoryState: RepositoryStateSnapshot;
  semanticResumeNote?: SemanticResumeNoteV1;
  sensitivity: Sensitivity;
  lastIngestSeq: string;
  stateRevision: string;
  updatedAt: string;
}

export interface ContinuationCheckpointV2 {
  id: string;
  schemaVersion: 2;
  checkpointRevision: string;
  projectId: string;
  workspaceId: string;
  branchKey?: string;
  taskLineageId: string;
  sourceSessionId: string;
  sourceAgent: string;
  kind: "pre_compact" | "session_end" | "idle" | "manual" | "crash_recovery";
  parentCheckpointId?: string;
  workStateRevision: string;
  canonicalState: CanonicalWorkStateV1;
  memoryWatermark: string;
  contentHash: string;
  sensitivity: Sensitivity;
  createdAt: string;
  expiresAt?: string;
}

export type CheckpointDispositionKind =
  | "created"
  | "accepted"
  | "superseded"
  | "expired"
  | "reopened"
  | "retracted";

export const CHECKPOINT_DISPOSITION_KINDS = [
  "created",
  "accepted",
  "superseded",
  "expired",
  "reopened",
  "retracted",
] as const satisfies readonly CheckpointDispositionKind[];

export interface CheckpointDispositionEvent {
  eventId: string;
  checkpointId: string;
  kind: CheckpointDispositionKind;
  expectedProjectionRevision: string;
  resultingProjectionRevision: string;
  relatedCheckpointId?: string;
  relatedDeliveryAttemptId?: string;
  reasonCode: string;
  source: "daemon" | "runtime" | "user";
  createdAt: string;
}

export interface CheckpointDispositionProjection {
  checkpointId: string;
  state: "open" | "accepted" | "superseded" | "expired" | "retracted";
  projectionRevision: string;
  latestEventId: string;
  // §6.1 post-claim command が projection CAS で比較する attempt
  activeDeliveryAttemptId?: string;
  // §6.1 post-claim command が projection CAS で比較する fence
  activeClaimFence?: string;
  // §6.1 attempt と同一 transaction で更新する lease
  activeLeaseUntil?: string;
}

export interface CheckpointMetadataV1 {
  checkpointId: string;
  checkpointRevision: string;
  taskLineageId: string;
  kind: ContinuationCheckpointV2["kind"];
  sourceSessionId: string;
}

export interface DispositionAuthorityContextV1 {
  source: "daemon" | "runtime" | "user";
  userAuthorizedCrossLineage: boolean;
  sourceEventIds: string[];
}

export type DeliveryAttemptState =
  | "claimed"
  | "delivered"
  | "engaged"
  | "accepted"
  | "dismissed"
  | "abandoned";

export const DELIVERY_ATTEMPT_STATES = [
  "claimed",
  "delivered",
  "engaged",
  "accepted",
  "dismissed",
  "abandoned",
] as const satisfies readonly DeliveryAttemptState[];

export type EngagementEvidenceKind =
  | "explicit_accept"
  | "explicit_continue_prompt"
  | "related_file_action"
  | "related_command"
  | "related_test"
  | "related_todo_progress"
  | "manual_resume_tool";

export const ENGAGEMENT_EVIDENCE_KINDS = [
  "explicit_accept",
  "explicit_continue_prompt",
  "related_file_action",
  "related_command",
  "related_test",
  "related_todo_progress",
  "manual_resume_tool",
] as const satisfies readonly EngagementEvidenceKind[];

export interface CheckpointAnchorV1 {
  anchorId: string;
  checkpointId: string;
  checkpointRevision: string;
  taskLineageId: string;
  kind: "file" | "symbol" | "command" | "test" | "todo" | "task_lineage";
  valueHash: string;
  sourceEventIds: string[];
}

export interface EngagementEvidence {
  kind: EngagementEvidenceKind;
  sourceEventIds: string[];
  score: number;
  checkpointAnchorIds: string[];
  successful: boolean;
  observedAt: string;
}

export interface ContradictionEvidenceV1 {
  contradictionId: string;
  kind: "explicit_rejection" | "new_task_confirmed" | "workspace_incompatible" | "runtime_invalidated";
  sourceEventIds: string[];
  observedAt: string;
}

export interface ContradictionScanRangeV1 {
  fromIngestSeq: string;
  toIngestSeq: string;
  scannedAt: string;
}

export interface EngagementEvaluationContextV1 {
  sourceEvents: NormalizedContinuityEvent[];
  checkpointAnchors: CheckpointAnchorV1[];
  // §6.4 caller 値は診断用で、acceptance は daemon event store を再検索する
  contradictions: ContradictionEvidenceV1[];
  // §6.4 caller の範囲は authoritative scan を制限しない
  contradictionScan: ContradictionScanRangeV1;
  destinationTurnId: string;
  destinationTurnIdSource: TurnIdSource;
  destinationAgentVersion: string;
  destinationCapabilityHash: string;
  turnIdentityDisposition: CapabilityTestDisposition;
  evaluationStartedAt: string;
  evaluationEndedAt: string;
}

export interface CheckpointDeliveryAttempt {
  attemptId: string;
  checkpointId: string;
  checkpointRevision: string;
  destinationSessionId: string;
  destinationAgent: string;
  state: DeliveryAttemptState;
  // §6.1 claim CAS で比較する fence
  claimFence: string;
  leaseUntil: string;
  heartbeatUntil: string;
  injectedContentHash?: string;
  injectionId?: string;
  engagementEvidence: EngagementEvidence[];
  createdAt: string;
  deliveredAt?: string;
  engagedAt?: string;
  acceptedAt?: string;
  dismissedAt?: string;
  abandonedAt?: string;
  revision: string;
}

export type DeliveryCommandV1 =
  | { kind: "mark_delivered"; attemptId: string; revision: string; fence: string; sessionId: string; contentHash: string }
  | { kind: "record_engagement"; attemptId: string; revision: string; fence: string; sessionId: string; evidence: EngagementEvidence }
  | { kind: "accept"; attemptId: string; revision: string; fence: string; sessionId: string; projectionRevision: string }
  | { kind: "dismiss"; attemptId: string; revision: string; fence: string; sessionId: string }
  | { kind: "abandon"; attemptId: string; revision: string; fence: string; sessionId: string; reason: string }
  | { kind: "renew_lease"; attemptId: string; revision: string; fence: string; sessionId: string; requestedLeaseUntil: string };

export interface ResumeSuppressionEntryV1 {
  checkpointId: string;
  sessionId: string;
  reason: "dismissed";
  attemptId: string;
  createdAt: string;
}

export type ReconciliationStatus =
  | "exact"
  | "fast_forward_compatible"
  | "stale_but_usable"
  | "requires_verification"
  | "incompatible";

export const RECONCILIATION_STATUSES = [
  "exact",
  "fast_forward_compatible",
  "stale_but_usable",
  "requires_verification",
  "incompatible",
] as const satisfies readonly ReconciliationStatus[];

export type ResumeDeliveryStrategy =
  | "native_prompt_gate"
  | "session_start_full"
  | "next_prompt_synthesized"
  | "manual_only";

export const RESUME_DELIVERY_STRATEGIES = [
  "native_prompt_gate",
  "session_start_full",
  "next_prompt_synthesized",
  "manual_only",
] as const satisfies readonly ResumeDeliveryStrategy[];

export type ResumeMode = "smart" | "always" | "hint_only" | "compact_only" | "off";

export const RESUME_MODES = ["smart", "always", "hint_only", "compact_only", "off"] as const satisfies readonly ResumeMode[];

export type ResumeDeliveryBoundary = "session_start" | "first_user_prompt" | "post_compact" | "manual";

export const RESUME_DELIVERY_BOUNDARIES = [
  "session_start",
  "first_user_prompt",
  "post_compact",
  "manual",
] as const satisfies readonly ResumeDeliveryBoundary[];

export interface ResumeCapsuleV1 {
  schemaVersion: 1;
  injectionId: string;
  checkpointId: string;
  checkpointRevision: string;
  taskLineageId: string;
  sourceAgent: string;
  ageSeconds: number;
  reconciliation: ReconciliationStatus;
  workState: CanonicalWorkStateV1;
  selectedMemoryIds: string[];
  warnings: string[];
}

export interface ResumeThresholdProfileV1 {
  profileId: string;
  datasetVersion: string;
  fullResumeMinScore: number;
  hintMinScore: number;
  ambiguityMargin: number;
  maxCandidates: number;
}

export type ResumeDecisionAction =
  | "none"
  | "hint"
  | "candidate_list"
  | "verification_capsule"
  | "full_capsule"
  | "manual_only";

export const RESUME_DECISION_ACTIONS = [
  "none",
  "hint",
  "candidate_list",
  "verification_capsule",
  "full_capsule",
  "manual_only",
] as const satisfies readonly ResumeDecisionAction[];

export interface RankedResumeCandidateV1 {
  rank: number;
  checkpointId: string;
  checkpointRevision: string;
  taskLineageId: string;
  score: number;
  reconciliationStatus: ReconciliationStatus;
  reasonCodes: string[];
  ageSeconds: number;
}

export interface ResumeSelectionDecisionV1 {
  schemaVersion: 1;
  boundary: ResumeDeliveryBoundary;
  datasetVersion: string;
  thresholdProfileId: string;
  capabilityHash: string;
  strategy: ResumeDeliveryStrategy;
  mode: ResumeMode;
  action: ResumeDecisionAction;
  selectedCheckpointId?: string;
  rankedCandidates: RankedResumeCandidateV1[];
  topScore?: number;
  topMargin?: number;
  confidenceBand: "high" | "medium" | "low" | "none";
  decisionReasonCodes: string[];
  fallbackReasonCode?: string;
  reconciliationReportHash?: string;
}

export type DerivedArtifactKind =
  | "summary"
  | "semantic_resume_note"
  | "checkpoint_semantic_note"
  | "consolidated_memory"
  | "embedding_item"
  | "context_pack_cache"
  | "cloud_projection";

export const DERIVED_ARTIFACT_KINDS = [
  "summary",
  "semantic_resume_note",
  "checkpoint_semantic_note",
  "consolidated_memory",
  "embedding_item",
  "context_pack_cache",
  "cloud_projection",
] as const satisfies readonly DerivedArtifactKind[];

export type DerivedArtifactStatus = "active" | "stale" | "invalidated" | "rebuilding";

export const DERIVED_ARTIFACT_STATUSES = [
  "active",
  "stale",
  "invalidated",
  "rebuilding",
] as const satisfies readonly DerivedArtifactStatus[];

export type DerivedArtifactSourceRefV1 =
  | {
      kind: "memory";
      memoryId: string;
      memoryRevision: string;
      contentHash: string;
    }
  | {
      kind: "artifact";
      artifactId: string;
      artifactKind: DerivedArtifactKind;
      artifactRevision: string;
      contentHash: string;
    };

export interface DerivedArtifactDependencyV1 {
  artifactId: string;
  artifactKind: DerivedArtifactKind;
  artifactRevision: string;
  sources: DerivedArtifactSourceRefV1[];
  // §12.3 write 時に materialize し、read 時の traversal では導出しない
  baseMemoryClosure: Array<{ memoryId: string; memoryRevision: string; contentHash: string }>;
  sourceEventIds: string[];
  generationId?: string;
}

export interface DerivedArtifactInvalidationEventV1 {
  eventId: string;
  artifactId: string;
  artifactKind: DerivedArtifactKind;
  expectedArtifactRevision: string;
  sourceMemoryId: string;
  invalidatingMemoryRevision: string;
  viaArtifactId?: string;
  hopDepth: number;
  reason:
    | "memory_updated"
    | "memory_superseded"
    | "memory_retracted"
    | "memory_invalidated"
    | "source_artifact_invalidated";
  resultingStatus: "stale" | "invalidated";
  idempotencyKey: string;
  createdAt: string;
}

export type CapabilityTestDisposition = "not_run" | "proven" | "unsupported" | "unknown_after_test";

export const CAPABILITY_TEST_DISPOSITIONS = [
  "not_run",
  "proven",
  "unsupported",
  "unknown_after_test",
] as const satisfies readonly CapabilityTestDisposition[];

export type ContractPreflightState = "incomplete" | "complete";

export const CONTRACT_PREFLIGHT_STATES = ["incomplete", "complete"] as const satisfies readonly ContractPreflightState[];

export interface RequiredCapabilityScenarioV1 {
  scenarioId: string;
  title: string;
  appliesToAgents: string[];
  requiredFor: Array<"generic_phase3" | "automatic_strategy" | "tier_a">;
}

export interface CapabilityScenarioManifestV1 {
  manifestVersion: string;
  // §13 exact-set 照合に用いる manifest hash
  manifestHash: string;
  scenarios: RequiredCapabilityScenarioV1[];
}
