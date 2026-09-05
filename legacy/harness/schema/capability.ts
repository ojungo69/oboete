export type EventKind =
  | "session_started"
  | "user_prompted"
  | "assistant_completed"
  | "tool_started"
  | "tool_completed"
  | "tool_failed"
  | "turn_completed"
  | "pre_compact"
  | "post_compact"
  | "session_idle"
  | "session_interrupted"
  | "session_ended";

/** 実測が付いた cell が取り得る値。`Capability` はこれに未観測（unknown）を足したもの。 */
export type ObservedCapability = "native" | "synthesized" | "unsupported";

export type Capability = ObservedCapability | "unknown";

export interface CapabilityEvidence {
  value: Capability;
  // 未観測 cell は evidenceKind / verifiedAt を持たない（観測していないものに
  // 証跡種別と検証時刻を書くと provenance の捏造になる）。§7.2 の型は必須だが、
  // 本 harness では value==="unknown" のとき null を明示する。
  coverage?: number; // 既知の欠落があるcapabilityの被覆率
  sourceEvents: string[]; // synthesized時の根拠native event
  nativeVersion: string; // 検証したexact CLI version
  sourceCommit?: string;
  evidenceKind: "official-doc" | "source-test" | "real-cli-e2e" | null;
  verifiedAt: string | null;
  limitations: string[];
  // どの capture fixture が根拠か。cell 間で「同一の実測に基づくか」を比較するために持つ
  // （limitations の自由文から fixture 名を読み取るのは照合として弱い）
  sourceFixtureId?: string;
  // 申告値を**導いた**観測記録。matrix 直下 `evidenceSources` への添字。
  // 単数の digest 欄にしない: 1 つの fixture が複数の run を束ねるため
  // （claude/interrupt-and-hook-timeout は 5 本の記録を根拠にしている）。
  // 含意は片方向だけ: real-cli-e2e の cell は必ず空でない配列を持つ（`isProven` が要求する）。
  // 逆は成り立たない —— source-test の cell も、値を導いた legacy 記録（manifest 無し）を
  // ここに載せる。したがって「refs がある」は裏付けの証明にならないので、
  // 種別を見ずにこの欄で判断しない。
  evidenceRefs?: number[];
}

/**
 * fixture が名指しする観測記録。`harness/fixtures/<cli>/raw/` からの相対 path で指す。
 *
 * digest を 2 つ持つのは役割が違うため（data-model.md §2.1a）:
 * - `evidenceHash` は正規化抜粋の SHA-256 で、同じ scenario を取り直しても変わらない
 * - `captureRawHash` は生 byte の SHA-256 で、「この記録そのものか」を結び付ける
 *   （正規化が伏せる差だけを変えた記録を通さない）
 */
export interface EvidenceRef {
  path: string;
  evidenceHash: string;
  captureRawHash: string;
  normalizationVersion: number;
  /**
   * 同じ run の manifest。**無い ref は real-cli-e2e の根拠にならない**（legacy 証拠）。
   * digest の照合は manifest の有無に関わらず必ず行う。
   */
  manifest?: string;
  /** manifest ファイルの生 byte の SHA-256。`manifest` があるとき必須 */
  manifestHash?: string;
}

/**
 * rig が run ごとに書く素性の記録。fixture の自己申告ではないことが real-cli-e2e の根拠になる。
 * schema は harness/schema/evidence-manifest.schema.json（1 対 1）。
 */
export interface RunManifest {
  manifestVersion: number;
  cli: "claude" | "codex";
  /** `.version` を単一行として読み、末尾の CRLF か LF を 1 つだけ取り除いた値 */
  cliVersion: string;
  scenarioId: string;
  /** 観測記録の 1 行目の at。captureRawHash が縛るので fixture の申告と照合できる */
  capturedAt: string;
  isolated: boolean;
  internalRunMarker: boolean;
  exitStatus: number;
  recorderErrors: number;
  capture: string;
  captureRawHash: string;
  captureHash: string;
  normalizationVersion: number;
}

/** matrix 直下に置く証拠の表。cell からは添字で参照する（fixtureId → path の昇順） */
export interface EvidenceSource {
  fixtureId: string;
  /** 置き場からの相対 path。絶対 path は入らない */
  path: string;
  evidenceHash: string;
  normalizationVersion: number;
  /** legacy 証拠（manifest を持たない ref）は null */
  manifestHash: string | null;
  cliVersion: string;
  /** 制約付き識別子。自由文の scenario は成果物へ出さない */
  scenarioId: string;
}

/**
 * 自動配送の経路。`manual_only` は「自動配送しない」であって「resume できない」ではない。
 *
 * - native_prompt_gate: CLI 自身が prompt を model に渡す前に context を差し込める
 * - next_prompt_synthesized: hook 合成で次 prompt 前配送を実現できる（両 cell が同一実測で証明済み）
 * - session_start_full: SessionStart でのみ full 配送できる
 * - manual_only: 自動配送の証明が無い（既定値）
 */
export type ResumeDeliveryStrategy =
  | "native_prompt_gate"
  | "next_prompt_synthesized"
  | "session_start_full"
  | "manual_only";

export type ToolFailurePhase =
  | "executed"
  | "permission_denied"
  | "schema_invalid"
  | "unknown_tool"
  | "interrupt"
  | "unknown";

export type CompactionRecoveryStrategy =
  | "native_pre_and_post"
  | "native_pre_next_prompt"
  | "session_compaction_event"
  | "turn_checkpoint_detect_reset"
  | "unsupported";

export interface AdapterCapabilities {
  capture: Record<EventKind, CapabilityEvidence>;
  // 観測できた phase と、観測を試みていない phase を区別する（前者だけを並べると
  // 「サポートしていない」と読めてしまうため）
  toolFailurePhases: ToolFailurePhase[];
  toolFailurePhasesUntested: ToolFailurePhase[];
  sessionStartInjection: CapabilityEvidence;
  promptAwareInjection: CapabilityEvidence;
  // prompt が model に渡る前に context が可視だったか。hook が印字しただけでは足りない
  // ため promptAwareInjection とは別 cell にする（両方揃って初めて prompt 経路を名乗れる）
  promptDeliveryBeforeModel: CapabilityEvidence;
  // compact 後の full 配送がちょうど 1 回か（重複 hook の dedupe を含む）
  compactSingleDelivery: CapabilityEvidence;
  // §7.2 の union に "unknown" が無いため、未観測を表現できるよう null を許す
  // （"unsupported" と書くと未計測を否定的事実として断定してしまう）
  compactionRecoveryStrategy: CompactionRecoveryStrategy | null;
  trueSessionEnd: CapabilityEvidence;
  subagentCapture: CapabilityEvidence;
  stableNativeSessionId: CapabilityEvidence;
  // 上の cell 群から導出する。手で書き換えない
  resumeDeliveryStrategy: ResumeDeliveryStrategy;
  // capability hash の入力（exact version + 各 fixture の evidence hash）。
  // hash 値そのものは scenario manifest が揃ってから計算する（addendum §13）
  capabilityHashInputs: string[];
}

export interface CaptureFixture {
  fixtureId: string; // 例 "claude/lifecycle-basic"
  cli: "claude" | "codex";
  nativeVersion: string; // capture 時点の exact `--version` 出力
  capturedAt: string; // ISO 8601
  scenario: string; // 何を観測したか 1 行。**fixture 内の説明であって成果物へは出さない**
  scenarioId: string; // 成果物へ出る識別子。^[a-z0-9]+(?:[.-][a-z0-9]+)*$
  // capability 既定 "native"。Stop→turn_completed 等の合成は "synthesized" + sourceEvents（§7.2）。
  observedEvents: Array<{
    kind: EventKind | "raw";
    raw?: unknown;
    at?: string;
    capability?: "native" | "synthesized";
    sourceEvents?: string[];
    coverage?: number;
    limitations?: string[];
    // limitations と同じ長さで位置対応する closed enum。成果物へ出るのはこちら
    limitationCodes?: string[];
  }>;
  toolFailurePhasesObserved: ToolFailurePhase[];
  limitations: string[]; // 散文。**fixture 内の説明であって成果物へは出さない**
  limitationCodes: string[]; // limitations と同じ長さで位置対応する closed enum
  rig: { isolated: boolean; internalRunMarker: boolean }; // 隔離 rig 下で取ったか
  // 実 CLI 観測の根拠。組み立て側が各記録から digest を再計算して照合する。
  // official-doc / source-test 由来の fixture は持たない（required にしない理由）
  evidence?: EvidenceRef[];
  // 高位 cell の観測結果（観測できた fixture だけが書く。書かなければ unknown のまま）
  highLevel?: Partial<{
    sessionStartInjection: ObservedCapability;
    promptAwareInjection: ObservedCapability;
    promptDeliveryBeforeModel: ObservedCapability;
    compactSingleDelivery: ObservedCapability;
    compactionRecoveryStrategy: CompactionRecoveryStrategy;
    trueSessionEnd: ObservedCapability;
    subagentCapture: ObservedCapability;
    stableNativeSessionId: ObservedCapability;
  }>;
}

export const EVENT_KINDS: readonly EventKind[] = [
  "session_started",
  "user_prompted",
  "assistant_completed",
  "tool_started",
  "tool_completed",
  "tool_failed",
  "turn_completed",
  "pre_compact",
  "post_compact",
  "session_idle",
  "session_interrupted",
  "session_ended",
] as const;

export const TOOL_FAILURE_PHASES: readonly ToolFailurePhase[] = [
  "executed",
  "permission_denied",
  "schema_invalid",
  "unknown_tool",
  "interrupt",
  "unknown",
] as const;

export function unknownEvidence(nativeVersion: string): CapabilityEvidence {
  return {
    value: "unknown",
    sourceEvents: [],
    nativeVersion,
    evidenceKind: null,
    verifiedAt: null,
    limitations: ["not observed in Phase 0B"],
  };
}

export function emptyMatrix(nativeVersion: string): AdapterCapabilities {
  const capture = {} as Record<EventKind, CapabilityEvidence>;
  for (const kind of EVENT_KINDS) {
    capture[kind] = unknownEvidence(nativeVersion);
  }
  return {
    capture,
    toolFailurePhases: [],
    toolFailurePhasesUntested: [],
    sessionStartInjection: unknownEvidence(nativeVersion),
    promptAwareInjection: unknownEvidence(nativeVersion),
    promptDeliveryBeforeModel: unknownEvidence(nativeVersion),
    compactSingleDelivery: unknownEvidence(nativeVersion),
    compactionRecoveryStrategy: null,
    trueSessionEnd: unknownEvidence(nativeVersion),
    subagentCapture: unknownEvidence(nativeVersion),
    stableNativeSessionId: unknownEvidence(nativeVersion),
    resumeDeliveryStrategy: "manual_only",
    capabilityHashInputs: [],
  };
}

/** 実 CLI で観測され、肯定的な結論が出ている cell だけを「証明済み」とする。 */
function isProven(cell: CapabilityEvidence): boolean {
  return (
    (cell.value === "native" || cell.value === "synthesized") &&
    cell.evidenceKind === "real-cli-e2e" &&
    typeof cell.verifiedAt === "string" &&
    cell.verifiedAt.length > 0 &&
    // 種別と時刻は cell が自分で名乗る値なので、裏付けた記録の実在もここで要求する。
    // 組み立てを通った matrix なら real-cli-e2e は必ず ref を持つが、この関数は
    // 出来合いの matrix にも掛かる。名乗りだけで単独 cell の tier を通さない。
    // **ここで見えるのは「番号が付いているか」まで**。番号が `evidenceSources` の範囲に
    // あるか・その記録が本当に裏付けているかは、番号を振った組み立て側でしか照合できない
    // （この関数は cell しか受け取らない）。手書きの matrix を信用してよい根拠にはならない
    Array.isArray(cell.evidenceRefs) &&
    cell.evidenceRefs.length > 0
  );
}

/**
 * 2 つの cell が「同一の実測」に基づくか。exact version・fixture・**裏付けた観測記録**の
 * 3 つが揃って一致することを要求する。別々の run をつなぎ合わせて経路を主張させないための
 * ゲートなので、裏付けの無い cell は「照合できない」= 不合格とする。
 */
function sameEvidenceSource(a: CapabilityEvidence, b: CapabilityEvidence): boolean {
  if (a.nativeVersion !== b.nativeVersion) return false;
  if (!a.sourceFixtureId || a.sourceFixtureId !== b.sourceFixtureId) return false;
  // 同じ fixture であることは同一実測の証明にならない。1 つの fixture が複数の run を
  // 束ねられる以上、**両 cell を裏付けた記録に同じものが 1 件でもある**ことを要求する
  const refsA = a.evidenceRefs ?? [];
  const refsB = b.evidenceRefs ?? [];
  return refsA.some((r) => refsB.includes(r));
}

/**
 * 配送経路を cell から導出する。addendum §8 の tier 定義をそのまま実装する。
 * 証明が欠けたら必ず下位の経路へ落ちる（既定 manual_only）。
 *
 * §8 の 2 つの tier は要求が非対称なので、揃えずに書き分ける:
 * - `native_prompt_gate` = 「pre-model 配送が native かつ real-CLI 実測済み」。
 *   promptAwareInjection には条件が無い（合成が要らないため対で縛る意味が無い）。
 * - `next_prompt_synthesized` = 「pre-model 配送と prompt-aware injection の **両方** が
 *   synthesized で、かつ同一 exact version の fixture / evidence hash による実測」。
 *   同条の「half-proven な synthesized 対は無効」がこの縛りの根拠。
 *
 * したがって native と synthesized が割れた対はどちらの tier も満たさず、下位へ落ちる。
 */
export function resolveResumeDeliveryStrategy(caps: AdapterCapabilities): ResumeDeliveryStrategy {
  const prompt = caps.promptAwareInjection;
  const beforeModel = caps.promptDeliveryBeforeModel;

  if (isProven(beforeModel) && beforeModel.value === "native") return "native_prompt_gate";

  const synthesizedPair =
    isProven(beforeModel) &&
    beforeModel.value === "synthesized" &&
    isProven(prompt) &&
    prompt.value === "synthesized" &&
    sameEvidenceSource(prompt, beforeModel);
  if (synthesizedPair) return "next_prompt_synthesized";

  if (isProven(caps.sessionStartInjection)) return "session_start_full";
  return "manual_only";
}
