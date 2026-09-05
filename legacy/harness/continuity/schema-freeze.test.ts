import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { validateAgainstSchema, validateContractValue, type JsonSchemaDocument } from "../schema/validate.ts";
import * as contract from "../schema/continuity.ts";
import { readIJsonFile } from "../schema/jcs.ts";

const root = readIJsonFile<JsonSchemaDocument>(new URL("../schema/continuity.schema.json", import.meta.url));

const defs = (root.$defs ?? {}) as Record<string, Record<string, unknown>>;

/**
 * TS の union 定数 → 対応する `$defs` 名。名前の対応規則が不規則（SENSITIVITIES ↔ Sensitivity、
 * FRESHNESS_VALUES ↔ Freshness）なので明示する。値の集合だけを突き合わせると Freshness と
 * Sensitivity を入れ替えても素通りするため、対応表そのものを契約として持つ。
 */
const NAMED_ENUMS: Record<string, keyof typeof contract> = {
  TaskBindingRole: "TASK_BINDING_ROLES",
  BoundaryEvidenceKind: "BOUNDARY_EVIDENCE_KINDS",
  TaskBoundaryProposalState: "TASK_BOUNDARY_PROPOSAL_STATES",
  TaskBoundaryDecisionSource: "TASK_BOUNDARY_DECISION_SOURCES",
  EvidenceKind: "EVIDENCE_KINDS",
  Freshness: "FRESHNESS_VALUES",
  Sensitivity: "SENSITIVITIES",
  ContinuityCaptureMethod: "CONTINUITY_CAPTURE_METHODS",
  TurnIdSource: "TURN_ID_SOURCES",
  ContinuityOperationPhase: "CONTINUITY_OPERATION_PHASES",
  ReplayPolicy: "REPLAY_POLICIES",
  CheckpointDispositionKind: "CHECKPOINT_DISPOSITION_KINDS",
  DeliveryAttemptState: "DELIVERY_ATTEMPT_STATES",
  EngagementEvidenceKind: "ENGAGEMENT_EVIDENCE_KINDS",
  ReconciliationStatus: "RECONCILIATION_STATUSES",
  ResumeDeliveryStrategy: "RESUME_DELIVERY_STRATEGIES",
  ResumeMode: "RESUME_MODES",
  ResumeDeliveryBoundary: "RESUME_DELIVERY_BOUNDARIES",
  ResumeDecisionAction: "RESUME_DECISION_ACTIONS",
  DerivedArtifactKind: "DERIVED_ARTIFACT_KINDS",
  DerivedArtifactStatus: "DERIVED_ARTIFACT_STATUSES",
  CapabilityTestDisposition: "CAPABILITY_TEST_DISPOSITIONS",
  ContractPreflightState: "CONTRACT_PREFLIGHT_STATES",
};

/**
 * `$defs` 直下ではなく property の中に直接書かれた enum。TS 側は interface の中の
 * inline union なので名前で参照できず、上の対応表には載らない。凍結した値をここに写して
 * 変更を検出する（addendum が inline で書いているものを `$defs` に切り出すと転記が正本から離れる）。
 */
const INLINE_ENUMS = {
  "TaskBoundaryAuthorityContextV1.properties.userSurfaceAuthority.properties.surface": [
    "cli",
    "viewer",
    "mcp_user_authority",
  ],
  "ContinuityIngestAttestationV1.properties.channel": ["rpc", "spool"],
  "ObservedFile.properties.role": ["active", "modified", "read", "test", "config", "unknown"],
  "ObservedCommand.properties.status": ["succeeded", "failed", "unknown"],
  "ObservedTest.properties.status": ["passed", "failed", "partial", "unknown"],
  "PendingOperation.properties.kind": [
    "command",
    "file_mutation",
    "test",
    "tool",
    "migration",
    "external_side_effect",
    "other",
  ],
  "PendingOperation.properties.status": ["started", "succeeded", "failed", "unknown"],
  "DroppedEvidenceEntryV1.properties.reason": ["evicted", "orphaned_terminal"],
  "DroppedEvidenceEntryV1.properties.status": ["started", "succeeded", "failed", "unknown"],
  "ContinuationCheckpointV2.properties.kind": [
    "pre_compact",
    "session_end",
    "idle",
    "manual",
    "crash_recovery",
  ],
  "CheckpointDispositionEvent.properties.source": ["daemon", "runtime", "user"],
  "CheckpointDispositionProjection.properties.state": [
    "open",
    "accepted",
    "superseded",
    "expired",
    "retracted",
  ],
  "CheckpointMetadataV1.properties.kind": [
    "pre_compact",
    "session_end",
    "idle",
    "manual",
    "crash_recovery",
  ],
  "DispositionAuthorityContextV1.properties.source": ["daemon", "runtime", "user"],
  "CheckpointAnchorV1.properties.kind": ["file", "symbol", "command", "test", "todo", "task_lineage"],
  "ContradictionEvidenceV1.properties.kind": [
    "explicit_rejection",
    "new_task_confirmed",
    "workspace_incompatible",
    "runtime_invalidated",
  ],
  "ResumeSelectionDecisionV1.properties.confidenceBand": ["high", "medium", "low", "none"],
  "DerivedArtifactInvalidationEventV1.properties.reason": [
    "memory_updated",
    "memory_superseded",
    "memory_retracted",
    "memory_invalidated",
    "source_artifact_invalidated",
  ],
  "DerivedArtifactInvalidationEventV1.properties.resultingStatus": ["stale", "invalidated"],
  "RequiredCapabilityScenarioV1.properties.requiredFor.items": [
    "generic_phase3",
    "automatic_strategy",
    "tier_a",
  ],
} as const;

/**
 * property に直接書かれた `const`。`oneOf` の判別子（`kind`）と `schemaVersion` が大半で、
 * enum と同じく片方だけ書き換えると TS と schema が食い違う。値が 1 つの enum と同じ扱いにする。
 */
const INLINE_CONSTS = {
  "TaskBoundaryDecisionV1.oneOf[0].properties.kind": "confirm",
  "TaskBoundaryDecisionV1.oneOf[1].properties.kind": "reject",
  "ContinuityEventProvenanceV1.allOf[0].if.properties.evidenceKind": "native",
  "NormalizedContinuityEvent.allOf[0].if.properties.turnIdSource": "unavailable",
  "SemanticResumeNoteV1.properties.schemaVersion": 1,
  "CanonicalWorkStateV1.properties.schemaVersion": 1,
  "ContinuationCheckpointV2.properties.schemaVersion": 2,
  "DeliveryCommandV1.oneOf[0].properties.kind": "mark_delivered",
  "DeliveryCommandV1.oneOf[1].properties.kind": "record_engagement",
  "DeliveryCommandV1.oneOf[2].properties.kind": "accept",
  "DeliveryCommandV1.oneOf[3].properties.kind": "dismiss",
  "DeliveryCommandV1.oneOf[4].properties.kind": "abandon",
  "DeliveryCommandV1.oneOf[5].properties.kind": "renew_lease",
  "ResumeSuppressionEntryV1.properties.reason": "dismissed",
  "ResumeCapsuleV1.properties.schemaVersion": 1,
  "ResumeSelectionDecisionV1.properties.schemaVersion": 1,
  "DerivedArtifactSourceRefV1.oneOf[0].properties.kind": "memory",
  "DerivedArtifactSourceRefV1.oneOf[1].properties.kind": "artifact",
  "DerivedArtifactInvalidationEventV1.allOf[0].if.properties.reason": "source_artifact_invalidated",
} as const;

/* -------------------------------------------------------------------------
 * 上の 2 つの表を TS 型に結び直す。
 *
 * NAMED_ENUMS は TS の runtime 定数と schema を突き合わせるので TS 側の変更で落ちるが、
 * INLINE_ENUMS / INLINE_CONSTS の値は「schema をそのまま写したもの」でしかない。
 * TS の inline union だけを書き換えると、schema も表も揃ったまま素通りする（#25 レビュー指摘）。
 *
 * そこで表の値を `as const` で保ち、TS 型と一致することを **型として** 主張する。
 * schema ↔ 表は実行時の deepEqual、表 ↔ TS 型はここの型検査（CI の
 * `tsc -p harness/tsconfig.json`）が見るので、3 者のどれか 1 つだけを書き換えると落ちる。
 * ------------------------------------------------------------------------- */

type Assert<T extends true> = T;
/** 双方向の包含。union どうしを集合として比較する（片側包含だと欠落を見逃す） */
type SameSet<A extends string | number, B extends string | number> = [A] extends [B]
  ? [B] extends [A]
    ? true
    : false
  : false;
type EnumAt<K extends keyof typeof INLINE_ENUMS> = (typeof INLINE_ENUMS)[K][number];
type ConstAt<K extends keyof typeof INLINE_CONSTS> = (typeof INLINE_CONSTS)[K];
/** 同じ `$defs` に属する複数の `const`（`oneOf` の判別子）をまとめて 1 つの union にする */
type ConstsUnder<P extends string> = {
  [K in keyof typeof INLINE_CONSTS]: K extends `${P}${string}` ? (typeof INLINE_CONSTS)[K] : never;
}[keyof typeof INLINE_CONSTS];

/**
 * 名前付き union も型として突き合わせる。定数は `as const satisfies readonly T[]` なので
 * `typeof CONST[number]` はリテラルの union になり、片側包含では通らない。
 *
 * 注釈を `readonly T[]` にしていたときは、union にだけ値を足しても配列は部分集合として
 * 成立し、schema も定数も無傷なので全 test が通った（TS が許す値を schema が拒否する）。
 */
type _NamedEnumsMatchContract = [
  Assert<SameSet<contract.TaskBindingRole, (typeof contract.TASK_BINDING_ROLES)[number]>>,
  Assert<SameSet<contract.BoundaryEvidenceKind, (typeof contract.BOUNDARY_EVIDENCE_KINDS)[number]>>,
  Assert<SameSet<contract.TaskBoundaryProposalState, (typeof contract.TASK_BOUNDARY_PROPOSAL_STATES)[number]>>,
  Assert<SameSet<contract.TaskBoundaryDecisionSource, (typeof contract.TASK_BOUNDARY_DECISION_SOURCES)[number]>>,
  Assert<SameSet<contract.EvidenceKind, (typeof contract.EVIDENCE_KINDS)[number]>>,
  Assert<SameSet<contract.Freshness, (typeof contract.FRESHNESS_VALUES)[number]>>,
  Assert<SameSet<contract.Sensitivity, (typeof contract.SENSITIVITIES)[number]>>,
  Assert<SameSet<contract.ContinuityCaptureMethod, (typeof contract.CONTINUITY_CAPTURE_METHODS)[number]>>,
  Assert<SameSet<contract.TurnIdSource, (typeof contract.TURN_ID_SOURCES)[number]>>,
  Assert<SameSet<contract.ContinuityOperationPhase, (typeof contract.CONTINUITY_OPERATION_PHASES)[number]>>,
  Assert<SameSet<contract.ReplayPolicy, (typeof contract.REPLAY_POLICIES)[number]>>,
  Assert<SameSet<contract.CheckpointDispositionKind, (typeof contract.CHECKPOINT_DISPOSITION_KINDS)[number]>>,
  Assert<SameSet<contract.DeliveryAttemptState, (typeof contract.DELIVERY_ATTEMPT_STATES)[number]>>,
  Assert<SameSet<contract.EngagementEvidenceKind, (typeof contract.ENGAGEMENT_EVIDENCE_KINDS)[number]>>,
  Assert<SameSet<contract.ReconciliationStatus, (typeof contract.RECONCILIATION_STATUSES)[number]>>,
  Assert<SameSet<contract.ResumeDeliveryStrategy, (typeof contract.RESUME_DELIVERY_STRATEGIES)[number]>>,
  Assert<SameSet<contract.ResumeMode, (typeof contract.RESUME_MODES)[number]>>,
  Assert<SameSet<contract.ResumeDeliveryBoundary, (typeof contract.RESUME_DELIVERY_BOUNDARIES)[number]>>,
  Assert<SameSet<contract.ResumeDecisionAction, (typeof contract.RESUME_DECISION_ACTIONS)[number]>>,
  Assert<SameSet<contract.DerivedArtifactKind, (typeof contract.DERIVED_ARTIFACT_KINDS)[number]>>,
  Assert<SameSet<contract.DerivedArtifactStatus, (typeof contract.DERIVED_ARTIFACT_STATUSES)[number]>>,
  Assert<SameSet<contract.CapabilityTestDisposition, (typeof contract.CAPABILITY_TEST_DISPOSITIONS)[number]>>,
  Assert<SameSet<contract.ContractPreflightState, (typeof contract.CONTRACT_PREFLIGHT_STATES)[number]>>,
];

type _InlineEnumsMatchContract = [
  Assert<
    SameSet<
      NonNullable<contract.TaskBoundaryAuthorityContextV1["userSurfaceAuthority"]>["surface"],
      EnumAt<"TaskBoundaryAuthorityContextV1.properties.userSurfaceAuthority.properties.surface">
    >
  >,
  Assert<
    SameSet<
      contract.ContinuityIngestAttestationV1["channel"],
      EnumAt<"ContinuityIngestAttestationV1.properties.channel">
    >
  >,
  Assert<SameSet<contract.ObservedFile["role"], EnumAt<"ObservedFile.properties.role">>>,
  Assert<SameSet<contract.ObservedCommand["status"], EnumAt<"ObservedCommand.properties.status">>>,
  Assert<SameSet<contract.ObservedTest["status"], EnumAt<"ObservedTest.properties.status">>>,
  Assert<SameSet<contract.PendingOperation["kind"], EnumAt<"PendingOperation.properties.kind">>>,
  Assert<SameSet<contract.PendingOperation["status"], EnumAt<"PendingOperation.properties.status">>>,
  Assert<
    SameSet<contract.DroppedEvidenceEntryV1["reason"], EnumAt<"DroppedEvidenceEntryV1.properties.reason">>
  >,
  Assert<
    SameSet<
      NonNullable<contract.DroppedEvidenceEntryV1["status"]>,
      EnumAt<"DroppedEvidenceEntryV1.properties.status">
    >
  >,
  Assert<
    SameSet<
      contract.ContinuationCheckpointV2["kind"],
      EnumAt<"ContinuationCheckpointV2.properties.kind">
    >
  >,
  Assert<
    SameSet<
      contract.CheckpointDispositionEvent["source"],
      EnumAt<"CheckpointDispositionEvent.properties.source">
    >
  >,
  Assert<
    SameSet<
      contract.CheckpointDispositionProjection["state"],
      EnumAt<"CheckpointDispositionProjection.properties.state">
    >
  >,
  Assert<
    SameSet<contract.CheckpointMetadataV1["kind"], EnumAt<"CheckpointMetadataV1.properties.kind">>
  >,
  Assert<
    SameSet<
      contract.DispositionAuthorityContextV1["source"],
      EnumAt<"DispositionAuthorityContextV1.properties.source">
    >
  >,
  Assert<SameSet<contract.CheckpointAnchorV1["kind"], EnumAt<"CheckpointAnchorV1.properties.kind">>>,
  Assert<
    SameSet<contract.ContradictionEvidenceV1["kind"], EnumAt<"ContradictionEvidenceV1.properties.kind">>
  >,
  Assert<
    SameSet<
      contract.ResumeSelectionDecisionV1["confidenceBand"],
      EnumAt<"ResumeSelectionDecisionV1.properties.confidenceBand">
    >
  >,
  Assert<
    SameSet<
      contract.DerivedArtifactInvalidationEventV1["reason"],
      EnumAt<"DerivedArtifactInvalidationEventV1.properties.reason">
    >
  >,
  Assert<
    SameSet<
      contract.DerivedArtifactInvalidationEventV1["resultingStatus"],
      EnumAt<"DerivedArtifactInvalidationEventV1.properties.resultingStatus">
    >
  >,
  Assert<
    SameSet<
      contract.RequiredCapabilityScenarioV1["requiredFor"][number],
      EnumAt<"RequiredCapabilityScenarioV1.properties.requiredFor.items">
    >
  >,
];

type _InlineConstsMatchContract = [
  // oneOf の判別子: schema の全 branch をまとめて TS の union と突き合わせる
  Assert<SameSet<contract.TaskBoundaryDecisionV1["kind"], ConstsUnder<"TaskBoundaryDecisionV1.oneOf">>>,
  Assert<SameSet<contract.DeliveryCommandV1["kind"], ConstsUnder<"DeliveryCommandV1.oneOf">>>,
  Assert<
    SameSet<contract.DerivedArtifactSourceRefV1["kind"], ConstsUnder<"DerivedArtifactSourceRefV1.oneOf">>
  >,
  // 単一 property の const
  Assert<
    SameSet<contract.ResumeSuppressionEntryV1["reason"], ConstAt<"ResumeSuppressionEntryV1.properties.reason">>
  >,
  Assert<
    SameSet<
      contract.SemanticResumeNoteV1["schemaVersion"],
      ConstAt<"SemanticResumeNoteV1.properties.schemaVersion">
    >
  >,
  Assert<
    SameSet<
      contract.CanonicalWorkStateV1["schemaVersion"],
      ConstAt<"CanonicalWorkStateV1.properties.schemaVersion">
    >
  >,
  Assert<
    SameSet<
      contract.ContinuationCheckpointV2["schemaVersion"],
      ConstAt<"ContinuationCheckpointV2.properties.schemaVersion">
    >
  >,
  Assert<
    SameSet<contract.ResumeCapsuleV1["schemaVersion"], ConstAt<"ResumeCapsuleV1.properties.schemaVersion">>
  >,
  Assert<
    SameSet<
      contract.ResumeSelectionDecisionV1["schemaVersion"],
      ConstAt<"ResumeSelectionDecisionV1.properties.schemaVersion">
    >
  >,
  // if/then の判別値: TS 側の union の要素であること（集合そのものではない）
  Assert<
    ConstAt<"NormalizedContinuityEvent.allOf[0].if.properties.turnIdSource"> extends contract.TurnIdSource
      ? true
      : false
  >,
  Assert<
    ConstAt<"ContinuityEventProvenanceV1.allOf[0].if.properties.evidenceKind"> extends contract.EvidenceKind
      ? true
      : false
  >,
  Assert<
    ConstAt<"DerivedArtifactInvalidationEventV1.allOf[0].if.properties.reason"> extends contract.DerivedArtifactInvalidationEventV1["reason"]
      ? true
      : false
  >,
];

/** `additionalProperties` が schema（false 以外）でよい唯一の定義。任意の JSON を受ける再帰型。 */
const OPEN_OBJECT_ALLOWLIST = new Set(["JsonValue.oneOf[2]"]);

/** `$defs` の各定義を再帰的に歩き、(パス, node) を全部返す。 */
function* walk(node: unknown, path: string): Generator<[string, Record<string, unknown>]> {
  if (Array.isArray(node)) {
    for (const [i, child] of node.entries()) yield* walk(child, `${path}[${i}]`);
    return;
  }
  if (node === null || typeof node !== "object") return;
  const obj = node as Record<string, unknown>;
  yield [path, obj];
  for (const [key, child] of Object.entries(obj)) yield* walk(child, `${path}.${key}`);
}

function* walkDefs(): Generator<[string, Record<string, unknown>, boolean]> {
  for (const [name, def] of Object.entries(defs)) {
    for (const [path, node] of walk(def, name)) yield [path, node, path === name];
  }
}

/**
 * 凍結した `$defs` の名前そのもの。property の形（#31）とは別で、**定義が丸ごと消えても**
 * 誰も気付かない状態を塞ぐ。参照されていない定義は preflight も歩くだけで、
 * 消えたことは検出できない（contract-hashes は再生成すれば通る）
 */
const FROZEN_DEFS = [
  "BoundaryEvidence",
  "BoundaryEvidenceKind",
  "CanonicalWorkStateV1",
  "CapabilityScenarioManifestV1",
  "CapabilityTestDisposition",
  "CheckpointAnchorV1",
  "CheckpointDeliveryAttempt",
  "CheckpointDispositionEvent",
  "CheckpointDispositionKind",
  "CheckpointDispositionProjection",
  "CheckpointMetadataV1",
  "ContinuationCheckpointV2",
  "ContinuityCaptureMethod",
  "ContinuityEventProvenanceV1",
  "ContinuityIngestAttestationV1",
  "ContinuityOperationPhase",
  "ContinuityOperationRefV1",
  "ContractPreflightState",
  "ContradictionEvidenceV1",
  "ContradictionScanRangeV1",
  "DeliveryAttemptState",
  "DeliveryCommandV1",
  "DerivedArtifactDependencyV1",
  "DerivedArtifactInvalidationEventV1",
  "DerivedArtifactKind",
  "DerivedArtifactSourceRefV1",
  "DerivedArtifactStatus",
  "DispositionAuthorityContextV1",
  "DroppedEvidenceEntryV1",
  "EngagementEvaluationContextV1",
  "EngagementEvidence",
  "EngagementEvidenceKind",
  "EvidenceKind",
  "Freshness",
  "IsoTimestamp",
  "JsonPrimitive",
  "JsonValue",
  "NormalizedContinuityEvent",
  "Observed",
  "ObservedCommand",
  "ObservedFile",
  "ObservedTest",
  "OperationCorrelationV1",
  "PendingOperation",
  "RankedResumeCandidateV1",
  "ReconciliationStatus",
  "ReplayPolicy",
  "RepositoryStateSnapshot",
  "RequiredCapabilityScenarioV1",
  "ResumeCapsuleV1",
  "ResumeDecisionAction",
  "ResumeDeliveryBoundary",
  "ResumeDeliveryStrategy",
  "ResumeMode",
  "ResumeSelectionDecisionV1",
  "ResumeSuppressionEntryV1",
  "ResumeThresholdProfileV1",
  "SemanticResumeNoteV1",
  "Sensitivity",
  "SessionTaskBinding",
  "TaskBindingRole",
  "TaskBoundaryAuthorityContextV1",
  "TaskBoundaryDecisionSource",
  "TaskBoundaryDecisionV1",
  "TaskBoundaryProposalState",
  "TaskBoundaryProposalV1",
  "TurnIdSource",
] as const;

test("$defs の名前集合は凍結されている", () => {
  assert.deepEqual(Object.keys(defs).sort(), [...FROZEN_DEFS].sort());
});

/**
 * schema にしかない定義。`IsoTimestamp` は RFC3339 の `pattern` を持つ `string` で、
 * TS 側は素の `string` として書いてある（型として宣言する意味が無い）
 */
const SCHEMA_ONLY_DEFS = ["IsoTimestamp"];

/**
 * 上の凍結は schema ↔ 固定表の 2 点しか見ていないので、**TS の宣言だけ消しても**全部通る
 * （参照の無い型は `tsc` も咎めない）。型は実行時に列挙できないため、ソースから
 * `export type` / `export interface` の名前を取って第 3 点にする。
 *
 * harness は依存ゼロで走らせるので AST parser を持ち込めない。代わりに
 * **この file が書ける export の形を 3 つに限定**して、それ以外を落とす。名前を取れない形
 * （`export type { X }`・`export default interface X`・改行を挟んだ `export type\nX`）を
 * 素通りさせると、宣言が消えたのに集合が一致したままになる。
 */
/**
 * コメントと文字列の中身を消す。行ごとに `//` を探すと `"https://x"` の途中で切れ、
 * `export type Endpoint = "https://x";` が丸ごと消えて宣言を数え損なう。
 * 改行はそのまま残して行の対応を保つ。
 */
function stripCommentsAndStrings(source: string): string {
  let out = "";
  let i = 0;
  while (i < source.length) {
    const two = source.slice(i, i + 2);
    if (two === "//") {
      while (i < source.length && source[i] !== "\n") i++;
      continue;
    }
    if (two === "/*") {
      // 空文字にすると `export/**/interface X {}` が `exportinterface` になって
      // 宣言が走査から消える。区切りとして空白を 1 つ残す
      out += " ";
      for (i += 2; i < source.length && source.slice(i, i + 2) !== "*/"; i++) {
        if (source[i] === "\n") out += "\n";
      }
      i += 2;
      continue;
    }
    const quote = source[i];
    if (quote === '"' || quote === "'" || quote === "`") {
      for (i++; i < source.length && source[i] !== quote; i += source[i] === "\\" ? 2 : 1) {
        if (source[i] === "\n") out += "\n";
      }
      i++;
      out += '""'; // 中身は捨てるが、宣言の形は保つ
      continue;
    }
    out += source[i];
    i++;
  }
  return out;
}

function declaredTypeNames(source: string): string[] {
  const names: string[] = [];
  const code = stripCommentsAndStrings(source);
  // 正規表現リテラルと除算はこの file に無い前提。あると `/*` を誤ってコメント開始と読むので、
  // 素通りさせず落とす（`export const R = /[/*]/;` で以降の宣言が消えていた）
  if (code.includes("/")) {
    assert.fail("continuity.ts に `/` がある（正規表現リテラルや除算は走査の前提外）");
  }
  // 行単位ではなく `export` ごとに見る。`export const C = 1; export interface X {}` のように
  // 1 行に 2 つ書けるため、行の先頭だけを分類すると後ろの宣言を数え落とす
  for (const match of code.matchAll(/\bexport\b/g)) {
    // コメントを挟んだ `export /* doc */ interface X {}` も合法なので、空白は 1 つに潰す
    const statement = code.slice(match.index, match.index + 200).replace(/\s+/g, " ");
    const declaration = /^export (?:type|interface) (\w+)[\s<={]/.exec(statement);
    if (declaration) {
      names.push(declaration[1] as string);
      continue;
    }
    // 値の export（union 定数と CONTINUITY_LIMITS）は型ではないので数えない
    if (/^export const \w+[\s:=]/.test(statement)) continue;
    // `export` という名前の property（`interface X { export: string }`）は宣言ではない
    if (/^export ?\?? ?:/.test(statement)) continue;
    assert.fail(`型名を取れない export の形: ${statement.slice(0, 60)}`);
  }
  return names;
}

test("$defs の名前集合は continuity.ts の型宣言と一致する", () => {
  const source = readFileSync(new URL("../schema/continuity.ts", import.meta.url), "utf8");
  const declared = declaredTypeNames(source);
  assert.equal(new Set(declared).size, declared.length, "同じ名前を 2 度 export している");
  assert.deepEqual(
    declared.sort(),
    FROZEN_DEFS.filter((name) => !SCHEMA_ONLY_DEFS.includes(name)).sort(),
  );
});

test("名前を取れない export の形は落とす（AST を持たない代わりの縛り）", () => {
  const valid = "export type A = string;\nexport interface B {\n  x: string;\n}\nexport const C = [1] as const;\n";
  assert.deepEqual(declaredTypeNames(valid), ["A", "B"]);
  for (const bad of [
    "type X = string;\nexport type { X };\n", // 再 export（名前の位置が違う）
    "export default interface X {}\n",
    "export default interface X {} // note\n", // 行末コメントで消えない
    "export declare type X = string;\n",
    "export const R = 1;\nexport default interface X {}\n", // 1 つでも取れなければ落とす
  ]) {
    assert.throws(() => declaredTypeNames(bad), /型名を取れない export の形/, bad);
  }
  // 合法な書き方は落とさない（false positive はゲートを外させる圧力になる）
  assert.deepEqual(declaredTypeNames("export /* documented */ interface Extra {}\n"), ["Extra"]);
  assert.deepEqual(declaredTypeNames("export type\n  X = string;\n"), ["X"]); // 改行を跨ぐ宣言
  assert.deepEqual(declaredTypeNames("export interface A<T> {\n  x: T;\n}\n"), ["A"]);
  // 1 行に 2 つ書いても両方数える
  assert.deepEqual(declaredTypeNames("export const C = 1; export interface Extra {}\n"), ["Extra"]);
  // コメントを区切りに使った宣言も拾う（空文字に潰すと `exportinterface` になって消える）
  assert.deepEqual(declaredTypeNames("export/**/interface Extra {}\n"), ["Extra"]);
  assert.deepEqual(declaredTypeNames("export/*doc*/type A = string;\n"), ["A"]);
  // `export` という名前の property は宣言ではない
  assert.deepEqual(
    declaredTypeNames("export interface HasField {\n  export: string;\n}\n"),
    ["HasField"],
  );
  assert.deepEqual(
    declaredTypeNames("export interface HasField {\n  export?/**/: string;\n}\n"),
    ["HasField"],
  );
  // 正規表現リテラルは走査の前提外なので素通りさせない
  assert.throws(
    () => declaredTypeNames("export const R = /[/*]/;\nexport interface Extra {}\n"),
    /正規表現リテラル/,
  );
  // 字下げした宣言も拾う（拾わないと「型を足したのに凍結ゲートが気付かない」側に倒れる）
  assert.deepEqual(declaredTypeNames("  export interface Extra {}\n"), ["Extra"]);
  // 行末コメント・文字列の中の `//` で宣言を消さない
  assert.deepEqual(declaredTypeNames("export type A = string; // note\n"), ["A"]);
  assert.deepEqual(declaredTypeNames('export type Endpoint = "https://example.test";\n'), ["Endpoint"]);
  assert.deepEqual(declaredTypeNames('export type Q = "/* not a comment */";\nexport type B = 1;\n'), ["Q", "B"]);
  // コメントの中の宣言は数えない
  assert.deepEqual(declaredTypeNames("/*\nexport interface Ghost {}\n*/\nexport type A = string;\n"), ["A"]);
  assert.deepEqual(declaredTypeNames("// export interface Ghost {}\nexport type A = string;\n"), ["A"]);
});

test("continuity.schema.json は schema 側の誤記を持たない", () => {
  // validateContractValue は root を 1 度歩いて schema 側の誤記を throw する（値は通らなくてよい）。
  // $ref から辿れない $defs も対象なので、凍結した 67 定義すべてが検査される
  assert.doesNotThrow(() =>
    validateContractValue(Object.keys(defs)[0], undefined, root, contract.CONTINUITY_LIMITS),
  );
});

test("object はすべて closed（ネストした定義も含む）", () => {
  const open: string[] = [];
  for (const [path, node] of walkDefs()) {
    if (node.type !== "object") continue;
    if (node.additionalProperties === false) continue;
    if (OPEN_OBJECT_ALLOWLIST.has(path)) continue;
    open.push(path);
  }
  assert.deepEqual(open, []);
});

test("TS の union 定数と schema の enum が名前ごとに一致する", () => {
  const sorted = (xs: readonly string[]) => [...xs].sort();
  for (const [defName, constName] of Object.entries(NAMED_ENUMS)) {
    const fromSchema = defs[defName]?.enum;
    assert.ok(Array.isArray(fromSchema), `$defs.${defName} に enum が無い`);
    assert.deepEqual(
      sorted(contract[constName] as readonly string[]),
      sorted(fromSchema as string[]),
      `${constName} ↔ $defs.${defName}`,
    );
  }
  // 対応表の側が古くならないよう、schema にある名前付き enum の数とも突き合わせる
  const namedInSchema = Object.entries(defs)
    .filter(([, d]) => Array.isArray(d.enum))
    .map(([name]) => name);
  assert.deepEqual(namedInSchema.sort(), Object.keys(NAMED_ENUMS).sort());
});

test("property の中に直接書かれた enum も凍結する", () => {
  const found: Record<string, readonly string[]> = {};
  for (const [path, node, isTopLevel] of walkDefs()) {
    if (isTopLevel || !Array.isArray(node.enum)) continue;
    found[path] = node.enum as string[];
  }
  assert.deepEqual(found, INLINE_ENUMS);
});

test("同じ語彙を複製している inline enum どうしも突き合わせる", () => {
  // 上の test は 1 つずつ「今の綴り」に縛るだけなので、**複製した片方だけを増やす**変更は
  // 素通りする（両方の行を更新すれば緑のまま）。`DroppedEvidenceEntryV1.status` は退避した
  // `PendingOperation` の status をそのまま写す欄なので、語彙が割れたら記録できない状態が出る。
  // 凍結 schema なので $ref への差し替え（= 既存 node の削除）はできない。ここで縛る
  assert.deepEqual(
    INLINE_ENUMS["DroppedEvidenceEntryV1.properties.status"],
    INLINE_ENUMS["PendingOperation.properties.status"],
  );
});

test("property の中に直接書かれた const も凍結する", () => {
  // oneOf の判別子は値が 1 つの enum と同じ。片方だけ書き換えると、TS の
  // `{ kind: "mark_delivered" }` を満たす値が runtime 検証で落ちる
  const found: Record<string, unknown> = {};
  for (const [path, node, isTopLevel] of walkDefs()) {
    if (isTopLevel || !Object.hasOwn(node, "const")) continue;
    found[path] = node.const;
  }
  assert.deepEqual(found, INLINE_CONSTS);
});

test("turnId は turnIdSource と対で成立する（§3.1）", () => {
  // turnId 以外は契約を満たす形にしておく。ここが崩れていると turnId の可否ではなく
  // 別の違反を数えていても気付けない（下の assert で全体が 0 件であることも見る）
  const base: contract.NormalizedContinuityEvent = {
    eventId: "e1",
    canonicalFingerprint: "f1",
    kind: "prompt",
    ingestSeq: "1",
    occurredAt: "2026-08-16T00:00:00Z",
    sessionId: "s1",
    sourceAgent: "claude@1.0.0",
    provenance: {
      sourceAgentVersion: "1.0.0",
      evidenceKind: "synthesized",
      captureMethod: "hook",
    },
    payload: {},
    turnIdSource: "unavailable",
  };
  const ev = (turnIdSource: string, turnId?: string) => ({
    ...base,
    turnIdSource,
    ...(turnId === undefined ? {} : { turnId }),
  });
  const schema = { $ref: "#/$defs/NormalizedContinuityEvent" };
  const issues = (value: unknown) => validateAgainstSchema(value, schema, root);
  const violates = (value: unknown) =>
    issues(value).some((i) => /turnId/.test(`${i.path} ${i.message}`));

  // 土台そのものは 1 件も違反しない
  assert.deepEqual(issues(ev("unavailable")), []);

  // unavailable のときは turnId を持ってはいけない
  assert.equal(violates(ev("unavailable")), false);
  assert.equal(violates(ev("unavailable", "t1")), true);
  // native / synthesized_monotonic のときは turnId が要る
  for (const src of ["native", "synthesized_monotonic"]) {
    assert.equal(violates(ev(src, "t1")), false, src);
    assert.equal(violates(ev(src)), true, src);
  }
});

test("evidenceKind=native は attestation 一式を伴う（§3.1）", () => {
  // 凍結表は `if` の中身が書き換わったことは見るが、条件が実際に発火するかは見ない。
  // property 名を打ち間違えた `if` は既存の fixture を素通しするので、ここで発火を見る
  const base: contract.ContinuityEventProvenanceV1 = {
    sourceAgentVersion: "1.0.0",
    evidenceKind: "synthesized",
    captureMethod: "hook",
  };
  const nativeExtras: Record<string, unknown> = {
    capabilityHash: "h1",
    scenarioId: "sc1",
    ingestAttestation: {
      ingestReceiptId: "r1",
      peerIdentityId: "p1",
      channel: "rpc",
      attestedAt: "2026-08-16T00:00:00Z",
    },
  };
  const schema = { $ref: "#/$defs/ContinuityEventProvenanceV1" };
  const issues = (value: unknown) => validateAgainstSchema(value, schema, root);

  // 土台も、native に一式そろえた形も、違反ゼロ
  assert.deepEqual(issues(base), []);
  assert.deepEqual(issues({ ...base, evidenceKind: "native", ...nativeExtras }), []);

  for (const key of ["capabilityHash", "scenarioId", "ingestAttestation"]) {
    const partial = { ...nativeExtras };
    delete partial[key];
    const found = issues({ ...base, evidenceKind: "native", ...partial });
    assert.ok(
      found.some((i) => `${i.path} ${i.message}`.includes(key)),
      `native なのに ${key} 欠落が見逃された: ${JSON.stringify(found)}`,
    );
    // native 以外では同じ欠落が違反にならない（条件付きであることの確認）
    for (const kind of ["synthesized", "derived"]) {
      assert.deepEqual(issues({ ...base, evidenceKind: kind, ...partial }), [], `${kind}/${key}`);
    }
  }
});

test("子孫の無効化は viaArtifactId を伴う（§12.3）", () => {
  const base: contract.DerivedArtifactInvalidationEventV1 = {
    eventId: "e1",
    artifactId: "a1",
    artifactKind: "summary",
    expectedArtifactRevision: "r1",
    sourceMemoryId: "m1",
    invalidatingMemoryRevision: "mr1",
    hopDepth: 0,
    reason: "memory_updated",
    resultingStatus: "stale",
    idempotencyKey: "k1",
    createdAt: "2026-08-16T00:00:00Z",
  };
  const schema = { $ref: "#/$defs/DerivedArtifactInvalidationEventV1" };
  const issues = (value: unknown) => validateAgainstSchema(value, schema, root);

  assert.deepEqual(issues(base), []);
  assert.deepEqual(
    issues({ ...base, reason: "source_artifact_invalidated", hopDepth: 1, viaArtifactId: "a0" }),
    [],
  );

  // 子孫（source_artifact_invalidated）でだけ viaArtifactId が要る
  const missing = issues({ ...base, reason: "source_artifact_invalidated", hopDepth: 1 });
  assert.ok(
    missing.some((i) => `${i.path} ${i.message}`.includes("viaArtifactId")),
    JSON.stringify(missing),
  );
  for (const reason of ["memory_superseded", "memory_retracted", "memory_invalidated"]) {
    assert.deepEqual(issues({ ...base, reason }), [], reason);
  }
});

test("数値の範囲は正本が書いているものだけ", () => {
  // 正本に無い範囲を足すと、正当な値を契約が拒否する（IsoTimestamp の小数秒で実際に起きた）。
  // addendum で数値範囲が書かれているのは `Observed.confidence` の `// 0..1` 1 箇所だけなので、
  // schema 側の minimum/maximum もそこだけであることを固定する
  // path だけを集めると、`minimum` を -1 にしても `maximum` を消しても同じ集合になる。
  // 値まで凍結して、唯一の根拠つき範囲が弱められたときにも落ちるようにする
  const found: { path: string; minimum?: unknown; maximum?: unknown }[] = [];
  const walk = (node: unknown, path: string) => {
    if (Array.isArray(node)) {
      node.forEach((item, i) => walk(item, `${path}[${i}]`));
      return;
    }
    if (node === null || typeof node !== "object") return;
    const obj = node as Record<string, unknown>;
    if ("minimum" in obj || "maximum" in obj) {
      found.push({ path, minimum: obj.minimum, maximum: obj.maximum });
    }
    for (const [k, v] of Object.entries(obj)) walk(v, `${path}.${k}`);
  };
  walk(defs, "$defs");
  assert.deepEqual(found, [
    { path: "$defs.Observed.properties.confidence", minimum: 0, maximum: 1 },
  ]);
});

test("IsoTimestamp は成分の範囲まで見る", () => {
  const bad = (s: string) => validateAgainstSchema(s, { $ref: "#/$defs/IsoTimestamp" }, root).length > 0;
  assert.equal(bad("2026-08-16T00:00:00Z"), false);
  assert.equal(bad("2026-08-16T23:59:59.999Z"), false);
  // 正本は RFC3339（`time-secfrac = "." 1*DIGIT`）。小数秒の桁数を 3 に固定すると、
  // マイクロ秒で出す実装の正当な timestamp を契約が拒否してしまう
  for (const s of ["2026-08-16T00:00:00.1Z", "2026-08-16T00:00:00.123456Z"]) {
    assert.equal(bad(s), false, s);
  }
  // 小数点だけ・桁ゼロは RFC3339 でも不正
  assert.equal(bad("2026-08-16T00:00:00.Z"), true);
  for (const s of [
    "2026-99-99T99:99:99Z",
    "2026-00-16T00:00:00Z",
    "2026-13-16T00:00:00Z",
    "2026-08-00T00:00:00Z",
    "2026-08-32T00:00:00Z",
    "2026-08-16T24:00:00Z",
    "2026-08-16T00:60:00Z",
    "2026-08-16T00:00:60Z",
    // canonical profile の「offset は `Z` 固定」側（#33）。RFC3339 としては正当だが、同じ瞬間の
    // 綴りが 2 通りあると canonical hash と §22.8 の dedupe が割れるので拒否する。上の秒の範囲
    // （うるう秒 `:60` の拒否）は固定していたのに offset 側は 1 件も検証しておらず、pattern から
    // `Z` を外しても全 test が緑のままだった
    "2026-08-16T00:00:00+00:00",
    "2026-08-16T00:00:00-00:00",
    "2026-08-16T00:00:00+09:00",
    "2026-08-16T00:00:00.123+00:00",
    "2026-08-16T00:00:00z",
    "2026-08-16T00:00:00",
  ]) {
    assert.equal(bad(s), true, s);
  }
  // 暦としての実在は pattern では表せないので schema では通る。落とすのは runtime 層で、
  // `reference-model.ts` の `assertIdentityMaterial` が `occurredAt` を検査する（#27）。
  // ここを「落ちる」に変えたくなったら、pattern で暦を書けるようになったということなので、
  // runtime 側の検査が要らなくなる。その事実ごと壊れるべきなので schema 側の挙動を固定する
  assert.equal(bad("2026-02-30T00:00:00Z"), false);
});

test("decimal string の pattern は sequence/watermark 全部に付いている（正本 §22.6）", () => {
  // §22.6 は seq / epoch を decimal string として wire に出すと定めている。1 箇所だけ外しても
  // 他の test は通るので、pattern が付いている path の集合そのものを凍結する
  // `ContinuationCheckpointV2.memoryWatermark` は addendum が `string` としか書いておらず
  // §22.6 の列挙（server seq / device seq / epoch）にも入らないので、ここには含めない
  const decimal = "^(0|[1-9][0-9]*)$";
  const found: string[] = [];
  for (const [path, node] of walkDefs()) {
    if (node.pattern === decimal) found.push(path);
  }
  assert.deepEqual(found.sort(), [
    "CanonicalWorkStateV1.properties.lastIngestSeq",
    "ContradictionScanRangeV1.properties.fromIngestSeq",
    "ContradictionScanRangeV1.properties.toIngestSeq",
    "NormalizedContinuityEvent.properties.ingestSeq",
    "Observed.properties.ingestSeq",
    "PendingOperation.properties.startIngestSeq",
    "SemanticResumeNoteV1.properties.generatedFromIngestSeq",
  ]);
});

test("PendingOperation に足した 3 欄は任意のまま（#35 / #44）", () => {
  // 凍結後に足す欄を `required` に入れると、**この版より前に書かれた checkpoint** が
  // 復元時に丸ごと落ちる。欄そのものの有無ではなく「required に入っていないこと」を固定する
  const base: contract.PendingOperation = {
    operationId: "op1",
    correlation: {
      operationId: "op1",
      startEventId: "e1",
      operationMatchKey: "mk1",
      sessionId: "s1",
      taskLineageId: "l1",
    },
    kind: "command",
    description: "d",
    status: "started",
    replayPolicy: "never_auto",
    sourceEventIds: ["e1"],
    startedAt: "2026-08-16T00:00:00Z",
    sensitivity: "normal",
  };
  const schema = { $ref: "#/$defs/PendingOperation" };
  const issues = (value: unknown) => validateAgainstSchema(value, schema, root);

  assert.deepEqual(issues(base), []);
  assert.deepEqual(
    issues({
      ...base,
      startIngestSeq: "7",
      startTurnIdSource: "native",
      terminalFingerprint: "f1",
    }),
    [],
  );

  // 形は効いている（任意 = 何でも入る、ではない）
  for (const [field, bad] of [
    ["startIngestSeq", "007"],
    ["startIngestSeq", "not-a-decimal"],
    ["startTurnIdSource", "guessed"],
    ["terminalFingerprint", 1],
  ] as const) {
    const found = issues({ ...base, [field]: bad });
    assert.ok(found.length > 0, `${field}=${JSON.stringify(bad)} が素通りした`);
  }
});

test("droppedEvidence は共通欄だけを必須にする（#43 / #39）", () => {
  const entry: contract.DroppedEvidenceEntryV1 = {
    reason: "evicted",
    recordedAt: "2026-08-16T00:00:00Z",
    sensitivity: "normal",
  };
  const schema = { $ref: "#/$defs/DroppedEvidenceEntryV1" };
  const issues = (value: unknown) => validateAgainstSchema(value, schema, root);

  // 理由ごとの欄は任意。退避は operation を、孤児 terminal は event を名指しできるが、
  // どちらの形も同じ 1 定義で通る（oneOf で分岐させると理由が増えるたび移植が割れる）
  assert.deepEqual(issues(entry), []);
  assert.deepEqual(issues({ ...entry, operationId: "op1", status: "started" }), []);
  assert.deepEqual(issues({ ...entry, reason: "orphaned_terminal", eventId: "e1" }), []);

  // #39 の重複判定の鍵は文字列で、上限も他の識別子と揃っている。欄名の突き合わせは
  // 「欄が在るか」しか見ないので、型と上限だけが drift すると素通りする
  assert.deepEqual(issues({ ...entry, reason: "orphaned_terminal", terminalFingerprint: "f1" }), []);
  assert.ok(issues({ ...entry, terminalFingerprint: 1 }).length > 0);
  assert.ok(issues({ ...entry, terminalFingerprint: "x".repeat(8193) }).length > 0);

  for (const missing of ["reason", "recordedAt", "sensitivity"]) {
    const partial = Object.fromEntries(Object.entries(entry).filter(([field]) => field !== missing));
    const found = issues(partial);
    assert.ok(
      found.some((i) => `${i.path} ${i.message}`.includes(missing)),
      `${missing} の欠落が見逃された: ${JSON.stringify(found)}`,
    );
  }
  // 語彙の外と、知らない欄は落とす
  assert.ok(issues({ ...entry, reason: "trimmed" }).length > 0);
  assert.ok(issues({ ...entry, droppedBecause: "x" }).length > 0);

  // 上限を落とすと、退避の記録そのものが状態を無限に膨らませる（落とした証跡が本体より重くなる）
  const stateDef = defs.CanonicalWorkStateV1;
  assert.ok(stateDef, "$defs.CanonicalWorkStateV1 が無い");
  const state = stateDef.properties as Record<string, { maxItems?: number } | undefined>;
  assert.equal(state.droppedEvidence?.maxItems, contract.CONTINUITY_LIMITS.arrayItems);
  assert.equal(state.droppedEvidence?.maxItems, state.pendingOperations?.maxItems);
});

/**
 * 上の test 群は schema の側しか縛らない。TS 型は object literal の注釈として出てくるだけで、
 * 値は `unknown` 経由で validator に渡るため**余剰プロパティ検査が働かず**、schema にだけ欄を
 * 足しても素通りする（実測: `DroppedEvidenceEntryV1.terminalFingerprint` を TS 側から消しても
 * 凍結 test 17 件が全部 green のままだった）。TS mirror が schema と同じ形であることは
 * TS/Rust parity の前提そのものなので、欄名の集合を型と runtime の両方で突き合わせる。
 *
 * `Record<keyof T, true>` にしているのは、**両方向**を型で縛るため。TS から欄が消えれば
 * 余剰プロパティとして、TS に欄が増えれば不足として、どちらも `tsc` が落ちる。
 */
const DROPPED_EVIDENCE_FIELDS: Record<keyof contract.DroppedEvidenceEntryV1, true> = {
  reason: true,
  recordedAt: true,
  sensitivity: true,
  eventId: true,
  operationId: true,
  status: true,
  terminalFingerprint: true,
  adapterDeliveryId: true,
};

/** 相互代入可能性。片側だけの `extends` は広がりを見逃す */
type Exact<A, B> = [A] extends [B] ? ([B] extends [A] ? true : never) : never;

test("DroppedEvidenceEntryV1 の欄集合は schema と TS で一致する（#43 / #39）", () => {
  const entryDef = defs.DroppedEvidenceEntryV1;
  assert.ok(entryDef, "$defs.DroppedEvidenceEntryV1 が無い");
  const properties = entryDef.properties as Record<string, unknown>;
  assert.deepEqual(Object.keys(properties).sort(), Object.keys(DROPPED_EVIDENCE_FIELDS).sort());
});

test("重複判定の鍵は schema でも TS でも string（#39）", () => {
  // 欄名の集合しか見ていないと**値型の drift が残る**。`terminalFingerprint` を
  // `string | number` に広げても、`Record<keyof ...>` も validator の test も既存の string 代入も
  // 全部通る（実測）。鍵は同一性を決める欄なので、型と上限を両方向で tsc に見せる
  const exact: {
    terminalFingerprint: Exact<contract.DroppedEvidenceEntryV1["terminalFingerprint"], string | undefined>;
    adapterDeliveryId: Exact<contract.DroppedEvidenceEntryV1["adapterDeliveryId"], string | undefined>;
  } = { terminalFingerprint: true, adapterDeliveryId: true };
  assert.deepEqual(Object.values(exact), [true, true]);

  const entryDef = defs.DroppedEvidenceEntryV1;
  assert.ok(entryDef, "$defs.DroppedEvidenceEntryV1 が無い");
  const properties = entryDef.properties as Record<string, { type?: string; maxLength?: number }>;
  for (const field of ["terminalFingerprint", "adapterDeliveryId"] as const) {
    assert.equal(properties[field]?.type, "string", `${field} の type`);
    assert.equal(properties[field]?.maxLength, 8192, `${field} の maxLength`);
  }
});
