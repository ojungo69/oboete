import { test } from "node:test";
import assert from "node:assert/strict";
import { canonicalizeJson, readIJsonFile } from "../schema/jcs.ts";
import { validateContractValue, type JsonSchemaDocument } from "../schema/validate.ts";
import {
  EVENT_KINDS,
  type EventKind,
} from "../schema/capability.ts";
import {
  CONTINUITY_LIMITS,
  NON_OPERATION_EVENT_KINDS,
  OPERATION_EVENT_PHASES,
  type CanonicalWorkStateV1,
  type DroppedEvidenceEntryV1,
  type NormalizedContinuityEvent,
  type PendingOperation,
} from "../schema/continuity.ts";
import {
  assertOperationEnvelope,
  assertTurnIdentity,
  compareIngestSeq,
  contentHashOf,
  correlateTerminalEvent,
  finalizeAbandonedState,
  idempotencyKeyOf,
  ledgerKeyOf,
  reduceTaskWorkState,
  stampIntakeEvidence,
  type IdempotencyLedger,
  type IntakeContextV1,
  type IntakeStampedEventV1,
  type LedgerEntryV1,
  type TaskWorkStateSnapshotV1,
} from "./reference-model.ts";

const SCHEMA_ROOT = readIJsonFile<JsonSchemaDocument>(
  new URL("../schema/continuity.schema.json", import.meta.url),
);

const CAPABILITY_HASH = "b1946ac92492d2347c6235b4d2611184e2f4a1d94d4b3f7d3b5c3f9d0c6e8a11";
const VERSION = "2.1.228 (Claude Code)";

const START_OPERATION = {
  phase: "start",
  operationMatchKey: "match-key-1",
  operationKind: "Bash",
  nativeOperationId: "toolu_1",
  canonicalInputHash: "input-hash-1",
} as const satisfies NonNullable<NormalizedContinuityEvent["operation"]>;

const TERMINAL_OPERATION = { ...START_OPERATION, phase: "terminal" } as const;

/** nativeOperationId を出さない adapter（rule 2 でしか閉じられない） */
const MATCH_KEY_ONLY = { ...START_OPERATION, nativeOperationId: undefined } as const;

const ATTESTATION = {
  ingestReceiptId: "receipt-1",
  peerIdentityId: "peer-1",
  channel: "rpc",
  attestedAt: "2026-08-16T00:00:01Z",
} as const;

const INTAKE: IntakeContextV1 = {
  expectedSourceAgent: "claude",
  expectedSessionId: "session-1",
  exactAgentVersion: VERSION,
  nativeTurnIdentityProven: true,
  activeCapabilityHash: CAPABILITY_HASH,
  provenScenarios: [
    { scenarioId: "tool-call-lifecycle", captureMethod: "native_event", channel: "rpc" },
  ],
  attestation: ATTESTATION,
};

function emptyState(overrides: Partial<CanonicalWorkStateV1> = {}): CanonicalWorkStateV1 {
  return {
    schemaVersion: 1,
    taskLineageId: "lineage-1",
    projectId: "project-1",
    workspaceId: "workspace-1",
    sourceAgent: "claude",
    activeFiles: [],
    modifiedFiles: [],
    recentCommands: [],
    recentTests: [],
    pendingOperations: [],
    repositoryState: {
      repositoryId: "repo-1",
      workspaceId: "workspace-1",
      capturedAt: "2026-08-16T00:00:00Z",
    },
    sensitivity: "normal",
    lastIngestSeq: "10",
    stateRevision: "genesis",
    updatedAt: "2026-08-16T00:00:00Z",
    ...overrides,
  };
}

function emptySnapshot(overrides: Partial<CanonicalWorkStateV1> = {}): TaskWorkStateSnapshotV1 {
  return { state: emptyState(overrides), history: [] };
}

/**
 * intake 済みとして扱う目印を付ける。目印は型だけのものなので値は変わらない。test は
 * intake の各分岐を stampIntakeEvidence 側で直接確かめるので、還元器の test では
 * 組み立てた event をそのまま渡す。
 */
function asStamped(event: NormalizedContinuityEvent): IntakeStampedEventV1 {
  return event as IntakeStampedEventV1;
}

function startEvent(overrides: Partial<NormalizedContinuityEvent> = {}): IntakeStampedEventV1 {
  return asStamped({
    eventId: "event-start",
    adapterDeliveryId: "delivery-start",
    canonicalFingerprint: "fingerprint-start",
    kind: "tool_started",
    ingestSeq: "11",
    occurredAt: "2026-08-16T00:00:01Z",
    sessionId: "session-1",
    taskLineageId: "lineage-1",
    turnId: "turn-1",
    turnIdSource: "native",
    sourceAgent: "claude",
    provenance: {
      sourceAgentVersion: VERSION,
      evidenceKind: "native",
      captureMethod: "native_event",
      capabilityHash: CAPABILITY_HASH,
      scenarioId: "tool-call-lifecycle",
      ingestAttestation: ATTESTATION,
    },
    operation: START_OPERATION,
    payload: { tool_name: "Bash" },
    ...overrides,
  });
}

function terminalEvent(overrides: Partial<NormalizedContinuityEvent> = {}): IntakeStampedEventV1 {
  return asStamped({
    ...startEvent(),
    eventId: "event-terminal",
    adapterDeliveryId: "delivery-terminal",
    canonicalFingerprint: "fingerprint-terminal",
    kind: "tool_completed",
    ingestSeq: "12",
    occurredAt: "2026-08-16T00:00:02Z",
    operation: TERMINAL_OPERATION,
    payload: { tool_response: "ok" },
    successful: true,
    ...overrides,
  });
}

function deepFreeze<T>(value: T): T {
  if (value !== null && typeof value === "object") {
    for (const child of Object.values(value)) deepFreeze(child);
    Object.freeze(value);
  }
  return value;
}

function apply(
  snapshot: TaskWorkStateSnapshotV1,
  events: readonly IntakeStampedEventV1[],
  ledger: IdempotencyLedger = new Map(),
): { snapshot: TaskWorkStateSnapshotV1; ledger: IdempotencyLedger } {
  let current = snapshot;
  let currentLedger = ledger;
  for (const event of events) {
    const result = reduceTaskWorkState(current, event, currentLedger);
    current = result.snapshot;
    currentLedger = result.ledger;
  }
  return { snapshot: current, ledger: currentLedger };
}

// --- event kind の分類（#29） -----------------------------------------------

test("operation 系 kind と非 operation kind は EVENT_KINDS を過不足なく分ける", () => {
  // 語彙に kind を足したとき、どちらに属するか決めないまま素通りさせない。
  // 期待値を手で並べると足した kind が両方から漏れるので、EVENT_KINDS から導く。
  const classified = [...Object.keys(OPERATION_EVENT_PHASES), ...NON_OPERATION_EVENT_KINDS].sort();
  assert.deepEqual(classified, [...EVENT_KINDS].sort());
  assert.equal(new Set(classified).size, classified.length);
});

// --- §22.6 decimal string の比較 --------------------------------------------

test("ingestSeq は safe integer を超えても桁順・辞書順で比較できる", () => {
  // Number 化すると 9007199254740993 と 9007199254740992 が同値になり、順序が壊れる
  assert.equal(Number("9007199254740993") === Number("9007199254740992"), true);
  assert.equal(compareIngestSeq("9007199254740993", "9007199254740992"), 1);
  assert.equal(compareIngestSeq("9007199254740992", "9007199254740993"), -1);
  assert.equal(compareIngestSeq("9007199254740993", "9007199254740993"), 0);
  // 桁数が違えば辞書順ではなく桁数で決まる
  assert.equal(compareIngestSeq("9", "10"), -1);
  assert.equal(compareIngestSeq("0", "0"), 0);
});

test("decimal string でない ingestSeq は比較しない", () => {
  assert.throws(() => compareIngestSeq("01", "1"), /decimal string でない/);
  assert.throws(() => compareIngestSeq("1e3", "1"), /decimal string でない/);
  assert.throws(() => compareIngestSeq("-1", "1"), /decimal string でない/);
});

test("lastIngestSeq は遅れて届いた event で戻らない", () => {
  const late = startEvent({ ingestSeq: "3", eventId: "event-late" });
  const { snapshot } = apply(emptySnapshot({ lastIngestSeq: "9007199254740993" }), [late]);
  assert.equal(snapshot.state.lastIngestSeq, "9007199254740993");
  // 遅れて届いても revision は新しく作る（§4.2「Late ... events create later revisions」）
  assert.equal(snapshot.history.length, 1);
  assert.notEqual(snapshot.state.stateRevision, "genesis");
});

// --- §4.2 重複 no-op --------------------------------------------------------

test("同じ adapterDeliveryId を 10 回適用しても最初の 1 回しか効かない", () => {
  const first = reduceTaskWorkState(emptySnapshot(), startEvent(), new Map());
  assert.equal(first.outcome, "applied");
  const bytes = canonicalizeJson(first.snapshot.state);

  let snapshot = first.snapshot;
  let ledger = first.ledger;
  for (let attempt = 0; attempt < 10; attempt += 1) {
    // 再送は eventId・ingestSeq・occurredAt が違っても同じ配送 ID を持つ
    const redelivered = startEvent({
      eventId: `event-start-retry-${attempt}`,
      ingestSeq: String(20 + attempt),
      occurredAt: "2026-08-16T00:00:09Z",
    });
    const result = reduceTaskWorkState(snapshot, redelivered, ledger);
    assert.equal(result.outcome, "duplicate");
    assert.equal(canonicalizeJson(result.snapshot.state), bytes);
    assert.equal(result.contentHash, first.contentHash);
    assert.equal(result.snapshot.state.stateRevision, first.snapshot.state.stateRevision);
    assert.equal(result.snapshot.history.length, first.snapshot.history.length);
    assert.equal(result.ledger, ledger);
    snapshot = result.snapshot;
    ledger = result.ledger;
  }
  assert.equal(ledger.size, 1);
});

test("adapterDeliveryId が無い event は canonical fingerprint で重複判定する", () => {
  const event = startEvent({ adapterDeliveryId: undefined });
  assert.equal(idempotencyKeyOf(event), "fingerprint-start");
  const first = reduceTaskWorkState(emptySnapshot(), event, new Map());
  const again = reduceTaskWorkState(first.snapshot, { ...event, eventId: "other" }, first.ledger);
  assert.equal(again.outcome, "duplicate");
});

test("adapterDeliveryId に他 event の fingerprint を書いても先取りできない", () => {
  // v6 §8.2 の導出式は adapterDeliveryId と canonicalFingerprint を同じ keyspace に置く。
  // adapterDeliveryId は adapter が自由に採番する値なので、被害者の fingerprint を名乗る event を
  // 先に送ると、本物が診断ゼロの重複として消える
  const poison = reduceTaskWorkState(
    emptySnapshot(),
    startEvent({ eventId: "event-poison", adapterDeliveryId: "fingerprint-victim" }),
    new Map(),
  );
  assert.equal(poison.outcome, "applied");
  const victim = reduceTaskWorkState(
    poison.snapshot,
    startEvent({
      eventId: "event-victim",
      adapterDeliveryId: undefined,
      canonicalFingerprint: "fingerprint-victim",
      ingestSeq: "13",
      operation: { ...START_OPERATION, nativeOperationId: "toolu_victim" },
    }),
    poison.ledger,
  );
  assert.equal(victim.outcome, "applied");
  assert.equal(victim.snapshot.state.pendingOperations.length, 2);
});

test("同じ配送 ID で source hash が違う再配送は隔離する", () => {
  // §4.3「terminal は … payload/source hash が衝突しないこと」。重複として捨てると衝突検査が
  // 到達不能になり、訂正版の再配送も同じ鍵で消える
  const first = reduceTaskWorkState(emptySnapshot(), startEvent(), new Map());
  assert.equal(first.outcome, "applied");
  const conflicting = reduceTaskWorkState(
    first.snapshot,
    startEvent({ eventId: "event-start-corrupt", ingestSeq: "13", canonicalFingerprint: "fingerprint-other" }),
    first.ledger,
  );
  assert.equal(conflicting.outcome, "quarantined");
  assert.deepEqual(
    conflicting.diagnostics.map((d) => d.code),
    ["delivery_conflict"],
  );
  assert.equal(conflicting.ledger, first.ledger);
  // 同じ内容の再送はこれまでどおり重複 no-op
  const retry = reduceTaskWorkState(
    first.snapshot,
    startEvent({ eventId: "event-start-retry" }),
    first.ledger,
  );
  assert.equal(retry.outcome, "duplicate");
});

test("配送 ID が違えば別の論理 event として適用される", () => {
  // §8.2 の第一 authority は adapterDeliveryId。同じ fingerprint でも配送 ID が違えば
  // 「同一論理 event の再送」ではない
  const first = reduceTaskWorkState(emptySnapshot(), startEvent(), new Map());
  const other = startEvent({
    eventId: "event-start-2",
    adapterDeliveryId: "delivery-start-2",
    ingestSeq: "13",
    operation: { ...START_OPERATION, operationMatchKey: "match-key-2", nativeOperationId: "toolu_2" },
  });
  const second = reduceTaskWorkState(first.snapshot, other, first.ledger);
  assert.equal(second.outcome, "applied");
  assert.equal(second.snapshot.state.pendingOperations.length, 2);
});

test("適用は直前の状態を書き換えない", () => {
  const snapshot = emptySnapshot();
  deepFreeze(snapshot.state);
  const result = reduceTaskWorkState(snapshot, startEvent(), new Map());
  assert.equal(snapshot.state.pendingOperations.length, 0);
  assert.equal(result.snapshot.state.pendingOperations.length, 1);
  assert.notEqual(result.snapshot.state, snapshot.state);
});

test("別 lineage の event は適用しない", () => {
  assert.throws(
    () => reduceTaskWorkState(emptySnapshot(), startEvent({ taskLineageId: "lineage-2" }), new Map()),
    /別 lineage の event は適用しない/,
  );
  // lineage を持たない event は、還元先の状態の lineage に属するものとして扱う
  const result = reduceTaskWorkState(emptySnapshot(), startEvent({ taskLineageId: undefined }), new Map());
  assert.equal(result.snapshot.state.pendingOperations[0]?.correlation.taskLineageId, "lineage-1");
});

test("ledger を取り違えても同じ start が二重に pending へ入らない", () => {
  // 台帳と状態がずれた（復元・移送）ときの最後の砦。operationId が同じなら追加しない
  const first = reduceTaskWorkState(emptySnapshot(), startEvent(), new Map());
  const again = reduceTaskWorkState(first.snapshot, startEvent(), new Map());
  assert.equal(again.snapshot.state.pendingOperations.length, 1);
  assert.deepEqual(
    again.diagnostics.map((d) => d.code),
    ["duplicate_operation_start"],
  );
});

// --- §3.1 operation envelope ------------------------------------------------

test("operation event に envelope が無ければ schema violation", () => {
  assert.throws(
    () => assertOperationEnvelope(startEvent({ operation: undefined })),
    /operation envelope が無い/,
  );
  assert.throws(
    () => reduceTaskWorkState(emptySnapshot(), startEvent({ operation: undefined }), new Map()),
    /operation envelope が無い/,
  );
});

test("kind と envelope の phase がずれていれば schema violation", () => {
  assert.throws(
    () => assertOperationEnvelope(startEvent({ operation: TERMINAL_OPERATION })),
    /phase は start だが envelope は terminal/,
  );
});

test("operation 系でない既知の kind が envelope を持てば schema violation", () => {
  assert.throws(
    () => assertOperationEnvelope(startEvent({ kind: "session_started" })),
    /operation 系でない kind/,
  );
});

test("envelope を要求しない kind は素通りする", () => {
  // 締めすぎの確認: 非 operation kind（envelope 無し）と、語彙外の adapter 固有 kind は通る
  assertOperationEnvelope(startEvent({ kind: "session_started", operation: undefined }));
  assertOperationEnvelope(startEvent({ kind: "adapter_specific_kind" }));
  assertOperationEnvelope(startEvent({ kind: "adapter_specific_kind", operation: undefined }));
});

test("envelope の必須値が空なら schema violation", () => {
  assert.throws(
    () => assertOperationEnvelope(startEvent({ operation: { ...START_OPERATION, operationKind: "" } })),
    /operationMatchKey \/ operationKind が空/,
  );
});

// --- §3.1 turn identity -----------------------------------------------------

test("envelope の任意欄が空文字なら schema violation", () => {
  // schema は maxLength しか持たないので空文字が届く。空文字を「値がある」と読むと、rule 1 が
  // native ID を持たない operation 同士を全部同じものとして照合する
  assert.throws(
    () => assertOperationEnvelope(startEvent({ operation: { ...START_OPERATION, nativeOperationId: "" } })),
    /nativeOperationId \/ canonicalInputHash が空文字/,
  );
  assert.throws(
    () => assertOperationEnvelope(startEvent({ operation: { ...START_OPERATION, canonicalInputHash: "" } })),
    /nativeOperationId \/ canonicalInputHash が空文字/,
  );
  // 欄そのものが無いのは正しい形
  assertOperationEnvelope(
    startEvent({ operation: { ...START_OPERATION, nativeOperationId: undefined, canonicalInputHash: undefined } }),
  );
});

test("turnIdSource と turnId の有無が食い違えば schema violation", () => {
  assert.throws(
    () => assertTurnIdentity(startEvent({ turnId: undefined })),
    /turnIdSource が native なのに turnId が無い/,
  );
  assert.throws(
    () => assertTurnIdentity(startEvent({ turnIdSource: "unavailable" })),
    /unavailable なのに turnId がある/,
  );
  assert.throws(
    () => reduceTaskWorkState(emptySnapshot(), startEvent({ turnId: undefined }), new Map()),
    /turnId が無い/,
  );
});

test("turn 同一性の 3 通りの正しい組み合わせは通る", () => {
  assertTurnIdentity(startEvent());
  assertTurnIdentity(startEvent({ turnIdSource: "synthesized_monotonic", turnId: "turn-7" }));
  assertTurnIdentity(startEvent({ turnIdSource: "unavailable", turnId: undefined }));
});

// --- §3.1 intake ------------------------------------------------------------

test("native の条件を満たす event は native のまま", () => {
  assert.equal(stampIntakeEvidence(startEvent(), INTAKE).event.provenance.evidenceKind, "native");
});

test("受領証の attestedAt は書く層（intake）で暦検査を受ける", () => {
  // この値は caller 由来ではない（caller の受領証は destructure で捨てられ、daemon 自身の
  // `context.attestation` が刻印される）。書く層が検査しないと、受領証の時刻を 1 つ間違えた
  // daemon は全 event を落としながら、エラーは受領証ではなく event を名指しする
  const broken: IntakeContextV1 = {
    ...INTAKE,
    attestation: { ...ATTESTATION, attestedAt: "2026-02-30T00:00:00Z" },
  };
  assert.throws(
    () => stampIntakeEvidence(startEvent(), broken),
    /§22\.6 違反: 受領証の attestedAt が暦として実在しない/,
  );
  // 正当な受領証が通ることは直上の test（native の条件を満たす event は native のまま）が見る。
  // **空白は暦違反ではない**。この関数自身が「認証できない経路を欄が空の受領証で表す daemon」を
  // 前提にしているので、空白を落とすとその daemon の event が 100% 消え、しかもエラーは認証の
  // 欠落ではなく timestamp を名指しする（実測でそうなっていた）。降格で扱う
  for (const blank of ["", " "]) {
    const emptyReceipt: IntakeContextV1 = {
      ...INTAKE,
      attestation: { ingestReceiptId: blank, peerIdentityId: blank, channel: "rpc", attestedAt: blank },
    };
    assert.equal(
      stampIntakeEvidence(startEvent(), emptyReceipt).event.provenance.evidenceKind,
      "synthesized",
      `空白の受領証（${JSON.stringify(blank)}）`,
    );
  }
  // ただし**時刻だけ空白の受領証**は authority にしない。上で空白を「不在」として通すように
  // したので、ここで見ないと時刻を名乗らない受領証が native authority の根拠になる
  const timeless: IntakeContextV1 = {
    ...INTAKE,
    attestation: { ...ATTESTATION, attestedAt: " " },
  };
  assert.equal(
    stampIntakeEvidence(startEvent(), timeless).event.provenance.evidenceKind,
    "synthesized",
  );
});

test("空白の attestedAt を持つ event は還元器でも暦違反にしない", () => {
  // 読む層も同じ判断にする。intake で降格した event はその受領証を持ったまま還元器へ来るので、
  // ここで落とすと降格の意味が無くなる（結局その daemon の event は 1 つも適用できない）
  const event = startEvent({
    provenance: {
      ...startEvent().provenance,
      ingestAttestation: { ingestReceiptId: "", peerIdentityId: "", channel: "rpc", attestedAt: "" },
    },
  });
  assert.equal(reduceTaskWorkState(emptySnapshot(), event, new Map()).outcome, "applied");
});

test("native の条件を 1 つでも欠けば synthesized へ落ちる", () => {
  const cases: Array<[string, NormalizedContinuityEvent]> = [

    [
      "capabilityHash が active と違う",
      startEvent({ provenance: { ...startEvent().provenance, capabilityHash: "0".repeat(64) } }),
    ],
    [
      "scenarioId が proven でない",
      startEvent({ provenance: { ...startEvent().provenance, scenarioId: "not-proven" } }),
    ],
    [
      "captureMethod が proven の組と違う",
      startEvent({ provenance: { ...startEvent().provenance, captureMethod: "hook" } }),
    ],
    [
      "sourceAgentVersion が exact でない",
      startEvent({ provenance: { ...startEvent().provenance, sourceAgentVersion: "2.1.228" } }),
    ],
    // `sourceAgent` の食い違いはここには無い。降格ではなく intake が受け取らないため
    // （scope selector なので降格では縛れない。→「認証済み peer と違う Agent 名を…」）
  ];
  for (const [label, event] of cases) {
    assert.equal(stampIntakeEvidence(event, INTAKE).event.provenance.evidenceKind, "synthesized", label);
  }
  // 認証されていない経路（受領証を出せない）でも native にならない
  const unauthenticated: IntakeContextV1 = { ...INTAKE, attestation: undefined };
  assert.equal(stampIntakeEvidence(startEvent(), unauthenticated).event.provenance.evidenceKind, "synthesized");
  // channel は intake の受領証側の値で判定する。proven の組に無い channel なら native にならない
  const spool: IntakeContextV1 = { ...INTAKE, attestation: { ...ATTESTATION, channel: "spool" } };
  assert.equal(stampIntakeEvidence(startEvent(), spool).event.provenance.evidenceKind, "synthesized");
});

test("caller の ingestAttestation は読まずに intake の受領証で置き換える", () => {
  const forged = startEvent({
    provenance: {
      ...startEvent().provenance,
      ingestAttestation: {
        ingestReceiptId: "forged-receipt",
        peerIdentityId: "attacker",
        channel: "rpc",
        attestedAt: "2026-08-16T00:00:01Z",
      },
    },
  });
  // 認証済み経路: 受領証は intake のものに差し替わる
  const stamped = stampIntakeEvidence(forged, INTAKE).event;
  assert.deepEqual(stamped.provenance.ingestAttestation, ATTESTATION);
  // 認証されていない経路: 名乗った受領証ごと落ちる
  const unauthenticated = stampIntakeEvidence(forged, { ...INTAKE, attestation: undefined }).event;
  assert.equal(unauthenticated.provenance.ingestAttestation, undefined);
  assert.equal(unauthenticated.provenance.evidenceKind, "synthesized");
});

test("capability matrix が無い daemon では native を与えない", () => {
  // 空文字同士は「一致」ではない。matrix が未整備（activeCapabilityHash が空）の daemon で
  // caller も空を名乗ると、§3.1 の「active exact-version capability matrix hash と等しいこと」を
  // 満たしていないのに native が成立してしまう
  const result = stampIntakeEvidence(
    startEvent({
      provenance: { ...startEvent().provenance, capabilityHash: "" },
    }),
    { ...INTAKE, activeCapabilityHash: "" },
  );
  assert.equal(result.event.provenance.evidenceKind, "synthesized");
});

test("§3.1 の必須 negative: capability hash を写した native 主張は hook/spool 経路で synthesized になる", () => {
  const forged = startEvent({
    provenance: {
      ...startEvent().provenance,
      // caller は native を主張し、正しい capability hash と proven な scenarioId を写している
      evidenceKind: "native",
      captureMethod: "hook",
      ingestAttestation: { ...startEvent().provenance.ingestAttestation!, channel: "spool" },
    },
  });
  assert.equal(stampIntakeEvidence(forged, INTAKE).event.provenance.evidenceKind, "synthesized");
});

// --- §4.3 terminal correlation ---------------------------------------------

function startedSnapshot(event = startEvent()): TaskWorkStateSnapshotV1 {
  return reduceTaskWorkState(emptySnapshot(), event, new Map()).snapshot;
}

/**
 * 順序材料（#35 の `startIngestSeq` / `startTurnIdSource`）を持たない状態。この版より前に
 * 書かれた checkpoint と、2 欄を書かない別実装の状態がこの形になる。どちらも凍結 schema 的には
 * 妥当なので、還元器は「材料が無い」経路を保ち続ける必要がある。
 */
function withoutStartFacts(snapshot: TaskWorkStateSnapshotV1): TaskWorkStateSnapshotV1 {
  return {
    ...snapshot,
    state: {
      ...snapshot.state,
      pendingOperations: snapshot.state.pendingOperations.map(
        ({ startIngestSeq: _seq, startTurnIdSource: _source, ...rest }) => rest,
      ),
    },
  };
}

// --- #35: 権威順序と turn 種別の材料を要素に載せる --------------------------

test("start を受理すると順序材料が PendingOperation に書かれる（#35 FR-001/FR-002）", () => {
  const started = startedSnapshot().state.pendingOperations[0];
  assert.equal(started?.startIngestSeq, "11");
  assert.equal(started?.startTurnIdSource, "native");
});

test("再配送 start は順序材料を書き換えない（#35 FR-003）", () => {
  // 遅れて届いた再配送が運ぶのは**再配送時の取り込み位置**であって元の start の権威順序ではない。
  // 上書きすると、低い値を名乗る偽 start で順序違反の terminal を通せる。逆に高い値で上書き
  // されると、正当な terminal が `terminal_out_of_order` で落ちて `unknown` に倒れる
  const first = startedSnapshot();
  const resent = startEvent({ adapterDeliveryId: "delivery-start-resent", ingestSeq: "99" });
  const again = reduceTaskWorkState(first, resent, new Map());
  assert.equal(again.outcome, "applied");
  const pending = again.snapshot.state.pendingOperations[0];
  assert.equal(pending?.startIngestSeq, "11");
  assert.equal(pending?.startTurnIdSource, "native");

  // 再配送の後でも、元の start より後の terminal は普通に閉じる
  const closed = reduceTaskWorkState(again.snapshot, terminalEvent({ ingestSeq: "12" }), new Map());
  assert.equal(closed.snapshot.state.pendingOperations[0]?.status, "succeeded");
  assert.deepEqual(closed.diagnostics, []);
});

test("順序材料を持たない pending は terminal を閉じずに unknown へ倒す（#35 FR-004）", () => {
  // この版より前に書かれた checkpoint と、別実装が書いた状態。材料が無いことを
  // 「順序を確認できた」と読み替えると、start より前の terminal が診断ゼロで通る
  const restored = withoutStartFacts(startedSnapshot());
  const correlation = correlateTerminalEvent(restored, terminalEvent());
  assert.equal(correlation.matched, null);
  if (correlation.matched !== null) return;
  assert.equal(correlation.diagnostic, "terminal_order_unverifiable");

  // 隔離ではなく unknown で閉じる。隔離は配送鍵を消費せず還元器は純関数なので、
  // 復元直後の全 terminal が永久に再送され続ける
  const closed = reduceTaskWorkState(restored, terminalEvent(), new Map());
  assert.equal(closed.outcome, "applied");
  assert.equal(closed.snapshot.state.pendingOperations[0]?.status, "unknown");
});

test("空白の順序材料は「無い」として読む（#35 FR-004）", () => {
  // JSON を経由した状態では欄が空文字で来うる。素の `!== undefined` で見ると空文字が
  // 材料として通り、`compareIngestSeq` が空文字を相手に順序を判定する
  const started = startedSnapshot();
  const blanked = started.state.pendingOperations.map((pending) => ({
    ...pending,
    startIngestSeq: "  ",
    startTurnIdSource: "" as PendingOperation["startTurnIdSource"],
  }));
  const restored: TaskWorkStateSnapshotV1 = {
    ...started,
    state: { ...started.state, pendingOperations: blanked },
  };
  const correlation = correlateTerminalEvent(restored, terminalEvent());
  assert.equal(correlation.matched, null);
  if (correlation.matched !== null) return;
  assert.equal(correlation.diagnostic, "terminal_order_unverifiable");
});

test("語彙の外の startIngestSeq は還元器を落とさず「無い」に倒れる（#35 FR-004）", () => {
  // `startIngestSeq` は復元した状態と別実装が書ける任意欄で、schema の pattern を通っている
  // 保証は無い。素で `compareIngestSeq` に渡すと throw し、**壊れた要素を狙っていない terminal
  // まで巻き添えで落ちる**（候補集合を走査するため）。空白と同じく「名乗っていない」に倒す
  const started = startedSnapshot();
  for (const malformed of ["007", "-1", "1 ", "12a"]) {
    const restored: TaskWorkStateSnapshotV1 = {
      ...started,
      state: {
        ...started.state,
        pendingOperations: started.state.pendingOperations.map((pending) => ({
          ...pending,
          startIngestSeq: malformed,
        })),
      },
    };
    const correlation = correlateTerminalEvent(restored, terminalEvent());
    assert.equal(correlation.matched, null, malformed);
    if (correlation.matched !== null) return;
    assert.equal(correlation.diagnostic, "terminal_order_unverifiable", malformed);
    // 還元器まで通しても throw しない（隔離ではなく unknown で台帳へ入る）
    const reduced = reduceTaskWorkState(restored, terminalEvent(), new Map());
    assert.equal(reduced.outcome, "applied", malformed);
    assert.equal(reduced.snapshot.state.pendingOperations[0]?.status, "unknown", malformed);
  }
});

test("turn 種別の材料が無い rule 2 は閉じずに unknown へ倒れる（#35 FR-004）", () => {
  // `eligibleOf` は材料が無い候補を落とさない（落とすと帰属を取り違える）。落とさないことと
  // **合格にすること**は別で、後者にすると「材料が無い＝検査を素通り」になる。この test は
  // 2 方向を固定する: 材料が無ければ理由つきで unknown、あって一致すれば普通に閉じる。
  //
  // 空白は「名乗っていない」であって値ではない。値として読むと `eligibleOf` が `"" !== "native"`
  // で候補ごと落とし、`terminal_unmatched`（何が起きたか名指ししない診断）に化けるので、
  // 欠落と空白は同じ `terminal_turn_unverifiable` に倒れなければならない
  const start = startEvent({ operation: MATCH_KEY_ONLY });
  const base = startedSnapshot(start);
  const terminal = terminalEvent({ operation: { ...TERMINAL_OPERATION, nativeOperationId: undefined } });
  // 順序は確認できる状態にしたまま、種別だけを欠落・空白にする
  for (const label of ["欠落", "空白"] as const) {
    const restored: TaskWorkStateSnapshotV1 = {
      ...base,
      state: {
        ...base.state,
        // 欠落は**欄そのものを消す**（`undefined` を値として置くと JCS が canonicalize できない）
        pendingOperations: base.state.pendingOperations.map(({ startTurnIdSource: _source, ...rest }) =>
          label === "欠落" ? rest : { ...rest, startTurnIdSource: " " as PendingOperation["startTurnIdSource"] },
        ),
      },
    };
    const correlation = correlateTerminalEvent(restored, terminal);
    assert.equal(correlation.matched, null, label);
    if (correlation.matched !== null) return;
    assert.equal(correlation.diagnostic, "terminal_turn_unverifiable", label);
    // 隔離ではなく unknown で台帳へ入る（隔離すると復元した状態は二度と閉じられない）
    const reduced = reduceTaskWorkState(restored, terminal, new Map());
    assert.equal(reduced.outcome, "applied", label);
    assert.deepEqual(reduced.diagnostics.map((d) => d.code), ["terminal_turn_unverifiable"], label);
    assert.equal(reduced.snapshot.state.pendingOperations[0]?.status, "unknown", label);
  }
  // 逆向き 1: 材料があって terminal と一致するなら、この検査は何も邪魔しない
  const closed = reduceTaskWorkState(base, terminal, new Map());
  assert.deepEqual(closed.diagnostics.map((d) => d.code), []);
  assert.equal(closed.snapshot.state.pendingOperations[0]?.status, "succeeded");
  // 逆向き 2: rule 1 は turn 両立を要求しない（§4.3）ので、種別が無くても止めてはいけない。
  // ここを固定しないと「材料が無ければ常に降格」に締めすぎた実装が緑のまま通る
  const native = startedSnapshot();
  const nativeRestored: TaskWorkStateSnapshotV1 = {
    ...native,
    state: {
      ...native.state,
      pendingOperations: native.state.pendingOperations.map(({ startTurnIdSource: _source, ...rest }) => rest),
    },
  };
  const byNativeId = reduceTaskWorkState(nativeRestored, terminalEvent(), new Map());
  assert.deepEqual(byNativeId.diagnostics.map((d) => d.code), []);
  assert.equal(byNativeId.snapshot.state.pendingOperations[0]?.status, "succeeded");
});

test("復元状態の欠けた turn 種別を terminal 側の主張で埋めない（#35 FR-004・信頼境界）", () => {
  // 上の test と同じ穴を、**悪用の形**で固定する。schema 通りの復元状態が `startIngestSeq` だけを
  // 持つとき（順序は確認できる）、`nativeOperationId` を名乗らない terminal が
  // `turnIdSource: "synthesized_monotonic"` を主張すると、照合の材料は terminal 側にしか無い。
  // 状態が種別を持たないので「一致した」とは言えないのに、修正前は診断ゼロで `succeeded` が
  // 確定した（実測）。wire が運ぶ値を権威にしない = 欠けた材料は terminal の主張で埋めない
  const base = startedSnapshot(startEvent({ operation: MATCH_KEY_ONLY }));
  const restored: TaskWorkStateSnapshotV1 = {
    ...base,
    state: {
      ...base.state,
      pendingOperations: base.state.pendingOperations.map(({ startTurnIdSource: _source, ...rest }) => ({
        ...rest,
        startIngestSeq: "9007199254740990",
      })),
    },
  };
  const forged = terminalEvent({
    operation: { ...TERMINAL_OPERATION, nativeOperationId: undefined },
    ingestSeq: "9007199254740991",
    turnIdSource: "synthesized_monotonic",
  });
  const reduced = reduceTaskWorkState(restored, forged, new Map());
  assert.equal(reduced.snapshot.state.pendingOperations[0]?.status, "unknown");
  assert.ok(reduced.diagnostics.some((d) => d.code === "terminal_turn_unverifiable"));
});

test("材料が要素に載れば、同じ id の兄弟でも取り違えない（#35 SC-001）", () => {
  // 側索引の鍵は `operationId` だったので、id が衝突する状態では**どちらの兄弟の材料かを
  // 原理的に判別できず**、材料なしに倒すしか無かった（実測: 兄弟 B の ingestSeq 100 を使って
  // A 宛ての terminal が通り、A が診断ゼロで succeeded になった）。要素に載せると鍵が要らない
  const shared = { ...START_OPERATION, nativeOperationId: undefined } as const;
  const base = startedSnapshot(startEvent({ operation: shared }));
  const twin = base.state.pendingOperations[0] as PendingOperation;
  const snapshot: TaskWorkStateSnapshotV1 = {
    ...base,
    state: {
      ...base.state,
      pendingOperations: [
        { ...twin, startIngestSeq: "100" },
        { ...twin, operationId: `${twin.operationId}`, startIngestSeq: "11" },
      ],
    },
  };
  // ingestSeq 50 は「100 の側は閉じえない / 11 の側は閉じえる」。候補が 1 件に絞れるので
  // ambiguous にもならず、正しい側が閉じる
  const terminal = terminalEvent({
    ingestSeq: "50",
    operation: { phase: "terminal", operationMatchKey: shared.operationMatchKey, operationKind: shared.operationKind },
  });
  const correlation = correlateTerminalEvent(snapshot, terminal);
  if (correlation.matched === null) assert.fail(`照合されなかった: ${correlation.detail}`);
  assert.equal(correlation.matched.startIngestSeq, "11");
});

test("nativeOperationId 一致で terminal が閉じる", () => {
  const snapshot = startedSnapshot();
  const correlation = correlateTerminalEvent(snapshot, terminalEvent());
  if (correlation.matched === null) assert.fail(`照合されなかった: ${correlation.detail}`);
  assert.equal(correlation.rule, "native_operation_id");

  const closed = reduceTaskWorkState(snapshot, terminalEvent(), new Map());
  assert.equal(closed.snapshot.state.pendingOperations[0]?.status, "succeeded");
  assert.equal(closed.snapshot.state.pendingOperations[0]?.terminalAt, "2026-08-16T00:00:02Z");
  assert.deepEqual(closed.diagnostics, []);
});

test("nativeOperationId が無ければ operationMatchKey + turn で閉じる", () => {
  const start = startEvent({ operation: { phase: "start", operationMatchKey: "match-key-1", operationKind: "Bash" } });
  const snapshot = startedSnapshot(start);
  const terminal = terminalEvent({
    operation: { phase: "terminal", operationMatchKey: "match-key-1", operationKind: "Bash" },
  });
  const correlation = correlateTerminalEvent(snapshot, terminal);
  if (correlation.matched === null) assert.fail(`照合されなかった: ${correlation.detail}`);
  assert.equal(correlation.rule, "match_key");
});

test("turn が確立していない terminal は rule 2 で閉じない", () => {
  const start = startEvent({
    turnId: undefined,
    turnIdSource: "unavailable",
    operation: { phase: "start", operationMatchKey: "match-key-1", operationKind: "Bash" },
  });
  const snapshot = startedSnapshot(start);
  const terminal = terminalEvent({
    turnId: undefined,
    turnIdSource: "unavailable",
    operation: { phase: "terminal", operationMatchKey: "match-key-1", operationKind: "Bash" },
  });
  const correlation = correlateTerminalEvent(snapshot, terminal);
  assert.equal(correlation.matched, null);
});

test("open な候補が複数ある matchKey 一致は閉じない", () => {
  const first = startEvent({
    operation: { phase: "start", operationMatchKey: "match-key-1", operationKind: "Bash" },
  });
  const second = startEvent({
    eventId: "event-start-2",
    adapterDeliveryId: "delivery-start-2",
    ingestSeq: "13",
    operation: { phase: "start", operationMatchKey: "match-key-1", operationKind: "Bash" },
  });
  const { snapshot } = apply(emptySnapshot(), [first, second]);
  assert.equal(snapshot.state.pendingOperations.length, 2);
  const terminal = terminalEvent({
    ingestSeq: "14",
    operation: { phase: "terminal", operationMatchKey: "match-key-1", operationKind: "Bash" },
  });
  const result = reduceTaskWorkState(snapshot, terminal, new Map());
  assert.deepEqual(
    result.diagnostics.map((d) => d.code),
    ["terminal_ambiguous"],
  );
  assert.deepEqual(
    result.snapshot.state.pendingOperations.map((p) => p.status),
    ["unknown", "unknown"],
  );
});

test("session が違う terminal は閉じない", () => {
  const snapshot = startedSnapshot();
  const result = reduceTaskWorkState(snapshot, terminalEvent({ sessionId: "session-2" }), new Map());
  assert.equal(result.outcome, "quarantined");
  assert.deepEqual(
    result.diagnostics.map((d) => d.code),
    ["terminal_orphaned", "dropped_evidence_recorded"],
  );
  assert.equal(result.snapshot.state.pendingOperations[0]?.status, "started");
});

test("権威順序で start より前の terminal は閉じず、候補を unknown にする", () => {
  const snapshot = startedSnapshot();
  const result = reduceTaskWorkState(snapshot, terminalEvent({ ingestSeq: "11" }), new Map());
  assert.deepEqual(
    result.diagnostics.map((d) => d.code),
    ["terminal_out_of_order"],
  );
  // terminal 証跡が来ている以上「まだ走っている」とは言えない。§4.3 の「閉じられない terminal は
  // 候補を unknown にする」に倒す（started のままにすると実行中だと主張することになる）
  assert.equal(result.snapshot.state.pendingOperations[0]?.status, "unknown");
});

test("turn 同一性が無い terminal は閉じず、候補を unknown にする", () => {
  // §4.3「どちらかが unavailable のとき rule 2 は適用されず、operation は unknown のままになる」
  const snapshot = startedSnapshot();
  const terminal = terminalEvent({
    turnIdSource: "unavailable",
    turnId: undefined,
    operation: { ...TERMINAL_OPERATION, nativeOperationId: undefined },
  });
  const result = reduceTaskWorkState(snapshot, terminal, new Map());
  assert.deepEqual(
    result.diagnostics.map((d) => d.code),
    ["terminal_unmatched"],
  );
  assert.equal(result.snapshot.state.pendingOperations[0]?.status, "unknown");
});

test("turn 同一性が無くても rule 1 なら閉じられる", () => {
  // 上と同じ event で nativeOperationId だけ残す。unknown 化が turn 不一致に効いていることを、
  // 「閉じられる側」でも確かめる（両方 unknown になるなら gate の意味が無い）
  const snapshot = startedSnapshot();
  const result = reduceTaskWorkState(
    snapshot,
    terminalEvent({ turnIdSource: "unavailable", turnId: undefined }),
    new Map(),
  );
  assert.deepEqual(result.diagnostics, []);
  assert.equal(result.snapshot.state.pendingOperations[0]?.status, "succeeded");
});

/** pendingOperations を schema 上限まで埋めた状態。`resolveFirst` で先頭だけ terminal 済みにする。 */
function filledSnapshot(resolveFirst: boolean): TaskWorkStateSnapshotV1 {
  const template = startedSnapshot().state.pendingOperations[0] as PendingOperation;
  const pendingOperations = Array.from({ length: CONTINUITY_LIMITS.arrayItems }, (_, index) => ({
    ...template,
    operationId: `op-${index}`,
    // nativeOperationId は本物の呼び出しごとに一意（同じ値を並べると再配送に見える）
    correlation: { ...template.correlation, operationId: `op-${index}`, nativeOperationId: `toolu_filled_${index}` },
    status: resolveFirst && index === 0 ? ("succeeded" as const) : ("started" as const),
  }));
  return emptySnapshot({ pendingOperations });
}

test("pendingOperations が上限のとき terminal 済みを落として新しい start を入れる", () => {
  const snapshot = filledSnapshot(true);
  const result = reduceTaskWorkState(snapshot, startEvent(), new Map());
  assert.equal(result.outcome, "applied");
  assert.equal(result.snapshot.state.pendingOperations.length, CONTINUITY_LIMITS.arrayItems);
  // 落ちたのは terminal 済みの op-0 だけ
  assert.equal(
    result.snapshot.state.pendingOperations.some((p) => p.operationId === "op-0"),
    false,
  );
  // 黙って落とさない
  assert.deepEqual(
    result.diagnostics.map((d) => d.code),
    ["pending_operations_evicted", "dropped_evidence_recorded"],
  );
  assert.match(result.diagnostics[0]?.detail ?? "", /op-0/);
});

test("同じ群の退避順は配列位置で決まる（startedAt では決めない）", () => {
  // `startedAt` は adapter が寄越した `occurredAt` の写しなので到着順と一致せず、event を出す側が
  // 動かせる。ここで startedAt 順に落とす実装に替えると、同じ event 列から別の状態・別の
  // stateRevision・別の hash が出る。配列位置が権威であることを固定する。
  const filled = filledSnapshot(false);
  const reversed = filled.state.pendingOperations.map((pending, index) => ({
    ...pending,
    status: "succeeded" as const,
    // 配列の先頭ほど新しい startedAt。startedAt 順に落とすなら末尾の op-255 が最初に落ちる
    startedAt: `2026-08-16T00:${String(59 - Math.floor(index / 60)).padStart(2, "0")}:${String(59 - (index % 60)).padStart(2, "0")}Z`,
  }));
  const result = reduceTaskWorkState(emptySnapshot({ pendingOperations: reversed }), startEvent(), new Map());
  assert.equal(result.outcome, "applied");
  const surviving = new Set(result.snapshot.state.pendingOperations.map((p) => p.operationId));
  assert.equal(surviving.has("op-0"), false);
  assert.equal(surviving.has(`op-${CONTINUITY_LIMITS.arrayItems - 1}`), true);
});

test("退避した operation の順序材料も落ちる（#35）", () => {
  // 側索引だった頃は、pendingOperations が 256 件で頭打ちの一方で表だけが単調増加しないよう、
  // 退避のたびに鍵を消す同期が要った。材料を要素に載せた今は operation と同じ寿命になるので、
  // 同期は不要になった代わりに「退避された側の材料が残っていない」ことは依然として要件
  // （残ると生存側の順序検査が他人の材料で通りうる）
  const snapshot = filledSnapshot(true);
  const seeded: TaskWorkStateSnapshotV1 = {
    ...snapshot,
    state: {
      ...snapshot.state,
      pendingOperations: snapshot.state.pendingOperations.map((pending) => ({
        ...pending,
        startIngestSeq: "1",
        startTurnIdSource: "native" as const,
      })),
    },
  };
  const result = reduceTaskWorkState(seeded, startEvent(), new Map());
  const survivors = result.snapshot.state.pendingOperations;
  assert.equal(survivors.length, CONTINUITY_LIMITS.arrayItems);
  assert.equal(
    survivors.some((pending) => pending.operationId === "op-0"),
    false,
  );
});

test("rule 1 の nativeOperationId に複数当たるなら、どれも閉じない", () => {
  // 凍結 schema は `nativeOperationId` にも一意性を課さないので、復元した checkpoint には
  // 同じ native id の確定済みと live が並びうる。§4.3 の rule 1 は「exact nativeOperationId +
  // 同じ session/lineage」で operation を一意に指す規則なので、2 件当たったら指せていない。
  // 件数を見ずに open だけで選ぶと、確定済み operation 宛ての再配送が live な兄弟を閉じる
  const pending = (operationId: string, status: "started" | "succeeded"): PendingOperation =>
    ({
      operationId,
      correlation: {
        operationId, startEventId: `s-${operationId}`, nativeOperationId: START_OPERATION.nativeOperationId,
        operationMatchKey: START_OPERATION.operationMatchKey, sessionId: "session-1",
        taskLineageId: "lineage-1", turnId: "turn-1", toolName: "Bash",
        canonicalInputHash: START_OPERATION.canonicalInputHash,
      },
      kind: "tool", description: "Bash", status, replayPolicy: "never_auto",
      sourceEventIds: [`s-${operationId}`], startedAt: "2026-08-16T00:00:01Z", sensitivity: "normal",
      // 順序が確認できる = 閉じられる状態にしておく
      startIngestSeq: "1", startTurnIdSource: "native",
    }) as unknown as PendingOperation;
  const snapshotOf = (...operations: PendingOperation[]): TaskWorkStateSnapshotV1 => ({
    state: emptyState({ pendingOperations: operations }),
    history: [],
  });
  const mixed = reduceTaskWorkState(
    snapshotOf(pending("op-settled", "succeeded"), pending("op-live", "started")),
    terminalEvent(),
    new Map(),
  );
  assert.deepEqual(mixed.diagnostics.map((d) => d.code), ["terminal_ambiguous"]);
  // live 側が「閉じられた」ことにならない。§4.3 どおり candidates は unknown まで
  assert.deepEqual(
    mixed.snapshot.state.pendingOperations.map((p) => `${p.operationId}:${p.status}`),
    ["op-settled:succeeded", "op-live:unknown"],
  );
  // 通す側も測る: 全件確定済みなら従来どおり「適用済み」で、曖昧扱いに変わらない
  const settled = reduceTaskWorkState(
    snapshotOf(pending("op-a", "succeeded"), pending("op-b", "succeeded")),
    terminalEvent(),
    new Map(),
  );
  assert.deepEqual(settled.diagnostics.map((d) => d.code), ["terminal_already_applied"]);
});

/** 状態の pending を全部よその lineage のものにする（凍結 schema は state との一致を課さない）。 */
function foreignLineage(snapshot: TaskWorkStateSnapshotV1): TaskWorkStateSnapshotV1 {
  return {
    ...snapshot,
    state: {
      ...snapshot.state,
      pendingOperations: snapshot.state.pendingOperations.map((pending) => ({
        ...pending,
        correlation: { ...pending.correlation, taskLineageId: "lineage-other" },
      })),
    },
  };
}

test("別 lineage の pending を再配送の相手にしない", () => {
  // `operationId` は eventId + matchKey からの導出で lineage を含まないので、復元した checkpoint に
  // 同じ derived id の別 lineage の pending が居ると、現 lineage 宛ての start がそれを「自分の
  // 再配送」と見なして重複になり、鍵を消費したまま現 lineage には operation が 1 件も残らない
  const applied = reduceTaskWorkState(
    foreignLineage(startedSnapshot()),
    startEvent(),
    new Map(),
  );
  assert.equal(applied.outcome, "applied");
  assert.deepEqual(applied.diagnostics.map((d) => d.code), []);
  const mine = applied.snapshot.state.pendingOperations.filter(
    (pending) => pending.correlation.taskLineageId === applied.snapshot.state.taskLineageId,
  );
  assert.equal(mine.length, 1);
  assert.equal(mine[0]?.status, "started");
});

test("放棄は自 lineage の operation だけを unknown にする", () => {
  // `sourceEventIds` は append-only なので、巻き込むと別 lineage の記録にこの session の
  // `session_ended` が永久に残る。§4.3 の「候補の外へ広げない」と同じ理由
  const abandon = startEvent({
    eventId: "event-abandon",
    adapterDeliveryId: "delivery-abandon",
    kind: "session_ended",
    ingestSeq: "12",
    operation: undefined,
  });
  const finalized = finalizeAbandonedState(foreignLineage(startedSnapshot()).state, abandon, new Map());
  assert.equal(finalized.state.pendingOperations[0]?.status, "started");
  assert.deepEqual(finalized.state.pendingOperations[0]?.sourceEventIds, ["event-start"]);
  // 自 lineage なら従来どおり倒れる（締めすぎていないことを通る側でも固定する）
  const own = finalizeAbandonedState(startedSnapshot().state, abandon, new Map());
  assert.equal(own.state.pendingOperations[0]?.status, "unknown");
});

test("再配送が持つ識別材料は、鍵を消費する前に記録へ埋める", () => {
  // 凍結 schema は `nativeOperationId` を required にしていないので、復元した checkpoint では
  // 欠けうる。欠けたまま重複として鍵だけ消費すると、その native id を名乗る terminal は rule 1 で
  // 候補ゼロになり `terminal_orphaned` の隔離を繰り返して operation が `started` のまま止まる
  const seeded = startedSnapshot(startEvent({ operation: MATCH_KEY_ONLY }));
  assert.equal(seeded.state.pendingOperations[0]?.correlation.nativeOperationId, undefined);
  const redelivered = reduceTaskWorkState(seeded, startEvent({ ingestSeq: "13" }), new Map());
  assert.deepEqual(redelivered.diagnostics.map((d) => d.code), ["duplicate_operation_start"]);
  assert.equal(redelivered.snapshot.state.pendingOperations[0]?.correlation.nativeOperationId, "toolu_1");
  // 埋まったので rule 1 の terminal が閉じられる
  const closed = reduceTaskWorkState(redelivered.snapshot, terminalEvent(), new Map());
  assert.deepEqual(closed.diagnostics.map((d) => d.code), []);
  assert.equal(closed.snapshot.state.pendingOperations[0]?.status, "succeeded");
});

test("turn scoped な欄は再配送で埋めない", () => {
  // `turnId` / `turnIdSource` は turn scoped で、再配送は元の start と違う turn で届きうる。
  // 記録が turn 同一性を持たないときに再配送側の turn を書くと、**元の start に無かった turn で
  // rule 2 の照合権限を与える**ことになる。欠落を埋めるのと違い、これは記録の意味を変える
  const unproven: IntakeContextV1 = { ...INTAKE, nativeTurnIdentityProven: false };
  const downgraded = stampIntakeEvidence(startEvent({ operation: MATCH_KEY_ONLY }), unproven).event;
  const seeded = startedSnapshot(downgraded);
  assert.equal(seeded.state.pendingOperations[0]?.startTurnIdSource, "unavailable");
  const redelivered = reduceTaskWorkState(
    seeded,
    startEvent({ operation: MATCH_KEY_ONLY, ingestSeq: "13" }),
    new Map(),
  );
  assert.deepEqual(redelivered.diagnostics.map((d) => d.code), ["duplicate_operation_start"]);
  assert.equal(redelivered.snapshot.state.pendingOperations[0]?.startTurnIdSource, "unavailable");
  assert.equal(redelivered.snapshot.state.pendingOperations[0]?.correlation.turnId, undefined);
});

test("start の順序で候補を絞ってから曖昧さを数える", () => {
  // start が 10 と 20 の候補が並ぶとき、`ingestSeq` 15 の terminal は 10 の側しか閉じえない。
  // 順序を「1 件に決めたあとの検査」にしていると両方が候補として数えられ、`terminal_ambiguous` で
  // 両方 `unknown` に倒れて台帳まで消費される（訂正版が重複 no-op になるので隔離より悪い）
  const first = startedSnapshot(
    startEvent({ eventId: "event-a", adapterDeliveryId: "delivery-a", ingestSeq: "10", operation: MATCH_KEY_ONLY }),
  );
  const both = reduceTaskWorkState(
    first,
    startEvent({ eventId: "event-b", adapterDeliveryId: "delivery-b", ingestSeq: "20", operation: MATCH_KEY_ONLY }),
    new Map(),
  );
  const late = terminalEvent({ ingestSeq: "15", operation: { ...TERMINAL_OPERATION, nativeOperationId: undefined } });
  const closed = reduceTaskWorkState(both.snapshot, late, new Map());
  assert.deepEqual(closed.diagnostics.map((d) => d.code), []);
  assert.deepEqual(closed.snapshot.state.pendingOperations.map((p) => p.status), ["succeeded", "started"]);
  // 全件が順序不適合なら絞らない。候補 1 件の順序違反を terminal_unmatched に化けさせない
  const tooEarly = reduceTaskWorkState(
    first,
    terminalEvent({ ingestSeq: "5", operation: { ...TERMINAL_OPERATION, nativeOperationId: undefined } }),
    new Map(),
  );
  assert.deepEqual(tooEarly.diagnostics.map((d) => d.code), ["terminal_out_of_order"]);
});

// 型境界そのものを tsc で固定する。実行時 test では固定できない（helper が
// `IntakeStampedEventV1` へキャストして返すので、引数型を `NormalizedContinuityEvent` に戻しても
// 実行結果は変わらない）。引数型が広がると下の `@ts-expect-error` が「未使用の抑止」になって
// tsc が落ちる = 締めた側と緩めた側の両方向で発火する
function _correlateRejectsUnstampedEvents(raw: NormalizedContinuityEvent): void {
  // @ts-expect-error correlateTerminalEvent は intake 済みの event しか受けない
  correlateTerminalEvent(emptySnapshot(), raw);
}

test("correlateTerminalEvent は intake が降格する欄を照合権限に使う", () => {
  // 公開 API を素の `NormalizedContinuityEvent` で受けていた根拠は「correlate は `provenance` を
  // 一度も読まないので authority label を消費しない」だったが、これは誤りだった。この関数は
  // rule 2 の候補選びで `turnIdSource` を見ていて、その欄は intake が認証結果に応じて書き換える。
  // intake を飛ばすと、証明の無い native turn 主張がそのまま照合権限になる。引数型を
  // `IntakeStampedEventV1` にして型で intake の通過を要求したので、その理由を実測で残す
  const unproven: IntakeContextV1 = { ...INTAKE, nativeTurnIdentityProven: false };
  const snapshot = startedSnapshot(startEvent({ operation: MATCH_KEY_ONLY }));
  const raw = terminalEvent({ operation: { ...TERMINAL_OPERATION, nativeOperationId: undefined } });
  // intake を通す前の主張（native）は rule 2 の候補選びを通る
  assert.equal(correlateTerminalEvent(snapshot, raw).matched?.operationId,
    snapshot.state.pendingOperations[0]?.operationId);
  // 同じ event でも intake が unavailable へ降格すると、turn 両立が成り立たず閉じられない
  const stamped = stampIntakeEvidence(raw, unproven).event;
  const downgraded = correlateTerminalEvent(snapshot, stamped);
  assert.equal(downgraded.matched, null);
  assert.equal(downgraded.diagnostic, "terminal_unmatched");
});

test("同名 operationId の兄弟でも、配列順で再配送の結果が変わらない", () => {
  // 選ぶ側を native id の枝だけ直していたので、derived id の枝は先頭 1 件で決めていた。
  // 隔離は鍵を消費せず還元器は純関数なので、衝突する兄弟が先頭に来ただけで健全な再配送が
  // 永久に収束しなくなる（実測で配列順により quarantined / applied に割れた）
  const base = startedSnapshot();
  const compatible = base.state.pendingOperations[0] as PendingOperation;
  const conflicting: PendingOperation = {
    ...compatible,
    correlation: { ...compatible.correlation, sessionId: "session-other" },
  };
  for (const order of [[conflicting, compatible], [compatible, conflicting]]) {
    const snapshot: TaskWorkStateSnapshotV1 = {
      ...base,
      state: { ...base.state, pendingOperations: [...order] },
    };
    const result = reduceTaskWorkState(snapshot, startEvent(), new Map());
    assert.equal(result.outcome, "applied");
    // 飛ばした衝突兄弟は診断に出る。隔離しないのが正しいが、黙って枠を占めさせない
    assert.deepEqual(result.diagnostics.map((d) => d.code), [
      "duplicate_operation_start",
      "start_sibling_conflict",
    ]);
  }
});

test("再配送の相手は derived id と native id の集合をまたいで選ぶ", () => {
  // 集合ごとに preferCompatible を掛けて `??` で繋ぐと、preferCompatible は非空配列に必ず値を
  // 返す（互換が無ければ先頭）ので、derived id 側が非空でありさえすれば全件衝突していても
  // native id 側が評価されない。隔離は鍵を消費せず還元器は純関数なので永久に収束しない
  const base = startedSnapshot();
  const template = base.state.pendingOperations[0] as PendingOperation;
  // derived id が一致する（= 同じ eventId・matchKey）が、input hash が違うので衝突する兄弟
  const conflicting: PendingOperation = {
    ...template,
    correlation: { ...template.correlation, canonicalInputHash: "input-hash-OTHER" },
  };
  // derived id は違うが native id と identity が完全に一致する兄弟
  const compatible: PendingOperation = {
    ...template,
    operationId: "op-native-twin",
    correlation: { ...template.correlation, operationId: "op-native-twin" },
  };
  const snapshot: TaskWorkStateSnapshotV1 = {
    ...base,
    state: { ...base.state, pendingOperations: [conflicting, compatible] },
  };
  const result = reduceTaskWorkState(snapshot, startEvent(), new Map());
  assert.equal(result.outcome, "applied");
  assert.deepEqual(result.diagnostics.map((d) => d.code), [
    "duplicate_operation_start",
    "start_sibling_conflict",
  ]);
  // 埋まったのは互換な兄弟のほう。衝突する兄弟は触られないが、名指しで診断に出る
  assert.equal(
    result.diagnostics[0]?.detail?.includes("op-native-twin"),
    true,
    JSON.stringify(result.diagnostics),
  );
  assert.equal(
    result.diagnostics[1]?.detail?.includes(conflicting.operationId),
    true,
    JSON.stringify(result.diagnostics),
  );

  // 締めすぎていないことを保持側でも固定する。両集合とも全件衝突なら従来どおり隔離する。
  // **連結順が効く配置にする**: native id 側の先頭を derived id の兄弟と別要素にしておくと、
  // `[...idMatches, ...nativeMatches]` は derived 側を、`[...nativeMatches, ...idMatches]` は
  // native 側を衝突の証拠に選ぶ。両方の集合で先頭が同じ要素だと、どちらの順でも同じ答えになり
  // **連結順を入れ替える変異が生き残る**（実測で生存を確認してからこの配置に直した）
  const nativeOnlyConflicting: PendingOperation = {
    ...template,
    operationId: "op-native-only",
    correlation: {
      ...template.correlation,
      operationId: "op-native-only",
      canonicalInputHash: "input-hash-ALSO-OTHER",
    },
  };
  const bothConflict: TaskWorkStateSnapshotV1 = {
    ...snapshot,
    // 配列は native 専用の兄弟が先頭。derived id で当たるのは 2 番目の `conflicting` だけ
    state: { ...base.state, pendingOperations: [nativeOnlyConflicting, conflicting] },
  };
  const quarantined = reduceTaskWorkState(bothConflict, startEvent(), new Map());
  assert.equal(quarantined.outcome, "quarantined");
  assert.deepEqual(quarantined.diagnostics.map((d) => d.code), ["start_conflict"]);
  assert.equal(
    quarantined.diagnostics[0]?.detail?.includes(conflicting.operationId),
    true,
    `derived id 側を証拠に選んでいない: ${quarantined.diagnostics[0]?.detail}`,
  );
  assert.equal(
    quarantined.diagnostics[0]?.detail?.includes("op-native-only"),
    false,
    "native id 側を証拠に選んでいる（連結順が逆）",
  );
});

test("再配送 start も provenance を記録できなかったことを診断に出す", () => {
  // withSourceEvent は上限に達すると黙って早期 return するのに、この経路だけ
  // sourceEventsFull + truncationDiagnostic の対を付けていなかった。revision と配送鍵は
  // 消費されるので、呼び出し側からは「記録された」と区別が付かない
  const base = startedSnapshot();
  const template = base.state.pendingOperations[0] as PendingOperation;
  const full: PendingOperation = {
    ...template,
    sourceEventIds: Array.from({ length: CONTINUITY_LIMITS.arrayItems }, (_, index) => `filler-${index}`),
  };
  // eventId が変わる本来の再配送契約。derived id は変わるので native id 側で当たる
  const redelivery = startEvent({
    eventId: "event-redeliver",
    adapterDeliveryId: "delivery-redeliver",
    canonicalFingerprint: "fingerprint-redeliver",
  });
  const result = reduceTaskWorkState(
    { ...base, state: { ...base.state, pendingOperations: [full] } },
    redelivery,
    new Map(),
  );
  assert.equal(result.outcome, "applied");
  assert.deepEqual(result.diagnostics.map((d) => d.code), [
    "duplicate_operation_start",
    "source_events_truncated",
  ]);

  // 通す側: 上限未満なら truncation は出ず、eventId は実際に記録される
  const room = reduceTaskWorkState(
    base,
    redelivery,
    new Map(),
  );
  assert.deepEqual(room.diagnostics.map((d) => d.code), ["duplicate_operation_start"]);
  assert.equal(
    room.snapshot.state.pendingOperations[0]?.sourceEventIds.includes("event-redeliver"),
    true,
  );
});

test("上限退避は別 lineage の要素を自 lineage の証跡より先に落とす", () => {
  // 「上限は容量の判断なので lineage を見ない」としていたが、それだと別 lineage の要素を注入して
  // 自 lineage の確定済み証跡を押し出せた（実測: 自 lineage の succeeded 1 件 + 別 lineage の
  // started 255 件で満杯にして自 lineage の start を入れると、succeeded が退避されて別 lineage は
  // 255 件全て残った）。lineage 外は照合・放棄のどの経路からも候補にならないので、残す価値が低い
  const base = startedSnapshot();
  const template = base.state.pendingOperations[0] as PendingOperation;
  const mine: PendingOperation = { ...template, status: "succeeded" };
  const theirs = Array.from({ length: CONTINUITY_LIMITS.arrayItems - 1 }, (_, index) => ({
    ...template,
    operationId: `their-${index}`,
    status: "started" as const,
    correlation: {
      ...template.correlation,
      operationId: `their-${index}`,
      taskLineageId: "lineage-other",
      nativeOperationId: `toolu_their_${index}`,
      operationMatchKey: `mk-their-${index}`,
    },
  }));
  const full: TaskWorkStateSnapshotV1 = {
    ...base,
    state: { ...base.state, pendingOperations: [mine, ...theirs] },
  };
  const result = reduceTaskWorkState(
    full,
    startEvent({
      eventId: "event-new",
      adapterDeliveryId: "delivery-new",
      ingestSeq: "99",
      operation: { ...MATCH_KEY_ONLY, operationMatchKey: "mk-new" },
    }),
    new Map(),
  );
  const kept = result.snapshot.state.pendingOperations;
  assert.equal(kept.length, CONTINUITY_LIMITS.arrayItems);
  // 自 lineage の確定済み証跡は残り、落ちたのは別 lineage の 1 件
  assert.equal(kept.filter((p) => p.correlation.taskLineageId === base.state.taskLineageId).length, 2);
  assert.equal(kept.filter((p) => p.correlation.taskLineageId === "lineage-other").length, 254);
  assert.equal(kept.find((p) => p.status === "succeeded")?.operationId, mine.operationId);
});

test("別 lineage の双子は順序材料の帰属を曖昧にしない", () => {
  // lineage で絞ると、別 lineage の双子は再配送の相手にならないので現 lineage 側に新しい pending が
  // 積まれる。`operationId` は eventId + matchKey からの導出で lineage を含まないため、状態には
  // **同じ id の pending が 2 件**並ぶ。材料を要素に載せた今（#35）は、双子がそれぞれ自分の
  // `startIngestSeq` を持つので帰属が曖昧になりようがない。側索引だった頃は鍵が `operationId` で、
  // 「どちらの材料か判別できない」として材料なしに倒す分岐が要り、その分岐が広すぎると
  // `turnIdSource` のすり替え検査まで無効化する fail open を作っていた（下の test で固定）
  const applied = reduceTaskWorkState(foreignLineage(startedSnapshot()), startEvent(), new Map());
  assert.equal(new Set(applied.snapshot.state.pendingOperations.map((p) => p.operationId)).size, 1);
  assert.equal(applied.snapshot.state.pendingOperations.length, 2);
  const closed = reduceTaskWorkState(applied.snapshot, terminalEvent(), new Map());
  assert.deepEqual(closed.diagnostics.map((d) => d.code), []);
  const mine = closed.snapshot.state.pendingOperations.find(
    (p) => p.correlation.taskLineageId === closed.snapshot.state.taskLineageId,
  );
  assert.equal(mine?.status, "succeeded");
  // 別 lineage の双子は候補ですらないので触らない
  const theirs = closed.snapshot.state.pendingOperations.find(
    (p) => p.correlation.taskLineageId === "lineage-other",
  );
  assert.equal(theirs?.status, "started");
});

test("別 lineage の双子は turn 種別のすり替え検査を無効化しない", () => {
  // 側索引を引く関数が別 lineage の同名 pending まで数えていたとき、`recordedSource !== undefined`
  // 節が常に false になり、すり替えた再配送が隔離されず**配送鍵まで消費した**（実測）。
  // 材料が要素に載った今は再配送の相手そのものから読むので、同名の他人が居ても影響しない
  const base = startedSnapshot(startEvent({ operation: MATCH_KEY_ONLY, turnIdSource: "native" }));
  const own = base.state.pendingOperations[0] as PendingOperation;
  const twin: PendingOperation = {
    ...own,
    correlation: { ...own.correlation, taskLineageId: "lineage-other" },
  };
  const withTwin: TaskWorkStateSnapshotV1 = {
    ...base,
    state: { ...base.state, pendingOperations: [own, twin] },
  };
  const switched = reduceTaskWorkState(
    withTwin,
    startEvent({ operation: MATCH_KEY_ONLY, turnIdSource: "synthesized_monotonic", ingestSeq: "13" }),
    new Map(),
  );
  assert.equal(switched.outcome, "quarantined");
  assert.deepEqual(switched.diagnostics.map((d) => d.code), ["start_conflict"]);
  assert.equal(switched.ledger.size, 0);
});

test("自 lineage で id が衝突していても、それぞれの材料で判定する（#35）", () => {
  // 側索引の鍵は `operationId` だったので、同じ lineage に同名の pending が並ぶと帰属を判別できず、
  // 材料なしに倒す（`terminal_order_unverifiable`）しか無かった。要素に載せた今は、同名でも
  // 各 pending が自分の `startIngestSeq` を持つので、健全な terminal は普通に閉じる
  const base = startedSnapshot(startEvent({ operation: MATCH_KEY_ONLY }));
  const own = base.state.pendingOperations[0] as PendingOperation;
  // 同名だが確定済みの兄弟。open は 1 件のままなので候補選びは曖昧にならない
  const settled: PendingOperation = { ...own, status: "succeeded" };
  const collided: TaskWorkStateSnapshotV1 = {
    ...base,
    state: { ...base.state, pendingOperations: [own, settled] },
  };
  const closed = reduceTaskWorkState(
    collided,
    terminalEvent({ operation: { ...TERMINAL_OPERATION, nativeOperationId: undefined } }),
    new Map(),
  );
  assert.deepEqual(closed.diagnostics.map((d) => d.code), []);
  assert.equal(closed.snapshot.state.pendingOperations[0]?.status, "succeeded");
  // 材料を持たない側だけは従来どおり倒れる（材料の有無で分かれ、id の衝突では分かれない）
  const stripped: TaskWorkStateSnapshotV1 = {
    ...collided,
    state: {
      ...collided.state,
      pendingOperations: [withoutStartFacts(collided).state.pendingOperations[0] as PendingOperation, settled],
    },
  };
  const unverifiable = reduceTaskWorkState(
    stripped,
    terminalEvent({ operation: { ...TERMINAL_OPERATION, nativeOperationId: undefined } }),
    new Map(),
  );
  assert.deepEqual(unverifiable.diagnostics.map((d) => d.code), ["terminal_order_unverifiable"]);
});

test("再配送 start が turn の種別をすり替えたら隔離する", () => {
  // `turnIdSource` は turn 同一性の一部なのに凍結 `OperationCorrelationV1` の外にしか無いので、
  // `turnId` の比較では見えない。重複として台帳に入れると記録は元の種別のまま残り、再配送側の
  // 種別で来た terminal は rule 2 の候補選びで落ちて unknown に倒れる（証跡が失われ鍵も消費済み）
  const started = startedSnapshot(startEvent({ operation: MATCH_KEY_ONLY, turnIdSource: "native" }));
  const switched = reduceTaskWorkState(
    started,
    startEvent({ operation: MATCH_KEY_ONLY, turnIdSource: "synthesized_monotonic", ingestSeq: "13" }),
    new Map(),
  );
  assert.equal(switched.outcome, "quarantined");
  assert.deepEqual(switched.diagnostics.map((d) => d.code), ["start_conflict"]);
  assert.equal(switched.snapshot.state.pendingOperations[0]?.startTurnIdSource, "native");
});

test("記録が降格されていても、証明が戻った再配送を隔離しない", () => {
  // 記録側の `unavailable` は intake が作る（proven でない version の native 主張はここへ落ちる）。
  // 証明が回復した後に同じ start が `native` で再配送されるのは正当な経路なのに、これを衝突に
  // すると還元器は純関数なので毎回同じ隔離になり、決定論的な永久隔離と無限再送になる。
  // すり替えの検知は「2 つの具体的な主張が食い違っている」場合だけに閉じる
  const downgraded = startedSnapshot(
    startEvent({ operation: MATCH_KEY_ONLY, turnId: undefined, turnIdSource: "unavailable" }),
  );
  const recovered = reduceTaskWorkState(
    downgraded,
    startEvent({ operation: MATCH_KEY_ONLY, turnIdSource: "native", ingestSeq: "13" }),
    new Map(),
  );
  assert.deepEqual(recovered.diagnostics.map((d) => d.code), ["duplicate_operation_start"]);
  // 免除しても記録は汚れない。再配送は `startTurnIdSource` を書き換えないので降格された種別のまま残る
  assert.equal(recovered.snapshot.state.pendingOperations[0]?.startTurnIdSource, "unavailable");
});

/** 上限まで埋めた状態のうち index 1 を index 0 と同名にする（frozen schema は一意性を課さない）。 */
function collidedFilledSnapshot(): TaskWorkStateSnapshotV1 {
  const snapshot = filledSnapshot(true);
  const pendingOperations = snapshot.state.pendingOperations.map((pending, index) =>
    index === 1
      ? { ...pending, operationId: "op-0", correlation: { ...pending.correlation, operationId: "op-0" } }
      : pending,
  );
  return { ...snapshot, state: { ...snapshot.state, pendingOperations } };
}

test("状態側で operationId が衝突していても退避は必要な件数しか落とさない", () => {
  // 落とす相手を `operationId` の集合で持つと、1 件分の枠を空けるつもりで同名の兄弟まで
  // まとめて消える。上限に収めるのが目的なのに、上限を下回ってなお生きている operation が消える
  const result = reduceTaskWorkState(collidedFilledSnapshot(), startEvent(), new Map());
  assert.equal(result.outcome, "applied");
  assert.equal(result.snapshot.state.pendingOperations.length, CONTINUITY_LIMITS.arrayItems);
  // 落ちたのは terminal 済みの 1 件だけで、同名の started な兄弟は残る
  const survivors = result.snapshot.state.pendingOperations.filter((p) => p.operationId === "op-0");
  assert.equal(survivors.length, 1);
  assert.equal(survivors[0]?.status, "started");
});

test("同名の兄弟が退避されても、生き残った側は自分の材料で閉じられる（#35）", () => {
  // 側索引だった頃は、鍵が `operationId` なので id が衝突しているとどちらの兄弟の材料か判別できず、
  // 退避のたびに同名の材料をまとめて消すしか無かった。その結果、**生き残った側まで
  // `terminal_order_unverifiable` で `unknown` に倒れていた**（証跡は失われ、鍵は消費済み）。
  // 材料を要素に載せた今は、退避された側の材料だけがその要素と一緒に消える
  const snapshot = collidedFilledSnapshot();
  const seeded: TaskWorkStateSnapshotV1 = {
    ...snapshot,
    state: {
      ...snapshot.state,
      pendingOperations: snapshot.state.pendingOperations.map((pending) => ({
        ...pending,
        startIngestSeq: "1",
        startTurnIdSource: "native" as const,
      })),
    },
  };
  const evicted = reduceTaskWorkState(seeded, startEvent(), new Map());
  // 生き残った同名の兄弟は 1 件で、材料も持ったまま
  const survivors = evicted.snapshot.state.pendingOperations.filter((p) => p.operationId === "op-0");
  assert.equal(survivors.length, 1);
  assert.equal(survivors[0]?.startIngestSeq, "1");
  const terminal = reduceTaskWorkState(
    evicted.snapshot,
    // 生き残ったのは index 1（`collidedFilledSnapshot` が op-0 に改名した started 側）なので、
    // その nativeOperationId で rule 1 の terminal を当てる
    terminalEvent({
      eventId: "term-collided", adapterDeliveryId: "d-term-collided",
      canonicalFingerprint: "f-term-collided", ingestSeq: "2",
      operation: { ...TERMINAL_OPERATION, nativeOperationId: "toolu_filled_1" },
    }),
    evicted.ledger,
  );
  assert.deepEqual(terminal.diagnostics.map((d) => d.code), []);
  assert.equal(
    terminal.snapshot.state.pendingOperations.find((p) => p.operationId === "op-0")?.status,
    "succeeded",
  );
});

test("上限に余裕があるときは退避の診断を出さない", () => {
  const result = reduceTaskWorkState(emptySnapshot(), startEvent(), new Map());
  assert.deepEqual(result.diagnostics, []);
  assert.equal(result.snapshot.state.pendingOperations.length, 1);
});

// --- #43 / #39: 状態から消えた証跡を状態に残す ------------------------------

/** `droppedEvidence` を `n` 件だけ持つ状態。`recordedAt` は**降順**（時刻で並べ替えたら落ちる）。 */
function withDroppedEvidence(snapshot: TaskWorkStateSnapshotV1, n: number): TaskWorkStateSnapshotV1 {
  const droppedEvidence: DroppedEvidenceEntryV1[] = Array.from({ length: n }, (_, index) => ({
    reason: "evicted",
    operationId: `dropped-${index}`,
    status: "unknown",
    // 先頭ほど**新しい**時刻。`recordedAt` 昇順で落とすと末尾の古い側が消える
    recordedAt: `2026-08-15T00:00:${String(59 - (index % 60)).padStart(2, "0")}Z`,
    sensitivity: "normal",
  }));
  return { ...snapshot, state: { ...snapshot.state, droppedEvidence } };
}

test("同名 operationId の兄弟が並んでも、落ちた側を記録から特定できる（#43 FR-005）", () => {
  // 凍結 schema は `operationId` に一意性を課さない（`maxLength` だけ）。この還元器は同名の
  // 兄弟が並ぶ状態を明示的に支えているので、記録が id と status だけだと**その状態でだけ**
  // 「どちらが live な集合から落ちたか」が分からなくなる
  const base = filledSnapshot(true);
  const [dropped, ...rest] = base.state.pendingOperations;
  const twin: PendingOperation = {
    ...(dropped as PendingOperation),
    // 同じ id・同じ status の兄弟。start だけが違う
    sourceEventIds: ["event-start-twin"],
    correlation: {
      ...(dropped as PendingOperation).correlation,
      startEventId: "event-start-twin",
      nativeOperationId: "toolu_twin",
    },
  };
  const snapshot = {
    ...base,
    state: {
      ...base.state,
      pendingOperations: [dropped as PendingOperation, twin, ...rest.slice(1)],
    },
  };
  const result = reduceTaskWorkState(snapshot, startEvent(), new Map());
  const record = result.snapshot.state.droppedEvidence?.[0];
  assert.equal(record?.reason, "evicted");
  assert.equal(record?.operationId, "op-0");
  // 生き残った双子と同じ値では特定にならない
  assert.equal(record?.eventId, "event-start");
  assert.notEqual(record?.eventId, twin.correlation.startEventId);

  // **start を名指すのは `correlation.startEventId`**。`sourceEventIds` は append-only の
  // provenance 配列で、schema は先頭が start であることも順序も保証しない。並べ替えた状態でも
  // 専用欄を読むこと（先頭を読むと、後から積まれた event を start として記録してしまう）
  const reordered = {
    ...snapshot,
    state: {
      ...snapshot.state,
      pendingOperations: [
        { ...(dropped as PendingOperation), sourceEventIds: ["later-event", "event-start"] },
        ...snapshot.state.pendingOperations.slice(1),
      ],
    },
  };
  const fromDedicated = reduceTaskWorkState(reordered, startEvent(), new Map());
  assert.equal(fromDedicated.snapshot.state.droppedEvidence?.[0]?.eventId, "event-start");
});

test("上限超えの復元配列は、記録が重複でも刈って報告する（#43 FR-015）", () => {
  // 重複の再送は「状態を変えない隔離」に倒す設計だが、刈った結果ごと捨てると凍結 schema に
  // 反する状態（257 件）を返し続けることになる。刈りは 1 度で上限に収まるので収束する
  const over = withDroppedEvidence(emptySnapshot(), CONTINUITY_LIMITS.arrayItems + 1);
  const already = {
    ...over,
    state: {
      ...over.state,
      droppedEvidence: [
        ...(over.state.droppedEvidence ?? []),
        {
          reason: "orphaned_terminal" as const,
          eventId: "event-terminal",
          terminalFingerprint: "fingerprint-terminal",
          adapterDeliveryId: "delivery-terminal",
          recordedAt: "2026-08-16T00:00:02Z",
          sensitivity: "private" as const,
        },
      ],
    },
  };
  const result = reduceTaskWorkState(already, terminalEvent(), new Map());
  assert.equal(result.outcome, "quarantined");
  assert.equal(result.snapshot.state.droppedEvidence?.length, CONTINUITY_LIMITS.arrayItems);
  assert.deepEqual(
    result.diagnostics.map((d) => d.code),
    ["terminal_orphaned", "dropped_evidence_overflowed"],
  );
  // 記録は増えていない（重複判定は効いている）
  assert.equal(
    result.snapshot.state.droppedEvidence?.filter((e) => e.reason === "orphaned_terminal").length,
    1,
  );
  // 2 度目は上限に収まっているので差分ゼロ。刈りが毎回 revision を動かすと収束しない
  const again = reduceTaskWorkState(result.snapshot, terminalEvent(), result.ledger);
  assert.equal(again.snapshot.state.stateRevision, result.snapshot.state.stateRevision);
});

test("刈りで自分の記録が落ちた孤児は積み直され、収束は 2 revision かかる（#43）", () => {
  // 上の test は重複対象を配列の**末尾**に置いている。**先頭**にあると、front trim で落ちるのは
  // その孤児自身の記録なので、次の再送は重複ではなくなって積み直される。収束はするが 1 revision
  // ではない——正本と evidence にはこの上限で書く（「1 度刈れば以後 no-op」は成り立たない）
  const filler = withDroppedEvidence(emptySnapshot(), CONTINUITY_LIMITS.arrayItems);
  const front = {
    ...filler,
    state: {
      ...filler.state,
      droppedEvidence: [
        {
          reason: "orphaned_terminal" as const,
          eventId: "event-terminal",
          terminalFingerprint: "fingerprint-terminal",
          adapterDeliveryId: "delivery-terminal",
          recordedAt: "2026-08-16T00:00:02Z",
          sensitivity: "private" as const,
        },
        ...(filler.state.droppedEvidence ?? []),
      ],
    },
  };
  // 1 回目: 上限超えを刈る。落ちるのは先頭 = この孤児自身の記録
  const first = reduceTaskWorkState(front, terminalEvent(), new Map());
  assert.equal(first.snapshot.state.droppedEvidence?.length, CONTINUITY_LIMITS.arrayItems);
  assert.equal(
    first.snapshot.state.droppedEvidence?.some((e) => e.reason === "orphaned_terminal"),
    false,
  );
  // 2 回目: 記録が無くなっているので積み直す
  const second = reduceTaskWorkState(first.snapshot, terminalEvent(), first.ledger);
  assert.equal(
    second.snapshot.state.droppedEvidence?.filter((e) => e.reason === "orphaned_terminal").length,
    1,
  );
  assert.notEqual(second.snapshot.state.stateRevision, first.snapshot.state.stateRevision);
  // 3 回目でようやく差分ゼロ
  const third = reduceTaskWorkState(second.snapshot, terminalEvent(), second.ledger);
  assert.equal(third.snapshot.state.stateRevision, second.snapshot.state.stateRevision);
});

test("退避した operation は droppedEvidence に残る（#43 FR-005）", () => {
  const snapshot = filledSnapshot(true);
  // 退避元の機密度を引き継ぐことまで見る（記録側で内容を見て決め直さない: Constitution III）
  const secret = snapshot.state.pendingOperations.map((pending) =>
    pending.operationId === "op-0" ? { ...pending, sensitivity: "secret" as const } : pending,
  );
  const result = reduceTaskWorkState(
    { ...snapshot, state: { ...snapshot.state, pendingOperations: secret } },
    startEvent(),
    new Map(),
  );
  assert.deepEqual(result.snapshot.state.droppedEvidence, [
    {
      reason: "evicted",
      operationId: "op-0",
      status: "succeeded",
      // 同名 `operationId` の兄弟が並んだときに「どちらが落ちたか」を判別できる識別子
      eventId: "event-start",
      recordedAt: "2026-08-16T00:00:01Z",
      sensitivity: "secret",
    },
  ]);
  // 記録自体が機密なので、状態の集約機密度も上がる
  assert.equal(result.snapshot.state.sensitivity, "secret");
});

test("孤児 terminal は droppedEvidence に残るが、鍵は消費しない（#39 FR-006）", () => {
  // 隔離の判断は変えない。start が後から届く順序前後は正常運用なので、台帳に入れると
  // 二度と閉じられなくなる。記録は隔離の代わりではなく、隔離した事実を状態にも残すもの
  const orphan = terminalEvent();
  const result = reduceTaskWorkState(emptySnapshot(), orphan, new Map());
  assert.equal(result.outcome, "quarantined");
  assert.equal(result.ledger.size, 0);
  assert.deepEqual(result.snapshot.state.droppedEvidence, [
    {
      reason: "orphaned_terminal",
      eventId: "event-terminal",
      // 再送の重複判定に使う鍵。§8.2 の順で配送鍵が第一 authority、指紋はその fallback
      terminalFingerprint: "fingerprint-terminal",
      adapterDeliveryId: "delivery-terminal",
      recordedAt: "2026-08-16T00:00:02Z",
      // 相手が居ないので機密度を引き継げない。fail-closed の既定に倒す
      sensitivity: "private",
    },
  ]);
  assert.deepEqual(result.diagnostics.map((d) => d.code).sort(), [
    "dropped_evidence_recorded",
    "terminal_orphaned",
  ]);
  // 記録は state に入ったので revision も history も進む（revision だけ動いて履歴が
  // 追いつかないと、状態の revision が自分の履歴の末尾に無いことになる）
  assert.notEqual(result.snapshot.state.stateRevision, emptySnapshot().state.stateRevision);
  assert.equal(result.snapshot.history.at(-1)?.revision, result.snapshot.state.stateRevision);
});

test("孤児の再配送は eventId が変わっても収束する（#39 DoS 回帰）", () => {
  // **再配送は eventId / ingestSeq / occurredAt が変わりうる**（台帳の鍵が adapterDeliveryId で
  // あって eventId ではないのと同じ理由）。重複判定を eventId で行うと同じ terminal が何度でも
  // 記録され、記録が 256 件で頭打ちになった後も `stateRevision` と `history` が伸び続ける。
  // 隔離は鍵を消費しないので再送は止まらず、これは収束しない
  // （実測: 修正前は同一 adapterDeliveryId・同一 fingerprint の 300 再送で 300 revision /
  // history 300 / 台帳 0。CAS token を持つ下流が全部空振りし、履歴だけが伸びる）
  let snapshot = emptySnapshot();
  let ledger: IdempotencyLedger = new Map();
  const revisions = new Set<string>();
  for (let i = 0; i < 300; i += 1) {
    const redelivered = terminalEvent({
      eventId: `event-terminal-redelivery-${i}`,
      ingestSeq: String(1000 + i),
      occurredAt: `2026-08-16T01:${String(i % 60).padStart(2, "0")}:00Z`,
    });
    const result = reduceTaskWorkState(snapshot, redelivered, ledger);
    assert.equal(result.outcome, "quarantined");
    snapshot = result.snapshot;
    ledger = result.ledger;
    revisions.add(snapshot.state.stateRevision);
  }
  assert.equal(revisions.size, 1);
  assert.equal(snapshot.history.length, 1);
  assert.equal(snapshot.state.droppedEvidence?.length, 1);
  assert.equal(ledger.size, 0);
});

test("同じ孤児 terminal の再配送は記録を増やさない（収束する）", () => {
  // 隔離は鍵を消費せず還元器は純関数なので、同じ terminal は再送され続ける。重複を
  // 落とさないと、記録が再送のたびに 1 件ずつ伸びて 256 件の枠を食い潰す
  const orphan = terminalEvent();
  const first = reduceTaskWorkState(emptySnapshot(), orphan, new Map());
  const again = reduceTaskWorkState(first.snapshot, orphan, first.ledger);
  assert.equal(again.outcome, "quarantined");
  assert.equal(again.snapshot.state.droppedEvidence?.length, 1);
  // 2 度目は状態が動かない。revision が変わると CAS token として使う下流が空振りする
  assert.equal(again.snapshot.state.stateRevision, first.snapshot.state.stateRevision);
  assert.equal(again.snapshot.history.length, first.snapshot.history.length);
  // 記録していないので `dropped_evidence_recorded` も出さない
  assert.deepEqual(again.diagnostics.map((d) => d.code), ["terminal_orphaned"]);

  // **指紋が違えば別の terminal**なので記録する（訂正版が黙って落ちない）
  const corrected = reduceTaskWorkState(
    again.snapshot,
    terminalEvent({
      eventId: "event-terminal-corrected",
      adapterDeliveryId: "delivery-terminal-corrected",
      canonicalFingerprint: "fingerprint-terminal-CORRECTED",
    }),
    again.ledger,
  );
  assert.equal(corrected.snapshot.state.droppedEvidence?.length, 2);
});

test("記録の重複判定は §8.2 の順（配送鍵が第一 authority、指紋は fallback）", () => {
  // 鍵をどちらか一方にすると、**両方向のうち片方が必ず壊れる**。実測で 2 つとも確認した:
  //
  // - 指紋だけを鍵にすると、配送鍵も `operationMatchKey` も違う 300 件の孤児が、同じ指紋を
  //   名乗るだけで記録 1 件に潰れた（history 1 / 記録 1 / 台帳 0、299 件が診断も無く消える）。
  //   `canonicalFingerprint` は adapter が算出して wire で運ぶ値なので、単独では同一性の
  //   権威にならない。「配送 ID が違えば同じ指紋でも別の論理 event」は §8.2 の規則で、
  //   台帳側は以前からそう振る舞っている
  // - `eventId` を鍵にすると、同一配送の再送が毎回別物として記録された（1 → 300 revision）。
  //   再配送は「同じ配送鍵・違う eventId・違う ingestSeq」という契約なので、これは**正直な
  //   adapter** が引き起こす
  //
  // 台帳と同じ優先順位（配送鍵 → 指紋）にすると両方が閉じる。keyspace も台帳と同じく分ける
  const measure = (
    label: string,
    vary: (i: number) => Partial<NormalizedContinuityEvent>,
  ) => {
    let snapshot = emptySnapshot();
    let ledger: IdempotencyLedger = new Map();
    const revisions = new Set<string>();
    for (let i = 0; i < 300; i += 1) {
      const result = reduceTaskWorkState(
        snapshot,
        terminalEvent({ eventId: `event-terminal-${label}-${i}`, ingestSeq: String(1000 + i), ...vary(i) }),
        ledger,
      );
      snapshot = result.snapshot;
      ledger = result.ledger;
      revisions.add(snapshot.state.stateRevision);
    }
    return {
      revisions: revisions.size,
      history: snapshot.history.length,
      dropped: snapshot.state.droppedEvidence?.length ?? 0,
      ledger: ledger.size,
    };
  };

  // 同じ配送鍵の再送は、指紋が変わっても 1 件に収束する。隔離は配送鍵を消費しないので
  // 再送は止まらず、ここが収束しないと記録が飽和した後も revision と history が伸び続ける
  assert.deepEqual(
    measure("same-delivery", (i) => ({ canonicalFingerprint: `fingerprint-${i}` })),
    { revisions: 1, history: 1, dropped: 1, ledger: 0 },
  );

  // 配送鍵が違えば別の論理 event。指紋が同じでも、相関材料が違えば別々に記録する
  assert.deepEqual(
    measure("cross-delivery", (i) => ({
      adapterDeliveryId: `delivery-${i}`,
      canonicalFingerprint: "same-fingerprint",
      operation: {
        ...TERMINAL_OPERATION,
        operationMatchKey: `match-${i}`,
        nativeOperationId: `toolu_${i}`,
        canonicalInputHash: `input-${i}`,
      },
    })),
    { revisions: 300, history: 300, dropped: 256, ledger: 0 },
  );

  // 同じ配送鍵で指紋が食い違うのは再送ではなく corruption。件数が 1 に収束するだけでは
  // 足りない——適用済み経路は同じ条件を `delivery_conflict` にしているので、孤児経路だけが
  // 黙って重複に倒すと source hash の食い違いが通常の再送と区別できなくなる
  const recorded = reduceTaskWorkState(emptySnapshot(), terminalEvent(), new Map());
  const corrupted = reduceTaskWorkState(
    recorded.snapshot,
    terminalEvent({ eventId: "event-terminal-corrupt", canonicalFingerprint: "fingerprint-OTHER" }),
    recorded.ledger,
  );
  assert.equal(corrupted.outcome, "quarantined");
  // 照合より前に落とす（台帳の衝突検査と同じ扱い）。start が届いた後の再送も同じ経路を通る
  assert.deepEqual(corrupted.diagnostics.map((d) => d.code), ["delivery_conflict"]);
  // corruption は「live な集合から落ちた証跡」ではないので記録も状態も動かさない
  assert.equal(corrupted.snapshot.state.droppedEvidence?.length, 1);
  assert.equal(corrupted.snapshot.state.droppedEvidence?.[0]?.terminalFingerprint, "fingerprint-terminal");
  assert.equal(corrupted.snapshot.state.stateRevision, recorded.snapshot.state.stateRevision);
  assert.equal(corrupted.ledger.size, recorded.ledger.size);

  // **start が届いた後の再送も同じ検査に当たる**。隔離は配送鍵を消費しないので、孤児を記録した
  // 後に start が来ると、同じ配送鍵の再送は照合経路へ進む。入口で見ないと corruption の検出が
  // start の到着順しだいになる（実測: 見ないと `applied` / 診断ゼロ / operation は succeeded）
  const afterStart = apply(emptySnapshot(), [terminalEvent({ canonicalFingerprint: "F1" })]);
  const started = reduceTaskWorkState(afterStart.snapshot, startEvent(), afterStart.ledger);
  const conflicting = reduceTaskWorkState(
    started.snapshot,
    terminalEvent({ eventId: "event-terminal-f2", canonicalFingerprint: "F2", ingestSeq: "13" }),
    started.ledger,
  );
  assert.equal(conflicting.outcome, "quarantined");
  assert.deepEqual(conflicting.diagnostics.map((d) => d.code), ["delivery_conflict"]);
  assert.equal(conflicting.snapshot.state.pendingOperations[0]?.status, "started");
  assert.equal(conflicting.ledger.size, started.ledger.size);
  // 対照: 同じ配送鍵でも**同じ指紋**なら再送なので、そのまま閉じられる
  const honest = reduceTaskWorkState(
    started.snapshot,
    terminalEvent({ eventId: "event-terminal-f1", canonicalFingerprint: "F1", ingestSeq: "13" }),
    started.ledger,
  );
  assert.equal(honest.outcome, "applied");
  assert.equal(honest.snapshot.state.pendingOperations[0]?.status, "succeeded");

  // 材料が欠けている側は「違う」と言えない。指紋を持たない記録（この版より前の checkpoint）に
  // 指紋つきの再送が来ても corruption ではなく重複（FR-012 と同じで、検査は材料が無ければ発動しない）
  const noFingerprint = emptySnapshot({
    droppedEvidence: [{
      reason: "orphaned_terminal",
      eventId: "event-old",
      adapterDeliveryId: "delivery-terminal",
      recordedAt: "2026-08-16T00:00:00Z",
      sensitivity: "private",
    }],
  });
  const resent = reduceTaskWorkState(noFingerprint, terminalEvent(), new Map());
  assert.deepEqual(resent.diagnostics.map((d) => d.code), ["terminal_orphaned"]);
  assert.equal(resent.snapshot.state.droppedEvidence?.length, 1);
  assert.equal(resent.snapshot.state.stateRevision, noFingerprint.state.stateRevision);

  // 配送鍵を名乗らない記録どうしは指紋で判定する（この版より前の checkpoint・配送鍵を持たない
  // adapter）。`ledgerKeyOf` の `d:`/`f:` と同じ分割なので、両者が混ざっても衝突しない
  const legacy = emptySnapshot({
    droppedEvidence: [{
      reason: "orphaned_terminal",
      eventId: "event-old",
      terminalFingerprint: "fingerprint-terminal",
      recordedAt: "2026-08-16T00:00:00Z",
      sensitivity: "private",
    }],
  });
  const blankDelivery = reduceTaskWorkState(
    legacy,
    terminalEvent({ eventId: "event-terminal-blank", adapterDeliveryId: undefined }),
    new Map(),
  );
  assert.equal(blankDelivery.snapshot.state.droppedEvidence?.length, 1);
  assert.equal(blankDelivery.snapshot.state.stateRevision, legacy.state.stateRevision);
});

test("開いた候補が 1 件も無い unmatched な terminal も記録する（#43）", () => {
  // 候補ゼロ（`terminal_orphaned`）だけを記録すると、「候補は居るが全員確定済みで turn も
  // 両立しない」terminal が診断だけで消える。開いた候補が居ないので `unknown` として事実を
  // 持つ相手も無く、後から start が届く見込みも無い——状態から静かに落ちる、#43 が塞ぐ損失
  const settled = apply(emptySnapshot(), [
    startEvent({
      eventId: "start-native", adapterDeliveryId: "d-start-native",
      canonicalFingerprint: "f-start-native", operation: MATCH_KEY_ONLY,
      ingestSeq: "11", turnIdSource: "native",
    }),
    terminalEvent({
      eventId: "term-native", adapterDeliveryId: "d-term-native",
      canonicalFingerprint: "f-term-native", operation: { ...MATCH_KEY_ONLY, phase: "terminal" },
      ingestSeq: "12", turnIdSource: "native",
    }),
  ]);
  const unmatched = reduceTaskWorkState(
    settled.snapshot,
    terminalEvent({
      eventId: "term-synth", adapterDeliveryId: "d-term-synth",
      canonicalFingerprint: "f-term-synth", operation: { ...MATCH_KEY_ONLY, phase: "terminal" },
      ingestSeq: "13", turnIdSource: "synthesized_monotonic",
    }),
    settled.ledger,
  );
  assert.equal(unmatched.outcome, "quarantined");
  assert.ok(unmatched.diagnostics.some((d) => d.code === "terminal_unmatched"));
  const recorded = unmatched.snapshot.state.droppedEvidence ?? [];
  assert.equal(recorded.length, 1);
  assert.equal(recorded[0]?.reason, "orphaned_terminal");
  assert.equal(recorded[0]?.terminalFingerprint, "f-term-synth");
  // 隔離は配送鍵を消費しない（後から説明のつく start が届けば同じ terminal で閉じられる）
  assert.equal(unmatched.ledger.size, settled.ledger.size);
  // 再送しても記録は増えない（重複判定は指紋）
  const again = reduceTaskWorkState(
    unmatched.snapshot,
    terminalEvent({
      eventId: "term-synth-2", adapterDeliveryId: "d-term-synth",
      canonicalFingerprint: "f-term-synth", operation: { ...MATCH_KEY_ONLY, phase: "terminal" },
      ingestSeq: "14", turnIdSource: "synthesized_monotonic",
    }),
    unmatched.ledger,
  );
  assert.equal(again.snapshot.state.droppedEvidence?.length, 1);
  assert.equal(again.snapshot.state.stateRevision, unmatched.snapshot.state.stateRevision);
});

test("記録だけの隔離は §4.1 の watermark を進めない", () => {
  // §4.1 は `lastIngestSeq` を「**適用された** event の最大 `ingestSeq`」と定義する。孤児は
  // 隔離で配送鍵を消費しないので、後から start が届けば同じ event が適用されうる。ここで
  // watermark を先に進めると、まだ状態に入っていない位置を「適用済みの最大」として名乗る
  const before = emptySnapshot();
  const orphan = terminalEvent({ ingestSeq: "500" });
  const result = reduceTaskWorkState(before, orphan, new Map());
  assert.equal(result.outcome, "quarantined");
  assert.equal(result.snapshot.state.lastIngestSeq, before.state.lastIngestSeq);
  // `updatedAt` は「revision を作った event の occurredAt」なので、こちらは進む
  assert.equal(result.snapshot.state.updatedAt, orphan.occurredAt);
  // 記録自体は入っている（進めないのは watermark だけ）
  assert.equal(result.snapshot.state.droppedEvidence?.length, 1);
});

test("復元した状態の順序材料は、そのまま権威として使われる（信頼境界）", () => {
  // #35 で材料を凍結 schema に載せた結果、**復元した状態が材料を運べるようになった**。
  // それが目的（状態だけを渡された実装が §4.3 を満たせる）だが、裏返すと還元器は状態側の値を
  // 検証しない。低い `startIngestSeq` を持つ状態を渡せば、順序違反の terminal が診断ゼロで通る。
  // これは #35 が作った穴ではない: 同じ状態を書ける相手は `status: "succeeded"` を直接書ける
  // （もっと強い）。ここで固定するのは「状態は権威である」という前提そのもので、
  // daemon 側が checkpoint の出どころを保証する責任を負う
  const started = startedSnapshot();
  const forged: TaskWorkStateSnapshotV1 = {
    ...started,
    state: {
      ...started.state,
      pendingOperations: started.state.pendingOperations.map((pending) => ({
        ...pending,
        startIngestSeq: "1",
      })),
    },
  };
  // 本来の start は ingestSeq 11 なので、10 の terminal は順序違反。偽の 1 なら通ってしまう
  const closed = reduceTaskWorkState(forged, terminalEvent({ ingestSeq: "10" }), new Map());
  assert.deepEqual(closed.diagnostics.map((d) => d.code), []);
  assert.equal(closed.snapshot.state.pendingOperations[0]?.status, "succeeded");
  // 対照: 材料が本物なら同じ terminal は落ちる
  const honest = reduceTaskWorkState(started, terminalEvent({ ingestSeq: "10" }), new Map());
  assert.deepEqual(honest.diagnostics.map((d) => d.code), ["terminal_out_of_order"]);
});

test("孤児の記録は、後から start が届いて閉じても消えない", () => {
  // 記録は「その時点で相手が居なかった」という履歴。閉じたときに消すのは、どの FR も
  // 求めていない規則を足すことになる（消すなら「いつ消すか」を別に決める必要がある）
  const orphan = reduceTaskWorkState(emptySnapshot(), terminalEvent(), new Map());
  const started = reduceTaskWorkState(orphan.snapshot, startEvent(), orphan.ledger);
  const closed = reduceTaskWorkState(started.snapshot, terminalEvent(), started.ledger);
  assert.equal(closed.snapshot.state.pendingOperations[0]?.status, "succeeded");
  assert.equal(closed.snapshot.state.droppedEvidence?.length, 1);
});

test("記録は識別と分類だけを持つ（#43 FR-007）", () => {
  // payload・引数・出力・description を入れると、状態から落とした証跡が**落とす前より
  // 詳しい**ことになる。schema の additionalProperties: false は「知らない欄」を止めるだけで、
  // 既知の欄に中身を詰めるのは止められないので、欄集合そのものを固定する
  const evicted = reduceTaskWorkState(filledSnapshot(true), startEvent(), new Map());
  const orphaned = reduceTaskWorkState(emptySnapshot(), terminalEvent(), new Map());
  assert.deepEqual(Object.keys(evicted.snapshot.state.droppedEvidence?.[0] ?? {}).sort(), [
    "eventId",
    "operationId",
    "reason",
    "recordedAt",
    "sensitivity",
    "status",
  ]);
  // `terminalFingerprint` と `adapterDeliveryId` は識別（どの terminal のどの配送か）であって
  // 内容ではない。event が名乗った値をそのまま写すだけで、payload は入らない
  assert.deepEqual(Object.keys(orphaned.snapshot.state.droppedEvidence?.[0] ?? {}).sort(), [
    "adapterDeliveryId",
    "eventId",
    "reason",
    "recordedAt",
    "sensitivity",
    "terminalFingerprint",
  ]);
});

test("記録が上限のときは配列の先頭から落とす（#43 FR-008 / FR-015）", () => {
  // `recordedAt` は adapter が寄越した `occurredAt` の写しなので、時刻で並べ替えると
  // event を出す側がどれを残すか選べる。`pendingOperations` の退避と同じ規則にする
  const full = withDroppedEvidence(filledSnapshot(true), CONTINUITY_LIMITS.arrayItems);
  const result = reduceTaskWorkState(full, startEvent(), new Map());
  const recorded = result.snapshot.state.droppedEvidence ?? [];
  assert.equal(recorded.length, CONTINUITY_LIMITS.arrayItems);
  // 落ちたのは先頭（`recordedAt` 昇順なら末尾の dropped-255 が落ちるので、そこで分かれる）
  assert.equal(recorded.some((entry) => entry.operationId === "dropped-0"), false);
  assert.equal(recorded.some((entry) => entry.operationId === "dropped-255"), true);
  assert.equal(recorded.at(-1)?.operationId, "op-0");
  // 黙って消さない
  assert.deepEqual(result.diagnostics.map((d) => d.code).sort(), [
    "dropped_evidence_overflowed",
    "dropped_evidence_recorded",
    "pending_operations_evicted",
  ]);
});

test("上限を超えた復元状態は、退避が無くても刈られて診断が出る（#43 FR-015）", () => {
  // 上限は「この還元器が作る状態」だけでなく**状態そのもの**の性質（凍結 schema の maxItems）。
  // 別実装や上限導入前の writer が上限超えの記録を書いた状態を素通しすると、還元器が自分の
  // 凍結 schema に反する状態を出し続ける。しかも刈った事実を黙ると doctor が気づけない
  const over = withDroppedEvidence(startedSnapshot(), CONTINUITY_LIMITS.arrayItems + 44);
  const result = reduceTaskWorkState(over, startEvent({
    eventId: "event-start-2",
    adapterDeliveryId: "delivery-start-2",
    ingestSeq: "12",
    operation: { ...START_OPERATION, operationMatchKey: "match-key-2", nativeOperationId: "toolu_2" },
  }), new Map());
  // 退避は起きていない（`pendingOperations` は 2 件で上限に遠い）
  assert.equal(result.snapshot.state.pendingOperations.length, 2);
  assert.equal(result.snapshot.state.droppedEvidence?.length, CONTINUITY_LIMITS.arrayItems);
  assert.deepEqual(result.diagnostics.map((d) => d.code), ["dropped_evidence_overflowed"]);
  // 落ちたのは先頭側
  assert.equal(result.snapshot.state.droppedEvidence?.[0]?.operationId, "dropped-44");
});

test("復元状態の空配列は「欄なし」と同じ状態になる（#43 FR-013）", () => {
  // 空配列と欄なしは同じ意味（何も落ちていない）。2 通りの綴りを許すと canonical hash が割れ、
  // 同じ event 列を還元した 2 ノードの `stateRevision` が永久に一致しない。空配列を書く writer は
  // 珍しくない（Rust/Go の marshaller は非 nil の空 slice を `[]` にする）
  const omitted = startedSnapshot();
  assert.equal(omitted.state.droppedEvidence, undefined);
  const spelled: TaskWorkStateSnapshotV1 = {
    ...omitted,
    state: { ...omitted.state, droppedEvidence: [] },
  };
  const fromOmitted = reduceTaskWorkState(omitted, terminalEvent(), new Map());
  const fromSpelled = reduceTaskWorkState(spelled, terminalEvent(), new Map());
  assert.equal(fromSpelled.snapshot.state.droppedEvidence, undefined);
  assert.equal(fromSpelled.contentHash, fromOmitted.contentHash);
});

test("revision をまたいで droppedEvidence の配列を共有しない（§4.2）", () => {
  // §4.2 の immutable revision は値の一致ではなく **object を分ける**ことで守る。共有したまま
  // どちらかを in-place で触ると（Rust/Go 移植では自然な書き方）、過去の revision の中身が
  // 変わって、保存済みの `contentHash` が自分の中身と合わなくなる
  const seeded = reduceTaskWorkState(withDroppedEvidence(filledSnapshot(true), 4), startEvent(), new Map());
  const carried = seeded.snapshot.state.droppedEvidence;
  assert.notEqual(carried, undefined);

  // 記録に触らない経路（terminal）でも配列は分かれる
  const closed = reduceTaskWorkState(seeded.snapshot, terminalEvent(), seeded.ledger);
  assert.deepEqual(closed.snapshot.state.droppedEvidence, carried);
  assert.notEqual(closed.snapshot.state.droppedEvidence, carried);

  // `commit` を通らない放棄の確定も同じ
  const abandoned = finalizeAbandonedState(
    seeded.snapshot.state,
    startEvent({
      eventId: "event-abandon-share",
      adapterDeliveryId: "delivery-abandon-share",
      kind: "session_ended",
      ingestSeq: "13",
      operation: undefined,
    }),
    new Map(),
  );
  assert.deepEqual(abandoned.state.droppedEvidence, carried);
  assert.notEqual(abandoned.state.droppedEvidence, carried);
});

// --- #44: 受理した terminal の指紋 -----------------------------------------

test("確定時に terminalFingerprint を残し、違う指紋の 2 通目を隔離する（#44 FR-010/FR-011）", () => {
  // §4.3 は terminal に「未適用であること」と「payload/source hash が衝突しないこと」の両方を
  // 課すが、配送 ID が違う 2 通目は dedupe で比べられない。成否が同じなら成否矛盾ゲートも
  // 素通りするので、受理した指紋を残さない限り**中身の違う 2 通目が黙って適用済み扱いになる**
  const closed = reduceTaskWorkState(startedSnapshot(), terminalEvent(), new Map());
  assert.equal(closed.snapshot.state.pendingOperations[0]?.terminalFingerprint, "fingerprint-terminal");

  const second = reduceTaskWorkState(
    closed.snapshot,
    terminalEvent({
      eventId: "event-terminal-2",
      adapterDeliveryId: "delivery-terminal-2",
      canonicalFingerprint: "fingerprint-terminal-OTHER",
    }),
    closed.ledger,
  );
  assert.equal(second.outcome, "quarantined");
  assert.deepEqual(second.diagnostics.map((d) => d.code), ["terminal_conflict"]);
  // 隔離なので状態は動かない（訂正版を後から入れ直せる）
  assert.equal(second.snapshot.state.stateRevision, closed.snapshot.state.stateRevision);
  assert.equal(second.ledger.size, closed.ledger.size);
});

test("指紋が同じなら別の配送 ID でも適用済みの再配送（#44 Acceptance 2）", () => {
  const closed = reduceTaskWorkState(startedSnapshot(), terminalEvent(), new Map());
  const resent = reduceTaskWorkState(
    closed.snapshot,
    terminalEvent({ eventId: "event-terminal-2", adapterDeliveryId: "delivery-terminal-2" }),
    closed.ledger,
  );
  assert.equal(resent.outcome, "applied");
  assert.deepEqual(resent.diagnostics.map((d) => d.code), ["terminal_already_applied"]);
});

test("指紋を持たない旧い状態では新しい検査が発動しない（#44 FR-012）", () => {
  // 材料が無いことを「衝突」と読むと、この版より前に閉じた operation 宛ての正当な再配送が
  // 全部隔離される。隔離は鍵を消費せず還元器は純関数なので、adapter は無限に再送する
  const closed = reduceTaskWorkState(startedSnapshot(), terminalEvent(), new Map());
  const old = {
    ...closed.snapshot,
    state: {
      ...closed.snapshot.state,
      pendingOperations: closed.snapshot.state.pendingOperations.map(
        ({ terminalFingerprint: _fingerprint, ...rest }) => rest,
      ),
    },
  };
  const second = reduceTaskWorkState(
    old,
    terminalEvent({
      eventId: "event-terminal-2",
      adapterDeliveryId: "delivery-terminal-2",
      canonicalFingerprint: "fingerprint-terminal-OTHER",
    }),
    closed.ledger,
  );
  assert.equal(second.outcome, "applied");
  assert.deepEqual(second.diagnostics.map((d) => d.code), ["terminal_already_applied"]);
});

test("兄弟の一方だけが指紋を持つ状態でも新しい検査は発動しない（#44 FR-012）", () => {
  // 上の test は**全員**が材料を持たない状態しか観測しない。実際に起きるのは混在
  // ——この版より前に閉じた兄弟（指紋なし）と、この版で閉じた兄弟（指紋あり）が同じ
  // matchKey に並ぶ状態。材料を持つ兄弟を先に見つけて衝突にすると、**指紋なしの兄弟宛ての
  // 正当な再配送**が隔離される。隔離は鍵を消費しないので adapter は無限再送になる
  const closed = reduceTaskWorkState(startedSnapshot(), terminalEvent(), new Map());
  const withFingerprint = closed.snapshot.state.pendingOperations[0] as PendingOperation;
  assert.notEqual(withFingerprint.terminalFingerprint, undefined);
  const { terminalFingerprint: _dropped, ...withoutFingerprint } = withFingerprint;
  const mixed: TaskWorkStateSnapshotV1 = {
    ...closed.snapshot,
    state: {
      ...closed.snapshot.state,
      // 指紋を持つ兄弟が先に並ぶ順序にする（`find` は先頭から見るので、この順序が最悪）
      pendingOperations: [
        withFingerprint,
        { ...withoutFingerprint, operationId: `${withFingerprint.operationId}-old` },
      ],
    },
  };
  const redelivered = reduceTaskWorkState(
    mixed,
    terminalEvent({
      eventId: "event-terminal-mixed",
      adapterDeliveryId: "delivery-terminal-mixed",
      canonicalFingerprint: "fingerprint-terminal-OTHER",
    }),
    closed.ledger,
  );
  assert.equal(redelivered.outcome, "applied");
  assert.deepEqual(redelivered.diagnostics.map((d) => d.code), ["terminal_already_applied"]);
});

test("指紋を持たない確定済み候補は event 経路からは作れない（#44 FR-012 の前提）", () => {
  // FR-012 のせいで「指紋を名乗らない候補が 1 件でもあれば衝突検査が発動しない」ので、
  // **囮を 1 件置けば集合全体の検査を無効化できる**という指摘がありうる。前提が成り立つのは
  // 囮を作れる場合だけなので、作れる経路を数える。
  //
  // 還元器が terminal で閉じるときは必ず指紋を書く。書かないのは `unknown` に倒したときだけで、
  // その `unknown` は `isOpen` が **open として数える**——つまり候補集合に open が残るので、
  // 衝突検査がある「候補が全部確定済み」の分岐にはそもそも入らない。
  // 残るのは**状態を書ける相手**（この版より前の checkpoint・別実装・欄を落とす reader）だけで、
  // その相手は囮を置くより先に `status` や `terminalFingerprint` を直接書ける。囮は新しい能力を
  // 与えないので、可用性（無限再送は永久に収束しない）を優先する FR-012 の選択を変えない
  const ambiguous = reduceTaskWorkState(startedSnapshot(), terminalEvent({ successful: undefined }), new Map());
  const pending = ambiguous.snapshot.state.pendingOperations[0] as PendingOperation;
  assert.equal(pending.status, "unknown");
  assert.equal(pending.terminalFingerprint, undefined);

  // 指紋なしのこの候補が居ても、確定済み経路（terminal_already_applied / terminal_conflict）
  // には落ちない。open として扱われるので、衝突検査そのものに到達しない
  const next = reduceTaskWorkState(
    ambiguous.snapshot,
    terminalEvent({
      eventId: "event-terminal-after-unknown",
      adapterDeliveryId: "delivery-terminal-after-unknown",
      canonicalFingerprint: "fingerprint-terminal-OTHER",
    }),
    ambiguous.ledger,
  );
  assert.equal(
    next.diagnostics.some((d) => d.code === "terminal_already_applied" || d.code === "terminal_conflict"),
    false,
  );
});

test("unknown に倒した operation には指紋を残さない（#44 FR-010）", () => {
  // `unknown` は「成否を主張できなかった」なので terminal を受理していない。ここで指紋を残すと、
  // 後から本物の terminal が届いたときに指紋違いの衝突として隔離され、永久に閉じられない
  const started = startedSnapshot();
  const ambiguous = reduceTaskWorkState(started, terminalEvent({ successful: undefined }), new Map());
  assert.equal(ambiguous.snapshot.state.pendingOperations[0]?.status, "unknown");
  assert.equal(ambiguous.snapshot.state.pendingOperations[0]?.terminalFingerprint, undefined);
});

test("記録側の空白の指紋は「無い」として読む（#44 FR-012）", () => {
  // 届く側の `canonicalFingerprint` は必須の identity 材料なので、空白は
  // `assertIdentityMaterial` が先に落とす（この検査には届かない）。空白がありうるのは
  // **状態側**——`terminalFingerprint` は任意欄で、別実装が空白を書きうる。空白を
  // 「違う指紋」と読むと、その operation 宛ての正当な再配送が毎回隔離されて収束しない
  const closed = reduceTaskWorkState(startedSnapshot(), terminalEvent(), new Map());
  for (const blank of ["", "   ", "​"]) {
    const blanked = {
      ...closed.snapshot,
      state: {
        ...closed.snapshot.state,
        pendingOperations: closed.snapshot.state.pendingOperations.map((pending) => ({
          ...pending,
          terminalFingerprint: blank,
        })),
      },
    };
    const second = reduceTaskWorkState(
      blanked,
      terminalEvent({
        eventId: "event-terminal-2",
        adapterDeliveryId: "delivery-terminal-2",
        canonicalFingerprint: "fingerprint-terminal-OTHER",
      }),
      closed.ledger,
    );
    assert.deepEqual(second.diagnostics.map((d) => d.code), ["terminal_already_applied"], JSON.stringify(blank));
  }
});

/** rule 2（matchKey 照合）の terminal。`nativeOperationId` を名乗らない adapter の形 */
function matchKeyTerminal(overrides: Partial<NormalizedContinuityEvent> = {}): IntakeStampedEventV1 {
  return terminalEvent({
    operation: { ...TERMINAL_OPERATION, nativeOperationId: undefined },
    ...overrides,
  });
}

/** 復元状態にだけありうる形: open なまま指紋を持つ pending。還元器は自分では作らない */
function openWithFingerprint(
  fingerprint: string,
  status: "started" | "unknown" = "started",
  start = startEvent(),
) {
  const started = startedSnapshot(start);
  return {
    ...started,
    state: {
      ...started.state,
      pendingOperations: started.state.pendingOperations.map((pending) => ({
        ...pending,
        status,
        terminalFingerprint: fingerprint,
      })),
    },
  };
}

test("open な候補が持つ指紋は上書きせず衝突にする（#44 FR-010/FR-011）", () => {
  // 閉じる経路は指紋を無条件に上書きする。確定済みの候補には衝突検査があるが、**open な
  // 候補には無かった**（実測: `started` + 指紋 F1 の復元状態に F2 の terminal を渡すと
  // `applied` / 診断ゼロ / `succeeded` / 指紋は F2 に化け、F1 の証跡は状態から消えた）。
  // 凍結 schema は open な要素が指紋を持つことを妨げないので、復元状態では必ず起きうる形
  for (const status of ["started", "unknown"] as const) {
    const restored = openWithFingerprint("F1", status);
    const conflicting = reduceTaskWorkState(
      restored,
      terminalEvent({ canonicalFingerprint: "F2" }),
      new Map(),
    );
    assert.equal(conflicting.outcome, "quarantined", status);
    assert.deepEqual(conflicting.diagnostics.map((d) => d.code), ["terminal_conflict"], status);
    // 隔離なので状態は動かず、証跡も残る（訂正版を後から入れ直せる）
    assert.equal(conflicting.snapshot.state.pendingOperations[0]?.status, status);
    assert.equal(conflicting.snapshot.state.pendingOperations[0]?.terminalFingerprint, "F1");
    assert.equal(conflicting.snapshot.state.stateRevision, restored.state.stateRevision);
    assert.equal(conflicting.ledger.size, 0);
  }

  // **rule 2 でも同じこと**。既定の fixture は `nativeOperationId` を持つので rule 1 しか通らず、
  // 検査を rule 1 に限定しても test は全件通ってしまう（外部レビューが変異で実測した）。
  // matchKey 照合の adapter は `nativeOperationId` を名乗らないので、そちらでも固定する
  const byMatchKey = reduceTaskWorkState(
    openWithFingerprint("F1", "started", startEvent({ operation: MATCH_KEY_ONLY })),
    matchKeyTerminal({ canonicalFingerprint: "F2" }),
    new Map(),
  );
  assert.equal(byMatchKey.outcome, "quarantined");
  assert.deepEqual(byMatchKey.diagnostics.map((d) => d.code), ["terminal_conflict"]);
  assert.equal(byMatchKey.snapshot.state.pendingOperations[0]?.terminalFingerprint, "F1");
  assert.equal(byMatchKey.ledger.size, 0);
});

test("指紋の衝突は台帳を消費する順序分岐より先に判定する（#44）", () => {
  // 隔離は配送鍵を消費しないが、順序の 2 分岐（`terminal_order_unverifiable` /
  // `terminal_out_of_order`）は消費する。後ろに置くと、指紋が食い違う terminal が順序の穴を
  // 通って `unknown` に化け、訂正版の再配送が重複 no-op として黙って捨てられる
  const unverifiable = reduceTaskWorkState(
    withoutStartFacts(openWithFingerprint("F1")),
    terminalEvent({ canonicalFingerprint: "F2" }),
    new Map(),
  );
  assert.deepEqual(unverifiable.diagnostics.map((d) => d.code), ["terminal_conflict"]);
  assert.equal(unverifiable.ledger.size, 0);

  const outOfOrder = reduceTaskWorkState(
    openWithFingerprint("F1"),
    terminalEvent({ canonicalFingerprint: "F2", ingestSeq: "1" }),
    new Map(),
  );
  assert.deepEqual(outOfOrder.diagnostics.map((d) => d.code), ["terminal_conflict"]);
  assert.equal(outOfOrder.ledger.size, 0);
});

test("open な候補の指紋検査は、材料が揃って初めて発動する（#44 FR-012）", () => {
  // 通す側も固定する。締めすぎると健全な terminal が台帳に入らず adapter は無限再送になる
  const sameFingerprint = reduceTaskWorkState(
    openWithFingerprint("fingerprint-terminal"),
    terminalEvent(),
    new Map(),
  );
  assert.equal(sameFingerprint.outcome, "applied");
  assert.equal(sameFingerprint.snapshot.state.pendingOperations[0]?.status, "succeeded");

  for (const blank of ["", "   ", "​"]) {
    const blanked = reduceTaskWorkState(
      openWithFingerprint(blank),
      terminalEvent({ canonicalFingerprint: "F2" }),
      new Map(),
    );
    assert.equal(blanked.outcome, "applied", JSON.stringify(blank));
    assert.equal(blanked.snapshot.state.pendingOperations[0]?.terminalFingerprint, "F2");
  }

  // 入ってくる側が `unknown` に倒れる経路では指紋を書かないので上書きも起きない。ここで
  // 隔離すると、`terminalEvidenceContradicts` は event 自身の性質で還元器は純関数なので、
  // 同じ event が毎回同じ判定で戻り、その operation は永久に閉じられない
  const ambiguous = reduceTaskWorkState(
    openWithFingerprint("F1"),
    terminalEvent({ canonicalFingerprint: "F2", successful: undefined }),
    new Map(),
  );
  assert.equal(ambiguous.outcome, "applied");
  assert.equal(ambiguous.snapshot.state.pendingOperations[0]?.status, "unknown");
  assert.equal(ambiguous.snapshot.state.pendingOperations[0]?.terminalFingerprint, "F1");

  // 通す側も非発火側も rule 2 で固定する（rule 1 限定の変異はここでも落ちる）
  const matchKeyStart = startEvent({ operation: MATCH_KEY_ONLY });
  const sameByMatchKey = reduceTaskWorkState(
    openWithFingerprint("fingerprint-terminal", "started", matchKeyStart),
    matchKeyTerminal(),
    new Map(),
  );
  assert.equal(sameByMatchKey.snapshot.state.pendingOperations[0]?.status, "succeeded");
  const ambiguousByMatchKey = reduceTaskWorkState(
    openWithFingerprint("F1", "started", matchKeyStart),
    matchKeyTerminal({ canonicalFingerprint: "F2", successful: undefined }),
    new Map(),
  );
  assert.equal(ambiguousByMatchKey.outcome, "applied");
  assert.equal(ambiguousByMatchKey.snapshot.state.pendingOperations[0]?.status, "unknown");
  assert.equal(ambiguousByMatchKey.snapshot.state.pendingOperations[0]?.terminalFingerprint, "F1");
});

test("droppedEvidence を持たない状態も読めて、必要になったら欄が生える（#43 FR-013/FR-014）", () => {
  // 任意欄なので、この版より前の checkpoint と別実装の状態には無い。無いことを
  // 「記録できない」と読むと、退避が黙って消える側に倒れる
  const old = filledSnapshot(true);
  assert.equal(old.state.droppedEvidence, undefined);
  const result = reduceTaskWorkState(old, startEvent(), new Map());
  assert.equal(result.snapshot.state.droppedEvidence?.length, 1);
  // 何も落ちていないなら空配列も作らない（空配列と欄なしが同じ意味の欄を 2 通り書かない）
  const quiet = reduceTaskWorkState(emptySnapshot(), startEvent(), new Map());
  assert.equal(quiet.snapshot.state.droppedEvidence, undefined);
});

test("語彙外の turnIdSource は turn 同一性の検査で落とす", () => {
  // 語彙は凍結 schema にしかなく、参照模型は event が schema 検証を通ってから届くとは限らない。
  // 語彙外の綴りは `unavailable` の分岐にも intake の `native` 証明要求にも当たらないので、
  // **降格を丸ごと迂回して自称 `turnId` を保持できた**（実測: "Native" / "NATIVE" / 末尾空白の
  // "native " / キリル а の同形異字 / "bogus" が診断ゼロで通った）。そのまま rule 2 に入ると
  // `sameTurnOf` は turnId の等値、`eligibleOf` は自己一致で通るので turn scope が丸ごと成立する
  for (const forged of ["Native", "NATIVE", "native ", "n\u0430tive", "bogus", ""]) {
    assert.throws(
      () => assertTurnIdentity(startEvent({ turnIdSource: forged as never, turnId: "turn-forged" })),
      /turnIdSource が語彙外/,
      `語彙外の ${JSON.stringify(forged)} が通った`,
    );
  }
  // 締めすぎていないこと: 語彙内の 3 つは通る
  assertTurnIdentity(startEvent({ turnIdSource: "native", turnId: "turn-1" }));
  assertTurnIdentity(startEvent({ turnIdSource: "synthesized_monotonic", turnId: "turn-1" }));
  assertTurnIdentity(startEvent({ turnIdSource: "unavailable", turnId: undefined }));
});

test("語彙外の turnIdSource は還元器と公開入口の 3 つとも受け取らない", () => {
  const forged = startEvent({ turnIdSource: "Native" as never, turnId: "turn-forged" });
  assert.throws(() => reduceTaskWorkState(emptySnapshot(), forged, new Map()), /語彙外/);
  assert.throws(() => correlateTerminalEvent(startedSnapshot(), terminalEvent({
    turnIdSource: "Native" as never, turnId: "turn-forged",
  })), /語彙外/);
  assert.throws(() => finalizeAbandonedState(startedSnapshot().state, startEvent({
    kind: "session_ended", operation: undefined, eventId: "event-abandon",
    adapterDeliveryId: "delivery-abandon", turnIdSource: "Native" as never, turnId: "turn-forged",
  }), new Map()), /語彙外/);
});

test("proven でない version の native turn 主張は unavailable へ降格する", () => {
  // §3.1「turnIdSource=native は exact version について proven な native turn identifier を要求する」
  const unproven: IntakeContextV1 = { ...INTAKE, nativeTurnIdentityProven: false };
  const downgrade = stampIntakeEvidence(startEvent(), unproven);
  const stamped = downgrade.event;
  assert.equal(stamped.turnIdSource, "unavailable");
  assert.equal(stamped.turnId, undefined);
  assertTurnIdentity(stamped);
  // §3.1「downgrade の理由は doctor が報告する」。黙って降格しない
  assert.deepEqual(
    downgrade.diagnostics.map((d) => d.code),
    ["turn_identity_downgraded"],
  );

  // 降格した start は rule 2 で閉じられない（自作の turnId で turn 両立を満たせない）
  const snapshot = startedSnapshot(stamped);
  const terminal = stampIntakeEvidence(
    terminalEvent({ operation: { ...TERMINAL_OPERATION, nativeOperationId: undefined } }),
    unproven,
  ).event;
  const result = reduceTaskWorkState(snapshot, terminal, new Map());
  assert.deepEqual(
    result.diagnostics.map((d) => d.code),
    ["terminal_unmatched"],
  );
});

test("証明は version に紐づく: 認証されない名乗りは native turn を保てない", () => {
  // nativeTurnIdentityProven は「その exact version について」の事実なので、
  // その version であること自体が認証されていない event には適用できない
  const cases: ReadonlyArray<readonly [string, NormalizedContinuityEvent, IntakeContextV1]> = [
    [
      "version が exact でない",
      startEvent({ provenance: { ...startEvent().provenance, sourceAgentVersion: "2.1.228" } }),
      INTAKE,
    ],
    // Agent 名の食い違いはここには無い（intake が受け取らないので降格の対象にならない）。
    // 認証できない経路として残るのは「受領証側が Agent を名乗っていない」形
    ["受領証が Agent を名乗らない", startEvent(), { ...INTAKE, expectedSourceAgent: "" }],
    ["受領証を出せない経路", startEvent(), { ...INTAKE, attestation: undefined }],
  ];
  for (const [label, event, context] of cases) {
    const stamped = stampIntakeEvidence(event, context).event;
    assert.equal(stamped.turnIdSource, "unavailable", label);
    assert.equal(stamped.turnId, undefined, label);
    assertTurnIdentity(stamped);
  }
});

test("proven な version の native turn 主張と adapter 側の monotonic turn は触らない", () => {
  // 降格が「native を名乗る全部」を潰していないことを、通るべき側で確かめる
  assert.equal(stampIntakeEvidence(startEvent(), INTAKE).event.turnIdSource, "native");
  assert.equal(stampIntakeEvidence(startEvent(), INTAKE).event.turnId, "turn-1");
  const monotonic = startEvent({ turnIdSource: "synthesized_monotonic", turnId: "turn-7" });
  const unproven: IntakeContextV1 = { ...INTAKE, nativeTurnIdentityProven: false };
  assert.equal(stampIntakeEvidence(monotonic, unproven).event.turnIdSource, "synthesized_monotonic");
  assert.equal(stampIntakeEvidence(monotonic, unproven).event.turnId, "turn-7");
  // capabilityHash の不一致は evidence を降格させるが turn は降格させない。capability matrix に
  // turn identity の cell が無い（#40）ので、hash は turn について何も語らないため
  const staleHash = startEvent({
    provenance: { ...startEvent().provenance, capabilityHash: `sha256:${"5".repeat(64)}` },
  });
  assert.equal(stampIntakeEvidence(staleHash, INTAKE).event.provenance.evidenceKind, "synthesized");
  assert.equal(stampIntakeEvidence(staleHash, INTAKE).event.turnIdSource, "native");
});

test("terminal 済みが無くても start は取り込む（枠が埋まっても詰まらせない）", () => {
  // 落とせるものが無いとき隔離すると、unknown を消す経路が他に無いので枠が永久に埋まり、
  // 以後どの tool 呼び出しも記録できなくなる（訂正版の無い隔離を adapter が再送し続ける）
  const snapshot = filledSnapshot(false);
  const result = reduceTaskWorkState(snapshot, startEvent(), new Map());
  assert.equal(result.outcome, "applied");
  assert.deepEqual(
    result.diagnostics.map((d) => d.code),
    ["pending_operations_evicted", "dropped_evidence_recorded"],
  );
  assert.equal(result.snapshot.state.pendingOperations.length, CONTINUITY_LIMITS.arrayItems);
  // 落とすのは最古の 1 件だけで、新しい start は必ず入る
  assert.equal(result.snapshot.state.pendingOperations.at(-1)?.correlation.startEventId, "event-start");
  assert.equal(
    result.snapshot.state.pendingOperations.some(
      (pending) => pending.operationId === snapshot.state.pendingOperations[0]?.operationId,
    ),
    false,
  );
});

test("canonicalInputHash が食い違う terminal は隔離する", () => {
  const snapshot = startedSnapshot();
  const conflicting = terminalEvent({
    operation: { ...TERMINAL_OPERATION, canonicalInputHash: "input-hash-other" },
  });
  const ledger: IdempotencyLedger = new Map();
  const result = reduceTaskWorkState(snapshot, conflicting, ledger);
  assert.equal(result.outcome, "quarantined");
  assert.deepEqual(
    result.diagnostics.map((d) => d.code),
    ["terminal_conflict"],
  );
  // 状態も台帳も動かない。動かすと訂正版の再配送が重複 no-op として捨てられる
  assert.equal(result.snapshot, snapshot);
  assert.equal(result.ledger, ledger);
  assert.equal(result.ledger.size, 0);
  assert.equal(result.snapshot.history.length, snapshot.history.length);
});

test("turnIdSource の種別が違えば rule 2 では閉じない", () => {
  const start = startEvent({
    operation: { phase: "start", operationMatchKey: "match-key-1", operationKind: "Bash" },
  });
  const snapshot = startedSnapshot(start);
  const terminal = terminalEvent({
    turnIdSource: "synthesized_monotonic",
    operation: { phase: "terminal", operationMatchKey: "match-key-1", operationKind: "Bash" },
  });
  const result = reduceTaskWorkState(snapshot, terminal, new Map());
  assert.deepEqual(
    result.diagnostics.map((d) => d.code),
    ["terminal_unmatched"],
  );
  // 理由は「turn 同一性が無い」ではなく「種別が違う」。§3.1 は降格の理由を doctor が
  // 報告することを求めているので取り違えない
  assert.match(result.diagnostics[0]?.detail ?? "", /turnIdSource/);
  // unknown に倒すのは種別が違う候補だけ。同じ matchKey で turn が違う open は巻き込まない
  const other = reduceTaskWorkState(
    snapshot,
    startEvent({
      eventId: "start-other", adapterDeliveryId: "d-other", canonicalFingerprint: "f-other", ingestSeq: "13",
      turnId: "turn-9",
      operation: { phase: "start", operationMatchKey: "match-key-1", operationKind: "Bash" },
    }),
    new Map(),
  );
  const scoped = reduceTaskWorkState(other.snapshot, terminal, other.ledger);
  assert.deepEqual(
    scoped.snapshot.state.pendingOperations.map((p) => p.status),
    ["unknown", "started"],
  );
});

test("確定済みの成否と矛盾する 2 度目の terminal は隔離する", () => {
  // 配送 ID が違う 2 通目は dedupe で内容を比べられず、identity 衝突検査も kind と input hash
  // しか見ないので、成否だけが逆の terminal が「適用済み」として黙って通っていた。受理済み
  // terminal の source hash は状態に持っていない（#43）が、確定した status は持っている
  const snapshot = startedSnapshot();
  const closed = reduceTaskWorkState(snapshot, terminalEvent(), new Map());
  const again = reduceTaskWorkState(
    closed.snapshot,
    terminalEvent({
      eventId: "event-terminal-2",
      adapterDeliveryId: "delivery-terminal-2",
      ingestSeq: "13",
      successful: false,
    }),
    closed.ledger,
  );
  assert.equal(again.outcome, "quarantined");
  assert.deepEqual(
    again.diagnostics.map((d) => d.code),
    ["terminal_conflict"],
  );
  assert.equal(again.snapshot.state.pendingOperations[0]?.status, "succeeded");
  assert.equal(again.ledger, closed.ledger);
});

test("空文字の turnId は schema violation", () => {
  // schema は maxLength しか課さないので空文字が届く。「turn がある」と読むと §4.3 rule 2 の
  // turn 同一性が空文字同士で成立し、無関係な turn の operation を閉じる。unavailable な turn を
  // 全部空文字で表す adapter では、全 operation が 1 つの turn に潰れる
  assert.throws(() => reduceTaskWorkState(emptySnapshot(), startEvent({ turnId: "" }), new Map()), /turnId が空文字/);
  assert.throws(
    () =>
      reduceTaskWorkState(
        emptySnapshot(),
        startEvent({ turnIdSource: "synthesized_monotonic", turnId: "" }),
        new Map(),
      ),
    /turnId が空文字/,
  );
  // unavailable は従来どおり turnId 不在を要求する（空文字での代用も落ちる）
  assert.throws(
    () => reduceTaskWorkState(emptySnapshot(), startEvent({ turnIdSource: "unavailable", turnId: "" }), new Map()),
    /turnId がある/,
  );
});

test("空文字の eventId は schema violation", () => {
  // deriveOperationId(eventId, matchKey) の材料なので、空文字だと同じ turn の rule 2 の start が
  // 2 件とも同じ operationId になり、2 件目が duplicate_operation_start として消える
  assert.throws(
    () => reduceTaskWorkState(emptySnapshot(), startEvent({ eventId: "" }), new Map()),
    /eventId が空文字/,
  );
  assert.throws(
    () =>
      finalizeAbandonedState(
        startedSnapshot().state,
        startEvent({ eventId: "", kind: "session_ended", ingestSeq: "4", operation: undefined }),
        new Map(),
      ),
    /eventId が空文字/,
  );
});

test("空文字の sessionId は schema violation", () => {
  // 候補選びも放棄も session で絞るので、空文字だと session を特定できない adapter の event が
  // 全部同じ scope に入り、別 session の terminal が診断ゼロで operation を閉じる。
  // 実 ID なら terminal_orphaned で隔離される（control）
  assert.throws(
    () => reduceTaskWorkState(emptySnapshot(), startEvent({ sessionId: "" }), new Map()),
    /sessionId が空文字/,
  );
  const started = startedSnapshot(startEvent({ sessionId: "session-A" }));
  const crossSession = reduceTaskWorkState(
    started,
    terminalEvent({ sessionId: "session-B" }),
    new Map(),
  );
  assert.equal(crossSession.outcome, "quarantined");
  assert.deepEqual(
    crossSession.diagnostics.map((d) => d.code),
    ["terminal_orphaned", "dropped_evidence_recorded"],
  );
});

test("構成要素が消えても sensitivity は下がらない", () => {
  // §10「sensitivity は構成要素 event の最大機密度を常に反映する」。集約値を実体に持たせる理由が
  // 「raw event の TTL 後には遡って判定できない」ことなので、構成要素が状態から消えても下げない
  // （retainPendingOperations の退避は保管上の都合で、機密でなくなった証跡ではない）
  const snapshot = emptySnapshot({ sensitivity: "secret" });
  assert.deepEqual(
    snapshot.state.activeFiles.filter((file) => file.sensitivity === "secret"),
    [],
  );
  const applied = reduceTaskWorkState(snapshot, startEvent(), new Map());
  assert.equal(applied.snapshot.state.sensitivity, "secret");
});

test("放棄の kind を reduceTaskWorkState に渡すと落ちる", () => {
  // operation envelope を持たないので汎用 commit に落ちて状態を変えないまま台帳の鍵だけを
  // 消費する。その台帳で finalizeAbandonedState を呼ぶと重複として捨てられ、放棄が永久に
  // 適用されない（operation が started のまま残って状態が嘘をつく）
  const abandon = startEvent({
    eventId: "event-abandon",
    adapterDeliveryId: "delivery-abandon",
    kind: "session_ended",
    ingestSeq: "12",
    operation: undefined,
  });
  assert.throws(
    () => reduceTaskWorkState(startedSnapshot(), abandon, new Map()),
    /finalizeAbandonedState に渡す/,
  );
  // 正しい入口に渡せば放棄は効く
  const finalized = finalizeAbandonedState(startedSnapshot().state, abandon, new Map());
  assert.equal(finalized.outcome, "applied");
  assert.equal(finalized.state.pendingOperations[0]?.status, "unknown");
});

test("閉じた operation に届いた自己矛盾 terminal も証跡の矛盾を出す", () => {
  // 矛盾は照合の成否と無関係な event 自身の性質。照合できた場合しか出さないと、同じ壊れた
  // adapter でも operation が既に閉じているときだけ terminal_already_applied に埋もれる
  const snapshot = startedSnapshot();
  const closed = reduceTaskWorkState(snapshot, terminalEvent(), new Map());
  const again = reduceTaskWorkState(
    closed.snapshot,
    terminalEvent({
      eventId: "event-terminal-2",
      adapterDeliveryId: "delivery-terminal-2",
      ingestSeq: "13",
      kind: "tool_failed",
      successful: true,
    }),
    closed.ledger,
  );
  assert.deepEqual(
    again.diagnostics.map((d) => d.code),
    ["terminal_already_applied", "terminal_evidence_contradicts"],
  );
  assert.equal(again.snapshot.state.pendingOperations[0]?.status, "succeeded");
});

test("identity が一致する兄弟がいれば terminal を隔離しない", () => {
  // §4.3 どおりに matchKey を導出しない adapter では、同じ matchKey で input hash が違う
  // pending が並ぶ。兄弟の identity を根拠に隔離すると live な operation が永久に閉じない
  const first = startEvent({ eventId: "event-start-a", operation: MATCH_KEY_ONLY });
  const firstTerminal = terminalEvent({
    eventId: "event-terminal-a",
    adapterDeliveryId: "delivery-terminal-a",
    ingestSeq: "12",
    operation: { ...MATCH_KEY_ONLY, phase: "terminal" },
  });
  const second = startEvent({
    eventId: "event-start-b",
    adapterDeliveryId: "delivery-start-b",
    ingestSeq: "13",
    operation: { ...MATCH_KEY_ONLY, canonicalInputHash: "input-hash-2" },
  });
  // 1 件目を閉じてから 2 件目を積む。この順なら候補は「確定済み 1 + 未確定 1」になり、
  // 2 件目の terminal は rule 2 で一意に閉じられる（両方 open だと ambiguous で unknown に倒れる）
  const both = apply(emptySnapshot(), [first, firstTerminal, second]);
  assert.deepEqual(
    both.snapshot.state.pendingOperations.map((pending) => pending.status),
    ["succeeded", "started"],
  );

  const closed = reduceTaskWorkState(
    both.snapshot,
    terminalEvent({
      eventId: "event-terminal-b",
      adapterDeliveryId: "delivery-terminal-b",
      ingestSeq: "14",
      operation: { ...MATCH_KEY_ONLY, phase: "terminal", canonicalInputHash: "input-hash-2" },
    }),
    both.ledger,
  );
  assert.deepEqual(
    closed.snapshot.state.pendingOperations.map((pending) => pending.status),
    ["succeeded", "succeeded"],
  );
  assert.deepEqual(closed.diagnostics, []);

  // どの候補とも identity が合わない terminal は従来どおり隔離する
  const alien = reduceTaskWorkState(
    both.snapshot,
    terminalEvent({
      eventId: "event-terminal-x",
      adapterDeliveryId: "delivery-terminal-x",
      ingestSeq: "15",
      operation: { ...MATCH_KEY_ONLY, phase: "terminal", canonicalInputHash: "input-hash-9" },
    }),
    both.ledger,
  );
  assert.equal(alien.outcome, "quarantined");
  assert.deepEqual(
    alien.diagnostics.map((d) => d.code),
    ["terminal_conflict"],
  );
});

test("同じ成否を名乗る 2 度目の terminal は適用済みとして扱う", () => {
  const snapshot = startedSnapshot();
  const closed = reduceTaskWorkState(snapshot, terminalEvent(), new Map());
  const again = reduceTaskWorkState(
    closed.snapshot,
    terminalEvent({ eventId: "event-terminal-2", adapterDeliveryId: "delivery-terminal-2", ingestSeq: "13" }),
    closed.ledger,
  );
  assert.deepEqual(
    again.diagnostics.map((d) => d.code),
    ["terminal_already_applied"],
  );
  assert.equal(again.snapshot.state.pendingOperations[0]?.status, "succeeded");
});

test("成否が違う兄弟が確定済みでも、成否が一致する再配送は隔離しない", () => {
  // rule 2 の候補は同じ matchKey の兄弟をまとめて拾う。同じ turn で同じ tool を同じ入力で
  // 2 回動かして成否が分かれると、片方の terminal の再配送がもう片方の成否を根拠に隔離される。
  // 隔離は台帳に入らないので adapter は無限再送になる
  const first = startEvent({ eventId: "event-start-a", operation: MATCH_KEY_ONLY });
  const firstTerminal = terminalEvent({
    eventId: "event-terminal-a",
    adapterDeliveryId: "delivery-terminal-a",
    ingestSeq: "12",
    operation: { ...MATCH_KEY_ONLY, phase: "terminal" },
  });
  const second = startEvent({ eventId: "event-start-b", adapterDeliveryId: "delivery-start-b", ingestSeq: "13", operation: MATCH_KEY_ONLY });
  const secondTerminal = terminalEvent({
    eventId: "event-terminal-b",
    adapterDeliveryId: "delivery-terminal-b",
    ingestSeq: "14",
    successful: false,
    kind: "tool_failed",
    operation: { ...MATCH_KEY_ONLY, phase: "terminal" },
  });
  const settled = apply(emptySnapshot(), [first, firstTerminal, second, secondTerminal]);
  assert.deepEqual(
    settled.snapshot.state.pendingOperations.map((pending) => pending.status),
    ["succeeded", "failed"],
  );

  const again = reduceTaskWorkState(
    settled.snapshot,
    terminalEvent({
      eventId: "event-terminal-a-2",
      adapterDeliveryId: "delivery-terminal-a-2",
      ingestSeq: "15",
      operation: { ...MATCH_KEY_ONLY, phase: "terminal" },
    }),
    settled.ledger,
  );
  assert.deepEqual(
    again.diagnostics.map((d) => d.code),
    ["terminal_already_applied"],
  );
  assert.deepEqual(
    again.snapshot.state.pendingOperations.map((pending) => pending.status),
    ["succeeded", "failed"],
  );
});

test("成否を主張しない 2 度目の terminal は矛盾ではない", () => {
  // unknown は「成否を主張していない」なので、確定済みの succeeded と矛盾しない。
  // 矛盾扱いにすると、成否を出さない adapter の再送が全部隔離されて無限再送になる
  const snapshot = startedSnapshot();
  const closed = reduceTaskWorkState(snapshot, terminalEvent(), new Map());
  const again = reduceTaskWorkState(
    closed.snapshot,
    terminalEvent({
      eventId: "event-terminal-3",
      adapterDeliveryId: "delivery-terminal-3",
      ingestSeq: "13",
      successful: undefined,
    }),
    closed.ledger,
  );
  assert.deepEqual(
    again.diagnostics.map((d) => d.code),
    ["terminal_already_applied"],
  );
  assert.equal(again.snapshot.state.pendingOperations[0]?.status, "succeeded");
});

test("空の canonicalFingerprint は schema violation", () => {
  // dedupe authority は adapterDeliveryId、無ければ canonical fingerprint。空文字を「値がある」と
  // 読むと、配送 ID を持たない event が全部 1 つの鍵に潰れ、配送 ID を持つ event は台帳に
  // source hash 無しで載って訂正版が衝突検査を素通りする
  assert.throws(
    () => reduceTaskWorkState(emptySnapshot(), startEvent({ canonicalFingerprint: "" }), new Map()),
    /canonicalFingerprint が空文字/,
  );
  assert.throws(
    () =>
      finalizeAbandonedState(
        startedSnapshot().state,
        startEvent({ eventId: "event-abandon", kind: "session_ended", ingestSeq: "4", operation: undefined, canonicalFingerprint: "" }),
        new Map(),
      ),
    /canonicalFingerprint が空文字/,
  );
});

test("成否を主張しない terminal は unknown を確定する", () => {
  const snapshot = startedSnapshot();
  const result = reduceTaskWorkState(snapshot, terminalEvent({ successful: undefined }), new Map());
  assert.equal(result.snapshot.state.pendingOperations[0]?.status, "unknown");
});

test("失敗の terminal は failed を確定する", () => {
  const snapshot = startedSnapshot();
  const result = reduceTaskWorkState(
    snapshot,
    terminalEvent({ kind: "tool_failed", successful: false }),
    new Map(),
  );
  assert.equal(result.snapshot.state.pendingOperations[0]?.status, "failed");
});

// --- §4.3 abandonment -------------------------------------------------------

test("放棄時に開いていた operation は unknown になり、元の状態は変わらない", () => {
  const snapshot = startedSnapshot();
  deepFreeze(snapshot.state);
  const abandonEvent = terminalEvent({
    eventId: "event-session-ended",
    kind: "session_ended",
    ingestSeq: "20",
    operation: undefined,
    successful: undefined,
  });
  const first = finalizeAbandonedState(snapshot.state, abandonEvent, new Map());
  const finalized = first.state;
  assert.equal(first.outcome, "applied");
  assert.equal(finalized.pendingOperations[0]?.status, "unknown");
  // 放棄も証跡を残す。何が unknown を確定させたかを状態から辿れるようにする
  assert.deepEqual(finalized.pendingOperations[0]?.sourceEventIds, [
    "event-start",
    "event-session-ended",
  ]);
  // 再実行の可否は放棄では緩めない
  assert.equal(finalized.pendingOperations[0]?.replayPolicy, "never_auto");
  assert.equal(snapshot.state.pendingOperations[0]?.status, "started");
  assert.notEqual(finalized.stateRevision, snapshot.state.stateRevision);
  assert.equal(finalized.lastIngestSeq, "20");

  // §4.2「重複した論理 event は no-op」。放棄経路も台帳を見る
  const again = finalizeAbandonedState(finalized, abandonEvent, first.ledger);
  assert.equal(again.outcome, "duplicate");
  assert.equal(again.state.stateRevision, finalized.stateRevision);
});

test("別 lineage の event では放棄を確定しない", () => {
  const snapshot = startedSnapshot();
  assert.throws(
    () =>
      finalizeAbandonedState(
        snapshot.state,
        terminalEvent({
          eventId: "event-session-ended",
          kind: "session_ended",
          taskLineageId: "lineage-2",
          ingestSeq: "20",
          operation: undefined,
          successful: undefined,
        }),
        new Map(),
      ),
    /別 lineage の event は適用しない/,
  );
});

test("放棄後に届いた terminal は元の operation を閉じる", () => {
  const snapshot = startedSnapshot();
  const abandoned = finalizeAbandonedState(
    snapshot.state,
    terminalEvent({
      eventId: "event-session-ended",
      kind: "session_ended",
      ingestSeq: "20",
      operation: undefined,
      successful: undefined,
    }),
    new Map(),
  ).state;
  const late = terminalEvent({ eventId: "event-terminal-late", ingestSeq: "30" });
  const result = reduceTaskWorkState(
    { ...snapshot, state: abandoned },
    late,
    new Map(),
  );
  assert.equal(result.snapshot.state.pendingOperations[0]?.status, "succeeded");
  assert.equal(result.snapshot.state.pendingOperations[0]?.operationId, snapshot.state.pendingOperations[0]?.operationId);
});

// --- 集約値 -----------------------------------------------------------------

test("sensitivity は構成要素の最大を取る", () => {
  const snapshot = startedSnapshot();
  // pendingOperation が private なので集約も private
  assert.equal(snapshot.state.sensitivity, "private");

  const withSecret = emptySnapshot({
    recentCommands: [
      {
        operationId: "op-secret",
        commandDisplay: "redacted",
        status: "unknown",
        sourceEventIds: ["event-x"],
        observedAt: "2026-08-16T00:00:00Z",
        evidenceKind: "native",
        sensitivity: "secret",
      },
    ],
  });
  const result = reduceTaskWorkState(withSecret, startEvent(), new Map());
  assert.equal(result.snapshot.state.sensitivity, "secret");
});

test("語彙外の sensitivity は最上位に倒す", () => {
  // indexOf の -1 をそのまま順位に使うと最下位（normal）に落ちる = fail open。
  // 語彙外は「機密度不明」なので、自動 resume を止める側へ倒す
  const foreign = emptySnapshot({
    recentCommands: [
      {
        operationId: "op-foreign",
        commandDisplay: "redacted",
        status: "unknown",
        sourceEventIds: ["event-x"],
        observedAt: "2026-08-16T00:00:00Z",
        evidenceKind: "native",
        sensitivity: "top-secret" as never,
      },
    ],
  });
  const result = reduceTaskWorkState(foreign, startEvent(), new Map());
  assert.equal(result.snapshot.state.sensitivity, "secret");
});

test("optional が全部無い状態も hash できる", () => {
  // canonicalizeJson は undefined を拒否する。欠けている optional を undefined のまま
  // 載せていないことの確認
  const { stateRevision: _revision, ...content } = emptyState();
  assert.match(contentHashOf(content), /^[0-9a-f]{64}$/);
});

// --- fixture parity ---------------------------------------------------------

interface ReductionFixture {
  intakeContext: IntakeContextV1;
  initialState: CanonicalWorkStateV1;
  events: NormalizedContinuityEvent[];
  expected: Array<{
    eventId: string;
    evidenceKind: string;
    outcome: string;
    contentHash: string;
    stateRevision: string;
    historyLength: number;
    pendingStatuses: string[];
    droppedEvidenceReasons: string[];
    diagnostics: string[];
  }>;
}

interface RejectionFixture {
  intakeContext: IntakeContextV1;
  cases: Array<{
    name: string;
    // `intake-diagnostic` は**棄却しない層**。intake は event をそのまま通し、診断だけを出す。
    // 参照実装の parity 検査（`tool-lifecycle-reduction.json`）は intake の返り値のうち `event` しか
    // 使わないので、診断だけを出す規則は fixture に置かないと**移植が一度も実装せずに全 fixture を
    // 満たせる**（#42 の session 束縛で同じ穴を塞いだのと同じ形）
    rejectedBy: "schema" | "runtime" | "intake" | "intake-reject" | "intake-diagnostic";
    reason: string;
    expectedDiagnostics?: string[];
    intakeOverride?: Partial<IntakeContextV1>;
    event: NormalizedContinuityEvent;
  }>;
}

test("negative fixture は宣言した層で落ちる", () => {
  const fixture = readIJsonFile<RejectionFixture>(
    new URL("../fixtures/continuity/invalid/rejected-events.json", import.meta.url),
  );
  assert.equal(fixture.cases.length > 0, true);
  // fixture の `intakeContext` は JSON なので型検査を受けない。必須欄が欠けると `undefined` が
  // 入り、`isBlank(undefined)` は文字列 "undefined" を検査して **false** を返すので、束縛が
  // 「実在する値」として有効になり全 event が落ちる。落ち方は `§3.1 違反: … は undefined なのに`
  // という config でなく event を名指しするエラーなので、欠落そのものを先に見る
  for (const field of ["expectedSourceAgent", "expectedSessionId", "exactAgentVersion"] as const) {
    assert.equal(typeof fixture.intakeContext[field], "string", `intakeContext.${field} が無い`);
  }
  const layers = new Set<string>();
  for (const testCase of fixture.cases) {
    layers.add(testCase.rejectedBy);
    const issues = validateContractValue(
      "NormalizedContinuityEvent",
      testCase.event,
      SCHEMA_ROOT,
      CONTINUITY_LIMITS,
    );
    if (testCase.rejectedBy === "schema") {
      assert.notEqual(issues.length, 0, testCase.name);
      continue;
    }
    // #29 の前提: 残りは schema では落ちない。落ちるようになったら runtime 側の検査が
    // 要らなくなるので、その事実ごと壊れるべき
    assert.deepEqual(issues, [], testCase.name);
    // runtime 層も intake 層も、受け持つ不変条件は §3.1（identity / envelope）と
    // §22.6（timestamp の実在、#27）の 2 節にまたがる。**節は case ごとに fixture の `reason` から
    // 取る**。節を固定で書くと、fixture が case ごとに節を宣言している唯一の場所（`reason`）を
    // test が一度も読まない。実測でそうなっていた: 和集合の `/§(3\.1|22\.6) 違反/` を全 case へ
    // 当てていたときは、§22.6 の case の occurredAt を実在する日付に戻しても緑のままで、
    // §22.6 の強制が negative fixture の被覆から丸ごと消えていた。
    // **`intake-reject` 側も同じ導出にする**。こちらだけ `/§3.1 違反/` を固定で書いていたので、
    // §22.6 を名乗る intake の case（受領証の attestedAt）は fixture として書けず、書く層の検査が
    // parity の被覆から外れていた
    const section = /^§([\d.]+)/.exec(testCase.reason)?.[1];
    assert.notEqual(section, undefined, `fixture の reason が節で始まっていない: ${testCase.name}`);
    // `replace` は String 引数だと**先頭 1 件しか置換しない**。`3.1.2` のような節が来ると
    // 2 つ目のドットが素のワイルドカードのまま残り、`§3.1X2 違反` にも一致する
    const violates = new RegExp(`§${section?.replaceAll(".", "\\.")} 違反`);
    if (testCase.rejectedBy === "runtime") {
      assert.throws(
        () => reduceTaskWorkState(emptySnapshot(), asStamped(testCase.event), new Map()),
        violates,
        testCase.name,
      );
      continue;
    }
    const context = { ...fixture.intakeContext, ...testCase.intakeOverride };
    // intake は 2 通りの落とし方をする。降格（証跡の質が足りない）と、受け取らない
    // （名乗っている identity が認証済み peer と矛盾する）。fixture でこの 2 つを区別しないと、
    // 降格しか実装しない移植でも fixture が緑になる
    if (testCase.rejectedBy === "intake-reject") {
      assert.throws(() => stampIntakeEvidence(testCase.event, context), violates, testCase.name);
      continue;
    }
    if (testCase.rejectedBy === "intake-diagnostic") {
      const stamped = stampIntakeEvidence(testCase.event, context);
      // 通すこと（種別も turn も書き換えないこと）と、診断の**コード名**の両方を見る。
      // コード名まで見ないと、移植が別の名前で出しても緑になる
      assert.equal(stamped.event.turnIdSource, testCase.event.turnIdSource, testCase.name);
      assert.equal(stamped.event.turnId, testCase.event.turnId, testCase.name);
      assert.deepEqual(
        stamped.diagnostics.map((diagnostic) => diagnostic.code),
        testCase.expectedDiagnostics ?? [],
        testCase.name,
      );
      continue;
    }
    assert.equal(
      stampIntakeEvidence(testCase.event, context).event.provenance.evidenceKind,
      "synthesized",
      testCase.name,
    );
  }
  assert.deepEqual([...layers].sort(), ["intake", "intake-diagnostic", "intake-reject", "runtime", "schema"]);

  // fixture の intakeContext に欠落があると、何をしても synthesized になって intake の case が
  // 素通りする。正当な経路なら native になることを対で確かめる
  const intakeCase = fixture.cases.find((testCase) => testCase.rejectedBy === "intake");
  if (intakeCase === undefined) assert.fail("intake の case が無い");
  const repaired: NormalizedContinuityEvent = {
    ...intakeCase.event,
    provenance: { ...intakeCase.event.provenance, captureMethod: "native_event" },
  };
  assert.equal(stampIntakeEvidence(repaired, fixture.intakeContext).event.provenance.evidenceKind, "native");
});

/**
 * parity fixture は 2 本ある。**旧い形（新しい欄を持たない状態）を消さない**のが要件で
 * （FR-013/FR-014 の証拠がそこにある）、新しい欄を**読む**側は別 fixture で見る。
 * 1 本にまとめると、移植が新しい欄を無視しても片方の hash が合ってしまう。
 */
for (const fixtureId of ["tool-lifecycle-reduction", "restored-state-reduction"]) {
  test(`fixture ${fixtureId} の期待値は参照実装の出力と一致する（TS/Rust parity の基準）`, () => {
    const fixture = readIJsonFile<ReductionFixture>(
      new URL(`../fixtures/continuity/${fixtureId}.json`, import.meta.url),
    );
    let snapshot: TaskWorkStateSnapshotV1 = {
      state: fixture.initialState,
      history: [],
    };
    let ledger: IdempotencyLedger = new Map();
    const actual = fixture.events.map((raw) => {
      const event = stampIntakeEvidence(raw, fixture.intakeContext).event;
      const result = reduceTaskWorkState(snapshot, event, ledger);
      snapshot = result.snapshot;
      ledger = result.ledger;
      return {
        eventId: event.eventId,
        evidenceKind: event.provenance.evidenceKind,
        outcome: result.outcome,
        contentHash: result.contentHash,
        stateRevision: result.snapshot.state.stateRevision,
        historyLength: result.snapshot.history.length,
        pendingStatuses: result.snapshot.state.pendingOperations.map((p) => p.status),
        // hash に含まれてはいるが、合わなかったときにどこが違うかを移植側に見せる
        // （`pendingStatuses` と同じ役割）
        droppedEvidenceReasons: (result.snapshot.state.droppedEvidence ?? []).map((e) => e.reason),
        diagnostics: result.diagnostics.map((d) => d.code),
      };
    });
    assert.deepEqual(actual, fixture.expected);
    assert.equal(fixture.expected.length > 0, true);
  });
}

// --- code-review 指摘の回帰（51a339c で実測された経路） ----------------------

test("還元後の状態は凍結 schema に適合する（terminal が何度届いても）", () => {
  // successful を持たない terminal は unknown を確定するが、unknown は open のままなので
  // 同じ operation に何度でも再照合される。上限を見ずに append すると、還元器自身が
  // sourceEventIds 256 件超の状態を出して parity の基準にならなくなる
  let snapshot = startedSnapshot();
  let ledger: IdempotencyLedger = new Map();
  let truncations = 0;
  for (let attempt = 0; attempt < 300; attempt += 1) {
    const result = reduceTaskWorkState(
      snapshot,
      terminalEvent({
        eventId: `event-terminal-${attempt}`,
        adapterDeliveryId: `delivery-terminal-${attempt}`,
        ingestSeq: `${100 + attempt}`,
        successful: undefined,
      }),
      ledger,
    );
    snapshot = result.snapshot;
    ledger = result.ledger;
    truncations += result.diagnostics.filter((d) => d.code === "source_events_truncated").length;
  }
  const pending = snapshot.state.pendingOperations[0] as PendingOperation;
  assert.equal(pending.status, "unknown");
  assert.equal(pending.sourceEventIds.length, CONTINUITY_LIMITS.arrayItems);
  // 黙って捨てない
  assert.ok(truncations > 0);
  assert.deepEqual(
    validateContractValue("CanonicalWorkStateV1", snapshot.state, SCHEMA_ROOT, CONTINUITY_LIMITS),
    [],
  );
});

test("別 Agent の event は状態に適用しない", () => {
  // OperationCorrelationV1 は Agent を持たず、scope は sessionId + taskLineageId だけ。
  // ここで束縛しないと、同じ session に居る別 Agent の terminal が他人の operation を閉じる
  const snapshot = startedSnapshot();
  assert.throws(
    () => reduceTaskWorkState(snapshot, terminalEvent({ sourceAgent: "codex" }), new Map()),
    /別 Agent の event は適用しない/,
  );
  // 同じ Agent なら通る
  assert.equal(reduceTaskWorkState(snapshot, terminalEvent(), new Map()).outcome, "applied");
});

test("空の adapterDeliveryId は fingerprint へ落とす", () => {
  // schema は minLength を持たないので空文字が届きうる。throw すると adapter の 1 種類のバグで
  // event stream 全体が止まる
  assert.equal(idempotencyKeyOf(startEvent({ adapterDeliveryId: "" })), "fingerprint-start");
  assert.equal(idempotencyKeyOf(startEvent({ adapterDeliveryId: undefined })), "fingerprint-start");
  assert.throws(
    () => idempotencyKeyOf(startEvent({ adapterDeliveryId: "", canonicalFingerprint: "" })),
    /idempotency key が無い/,
  );
});

test("一致しない nativeOperationId を名乗る terminal は matchKey へ落ちない", () => {
  // rule 1 を名乗った以上 rule 1 で判定する。落とすと、別 operation を診断なしで閉じられる
  const snapshot = startedSnapshot();
  const stray = terminalEvent({
    operation: { ...TERMINAL_OPERATION, nativeOperationId: "toolu_other" },
    successful: false,
  });
  const result = reduceTaskWorkState(snapshot, stray, new Map());
  assert.equal(result.outcome, "quarantined");
  assert.deepEqual(
    result.diagnostics.map((d) => d.code),
    ["terminal_orphaned", "dropped_evidence_recorded"],
  );
  assert.equal(result.snapshot.state.pendingOperations[0]?.status, "started");
});

test("start より先に届いた terminal は台帳に入れない（後から start が来れば閉じられる）", () => {
  // hook と transcript scan の取り込み順、再起動後の catch-up で順序前後は正常に起きる。
  // 候補が 1 件も無い terminal を台帳に入れると、後から start が届いても二度と閉じられない
  const empty = emptySnapshot();
  const early = reduceTaskWorkState(empty, terminalEvent(), new Map());
  assert.equal(early.outcome, "quarantined");
  assert.deepEqual(
    early.diagnostics.map((d) => d.code),
    ["terminal_orphaned", "dropped_evidence_recorded"],
  );
  assert.equal(early.ledger.size, 0);
  // start が届いてから同じ terminal を再配送すれば閉じられる
  const started = reduceTaskWorkState(early.snapshot, startEvent(), early.ledger);
  const closed = reduceTaskWorkState(started.snapshot, terminalEvent(), started.ledger);
  assert.equal(closed.outcome, "applied");
  assert.equal(closed.snapshot.state.pendingOperations[0]?.status, "succeeded");
});

test("start が状態に無い terminal は閉じないが、詰まらせず unknown に倒す", () => {
  // この版より前に書かれた checkpoint には順序材料が無い（#35 の 2 欄は任意）。ここで隔離すると
  // 復元後は全 terminal が隔離され、operation が started のまま二度と閉じられない（resume capsule が
  // 「まだ実行中」と偽る）。閉じずに unknown へ倒し、台帳には入れる
  const started = startedSnapshot();
  const restored: TaskWorkStateSnapshotV1 = withoutStartFacts(started);
  const result = reduceTaskWorkState(restored, terminalEvent(), new Map());
  assert.equal(result.outcome, "applied");
  assert.deepEqual(
    result.diagnostics.map((d) => d.code),
    ["terminal_order_unverifiable"],
  );
  assert.equal(result.snapshot.state.pendingOperations[0]?.status, "unknown");
  assert.equal(result.snapshot.state.pendingOperations[0]?.sourceEventIds.at(-1), "event-terminal");
  assert.equal(result.ledger.size, 1);
});

test("復元後の start 再配送は権威順序の材料を作らない", () => {
  // §6.4 の ingestSeq は event store が採番する watermark なので、再配送 event が運ぶのは
  // 再配送時の取り込み位置であって元の start の権威順序ではない。材料が無いまま閉じるより
  // unknown に倒す（§3.1 の fail closed）
  const started = startedSnapshot();
  const restored: TaskWorkStateSnapshotV1 = withoutStartFacts(started);
  const again = reduceTaskWorkState(restored, startEvent({ ingestSeq: "3" }), new Map());
  assert.deepEqual(
    again.diagnostics.map((d) => d.code),
    ["duplicate_operation_start"],
  );
  assert.equal(again.snapshot.state.pendingOperations[0]?.startIngestSeq, undefined);
  const closed = reduceTaskWorkState(again.snapshot, terminalEvent(), again.ledger);
  assert.deepEqual(
    closed.diagnostics.map((d) => d.code),
    ["terminal_order_unverifiable"],
  );
  assert.equal(closed.snapshot.state.pendingOperations[0]?.status, "unknown");
});

test("eventId が変わった再配送 start でも operation を二重に積まない", () => {
  // 再送契約（上の「同じ配送 ID の再送は…」）では eventId が変わる。台帳だけを失った復元では
  // 導出 operationId が一致しないので、nativeOperationId で拾わないと同じ operation が 2 件になり、
  // rule 1 の terminal が候補 2 件で何も閉じられなくなる
  const started = startedSnapshot();
  const restored: TaskWorkStateSnapshotV1 = withoutStartFacts(started);
  const redelivered = reduceTaskWorkState(
    restored,
    startEvent({ eventId: "event-start-retry-0", ingestSeq: "3" }),
    new Map(),
  );
  assert.deepEqual(
    redelivered.diagnostics.map((d) => d.code),
    ["duplicate_operation_start"],
  );
  assert.equal(redelivered.snapshot.state.pendingOperations.length, 1);
  // 候補が 1 件に保たれていることが要点。閉じる側は順序材料が無いので unknown（fail closed）
  const closed = reduceTaskWorkState(redelivered.snapshot, terminalEvent(), redelivered.ledger);
  assert.equal(closed.snapshot.state.pendingOperations.length, 1);
  assert.equal(closed.snapshot.state.pendingOperations[0]?.status, "unknown");
});

test("operationKind だけが違う再配送 start も隔離する", () => {
  // rule 2 の候補選びが kind の一致を見るのと対称。重複として台帳に入れると訂正版が戻せない
  const started = startedSnapshot();
  const forged = reduceTaskWorkState(
    started,
    startEvent({
      eventId: "event-start-rekind",
      ingestSeq: "3",
      operation: { ...START_OPERATION, operationKind: "Write" },
    }),
    new Map(),
  );
  assert.equal(forged.outcome, "quarantined");
  assert.deepEqual(
    forged.diagnostics.map((d) => d.code),
    ["start_conflict"],
  );
});

test("canonicalInputHash だけが違う再配送 start も隔離する", () => {
  // matchKey の導出が §4.3 どおりでない adapter だと、入力が違っても matchKey は一致しうる。
  // 再配送として台帳に入れると、訂正版が同じ配送 ID で来ても重複 no-op になって戻せない
  const started = startedSnapshot();
  const ledger: IdempotencyLedger = new Map();
  const forged = reduceTaskWorkState(
    started,
    startEvent({
      eventId: "event-start-rehash",
      ingestSeq: "3",
      operation: { ...START_OPERATION, canonicalInputHash: "input-hash-other" },
    }),
    ledger,
  );
  assert.equal(forged.outcome, "quarantined");
  assert.deepEqual(
    forged.diagnostics.map((d) => d.code),
    ["start_conflict"],
  );
  assert.equal(forged.ledger, ledger);
});

test("同じ nativeOperationId で matchKey が違う start は隔離する", () => {
  // 再配送として台帳に入れると、訂正版が同じ配送 ID で来ても重複 no-op になって戻せない
  const started = startedSnapshot();
  const forged = reduceTaskWorkState(
    started,
    startEvent({
      eventId: "event-start-forged",
      ingestSeq: "3",
      operation: { ...START_OPERATION, operationMatchKey: "match-key-other" },
    }),
    new Map(),
  );
  assert.equal(forged.outcome, "quarantined");
  assert.deepEqual(
    forged.diagnostics.map((d) => d.code),
    ["start_conflict"],
  );
  assert.equal(forged.ledger.size, 0);
  assert.equal(forged.snapshot.state.pendingOperations.length, 1);
});

test("放棄を確定しない kind を finalizeAbandonedState に渡せない", () => {
  // routing の取り違えで届いた user_prompted が同 session の実行中 operation を全部 unknown に
  // したうえで冪等キーを消費する事故を、入口で落とす
  const started = startedSnapshot();
  assert.throws(
    () =>
      finalizeAbandonedState(
        started.state,
        startEvent({ eventId: "event-prompt", kind: "user_prompted", ingestSeq: "4", operation: undefined }),
        new Map(),
      ),
    /放棄を確定しない kind/,
  );
});

test("旧 session の session_ended は resume 先の operation を放棄しない", () => {
  // lineage は session をまたいで続く（§5 の checkpoint は sourceSessionId と taskLineageId を
  // 別に持つ）。session を見ないと、遅れて届いた旧 session の放棄が live な operation を潰す
  const started = startedSnapshot();
  const resumed = reduceTaskWorkState(
    started,
    startEvent({
      eventId: "event-start-new-session",
      ingestSeq: "3",
      sessionId: "session-2",
      operation: { ...START_OPERATION, nativeOperationId: "toolu_resumed" },
    }),
    new Map(),
  );
  const result = finalizeAbandonedState(
    resumed.snapshot.state,
    startEvent({ eventId: "event-abandon", kind: "session_ended", ingestSeq: "4", operation: undefined }),
    new Map(),
  );
  const bySession = new Map(
    result.state.pendingOperations.map((p) => [p.correlation.sessionId, p.status]),
  );
  assert.equal(bySession.get("session-1"), "unknown");
  assert.equal(bySession.get("session-2"), "started");
});

test("nativeOperationId が違う 2 回目の呼び出しは別 operation として積む", () => {
  // 再配送の判定に matchKey を使うと、同じ tool を同じ入力で 2 回呼んだだけで 1 件に潰れる
  const started = startedSnapshot();
  const second = reduceTaskWorkState(
    started,
    startEvent({
      eventId: "event-start-2",
      ingestSeq: "3",
      operation: { ...START_OPERATION, nativeOperationId: "toolu_second" },
    }),
    new Map(),
  );
  assert.deepEqual(second.diagnostics, []);
  assert.equal(second.snapshot.state.pendingOperations.length, 2);
});

test("低い ingestSeq を名乗る偽 start で unknown を succeeded に変えられない", () => {
  // 順序材料を持たない状態（#35）に、被害者の identity を写した start を小さい ingestSeq で
  // 送ると、wire の値で順序材料を埋める実装では正規の terminal が順序検査を通って succeeded に
  // 化ける。§14 の zero-tolerance カウンタ `unsafe unknown replay` に直結する
  const started = startedSnapshot();
  const restored: TaskWorkStateSnapshotV1 = withoutStartFacts(started);
  const forged = reduceTaskWorkState(
    restored,
    startEvent({ eventId: "event-start-forged-seq", ingestSeq: "1", sessionId: "session-1" }),
    new Map(),
  );
  const closed = reduceTaskWorkState(forged.snapshot, terminalEvent(), forged.ledger);
  assert.deepEqual(
    closed.diagnostics.map((d) => d.code),
    ["terminal_order_unverifiable"],
  );
  assert.equal(closed.snapshot.state.pendingOperations[0]?.status, "unknown");
});

test("再配送された start は既存の順序材料を上書きしない", () => {
  // 後から来た再配送の ingestSeq で上書きすると、飛行中の terminal が順序違反に見える
  const started = startedSnapshot();
  const again = reduceTaskWorkState(started, startEvent({ ingestSeq: "99" }), new Map());
  const closed = reduceTaskWorkState(again.snapshot, terminalEvent(), again.ledger);
  assert.equal(closed.snapshot.state.pendingOperations[0]?.status, "succeeded");
});

test("turn をまたいだ terminal は matchKey が違っても rule 1 で閉じる", () => {
  // §4.3 は matchKey の入力に「turn when present」を含めるので、turn をまたいだ terminal が
  // start と違う matchKey を持つのは仕様どおり。rule 1 は turn を要求しない（turn 両立は
  // rule 2 の要件）ので、ここで matchKey 一致を求めると背景実行の完了が永久に閉じない
  const snapshot = startedSnapshot();
  const acrossTurn = terminalEvent({
    turnId: "turn-2",
    operation: { ...TERMINAL_OPERATION, operationMatchKey: "match-key-2" },
  });
  const result = reduceTaskWorkState(snapshot, acrossTurn, new Map());
  assert.equal(result.outcome, "applied");
  assert.deepEqual(result.diagnostics, []);
  assert.equal(result.snapshot.state.pendingOperations[0]?.status, "succeeded");
});

test("tool_failed が successful: true を名乗っても succeeded にしない", () => {
  // schema はどちらも valid なので通る。succeeded にすると壊れた adapter が失敗を握り潰せる
  const snapshot = startedSnapshot();
  const result = reduceTaskWorkState(
    snapshot,
    terminalEvent({ kind: "tool_failed", successful: true }),
    new Map(),
  );
  assert.equal(result.snapshot.state.pendingOperations[0]?.status, "unknown");
  assert.deepEqual(
    result.diagnostics.map((d) => d.code),
    ["terminal_evidence_contradicts"],
  );
});

test("terminal 済みの候補に対する identity 衝突も隔離する", () => {
  // 衝突検査が「terminal 済み」の早期 return より後にあると、corrupt な event が台帳へ入り
  // 訂正版の再配送が重複 no-op になる
  const snapshot = startedSnapshot();
  const closed = reduceTaskWorkState(snapshot, terminalEvent(), new Map());
  const forged = reduceTaskWorkState(
    closed.snapshot,
    terminalEvent({
      eventId: "event-terminal-forged",
      ingestSeq: "9",
      adapterDeliveryId: "delivery-forged",
      operation: { ...TERMINAL_OPERATION, canonicalInputHash: "input-hash-other" },
    }),
    closed.ledger,
  );
  assert.equal(forged.outcome, "quarantined");
  assert.deepEqual(
    forged.diagnostics.map((d) => d.code),
    ["terminal_conflict"],
  );
  assert.equal(forged.ledger.size, closed.ledger.size);
});

test("terminal 済みへの同一 identity の再配送は already_applied として台帳に入る", () => {
  // 衝突検査を前に出したせいで正当な再配送まで隔離しては困る
  const snapshot = startedSnapshot();
  const closed = reduceTaskWorkState(snapshot, terminalEvent(), new Map());
  const again = reduceTaskWorkState(
    closed.snapshot,
    terminalEvent({ eventId: "event-terminal-again", ingestSeq: "9", adapterDeliveryId: "delivery-again" }),
    closed.ledger,
  );
  assert.equal(again.outcome, "applied");
  assert.deepEqual(
    again.diagnostics.map((d) => d.code),
    ["terminal_already_applied"],
  );
});

test("tool_failed が successful: false なら矛盾しない", () => {
  const snapshot = startedSnapshot();
  const result = reduceTaskWorkState(
    snapshot,
    terminalEvent({ kind: "tool_failed", successful: false }),
    new Map(),
  );
  assert.equal(result.snapshot.state.pendingOperations[0]?.status, "failed");
  assert.deepEqual(result.diagnostics, []);
});

test("順序が確認できて hash が衝突する terminal は隔離が先に立つ", () => {
  // 権威順序の gate を先に置くと、順序 NG かつ hash 衝突の terminal が台帳へ入り、
  // 訂正版の再配送が重複 no-op として黙って捨てられる
  const snapshot = startedSnapshot();
  const conflicting = terminalEvent({
    ingestSeq: "5",
    operation: { ...TERMINAL_OPERATION, canonicalInputHash: "input-hash-other" },
  });
  const result = reduceTaskWorkState(snapshot, conflicting, new Map());
  assert.equal(result.outcome, "quarantined");
  assert.deepEqual(
    result.diagnostics.map((d) => d.code),
    ["terminal_conflict"],
  );
  assert.equal(result.ledger.size, 0);
});

test("権威順序に反する terminal が unknown にするのは一致した 1 件だけ", () => {
  // 同じ matchKey の start 2 件を別 turn に置く。rule 2 で適格なのは同じ turn の 1 件だけなので、
  // 巻き込みの有無がそのまま見える
  const first = startedSnapshot(startEvent({ operation: MATCH_KEY_ONLY }));
  const both = reduceTaskWorkState(
    first,
    startEvent({
      eventId: "event-start-2",
      adapterDeliveryId: "delivery-start-2",
      ingestSeq: "15",
      turnId: "turn-2",
      operation: MATCH_KEY_ONLY,
    }),
    new Map(),
  ).snapshot;
  const stale = terminalEvent({ ingestSeq: "5", operation: { ...MATCH_KEY_ONLY, phase: "terminal" } });
  const result = reduceTaskWorkState(both, stale, new Map());
  assert.deepEqual(
    result.diagnostics.map((d) => d.code),
    ["terminal_out_of_order"],
  );
  const statuses = result.snapshot.state.pendingOperations.map((pending) => pending.status);
  assert.deepEqual(statuses, ["unknown", "started"]);
  // 閉じられなかった terminal も証跡として残す（§4.3「preserve it as unmatched evidence」）
  assert.deepEqual(result.snapshot.state.pendingOperations[0]?.sourceEventIds, [
    "event-start",
    "event-terminal",
  ]);
  // 巻き込まれなかった側は証跡も付かない
  assert.deepEqual(result.snapshot.state.pendingOperations[1]?.sourceEventIds, ["event-start-2"]);
});

test("Agent 名を名乗らない event 同士で native authority は成立しない", () => {
  const anonymous = startEvent({ sourceAgent: "" });
  const context: IntakeContextV1 = { ...INTAKE, expectedSourceAgent: "" };
  const stamped = stampIntakeEvidence(anonymous, context).event;
  assert.equal(stamped.provenance.evidenceKind, "synthesized");
  assert.equal(stamped.turnIdSource, "unavailable");
});

test("認証済み peer と違う Agent 名を名乗る event は intake が受け取らない", () => {
  // `sourceAgent` は証跡の質ではなく scope selector（`assertSameScope` がこの値の等値で
  // 「どの状態を書き換えてよいか」を決める）。降格しても値は残るので、降格だけでは
  // peer=codex の event が claude の operation を診断ゼロで閉じるのを止められない
  for (const claimed of ["codex", "", " ", "\u{200B}"]) {
    assert.throws(
      () => stampIntakeEvidence(startEvent({ sourceAgent: claimed }), INTAKE),
      /§3.1 違反: 認証済み peer は claude なのに/,
      JSON.stringify(claimed),
    );
  }
});

test("認証済み peer と違う session を名乗る event は intake が受け取らない（#42）", () => {
  // §4.3 の correlation scope は「same session/task lineage」だが、状態は session を持たないので
  // `assertSameScope` では照合できない。intake が束縛しないと誰も束縛しないので、同じ Agent・
  // 同じ lineage の別 session を名乗る event が rule 1 で他人の operation を閉じられる
  for (const claimed of ["session-other", "", " ", "\u{200B}"]) {
    assert.throws(
      () => stampIntakeEvidence(startEvent({ sessionId: claimed }), INTAKE),
      /§3\.1 違反: 束縛された session は session-1 なのに/,
      JSON.stringify(claimed),
    );
  }
  // 降格では止まらないことを、この門が無い場合の実害として固定する。session の食い違いは
  // evidenceKind の判定に一切入らないので、別 session を名乗る event に **native の札がそのまま
  // 貼られる**。しかも還元器は evidenceKind を読まないので、札が何であれ照合は成立する
  const unbound = { ...INTAKE, expectedSessionId: "" };
  const foreign = stampIntakeEvidence(startEvent({ sessionId: "session-other" }), unbound).event;
  assert.equal(foreign.provenance.evidenceKind, "native");
  assert.equal(foreign.sessionId, "session-other");

  // 締めすぎていないことを通す側でも測る。session を特定できない経路（spool 等）は
  // `expectedSessionId` が空なので従来どおり素通しになる
  assert.equal(stampIntakeEvidence(startEvent(), unbound).event.sessionId, "session-1");
  // 一致していれば当然通り、native authority も従来どおり成立する
  assert.equal(stampIntakeEvidence(startEvent(), INTAKE).event.provenance.evidenceKind, "native");
});

test("認証できない経路の synthesized_monotonic は通すが診断に出す（#41）", () => {
  // Q2 の判断は「まず警告だけ出す」。`synthesized_monotonic` は adapter が自分で数える連番なので
  // capability の証明を要さず、正本も認証条件を課していない。ただし rule 2 の turn 両立は
  // この種別でも成立するので照合力としては native と同じ重みを持つ。締める前に観測できるようにする
  const claimed = startEvent({ turnIdSource: "synthesized_monotonic", turnId: "turn-1" });
  // 4 項の連言なので、全部同時に倒すと**どの項が効いているか**は測れない。ここは「全部欠けた
  // 経路」を見て、個々の項は下の 2 つ（受領証は在るが Agent 束縛が無い / version drift）で分ける。
  // `expectedSessionId` は倒さない（`startEvent()` の session は INTAKE の束縛と一致していて、
  // 倒しても倒さなくても結果が変わらない = 何も測らない override だった）
  const unauthenticated = stampIntakeEvidence(claimed, {
    ...INTAKE,
    expectedSourceAgent: "",
    attestation: undefined,
  });
  // 通す: 種別も turnId も落とさない（降格でも拒否でもない）
  assert.equal(unauthenticated.event.turnIdSource, "synthesized_monotonic");
  assert.equal(unauthenticated.event.turnId, "turn-1");
  assert.deepEqual(
    unauthenticated.diagnostics.map((d) => d.code),
    ["turn_identity_unauthenticated"],
  );
  // 認証できていれば診断は出ない（偽陽性を出さない側も測る）
  assert.deepEqual(stampIntakeEvidence(claimed, INTAKE).diagnostics, []);
  // **受領証が在っても Agent 束縛が無ければ「経路を認証できない」側**。受領証は peer を
  // 指しているが、`expectedSourceAgent` が空だと caller の名乗る Agent をその peer に結び付け
  // られない——`assertSameScope` は event の `sourceAgent` でどの状態を書くか決めるので、
  // 結び付けられない経路の turn 主張は rule 2 の照合力として信用できない。診断の文面も
  // 「認証できない経路」ではなく「peer に結び付けられない経路」と書く
  const unboundAgent = stampIntakeEvidence(claimed, { ...INTAKE, expectedSourceAgent: "" });
  assert.deepEqual(unboundAgent.diagnostics.map((d) => d.code), ["turn_identity_unauthenticated"]);
  // **version drift は「未認証」ではない**。CLI を上げた直後は exact version が一致しないので
  // native authority は消えるが、受領証も peer も Agent も一致しているので経路は認証済み。
  // 両者を同じ述語で見ると、通常の version 更新が未認証として観測に混ざる
  const drifted = stampIntakeEvidence(claimed, { ...INTAKE, exactAgentVersion: "9.9.9 (Claude Code)" });
  assert.deepEqual(drifted.diagnostics.map((d) => d.code), []);
  // authority のほうは正しく消える（緩めていないことを対で見る）
  assert.equal(drifted.event.provenance.evidenceKind, "synthesized");
  assert.equal(
    stampIntakeEvidence(startEvent(), { ...INTAKE, exactAgentVersion: "9.9.9 (Claude Code)" }).event
      .turnIdSource,
    "unavailable",
  );
  // native の降格経路とは独立。認証できない native 主張は従来どおり降格の診断だけが出る
  assert.deepEqual(
    stampIntakeEvidence(startEvent(), { ...INTAKE, attestation: undefined }).diagnostics.map((d) => d.code),
    ["turn_identity_downgraded"],
  );
});

test("認証できない経路では Agent 名の食い違いを降格で扱う", () => {
  // 締めすぎない側も測る。`expectedSourceAgent` が空 = 受領証が peer を名乗っていない経路では
  // 「違う」と言える相手が居ないので、従来どおり native を落とすだけにする
  const stamped = stampIntakeEvidence(startEvent({ sourceAgent: "codex" }), {
    ...INTAKE,
    expectedSourceAgent: "",
  });
  assert.equal(stamped.event.provenance.evidenceKind, "synthesized");
  assert.equal(stamped.event.sourceAgent, "codex");
});

test("revision ごとに pendingOperations の配列を分ける", () => {
  // 配列を共有すると、新しい revision への変更が過去の snapshot にも見える（§4.2 違反）
  const started = startedSnapshot();
  const next = reduceTaskWorkState(
    started,
    startEvent({
      eventId: "event-prompt",
      adapterDeliveryId: "delivery-prompt",
      kind: "user_prompted",
      ingestSeq: "13",
      operation: undefined,
    }),
    new Map(),
  );
  assert.equal(
    Object.is(started.state.pendingOperations, next.snapshot.state.pendingOperations),
    false,
  );
});



test("adapter 固有の kind でも envelope の欄は検査する", () => {
  // 既知の phase が無い kind は phase を照合できないが、envelope を持つなら reducer の
  // operation 経路にそのまま入る。空文字の native ID が rule 1 の照合権威になると、
  // 無関係な custom operation 同士が同じものとして畳まれる
  assert.throws(
    () =>
      assertOperationEnvelope(
        startEvent({
          kind: "adapter_custom_started",
          operation: { ...START_OPERATION, nativeOperationId: "" },
        }),
      ),
    /nativeOperationId \/ canonicalInputHash が空文字/,
  );
  // 未知の kind そのものは拒否しない（欄が揃っていれば通る）
  assertOperationEnvelope(startEvent({ kind: "adapter_custom_started" }));
});

test("operationId 一致で見つけた再配送 start でも native ID の違いは隔離する", () => {
  // 同じ eventId・matchKey で当たると native ID を一度も比べないまま重複として台帳に入り、
  // 訂正版が同じ配送 ID で来ても no-op になって戻せない
  const started = startedSnapshot();
  const forged = reduceTaskWorkState(
    started,
    startEvent({
      adapterDeliveryId: "delivery-start-2",
      canonicalFingerprint: "fingerprint-start-2",
      ingestSeq: "13",
      operation: { ...START_OPERATION, nativeOperationId: "toolu_other" },
    }),
    new Map(),
  );
  assert.equal(forged.outcome, "quarantined");
  assert.deepEqual(
    forged.diagnostics.map((d) => d.code),
    ["start_conflict"],
  );
  assert.equal(forged.snapshot.state.pendingOperations.length, 1);
});

test("operationKind が違う terminal は native ID が一致しても閉じない", () => {
  // rule 1 は nativeOperationId だけで候補を選ぶので、kind を見ないと Bash の operation を
  // 別種の terminal で閉じられる。kind は §4.3 の matchKey の入力に含まれる identity の一部
  const snapshot = startedSnapshot();
  const forged = reduceTaskWorkState(
    snapshot,
    terminalEvent({ operation: { ...TERMINAL_OPERATION, operationKind: "Read" } }),
    new Map(),
  );
  assert.equal(forged.outcome, "quarantined");
  assert.deepEqual(
    forged.diagnostics.map((d) => d.code),
    ["terminal_conflict"],
  );
  assert.equal(forged.snapshot.state.pendingOperations[0]?.status, "started");
});

test("放棄でも同じ配送 ID で source hash が違えば隔離する", () => {
  // 重複として黙って捨てると放棄が落ちて operation が started のまま残る。reducer 側の
  // delivery_conflict と判定を揃える
  const started = startedSnapshot();
  const abandon = startEvent({
    eventId: "event-abandon",
    kind: "session_ended",
    ingestSeq: "4",
    operation: undefined,
    adapterDeliveryId: "delivery-abandon",
    canonicalFingerprint: "fingerprint-abandon",
  });
  const first = finalizeAbandonedState(started.state, abandon, new Map());
  assert.equal(first.outcome, "applied");
  const conflicting = finalizeAbandonedState(
    first.state,
    startEvent({
      eventId: "event-abandon-corrupt",
      kind: "session_ended",
      ingestSeq: "5",
      operation: undefined,
      adapterDeliveryId: "delivery-abandon",
      canonicalFingerprint: "fingerprint-other",
    }),
    first.ledger,
  );
  assert.equal(conflicting.outcome, "quarantined");
  assert.equal(conflicting.state, first.state);
  assert.equal(conflicting.ledger, first.ledger);
  // 診断も還元器側と同じものを出す。doctor が受け取るのは outcome ではなく診断の側なので、
  // 空で返すと「なぜ放棄が落ちたか」が経路ごとに違う形でしか分からない
  assert.deepEqual(conflicting.diagnostics.map((d) => d.code), ["delivery_conflict"]);
  assert.equal(conflicting.diagnostics[0]?.eventId, "event-abandon-corrupt");
});

test("toolName を持たない schema 妥当な pending でも rule 1 の terminal は閉じる", () => {
  // toolName は凍結 schema の required に無い（required は operationId / startEventId /
  // operationMatchKey / sessionId / taskLineageId の 5 つ）。checkpoint から復元した状態や
  // 別実装が書いた状態では欠けうるので、kind を素で比べると健全な terminal が永久に隔離され、
  // 台帳にも入らないので adapter が無限再送になる
  const started = startedSnapshot();
  const pending = started.state.pendingOperations[0];
  if (pending === undefined) assert.fail("pending が無い");
  const { toolName: _dropped, ...correlation } = pending.correlation;
  const state = { ...started.state, pendingOperations: [{ ...pending, correlation }] };
  assert.deepEqual(
    validateContractValue("CanonicalWorkStateV1", state, SCHEMA_ROOT, CONTINUITY_LIMITS),
    [],
  );
  const closed = reduceTaskWorkState({ ...started, state }, terminalEvent(), new Map());
  assert.equal(closed.outcome, "applied");
  assert.deepEqual(closed.diagnostics, []);
  assert.equal(closed.snapshot.state.pendingOperations[0]?.status, "succeeded");
  // start 側も同じ非対称を持っていたので、再配送 start が隔離されないことも見る
  const again = reduceTaskWorkState(
    { ...started, state },
    startEvent({ adapterDeliveryId: "delivery-start-3", canonicalFingerprint: "fingerprint-start-3", ingestSeq: "14" }),
    new Map(),
  );
  assert.equal(again.outcome, "applied");
  assert.deepEqual(
    again.diagnostics.map((d) => d.code),
    ["duplicate_operation_start"],
  );
});

// --- round 9: identity 材料の省略・空白・session（#37） ------------------------

test("identity が一致する確定済み兄弟がいても、衝突する open な候補は閉じない", () => {
  // rule 2 で matchKey を共有する「確定済み A（hash A）」と「open な B（hash B）」が並ぶとき、
  // A の terminal を再配送すると「互換な候補が 1 件でもあれば全体を免除する」実装では
  // B に terminal が付いてしまう。確定済みの互換候補が囮になる形
  const A = { ...MATCH_KEY_ONLY, canonicalInputHash: "input-hash-A" } as const;
  const B = { ...MATCH_KEY_ONLY, canonicalInputHash: "input-hash-B" } as const;
  const prepared = apply(emptySnapshot(), [
    startEvent({ eventId: "start-a", adapterDeliveryId: "d-start-a", canonicalFingerprint: "f-start-a", operation: A, ingestSeq: "11" }),
    terminalEvent({ eventId: "term-a", adapterDeliveryId: "d-term-a", canonicalFingerprint: "f-term-a", operation: { ...A, phase: "terminal" }, ingestSeq: "12" }),
    startEvent({ eventId: "start-b", adapterDeliveryId: "d-start-b", canonicalFingerprint: "f-start-b", operation: B, ingestSeq: "13" }),
  ]);
  assert.deepEqual(
    prepared.snapshot.state.pendingOperations.map((p) => p.status),
    ["succeeded", "started"],
  );
  const redelivered = reduceTaskWorkState(
    prepared.snapshot,
    terminalEvent({ eventId: "term-a2", adapterDeliveryId: "d-term-a2", canonicalFingerprint: "f-term-a", operation: { ...A, phase: "terminal" }, ingestSeq: "14" }),
    prepared.ledger,
  );
  assert.equal(redelivered.outcome, "applied");
  assert.deepEqual(
    redelivered.diagnostics.map((d) => d.code),
    ["terminal_already_applied"],
  );
  // B は起動したまま。囮に引きずられて succeeded にならない
  assert.deepEqual(
    redelivered.snapshot.state.pendingOperations.map((p) => p.status),
    ["succeeded", "started"],
  );
});

test("候補が複数あるとき canonicalInputHash を省いた terminal は照合できないものとして扱う", () => {
  // 両方 present のときだけ比べる衝突検査は復元耐性のためにあるが、そのままだと欄を省くだけで
  // 検査を無効化できる。省略は wire 側の自由なので、これは攻撃者が選べる経路。
  // 弁別子が hash しか残っていない形＝同じ matchKey に hash 違いの兄弟が並ぶ形でだけ発火する
  const A = { ...MATCH_KEY_ONLY, canonicalInputHash: "input-hash-A" } as const;
  const B = { ...MATCH_KEY_ONLY, canonicalInputHash: "input-hash-B" } as const;
  const prepared = apply(emptySnapshot(), [
    startEvent({ eventId: "start-a", adapterDeliveryId: "d-start-a", canonicalFingerprint: "f-start-a", operation: A, ingestSeq: "11" }),
    terminalEvent({ eventId: "term-a", adapterDeliveryId: "d-term-a", canonicalFingerprint: "f-term-a", operation: { ...A, phase: "terminal" }, ingestSeq: "12" }),
    startEvent({ eventId: "start-b", adapterDeliveryId: "d-start-b", canonicalFingerprint: "f-start-b", operation: B, ingestSeq: "13" }),
  ]);
  const omitted = reduceTaskWorkState(
    prepared.snapshot,
    terminalEvent({
      eventId: "term-x", adapterDeliveryId: "d-term-x", canonicalFingerprint: "f-term-x", ingestSeq: "14",
      operation: { ...MATCH_KEY_ONLY, phase: "terminal", canonicalInputHash: undefined },
    }),
    prepared.ledger,
  );
  assert.equal(omitted.outcome, "applied");
  assert.deepEqual(
    omitted.diagnostics.map((d) => d.code),
    ["terminal_identity_unverifiable"],
  );
  // succeeded を名乗られても B は unknown に倒れる。確定済みの A は動かない
  assert.deepEqual(
    omitted.snapshot.state.pendingOperations.map((p) => p.status),
    ["succeeded", "unknown"],
  );
  // 隔離ではなく台帳に入るので、後から届いた本物の terminal がそのまま閉じられる
  const real = reduceTaskWorkState(
    omitted.snapshot,
    terminalEvent({
      eventId: "event-fail", adapterDeliveryId: "delivery-fail", canonicalFingerprint: "fingerprint-fail",
      kind: "tool_failed", successful: false, operation: { ...B, phase: "terminal" }, ingestSeq: "15",
    }),
    omitted.ledger,
  );
  assert.deepEqual(real.diagnostics, []);
  assert.deepEqual(
    real.snapshot.state.pendingOperations.map((p) => p.status),
    ["succeeded", "failed"],
  );
});

test("照合不能で unknown に倒すのは付け替え先になりうる候補だけ", () => {
  // ゲートが発火する形（同じ turn の候補が 2 件以上）を作ったうえで、turn が両立しない open な
  // 兄弟が巻き込まれないことを見る。閉じえない候補はそもそもこの terminal の candidates ではない
  const base = startedSnapshot(startEvent({ operation: MATCH_KEY_ONLY }));
  const template = base.state.pendingOperations[0] as PendingOperation;
  const openSameTurn: PendingOperation = {
    ...template,
    correlation: { ...template.correlation, canonicalInputHash: "hash-a" },
  };
  const settledSameTurn: PendingOperation = {
    ...template,
    operationId: "op-settled",
    status: "succeeded",
    correlation: { ...template.correlation, operationId: "op-settled" },
  };
  const openOtherTurn: PendingOperation = {
    ...template,
    operationId: "op-other-turn",
    correlation: { ...template.correlation, operationId: "op-other-turn", turnId: "turn-2" },
  };
  const omitsHash = terminalEvent({
    operation: (({ nativeOperationId, canonicalInputHash, ...rest }) => rest)(TERMINAL_OPERATION) as never,
  });
  const result = reduceTaskWorkState(
    {
      ...base,
      state: { ...base.state, pendingOperations: [openSameTurn, settledSameTurn, openOtherTurn] },
    },
    omitsHash,
    new Map(),
  );
  assert.deepEqual(result.diagnostics.map((d) => d.code), ["terminal_identity_unverifiable"]);
  assert.deepEqual(
    result.snapshot.state.pendingOperations.map((p) => p.status),
    ["unknown", "succeeded", "started"],
  );
});

test("公開している鍵関数で台帳を組み立てると重複判定が効く", () => {
  // `IdempotencyLedger` は caller が渡して caller に返る = 構築・永続化・復元は caller の責務。
  // なのに公開していたのは接頭辞の無い `idempotencyKeyOf` だけで、還元器が引くのは `d:` / `f:` の
  // ついた内部鍵だった。公開関数で組み立て直した台帳は全 entry で食い違い、重複判定が発火しない
  const event = startEvent();
  const applied = reduceTaskWorkState(emptySnapshot(), event, new Map());
  assert.deepEqual([...applied.ledger.keys()], [ledgerKeyOf(event)]);
  // 公開鍵で組み立て直した台帳でも、還元器と同じ entry を指す
  const rebuilt = new Map([[ledgerKeyOf(event), applied.ledger.get(ledgerKeyOf(event)) as LedgerEntryV1]]);
  const again = reduceTaskWorkState(applied.snapshot, event, rebuilt);
  assert.equal(again.outcome, "duplicate");
  assert.equal(again.snapshot.state.stateRevision, applied.snapshot.state.stateRevision);
  // 接頭辞の無い鍵で組み立てると重複判定が発火せず、そのまま適用されてしまう（塞ぐ前の挙動）
  const wrongKey = new Map([[idempotencyKeyOf(event), applied.ledger.get(ledgerKeyOf(event)) as LedgerEntryV1]]);
  assert.notEqual(reduceTaskWorkState(applied.snapshot, event, wrongKey).outcome, "duplicate");
  // 台帳の鍵は `idempotencyKeyOf` の戻り値そのものではない（wire の導出式と keyspace を分けている）
  assert.notEqual(ledgerKeyOf(event), idempotencyKeyOf(event));
});

test("再配送 start も原因 event を operation に残す", () => {
  // この経路は配送鍵を消費し revision も進め、記録に欠けている識別材料も埋めるのに、
  // 状態を変える他の経路が全部呼んでいる `withSourceEvent` だけ呼んでいなかった
  // 再配送契約は「同じ adapterDeliveryId・違う eventId・違う ingestSeq」なので derived id は
  // 一致しない。再配送として拾えるのは nativeOperationId が一致する場合（本物の呼び出しごとに一意）
  const seeded = startedSnapshot();
  const redelivered = reduceTaskWorkState(
    seeded,
    startEvent({
      eventId: "event-start-2",
      canonicalFingerprint: "fingerprint-start-2",
      ingestSeq: "13",
    }),
    new Map(),
  );
  assert.deepEqual(redelivered.diagnostics.map((d) => d.code), ["duplicate_operation_start"]);
  assert.deepEqual(
    redelivered.snapshot.state.pendingOperations[0]?.sourceEventIds,
    ["event-start", "event-start-2"],
  );
  // 同じ eventId の再配送（台帳だけ失った復元）では増えない
  const sameId = reduceTaskWorkState(seeded, startEvent({ ingestSeq: "13" }), new Map());
  assert.deepEqual(sameId.diagnostics.map((d) => d.code), ["duplicate_operation_start"]);
  assert.deepEqual(sameId.snapshot.state.pendingOperations[0]?.sourceEventIds, ["event-start"]);
});

test("既に記録済みの event で source_events_truncated を出さない", () => {
  // `withSourceEvent` は「既に記録済みなら何もしない」を先に見るので、上限に達していても
  // その event は失われていない。長さだけで判定すると、何も失っていないのに診断が出る
  const base = startedSnapshot(startEvent({ operation: MATCH_KEY_ONLY }));
  const pending = base.state.pendingOperations[0] as PendingOperation;
  const full: PendingOperation = {
    ...pending,
    sourceEventIds: [
      ...Array.from({ length: CONTINUITY_LIMITS.arrayItems - 1 }, (_, index) => `filler-${index}`),
      "event-terminal",
    ],
  };
  const closed = reduceTaskWorkState(
    { ...base, state: { ...base.state, pendingOperations: [full] } },
    terminalEvent({ operation: { ...TERMINAL_OPERATION, nativeOperationId: undefined } }),
    new Map(),
  );
  assert.deepEqual(closed.diagnostics.map((d) => d.code), []);
  // 本当に記録できない場合は従来どおり出る
  const otherIds: PendingOperation = {
    ...pending,
    sourceEventIds: Array.from({ length: CONTINUITY_LIMITS.arrayItems }, (_, index) => `filler-${index}`),
  };
  const truncated = reduceTaskWorkState(
    { ...base, state: { ...base.state, pendingOperations: [otherIds] } },
    terminalEvent({ operation: { ...TERMINAL_OPERATION, nativeOperationId: undefined } }),
    new Map(),
  );
  assert.deepEqual(truncated.diagnostics.map((d) => d.code), ["source_events_truncated"]);
  // **この event が書きに行っていない operation を巻き込まない**。上限に達した無関係な
  // pending を 1 件足しても、診断が名乗るのは照合された相手だけ。1 件しか置かない fixture だと
  // 対象集合を「全 pending」へ広げる変異が緑のまま通る（実測で確認してからこの 1 件を足した）
  const bystander: PendingOperation = {
    ...pending,
    operationId: "op-bystander",
    correlation: {
      ...pending.correlation,
      operationId: "op-bystander",
      operationMatchKey: "match-key-bystander",
      nativeOperationId: "toolu_bystander",
    },
    sourceEventIds: Array.from({ length: CONTINUITY_LIMITS.arrayItems }, (_, index) => `other-${index}`),
  };
  const withBystander = reduceTaskWorkState(
    { ...base, state: { ...base.state, pendingOperations: [otherIds, bystander] } },
    terminalEvent({ operation: { ...TERMINAL_OPERATION, nativeOperationId: undefined } }),
    new Map(),
  );
  assert.deepEqual(withBystander.diagnostics.map((d) => d.code), ["source_events_truncated"]);
  assert.equal(
    withBystander.diagnostics[0]?.detail?.includes("op-bystander"),
    false,
    `書きに行っていない operation を名乗っている: ${withBystander.diagnostics[0]?.detail}`,
  );
});

test("記録できる候補が 1 件も無い terminal は、兄弟の有無に関わらず鍵を残す", () => {
  // start より先に terminal が届くのは正常運用（hook と transcript scan の取り込み順、再起動後の
  // catch-up）。隔離しておけば start が届いた後の再配送で拾い直せるが、commit すると鍵が焼けて
  // 二度と閉じられない。ところが「隔離するコード」を名前で並べていたので、**確定済みで同じ
  // matchKey の兄弟が 1 件居るだけで** `terminal_orphaned` ではなく `terminal_unmatched` に落ち、
  // 候補を 1 件も unknown にしないまま鍵だけ消費していた（実測）
  const base = startedSnapshot(startEvent({ operation: MATCH_KEY_ONLY }));
  const template = base.state.pendingOperations[0] as PendingOperation;
  const settledSibling: PendingOperation = { ...template, status: "succeeded" };
  const early = terminalEvent({
    eventId: "term-early",
    adapterDeliveryId: "delivery-early",
    canonicalFingerprint: "fingerprint-early",
    ingestSeq: "20",
    turnId: "turn-2",
    operation: { ...TERMINAL_OPERATION, nativeOperationId: undefined },
  });
  for (const pendingOperations of [[], [settledSibling]]) {
    const result = reduceTaskWorkState(
      { ...base, state: { ...base.state, pendingOperations } },
      early,
      new Map(),
    );
    assert.equal(result.outcome, "quarantined");
    assert.equal(result.ledger.size, 0);
  }
  // 締めすぎていない: 既に閉じた operation への再配送は本当に重複なので鍵を消費する
  const applied = reduceTaskWorkState(
    {
      ...base,
      state: { ...base.state, pendingOperations: [settledSibling] },
    },
    terminalEvent({ operation: { ...TERMINAL_OPERATION, nativeOperationId: undefined } }),
    new Map(),
  );
  assert.equal(applied.outcome, "applied");
  assert.deepEqual(applied.diagnostics.map((d) => d.code), ["terminal_already_applied"]);
  assert.equal(applied.ledger.size, 1);
});

test("確定済みで別 turn の兄弟は照合不能ゲートを発火させない", () => {
  // `compatible` を母数にしていたとき、**閉じえない兄弟が健全な照合を潰していた**（実測: 兄弟が
  // 居なければ診断ゼロで succeeded なのに、確定済み・別 turn の兄弟が 1 件並ぶだけで
  // terminal_identity_unverifiable になり、open な候補が unknown に倒れて台帳まで消費された）
  const base = startedSnapshot(startEvent({ operation: MATCH_KEY_ONLY }));
  const mine = base.state.pendingOperations[0] as PendingOperation;
  const settledOtherTurn: PendingOperation = {
    ...mine,
    operationId: "op-settled",
    status: "succeeded",
    correlation: { ...mine.correlation, operationId: "op-settled", turnId: "turn-2" },
  };
  const omitsHash = terminalEvent({
    operation: (({ nativeOperationId, canonicalInputHash, ...rest }) => rest)(TERMINAL_OPERATION) as never,
  });
  const withSibling = reduceTaskWorkState(
    { ...base, state: { ...base.state, pendingOperations: [mine, settledOtherTurn] } },
    omitsHash,
    new Map(),
  );
  assert.deepEqual(withSibling.diagnostics.map((d) => d.code), []);
  assert.deepEqual(withSibling.snapshot.state.pendingOperations.map((p) => p.status), ["succeeded", "succeeded"]);
});

test("復元した状態が toolName を持たなくても rule 2 の候補になる", () => {
  // `toolName` は凍結 schema の required に無い。兄弟の 2 箇所は両方 present ガードを持つのに
  // 候補の絞り込みだけ素で比べていたので、欠けた状態では候補ゼロ = terminal_orphaned の隔離に
  // なった。隔離は台帳を消費せず還元器は純関数なので、同じ terminal が毎回同じ隔離で収束しない
  const base = startedSnapshot(startEvent({ operation: MATCH_KEY_ONLY }));
  const pending = base.state.pendingOperations[0] as PendingOperation;
  const withoutToolName: PendingOperation = {
    ...pending,
    correlation: (({ toolName, ...rest }) => rest)(pending.correlation) as typeof pending.correlation,
  };
  const closed = reduceTaskWorkState(
    { ...base, state: { ...base.state, pendingOperations: [withoutToolName] } },
    terminalEvent({ operation: { ...TERMINAL_OPERATION, nativeOperationId: undefined } }),
    new Map(),
  );
  assert.deepEqual(closed.diagnostics.map((d) => d.code), []);
  assert.equal(closed.snapshot.state.pendingOperations[0]?.status, "succeeded");
  // 締めすぎていない: toolName を持っていて kind が違うなら従来どおり候補にしない
  const wrongKind: PendingOperation = {
    ...pending,
    correlation: { ...pending.correlation, toolName: "Read" },
  };
  const orphaned = reduceTaskWorkState(
    { ...base, state: { ...base.state, pendingOperations: [wrongKind] } },
    terminalEvent({ operation: { ...TERMINAL_OPERATION, nativeOperationId: undefined } }),
    new Map(),
  );
  assert.equal(orphaned.outcome, "quarantined");
  assert.deepEqual(orphaned.diagnostics.map((d) => d.code), ["terminal_orphaned", "dropped_evidence_recorded"]);
});

test("turn が両立しない兄弟は照合不能ゲートの母数に入らない", () => {
  // 照合不能ゲートの理由は「terminal が hash を省くことで**別の候補へ付け替えられる**」こと。
  // §4.3 の rule 2 で閉じられない turn 非両立の候補は、そもそもこの terminal の付け替え先に
  // なりえないので母数に入れない（入れると、閉じえない兄弟が居るだけで健全な照合が潰れ、
  // 台帳まで消費される）。当然その候補も巻き込まない
  const prepared = apply(emptySnapshot(), [
    startEvent({ eventId: "start-a", adapterDeliveryId: "d-start-a", canonicalFingerprint: "f-start-a", operation: MATCH_KEY_ONLY, ingestSeq: "11", turnIdSource: "native" }),
    startEvent({ eventId: "start-b", adapterDeliveryId: "d-start-b", canonicalFingerprint: "f-start-b", operation: MATCH_KEY_ONLY, ingestSeq: "12", turnIdSource: "synthesized_monotonic" }),
  ]);
  assert.deepEqual(prepared.snapshot.state.pendingOperations.map((p) => p.status), ["started", "started"]);
  const omitted = reduceTaskWorkState(
    prepared.snapshot,
    terminalEvent({
      eventId: "term-x", adapterDeliveryId: "d-term-x", canonicalFingerprint: "f-term-x", ingestSeq: "13",
      operation: { ...MATCH_KEY_ONLY, phase: "terminal", canonicalInputHash: undefined }, turnIdSource: "native",
    }),
    prepared.ledger,
  );
  // 付け替え先が居ないので発火せず、rule 2 の「open な候補が 1 件」がそのまま成立する
  assert.deepEqual(omitted.diagnostics.map((d) => d.code), []);
  assert.deepEqual(omitted.snapshot.state.pendingOperations.map((p) => p.status), ["succeeded", "started"]);
});

test("候補が 1 件なら canonicalInputHash の省略は照合を妨げない", () => {
  // §4.3 の matchKey は canonical input hash を入力に含むので、仕様どおりに導出する adapter では
  // hash 違いの兄弟は候補に並ばない。付け替えられる相手が居ない以上、省略で盗めるものが無い。
  // 「terminal は入力ではなく結果なので hash を載せない」adapter を締め出さないための対照
  for (const [label, op] of [
    ["rule 2（matchKey）", MATCH_KEY_ONLY],
    ["rule 1（nativeOperationId）", START_OPERATION],
  ] as const) {
    const started = startedSnapshot(startEvent({ operation: op }));
    const omitted = reduceTaskWorkState(
      started,
      terminalEvent({ operation: { ...op, phase: "terminal", canonicalInputHash: undefined } }),
      new Map(),
    );
    assert.deepEqual(omitted.diagnostics, [], label);
    assert.equal(omitted.snapshot.state.pendingOperations[0]?.status, "succeeded", label);
  }
});

test("確定済みの候補に矛盾する terminal は hash を省いても隔離される", () => {
  // 照合不能の検査を成否矛盾検査より前に置くと、hash を省くだけで隔離（台帳を消費しない）を
  // 回避して照合不能（台帳を消費する）に化け、訂正版の再配送が重複 no-op として捨てられる
  const A = { ...MATCH_KEY_ONLY, canonicalInputHash: "input-hash-A" } as const;
  const B = { ...MATCH_KEY_ONLY, canonicalInputHash: "input-hash-B" } as const;
  const failed = { kind: "tool_failed", successful: false } as const;
  const prepared = apply(emptySnapshot(), [
    startEvent({ eventId: "start-a", adapterDeliveryId: "d-start-a", canonicalFingerprint: "f-start-a", operation: A, ingestSeq: "11" }),
    terminalEvent({ eventId: "term-a", adapterDeliveryId: "d-term-a", canonicalFingerprint: "f-term-a", operation: { ...A, phase: "terminal" }, ingestSeq: "12", ...failed }),
    startEvent({ eventId: "start-b", adapterDeliveryId: "d-start-b", canonicalFingerprint: "f-start-b", operation: B, ingestSeq: "13" }),
    terminalEvent({ eventId: "term-b", adapterDeliveryId: "d-term-b", canonicalFingerprint: "f-term-b", operation: { ...B, phase: "terminal" }, ingestSeq: "14", ...failed }),
  ]);
  const forged = reduceTaskWorkState(
    prepared.snapshot,
    terminalEvent({
      eventId: "term-x", adapterDeliveryId: "d-term-x", canonicalFingerprint: "f-term-x", ingestSeq: "15",
      operation: { ...MATCH_KEY_ONLY, phase: "terminal", canonicalInputHash: undefined },
    }),
    prepared.ledger,
  );
  assert.equal(forged.outcome, "quarantined");
  assert.deepEqual(
    forged.diagnostics.map((d) => d.code),
    ["terminal_conflict"],
  );
  // 隔離は配送鍵を消費しないので、訂正版が後から効く
  assert.equal(forged.ledger.size, prepared.ledger.size);
});

test("turn 種別が両立しない兄弟は矛盾する terminal の言い訳にならない", () => {
  // 確定済みの候補で再配送を説明するときにも「この terminal が閉じえた候補」だけを使う。
  // 素の候補集合で説明を許すと、種別が両立しない兄弟が囮になって隔離（台帳を消費しない）を
  // 回避し、`terminal_already_applied` として台帳を消費するので訂正版が重複 no-op になる
  const failed = { kind: "tool_failed", successful: false } as const;
  const prepared = apply(emptySnapshot(), [
    startEvent({ eventId: "start-a", adapterDeliveryId: "d-start-a", canonicalFingerprint: "f-start-a", operation: MATCH_KEY_ONLY, ingestSeq: "11", turnIdSource: "native" }),
    startEvent({ eventId: "start-b", adapterDeliveryId: "d-start-b", canonicalFingerprint: "f-start-b", operation: MATCH_KEY_ONLY, ingestSeq: "12", turnIdSource: "synthesized_monotonic" }),
    terminalEvent({ eventId: "term-a", adapterDeliveryId: "d-term-a", canonicalFingerprint: "f-term-a", operation: { ...MATCH_KEY_ONLY, phase: "terminal" }, ingestSeq: "13", turnIdSource: "native", ...failed }),
    terminalEvent({ eventId: "term-b", adapterDeliveryId: "d-term-b", canonicalFingerprint: "f-term-b", operation: { ...MATCH_KEY_ONLY, phase: "terminal" }, ingestSeq: "14", turnIdSource: "synthesized_monotonic" }),
  ]);
  assert.deepEqual(
    prepared.snapshot.state.pendingOperations.map((pending) => pending.status),
    ["failed", "succeeded"],
  );
  // native の候補は failed で確定しているので、succeeded を名乗る native の 2 通目は矛盾する。
  // succeeded の兄弟は synthesized_monotonic なのでこの terminal では閉じえない
  const forged = reduceTaskWorkState(
    prepared.snapshot,
    terminalEvent({
      // 指紋は A が受理したものと**同じ**にする。違う指紋にすると #44 の衝突検査でも
      // `terminal_conflict` になり、この test が成否矛盾ゲートを見ているのか指紋ゲートを
      // 見ているのか区別できない（実測: そちらが先に塞ぐので、成否矛盾ゲートを壊す変異が
      // 生存した）。同じ指紋なら塞げるのは成否矛盾ゲートだけ
      eventId: "term-x", adapterDeliveryId: "d-term-x", canonicalFingerprint: "f-term-a", ingestSeq: "15",
      operation: { ...MATCH_KEY_ONLY, phase: "terminal" }, turnIdSource: "native",
    }),
    prepared.ledger,
  );
  assert.equal(forged.outcome, "quarantined");
  assert.deepEqual(forged.diagnostics.map((d) => d.code), ["terminal_conflict"]);
  assert.equal(forged.ledger.size, prepared.ledger.size);
  // 対照: 種別が両立する候補と成否が一致する再配送は従来どおり適用済みとして通る
  const honest = reduceTaskWorkState(
    prepared.snapshot,
    terminalEvent({
      eventId: "term-y", adapterDeliveryId: "d-term-y", canonicalFingerprint: "f-term-a", ingestSeq: "16",
      operation: { ...MATCH_KEY_ONLY, phase: "terminal" }, turnIdSource: "native", ...failed,
    }),
    prepared.ledger,
  );
  assert.equal(honest.outcome, "applied");
  assert.deepEqual(honest.diagnostics.map((d) => d.code), ["terminal_already_applied"]);
});

test("turn が両立する確定済み候補が 1 件も無いなら適用済みを名乗らない", () => {
  // 「候補は全件確定済みで、この terminal が閉じえたものは 1 件も無い」は再配送では説明できない。
  // `terminal_already_applied` を名乗ると、閉じえなかった terminal が適用済みとして台帳に入る
  const prepared = apply(emptySnapshot(), [
    startEvent({ operation: MATCH_KEY_ONLY, ingestSeq: "11", turnIdSource: "synthesized_monotonic" }),
    terminalEvent({ eventId: "term-b", adapterDeliveryId: "d-term-b", canonicalFingerprint: "f-term-b", operation: { ...MATCH_KEY_ONLY, phase: "terminal" }, ingestSeq: "12", turnIdSource: "synthesized_monotonic" }),
  ]);
  assert.equal(prepared.snapshot.state.pendingOperations[0]?.status, "succeeded");
  const agreeing = reduceTaskWorkState(
    prepared.snapshot,
    terminalEvent({
      eventId: "term-agree", adapterDeliveryId: "d-agree", canonicalFingerprint: "f-agree",
      ingestSeq: "13", operation: { ...MATCH_KEY_ONLY, phase: "terminal" }, turnIdSource: "native",
    }),
    prepared.ledger,
  );
  // 保持する相手が居ないので状態にも記録する（#43）。診断だけで流すと terminal が消える
  assert.deepEqual(
    agreeing.diagnostics.map((d) => d.code),
    ["terminal_unmatched", "dropped_evidence_recorded"],
  );
  assert.equal(agreeing.snapshot.state.droppedEvidence?.length, 1);
  // 候補は確定済みなので `unknown` に倒す相手は居ない
  assert.equal(agreeing.snapshot.state.pendingOperations[0]?.status, "succeeded");
  // 対照: turn が両立する候補があれば従来どおり適用済みとして通る
  const same = reduceTaskWorkState(
    prepared.snapshot,
    terminalEvent({
      eventId: "term-same", adapterDeliveryId: "d-same", canonicalFingerprint: "f-term-b", ingestSeq: "14",
      operation: { ...MATCH_KEY_ONLY, phase: "terminal" }, turnIdSource: "synthesized_monotonic",
    }),
    prepared.ledger,
  );
  assert.deepEqual(same.diagnostics.map((d) => d.code), ["terminal_already_applied"]);
});

test("状態側で operationId が衝突していても terminal は 1 件しか閉じない", () => {
  // `operationId` は `eventId` + matchKey からの導出なので還元器は重複を作らないが、凍結 schema は
  // `maxLength` しか課さず一意性も要求しない。復元した checkpoint や別実装が書いた状態では
  // schema 妥当なまま重複しうるので、`operationId` の等値で当てると 1 通で N 件が閉じる
  const pending = (operationId: string, nativeOperationId: string): PendingOperation =>
    ({
      operationId,
      correlation: {
        operationId, startEventId: `start-${nativeOperationId}`, nativeOperationId,
        operationMatchKey: `match-${nativeOperationId}`, sessionId: "session-1", taskLineageId: "lineage-1",
        turnId: "turn-1", toolName: "Bash", canonicalInputHash: "input-hash-1",
      },
      kind: "tool", description: "Bash", status: "started", replayPolicy: "never_auto",
      sourceEventIds: [`start-${nativeOperationId}`], startedAt: "2026-08-16T00:00:01Z", sensitivity: "normal",
      // 順序材料はそれぞれの要素に載る（#35）。id が衝突していても取り違えは起きない
      startIngestSeq: "11", startTurnIdSource: "native",
    }) as unknown as PendingOperation;
  for (const duplicated of ["", "op-dup"]) {
    const victim: TaskWorkStateSnapshotV1 = {
      state: emptyState({ pendingOperations: [pending(duplicated, "toolu_1"), pending(duplicated, "toolu_2")] }),
      history: [],
    };
    const result = reduceTaskWorkState(victim, terminalEvent(), new Map());
    // 動くのは rule 1 が指した 1 件だけ。もう 1 件は同じ id でも触られない
    assert.deepEqual(
      result.snapshot.state.pendingOperations.map((p) => p.status),
      ["succeeded", "started"],
      JSON.stringify(duplicated),
    );
    assert.deepEqual(result.diagnostics.map((d) => d.code), [], JSON.stringify(duplicated));
  }
});

test("identity で候補から外れた兄弟は rule 1 の候補数に数えない", () => {
  // 通す側を測る。`byNativeId` は `identityConflicts` で絞る前の集合なので、そこで数えると
  // 「native id は同じだが input hash や kind が違う」= 既に候補から外れている兄弟まで数に入り、
  // 健全な terminal が terminal_ambiguous で unknown に倒れて台帳まで消費される（訂正版が
  // 重複 no-op になるので隔離より悪い）。この関数の他の判断はすべて `compatible` を見ている
  const pending = (operationId: string, o: { nativeOperationId?: string; canonicalInputHash?: string; toolName?: string }): PendingOperation =>
    ({
      operationId,
      correlation: {
        operationId, startEventId: `start-${operationId}`,
        nativeOperationId: o.nativeOperationId ?? START_OPERATION.nativeOperationId,
        operationMatchKey: `match-${operationId}`, sessionId: "session-1", taskLineageId: "lineage-1",
        turnId: "turn-1", toolName: o.toolName ?? "Bash",
        canonicalInputHash: o.canonicalInputHash ?? START_OPERATION.canonicalInputHash,
      },
      kind: "tool", description: "Bash", status: "started", replayPolicy: "never_auto",
      sourceEventIds: [`start-${operationId}`], startedAt: "2026-08-16T00:00:01Z", sensitivity: "normal",
      startIngestSeq: "1", startTurnIdSource: "native",
    }) as unknown as PendingOperation;
  const target = pending("op-target", {});
  for (const [label, sibling] of [
    ["input hash が違う", pending("op-sibling", { canonicalInputHash: "other-hash" })],
    ["kind が違う", pending("op-sibling", { toolName: "Read" })],
    ["対照: native id が別", pending("op-sibling", { nativeOperationId: "toolu_other" })],
  ] as const) {
    const snapshot: TaskWorkStateSnapshotV1 = {
      state: emptyState({ pendingOperations: [sibling, target] }),
      history: [],
    };
    const result = reduceTaskWorkState(snapshot, terminalEvent({ ingestSeq: "50" }), new Map());
    assert.deepEqual(result.diagnostics.map((d) => d.code), [], label);
    assert.deepEqual(
      result.snapshot.state.pendingOperations.map((p) => `${p.operationId}:${p.status}`),
      ["op-sibling:started", "op-target:succeeded"],
      label,
    );
  }
});

test("同名 operationId の兄弟でも、順序検査は自分の材料で行う（#35）", () => {
  // 側索引の鍵は `operationId` だったので、同じ id の pending が並ぶ状態では**どちらの兄弟の材料か
  // 判別できず**、判別せずに引くと兄弟 B の `ingestSeq` で A 宛ての terminal が権威順序を通った
  // （逆向きの値では健全な terminal が弾かれた）。当時は両方まとめて `terminal_order_unverifiable` に
  // 倒すしか無かったが、材料を要素に載せた今は A の材料で A を判定できる
  const pending = (nativeOperationId: string, startIngestSeq: string): PendingOperation =>
    ({
      operationId: "dup",
      correlation: {
        operationId: "dup", startEventId: `start-${nativeOperationId}`, nativeOperationId,
        operationMatchKey: `match-${nativeOperationId}`, sessionId: "session-1", taskLineageId: "lineage-1",
        turnId: "turn-1", toolName: "Bash", canonicalInputHash: `hash-${nativeOperationId}`,
      },
      kind: "tool", description: "Bash", status: "started", replayPolicy: "never_auto",
      sourceEventIds: [`start-${nativeOperationId}`], startedAt: "2026-08-16T00:00:01Z", sensitivity: "normal",
      startIngestSeq, startTurnIdSource: "native",
    }) as unknown as PendingOperation;
  // A の start は 100、B の start は 10。A 宛ての terminal は **A の 100** とだけ比べる
  for (const [label, ingestSeq, expected] of [
    ["A の材料より前", "50", "terminal_out_of_order"],
    ["A の材料より後", "150", undefined],
  ] as const) {
    const victim: TaskWorkStateSnapshotV1 = {
      state: emptyState({ pendingOperations: [pending("toolu_a", "100"), pending("toolu_b", "10")] }),
      history: [],
    };
    const result = reduceTaskWorkState(
      victim,
      terminalEvent({
        ingestSeq,
        operation: {
          ...TERMINAL_OPERATION, nativeOperationId: "toolu_a",
          operationMatchKey: "match-toolu_a", canonicalInputHash: "hash-toolu_a",
        },
      }),
      new Map(),
    );
    assert.deepEqual(
      result.diagnostics.map((d) => d.code),
      expected === undefined ? [] : [expected],
      label,
    );
    assert.deepEqual(
      result.snapshot.state.pendingOperations.map((p) => p.status),
      [expected === undefined ? "succeeded" : "unknown", "started"],
      label,
    );
  }
});

test("correlateTerminalEvent は terminal 以外の operation event を受け取らない", () => {
  // 公開型は phase で判別されないので、start を渡しても `assertOperationEnvelope` は通る。
  // 経路の取り違えが `terminal_unmatched`（照合できなかっただけの正常な結果）に化けると、
  // caller は start を「未照合の terminal 証跡」として保存できてしまう
  // `progress` 相は `OPERATION_EVENT_PHASES` に対応する kind が無いので `assertOperationEnvelope`
  // が先に落とす。ここで塞ぐのは「kind と phase は整合しているが terminal ではない」形
  assert.throws(
    () => correlateTerminalEvent(startedSnapshot(), startEvent()),
    /terminal 以外の operation event は correlateTerminalEvent に渡さない/,
  );
});

test("同じ native id の兄弟が並んでも identity が一致する側への再配送は通る", () => {
  // 先頭 1 件だけを見ると、その先頭が衝突しているせいで identity が完全に一致する兄弟への
  // 健全な再配送が永久に隔離される（訂正版も同じ配送鍵なので戻せない）
  const pending = (operationId: string, suffix: string): PendingOperation =>
    ({
      operationId,
      correlation: {
        operationId, startEventId: `start-${suffix}`, nativeOperationId: START_OPERATION.nativeOperationId,
        operationMatchKey: `match-${suffix}`, sessionId: "session-1", taskLineageId: "lineage-1",
        turnId: "turn-1", toolName: "Bash", canonicalInputHash: `hash-${suffix}`,
      },
      kind: "tool", description: "Bash", status: "started", replayPolicy: "never_auto",
      sourceEventIds: [`start-${suffix}`], startedAt: "2026-08-16T00:00:01Z", sensitivity: "normal",
    }) as unknown as PendingOperation;
  const snapshot: TaskWorkStateSnapshotV1 = {
    state: emptyState({ pendingOperations: [pending("op-1", "one"), pending("op-2", "two")] }),
    history: [],
  };
  const redelivered = reduceTaskWorkState(
    snapshot,
    startEvent({
      eventId: "start-again", adapterDeliveryId: "d-start-again", canonicalFingerprint: "f-start-again",
      ingestSeq: "20",
      operation: { ...START_OPERATION, operationMatchKey: "match-two", canonicalInputHash: "hash-two" },
    }),
    new Map(),
  );
  assert.equal(redelivered.outcome, "applied");
  assert.deepEqual(redelivered.diagnostics.map((d) => d.code), [
    "duplicate_operation_start",
    "start_sibling_conflict",
  ]);
  // 二重に積まない
  assert.equal(redelivered.snapshot.state.pendingOperations.length, 2);
});

test("operationId が衝突していても放棄は自 session の operation だけを unknown にする", () => {
  // `abandoned` を `operationId` の集合で持つと、直前の session 絞り込みが id の重複で無意味に
  // なり、旧 session の session_ended が resume 先の live な operation まで unknown にする
  const pending = (operationId: string, sessionId: string, n: string): PendingOperation =>
    ({
      operationId,
      correlation: {
        operationId, startEventId: `s-${n}`, nativeOperationId: n, operationMatchKey: `m-${n}`,
        sessionId, taskLineageId: "lineage-1", turnId: "turn-1", toolName: "Bash", canonicalInputHash: "h",
      },
      kind: "tool", description: "Bash", status: "started", replayPolicy: "never_auto",
      sourceEventIds: [`s-${n}`], startedAt: "2026-08-16T00:00:01Z", sensitivity: "normal",
    }) as unknown as PendingOperation;
  const ended = {
    eventId: "end-1", adapterDeliveryId: "d-end", canonicalFingerprint: "f-end", kind: "session_ended",
    ingestSeq: "20", occurredAt: "2026-08-16T00:00:09Z", sessionId: "session-old", taskLineageId: "lineage-1",
    turnIdSource: "unavailable", sourceAgent: "claude",
    provenance: { sourceAgentVersion: VERSION, evidenceKind: "synthesized", captureMethod: "native_event" },
    payload: {},
  } as unknown as Parameters<typeof finalizeAbandonedState>[1];
  const result = finalizeAbandonedState(
    emptyState({ pendingOperations: [pending("dup", "session-old", "n1"), pending("dup", "session-live", "n2")] }),
    ended,
    new Map(),
  );
  assert.deepEqual(
    result.state?.pendingOperations.map((p) => `${p.correlation.sessionId}:${p.status}`),
    ["session-old:unknown", "session-live:started"],
  );
});

test("turn が両立しない open な兄弟は確定済みへの再配送を妨げない", () => {
  // open / 確定済みの切り分けを turn の絞り込みより先にすると、閉じえない open な兄弟が
  // 「open が居る」と数えられて確定済み経路が飛ばされ、健全な再配送が `terminal_unmatched` に
  // 化けて兄弟を `unknown` に倒し、台帳まで消費する
  const prepared = apply(emptySnapshot(), [
    startEvent({ operation: MATCH_KEY_ONLY, ingestSeq: "11", turnIdSource: "native" }),
    terminalEvent({ eventId: "term-a", adapterDeliveryId: "d-term-a", canonicalFingerprint: "f-term-a", operation: { ...MATCH_KEY_ONLY, phase: "terminal" }, ingestSeq: "12", turnIdSource: "native" }),
    startEvent({ eventId: "start-b", adapterDeliveryId: "d-start-b", canonicalFingerprint: "f-start-b", operation: MATCH_KEY_ONLY, ingestSeq: "13", turnIdSource: "synthesized_monotonic" }),
  ]);
  assert.deepEqual(prepared.snapshot.state.pendingOperations.map((p) => p.status), ["succeeded", "started"]);
  const redelivered = reduceTaskWorkState(
    prepared.snapshot,
    terminalEvent({
      eventId: "term-x", adapterDeliveryId: "d-term-x", canonicalFingerprint: "f-term-a", ingestSeq: "14",
      operation: { ...MATCH_KEY_ONLY, phase: "terminal" }, turnIdSource: "native",
    }),
    prepared.ledger,
  );
  assert.deepEqual(redelivered.diagnostics.map((d) => d.code), ["terminal_already_applied"]);
  // 閉じえない兄弟は巻き込まない
  assert.deepEqual(redelivered.snapshot.state.pendingOperations.map((p) => p.status), ["succeeded", "started"]);
});

test("unknown に倒す相手は候補だけで、別 session の同名 operation を巻き込まない", () => {
  // 閉じられなかった候補を id で当てると、状態側で id が重複しているとき候補ですらない
  // operation——別 session のもの——まで `unknown` になる。§4.3 が集合単位で指示しているのは
  // 「candidates を unknown のままにする」であって、候補の外へ広げてよいとは言っていない
  const pending = (sessionId: string, n: string, turnId: string): PendingOperation =>
    ({
      operationId: "dup",
      correlation: {
        operationId: "dup", startEventId: `s-${n}`, operationMatchKey: START_OPERATION.operationMatchKey,
        sessionId, taskLineageId: "lineage-1", turnId, toolName: "Bash",
        canonicalInputHash: START_OPERATION.canonicalInputHash,
      },
      kind: "tool", description: "Bash", status: "started", replayPolicy: "never_auto",
      sourceEventIds: [`s-${n}`], startedAt: "2026-08-16T00:00:01Z", sensitivity: "normal",
    }) as unknown as PendingOperation;
  const victim: TaskWorkStateSnapshotV1 = {
    // 候補側は turn 同一性が無いので rule 2 では閉じられず `unknown` に倒れる
    state: emptyState({ pendingOperations: [pending("session-1", "n1", "turn-9"), pending("session-other", "n2", "turn-1")] }),
    history: [],
  };
  const result = reduceTaskWorkState(
    victim,
    terminalEvent({ operation: { ...MATCH_KEY_ONLY, phase: "terminal" } }),
    new Map(),
  );
  assert.deepEqual(result.diagnostics.map((d) => d.code), ["terminal_unmatched"]);
  assert.deepEqual(
    result.snapshot.state.pendingOperations.map((p) => `${p.correlation.sessionId}:${p.status}`),
    ["session-1:unknown", "session-other:started"],
  );
});

test("turn が両立しなくても成否が矛盾する terminal は隔離する", () => {
  // 矛盾の検出は「閉じる権限があるか」ではなく「壊れた証跡か」の判定なので turn 両立は要らない。
  // ここまで絞ると候補が全部落ちたときに `find` が undefined を返し、隔離（台帳を消費しない）が
  // `terminal_already_applied`（台帳を消費する）に化けて、訂正版が重複 no-op になる
  const failed = { kind: "tool_failed", successful: false } as const;
  const prepared = apply(emptySnapshot(), [
    startEvent({ operation: MATCH_KEY_ONLY, ingestSeq: "11", turnIdSource: "synthesized_monotonic" }),
    terminalEvent({ eventId: "term-a", adapterDeliveryId: "d-term-a", canonicalFingerprint: "f-term-a", operation: { ...MATCH_KEY_ONLY, phase: "terminal" }, ingestSeq: "12", turnIdSource: "synthesized_monotonic", ...failed }),
  ]);
  assert.equal(prepared.snapshot.state.pendingOperations[0]?.status, "failed");
  // turn 種別が違う / turn そのものが違う のどちらでも、成否が逆なら隔離する
  for (const [label, extra] of [
    ["種別違い", { turnIdSource: "native" }],
    ["別 turn", { turnIdSource: "synthesized_monotonic", turnId: "turn-2" }],
  ] as const) {
    const forged = reduceTaskWorkState(
      prepared.snapshot,
      terminalEvent({
        eventId: `term-${label}`, adapterDeliveryId: `d-${label}`, canonicalFingerprint: `f-${label}`,
        ingestSeq: "13", operation: { ...MATCH_KEY_ONLY, phase: "terminal" }, ...extra,
      }),
      prepared.ledger,
    );
    assert.equal(forged.outcome, "quarantined", label);
    assert.deepEqual(forged.diagnostics.map((d) => d.code), ["terminal_conflict"], label);
    // 隔離は配送鍵を消費しないので、訂正版が後から効く
    assert.equal(forged.ledger.size, prepared.ledger.size, label);
  }
});

/**
 * turn 種別が違う live な B（synthesized_monotonic）と、確定済みの A（native）が同じ matchKey で
 * 並ぶ状態。**open を先頭に置く**: 矛盾の相手を配列順で拾う実装なので、確定済みを先頭にすると
 * 母数を広げる変異が同じ候補を掴んでしまい、母数の違いが観測できない
 */
function openThenSettledSibling(): ReturnType<typeof apply> {
  const prepared = apply(emptySnapshot(), [
    startEvent({ eventId: "start-b", adapterDeliveryId: "d-start-b", canonicalFingerprint: "f-start-b", operation: MATCH_KEY_ONLY, ingestSeq: "11", turnIdSource: "synthesized_monotonic" }),
    startEvent({ operation: MATCH_KEY_ONLY, ingestSeq: "12", turnIdSource: "native" }),
    terminalEvent({ eventId: "term-a", adapterDeliveryId: "d-term-a", canonicalFingerprint: "f-term-a", operation: { ...MATCH_KEY_ONLY, phase: "terminal" }, ingestSeq: "13", turnIdSource: "native" }),
  ]);
  assert.deepEqual(prepared.snapshot.state.pendingOperations.map((p) => p.status), ["started", "succeeded"]);
  return prepared;
}

test("記録できる open な候補が居るなら、確定済みとの矛盾より unmatched を優先する", () => {
  // §4.3:368 は「zero か複数の open にマッチした terminal は何も閉じず、unmatched な証跡として
  // 保存し candidates を unknown にする」と終状態を名指ししている。ここで隔離を優先すると
  // open な候補は `started` のまま残って状態が嘘をつき、しかも `turnIdSource` の食い違いは
  // adapter の捕捉経路という定常的な性質なので「訂正版」が存在せず、還元器は純関数なので
  // 再送は毎回同じ隔離になる = adapter は無限再送する
  const failed = { kind: "tool_failed", successful: false } as const;
  const prepared = openThenSettledSibling();
  const orphaned = reduceTaskWorkState(
    prepared.snapshot,
    terminalEvent({
      eventId: "term-x", adapterDeliveryId: "d-term-x", canonicalFingerprint: "f-term-x", ingestSeq: "14",
      operation: { ...MATCH_KEY_ONLY, phase: "terminal" }, turnIdSource: "native", turnId: "turn-9", ...failed,
    }),
    prepared.ledger,
  );
  assert.equal(orphaned.outcome, "applied");
  assert.deepEqual(orphaned.diagnostics.map((d) => d.code), ["terminal_unmatched"]);
  assert.deepEqual(orphaned.snapshot.state.pendingOperations.map((p) => p.status), ["unknown", "succeeded"]);
  assert.equal(orphaned.snapshot.history.length, prepared.snapshot.history.length + 1);
  // 抑止するのは隔離という行動だけで、矛盾していた事実は doctor に届ける（§3.1 は棄却・降格の
  // 理由を報告できることを求めている）
  assert.match(orphaned.diagnostics[0]?.detail ?? "", /succeeded で確定済みなのに failed を名乗っている/);
});

test("矛盾の診断は確定済みの候補を名指しする", () => {
  // 矛盾判定の母数を `compatible` 全体に広げると、open な兄弟のほうが先に見つかって
  // 「started で確定済み」という自己矛盾した診断になる。open は成否を主張していない
  const failed = { kind: "tool_failed", successful: false } as const;
  const prepared = openThenSettledSibling();
  const forged = reduceTaskWorkState(
    prepared.snapshot,
    terminalEvent({
      eventId: "term-y", adapterDeliveryId: "d-term-y", canonicalFingerprint: "f-term-y", ingestSeq: "14",
      operation: { ...MATCH_KEY_ONLY, phase: "terminal" }, turnIdSource: "native", ...failed,
    }),
    prepared.ledger,
  );
  assert.equal(forged.outcome, "quarantined");
  assert.deepEqual(forged.diagnostics.map((d) => d.code), ["terminal_conflict"]);
  assert.match(forged.diagnostics[0]?.detail ?? "", /succeeded で確定済み/);
});

test("記録側も canonicalInputHash を持たないなら省略は照合を妨げない", () => {
  const noHash = { ...MATCH_KEY_ONLY, canonicalInputHash: undefined } as const;
  const closed = apply(emptySnapshot(), [
    startEvent({ operation: noHash }),
    terminalEvent({ operation: { ...noHash, phase: "terminal" } }),
  ]);
  assert.equal(closed.snapshot.state.pendingOperations[0]?.status, "succeeded");
});

test("空白文字だけの identity 材料は空文字と同じく schema violation", () => {
  // schema は maxLength しか課さないので、空文字と同じ実害が空白 1 文字でもそのまま起きる。
  // U+FEFF は JS の `\s` に入るが U+200B は入らないので、書式制御文字も落とす
  for (const blank of [" ", "\t", "\n", "\u{FEFF}", "\u{200B}"]) {
    for (const field of ["canonicalFingerprint", "eventId", "sessionId", "turnId"] as const) {
      assert.throws(
        () => reduceTaskWorkState(emptySnapshot(), startEvent({ [field]: blank }), new Map()),
        /§3.1 違反/,
        `${field} = ${JSON.stringify(blank)}`,
      );
    }
    // `sourceAgent` は状態側も event 側も maxLength しか課されないので、Agent 同一性を「不明」として
    // 空白で表す adapter が 2 つあると `assertSameScope` の等値で互いの状態を書き換えられる
    assert.throws(
      () =>
        reduceTaskWorkState(
          { state: emptyState({ sourceAgent: blank }), history: [] },
          startEvent({ sourceAgent: blank }),
          new Map(),
        ),
      /sourceAgent が空文字/,
      `sourceAgent = ${JSON.stringify(blank)}`,
    );
    assert.throws(
      () => reduceTaskWorkState(emptySnapshot(), startEvent({ operation: { ...START_OPERATION, operationMatchKey: blank } }), new Map()),
      /§3.1 違反/,
    );
  }
  // 空白を含むだけの値と "0" は identity として妥当なので落とさない
  assert.equal(reduceTaskWorkState(emptySnapshot(), startEvent({ sessionId: "session 1" }), new Map()).outcome, "applied");
  assert.equal(reduceTaskWorkState(emptySnapshot(), startEvent({ sessionId: "0" }), new Map()).outcome, "applied");
  // adapterDeliveryId は「無い」を表せるので落とさず fingerprint に落ちる
  assert.equal(idempotencyKeyOf(startEvent({ adapterDeliveryId: " " })), "fingerprint-start");
});

test("空白だけの taskLineageId は lineage scope として認めない", () => {
  // `sourceAgent` と同じ形。凍結 schema は `taskLineageId` にも maxLength しか課さないので、
  // lineage が空白の状態と lineage を省いた event が組み合わさると等値検査に到達せず素通りする。
  // 素通りすると無関係な task の operation が同じ scope に潰れ、terminal が他人の operation を
  // 閉じ、`session_ended` がそれを放棄できる
  for (const blank of ["", " ", "\t", "\u{FEFF}", "\u{200B}"]) {
    // 状態側が空白 + event が lineage を省く（等値比較に到達しない経路）
    assert.throws(
      () =>
        reduceTaskWorkState(
          emptySnapshot({ taskLineageId: blank }),
          startEvent({ taskLineageId: undefined }),
          new Map(),
        ),
      /状態の taskLineageId が空文字/,
      `state = ${JSON.stringify(blank)}`,
    );
    // 状態側も event 側も空白（`"" !== ""` が false になる経路）
    assert.throws(
      () =>
        reduceTaskWorkState(
          emptySnapshot({ taskLineageId: blank }),
          startEvent({ taskLineageId: blank }),
          new Map(),
        ),
      /taskLineageId が空文字/,
      `both = ${JSON.stringify(blank)}`,
    );
    // event 側だけ空白。今は等値比較でも落ちるが、空白を「不在」と読む実装でも落ちることを固定する
    assert.throws(
      () => reduceTaskWorkState(emptySnapshot(), startEvent({ taskLineageId: blank }), new Map()),
      /event の taskLineageId が空文字/,
      `event = ${JSON.stringify(blank)}`,
    );
  }
  // 3 つの入口すべてに効く（`assertSameScope` を全入口が呼ぶことをここで固定する）
  const started = startedSnapshot();
  const blankState = { ...started, state: { ...started.state, taskLineageId: " " } };
  assert.throws(
    () => correlateTerminalEvent(blankState, terminalEvent({ taskLineageId: undefined })),
    /状態の taskLineageId が空文字/,
  );
  assert.throws(
    () =>
      finalizeAbandonedState(
        blankState.state,
        startEvent({ kind: "session_ended", ingestSeq: "12", operation: undefined, taskLineageId: undefined }),
        new Map(),
      ),
    /状態の taskLineageId が空文字/,
  );
  // 締めすぎていないことを通る側でも固定する。空白を含むだけの lineage は妥当
  assert.equal(
    reduceTaskWorkState(
      emptySnapshot({ taskLineageId: "lineage 1" }),
      startEvent({ taskLineageId: "lineage 1" }),
      new Map(),
    ).outcome,
    "applied",
  );
  assert.equal(reduceTaskWorkState(emptySnapshot(), startEvent(), new Map()).outcome, "applied");
});

test("ingestSeq と scope を両方破った event は 3 つの入口すべてで §22.6 を名乗る", () => {
  // 入口の検査順（envelope → turn → identity → ingestSeq → scope）を**通る側でなく落ちる側**で
  // 固定する。順序が入口ごとに違うと、同じ壊れた event が還元器では §22.6、直接呼びでは
  // 「別 Agent」と報告される。`rejected-events.json` は case ごとに 1 つの節を宣言して
  // TS / Rust のパリティ基準にしているので、分類が割れると移植側は同じ fixture を満たしたまま
  // 違う節を返す
  const broken = { ingestSeq: "01", sourceAgent: "codex" } as const; // 先頭 0 は decimal string でない
  const started = startedSnapshot();
  assert.throws(
    () => reduceTaskWorkState(emptySnapshot(), startEvent(broken), new Map()),
    /ingestSeq が decimal string でない/,
    "reduce",
  );
  assert.throws(
    () => correlateTerminalEvent(started, terminalEvent(broken)),
    /ingestSeq が decimal string でない/,
    "correlate",
  );
  assert.throws(
    () =>
      finalizeAbandonedState(
        started.state,
        startEvent({ ...broken, kind: "session_ended", operation: undefined }),
        new Map(),
      ),
    /ingestSeq が decimal string でない/,
    "abandon",
  );
  // scope だけを破った event は今までどおり scope で落ちる（ingestSeq 検査が scope 検査を
  // 飲み込んでいないことを固定する）
  assert.throws(
    () => correlateTerminalEvent(started, terminalEvent({ sourceAgent: "codex" })),
    /別 Agent の event は適用しない/,
    "correlate: scope のみ",
  );
});

test("occurredAt の綴りが凍結 IsoTimestamp から外れていたら落ちる（#27）", () => {
  // 暦検査は先頭 19 文字しか見ないので、綴りを先に当てないと「指す瞬間が一意」の主張が
  // 成り立たない値を通す。schema 検証を通ってから届く保証は無い（`validateContractValue` は
  // test からしか呼ばれない）ので、還元器が自分で当てる
  const misspelled = [
    "2026-08-16T00:00:01+09:00", // 数値 offset。19 文字目までは妥当だが指す瞬間は 9 時間ずれる
    "2026-08-16T00:00:01-00:00",
    "2026-08-16T00:00:01", // offset 無し。下流が local time として読む
    "2026-08-16T00:00:01ZZZGARBAGE", // 末尾ゴミ。下流の toISOString() が投げる
    "2026-08-16T00:00:01X", // 末尾 1 文字だけ違う（Z 固定を外す変異はここだけで死ぬ）
    "2026-08-16T00:00:01xyzZ", // Z で終わるが小数部の位置がゴミ（小数部の綴り検査はここだけで死ぬ）
    "2026-08-16", // 短い。slice がそのまま返し Date.parse も startsWith も通ってしまう
    "2026",
    "2026-08-16t00:00:01z", // 小文字
  ];
  for (const occurredAt of misspelled) {
    // 綴りの話であることを先に固定する（schema 側でも落ちる値であること）
    assert.notEqual(
      validateContractValue("IsoTimestamp", occurredAt, SCHEMA_ROOT, CONTINUITY_LIMITS).length,
      0,
      occurredAt,
    );
    assert.throws(
      () => reduceTaskWorkState(emptySnapshot(), startEvent({ occurredAt }), new Map()),
      /occurredAt が暦として実在しない/,
      `reduce: ${occurredAt}`,
    );
    assert.throws(
      () => correlateTerminalEvent(startedSnapshot(), terminalEvent({ occurredAt })),
      /occurredAt が暦として実在しない/,
      `correlate: ${occurredAt}`,
    );
  }
});

test("暦として実在しない occurredAt は 3 つの入口すべてで落ちる（#27）", () => {
  // pattern は成分の範囲までしか書けないので schema は通る。`new Date()` は例外を投げずに
  // 翌月へ繰り上げるため、素通しにすると状態の updatedAt / startedAt / terminalAt が
  // 申告と別の瞬間を指す
  const impossible = [
    "2026-02-30T00:00:00Z", // 2 月に 30 日は無い
    "2026-04-31T00:00:00Z", // 4 月は 30 日まで
    "2027-02-29T00:00:00Z", // 2027 年は閏年ではない
    "2100-02-29T00:00:00Z", // 100 で割れて 400 で割れない年は閏年ではない
  ];
  for (const occurredAt of impossible) {
    // schema としては妥当であることを先に固定する（この test が pattern の話に化けないように）
    assert.equal(
      validateContractValue("IsoTimestamp", occurredAt, SCHEMA_ROOT, CONTINUITY_LIMITS).length,
      0,
      occurredAt,
    );
    assert.throws(
      () => reduceTaskWorkState(emptySnapshot(), startEvent({ occurredAt }), new Map()),
      /occurredAt が暦として実在しない/,
      `reduce: ${occurredAt}`,
    );
    assert.throws(
      () => correlateTerminalEvent(startedSnapshot(), terminalEvent({ occurredAt })),
      /occurredAt が暦として実在しない/,
      `correlate: ${occurredAt}`,
    );
    assert.throws(
      () =>
        finalizeAbandonedState(
          startedSnapshot().state,
          startEvent({ occurredAt, kind: "session_ended", ingestSeq: "12", operation: undefined }),
          new Map(),
        ),
      /occurredAt が暦として実在しない/,
      `abandon: ${occurredAt}`,
    );
  }
  // 締めすぎていないことを通る側でも固定する。実在する日付・閏日・小数秒・秒 59 は落とさない
  for (const occurredAt of [
    "2026-08-16T00:00:01Z",
    "2028-02-29T00:00:00Z", // 2028 年は閏年
    "2000-02-29T00:00:00Z", // 400 で割れる年は閏年
    // 小数秒は暦の判定に使わないので桁数を問わない。ECMA-262 の書式は 3 桁ちょうどで、
    // それ以上の扱いは実装依存（丸め上がる実装だと秒 59 が翌秒に化けて誤拒否になりうる）
    "2026-08-16T23:59:59.999Z",
    "2026-08-16T23:59:59.999999Z",
    "2026-08-16T23:59:59.999999999999Z",
    "2026-01-31T00:00:00Z",
  ]) {
    assert.equal(
      reduceTaskWorkState(emptySnapshot(), startEvent({ occurredAt }), new Map()).outcome,
      "applied",
      occurredAt,
    );
  }
});

test("再配送 start が session を変えたら隔離する", () => {
  // operationId は eventId + matchKey から導出するので session を含まない。assertSameScope は
  // lineage と Agent しか束縛せず、状態は session を持たない。ここで比べないと誰も比べない
  const started = startedSnapshot(startEvent({ operation: MATCH_KEY_ONLY }));
  const moved = reduceTaskWorkState(
    started,
    startEvent({ operation: MATCH_KEY_ONLY, sessionId: "session-2", ingestSeq: "13" }),
    new Map(),
  );
  assert.equal(moved.outcome, "quarantined");
  assert.deepEqual(
    moved.diagnostics.map((d) => d.code),
    ["start_conflict"],
  );
  assert.equal(moved.snapshot.state.pendingOperations[0]?.correlation.sessionId, "session-1");
  // 対照: session が同じ再配送は従来どおり重複
  const same = reduceTaskWorkState(
    started,
    startEvent({ operation: MATCH_KEY_ONLY, ingestSeq: "13" }),
    new Map(),
  );
  assert.deepEqual(
    same.diagnostics.map((d) => d.code),
    ["duplicate_operation_start"],
  );
});

test("放棄で証跡を記録できなかった operation は還元器と同じく報告する", () => {
  const started = startedSnapshot(startEvent({ operation: MATCH_KEY_ONLY }));
  const pending = started.state.pendingOperations[0];
  if (pending === undefined) assert.fail("pending が無い");
  const ended = startEvent({
    eventId: "event-end", adapterDeliveryId: "delivery-end", canonicalFingerprint: "fingerprint-end",
    kind: "session_ended", operation: undefined, ingestSeq: "20",
  });
  const full = Array.from({ length: CONTINUITY_LIMITS.arrayItems }, (_, i) => `filler-${i}`);
  const truncated = finalizeAbandonedState(
    { ...started.state, pendingOperations: [{ ...pending, sourceEventIds: full }] },
    ended,
    new Map(),
  );
  assert.equal(truncated.outcome, "applied");
  assert.equal(truncated.state.pendingOperations[0]?.status, "unknown");
  assert.deepEqual(
    truncated.diagnostics.map((d) => d.code),
    ["source_events_truncated"],
  );
  // 対照: 余裕があれば診断は出ず、証跡が入る
  const recorded = finalizeAbandonedState(started.state, ended, new Map());
  assert.deepEqual(recorded.diagnostics, []);
  assert.equal(recorded.state.pendingOperations[0]?.sourceEventIds.includes("event-end"), true);
});

test("直接呼びの correlateTerminalEvent も envelope の欠落を schema violation にする", () => {
  // 公開 API なので還元器を経由しない呼び出しがありうる。ここを飛ばすと §3.1 違反が
  // 「照合できなかっただけ」の terminal_unmatched に化けて、壊れた証跡がそのまま残る
  assert.throws(
    () => correlateTerminalEvent(emptySnapshot(), terminalEvent({ operation: undefined })),
    /operation envelope が無い/,
  );
});

test("再配送 start が turn を変えたら隔離する", () => {
  // §4.3 は matchKey の入力に「turn when present」を含むので、正しく導出された matchKey なら
  // turn が違えば matchKey も違う。導出は wire 越しに検証できないので、記録された turn を
  // 素通りさせると rule 2 の候補選びが古い turn で絞り、本来の turn の terminal が閉じられない
  const started = startedSnapshot(startEvent({ operation: MATCH_KEY_ONLY }));
  const moved = reduceTaskWorkState(
    started,
    startEvent({ operation: MATCH_KEY_ONLY, turnId: "turn-2", ingestSeq: "13" }),
    new Map(),
  );
  assert.equal(moved.outcome, "quarantined");
  assert.deepEqual(
    moved.diagnostics.map((d) => d.code),
    ["start_conflict"],
  );
  assert.equal(moved.snapshot.state.pendingOperations[0]?.correlation.turnId, "turn-1");
  // `turnId` は OperationCorrelationV1 の required に無く unavailable では正当に不在なので、
  // 片側だけ持たない再配送は隔離しない（復元で欠けた状態に届いた健全な再配送を殺さない）
  const unavailable = reduceTaskWorkState(
    started,
    startEvent({ operation: MATCH_KEY_ONLY, turnId: undefined, turnIdSource: "unavailable", ingestSeq: "13" }),
    new Map(),
  );
  assert.deepEqual(
    unavailable.diagnostics.map((d) => d.code),
    ["duplicate_operation_start"],
  );
});

// --- round 12: intake authority の空白・公開 API の scope・turn 種別（#37） -------

test("空白だけの intake authority 値は native を成立させない", () => {
  // 「未設定」を空文字で表すとは限らない。空白 1 文字・タブ・書式制御文字で表す daemon でも、
  // caller が同じ値を名乗れば一致してしまうので、identity 材料と同じ `isBlank` で落とす
  for (const blank of ["", " ", "\t", "\u{200B}", "\u{FEFF}"]) {
    const label = JSON.stringify(blank);
    const hashBlank = stampIntakeEvidence(
      { ...startEvent(), provenance: { ...startEvent().provenance, capabilityHash: blank } },
      { ...INTAKE, activeCapabilityHash: blank },
    );
    assert.equal(hashBlank.event.provenance.evidenceKind, "synthesized", `capabilityHash ${label}`);
    const agentBlank = stampIntakeEvidence(
      { ...startEvent(), sourceAgent: blank },
      { ...INTAKE, expectedSourceAgent: blank },
    );
    assert.equal(agentBlank.event.provenance.evidenceKind, "synthesized", `sourceAgent ${label}`);
    const versionBlank = stampIntakeEvidence(
      { ...startEvent(), provenance: { ...startEvent().provenance, sourceAgentVersion: blank } },
      { ...INTAKE, exactAgentVersion: blank },
    );
    assert.equal(versionBlank.event.provenance.evidenceKind, "synthesized", `version ${label}`);
    // **session だけは向きが逆**。`expectedSessionId` の空白は「権限のある session を名乗れない
    // 経路」を表すので、authority を消すのではなく**束縛を張らない**（張ると認証できない経路の
    // event が全部落ちる）。空白の表し方が違っても同じに扱われることを固定する: `isBlank` を
    // `!== ""` に狭めると、空白 1 文字の束縛が「実在する session 名」として有効になり、
    // その経路の event が丸ごと `§3.1 違反` で落ちる
    assert.doesNotThrow(
      () =>
        stampIntakeEvidence(
          { ...startEvent(), sessionId: "session-INTRUDER" },
          { ...INTAKE, expectedSessionId: blank },
        ),
      `expectedSessionId ${label}`,
    );
  }
  // 対照: 空白でない束縛は従来どおり効く（緩めていない側も見る）
  assert.throws(
    () =>
      stampIntakeEvidence(
        { ...startEvent(), sessionId: "session-INTRUDER" },
        { ...INTAKE, expectedSessionId: "session-1" },
      ),
    /§3\.1 違反: 束縛された session/,
  );
  // 対照: 空白を含むが空白だけではない値は従来どおり native
  const spaced = "2.1.228 (Claude Code)";
  assert.equal(stampIntakeEvidence(startEvent(), { ...INTAKE, exactAgentVersion: spaced }).event.provenance.evidenceKind, "native");
});

// --- round 13: 受領証の identity 欄と scenarioId の空白（#37） ---

test("欄が空白だけの受領証は native authority の根拠にならない", () => {
  // §3.1 は受領証を「その認証済み取り込みの receipt」と定義し、evidenceKind を「認証済み
  // peer identity」から導けと言う。認証できない経路を undefined ではなく空の受領証で表す
  // daemon では、存在だけを見ると誰も名乗っていない受領証で native が成立する
  for (const blank of ["", " ", "\t", "\u{200B}", "\u{FEFF}"]) {
    const label = JSON.stringify(blank);
    const receiptBlank = stampIntakeEvidence(startEvent(), {
      ...INTAKE,
      attestation: { ...ATTESTATION, ingestReceiptId: blank },
    });
    assert.equal(receiptBlank.event.provenance.evidenceKind, "synthesized", `receiptId ${label}`);
    // 認証が成立しない以上、その version についての turn identity の証明も適用できない
    assert.equal(receiptBlank.event.turnIdSource, "unavailable", `receiptId ${label} turn`);
    const peerBlank = stampIntakeEvidence(startEvent(), {
      ...INTAKE,
      attestation: { ...ATTESTATION, peerIdentityId: blank },
    });
    assert.equal(peerBlank.event.provenance.evidenceKind, "synthesized", `peerId ${label}`);
    assert.equal(peerBlank.event.turnIdSource, "unavailable", `peerId ${label} turn`);
  }
  // 対照: 欄が埋まっている受領証は従来どおり native
  assert.equal(stampIntakeEvidence(startEvent(), INTAKE).event.provenance.evidenceKind, "native");
});

test("空白だけの scenarioId は proven な scenario を名指したことにならない", () => {
  // §3.1 の proven は `scenarioId` が scenario を naming していることを要求する。matrix 側にも
  // 空白の entry がある daemon で、caller が同じ空白を名乗ると等値で proven が成立してしまう
  for (const blank of ["", " ", "\t", "\u{200B}", "\u{FEFF}"]) {
    const stamped = stampIntakeEvidence(
      { ...startEvent(), provenance: { ...startEvent().provenance, scenarioId: blank } },
      { ...INTAKE, provenScenarios: [{ scenarioId: blank, captureMethod: "native_event", channel: "rpc" }] },
    );
    assert.equal(stamped.event.provenance.evidenceKind, "synthesized", JSON.stringify(blank));
  }
  // 対照: 名前が実体を持つなら従来どおり native（空白を含むだけの id は落とさない）
  const spaced = "tool call lifecycle";
  assert.equal(
    stampIntakeEvidence(
      { ...startEvent(), provenance: { ...startEvent().provenance, scenarioId: spaced } },
      { ...INTAKE, provenScenarios: [{ scenarioId: spaced, captureMethod: "native_event", channel: "rpc" }] },
    ).event.provenance.evidenceKind,
    "native",
  );
});

test("直接呼びの correlateTerminalEvent も空白の identity 材料を拒否する", () => {
  // `assertSameScope` は lineage と Agent しか束縛せず、候補の絞り込みは `sessionId` の等値
  // だけを見る。空白の `sessionId` を持つ terminal は、同じく空白の `sessionId` を持つ
  // pending（復元した checkpoint や別実装が書いた状態。凍結 schema に minLength は無い）と
  // 一致して閉じてしまう。空白同士は「同じ session」ではなく「どちらも名乗っていない」
  // 還元器は空白の sessionId を受け付けないので、被害側の状態は「復元した checkpoint」を模して
  // 正常に作った pending の sessionId だけを空にする
  const started = startedSnapshot(startEvent({ operation: MATCH_KEY_ONLY }));
  const victim: TaskWorkStateSnapshotV1 = {
    ...started,
    state: {
      ...started.state,
      pendingOperations: started.state.pendingOperations.map((pending) => ({
        ...pending,
        correlation: { ...pending.correlation, sessionId: "" },
      })),
    },
  };
  for (const blank of ["", " ", "\t", "\u{200B}"]) {
    assert.throws(
      () =>
        correlateTerminalEvent(
          victim,
          terminalEvent({ sessionId: blank, operation: { ...MATCH_KEY_ONLY, phase: "terminal" } }),
        ),
      /sessionId が空文字/,
      JSON.stringify(blank),
    );
  }
  // 対照: 名乗りのある session は従来どおり照合まで進む（ここでは候補ゼロ）
  const ok = correlateTerminalEvent(victim, terminalEvent({ sessionId: "session-2" }));
  if (ok.matched !== null) assert.fail("別 session の pending を閉じた");
  assert.equal(ok.diagnostic, "terminal_orphaned");
});

test("直接呼びの correlateTerminalEvent も decimal string でない ingestSeq を拒否する", () => {
  // §22.6 の制約は `compareIngestSeq` が start を選んだ後にしか走らないので、候補ゼロで
  // 早期 return する経路では検査されない。還元器は入口で落とすのに直接呼びだけが
  // `terminal_orphaned` を返すと、§22.6 違反が正常な結果に化けて壊れた順序証跡が残る
  for (const bad of ["", " ", "007", "1e3", "-1", "1.0", "abc"]) {
    assert.throws(
      () => correlateTerminalEvent(emptySnapshot(), terminalEvent({ ingestSeq: bad })),
      /ingestSeq が decimal string でない/,
      JSON.stringify(bad),
    );
  }
  // 対照: 妥当な decimal string は従来どおり照合の結果（ここでは候補ゼロ）を返す
  const ok = correlateTerminalEvent(emptySnapshot(), terminalEvent({ ingestSeq: "12" }));
  if (ok.matched !== null) assert.fail("候補が無いのに閉じた");
  assert.equal(ok.diagnostic, "terminal_orphaned");
});

test("直接呼びの correlateTerminalEvent も別 Agent の terminal を拒否する", () => {
  // 候補の絞り込みは session と lineage しか見ない。還元器と同じ検査を入口でしないと、
  // 別 Agent の terminal が「権威ある一致」として返り、consumer がそれを適用する
  const started = startedSnapshot(startEvent());
  assert.throws(
    () => correlateTerminalEvent(started, terminalEvent({ sourceAgent: "codex" })),
    /別 Agent の event は適用しない/,
  );
  // 対照: 同じ Agent は従来どおり閉じられる
  assert.equal(correlateTerminalEvent(started, terminalEvent()).matched?.status, "started");
});

test("turn 種別が違う候補は rule 2 の候補から外す", () => {
  // §4.3「rule 2 は双方が同じ turnIdSource 種別の turn 同一性を持つことを要求する」。
  // 同じ matchKey・同じ turnId で種別だけ違う 2 件が並ぶとき、種別で絞れば 1 件になるので
  // rule 2 の「exactly one open candidate」が成立する
  const prepared = apply(emptySnapshot(), [
    startEvent({ eventId: "start-a", adapterDeliveryId: "d-sa", canonicalFingerprint: "f-sa", operation: MATCH_KEY_ONLY, ingestSeq: "11", turnIdSource: "native" }),
    startEvent({ eventId: "start-b", adapterDeliveryId: "d-sb", canonicalFingerprint: "f-sb", operation: MATCH_KEY_ONLY, ingestSeq: "12", turnIdSource: "synthesized_monotonic" }),
  ]);
  assert.equal(prepared.snapshot.state.pendingOperations.length, 2);
  const closed = reduceTaskWorkState(
    prepared.snapshot,
    terminalEvent({ eventId: "term-x", adapterDeliveryId: "d-tx", canonicalFingerprint: "f-tx", ingestSeq: "13", operation: { ...MATCH_KEY_ONLY, phase: "terminal" }, turnIdSource: "native" }),
    prepared.ledger,
  );
  assert.deepEqual(closed.diagnostics, []);
  assert.deepEqual(
    closed.snapshot.state.pendingOperations.map((p) => p.status),
    ["succeeded", "started"],
  );
});

test("turn 種別の材料が無い候補は種別違いとして落とさない", () => {
  // 種別は start 側の材料（`startTurnIdSource`、#35）にしかなく、復元した古い状態では欠ける。材料が無いことを
  // 「種別が違う」と読むと、復元直後の全 terminal が理由を取り違えた診断になる
  const prepared = apply(emptySnapshot(), [
    startEvent({ eventId: "start-a", adapterDeliveryId: "d-sa", canonicalFingerprint: "f-sa", operation: MATCH_KEY_ONLY, ingestSeq: "11", turnIdSource: "native" }),
    startEvent({ eventId: "start-b", adapterDeliveryId: "d-sb", canonicalFingerprint: "f-sb", operation: MATCH_KEY_ONLY, ingestSeq: "12", turnIdSource: "synthesized_monotonic" }),
  ]);
  const restored = withoutStartFacts({ state: prepared.snapshot.state, history: [] });
  const result = correlateTerminalEvent(
    restored,
    terminalEvent({ operation: { ...MATCH_KEY_ONLY, phase: "terminal" }, turnIdSource: "native" }),
  );
  // 材料が無いので 2 件とも残り、種別違いではなく曖昧として報告する
  if (result.matched !== null) assert.fail("材料が無いのに閉じた");
  assert.equal(result.diagnostic, "terminal_ambiguous");
  assert.equal(result.unresolved.length, 2);
});

/**
 * 凍結 `OperationCorrelationV1` の任意欄は `maxLength` しか課さないので、復元した checkpoint や
 * 別実装が書いた状態は schema 妥当なまま空白の欄を持ちうる。空白を「present で食い違う値」と
 * 読むと、還元器は純関数なので**毎回同じ隔離**になり、しかも配送鍵を消費しないので訂正版も
 * 届かない（決定論的な無限再送）。任意欄の空白は「主張していない」として扱う。
 */
function pendingWithBlank(field: "toolName" | "canonicalInputHash" | "nativeOperationId" | "turnId", blank: string) {
  const base = startedSnapshot();
  const pending = base.state.pendingOperations[0] as PendingOperation;
  return {
    ...base,
    state: {
      ...base.state,
      pendingOperations: [{ ...pending, correlation: { ...pending.correlation, [field]: blank } }],
    },
  };
}

for (const blank of ["", " "]) {
  test(`空白の toolName を持つ pending でも rule 2 の terminal が閉じる（${JSON.stringify(blank)}）`, () => {
    const start = startEvent({ operation: { phase: "start", operationMatchKey: "match-key-1", operationKind: "Bash" } });
    const base = startedSnapshot(start);
    const pending = base.state.pendingOperations[0] as PendingOperation;
    const snapshot = {
      ...base,
      state: {
        ...base.state,
        pendingOperations: [{ ...pending, correlation: { ...pending.correlation, toolName: blank } }],
      },
    };
    const terminal = terminalEvent({
      operation: { phase: "terminal", operationMatchKey: "match-key-1", operationKind: "Bash" },
    });
    const result = reduceTaskWorkState(snapshot, terminal, new Map());
    assert.equal(result.outcome, "applied");
    assert.equal(result.snapshot.state.pendingOperations[0]?.status, "succeeded");
  });

  test(`空白の canonicalInputHash を持つ pending は terminal_conflict にしない（${JSON.stringify(blank)}）`, () => {
    const snapshot = pendingWithBlank("canonicalInputHash", blank);
    const result = reduceTaskWorkState(snapshot, terminalEvent(), new Map());
    assert.equal(result.outcome, "applied");
    assert.equal(result.snapshot.state.pendingOperations[0]?.status, "succeeded");
    assert.deepEqual(result.diagnostics, []);
  });

  test(`空白の任意欄を持つ pending への再配送 start は隔離せず埋め直す（${JSON.stringify(blank)}）`, () => {
    // 台帳だけ失った復元。`"" ?? x` は `""` のままなので、`??` で埋めていると一生埋まらない
    for (const field of ["nativeOperationId", "canonicalInputHash", "toolName"] as const) {
      const snapshot = pendingWithBlank(field, blank);
      const result = reduceTaskWorkState(snapshot, startEvent(), new Map());
      assert.equal(result.outcome, "applied", `${field}: ${result.outcome}`);
      assert.deepEqual(
        result.diagnostics.map((d) => d.code),
        ["duplicate_operation_start"],
        field,
      );
      const filled = result.snapshot.state.pendingOperations[0]?.correlation;
      assert.notEqual(filled?.[field], blank, `${field} が空白のまま残った`);
    }
  });

  test(`空白の turnId を持つ pending への再配送 start は start_conflict にしない（${JSON.stringify(blank)}）`, () => {
    const snapshot = pendingWithBlank("turnId", blank);
    const result = reduceTaskWorkState(snapshot, startEvent(), new Map());
    assert.equal(result.outcome, "applied");
    assert.deepEqual(
      result.diagnostics.map((d) => d.code),
      ["duplicate_operation_start"],
    );
  });
}

test("互換な兄弟が複数居るとき、届いた native ID を名乗る側に再配送を帰属させる", () => {
  // `nativeIdTaken` は「2 件目を作らない」だけで帰属は動かさない。相手を配列順で決めると、
  // `withSourceEvent` も truncation 判定も**その native ID を持たないほう**に当たり、再配送の
  // provenance が本来の operation に残らない
  const base = startedSnapshot();
  const derived = base.state.pendingOperations[0] as PendingOperation;
  // 両方とも原因 event を未記録にしておく。記録済みだと `withSourceEvent` が早期 return して
  // どちらを選んでも差が出ない
  const blank: PendingOperation = {
    ...derived,
    sourceEventIds: [],
    correlation: { ...derived.correlation, nativeOperationId: "" },
  };
  const named: PendingOperation = {
    ...derived,
    operationId: "op-already-named",
    sourceEventIds: [],
    correlation: { ...derived.correlation, nativeOperationId: START_OPERATION.nativeOperationId },
  };
  const start = startEvent();
  const result = reduceTaskWorkState(
    { ...base, state: { ...base.state, pendingOperations: [blank, named] } },
    start,
    new Map(),
  );
  assert.equal(result.outcome, "applied");
  assert.deepEqual(
    result.snapshot.state.pendingOperations.map((pending) => pending.sourceEventIds),
    [[], [start.eventId]],
    "native ID を持たないほうに再配送が帰属した",
  );
});

test("名乗っている兄弟が互換でも、空白の native ID を埋めない（2 件目を作らない）", () => {
  // 復元した checkpoint に (1) derived id が一致するが native ID が空白の pending と
  // (2) 届いた native ID を既に名乗る別の pending が並ぶ。両方とも identity は互換。
  // 空白の側へ native ID を書くと**同じ native ID の pending が 2 件**になり、rule 1 は候補 2 件を
  // 曖昧と見るので後続の terminal はどちらも閉じられない。
  // **この経路を守っているのは `nativeIdTaken` ではなく上の帰属優先**: 名乗り手が互換なら
  // `existing` はその名乗り手になり、埋める先が空白でなくなるので `nativeIdTaken` は参照されない
  // （実測: `nativeIdTaken` を消してもこの test は緑のまま。落ちるのは非互換の名乗り手の test）。
  // 互換・非互換で守り手が違うので、2 つの test は別のゲートを固定している
  const blankNative = pendingWithBlank("nativeOperationId", "");
  const derived = blankNative.state.pendingOperations[0] as PendingOperation;
  const alreadyNamed: PendingOperation = {
    ...derived,
    operationId: "op-already-named",
    correlation: { ...derived.correlation, nativeOperationId: START_OPERATION.nativeOperationId },
  };
  const snapshot = {
    ...blankNative,
    state: { ...blankNative.state, pendingOperations: [derived, alreadyNamed] },
  };
  const result = reduceTaskWorkState(snapshot, startEvent(), new Map());
  assert.equal(result.outcome, "applied");
  assert.deepEqual(
    result.diagnostics.map((d) => d.code),
    ["duplicate_operation_start"],
  );
  const naming = result.snapshot.state.pendingOperations.filter(
    (pending) => pending.correlation.nativeOperationId === START_OPERATION.nativeOperationId,
  );
  assert.deepEqual(
    naming.map((pending) => pending.operationId),
    ["op-already-named"],
    "空白側にも native ID が書かれて 2 件になった",
  );
  // 曖昧にならないことまで見る。復元が重複を作っていたら rule 1 の候補が 2 件になり、terminal は
  // 曖昧としてどちらも閉じられない。組み立てた pending は元の start の取り込み連番を持つ（#35）ので、
  // 候補が一意なら診断ゼロで閉じる
  const closed = reduceTaskWorkState(result.snapshot, terminalEvent(), new Map());
  assert.equal(closed.outcome, "applied");
  assert.deepEqual(closed.diagnostics.map((d) => d.code), [], "rule 1 の候補が 1 件に定まっていない");
});

test("別 session の名乗り手は埋め戻しを止めない（rule 1 の候補集合と同じ絞り方）", () => {
  // 抑止の走査集合が session で絞られていないと、rule 1 の候補に入らない名乗り手が埋め戻しを
  // 止める。埋めなかった側も native ID を持たないので、その session の terminal は候補ゼロ =
  // `terminal_orphaned` で隔離される。隔離は鍵を消費せず還元器は純関数なので永久に収束しない
  const base = startedSnapshot();
  const derived = base.state.pendingOperations[0] as PendingOperation;
  const mine: PendingOperation = {
    ...derived,
    correlation: { ...derived.correlation, nativeOperationId: "" },
  };
  // **`operationId` は derived と同じにする**。`idMatches` は session で絞っていないので、
  // 別 session の pending が兄弟に入るのはこの経路（derived id 一致）だけ。別の id にすると
  // そもそも兄弟にならず、走査集合の広さを測れない
  const otherSession: PendingOperation = {
    ...derived,
    correlation: {
      ...derived.correlation,
      sessionId: "session-OTHER",
      nativeOperationId: START_OPERATION.nativeOperationId,
    },
  };
  const result = reduceTaskWorkState(
    { ...base, state: { ...base.state, pendingOperations: [mine, otherSession] } },
    startEvent(),
    new Map(),
  );
  assert.equal(result.outcome, "applied");
  assert.equal(
    result.snapshot.state.pendingOperations[0]?.correlation.nativeOperationId,
    START_OPERATION.nativeOperationId,
    "別 session の名乗り手が埋め戻しを止めた",
  );
  // 埋まっていれば自 session の terminal は閉じられる。止まっていると候補ゼロで隔離され、
  // 同じ terminal が何度来ても同じ隔離になる
  const closed = reduceTaskWorkState(result.snapshot, terminalEvent(), new Map());
  assert.equal(closed.outcome, "applied", "自 session の terminal が隔離された");
});

test("名乗っている兄弟が非互換なら、空白の native ID を埋めない（2 件目を作らない）", () => {
  // 上の優先は `compatible` の中しか見ないので、名乗っている側が**非互換**（identity が
  // 食い違う corruption）だと空振りする。そのまま埋めると同じ native ID の pending が 2 件になり、
  // 後続の terminal は rule 1 で候補 2 件 = 曖昧になってどちらも閉じられない
  const base = startedSnapshot();
  const derived = base.state.pendingOperations[0] as PendingOperation;
  const compatible: PendingOperation = {
    ...derived,
    correlation: { ...derived.correlation, nativeOperationId: "" },
  };
  // matchKey が違うので `startConflictsWith` が真 = 互換な候補には入らない。それでも
  // native ID が一致するので兄弟ではある
  const claimer: PendingOperation = {
    ...derived,
    operationId: "op-native-claimer",
    correlation: {
      ...derived.correlation,
      operationMatchKey: "match-key-OTHER",
      nativeOperationId: START_OPERATION.nativeOperationId,
    },
  };
  const snapshot = {
    ...base,
    state: { ...base.state, pendingOperations: [compatible, claimer] },
  };
  const result = reduceTaskWorkState(snapshot, startEvent(), new Map());
  assert.equal(result.outcome, "applied");
  const naming = result.snapshot.state.pendingOperations.filter(
    (pending) => pending.correlation.nativeOperationId === START_OPERATION.nativeOperationId,
  );
  assert.deepEqual(
    naming.map((pending) => pending.operationId),
    ["op-native-claimer"],
    "非互換な名乗り手が居るのに空白側へ書いて 2 件になった",
  );
  // 衝突していた兄弟は診断で名指しする（黙って飛ばさない）
  assert.deepEqual(
    result.diagnostics.map((d) => d.code).sort(),
    ["duplicate_operation_start", "start_sibling_conflict"],
  );
  // **どちらが閉じるかまで固定する**。曖昧にならないことだけを見ると、閉じる先が入れ替わる
  // 退行が緑のまま通る。この状態で rule 1 が見るのは native ID なので、閉じるのは**名乗って
  // いる側**（`op-native-claimer`）で、再配送の provenance を受け取った側ではない。
  // **既知の残渣**（#58）: 直前に corruption として名指しした兄弟のほうが閉じ、真の相手は
  // `started` のまま退避まで残る。ここで別の選び方をすると収束しない形（隔離は鍵を消費せず
  // 還元器は純関数）か、同じ native ID の pending 2 件（rule 1 が曖昧）のどちらかになるので、
  // 状態が壊れている以上どれかは失う。失う先を固定して観測できるようにするのが今の選択
  const closed = reduceTaskWorkState(result.snapshot, terminalEvent(), new Map());
  assert.equal(closed.outcome, "applied");
  assert.ok(
    !closed.diagnostics.some((d) => d.code === "terminal_ambiguous"),
    "rule 1 の候補が 2 件になっている",
  );
  assert.deepEqual(
    closed.snapshot.state.pendingOperations.map((pending) => [pending.operationId, pending.status]),
    [
      [derived.operationId, "started"],
      ["op-native-claimer", "succeeded"],
    ],
    "閉じる先が入れ替わった",
  );
});

test("provenance が無い event は intake でも §3.1 で落ちる（書く層にも置く）", () => {
  // intake は生の adapter 出力に最初に触る層。`event.provenance` を素で destructure するので、
  // 還元器側のガードだけだと、通常の経路（intake → reduce）では節を名乗らない TypeError で死ぬ
  const withoutProvenance = { ...startEvent(), provenance: undefined } as unknown as NormalizedContinuityEvent;
  assert.throws(() => stampIntakeEvidence(withoutProvenance, INTAKE), /§3\.1 違反: provenance が無い/);
});

test("provenance が無い event は TypeError でなく §3.1 で落ちる", () => {
  // この経路の他の検査はすべて `§3.1 違反:` / `§22.6 違反:` の形で投げ、`rejected-events.json` は
  // case ごとに 1 つの節を宣言して TS/Rust パリティの基準にしている。節を名乗らない TypeError は
  // その分類契約から外れる
  const withoutProvenance = { ...startEvent(), provenance: undefined } as unknown as Parameters<
    typeof reduceTaskWorkState
  >[1];
  assert.throws(
    () => reduceTaskWorkState(emptySnapshot(), withoutProvenance, new Map()),
    /§3\.1 違反: provenance が無い/,
  );
});

test("届いた start が native ID を持たないときは、名乗る兄弟を優先せず配列順で選ぶ", () => {
  // 優先してよい根拠は「互換の定義上、名乗っている値は届いたものと一致している」だが、
  // `startConflictsWith` の native ID 比較は両方が declared のときだけ走る。届いた側が持たない
  // なら一致は一度も確かめていないので、優先の根拠が無い。根拠が無いまま選び先を変えると、
  // `operationId` が重複する状態で配列順とは違う相手が選ばれ、**空白の native ID がどちらの
  // 兄弟から落ちるかが変わる**
  const start = startEvent({ operation: MATCH_KEY_ONLY });
  const base = startedSnapshot(start);
  const pending = { ...(base.state.pendingOperations[0] as PendingOperation), sourceEventIds: [] };
  // **名乗っている側を先頭に置く**。後ろに置くと、届いた native ID が無いときの `find` は
  // 「native ID を持たない兄弟」を拾って結果が配列順と一致してしまい、ガードの有無を区別できない
  const snapshot = {
    ...base,
    state: {
      ...base.state,
      pendingOperations: [
        { ...pending, correlation: { ...pending.correlation, nativeOperationId: "toolu_other" } },
        { ...pending, correlation: { ...pending.correlation, nativeOperationId: "" } },
      ],
    },
  };
  const result = reduceTaskWorkState(snapshot, start, new Map());
  assert.equal(result.outcome, "applied");
  assert.deepEqual(
    result.snapshot.state.pendingOperations.map((entry) => entry.sourceEventIds),
    [[start.eventId], []],
    "配列順でなく native ID の有無で帰属先が決まった",
  );
  // 届いた start に書く値が無いので、選ばれた側の欄は動かない。空白側も選ばれていないので残る
  assert.deepEqual(
    result.snapshot.state.pendingOperations.map((entry) => entry.correlation.nativeOperationId),
    ["toolu_other", ""],
    "選んでいないほうの空白まで正規化した",
  );
});

test("空白でない任意欄の食い違いは今までどおり衝突として落とす", () => {
  // 締めすぎの逆方向。空白を通す変更が「任意欄の検査そのもの」を殺していないことを固定する
  const snapshot = pendingWithBlank("canonicalInputHash", "input-hash-OTHER");
  const result = reduceTaskWorkState(snapshot, terminalEvent(), new Map());
  assert.equal(result.outcome, "quarantined");
  assert.deepEqual(
    result.diagnostics.map((d) => d.code),
    ["terminal_conflict"],
  );
});

test("受領証の attestedAt も暦検査を受ける（凍結 $comment は IsoTimestamp 型に対する約束）", () => {
  // 凍結 schema の `IsoTimestamp` の `$comment` は「暦としての実在は runtime validator が
  // 受け持つ」と型そのものに対して書いており、`$def` は 20 近い欄から参照されている。
  // event が持ち込む IsoTimestamp は occurredAt と attestedAt の 2 つ
  const bad = startEvent({
    provenance: {
      ...startEvent().provenance,
      ingestAttestation: { ...ATTESTATION, attestedAt: "2026-02-30T00:00:00Z" },
    },
  });
  assert.throws(
    () => reduceTaskWorkState(emptySnapshot(), bad, new Map()),
    /attestedAt が暦として実在しない/,
  );
  // 締めすぎていないことも固定する
  assert.equal(reduceTaskWorkState(emptySnapshot(), startEvent(), new Map()).outcome, "applied");
});
