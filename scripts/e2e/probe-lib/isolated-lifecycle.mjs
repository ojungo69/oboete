import fs from "node:fs";
import path from "node:path";

import { CLAUDE_COMPACT_PROMPT, shellQuote, writeCompactFixture } from "./agents.mjs";
import { PreconditionError, waitUntil } from "./process.mjs";
import { readyTui, tuiCmd, tuiQuit, tuiSubmit } from "./tmux.mjs";
import { LIFECYCLE_ASSERTS, LIFECYCLE_CHECKS } from "./isolated-lifecycle-report.mjs";
import {
  DONE_PROMPT,
  assertAgentOutput,
  buildFactSeedingPrompt,
  configureRemote,
  databasePath,
  factSet,
  launchAgent,
  lifecycleRecallPrompt,
  prepareAgent,
  prepareOboeteHome,
  readIfPresent,
  requireAgentSuccess,
  runObserver,
  waitForSummary,
} from "./isolated-agent.mjs";
import {
  added,
  assertion,
  eventDelta,
  eventSource,
  evaluateLifecycleCheck,
  evaluated,
  includedMemoryIds,
  session,
} from "./isolated-lifecycle-state.mjs";

export async function waitForLifecycleState(database, agent, predicate, options, dependencies, label) {
  let latest;
  const found = await waitUntil(() => {
    try {
      latest = dependencies.inspectLifecycle(database, agent);
      if (predicate(latest)) return latest;
    } catch (error) {
      if (!(error instanceof PreconditionError) || !error.message.startsWith("The oboete database is missing:")) {
        throw error;
      }
    }
    return null;
  }, options.timeoutMs, 250, dependencies);
  if (found) return found;
  const observed = (latest?.events ?? [])
    .slice(-12)
    .map((event) => {
      const source = eventSource(event);
      const sourceSuffix = source ? `:${source}` : "";
      return `${event.kind}:${event.nativeSessionId}${sourceSuffix}`;
    })
    .join(",");
  const TimeoutError = options.contract ? Error : PreconditionError;
  throw new TimeoutError(
    `${label} was not observed within ${options.timeoutMs / 1000}s; latest events=${observed || "none"}.`,
  );
}

function lifecycleTuiArgv(agent, prepared, parentNativeSessionId, action) {
  const argv =
    agent === "claude"
      ? [
          "claude",
          "--settings",
          path.join(prepared.config, "settings.json"),
          "--dangerously-skip-permissions",
          "--resume",
          parentNativeSessionId,
        ]
      : tuiCmd([action === "fork" ? "fork" : "resume", parentNativeSessionId]);
  return argv;
}

/** Only the pane's required overrides may appear in tmux's world-readable -e arguments. */
export function startLifecycleTui(options) {
  const {
    agent,
    action,
    directory,
    runtimeDir,
    repo,
    parentNativeSessionId,
    oboeteHome,
    homes,
    dependencies,
  } = options;
  fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
  const prepared = prepareAgent(agent, runtimeDir, homes, "", repo);
  const argv = lifecycleTuiArgv(agent, prepared, parentNativeSessionId, action);
  const name = `oboete-${agent}-${action}-${process.pid}-${Date.now().toString(36)}`.slice(0, 60);
  const env = {
    TERM: "xterm-256color",
    PATH: dependencies.childEnv().PATH,
    OBOETE_HOME: oboeteHome,
    ...(agent === "codex" ? { CODEX_HOME: prepared.config } : {}),
  };
  let session;
  try {
    session = dependencies.tuiSession({
      name,
      command: argv.map((arg) => shellQuote(arg)).join(" "),
      cwd: repo,
      env,
    });
  } catch (error) {
    throw new PreconditionError(
      `${agent} ${action} TUI could not start: ${error instanceof Error ? error.message : String(error)}.`,
    );
  }
  return {
    argv,
    name,
    tui: session,
  };
}

function saveTuiPane(directory, opened) {
  try {
    fs.writeFileSync(path.join(directory, "pane.txt"), opened.tui.capture());
  } catch {
    // The tmux session may already have exited; the primary failure remains authoritative.
  }
}

function soleNativeSession(events, label) {
  const ids = [...new Set(events.map((event) => event.nativeSessionId))];
  if (ids.length === 0) throw new Error(`No ${label} session was captured.`);
  if (ids.length > 1) throw new Error(`Ambiguous ${label} sessions: ${ids.join(", ")}.`);
  return ids[0];
}

export function nativeSessionFromPrompt(beforePrompt, after, parentNativeSessionId) {
  return soleNativeSession(
    added(beforePrompt, after, "events")
      .filter((event) => event.kind === "prompt" && event.nativeSessionId !== parentNativeSessionId),
    "TUI prompt",
  );
}

export function claudeNativeSessionFromStart(before, after, source) {
  return soleNativeSession(
    added(before, after, "events")
      .filter((event) => event.kind === "session_start" && eventSource(event) === source),
    `Claude SessionStart source=${source}`,
  );
}

function actionResult(agent, check, started, dependencies, evaluation, details = {}) {
  return {
    agent,
    check,
    elapsedMs: Math.max(0, dependencies.now() - started),
    ...evaluation,
    ...details,
  };
}

async function prepareParentTurn({ agent, suite, options, dependencies }, opened) {
  const database = databasePath(suite.oboeteHome);
  const before = dependencies.inspectLifecycle(database, agent);
  await tuiSubmit(opened.name, opened.tui, DONE_PROMPT, options, dependencies);
  const after = await waitForLifecycleState(
    database,
    agent,
    (snapshot) => added(before, snapshot, "events").some(
      (event) => event.nativeSessionId === suite.parentNativeSessionId && event.kind === "turn_end",
    ),
    options,
    dependencies,
    `${agent} preparation turn`,
  );
  await readyTui(agent, opened.tui, options, dependencies);
  return after;
}

function newPromptTurnEnded(beforePrompt, snapshot, parentNativeSessionId) {
  return added(beforePrompt, snapshot, "events")
    .filter((event) => event.kind === "prompt" && event.nativeSessionId !== parentNativeSessionId)
    .some((prompt) => snapshot.events.some(
      (event) => event.sessionId === prompt.sessionId && event.kind === "turn_end",
    ));
}

async function runResumeLifecycle(context) {
  const { agent, suite, options, homes, dependencies } = context;
  const started = dependencies.now();
  const before = dependencies.inspectLifecycle(databasePath(suite.oboeteHome), agent);
  const directory = path.join(suite.root, "resume");
  const extraArgs = agent === "claude" ? ["--resume", suite.parentNativeSessionId] : ["resume", suite.parentNativeSessionId];
  const result = await launchAgent({
    agent,
    directory,
    repo: suite.repo,
    prompt: DONE_PROMPT,
    options,
    homes,
    dependencies,
    oboeteHome: suite.oboeteHome,
    launch: { runtimeDir: suite.runtimeDir, extraArgs },
  });
  requireAgentSuccess(result, `${agent} resume`);
  const after = dependencies.inspectLifecycle(databasePath(suite.oboeteHome), agent);
  return actionResult(
    agent,
    "resume",
    started,
    dependencies,
    evaluateLifecycleCheck({
      agent,
      check: "resume",
      before,
      after,
      parentNativeSessionId: suite.parentNativeSessionId,
    }),
    { stdout: result.stdoutPath, stderr: result.stderrPath, eventDelta: eventDelta(before, after) },
  );
}

function openSuiteTui(agent, action, directory, suite, homes, dependencies) {
  return startLifecycleTui({
    agent,
    action,
    directory,
    runtimeDir: suite.runtimeDir,
    repo: suite.repo,
    parentNativeSessionId: suite.parentNativeSessionId,
    oboeteHome: suite.oboeteHome,
    homes,
    dependencies,
  });
}

function codexCompactResult(configuration) {
  const { agent, started, dependencies, before, after, suite, directory, opened } = configuration;
  return actionResult(
    agent,
    "compact",
    started,
    dependencies,
    evaluateLifecycleCheck({
      agent,
      check: "compact",
      before,
      after,
      parentNativeSessionId: suite.parentNativeSessionId,
    }),
    {
      pane: path.join(directory, "pane.txt"),
      argv: opened.argv,
      eventDelta: eventDelta(before, after),
    },
  );
}

async function runCodexCompactLifecycle(context) {
  const { agent, suite, options, homes, dependencies } = context;
  const started = dependencies.now();
  const database = databasePath(suite.oboeteHome);
  const directory = path.join(suite.root, "compact");
  const opened = openSuiteTui(agent, "compact", directory, suite, homes, dependencies);
  let before;
  let after;
  try {
    await readyTui(agent, opened.tui, options, dependencies);
    before = await prepareParentTurn(context, opened);

    await tuiSubmit(opened.name, opened.tui, "/compact", options, dependencies);
    await waitForLifecycleState(
      database,
      agent,
      (snapshot) => added(before, snapshot, "events").some(
        (event) => event.nativeSessionId === suite.parentNativeSessionId && event.kind === "compaction_summary",
      ),
      options,
      dependencies,
      "Codex PostCompact",
    );
    // Codex fires SessionStart(compact) only when the next turn starts (run 2026-09-05T06-02-58-033Z).
    await readyTui(agent, opened.tui, options, dependencies);
    let beforePrompt;
    await tuiSubmit(opened.name, opened.tui, DONE_PROMPT, {
      ...options,
      beforeSubmit: () => { beforePrompt = dependencies.inspectLifecycle(database, agent); },
    }, dependencies);
    await waitForLifecycleState(
      database,
      agent,
      (snapshot) => {
        const events = added(beforePrompt, snapshot, "events").filter(
          (event) => event.nativeSessionId === suite.parentNativeSessionId,
        );
        const prompt = events.findIndex((event) => event.kind === "prompt");
        return prompt >= 0 && events.slice(prompt + 1).some((event) => event.kind === "turn_end");
      },
      options,
      dependencies,
      "Codex post-compact prompt and following turn_end",
    );
    // Lizard's JavaScript reader loses this function's closing brace when an object literal sits
    // inline in an argument list, so the value lives in a named local.
    const contractOptions = { ...options, contract: true };
    after = await waitForLifecycleState(
      database,
      agent,
      (snapshot) => added(before, snapshot, "events").some(
        (event) => event.nativeSessionId === suite.parentNativeSessionId &&
          event.kind === "session_start" && eventSource(event) === "compact",
      ),
      contractOptions,
      dependencies,
      "Codex SessionStart source=compact",
    );
  } finally {
    saveTuiPane(directory, opened);
    // A missing hook or incomplete turn must not be interrupted by an exit keystroke.
    if (after !== undefined) await tuiQuit(opened.tui, opened.name, options, dependencies);
    else opened.tui.kill();
  }
  return codexCompactResult({ agent, started, dependencies, before, after, suite, directory, opened });
}

async function runCompactLifecycle(context) {
  const { agent, suite, options, homes, dependencies } = context;
  if (agent === "codex") return runCodexCompactLifecycle(context);
  const started = dependencies.now();
  const database = databasePath(suite.oboeteHome);
  const before = dependencies.inspectLifecycle(database, agent);
  writeCompactFixture(path.join(suite.repo, "big.txt"));
  const launch = {
    extraArgs: ["--resume", suite.parentNativeSessionId],
    env: { CLAUDE_CODE_AUTO_COMPACT_WINDOW: "100000" },
  };
  const run = async (name, prompt) => {
    const result = await launchAgent({
      agent,
      directory: path.join(suite.root, name),
      repo: suite.repo,
      prompt,
      options,
      homes,
      dependencies,
      oboeteHome: suite.oboeteHome,
      launch: { runtimeDir: suite.runtimeDir, ...launch },
    });
    requireAgentSuccess(result, `${agent} compact`);
  };
  await run("compact", CLAUDE_COMPACT_PROMPT);
  let after = dependencies.inspectLifecycle(database, agent);
  const count = () =>
    added(before, after, "events").filter(
      (event) => event.kind === "compaction_summary" && event.nativeSessionId === suite.parentNativeSessionId,
    ).length;
  if (count() === 0) {
    await run("compact-followup", DONE_PROMPT);
    after = dependencies.inspectLifecycle(database, agent);
  }
  if (count() === 0) throw new PreconditionError("The CLI emitted no PostCompact event, so compaction was not driven.");
  return actionResult(
    agent,
    "compact",
    started,
    dependencies,
    evaluateLifecycleCheck({
      agent,
      check: "compact",
      before,
      after,
      parentNativeSessionId: suite.parentNativeSessionId,
    }),
    {
      ...lifecycleEvidence(suite.root, ["compact", "compact-followup"]),
      eventDelta: eventDelta(before, after),
    },
  );
}

function forkLifecycleResult(configuration) {
  const { agent, started, dependencies, before, after, suite, childNativeSessionId, details } = configuration;
  return actionResult(
    agent,
    "fork",
    started,
    dependencies,
    evaluateLifecycleCheck({
      agent,
      check: "fork",
      before,
      after,
      parentNativeSessionId: suite.parentNativeSessionId,
      childNativeSessionId,
    }),
    details,
  );
}

function launchClaudeFork(agent, directory, suite, options, homes, dependencies) {
  return launchAgent({
    agent,
    directory,
    repo: suite.repo,
    prompt: lifecycleRecallPrompt(agent),
    options,
    homes,
    dependencies,
    oboeteHome: suite.oboeteHome,
    launch: {
      runtimeDir: suite.runtimeDir,
      extraArgs: ["--resume", suite.parentNativeSessionId, "--fork-session"],
    },
  });
}

async function runForkLifecycle(context) {
  const { agent, suite, options, homes, dependencies } = context;
  const started = dependencies.now();
  const database = databasePath(suite.oboeteHome);
  const before = dependencies.inspectLifecycle(database, agent);
  const directory = path.join(suite.root, "fork");
  let after;
  let details;
  let childNativeSessionId;

  if (agent === "claude") {
    const result = await launchClaudeFork(agent, directory, suite, options, homes, dependencies);
    requireAgentSuccess(result, `${agent} fork`);
    after = dependencies.inspectLifecycle(database, agent);
    childNativeSessionId = claudeNativeSessionFromStart(before, after, "fork");
    details = {
      stdout: result.stdoutPath,
      stderr: result.stderrPath,
      eventDelta: eventDelta(before, after),
    };
  } else {
    const opened = openSuiteTui(agent, "fork", directory, suite, homes, dependencies);
    let beforePrompt;
    try {
      await readyTui(agent, opened.tui, options, dependencies);
      beforePrompt = dependencies.inspectLifecycle(database, agent);
      await tuiSubmit(opened.name, opened.tui, lifecycleRecallPrompt(agent), options, dependencies);
      after = await waitForLifecycleState(
        database,
        agent,
        (snapshot) => newPromptTurnEnded(beforePrompt, snapshot, suite.parentNativeSessionId),
        options,
        dependencies,
        "forked Codex turn",
      );
      const pane = path.join(directory, "pane.txt");
      details = { pane, argv: opened.argv, eventDelta: eventDelta(before, after) };
    } finally {
      saveTuiPane(directory, opened);
      await tuiQuit(opened.tui, opened.name, options, dependencies);
    }
    childNativeSessionId = nativeSessionFromPrompt(
      beforePrompt,
      after,
      suite.parentNativeSessionId,
    );
  }

  return forkLifecycleResult({ agent, started, dependencies, before, after, suite, childNativeSessionId, details });
}

function clearLifecycleResult(configuration) {
  const { agent, started, dependencies, before, beforePrompt, after, suite, childNativeSessionId, directory, opened } = configuration;
  return actionResult(
    agent,
    "clear",
    started,
    dependencies,
    evaluateLifecycleCheck({
      agent,
      check: "clear",
      before,
      beforePrompt,
      after,
      parentNativeSessionId: suite.parentNativeSessionId,
      childNativeSessionId,
    }),
    {
      pane: path.join(directory, "pane.txt"),
      argv: opened.argv,
      eventDelta: eventDelta(before, after),
      ...lifecycleEvidence(suite.root, ["clear/observe"]),
    },
  );
}

function waitForClearParentEnd(database, agent, before, suite, options, dependencies) {
  return waitForLifecycleState(
    database,
    agent,
    (snapshot) => added(before, snapshot, "events").some(
      (event) => event.nativeSessionId === suite.parentNativeSessionId && event.kind === "session_end",
    ),
    options,
    dependencies,
    "Claude parent SessionEnd",
  );
}

function waitForClearParentSummary(database, agent, suite, options, dependencies) {
  return waitForLifecycleState(
    database,
    agent,
    (snapshot) => ["done", "no_content"].includes(session(snapshot, suite.parentNativeSessionId)?.summaryState),
    options,
    dependencies,
    `${agent} parent summary done or no_content`,
  );
}

function waitForClaudeClearStart(database, agent, before, options, dependencies) {
  return waitForLifecycleState(
    database,
    agent,
    (snapshot) =>
      added(before, snapshot, "events").some(
        (event) => event.kind === "session_start" && eventSource(event) === "clear",
      ),
    { ...options, contract: true },
    dependencies,
    "Claude SessionStart source=clear",
  );
}

async function runClearLifecycle(context) {
  const { agent, suite, options, homes, dependencies } = context;
  const started = dependencies.now();
  const database = databasePath(suite.oboeteHome);
  const directory = path.join(suite.root, "clear");
  const opened = openSuiteTui(agent, "clear", directory, suite, homes, dependencies);
  let before;
  let beforePrompt;
  let after;
  try {
    await readyTui(agent, opened.tui, options, dependencies);
    before = agent === "codex"
      ? await prepareParentTurn(context, opened)
      : dependencies.inspectLifecycle(database, agent);
    await tuiSubmit(opened.name, opened.tui, agent === "codex" ? "/new" : "/clear", options, dependencies);
    if (agent === "claude") {
      await waitForClearParentEnd(database, agent, before, suite, options, dependencies);
      // A2 never substitutes an older summary while the ended parent's summary is pending.
      const observe = await runObserver(suite.repo, directory, suite.oboeteHome, options, dependencies);
      if (![0, 1].includes(observe.exitCode)) throw new Error(`Oboete observe exited ${observe.exitCode}.`);
      await waitForClearParentSummary(database, agent, suite, options, dependencies);
      await waitForClaudeClearStart(database, agent, before, options, dependencies);
    }
    await readyTui(agent, opened.tui, options, dependencies);
    // /new's startup hook is lazy: capture A18's first-turn detection boundary before submitting.
    await tuiSubmit(opened.name, opened.tui, lifecycleRecallPrompt(agent), {
      ...options,
      beforeSubmit: () => { beforePrompt = dependencies.inspectLifecycle(database, agent); },
    }, dependencies);
    after = await waitForLifecycleState(
      database,
      agent,
      (snapshot) => {
        if (agent === "codex") {
          return newPromptTurnEnded(beforePrompt, snapshot, suite.parentNativeSessionId);
        }
        const clearSessions = new Set(
          added(before, snapshot, "events")
            .filter((event) => event.kind === "session_start" && eventSource(event) === "clear")
            .map((event) => event.sessionId),
        );
        return added(before, snapshot, "events").some(
          (event) => clearSessions.has(event.sessionId) && event.kind === "turn_end",
        );
      },
      options,
      dependencies,
      `${agent} clear turn`,
    );
    await readyTui(agent, opened.tui, options, dependencies);
  } finally {
    saveTuiPane(directory, opened);
    // tuiQuit waits for a quiet second after the final evidence snapshot before sending keys.
    await tuiQuit(opened.tui, opened.name, options, dependencies);
  }
  const childNativeSessionId =
    agent === "codex"
      ? nativeSessionFromPrompt(beforePrompt, after, suite.parentNativeSessionId)
      : claudeNativeSessionFromStart(before, after, "clear");
  return clearLifecycleResult({ agent, started, dependencies, before, beforePrompt, after, suite, childNativeSessionId, directory, opened });
}

function verifyLifecycleParent(agent, root, repo, oboeteHome, runtimeDir, dependencies, snapshot) {
  const after = dependencies.inspectLifecycle(databasePath(oboeteHome), agent);
  const parents = added(snapshot, after, "sessions");
  if (parents.length !== 1) {
    throw new Error(`Expected one new lifecycle parent session, found ${parents.length}: ${parents.map((row) => row.nativeSessionId).join(", ")}.`);
  }
  const parentSession = parents[0];
  const preconditions = [];
  const starts = added(snapshot, after, "events").filter(
    (event) => event.sessionId === parentSession.id && event.kind === "session_start" && eventSource(event) === "startup",
  );
  assertion(preconditions, "S2 records one SessionStart source=startup", starts.length === 1, 1, starts.length);
  const packs = after.injections.filter(
    (injection) => injection.sessionId === parentSession.id && injection.kind === "session_start" &&
      injection.channel === `${agent}:SessionStart` && injection.state === "emitted" &&
      injection.contextEpoch === parentSession.contextEpoch,
  );
  assertion(preconditions, `S2 emits one startup pack through ${agent}:SessionStart`, packs.length === 1, 1, packs.length);
  const summaries = new Set(snapshot.memories.filter(
    (memory) => memory.sourceSessionId === snapshot.sessions[0].id && memory.type === "session_summary" &&
      memory.repoId === parentSession.repoId && memory.deletedAt === null,
  ).map((memory) => memory.id));
  const packIds = new Set(packs.map((injection) => injection.id));
  const included = [...includedMemoryIds(after, (item) => packIds.has(item.injectionId))];
  assertion(preconditions, "S2 startup pack includes S1's repository summary", included.some((id) => summaries.has(id)), [...summaries], included);
  const evaluation = evaluated(preconditions);
  if (evaluation.status !== "pass") throw Object.assign(new Error(evaluation.reason), { assertions: preconditions });
  return {
    root,
    repo,
    oboeteHome,
    runtimeDir,
    parentNativeSessionId: parentSession.nativeSessionId,
    preconditions,
  };
}

function prepareLifecycleParent(oboeteHome, agent, dependencies, repo) {
  const snapshot = dependencies.inspectLifecycle(databasePath(oboeteHome), agent);
  if (snapshot.sessions.length !== 1) {
    throw new Error(`Expected one seed session, found ${snapshot.sessions.length}.`);
  }
  fs.writeFileSync(path.join(repo, "NOTES.md"), "The lifecycle facts are intentionally hidden.\n");
  return snapshot;
}

async function seedLifecycle(agent, context) {
  const { options, runId, runDir, homes, dependencies } = context;
  const root = path.join(runDir, "lifecycle", agent);
  const repo = dependencies.gitInit(path.join(root, "repo"));
  const git = await configureRemote(repo, root, options, dependencies);
  if (git.exitCode !== 0) throw new Error(`Git remote setup exited ${git.exitCode}.`);

  const oboeteHome = path.join(root, "oboete-home");
  prepareOboeteHome(oboeteHome, homes.oboete);
  const runtimeDir = path.join(root, "runtime");
  const facts = factSet(`lifecycle-${runId}-${agent}`);
  const seeded = await launchAgent({
    agent,
    directory: path.join(root, "seed"),
    repo,
    prompt: buildFactSeedingPrompt(facts),
    options,
    homes,
    dependencies,
    oboeteHome,
    launch: { runtimeDir },
  });
  requireAgentSuccess(seeded, `${agent} seed`);
  const noteCheck = assertAgentOutput(readIfPresent(path.join(repo, "NOTES.md")), facts);
  if (!noteCheck.pass) throw new Error("The seed file is missing one or more lifecycle facts.");

  const observerEnv = dependencies.childEnv(
    { OBOETE_HOME: oboeteHome },
    { credentials: !options.noCredentials },
  );
  const observe = await runObserver(repo, root, oboeteHome, options, dependencies);
  if (![0, 1].includes(observe.exitCode)) throw new Error(`Oboete observe exited ${observe.exitCode}.`);
  const search = await waitForSummary(repo, path.join(root, "search"), facts, options, dependencies, observerEnv);
  if (!search.found) throw new Error("The seed summary was not found.");

  const snapshot = prepareLifecycleParent(oboeteHome, agent, dependencies, repo);
  // S1 owns the facts. S2 receives them only through its startup pack, so lifecycle actions
  // cannot pass from a replay of the fact-seeding prompt, and S1 remains an ended summary source.
  const parent = await launchAgent({
    agent,
    directory: path.join(root, "parent"),
    repo,
    prompt: DONE_PROMPT,
    options,
    homes,
    dependencies,
    oboeteHome,
    launch: { runtimeDir },
  });
  requireAgentSuccess(parent, `${agent} lifecycle parent`);
  return verifyLifecycleParent(agent, root, repo, oboeteHome, runtimeDir, dependencies, snapshot);
}

function lifecycleEvidence(root, legs) {
  const evidence = {};
  for (const stream of ["stdout", "stderr"]) {
    const paths = Object.fromEntries(legs.map((leg) => [
      leg, path.join(root, path.basename(leg) === "observe" ? `${leg}.${stream}.txt` : `${leg}/${stream}.txt`),
    ]).filter(([, file]) => fs.existsSync(file)));
    if (Object.keys(paths).length > 0) evidence[stream] = paths;
  }
  return evidence;
}

function recordLifecycleSeedFailure(agent, context, error) {
  const reason = error instanceof Error ? error.message : String(error);
  for (const check of LIFECYCLE_CHECKS) context.recordResult({
    agent,
    check,
    elapsedMs: 0,
    status: error instanceof PreconditionError ? "blocked" : "fail",
    assertions: error.assertions ?? [],
    reason,
    ...lifecycleEvidence(path.join(context.runDir, "lifecycle", agent), ["seed", "observe", "search", "parent"]),
  });
}

function recordLifecycleCheckFailure(agent, context, suite, check, started, checkBefore, error) {
  let reason = error instanceof Error ? error.message : String(error);
  let status = error instanceof PreconditionError ? "blocked" : "fail";
  const evidence = lifecycleEvidence(suite.root, [check, `${check}-followup`, `${check}/observe`]);
  const directory = path.join(suite.root, check);
  const pane = path.join(directory, "pane.txt");
  if (fs.existsSync(pane)) evidence.pane = pane;
  if (checkBefore !== undefined) {
    try {
      const checkAfter = context.dependencies.inspectLifecycle(databasePath(suite.oboeteHome), agent);
      evidence.eventDelta = eventDelta(checkBefore, checkAfter);
    } catch (inspectionError) {
      status = "fail";
      reason += ` Evidence inspection failed: ${inspectionError instanceof Error ? inspectionError.message : String(inspectionError)}`;
    }
  }
  context.dependencies.log(`[${agent}:${check}] ${status}: ${reason}`);
  context.recordResult({
    agent,
    check,
    elapsedMs: Math.max(0, context.dependencies.now() - started),
    status,
    assertions: suite.preconditions,
    reason,
    ...evidence,
  });
}

export async function runLifecycleAgent(agent, context) {
  let suite;
  try {
    suite = await seedLifecycle(agent, context);
  } catch (error) {
    recordLifecycleSeedFailure(agent, context, error);
    return;
  }

  const runners = {
    resume: runResumeLifecycle,
    compact: runCompactLifecycle,
    fork: runForkLifecycle,
    clear: runClearLifecycle,
  };
  for (const check of LIFECYCLE_CHECKS) {
    context.dependencies.log(`[${agent}:${check}] asserts ${LIFECYCLE_ASSERTS[check]}`);
    const started = context.dependencies.now();
    let checkBefore;
    try {
      checkBefore = context.dependencies.inspectLifecycle(databasePath(suite.oboeteHome), agent);
      const result = await runners[check]({ ...context, agent, suite });
      const reasonSuffix = result.reason ? `: ${result.reason}` : "";
      context.dependencies.log(`[${agent}:${check}] ${result.status}${reasonSuffix}`);
      context.recordResult({ ...result, assertions: [...suite.preconditions, ...result.assertions] });
    } catch (error) {
      recordLifecycleCheckFailure(agent, context, suite, check, started, checkBefore, error);
    }
  }
}
