import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  assertAgentOutput,
  buildFactSeedingPrompt,
  claudeNativeSessionFromStart,
  createLifecycleReport,
  createReport,
  enumerateLifecycleAgents,
  enumeratePairs,
  evaluateLifecycleCheck,
  inspectLifecycle,
  nativeSessionFromPrompt,
  observerLeaseIsFree,
  parseArguments,
  requireAgentSuccess,
  resolveSourceHomes,
  retargetCodexTrust,
  runHarness,
  startLifecycleTui,
  waitForLifecycleState,
} from "./isolated-user.mjs";
import { PreconditionError, childEnv as probeChildEnv } from "./probe-lib/agents.mjs";
import { readyTui } from "./probe-lib/tmux.mjs";

test("parseArguments accepts the T054 flags", () => {
  const options = parseArguments([
    "--pairs",
    "claude:codex,grok:pi",
    "--no-credentials",
    "--daily",
    "--timeout",
    "45",
    "--run-dir",
    "/tmp/oboete-run",
  ]);

  assert.deepEqual(options.pairs, [
    { from: "claude", to: "codex" },
    { from: "grok", to: "pi" },
  ]);
  assert.equal(options.noCredentials, true);
  assert.equal(options.daily, true);
  assert.equal(options.timeoutMs, 45_000);
  assert.equal(options.runDir, "/tmp/oboete-run");
});

test("parseArguments defaults to all pairs and rejects invalid usage", () => {
  assert.equal(parseArguments([]).pairs.length, 12);
  assert.throws(() => parseArguments(["--timeout", "0"]), /positive integer/);
  assert.throws(() => parseArguments(["--wat"]), /Unknown option/);
});

test("parseArguments selects the Claude and Codex lifecycle checks", () => {
  const all = parseArguments(["--lifecycle"]);
  assert.equal(all.lifecycle, true);
  assert.deepEqual(all.agents, ["claude", "codex"]);

  const codex = parseArguments(["--lifecycle", "--agents", "codex"]);
  assert.deepEqual(codex.agents, ["codex"]);
  assert.deepEqual(enumerateLifecycleAgents("codex,claude"), ["codex", "claude"]);
  assert.throws(() => parseArguments(["--agents", "codex"]), /requires --lifecycle/);
  assert.throws(() => parseArguments(["--lifecycle", "--pairs", "claude:codex"]), /cannot be combined/);
  assert.throws(() => parseArguments(["--lifecycle", "--agents", "grok"]), /Unknown lifecycle agent/);
  assert.throws(() => parseArguments(["--lifecycle", "--agents", "codex,codex"]), /Duplicate lifecycle agent/);
});

test("resolveSourceHomes keeps every configured source inside the isolated account", (t) => {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "oboete-isolated-home-"));
  t.after(() => fs.rmSync(home, { recursive: true, force: true }));

  assert.equal(resolveSourceHomes({ CODEX_HOME: path.join(home, "codex") }, home).codex, path.join(home, "codex"));
  assert.throws(
    () => resolveSourceHomes({ GROK_HOME: path.resolve(home, "..", "maintainer-grok") }, home),
    /escapes the isolated account/,
  );
});

test("enumeratePairs returns all 12 ordered cross-agent pairs", () => {
  assert.deepEqual(enumeratePairs("all"), [
    { from: "claude", to: "codex" },
    { from: "claude", to: "grok" },
    { from: "claude", to: "pi" },
    { from: "codex", to: "claude" },
    { from: "codex", to: "grok" },
    { from: "codex", to: "pi" },
    { from: "grok", to: "claude" },
    { from: "grok", to: "codex" },
    { from: "grok", to: "pi" },
    { from: "pi", to: "claude" },
    { from: "pi", to: "codex" },
    { from: "pi", to: "grok" },
  ]);
  assert.throws(() => enumeratePairs("claude:claude"), /distinct agents/);
  assert.throws(() => enumeratePairs("claude:codex,claude:codex"), /duplicate pair/);
});

test("buildFactSeedingPrompt asks for one append tool call and preserves every fact", () => {
  const facts = [
    "fact-run-1: the build token is cedar",
    "fact-run-2: the release bird is heron",
    "fact-run-3: 配布色は琥珀",
  ];
  const prompt = buildFactSeedingPrompt(facts);

  assert.match(prompt, /exactly one tool call/i);
  assert.match(prompt, />> NOTES\.md/);
  for (const fact of facts) assert.ok(prompt.includes(fact), fact);
});

test("assertAgentOutput reports only facts absent from normalized output", () => {
  const facts = ["fact-one: cedar", "fact-two: heron", "fact-three: 琥珀"];

  assert.deepEqual(
    assertAgentOutput("fact-one: cedar\nfact-two:   heron\nfact-three: 琥珀", facts),
    { pass: true, missingFacts: [], degradedMarker: false },
  );
  assert.deepEqual(assertAgentOutput("fact-one: cedar; fact-three: 琥珀", facts), {
    pass: false,
    missingFacts: ["fact-two: heron"],
    degradedMarker: false,
  });
  assert.deepEqual(
    assertAgentOutput("fact-one: cedar; fact-two: heron; fact-three: 琥珀", facts, {
      requireDegraded: true,
    }),
    { pass: false, missingFacts: [], degradedMarker: false },
  );
  assert.deepEqual(
    assertAgentOutput(
      "fact-one: cedar; fact-two: heron; fact-three: 琥珀\n> degraded: No summarizer is configured, so these are rule-based notes.",
      facts,
      { requireDegraded: true },
    ),
    { pass: true, missingFacts: [], degradedMarker: true },
  );
  assert.equal(
    assertAgentOutput(
      "fact-one: cedar; fact-two: heron; fact-three: 琥珀\nNo > degraded: line with rule-based notes was present.",
      facts,
      { requireDegraded: true },
    ).pass,
    false,
  );
});

test("createReport has the required pair shape and redacts the run directory", () => {
  const report = createReport({
    runId: "2026-09-04T12-00-00-000Z",
    runDir: "/tmp/private-run",
    startedAt: "2026-09-04T12:00:00.000Z",
    finishedAt: "2026-09-04T12:00:03.000Z",
    noCredentials: false,
    timeoutMs: 120_000,
    requestedPairs: 2,
    results: [
      {
        from: "claude",
        to: "codex",
        elapsedMs: 3_000,
        status: "pass",
        missingFacts: [],
        degradedMarker: true,
        stdout: {
          seed: "/tmp/private-run/claude-to-codex/seed/stdout.txt",
          receive: "/tmp/private-run/claude-to-codex/receive/stdout.txt",
        },
        stderr: {
          seed: "/tmp/private-run/claude-to-codex/seed/stderr.txt",
          receive: "/tmp/private-run/claude-to-codex/receive/stderr.txt",
        },
      },
    ],
  });

  assert.equal(report.runDir, "<run>");
  assert.equal(report.summary, "1 of 2 requested pairs pass (partial run; SC-001 needs all 12)");
  assert.deepEqual(report.pairs[0].agents, { seed: "claude", receive: "codex" });
  assert.equal(report.pairs[0].elapsed_ms, 3_000);
  assert.equal(report.pairs[0].status, "pass");
  assert.deepEqual(report.pairs[0].missing_facts, []);
  assert.equal(report.pairs[0].degraded_marker, true);
  assert.equal(report.pairs[0].stdout.seed, "<run>/claude-to-codex/seed/stdout.txt");
  assert.ok(!JSON.stringify(report).includes("/tmp/private-run"));
});

test("createReport counts the pairs the run asked for and keeps 12 as the SC-001 target", () => {
  const result = (index) => ({
    from: "claude",
    to: "codex",
    elapsedMs: index,
    status: "pass",
    missingFacts: [],
    stdout: {},
    stderr: {},
  });
  const base = {
    runId: "run",
    runDir: "/tmp/run",
    startedAt: "2026-09-04T12:00:00.000Z",
    finishedAt: "2026-09-04T12:00:01.000Z",
    noCredentials: false,
    timeoutMs: 120_000,
  };

  const full = createReport({
    ...base,
    requestedPairs: 12,
    results: Array.from({ length: 12 }, (unused, index) => result(index)),
  });
  assert.equal(full.summary, "12 of 12 pairs pass");
  assert.equal(full.requested_pairs, 12);

  // Mid-run report of that same full run: honest about the denominator it is still working towards.
  const partial = createReport({
    ...base,
    requestedPairs: 12,
    results: [result(0), result(1)],
  });
  assert.equal(partial.summary, "2 of 12 pairs pass");

  const single = createReport({ ...base, requestedPairs: 1, results: [result(0)] });
  assert.equal(single.summary, "1 of 1 requested pairs pass (partial run; SC-001 needs all 12)");
  assert.equal(single.requested_pairs, 1);
  assert.equal(single.total_pairs, 12);
});

test("createLifecycleReport records every check, assertion, and blocked reason", () => {
  const report = createLifecycleReport({
    runId: "run",
    runDir: "/tmp/private-lifecycle-run",
    startedAt: "2026-09-05T12:00:00.000Z",
    finishedAt: "2026-09-05T12:00:01.000Z",
    noCredentials: true,
    timeoutMs: 120_000,
    agents: ["claude"],
    results: [
      {
        agent: "claude",
        check: "resume",
        elapsedMs: 1_000,
        status: "blocked",
        assertions: [],
        reason: "resume state unavailable",
        stdout: "/tmp/private-lifecycle-run/resume/stdout.txt",
        eventDelta: [{ kind: "prompt", native_session_id: "session" }],
      },
    ],
  });

  assert.equal(report.mode, "lifecycle");
  assert.equal(report.summary, "0 of 4 lifecycle checks pass.");
  assert.equal(report.lifecycle_checks[0].status, "blocked");
  assert.match(report.lifecycle_checks[0].asserts, /oboete conversation/);
  assert.equal(report.lifecycle_checks[0].reason, "resume state unavailable");
  assert.equal(report.lifecycle_checks[0].stdout, "<run>/resume/stdout.txt");
  assert.deepEqual(report.lifecycle_checks[0].event_delta, [{ kind: "prompt", native_session_id: "session" }]);
});

test("inspectLifecycle reads the identity and delivery evidence without writing", (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "oboete-lifecycle-db-"));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const oboeteHome = path.join(directory, ".oboete");
  fs.mkdirSync(oboeteHome);
  const file = path.join(oboeteHome, "memory.db");
  const db = new DatabaseSync(file);
  const migrations = new URL("../../src/db/migrations/", import.meta.url);
  for (const name of fs.readdirSync(migrations).filter((name) => name.endsWith(".sql")).sort()) {
    db.exec(fs.readFileSync(new URL(name, migrations), "utf8"));
  }
  db.exec(`
    PRAGMA journal_mode = WAL;
    PRAGMA foreign_keys = ON;
    INSERT INTO repos (id, identity_kind, normalized_identity) VALUES ('repo', 'remote', 'test');
    INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, context_epoch, status, started_at, summary_state)
      VALUES ('session', 'repo', 'codex', 'native', 'session', 1, 'active', 1, 'pending');
    INSERT INTO raw_events (id, repo_id, session_id, kind, payload_json, captured_at)
      VALUES ('event', 'repo', 'session', 'session_start', '{"source":"compact"}', 2);
    INSERT INTO injections (id, session_id, conversation_id, kind, channel, state, context_epoch, pack_hash, delivery_count, created_at)
      VALUES ('injection', 'session', 'session', 'session_start', 'codex:SessionStart', 'emitted', 1, 'hash', 1, 3);
    INSERT INTO injection_items (id, injection_id, conversation_id, context_epoch, memory_id, decision)
      VALUES (1, 'injection', 'session', 1, 'memory', 'included');
    INSERT INTO memories (id, repo_id, type, source_session_id, content_hash, sensitivity)
      VALUES ('memory', 'repo', 'session_summary', 'session', 'hash', 'local_only');
  `);
  t.after(() => db.close());

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

test("retargetCodexTrust points the copied trust rows at the copied hooks.json", () => {
  const source = "/home/oboete-dogfood/.codex/hooks.json";
  const copy = "/run/pair/seed/agent-home/hooks.json";
  const config = [
    "[mcp_servers.oboete]",
    'command = "node"',
    "",
    `[hooks.state."${source}:session_start:0:0"]`,
    'trusted_hash = "sha256:aaa"',
    "",
    `[hooks.state."${source}:pre_tool_use:1:0"]`,
    'trusted_hash = "sha256:bbb"',
    "",
    '[hooks.state."/home/oboete-dogfood/.codex/other-hooks.json:stop:0:0"]',
    'trusted_hash = "sha256:ccc"',
    "",
  ].join("\n");

  const retargeted = retargetCodexTrust(config, source, copy);

  assert.ok(retargeted.includes(`[hooks.state."${copy}:session_start:0:0"]`));
  assert.ok(retargeted.includes(`[hooks.state."${copy}:pre_tool_use:1:0"]`));
  // The hash covers the handler group alone, so the rows keep the value setup computed.
  assert.ok(retargeted.includes('trusted_hash = "sha256:aaa"'));
  assert.ok(!retargeted.includes(`"${source}:`));
  // A row naming a different hooks file is not this harness's to move.
  assert.ok(retargeted.includes('[hooks.state."/home/oboete-dogfood/.codex/other-hooks.json:stop:0:0"]'));
  assert.equal(retargeted.split("\n").length, config.split("\n").length);
});

test("retargetCodexTrust refuses a config that trusts no oboete hook", () => {
  assert.throws(
    () => retargetCodexTrust('[mcp_servers.oboete]\ncommand = "node"\n', "/h/.codex/hooks.json", "/run/hooks.json"),
    (error) => error.name === "PreconditionError" && /no Codex trust row names/.test(error.message),
  );
});

function lifecycleSnapshot(agent) {
  return {
    sessions: [
      {
        id: "parent",
        repoId: "repo",
        nativeSessionId: "native-parent",
        conversationId: "parent",
        contextEpoch: 0,
        lastCompactionKey: null,
        status: "ended",
        startedAt: 1,
      },
    ],
    events: [
      {
        id: "event-start",
        sessionId: "parent",
        nativeSessionId: "native-parent",
        kind: "session_start",
        payload: { source: "startup" },
        capturedAt: 1,
      },
      {
        id: "event-prompt",
        sessionId: "parent",
        nativeSessionId: "native-parent",
        kind: "prompt",
        payload: {},
        capturedAt: 2,
      },
    ],
    injections: [],
    items: [],
    memories: [{ id: "memory-parent", repoId: "repo", sourceSessionId: "parent", deletedAt: null }],
    agent,
  };
}

function childSession() {
  return {
    id: "child",
    repoId: "repo",
    nativeSessionId: "native-child",
    conversationId: "child",
    contextEpoch: 0,
    lastCompactionKey: null,
    status: "active",
    startedAt: 2,
  };
}

function addMemoryInjection(snapshot, agent, kind, channel, sessionId = "child", contextEpoch = 0) {
  const injectionId = `injection-${sessionId}-${contextEpoch}-${snapshot.injections.length}`;
  snapshot.injections.push({
    id: injectionId,
    sessionId,
    conversationId: sessionId,
    kind,
    channel,
    state: "emitted",
    contextEpoch,
    packHash: `pack-${sessionId}`,
    deliveryCount: 1,
  });
  snapshot.items.push({
    id: snapshot.items.length + 1,
    injectionId,
    conversationId: sessionId,
    contextEpoch,
    memoryId: "memory-parent",
    decision: "included",
    agent,
  });
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
    const before = lifecycleSnapshot(agent);
    if (agent === "codex") {
      Object.assign(before.sessions[0], { status: "active", summaryState: null });
      Object.assign(before.memories[0], { sourceSessionId: "seed", type: "session_summary" });
    }
    addMemoryInjection(before, agent, "session_start", `${agent}:SessionStart`, "parent");
    const beforePrompt = structuredClone(before);
    const after = structuredClone(before);
    after.sessions.push(childSession());
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
    if (agent === "codex") {
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

/**
 * One isolated account, standing in for the dogfood user: only the files the harness copies.
 * The Codex trust row is the one `oboete setup` writes, keyed by the account's own hooks.json.
 */
function isolatedAccount(t) {
  const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "oboete-account-")));
  t.after(() => fs.rmSync(home, { recursive: true, force: true }));
  const hooksPath = path.join(home, ".codex", "hooks.json");
  fs.mkdirSync(path.dirname(hooksPath), { recursive: true });
  fs.writeFileSync(
    hooksPath,
    JSON.stringify({
      hooks: { SessionStart: [{ hooks: [{ type: "command", command: "oboete hook" }], oboete: true }] },
    }),
  );
  fs.writeFileSync(
    path.join(home, ".codex", "config.toml"),
    `[mcp_servers.oboete]\ncommand = "node"\n\n[hooks.state."${hooksPath}:session_start:0:0"]\ntrusted_hash = "sha256:aaa"\n`,
  );
  fs.mkdirSync(path.join(home, ".claude"), { recursive: true });
  fs.writeFileSync(path.join(home, ".claude", "settings.json"), "{}\n");
  fs.mkdirSync(path.join(home, ".oboete"), { recursive: true });
  fs.writeFileSync(path.join(home, ".oboete", "config.toml"), "[observer]\npreset = \"nim\"\n");
  return { home, hooksPath };
}

test("Codex lifecycle TUI uses childEnv and the recorded fork/resume commands", (t) => {
  const account = isolatedAccount(t);
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "oboete-tui-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const previous = process.env.OBOETE_NIM_API_KEY;
  process.env.OBOETE_NIM_API_KEY = "must-not-reach-codex";
  t.after(() => {
    if (previous === undefined) delete process.env.OBOETE_NIM_API_KEY;
    else process.env.OBOETE_NIM_API_KEY = previous;
  });
  let launch;

  const open = (agent, action, childEnv = probeChildEnv) => startLifecycleTui({
    agent,
    action,
    directory: path.join(root, action),
    runtimeDir: path.join(root, "runtime"),
    repo: path.join(root, "repo"),
    parentNativeSessionId: "parent-native",
    oboeteHome: path.join(root, "oboete-home"),
    homes: resolveSourceHomes({}, account.home),
    dependencies: {
      childEnv,
      tuiSession: (options) => {
        launch = options;
        return { send() {}, capture: () => "", waitFor: async () => true, kill() {} };
      },
    },
  });

  const opened = open("codex", "fork");
  assert.deepEqual(opened.argv.slice(-2), ["fork", "parent-native"]);
  assert.ok(!opened.argv.includes("--dangerously-bypass-hook-trust"));
  assert.equal(launch.env.OBOETE_HOME, path.join(root, "oboete-home"));
  assert.equal(launch.env.TERM, "xterm-256color");
  assert.equal(launch.env.OBOETE_NIM_API_KEY, undefined);
  assert.deepEqual(Object.keys(launch.env).sort(), ["CODEX_HOME", "OBOETE_HOME", "PATH", "TERM"]);
  assert.ok(!launch.command.includes("must-not-reach-codex"));

  const compact = open("codex", "compact");
  assert.deepEqual(compact.argv.slice(-2), ["resume", "parent-native"]);
  assert.equal(launch.env.OBOETE_NIM_API_KEY, undefined);
  open("claude", "clear", () => ({ ...probeChildEnv(), GITHUB_TOKEN: "private", ARBITRARY_VARIABLE: "private" }));
  assert.deepEqual(Object.keys(launch.env).sort(), ["OBOETE_HOME", "PATH", "TERM"]);
  assert.ok(!launch.command.includes("private"));
});

test("seed failures distinguish executed CLI errors, timeouts, and provider outages", async (t) => {
  const account = isolatedAccount(t);
  const options = {
    lifecycle: true,
    agents: ["codex"],
    pairs: [],
    noCredentials: false,
    daily: false,
    timeoutMs: 1_000,
  };
  const run = async (stderr, exitCode = 2) => {
    const runDir = fs.mkdtempSync(path.join(os.tmpdir(), "oboete-lifecycle-exit-"));
    t.after(() => fs.rmSync(runDir, { recursive: true, force: true }));
    return runHarness(
      { ...options, runDir },
      {
        gitInit: (repo) => {
          fs.mkdirSync(repo, { recursive: true });
          return repo;
        },
        childEnv: probeChildEnv,
        runTimed: async (argv, options) => {
          fs.writeFileSync(options.stdoutPath, "");
          fs.writeFileSync(options.stderrPath, argv[0] === "git" ? "" : stderr);
          return {
            exitCode: argv[0] === "git" ? 0 : exitCode, signal: null, elapsedMs: 1,
            stdout: "", stderr: argv[0] === "git" ? "" : stderr,
          };
        },
        now: () => Date.parse("2026-09-05T09:00:00.000Z"),
        env: {},
        home: account.home,
        repoRoot: account.home,
        log: () => {},
      },
    );
  };

  const failed = await run("unknown option --fork");
  assert.deepEqual(new Set(failed.lifecycle_checks.map((item) => item.status)), new Set(["fail"]));
  for (const row of failed.lifecycle_checks) {
    assert.equal(row.stdout.seed, "<run>/lifecycle/codex/seed/stdout.txt");
    assert.equal(row.stderr.seed, "<run>/lifecycle/codex/seed/stderr.txt");
  }
  const blocked = await run("API Error: 529 Overloaded");
  assert.deepEqual(new Set(blocked.lifecycle_checks.map((item) => item.status)), new Set(["blocked"]));
  const timedOut = await run("", 124);
  assert.deepEqual(new Set(timedOut.lifecycle_checks.map((item) => item.status)), new Set(["blocked"]));
});

test("agent-exit classification is shared by every lifecycle action", () => {
  assert.throws(
    () => requireAgentSuccess({ exitCode: 124, stdout: "", stderr: "" }, "codex resume"),
    (error) => error.name === "PreconditionError" && /exited 124/.test(error.message),
  );
  assert.throws(
    () => requireAgentSuccess({ exitCode: 1, stdout: "", stderr: "API Error: 529 Overloaded" }, "codex resume"),
    (error) => error.name === "PreconditionError" && /Overloaded/.test(error.message),
  );
  assert.throws(
    () => requireAgentSuccess({ exitCode: 1, stdout: "", stderr: "API Error: invalid request" }, "codex resume"),
    (error) => error.name === "Error" && /invalid request/.test(error.message),
  );
});

for (const [label, options, errorClass] of [
  ["environment", {}, PreconditionError],
  ["contract", { contract: true }, Error],
]) test(`waitForLifecycleState classifies a ${label} timeout`, async () => {
  let clock = 0;
  await assert.rejects(waitForLifecycleState(
    "/tmp/memory.db", "codex", () => false, { timeoutMs: 500, ...options },
    {
      inspectLifecycle: () => lifecycleSnapshot("codex"),
      now: () => clock,
      sleep: async (ms) => { clock += ms; },
    },
    "required state",
  ), (error) => error.constructor === errorClass && /required state was not observed within 0.5s/.test(error.message));
  assert.equal(clock, 500);
});

test("waitForLifecycleState rethrows inspection defects instead of reporting a timeout", async () => {
  let sleeps = 0;
  await assert.rejects(
    waitForLifecycleState(
      "/tmp/memory.db",
      "codex",
      () => false,
      { timeoutMs: 250 },
      {
        inspectLifecycle: () => {
          throw new Error("broken lifecycle SQL");
        },
        sleep: async () => {
          sleeps += 1;
        },
      },
      "state",
    ),
    /broken lifecycle SQL/,
  );
  assert.equal(sleeps, 0);
});

test("readyTui waits for Codex's complete composer marker", async () => {
  const panes = ["old transcript ›", "old transcript ›", "› Ask Codex"];
  let index = 0;
  const result = await readyTui(
    "codex",
    { capture: () => panes[index] },
    { timeoutMs: 600 },
    { sleep: async () => { index += 1; } },
  );
  assert.equal(result, panes[2]);
});

test("Codex fork and clear select the session created by the post-command prompt", () => {
  const beforePrompt = lifecycleSnapshot("codex");
  const after = structuredClone(beforePrompt);
  after.sessions.push(childSession());
  after.events.push({
    id: "new-prompt",
    sessionId: "child",
    nativeSessionId: "native-child",
    kind: "prompt",
    payload: {},
  });

  assert.equal(nativeSessionFromPrompt(beforePrompt, after, "native-parent"), "native-child");
  after.events.push({ ...after.events.at(-1), id: "other-prompt", nativeSessionId: "native-other" });
  assert.throws(() => nativeSessionFromPrompt(beforePrompt, after, "native-parent"), /Ambiguous.*native-child, native-other/);
});

test("Claude names every ambiguous native session instead of returning a null child", () => {
  const before = lifecycleSnapshot("claude");
  const after = structuredClone(before);
  for (const id of ["first", "second"]) after.events.push({
    id, nativeSessionId: id, kind: "session_start", payload: { source: "fork" },
  });
  assert.throws(() => claudeNativeSessionFromStart(before, after, "fork"), /Ambiguous.*source=fork.*first, second/);
});

/** Every child the harness would spawn, recorded; the agents behave the way a passing pair does. */
function recordingDependencies(home) {
  const calls = [];
  let facts = [];
  return {
    calls,
    observerLeaseIsFree: () => true,
    gitInit: (repo) => {
      fs.mkdirSync(repo, { recursive: true });
      return repo;
    },
    // The double follows the real childEnv (probe-lib/agents.mjs): credentials only on request.
    childEnv: (extra = {}, { credentials = false } = {}) => ({
      PATH: "/usr/bin",
      HOME: home,
      ...(credentials
        ? {
            OBOETE_NIM_API_KEY: "nim-secret",
            OBOETE_CF_API_TOKEN: "cf-secret",
            OBOETE_CF_ACCOUNT_ID: "cf-account",
          }
        : {}),
      ...extra,
    }),
    runTimed: async (argv, options) => {
      calls.push({ argv, env: options.env, cwd: options.cwd, stdoutPath: options.stdoutPath });
      const prompt = argv.find((word) => word.includes("durable facts about this repository"));
      if (prompt) {
        facts = prompt.split("\n").slice(1, 4);
        fs.writeFileSync(path.join(options.cwd, "NOTES.md"), `${facts.join("\n")}\n`);
      }
      const stdout =
        argv[0] === "oboete" && argv[1] === "search"
          ? JSON.stringify({ memories: [{ text: facts.join(" ") }] })
          : JSON.stringify({ result: facts.join(" | ") });
      return { exitCode: 0, signal: null, elapsedMs: 1, stdout, stderr: "" };
    },
    sleep: async () => {},
    now: () => Date.parse("2026-09-05T09:00:00.000Z"),
    env: {},
    home,
    repoRoot: home,
    log: () => {},
  };
}

// Drive the harness through recorded hook/ledger effects, without starting an agent or tmux.
function lifecycleRun(t, agent, {
  startupSummary = true, followupFailure = false,
  compactStart = true, compactPrompt = true, compactTurnEndDelayMs = 600, timeoutMs = 5_000,
  summaryState = "done", summaryDelayMs = 600, noCredentials = false, daily = false,
  observerLeaseDelayMs = 200,
  earlyClearStartup = false, clearStart = true,
} = {}) {
  const { home } = isolatedAccount(t);
  const runDir = path.join(home, "run");
  const dependencies = recordingDependencies(home);
  const state = { sessions: [], events: [], injections: [], items: [], memories: [] };
  const operations = [];
  const timeline = [];
  let clock = Date.parse("2026-09-05T09:00:00.000Z");
  let action;
  let compactPending = false;
  let compactStartAt;
  let compactPromptAt;
  let compactTurnEndAt;
  let summaryReadyAt;
  let observerBusyUntil = 0;
  let clearPackAt;
  let recallEnters = 0;
  let active = "parent";
  let pane = "";
  let pending = "";
  let reports = 0;
  const event = (id, kind, payload = {}) => {
    const session = state.sessions.find((row) => row.id === id);
    // capture.ts reopens ended sessions on each new event; only SessionEnd makes them observable.
    session.status = kind === "session_end" ? "ended" : "active";
    session.summaryState = kind === "session_end" ? "pending" : null;
    if (kind === "session_end") {
      observerBusyUntil = clock + observerLeaseDelayMs;
      timeline.push({ action, type: "session-end", at: clock });
    }
    state.events.push({
      id: `event-${state.events.length}`, sessionId: id, nativeSessionId: `native-${id}`, kind, payload, capturedAt: clock,
    });
  };
  const open = (id, source) => {
    state.sessions.push({
      id, repoId: "repo", nativeSessionId: `native-${id}`, conversationId: id, contextEpoch: 0,
      status: "active", summaryState: null,
    });
    if (source) event(id, "session_start", { source });
  };
  const pack = (id, kind, channel) => {
    addMemoryInjection(state, agent, kind, channel, id, state.sessions.find((row) => row.id === id).contextEpoch);
    if (id === "clear") {
      const parent = state.sessions.find((row) => row.id === "parent");
      state.items.at(-1).memoryId = parent.summaryState === "pending" ? null
        : parent.summaryState === "done" ? "memory-observed-parent" : "memory-parent";
    }
  };
  const turn = (id, completed = true) => {
    event(id, "prompt");
    if (completed) event(id, "turn_end");
    operations.push(`turn:${id}`);
  };
  const runTimed = dependencies.runTimed;
  dependencies.runTimed = async (argv, options) => {
    const leg = path.basename(path.dirname(options.stdoutPath));
    if (argv[0] === "oboete" && argv[1] === "observe") {
      assert.ok(clock >= observerBusyUntil, "observe must wait for the existing worker's lease");
    }
    let result = await runTimed(argv, options);
    if (argv[0] === "oboete" && argv[1] === "observe" && leg === "clear") {
      operations.push("observe:parent");
      timeline.push({ action, type: "observe-parent", at: clock });
      const parent = state.sessions.find((row) => row.id === "parent");
      if (parent.status === "ended" && parent.summaryState === "pending") summaryReadyAt = clock + summaryDelayMs;
    } else if (argv[0] === "oboete" && argv[1] === "observe") {
      state.sessions.find((row) => row.id === "seed").summaryState = "done";
    }
    if (argv[0] === agent) {
      if (leg === "seed") {
        open("seed", "startup");
        turn("seed");
        event("seed", "session_end");
        state.memories.push({ id: "memory-parent", repoId: "repo", type: "session_summary", sourceSessionId: "seed", deletedAt: null });
      } else if (leg === "parent") {
        open("parent", "startup");
        pack("parent", "session_start", `${agent}:SessionStart`);
        if (!startupSummary) {
          state.injections.at(-1).state = "omitted";
          state.items.pop();
        }
        turn("parent");
        event("parent", "session_end");
      } else if (leg === "resume") {
        if (agent === "claude") event("parent", "session_start", { source: "resume" });
        turn("parent");
        event("parent", "session_end");
      } else if (leg === "compact") {
        if (!followupFailure) {
          state.sessions.find((row) => row.id === "parent").contextEpoch += 1;
          event("parent", "compaction_summary");
          event("parent", "session_start", { source: "compact" });
          pack("parent", "session_start", "claude:SessionStart");
        }
        event("parent", "session_end");
      } else if (leg === "compact-followup") {
        result = { ...result, exitCode: 2, stderr: "invalid request in follow-up" };
      } else if (leg === "fork") {
        open("fork", "fork");
        pack("fork", "prompt", "claude:UserPromptSubmit");
        turn("fork");
        event("fork", "session_end");
      }
    }
    fs.mkdirSync(path.dirname(options.stdoutPath), { recursive: true });
    fs.writeFileSync(options.stdoutPath, result.stdout);
    fs.writeFileSync(options.stderrPath, result.stderr);
    return result;
  };
  dependencies.inspectLifecycle = () => {
    timeline.push({ action, type: "snapshot", at: clock });
    return structuredClone(state);
  };
  dependencies.observerLeaseIsFree = () => clock >= observerBusyUntil;
  dependencies.now = () => clock;
  dependencies.sleep = async (ms) => {
    clock += ms;
    if (clock >= compactStartAt) {
      compactStartAt = undefined;
      event("parent", "session_start", { source: "compact" });
      pack("parent", "session_start", "codex:SessionStart");
      timeline.push({ action, type: "compact-start", at: clock });
    }
    if (clock >= compactPromptAt) {
      compactPromptAt = undefined;
      turn("parent", false);
    }
    if (clock >= compactTurnEndAt) {
      compactTurnEndAt = undefined;
      event("parent", "turn_end");
      timeline.push({ action, type: "compact-turn-end", at: clock });
    }
    if (clock >= summaryReadyAt) {
      summaryReadyAt = undefined;
      state.sessions.find((row) => row.id === "parent").summaryState = summaryState;
      if (summaryState === "done") state.memories.push({
        id: "memory-observed-parent", repoId: "repo", type: "session_summary", sourceSessionId: "parent", deletedAt: null,
      });
      timeline.push({ action, type: "parent-summary", summaryState, at: clock });
    }
    if (clearPackAt !== undefined && (clock >= clearPackAt || state.sessions.find((row) => row.id === "parent").summaryState !== "pending")) {
      clearPackAt = undefined;
      pack("clear", "session_start", "claude:SessionStart");
      pane = "Ask Claude";
    }
  };
  dependencies.log = (message) => {
    if (!message.includes("] asserts ")) return;
    if (reports > 0) {
      const report = JSON.parse(fs.readFileSync(path.join(runDir, "report.json"), "utf8"));
      assert.equal(report.lifecycle_checks.length, reports, "report must include every completed check before the next starts");
    }
    reports += 1;
  };
  dependencies.tuiSession = ({ name }) => {
    action = ["compact", "fork", "clear"].find((check) => name.includes(`-${check}-`));
    active = "parent";
    if (name.includes("-fork-")) { active = "fork"; open("fork"); }
    pane = agent === "codex" ? "› Ask Codex" : "Ask Claude";
    return {
      capture: () => pane,
      kill() {
        timeline.push({ action, type: "kill", at: clock });
        compactPending = false;
        compactStartAt = undefined;
        compactPromptAt = undefined;
        compactTurnEndAt = undefined;
        clearPackAt = undefined;
      },
    };
  };
  dependencies.tmux = (argv) => {
    timeline.push({ action, type: "key", key: argv.at(-1), at: clock });
    if (argv.includes("-l")) {
      pending = argv.at(-1);
      if (earlyClearStartup && action === "clear" && pending.startsWith("Recall the repository")) {
        open("clear", "startup");
        pack("clear", "session_start", "codex:SessionStart");
      }
      pane = agent === "codex" ? `› ${pending}` : `╭─────────────────╮\n│ > ${pending}\n╰─────────────────╯`;
      recallEnters = 0;
    }
    if (argv.at(-1) === "C-m") {
      // Claude may drop Enter while finishing startup; its framed > composer must trigger retry.
      if (agent === "claude" && pending.startsWith("Recall the repository") && ++recallEnters === 1) return { status: 0 };
      operations.push(pending);
      if (pending === "/compact") {
        state.sessions.find((row) => row.id === "parent").contextEpoch += 1;
        event("parent", "compaction_summary");
        event("parent", "turn_end"); // /compact completes its own task before the follow-up prompt.
        timeline.push({ action, type: "post-compact", at: clock });
        compactPending = true;
      } else if (pending === "/new") {
        active = "clear";
      } else if (pending === "/clear") {
        event("parent", "session_end");
        active = "clear";
        open("clear", clearStart ? "clear" : undefined);
        clearPackAt = clock + 1_000;
      } else if (pending === "/quit") {
        event(state.sessions.some((row) => row.id === active) ? active : "parent", "session_end");
      } else if (compactPending) {
        compactPending = false;
        // Separate captures expose the lazy hook before UserPromptSubmit reaches the database.
        if (compactStart) compactStartAt = clock + 200;
        if (compactPrompt) compactPromptAt = clock + 400;
        compactTurnEndAt = clock + 400 + compactTurnEndDelayMs;
      } else {
        if (!state.sessions.some((row) => row.id === active)) {
          open(active, "startup");
          pack(active, "session_start", `${agent}:SessionStart`);
          clock += 200;
        }
        if (agent === "codex") {
          const epoch = state.sessions.find((row) => row.id === active).contextEpoch;
          if (!state.injections.some((row) => row.sessionId === active && row.contextEpoch === epoch)) {
            pack(active, "session_start", "codex:UserPromptSubmit");
          }
        }
        turn(active);
      }
      pane = agent === "codex" ? "› Ask Codex" : clearPackAt !== undefined
        ? "Running SessionStart hooks..." : "● DONE\n╭─────────────────╮\n│ > \n╰─────────────────╯\nshift+tab";
    }
    return { status: 0 };
  };
  return {
    dependencies, operations, timeline, runDir,
    run: () => runHarness({ lifecycle: true, agents: [agent], timeoutMs, runDir, noCredentials, daily }, dependencies),
  };
}

for (const agent of ["claude", "codex"]) {
  test(`${agent} lifecycle seeds S1, drives S2, and persists each completed check`, async (t) => {
    const fixture = lifecycleRun(t, agent);
    const report = await fixture.run();
    assert.equal(report.summary, "4 of 4 lifecycle checks pass.", JSON.stringify(report.lifecycle_checks));
    const calls = fixture.dependencies.calls.filter((call) => call.argv[0] === agent);
    assert.match(calls[0].argv.join(" "), /durable facts/);
    const parent = calls[1].argv;
    assert.ok(parent.includes("Reply with exactly DONE and do not use tools."));
    assert.equal(parent.some((word) => /cedar|heron|琥珀|native-seed/.test(word)), false);
    for (const call of calls.slice(2)) assert.ok(call.argv.includes("native-parent"));
    assert.ok(report.lifecycle_checks.every((row) => row.assertions.some((item) => /S1's repository summary/.test(item.assertion) && item.pass)));
    const observers = fixture.dependencies.calls.filter((call) => call.argv[0] === "oboete" && call.argv[1] === "observe");
    assert.equal(observers.length, agent === "claude" ? 2 : 1);
    for (const observer of observers) {
      assert.deepEqual(observer.env, observers[0].env);
      assert.equal(observer.env.OBOETE_NIM_API_KEY, "nim-secret");
    }
    const clearEvents = fixture.timeline.filter((event) => event.action === "clear");
    const observed = clearEvents.find((event) => event.type === "observe-parent");
    const summarized = clearEvents.find((event) => event.type === "parent-summary");
    const recalled = clearEvents.find((event) => event.type === "key" && event.key.startsWith("Recall the repository"));
    const clearCheck = report.lifecycle_checks.find((row) => row.check === "clear");
    assert.deepEqual(clearCheck.assertions.find((item) => /includes a memory from the parent repository/.test(item.assertion)).actual,
      [agent === "claude" ? "memory-observed-parent" : "memory-parent"]);
    if (agent === "claude") {
      const ended = clearEvents.find((event) => event.type === "session-end");
      assert.ok(ended.at < observed.at && observed.at < summarized.at && summarized.at < recalled.at,
        "recall waits for SessionEnd, the observer lease, and the parent's summary");
      const submitKeys = clearEvents.filter((event) => event.type === "key" && event.at > recalled.at);
      assert.deepEqual(submitKeys.slice(0, 2).map((event) => event.key), ["C-m", "C-m"]);
    }
    if (agent === "codex") {
      assert.equal(observed, undefined, "no observe between /new and recall");
      assert.equal(summarized, undefined, "an active parent cannot be summarized");
      const compact = fixture.operations.indexOf("/compact");
      assert.equal(fixture.operations[compact + 1], "Reply with exactly DONE and do not use tools.");
      assert.equal(fixture.operations[compact + 2], "turn:parent");
      assert.equal(fixture.operations[compact + 3], "/quit");
      const compactEvents = fixture.timeline.filter((event) => event.action === "compact");
      const postCompactIndex = compactEvents.findIndex((event) => event.type === "post-compact");
      const start = compactEvents.find((event) => event.type === "compact-start");
      assert.ok(start, "the next turn triggers the lazy SessionStart(compact)");
      const nextKey = compactEvents.slice(postCompactIndex + 1).find((event) => event.type === "key");
      assert.ok(nextKey.at - compactEvents[postCompactIndex].at >= 1_000, "settle after PostCompact before the next turn");
      assert.ok(nextKey.at < start.at, "send the next turn before waiting for its SessionStart");
      const compactCheck = report.lifecycle_checks.find((row) => row.check === "compact");
      const compactStartEvent = compactCheck.event_delta.find((event) => event.kind === "session_start");
      const compactPrompt = compactCheck.event_delta.find((event) => event.kind === "prompt");
      assert.ok(compactStartEvent.captured_at < compactPrompt.captured_at);
      assert.ok(compactEvents.some((event) => event.type === "snapshot" &&
        event.at >= compactStartEvent.captured_at && event.at < compactPrompt.captured_at),
      "keep waiting when SessionStart has arrived but the prompt has not");
      assert.ok(compactCheck.event_delta.some((event) => event.kind === "turn_end" && event.captured_at > compactPrompt.captured_at));
      const clear = fixture.operations.indexOf("/new");
      assert.equal(fixture.operations[clear - 1], "turn:parent");
      assert.match(fixture.operations[clear + 1], /Recall the repository/);
      assert.equal(fixture.operations[clear + 2], "turn:clear");
      const newCommand = clearEvents.findLast((event) => event.type === "key" && event.key === "C-m" && event.at < recalled.at);
      assert.ok(recalled.at - newCommand.at >= 1_000, "settle the composer after /new before recalling");
      const quitIndex = clearEvents.findIndex((event) => event.key === "/quit");
      const lastSnapshot = clearEvents.slice(0, quitIndex).findLast((event) => event.type === "snapshot");
      const firstExitKey = clearEvents.find((event) => event.type === "key" && event.at > lastSnapshot.at);
      assert.ok(firstExitKey.at - lastSnapshot.at >= 1_000, "settle after the final evidence snapshot before any exit key");
    }
  });
}

for (const [label, options, status, reason] of [
  ["its lazy SessionStart", { compactStart: false }, "fail", /^Codex SessionStart source=compact was not observed/],
  ["the next turn_end", { compactTurnEndDelayMs: Infinity }, "blocked", /^Codex post-compact prompt and following turn_end was not observed/],
  ["the next prompt", { compactPrompt: false }, "blocked", /^Codex post-compact prompt and following turn_end was not observed/],
]) test(`Codex compact times out without ${label} after sending the next turn and kills without quit`, async (t) => {
  const fixture = lifecycleRun(t, "codex", { ...options, timeoutMs: 1_500 });
  const report = await fixture.run();
  const compact = report.lifecycle_checks.find((row) => row.check === "compact");
  assert.equal(compact.status, status);
  assert.match(compact.reason, reason);
  assert.match(compact.reason, /within 1.5s/);
  const events = fixture.timeline.filter((event) => event.action === "compact");
  const index = events.findLastIndex((event) => event.key === "C-m");
  const killed = events.slice(index + 1).find((event) => event.type === "kill");
  assert.ok(killed.at - events[index].at >= 1_500, "wait for the missing evidence until the deadline");
  assert.ok(events.slice(index + 1).every((event) => event.type !== "key"));
  assert.equal(events.filter((event) => event.key === "Reply with exactly DONE and do not use tools.").length, 2);
});

test("Claude clear accepts no_content and keeps no-credentials on its observer", async (t) => {
  const fixture = lifecycleRun(t, "claude", { summaryState: "no_content", noCredentials: true });
  const report = await fixture.run();
  const clear = report.lifecycle_checks.find((row) => row.check === "clear");
  assert.equal(clear.status, "pass", clear.reason);
  const observers = fixture.dependencies.calls.filter((call) => call.argv[0] === "oboete" && call.argv[1] === "observe");
  assert.equal(observers.length, 2);
  assert.deepEqual(observers[1].env, observers[0].env);
  for (const key of ["OBOETE_NIM_API_KEY", "OBOETE_CF_API_TOKEN", "OBOETE_CF_ACCOUNT_ID"]) {
    assert.equal(observers[1].env[key], undefined);
  }
  const summary = fixture.timeline.find((event) => event.type === "parent-summary");
  assert.equal(summary.summaryState, "no_content");
  const recall = fixture.timeline.find((event) => event.action === "clear" && event.key?.startsWith("Recall the repository"));
  assert.ok(summary.at < recall.at);
});

test("Claude clear blocks a pending parent summary before recall", async (t) => {
  const fixture = lifecycleRun(t, "claude", { summaryDelayMs: Infinity, timeoutMs: 1_500 });
  const report = await fixture.run();
  const clear = report.lifecycle_checks.find((row) => row.check === "clear");
  assert.equal(clear.status, "blocked");
  assert.match(clear.reason, /parent summary done or no_content was not observed within 1.5s/);
  assert.equal(fixture.timeline.some((event) => event.action === "clear" && event.key?.startsWith("Recall the repository")), false);
});

test("Claude clear fails when SessionStart source=clear is missing", async (t) => {
  const fixture = lifecycleRun(t, "claude", { clearStart: false, timeoutMs: 1_500 });
  const report = await fixture.run();
  const clear = report.lifecycle_checks.find((row) => row.check === "clear");
  assert.equal(clear.status, "fail");
  assert.match(clear.reason, /^Claude SessionStart source=clear was not observed within 1.5s/);
  assert.equal(fixture.timeline.some((event) => event.action === "clear" && event.key?.startsWith("Recall the repository")), false);
});

for (const [status, options] of [
  ["pass", {}], ["blocked", { summaryDelayMs: Infinity }], ["fail", { clearStart: false }],
]) test(`Claude clear links observe output in report.json on ${status}`, async (t) => {
  const fixture = lifecycleRun(t, "claude", { ...options, timeoutMs: 1_500 });
  await fixture.run();
  const report = JSON.parse(fs.readFileSync(path.join(fixture.runDir, "report.json"), "utf8"));
  const clear = report.lifecycle_checks.find((row) => row.check === "clear");
  assert.equal(clear.status, status, clear.reason);
  for (const stream of ["stdout", "stderr"]) {
    const relative = `lifecycle/claude/clear/observe.${stream}.txt`;
    assert.equal(clear[stream]?.["clear/observe"], `<run>/${relative}`);
    assert.ok(fs.existsSync(path.join(fixture.runDir, relative)));
  }
});

test("daily lifecycle evidence bounds and scrubs reasons while report.json keeps the full diagnostic", async (t) => {
  for (const [firstLine, expected] of [
    ["startup diagnostic " + "x".repeat(280), ("startup diagnostic " + "x".repeat(280)).slice(0, 240)],
    ['startup: OBOETE_NIM_API_KEY="demo key" more text', "startup: [redacted]"],
    ["startup: OBOETE_CF_API_TOKEN=demo-token more text", "startup: [redacted]"],
    ["startup: OBOETE_CF_ACCOUNT_ID demo-account", "startup: [redacted]"],
    ["HTTP 401: Bearer demo.jwt-token+/= denied", "HTTP 401: Bearer [redacted] denied"],
    ["pane C:\\tmp\\a | b", "pane C:\\\\tmp\\\\a \\| b"],
  ]) {
    const fixture = lifecycleRun(t, "codex", { daily: true });
    const reason = `${firstLine}\nprivate pane at ${fixture.runDir}\nsecond private pane line`;
    fixture.dependencies.tuiSession = () => ({
      capture() { throw new PreconditionError(reason); },
      kill() {},
    });
    await fixture.run();
    const report = JSON.parse(fs.readFileSync(path.join(fixture.runDir, "report.json"), "utf8"));
    assert.equal(report.lifecycle_checks.find((row) => row.check === "clear").reason,
      `${firstLine}\nprivate pane at <run>\nsecond private pane line`);
    const markdown = fs.readFileSync(path.join(fixture.dependencies.home, "docs/evidence/m1-dogfood.md"), "utf8");
    const rows = markdown.split("\n").filter((line) => line.startsWith("| codex |") && line.includes("| blocked |"));
    assert.equal(rows.length, 3);
    for (const row of rows) assert.equal(row.split(" | ").at(-1), `${expected} |`);
    assert.doesNotMatch(markdown, /private pane|OBOETE_.*(?:API_KEY|API_TOKEN|ACCOUNT_ID)|demo[- .]/);
  }
});

test("an omitted S2 startup pack fails the seed precondition and links all completed seed legs", async (t) => {
  const fixture = lifecycleRun(t, "codex", { startupSummary: false });
  const report = await fixture.run();
  for (const row of report.lifecycle_checks) {
    assert.equal(row.status, "fail");
    assert.match(row.reason, /S2.*startup pack/);
    for (const stream of ["stdout", "stderr"]) assert.deepEqual(Object.keys(row[stream]), ["seed", "observe", "search", "parent"]);
  }
  assert.equal(fixture.operations.length, 2, "no lifecycle action may run after a failed S2 precondition");
});

test("Codex /new leaves the parent active and completes the child's lazy startup turn", async (t) => {
  const report = await lifecycleRun(t, "codex").run();
  const clear = report.lifecycle_checks.find((row) => row.check === "clear");
  assert.equal(clear.status, "pass", clear.reason);
  assert.equal(clear.evidence.parent_session_end_count, 0);
  assert.equal(clear.assertions.find((item) => /parent stays active/.test(item.assertion)).actual, "active");
  const start = clear.event_delta.find((event) => event.kind === "session_start");
  const prompt = clear.event_delta.find((event) => event.kind === "prompt");
  assert.equal(start.payload.source, "startup");
  assert.ok(start.captured_at < prompt.captured_at);
  assert.ok(clear.event_delta.some((event) => event.native_session_id === "native-clear" && event.kind === "turn_end"));
});

test("Codex /new recalls without waiting for a parent summary in no-credentials mode", async (t) => {
  const fixture = lifecycleRun(t, "codex", { summaryDelayMs: Infinity, noCredentials: true, timeoutMs: 1_500 });
  const report = await fixture.run();
  const clear = report.lifecycle_checks.find((row) => row.check === "clear");
  assert.equal(clear.status, "pass", clear.reason);
  assert.equal(fixture.timeline.some((event) => event.type === "parent-summary"), false);
  assert.ok(fixture.timeline.some((event) => event.action === "clear" && event.key?.startsWith("Recall the repository")));
  const observers = fixture.dependencies.calls.filter((call) => call.argv[1] === "observe");
  assert.equal(observers.length, 1);
  for (const key of ["OBOETE_NIM_API_KEY", "OBOETE_CF_API_TOKEN", "OBOETE_CF_ACCOUNT_ID"]) {
    assert.equal(observers[0].env[key], undefined);
  }
});

test("Codex clear rejects a startup that arrives while the recall prompt is still being typed", async (t) => {
  const report = await lifecycleRun(t, "codex", { earlyClearStartup: true }).run();
  const clear = report.lifecycle_checks.find((row) => row.check === "clear");
  assert.equal(clear.status, "fail");
  assert.match(clear.reason, /before the recall prompt is submitted/);
});

test("an occupied observer lease blocks instead of starting a competing observe", async (t) => {
  const fixture = lifecycleRun(t, "codex", { observerLeaseDelayMs: Infinity, timeoutMs: 1_500 });
  const report = await fixture.run();
  assert.ok(report.lifecycle_checks.every((check) => check.status === "blocked" && /observer lease/.test(check.reason)));
  assert.equal(fixture.dependencies.calls.some((call) => call.argv[1] === "observe"), false);
});

test("a Claude compaction follow-up failure retains both legs' output paths", async (t) => {
  const report = await lifecycleRun(t, "claude", { followupFailure: true }).run();
  const compact = report.lifecycle_checks.find((row) => row.check === "compact");
  assert.equal(compact.status, "fail");
  assert.match(compact.reason, /invalid request in follow-up/);
  for (const stream of ["stdout", "stderr"]) {
    assert.equal(compact[stream].compact, `<run>/lifecycle/claude/compact/${stream}.txt`);
    assert.equal(compact[stream]["compact-followup"], `<run>/lifecycle/claude/compact-followup/${stream}.txt`);
  }
});

test("runHarness keeps oboete credentials off every agent and trusts the copied Codex hooks", async (t) => {
  const account = isolatedAccount(t);
  const runDir = fs.mkdtempSync(path.join(os.tmpdir(), "oboete-run-"));
  t.after(() => fs.rmSync(runDir, { recursive: true, force: true }));
  const dependencies = recordingDependencies(account.home);

  const report = await runHarness(
    {
      pairs: [{ from: "codex", to: "claude" }],
      noCredentials: false,
      daily: false,
      timeoutMs: 120_000,
      runDir,
    },
    dependencies,
  );

  assert.equal(report.pairs[0].status, "pass", JSON.stringify(report.pairs[0].reason));
  assert.equal(report.summary, "1 of 1 requested pairs pass (partial run; SC-001 needs all 12)");

  // FR-016: an agent CLI never sees oboete's provider credentials, credentials or not in this run.
  for (const agent of ["codex", "claude"]) {
    const call = dependencies.calls.find((entry) => entry.argv[0] === agent);
    assert.ok(call, agent);
    assert.equal(call.env.OBOETE_NIM_API_KEY, undefined, agent);
    assert.equal(call.env.OBOETE_CF_API_TOKEN, undefined, agent);
    assert.equal(call.env.OBOETE_CF_ACCOUNT_ID, undefined, agent);
    assert.ok(call.env.OBOETE_HOME, agent);
  }
  // The oboete legs are the only ones that need them, so this run still exercises the provider.
  const observe = dependencies.calls.find((entry) => entry.argv[0] === "oboete" && entry.argv[1] === "observe");
  assert.equal(observe.env.OBOETE_NIM_API_KEY, "nim-secret");
  assert.equal(observe.env.OBOETE_CF_ACCOUNT_ID, "cf-account");

  // The copied Codex home carries its own trust, so the run gates the trust rule instead of it.
  const codex = dependencies.calls.find((entry) => entry.argv[0] === "codex");
  assert.ok(!codex.argv.includes("--dangerously-bypass-hook-trust"));
  const agentHome = path.join(runDir, "codex-to-claude", "seed", "agent-home");
  const copied = fs.readFileSync(path.join(agentHome, "config.toml"), "utf8");
  assert.ok(copied.includes(`[hooks.state."${path.join(agentHome, "hooks.json")}:session_start:0:0"]`));
  assert.ok(!copied.includes(account.hooksPath));
  assert.equal(codex.env.CODEX_HOME, agentHome);
});

test("every harness test file is run by npm test", () => {
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
  const files = fs
    .readdirSync(path.join(root, "scripts", "e2e"), { recursive: true })
    .filter((entry) => entry.endsWith(".test.mjs"))
    .map((entry) => `scripts/e2e/${entry.split(path.sep).join("/")}`);
  assert.ok(
    files.some((file) => file.split("/").length > 3),
    "expected a harness test below scripts/e2e/, otherwise the ** in the globs is untested",
  );

  const { scripts } = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8"));
  const globs = [...scripts.test.matchAll(/'([^']+)'/g)].map((match) => match[1]);
  for (const file of files) {
    assert.ok(
      globs.some((glob) => path.matchesGlob(file, glob)),
      `npm test does not run ${file} (globs: ${globs.join(" ")})`,
    );
  }
});
