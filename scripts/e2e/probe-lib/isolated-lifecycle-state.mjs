import fs from "node:fs";
import { DatabaseSync } from "node:sqlite";

import { PreconditionError } from "./process.mjs";

const CODEX_LIFECYCLE_RUN = "2026-09-05T06-02-58-033Z";
export const CODEX_CLEAR_RUN = "2026-09-05T07-03-44-495Z";

function parsePayload(value) {
  try {
    return JSON.parse(String(value ?? "{}"));
  } catch {
    return {};
  }
}

function readLifecycleEvents(db, agent) {
  const sessions = db
    .prepare(
      `SELECT id, repo_id AS repoId, native_session_id AS nativeSessionId,
                conversation_id AS conversationId, context_epoch AS contextEpoch,
                status, summary_state AS summaryState
         FROM sessions WHERE agent = ? ORDER BY started_at, id`,
    )
    .all(agent)
    .map((row) => ({ ...row, contextEpoch: Number(row.contextEpoch) }));
  const events = db
    .prepare(
      `SELECT e.id, e.session_id AS sessionId, s.native_session_id AS nativeSessionId,
                e.kind, e.payload_json AS payloadJson, e.captured_at AS capturedAt
         FROM raw_events e JOIN sessions s ON s.id = e.session_id
         WHERE s.agent = ? ORDER BY e.captured_at, e.rowid`,
    )
    .all(agent)
    .map(({ payloadJson, ...row }) => ({ ...row, payload: parsePayload(payloadJson) }));
  return { sessions, events };
}

/** Read-only evidence for the lifecycle assertions. */
export function inspectLifecycle(databasePath, agent) {
  if (!fs.existsSync(databasePath)) throw new PreconditionError(`The oboete database is missing: ${databasePath}.`);
  const db = new DatabaseSync(databasePath, { readOnly: true, timeout: 1000 });
  try {
    const { sessions, events } = readLifecycleEvents(db, agent);
    const injections = db
      .prepare(
        `SELECT i.id, i.session_id AS sessionId, i.conversation_id AS conversationId,
                i.kind, i.channel, i.state, i.context_epoch AS contextEpoch,
                i.pack_hash AS packHash, i.delivery_count AS deliveryCount
         FROM injections i JOIN sessions s ON s.id = i.session_id
         WHERE s.agent = ? ORDER BY i.created_at, i.id`,
      )
      .all(agent)
      .map((row) => ({
        ...row,
        contextEpoch: Number(row.contextEpoch),
        deliveryCount: Number(row.deliveryCount ?? 0),
      }));
    const items = db
      .prepare(
        `SELECT ii.id, ii.injection_id AS injectionId, ii.conversation_id AS conversationId,
                ii.memory_id AS memoryId, ii.decision
         FROM injection_items ii
         JOIN injections i ON i.id = ii.injection_id
         JOIN sessions s ON s.id = i.session_id
         WHERE s.agent = ? ORDER BY ii.id`,
      )
      .all(agent);
    const memories = db
      .prepare(
        `SELECT id, repo_id AS repoId, type, source_session_id AS sourceSessionId, deleted_at AS deletedAt
         FROM memories
         WHERE repo_id IN (SELECT repo_id FROM sessions WHERE agent = ?)
         ORDER BY id`,
      )
      .all(agent);
    return { sessions, events, injections, items, memories };
  } finally {
    db.close();
  }
}

export function assertion(asserts, statement, pass, expected, actual) {
  asserts.push({ assertion: statement, pass: Boolean(pass), expected, actual });
}

export function session(snapshot, nativeSessionId) {
  return snapshot.sessions.find((row) => row.nativeSessionId === nativeSessionId);
}

export function added(before, after, field) {
  const known = new Set(before[field].map((row) => row.id));
  return after[field].filter((row) => !known.has(row.id));
}

export function eventDelta(before, after) {
  return added(before, after, "events").map((event) => ({
    id: event.id,
    session_id: event.sessionId,
    native_session_id: event.nativeSessionId,
    kind: event.kind,
    payload: event.payload,
    captured_at: event.capturedAt,
  }));
}

export function eventSource(event) {
  return typeof event.payload?.source === "string" ? event.payload.source : null;
}

function injectionFingerprint(snapshot, conversationId, kind) {
  return snapshot.injections
    .filter((row) => row.conversationId === conversationId && (kind === undefined || row.kind === kind))
    .map((row) => `${row.id}:${row.state}:${row.packHash ?? ""}:${row.deliveryCount}`)
    .sort();
}

function repoMemoryIds(snapshot, repoId) {
  return new Set(
    snapshot.memories
      .filter((memory) => memory.repoId === repoId && memory.deletedAt === null)
      .map((memory) => memory.id),
  );
}

export function includedMemoryIds(snapshot, matches) {
  return new Set(
    snapshot.items
      .filter((item) => item.decision === "included" && item.memoryId !== null && matches(item))
      .map((item) => item.memoryId),
  );
}

function includesRepositoryMemory(repository, included) {
  return [...included].some((memoryId) => repository.has(memoryId));
}

export function evaluated(assertions) {
  const failed = assertions.filter((item) => !item.pass);
  return {
    status: failed.length === 0 ? "pass" : "fail",
    assertions,
    ...(failed.length === 0 ? {} : { reason: failed.map((item) => item.assertion).join("; ") }),
  };
}

function evaluateResumeCheck(assertions, agent, before, after, parentBefore, parentAfter) {
  assertion(
    assertions,
    "resume keeps the same oboete session and conversation",
    parentBefore !== undefined &&
      parentAfter !== undefined &&
      parentAfter.id === parentBefore.id &&
      parentAfter.conversationId === parentBefore.conversationId,
    parentBefore === undefined ? null : { id: parentBefore.id, conversationId: parentBefore.conversationId },
    parentAfter === undefined ? null : { id: parentAfter.id, conversationId: parentAfter.conversationId },
  );
  assertion(
    assertions,
    "resume leaves context_epoch unchanged",
    parentBefore !== undefined && parentAfter?.contextEpoch === parentBefore.contextEpoch,
    parentBefore?.contextEpoch ?? null,
    parentAfter?.contextEpoch ?? null,
  );
  const starts = added(before, after, "events").filter(
    (event) => event.sessionId === parentAfter?.id && event.kind === "session_start" && eventSource(event) === "resume",
  );
  assertion(
    assertions,
    agent === "codex"
      ? "Codex production hooks omit SessionStart source=resume"
      : "Claude records one SessionStart source=resume",
    starts.length === (agent === "codex" ? 0 : 1),
    agent === "codex" ? 0 : 1,
    starts.length,
  );
  const prompts = added(before, after, "events").filter(
    (event) => event.sessionId === parentAfter?.id && event.kind === "prompt",
  );
  assertion(assertions, "resume records one prompt on the resumed session", prompts.length === 1, 1, prompts.length);
  const beforeStart = injectionFingerprint(before, parentBefore?.conversationId, "session_start");
  const afterStart = injectionFingerprint(after, parentAfter?.conversationId, "session_start");
  assertion(
    assertions,
    "resume adds no session-start injection",
    JSON.stringify(afterStart) === JSON.stringify(beforeStart),
    beforeStart,
    afterStart,
  );
  return evaluated(assertions);
}

function assertCompactPromptOrder(assertions, agent, compactStarts, newEvents) {
  if (agent === "codex") {
    const start = compactStarts[0];
    const prompt = newEvents.find((event) => event.kind === "prompt");
    assertion(
      assertions,
      `the compact SessionStart precedes the next prompt (lazy hook, run ${CODEX_LIFECYCLE_RUN})`,
      Number.isFinite(start?.capturedAt) && Number.isFinite(prompt?.capturedAt) &&
        start.capturedAt < prompt.capturedAt,
      "SessionStart captured_at < next prompt captured_at",
      { sessionStart: start?.capturedAt ?? null, prompt: prompt?.capturedAt ?? null },
    );
  }
}

function assertCompactPack(assertions, agent, before, after, parentBefore, parentAfter) {
  const epoch = parentBefore === undefined ? null : parentBefore.contextEpoch + 1;
  const channel = `${agent}:SessionStart`;
  const packs = added(before, after, "injections").filter(
    (injection) =>
      injection.sessionId === parentAfter?.id &&
      injection.kind === "session_start",
  );
  assertion(
    assertions,
    `compaction emits one new-epoch session-start pack through ${channel}`,
    packs.length === 1 && packs[0].channel === channel &&
      packs[0].state === "emitted" && packs[0].contextEpoch === epoch,
    [{ channel, state: "emitted", contextEpoch: epoch }],
    packs.map(({ channel, state, contextEpoch }) => ({ channel, state, contextEpoch })),
  );
  const packIds = new Set(packs.map((injection) => injection.id));
  const included = includedMemoryIds(after, (item) => packIds.has(item.injectionId));
  const repository = repoMemoryIds(after, parentBefore?.repoId);
  assertion(
    assertions,
    "the compact session-start pack includes repository memory",
    includesRepositoryMemory(repository, included),
    [...repository],
    [...included],
  );
}

function evaluateCompactCheck(assertions, agent, before, after, parentBefore, parentAfter) {
  const newEvents = added(before, after, "events").filter((event) => event.sessionId === parentAfter?.id);
  const compactions = newEvents.filter((event) => event.kind === "compaction_summary");
  const compactStarts = newEvents.filter(
    (event) => event.kind === "session_start" && eventSource(event) === "compact",
  );
  assertion(
    assertions,
    "compaction advances context_epoch exactly once",
    parentBefore !== undefined && parentAfter?.contextEpoch === parentBefore.contextEpoch + 1,
    parentBefore === undefined ? null : parentBefore.contextEpoch + 1,
    parentAfter?.contextEpoch ?? null,
  );
  assertion(assertions, "one compaction event is recorded", compactions.length === 1, 1, compactions.length);
  assertion(
    assertions,
    "one SessionStart source=compact is recorded",
    compactStarts.length === 1,
    1,
    compactStarts.length,
  );
  assertCompactPromptOrder(assertions, agent, compactStarts, newEvents);
  assertCompactPack(assertions, agent, before, after, parentBefore, parentAfter);
  const remaining = new Set(after.events.map((event) => event.id));
  const lost = before.events
    .filter((event) => event.sessionId === parentBefore?.id)
    .map((event) => event.id)
    .filter((id) => !remaining.has(id));
  assertion(assertions, "no event captured before compaction is lost", lost.length === 0, [], lost);
  return evaluated(assertions);
}

function assertClaudeForkEvents(assertions, agent, before, after, child) {
  if (agent === "claude") {
    const forkStarts = added(before, after, "events").filter(
      (event) => event.sessionId === child?.id && event.kind === "session_start" && eventSource(event) === "fork",
    );
    assertion(assertions, "Claude fork records one SessionStart source=fork", forkStarts.length === 1, 1, forkStarts.length);
    const starts = after.injections.filter(
      (injection) => injection.sessionId === child?.id && injection.kind === "session_start",
    );
    assertion(assertions, "Claude fork emits no SessionStart pack", starts.length === 0, 0, starts.length);
  }
}

function evaluateForkCheck(options) {
  const { assertions, agent, before, after, parentBefore, parentAfter, child, memories } = options;
  const included = includedMemoryIds(after, (item) => item.conversationId === child?.conversationId);
  assertion(
    assertions,
    "fork creates a separate root conversation",
    child !== undefined &&
      parentBefore !== undefined &&
      child.id !== parentBefore.id &&
      child.conversationId === child.id &&
      child.conversationId !== parentBefore.conversationId,
    "new session whose conversation_id equals its id and differs from the parent",
    child === undefined ? null : { id: child.id, conversationId: child.conversationId },
  );
  assertion(
    assertions,
    "fork keeps the parent repository identity",
    child !== undefined && child.repoId === parentBefore?.repoId,
    parentBefore?.repoId ?? null,
    child?.repoId ?? null,
  );
  assertion(
    assertions,
    "fork includes a memory from the parent repository",
    includesRepositoryMemory(memories, included),
    [...memories],
    [...included],
  );
  const beforeParent = injectionFingerprint(before, parentBefore?.conversationId);
  const afterParent = injectionFingerprint(after, parentAfter?.conversationId);
  assertion(
    assertions,
    "fork adds no injection to the parent conversation",
    JSON.stringify(afterParent) === JSON.stringify(beforeParent),
    beforeParent,
    afterParent,
  );
  assertClaudeForkEvents(assertions, agent, before, after, child);
  return evaluated(assertions);
}

function assertClearIdentity(assertions, agent, before, child, parentBefore) {
  if (agent === "codex") {
    assertion(
      assertions,
      "Codex /new creates a fresh root conversation in the same repository",
      child !== undefined &&
        parentBefore !== undefined &&
        !before.sessions.some((row) => row.id === child.id) &&
        child.id !== parentBefore.id &&
        child.conversationId === child.id &&
        child.conversationId !== parentBefore.conversationId &&
        child.repoId === parentBefore.repoId,
      "new root in the parent repository",
      child === undefined ? null : { id: child.id, conversationId: child.conversationId, repoId: child.repoId },
    );
  } else {
    assertion(
      assertions,
      "Claude clear stays in the parent repository",
      child !== undefined && child.repoId === parentBefore?.repoId,
      parentBefore?.repoId ?? null,
      child?.repoId ?? null,
    );
  }
}

function assertCodexClearStart(assertions, before, beforePrompt, newEvents) {
  const commandEvents = beforePrompt === undefined ? [] : added(before, beforePrompt, "events");
  const commandSessions = beforePrompt === undefined ? [] : added(before, beforePrompt, "sessions");
  const earlyStarts = commandEvents.filter((event) => event.kind === "session_start");
  assertion(
    assertions,
    "Codex /new creates no oboete session or session_start before the recall prompt is submitted (A18 detection at the first turn)",
    beforePrompt !== undefined && commandSessions.length === 0 && earlyStarts.length === 0,
    { sessions: 0, sessionStarts: 0 },
    { sessions: commandSessions.length, sessionStarts: earlyStarts.length },
  );
  const starts = newEvents.filter((event) => event.kind === "session_start");
  assertion(
    assertions,
    "Codex /new records exactly one SessionStart source=startup on the child",
    starts.length === 1 && eventSource(starts[0]) === "startup",
    ["startup"],
    starts.map(eventSource),
  );
  const start = starts[0];
  const prompt = newEvents.find((event) => event.kind === "prompt");
  assertion(
    assertions,
    `Codex /new SessionStart source=startup precedes the child's prompt (lazy hook, run ${CODEX_CLEAR_RUN})`,
    Number.isFinite(start?.capturedAt) && Number.isFinite(prompt?.capturedAt) &&
      start.capturedAt < prompt.capturedAt,
    "SessionStart captured_at < child prompt captured_at",
    { sessionStart: start?.capturedAt ?? null, prompt: prompt?.capturedAt ?? null },
  );
}

function assertClearPack(assertions, agent, before, after, child, memories) {
  const channel = `${agent}:SessionStart`;
  const starts = added(before, after, "injections").filter(
    (injection) =>
      injection.sessionId === child?.id &&
      injection.kind === "session_start",
  );
  assertion(
    assertions,
    `${agent} clear emits one session-start pack through ${channel}`,
    starts.length === 1 && starts[0].channel === channel && starts[0].state === "emitted",
    [{ channel, state: "emitted" }],
    starts.map(({ channel, state }) => ({ channel, state })),
  );
  const startIds = new Set(starts.map((injection) => injection.id));
  const clearMemoryIds = includedMemoryIds(after, (item) => startIds.has(item.injectionId));
  assertion(
    assertions,
    "clear includes a memory from the parent repository",
    includesRepositoryMemory(memories, clearMemoryIds),
    [...memories],
    [...clearMemoryIds],
  );
}

function evaluateClearCheck(options) {
  const { assertions, agent, before, beforePrompt, after, parentBefore, parentAfter, child, memories } = options;
  assertClearIdentity(assertions, agent, before, child, parentBefore);
  const newEvents = added(before, after, "events").filter((event) => event.sessionId === child?.id);
  if (agent === "codex") {
    assertCodexClearStart(assertions, before, beforePrompt, newEvents);
    const beforeParent = injectionFingerprint(before, parentBefore?.conversationId);
    const afterParent = injectionFingerprint(after, parentAfter?.conversationId);
    assertion(
      assertions,
      "Codex /new leaves the parent conversation's injections unchanged",
      JSON.stringify(afterParent) === JSON.stringify(beforeParent),
      beforeParent,
      afterParent,
    );
    assertion(
      assertions,
      `Codex parent stays active because /new fires no SessionEnd (run ${CODEX_CLEAR_RUN})`,
      parentAfter?.status === "active",
      "active",
      parentAfter?.status ?? null,
    );
  } else {
    const starts = newEvents.filter(
      (event) => event.kind === "session_start" && eventSource(event) === "clear",
    );
    assertion(assertions, "Claude clear records one SessionStart source=clear", starts.length === 1, 1, starts.length);
  }
  assertClearPack(assertions, agent, before, after, child, memories);
  return {
    ...evaluated(assertions),
    ...(agent === "codex" ? { evidence: {
      parent_session_end_count: added(before, after, "events").filter(
        (event) => event.sessionId === parentBefore?.id && event.kind === "session_end",
      ).length,
    } } : {}),
  };
}

/** Apply the contracts/agents.md identity rules to evidence captured around one real CLI action. */
export function evaluateLifecycleCheck({
  agent,
  check,
  before,
  beforePrompt,
  after,
  parentNativeSessionId,
  childNativeSessionId,
}) {
  const assertions = [];
  const parentBefore = session(before, parentNativeSessionId);
  const parentAfter = session(after, parentNativeSessionId);

  if (check === "resume") {
    return evaluateResumeCheck(assertions, agent, before, after, parentBefore, parentAfter);
  }

  if (check === "compact") {
    return evaluateCompactCheck(assertions, agent, before, after, parentBefore, parentAfter);
  }

  const child = session(after, childNativeSessionId);
  const memories = repoMemoryIds(after, parentBefore?.repoId);

  if (check === "fork") {
    return evaluateForkCheck({ assertions, agent, before, after, parentBefore, parentAfter, child, memories });
  }

  if (check === "clear") {
    return evaluateClearCheck({ assertions, agent, before, beforePrompt, after, parentBefore, parentAfter, child, memories });
  }

  throw new Error(`Unknown lifecycle check: ${check}.`);
}
