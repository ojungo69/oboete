import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";

import { observerLeaseIsFree } from "./probe-lib/isolated-agent.mjs";
import { evaluateLifecycleCheck, inspectLifecycle } from "./probe-lib/isolated-lifecycle-state.mjs";
import {
  addMemoryInjection,
  childSession,
  lifecycleDatabase,
  lifecycleSnapshot,
} from "./isolated-user.test-support.mjs";

test("inspectLifecycle reads the identity and delivery evidence without writing", (t) => {
  const { directory, file, db } = lifecycleDatabase(t);
  const snapshot = inspectLifecycle(file, "codex");
  assert.equal(snapshot.sessions[0].contextEpoch, 1);
  assert.equal(snapshot.sessions[0].status, "active");
  assert.equal(snapshot.sessions[0].summaryState, "pending");
  assert.equal(snapshot.events[0].payload.source, "compact");
  assert.equal(snapshot.events[0].capturedAt, 2);
  assert.equal(snapshot.injections[0].channel, "codex:SessionStart");
  assert.equal(snapshot.items[0].memoryId, "memory");
  assert.equal(snapshot.memories[0].repoId, "repo");
  assert.equal(snapshot.memories[0].sourceSessionId, "session");
  assert.equal(snapshot.memories[0].type, "session_summary");
  assert.equal(observerLeaseIsFree(file), true);
  const now = 100_000;
  for (const [heartbeat, free] of [
    [now, false], [now - 6_000, false], [now - 6_001, true],
    [now + 60_000, false], [now + 60_001, true], [null, true],
  ]) {
    db.prepare("UPDATE worker_lease SET owner_token = 'worker', heartbeat_at = ? WHERE id = 1").run(heartbeat);
    assert.equal(observerLeaseIsFree(file, now), free, `heartbeat ${heartbeat}`);
  }
  db.prepare("UPDATE worker_lease SET owner_token = NULL WHERE id = 1").run();
  assert.equal(observerLeaseIsFree(file), true);
  const missing = path.join(directory, "missing.db");
  assert.equal(observerLeaseIsFree(missing), true);
  assert.equal(fs.existsSync(missing), false, "lease inspection must not create a database");
});

function compactLifecycleState(agent) {
  const before = lifecycleSnapshot(agent);
  const after = structuredClone(before);
  after.sessions[0].contextEpoch = 1;
  after.events.push({
    id: "event-compact", sessionId: "parent", nativeSessionId: "native-parent",
    kind: "compaction_summary", payload: {}, capturedAt: 3,
  });
  after.events.push({
    id: "event-compact-start", sessionId: "parent", nativeSessionId: "native-parent",
    kind: "session_start", payload: { source: "compact" }, capturedAt: 4,
  });
  if (agent === "codex") after.events.push({
    id: "event-next-prompt", sessionId: "parent", nativeSessionId: "native-parent",
    kind: "prompt", payload: {}, capturedAt: 5,
  });
  addMemoryInjection(after, agent, "session_start", `${agent}:SessionStart`, "parent", 1);
  after.memories.push({ ...after.memories[0], id: "new-summary" });
  after.items.at(-1).memoryId = "new-summary";

  const result = evaluateLifecycleCheck({
    agent,
    check: "compact",
    before,
    after,
    parentNativeSessionId: "native-parent",
  });
  assert.equal(result.status, "pass", result.reason);

  const start = (snapshot) => snapshot.events.find((event) => event.id === "event-compact-start");
  return { before, after, result, start };
}

function assertRejectedCompactMutations(agent, before, after, start) {
  for (const [label, mutate, reason] of [
    ["missing compact start", (snapshot) => { snapshot.events = snapshot.events.filter((event) => event.id !== "event-compact-start"); }, /SessionStart source=compact/],
    ["wrong source", (snapshot) => { start(snapshot).payload.source = "startup"; }, /SessionStart source=compact/],
    ["wrong session", (snapshot) => { start(snapshot).sessionId = "other"; }, /SessionStart source=compact/],
    ["duplicate compact start", (snapshot) => snapshot.events.push({ ...start(snapshot), id: "duplicate" }), /SessionStart source=compact/],
    ["duplicate compaction", (snapshot) => snapshot.events.push({ ...snapshot.events.find((event) => event.kind === "compaction_summary"), id: "duplicate" }), /one compaction event/],
    ["prompt fallback", (snapshot) => { snapshot.injections[0].channel = `${agent}:UserPromptSubmit`; }, /session-start pack/],
    ["pending pack", (snapshot) => { snapshot.injections[0].state = "pending"; }, /session-start pack/],
    ["old epoch pack", (snapshot) => { snapshot.injections[0].contextEpoch = 0; }, /session-start pack/],
    ["duplicate pack", (snapshot) => snapshot.injections.push({ ...snapshot.injections[0], id: "duplicate" }), /session-start pack/],
    ["extra fallback pack", (snapshot) => snapshot.injections.push({ ...snapshot.injections[0], id: "fallback", channel: `${agent}:UserPromptSubmit` }), /session-start pack/],
    ["extra pending pack", (snapshot) => snapshot.injections.push({ ...snapshot.injections[0], id: "pending", state: "pending" }), /session-start pack/],
    ["extra old epoch pack", (snapshot) => snapshot.injections.push({ ...snapshot.injections[0], id: "old-epoch", contextEpoch: 0 }), /session-start pack/],
    ["no repository memory", (snapshot) => { snapshot.items[0].memoryId = "other-repo-memory"; }, /includes repository memory/],
    ["deleted repository memory", (snapshot) => { snapshot.memories.at(-1).deletedAt = 6; }, /includes repository memory/],
    ["foreign repository memory", (snapshot) => { snapshot.memories.at(-1).repoId = "other-repo"; }, /includes repository memory/],
    ["lost earlier event", (snapshot) => snapshot.events.shift(), /no event captured before compaction is lost/],
  ]) {
    const invalid = structuredClone(after);
    mutate(invalid);
    const rejected = evaluateLifecycleCheck({
      agent, check: "compact", before, after: invalid, parentNativeSessionId: "native-parent",
    });
    assert.equal(rejected.status, "fail", label);
    assert.match(rejected.reason, reason, label);
  }
}

function addClearLifecycleEvents(after, agent) {
  after.events.push({
    id: "event-clear-start",
    sessionId: "child",
    nativeSessionId: "native-child",
    kind: "session_start",
    payload: { source: agent === "codex" ? "startup" : "clear" },
    capturedAt: 3,
  });
  after.events.push(
    {
      id: "event-clear-prompt",
      sessionId: "child",
      nativeSessionId: "native-child",
      kind: "prompt",
      payload: {},
      capturedAt: 4,
    },
    {
      id: "event-clear-end",
      sessionId: "child",
      nativeSessionId: "native-child",
      kind: "turn_end",
      payload: {},
      capturedAt: 5,
    },
  );
  addMemoryInjection(
    after,
    agent,
    "session_start",
    `${agent}:SessionStart`,
  );
  after.memories.push({ ...after.memories[0], id: "new-summary" });
  after.items.at(-1).memoryId = "new-summary";
}

function clearLifecycleState(agent) {
  const before = lifecycleSnapshot(agent);
  if (agent === "codex") {
    Object.assign(before.sessions[0], { status: "active", summaryState: null });
    Object.assign(before.memories[0], { sourceSessionId: "seed", type: "session_summary" });
  }
  addMemoryInjection(before, agent, "session_start", `${agent}:SessionStart`, "parent");
  const beforePrompt = structuredClone(before);
  const after = structuredClone(before);
  after.sessions.push(childSession());
  addClearLifecycleEvents(after, agent);

  const result = evaluateLifecycleCheck({
    agent,
    check: "clear",
    before,
    beforePrompt: agent === "codex" ? beforePrompt : undefined,
    after,
    parentNativeSessionId: "native-parent",
    childNativeSessionId: "native-child",
  });
  assert.equal(result.status, "pass", result.reason);
  return { before, beforePrompt, after, result };
}

function assertRejectedClearMutations(agent, before, beforePrompt, after) {
  for (const [label, mutate, reason] of [
    ["wrong injection channel", (snapshot) => { snapshot.injections[1].channel = `${agent}:UserPromptSubmit`; }, /emits one session-start pack through/],
    ...(agent === "claude" ? [
      ["missing clear SessionStart", (snapshot) => { snapshot.events = snapshot.events.filter((event) => event.id !== "event-clear-start"); }, /Claude clear records one SessionStart source=clear/],
    ] : []),
  ]) {
    const invalid = structuredClone(after);
    mutate(invalid);
    const rejected = evaluateLifecycleCheck({
      agent, check: "clear", before, beforePrompt, after: invalid,
      parentNativeSessionId: "native-parent", childNativeSessionId: "native-child",
    });
    assert.equal(rejected.status, "fail", label);
    assert.equal(rejected.assertions.find((item) => reason.test(item.assertion))?.pass, false, label);
  }
}

function assertCodexClearLifecycle(before, beforePrompt, after, result, agent) {
  assert.equal(result.evidence.parent_session_end_count, 0);
  const parentState = result.assertions.find((item) => /parent stays active/.test(item.assertion));
  assert.equal(parentState.actual, "active");
  assert.match(parentState.assertion, /\/new fires no SessionEnd.*2026-09-05T07-03-44-495Z/);
  assert.match(result.assertions.find((item) => /startup.*precedes/.test(item.assertion)).assertion, /2026-09-05T07-03-44-495Z/);
  assert.ok(result.assertions.find((item) => /includes a memory from the parent repository/.test(item.assertion)).expected.includes("new-summary"));
  const start = (snapshot) => snapshot.events.find((event) => event.id === "event-clear-start");
  for (const [label, mutate, reason] of [
    ["missing startup", (snapshot) => { snapshot.events = snapshot.events.filter((event) => event.id !== "event-clear-start"); }, /one SessionStart source=startup/],
    ["late startup", (snapshot) => { start(snapshot).capturedAt = 6; }, /startup.*precedes/],
    ["simultaneous startup", (snapshot) => { start(snapshot).capturedAt = 4; }, /startup.*precedes/],
    ["missing timestamp", (snapshot) => { delete start(snapshot).capturedAt; }, /startup.*precedes/],
    ["null timestamp", (snapshot) => { start(snapshot).capturedAt = null; }, /startup.*precedes/],
    ["wrong source", (snapshot) => { start(snapshot).payload.source = "clear"; }, /one SessionStart source=startup/],
    ["duplicate startup", (snapshot) => snapshot.events.push({ ...start(snapshot), id: "duplicate" }), /one SessionStart source=startup/],
    ["pending pack", (snapshot) => { snapshot.injections[1].state = "pending"; }, /session-start pack/],
    ["extra fallback pack", (snapshot) => snapshot.injections.push({ ...snapshot.injections[1], id: "fallback", channel: "codex:UserPromptSubmit" }), /session-start pack/],
    ["no repository memory", (snapshot) => { snapshot.items[1].memoryId = "other-repo-memory"; }, /includes a memory from the parent repository/],
    ["parent injection changed", (snapshot) => { snapshot.injections[0].deliveryCount += 1; }, /parent conversation.*unchanged/],
    ["parent injection added", (snapshot) => addMemoryInjection(snapshot, agent, "prompt", "codex:UserPromptSubmit", "parent"), /parent conversation.*unchanged/],
    ["parent ended", (snapshot) => { snapshot.sessions[0].status = "ended"; }, /parent stays active/],
    ["parent missing", (snapshot) => { snapshot.sessions.shift(); }, /parent stays active/],
  ]) {
    const invalid = structuredClone(after);
    mutate(invalid);
    const rejected = evaluateLifecycleCheck({
      agent, check: "clear", before, beforePrompt, after: invalid,
      parentNativeSessionId: "native-parent", childNativeSessionId: "native-child",
    });
    assert.equal(rejected.status, "fail", label);
    assert.match(rejected.reason, reason, label);
  }
  const reused = structuredClone(before);
  reused.sessions.push(childSession());
  const reusedRoot = evaluateLifecycleCheck({
    agent, check: "clear", before: reused, beforePrompt: reused, after,
    parentNativeSessionId: "native-parent", childNativeSessionId: "native-child",
  });
  assert.equal(reusedRoot.status, "fail");
  assert.match(reusedRoot.reason, /fresh root conversation/);
}

for (const agent of ["claude", "codex"]) {
  test(`${agent} resume evaluation enforces identity, epoch, and production hook policy`, () => {
    const before = lifecycleSnapshot(agent);
    const after = structuredClone(before);
    after.events.push({
      id: "event-resume-prompt",
      sessionId: "parent",
      nativeSessionId: "native-parent",
      kind: "prompt",
      payload: {},
    });
    if (agent === "claude") {
      after.events.push({
        id: "event-resume",
        sessionId: "parent",
        nativeSessionId: "native-parent",
        kind: "session_start",
        payload: { source: "resume" },
      });
    }

    const result = evaluateLifecycleCheck({
      agent,
      check: "resume",
      before,
      after,
      parentNativeSessionId: "native-parent",
    });
    assert.equal(result.status, "pass", result.reason);
    assert.ok(result.assertions.every((item) => item.pass));

    for (const [label, mutate, assertion] of [
      ["session id changed", (snapshot) => { snapshot.sessions[0].id = "other"; }, "resume keeps the same oboete session and conversation"],
      ["conversation changed", (snapshot) => { snapshot.sessions[0].conversationId = "other"; }, "resume keeps the same oboete session and conversation"],
      ["epoch advanced", (snapshot) => { snapshot.sessions[0].contextEpoch += 1; }, "resume leaves context_epoch unchanged"],
      ["session-start injection added", (snapshot) => addMemoryInjection(snapshot, agent, "session_start", `${agent}:SessionStart`, "parent"), "resume adds no session-start injection"],
      ["missing prompt", (snapshot) => { snapshot.events = snapshot.events.filter((event) => event.id !== "event-resume-prompt"); }, "resume records one prompt on the resumed session"],
      ...(agent === "codex" ? [
        ["unexpected resume SessionStart", (snapshot) => snapshot.events.push({
          id: "event-resume", sessionId: "parent", nativeSessionId: "native-parent",
          kind: "session_start", payload: { source: "resume" },
        }), "Codex production hooks omit SessionStart source=resume"],
      ] : [
        ["missing resume SessionStart", (snapshot) => { snapshot.events = snapshot.events.filter((event) => event.id !== "event-resume"); }, "Claude records one SessionStart source=resume"],
      ]),
    ]) {
      const invalid = structuredClone(after);
      mutate(invalid);
      const rejected = evaluateLifecycleCheck({
        agent, check: "resume", before, after: invalid, parentNativeSessionId: "native-parent",
      });
      assert.equal(rejected.status, "fail", label);
      assert.equal(rejected.assertions.find((item) => item.assertion === assertion)?.pass, false, label);
    }
  });

  test(`${agent} compact evaluation requires one epoch and preserves prior events`, () => {
    const { before, after, result, start } = compactLifecycleState(agent);
    assertRejectedCompactMutations(agent, before, after, start);

    if (agent === "codex") {
      const order = result.assertions.find((item) => /compact SessionStart precedes the next prompt/.test(item.assertion));
      assert.match(order.assertion, /2026-09-05T06-02-58-033Z/);
      for (const capturedAt of [5, 6, undefined, null]) {
        const invalid = structuredClone(after);
        start(invalid).capturedAt = capturedAt;
        const rejected = evaluateLifecycleCheck({
          agent, check: "compact", before, after: invalid, parentNativeSessionId: "native-parent",
        });
        assert.equal(rejected.status, "fail", `compact start captured at ${capturedAt}`);
        assert.match(rejected.reason, /compact SessionStart precedes the next prompt/);
      }
    }

    after.sessions[0].contextEpoch = 0;
    const failed = evaluateLifecycleCheck({
      agent,
      check: "compact",
      before,
      after,
      parentNativeSessionId: "native-parent",
    });
    assert.equal(failed.status, "fail");
    assert.match(failed.reason, /context_epoch/);

    after.sessions[0].contextEpoch = 1;
    after.injections = [];
    after.items = [];
    const missingPack = evaluateLifecycleCheck({
      agent,
      check: "compact",
      before,
      after,
      parentNativeSessionId: "native-parent",
    });
    assert.equal(missingPack.status, "fail");
    assert.match(missingPack.reason, /session-start pack/);
  });

  test(`${agent} fork evaluation requires a new root, repository memory, and an untouched parent`, () => {
    const before = lifecycleSnapshot(agent);
    addMemoryInjection(before, agent, "session_start", `${agent}:SessionStart`, "parent");
    const after = structuredClone(before);
    after.sessions.push(childSession());
    after.events.push({
      id: "event-fork-start",
      sessionId: "child",
      nativeSessionId: "native-child",
      kind: "session_start",
      payload: { source: agent === "claude" ? "fork" : "resume" },
    });
    addMemoryInjection(after, agent, "prompt", `${agent}:UserPromptSubmit`);
    after.memories.push({ ...after.memories[0], id: "new-summary" });
    after.items.at(-1).memoryId = "new-summary";

    const result = evaluateLifecycleCheck({
      agent,
      check: "fork",
      before,
      after,
      parentNativeSessionId: "native-parent",
      childNativeSessionId: "native-child",
    });
    assert.equal(result.status, "pass", result.reason);
    for (const [label, mutate, reason] of [
      ["parent conversation reused", (snapshot) => { snapshot.sessions[1].conversationId = "parent"; }, /separate root conversation/],
      ["parent session reused", (snapshot) => { snapshot.sessions[1].id = "parent"; }, /separate root conversation/],
      ["foreign repository", (snapshot) => { snapshot.sessions[1].repoId = "other-repo"; }, /parent repository identity/],
      ["parent ledger changed", (snapshot) => { snapshot.injections[0].deliveryCount += 1; }, /no injection to the parent/],
      ["parent ledger added", (snapshot) => addMemoryInjection(snapshot, agent, "prompt", `${agent}:UserPromptSubmit`, "parent"), /no injection to the parent/],
      ["no repository memory", (snapshot) => { snapshot.items.at(-1).memoryId = "other-repo-memory"; }, /includes a memory from the parent repository/],
      ...(agent === "claude" ? [
        ["missing fork SessionStart", (snapshot) => { snapshot.events = snapshot.events.filter((event) => event.id !== "event-fork-start"); }, /Claude fork records one SessionStart source=fork/],
        ["unexpected fork SessionStart pack", (snapshot) => addMemoryInjection(snapshot, agent, "session_start", "claude:SessionStart"), /Claude fork emits no SessionStart pack/],
      ] : []),
    ]) {
      const invalid = structuredClone(after);
      mutate(invalid);
      const rejected = evaluateLifecycleCheck({
        agent, check: "fork", before, after: invalid,
        parentNativeSessionId: "native-parent", childNativeSessionId: "native-child",
      });
      assert.equal(rejected.status, "fail", label);
      assert.match(rejected.reason, reason, label);
      assert.equal(rejected.assertions.find((item) => reason.test(item.assertion))?.pass, false, label);
    }
  });

  test(`${agent} clear evaluation enforces its measured injection source`, () => {
    const { before, beforePrompt, after, result } = clearLifecycleState(agent);
    assertRejectedClearMutations(agent, before, beforePrompt, after);
    if (agent === "codex") {
      assertCodexClearLifecycle(before, beforePrompt, after, result, agent);
    }
  });
}

test("Claude clear is keyed by its emitted source even when the native session id is reused", () => {
  const before = lifecycleSnapshot("claude");
  const after = structuredClone(before);
  after.events.push(
    {
      id: "event-clear-start",
      sessionId: "parent",
      nativeSessionId: "native-parent",
      kind: "session_start",
      payload: { source: "clear" },
    },
    {
      id: "event-clear-end",
      sessionId: "parent",
      nativeSessionId: "native-parent",
      kind: "turn_end",
      payload: {},
    },
  );
  after.injections.push({
    id: "injection-clear",
    sessionId: "parent",
    conversationId: "parent",
    kind: "session_start",
    channel: "claude:SessionStart",
    state: "emitted",
    contextEpoch: 0,
    packHash: "pack-clear",
    deliveryCount: 1,
  });
  after.items.push({
    id: 1,
    injectionId: "injection-clear",
    conversationId: "parent",
    contextEpoch: 0,
    memoryId: "memory-parent",
    decision: "included",
  });

  const result = evaluateLifecycleCheck({
    agent: "claude",
    check: "clear",
    before,
    after,
    parentNativeSessionId: "native-parent",
    childNativeSessionId: "native-parent",
  });
  assert.equal(result.status, "pass", result.reason);
});

test("Codex clear fails if oboete creates the new session before submitting the recall prompt", () => {
  const before = lifecycleSnapshot("codex");
  Object.assign(before.sessions[0], { status: "active", summaryState: null });
  const beforePrompt = structuredClone(before);
  beforePrompt.sessions.push(childSession());
  const after = structuredClone(beforePrompt);
  after.events.push({
    id: "event-clear-start", sessionId: "child", nativeSessionId: "native-child",
    kind: "session_start", payload: { source: "startup" }, capturedAt: 3,
  });
  after.events.push({
    id: "event-clear-prompt",
    sessionId: "child",
    nativeSessionId: "native-child",
    kind: "prompt",
    payload: {},
    capturedAt: 4,
  });
  addMemoryInjection(after, "codex", "session_start", "codex:SessionStart");

  const result = evaluateLifecycleCheck({
    agent: "codex",
    check: "clear",
    before,
    beforePrompt,
    after,
    parentNativeSessionId: "native-parent",
    childNativeSessionId: "native-child",
  });
  assert.equal(result.status, "fail");
  assert.match(result.reason, /before the recall prompt is submitted/);
  beforePrompt.sessions.pop();
  beforePrompt.events.push(after.events.find((event) => event.id === "event-clear-start"));
  const earlyStart = evaluateLifecycleCheck({
    agent: "codex", check: "clear", before, beforePrompt, after,
    parentNativeSessionId: "native-parent", childNativeSessionId: "native-child",
  });
  assert.equal(earlyStart.status, "fail");
  assert.match(earlyStart.reason, /before the recall prompt is submitted/);
});
