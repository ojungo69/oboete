import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { runHarness } from "./isolated-user.mjs";
import {
  claudeNativeSessionFromStart,
  nativeSessionFromPrompt,
  waitForLifecycleState,
} from "./probe-lib/isolated-lifecycle.mjs";
import { PreconditionError, childEnv as probeChildEnv } from "./probe-lib/process.mjs";
import { readyTui } from "./probe-lib/tmux.mjs";
import {
  addMemoryInjection,
  childSession,
  isolatedAccount,
  lifecycleSnapshot,
  recordingDependencies,
} from "./isolated-user.test-support.mjs";

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

function newLifecycleRunState(t) {
  const { home } = isolatedAccount(t);
  const runDir = path.join(home, "run");
  const dependencies = recordingDependencies(home);
  const state = { sessions: [], events: [], injections: [], items: [], memories: [] };
  const operations = [];
  const timeline = [];
  return { runDir, dependencies, state, operations, timeline };
}

// Drive the harness through recorded hook/ledger effects, without starting an agent or tmux.
function lifecycleRun(configuration) {
  const { t, agent, options = {} } = configuration;
  const {
    startupSummary = true, followupFailure = false,
    compactStart = true, compactPrompt = true, compactTurnEndDelayMs = 600, timeoutMs = 5_000,
    summaryState = "done", summaryDelayMs = 600, noCredentials = false, daily = false,
    observerLeaseDelayMs = 200,
    earlyClearStartup = false, clearStart = true,
  } = options;
  const { runDir, dependencies, state, operations, timeline } = newLifecycleRunState(t);
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

  function recordObserveEffects(argv, leg) {
    if (argv[0] === "oboete" && argv[1] === "observe" && leg === "clear") {
      operations.push("observe:parent");
      timeline.push({ action, type: "observe-parent", at: clock });
      const parent = state.sessions.find((row) => row.id === "parent");
      if (parent.status === "ended" && parent.summaryState === "pending") summaryReadyAt = clock + summaryDelayMs;
    } else if (argv[0] === "oboete" && argv[1] === "observe") {
      state.sessions.find((row) => row.id === "seed").summaryState = "done";
    }
  }

  function recordAgentEffects(leg, result) {
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
    return result;
  }

  dependencies.runTimed = async (argv, options) => {
    const leg = path.basename(path.dirname(options.stdoutPath));
    if (argv[0] === "oboete" && argv[1] === "observe") {
      assert.ok(clock >= observerBusyUntil, "observe must wait for the existing worker's lease");
    }
    let result = await runTimed(argv, options);
    recordObserveEffects(argv, leg);
    if (argv[0] === agent) {
      result = recordAgentEffects(leg, result);
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

  function loadPendingCommand(argv) {
    pending = argv.at(-1);
    if (earlyClearStartup && action === "clear" && pending.startsWith("Recall the repository")) {
      open("clear", "startup");
      pack("clear", "session_start", "codex:SessionStart");
    }
    pane = agent === "codex" ? `› ${pending}` : `╭─────────────────╮\n│ > ${pending}\n╰─────────────────╯`;
    recallEnters = 0;
  }

  function compactPendingCommand() {
    state.sessions.find((row) => row.id === "parent").contextEpoch += 1;
    event("parent", "compaction_summary");
    event("parent", "turn_end"); // /compact completes its own task before the follow-up prompt.
    timeline.push({ action, type: "post-compact", at: clock });
    compactPending = true;
  }

  function clearPendingCommand() {
    event("parent", "session_end");
    active = "clear";
    open("clear", clearStart ? "clear" : undefined);
    clearPackAt = clock + 1_000;
  }

  function quitPendingCommand() {
    event(state.sessions.some((row) => row.id === active) ? active : "parent", "session_end");
  }

  function followCompactPendingCommand() {
    compactPending = false;
    // Separate captures expose the lazy hook before UserPromptSubmit reaches the database.
    if (compactStart) compactStartAt = clock + 200;
    if (compactPrompt) compactPromptAt = clock + 400;
    compactTurnEndAt = clock + 400 + compactTurnEndDelayMs;
  }

  function regularPendingCommand() {
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

  function submitPendingCommand() {
    operations.push(pending);
    if (pending === "/compact") {
      compactPendingCommand();
    } else if (pending === "/new") {
      active = "clear";
    } else if (pending === "/clear") {
      clearPendingCommand();
    } else if (pending === "/quit") {
      quitPendingCommand();
    } else if (compactPending) {
      followCompactPendingCommand();
    } else {
      regularPendingCommand();
    }
    pane = agent === "codex" ? "› Ask Codex" : clearPackAt !== undefined
      ? "Running SessionStart hooks..." : "● DONE\n╭─────────────────╮\n│ > \n╰─────────────────╯\nshift+tab";
  }

  dependencies.tmux = (argv) => {
    timeline.push({ action, type: "key", key: argv.at(-1), at: clock });
    if (argv.includes("-l")) {
      loadPendingCommand(argv);
    }
    if (argv.at(-1) === "C-m") {
      // Claude may drop Enter while finishing startup; its framed > composer must trigger retry.
      if (agent === "claude" && pending.startsWith("Recall the repository") && ++recallEnters === 1) return { status: 0 };
      submitPendingCommand();
    }
    return { status: 0 };
  };
  function lifecycleResult() {
    return {
      dependencies, operations, timeline, runDir,
      run: () => runHarness({ lifecycle: true, agents: [agent], timeoutMs, runDir, noCredentials, daily }, dependencies),
    };
  }
  return lifecycleResult();
}

for (const agent of ["claude", "codex"]) {
  test(`${agent} lifecycle seeds S1, drives S2, and persists each completed check`, async (t) => {
    const fixture = lifecycleRun({ t, agent });
    const report = await fixture.run();
    const evidence = assertLifecycleRunBasics(agent, fixture, report);
    if (agent === "claude") {
      assertClaudeLifecycleTimeline(evidence);
    }
    if (agent === "codex") {
      assertCodexLifecycleTimeline(fixture, report, evidence);
    }
  });
}

for (const [label, options, status, reason] of [
  ["its lazy SessionStart", { compactStart: false }, "fail", /^Codex SessionStart source=compact was not observed/],
  ["the next turn_end", { compactTurnEndDelayMs: Infinity }, "blocked", /^Codex post-compact prompt and following turn_end was not observed/],
  ["the next prompt", { compactPrompt: false }, "blocked", /^Codex post-compact prompt and following turn_end was not observed/],
]) test(`Codex compact times out without ${label} after sending the next turn and kills without quit`, async (t) => {
  const fixture = lifecycleRun({ t, agent: "codex", options: { ...options, timeoutMs: 1_500 } });
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
  const fixture = lifecycleRun({ t, agent: "claude", options: { summaryState: "no_content", noCredentials: true } });
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
  const fixture = lifecycleRun({ t, agent: "claude", options: { summaryDelayMs: Infinity, timeoutMs: 1_500 } });
  const report = await fixture.run();
  const clear = report.lifecycle_checks.find((row) => row.check === "clear");
  assert.equal(clear.status, "blocked");
  assert.match(clear.reason, /parent summary done or no_content was not observed within 1.5s/);
  assert.equal(fixture.timeline.some((event) => event.action === "clear" && event.key?.startsWith("Recall the repository")), false);
});

test("Claude clear fails when SessionStart source=clear is missing", async (t) => {
  const fixture = lifecycleRun({ t, agent: "claude", options: { clearStart: false, timeoutMs: 1_500 } });
  const report = await fixture.run();
  const clear = report.lifecycle_checks.find((row) => row.check === "clear");
  assert.equal(clear.status, "fail");
  assert.match(clear.reason, /^Claude SessionStart source=clear was not observed within 1.5s/);
  assert.equal(fixture.timeline.some((event) => event.action === "clear" && event.key?.startsWith("Recall the repository")), false);
});

for (const [status, options] of [
  ["pass", {}], ["blocked", { summaryDelayMs: Infinity }], ["fail", { clearStart: false }],
]) test(`Claude clear links observe output in report.json on ${status}`, async (t) => {
  const fixture = lifecycleRun({ t, agent: "claude", options: { ...options, timeoutMs: 1_500 } });
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
    const fixture = lifecycleRun({ t, agent: "codex", options: { daily: true } });
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
  const fixture = lifecycleRun({ t, agent: "codex", options: { startupSummary: false } });
  const report = await fixture.run();
  for (const row of report.lifecycle_checks) {
    assert.equal(row.status, "fail");
    assert.match(row.reason, /S2.*startup pack/);
    for (const stream of ["stdout", "stderr"]) assert.deepEqual(Object.keys(row[stream]), ["seed", "observe", "search", "parent"]);
  }
  assert.equal(fixture.operations.length, 2, "no lifecycle action may run after a failed S2 precondition");
});

test("Codex /new leaves the parent active and completes the child's lazy startup turn", async (t) => {
  const report = await lifecycleRun({ t, agent: "codex" }).run();
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
  const fixture = lifecycleRun({ t, agent: "codex", options: { summaryDelayMs: Infinity, noCredentials: true, timeoutMs: 1_500 } });
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
  const report = await lifecycleRun({ t, agent: "codex", options: { earlyClearStartup: true } }).run();
  const clear = report.lifecycle_checks.find((row) => row.check === "clear");
  assert.equal(clear.status, "fail");
  assert.match(clear.reason, /before the recall prompt is submitted/);
});

test("an occupied observer lease blocks instead of starting a competing observe", async (t) => {
  const fixture = lifecycleRun({ t, agent: "codex", options: { observerLeaseDelayMs: Infinity, timeoutMs: 1_500 } });
  const report = await fixture.run();
  assert.ok(report.lifecycle_checks.every((check) => check.status === "blocked" && /observer lease/.test(check.reason)));
  assert.equal(fixture.dependencies.calls.some((call) => call.argv[1] === "observe"), false);
});

test("a Claude compaction follow-up failure retains both legs' output paths", async (t) => {
  const report = await lifecycleRun({ t, agent: "claude", options: { followupFailure: true } }).run();
  const compact = report.lifecycle_checks.find((row) => row.check === "compact");
  assert.equal(compact.status, "fail");
  assert.match(compact.reason, /invalid request in follow-up/);
  for (const stream of ["stdout", "stderr"]) {
    assert.equal(compact[stream].compact, `<run>/lifecycle/claude/compact/${stream}.txt`);
    assert.equal(compact[stream]["compact-followup"], `<run>/lifecycle/claude/compact-followup/${stream}.txt`);
  }
});

function assertClaudeLifecycleTimeline(options) {
  const { clearEvents, observed, summarized, recalled } = options;
  const ended = clearEvents.find((event) => event.type === "session-end");
  assert.ok(ended.at < observed.at && observed.at < summarized.at && summarized.at < recalled.at,
    "recall waits for SessionEnd, the observer lease, and the parent's summary");
  const submitKeys = clearEvents.filter((event) => event.type === "key" && event.at > recalled.at);
  assert.deepEqual(submitKeys.slice(0, 2).map((event) => event.key), ["C-m", "C-m"]);
}

function assertCodexLifecycleTimeline(fixture, report, evidence) {
  const { clearEvents, observed, summarized, recalled } = evidence;
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

function assertLifecycleRunBasics(agent, fixture, report) {
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
  return { clearEvents, observed, summarized, recalled };
}
