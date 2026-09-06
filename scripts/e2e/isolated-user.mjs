#!/usr/bin/env node
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { setTimeout as sleep } from "node:timers/promises";
import { parseArgs as parseNodeArgs } from "node:util";
import { fileURLToPath } from "node:url";

import {
  AGENT_OUTAGE_RE,
  CLAUDE_COMPACT_PROMPT,
  GROK_ISOLATION_ENV,
  PreconditionError,
  childEnv,
  copyMode,
  finalText,
  gitInit,
  isCredentialVariable,
  redactValue,
  runTimed,
  shellQuote,
  waitUntil,
  writeCompactFixture,
} from "./probe-lib/agents.mjs";
import { readyTui, tmux, tmuxSession, tuiCmd, tuiQuit, tuiSubmit } from "./probe-lib/tmux.mjs";

export const AGENTS = ["claude", "codex", "grok", "pi"];
export const LIFECYCLE_AGENTS = ["claude", "codex"];
export const LIFECYCLE_CHECKS = ["resume", "compact", "fork", "clear"];

const AGENT_SET = new Set(AGENTS);
const LIFECYCLE_AGENT_SET = new Set(LIFECYCLE_AGENTS);
const TOTAL_PAIRS = AGENTS.length * (AGENTS.length - 1);
const DEFAULT_TIMEOUT_MS = 120_000;
const SYNTHETIC_REMOTE = "https://example.invalid/oboete-e2e.git";
const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(HERE, "../..");
const CODEX_LIFECYCLE_RUN = "2026-09-05T06-02-58-033Z";
const CODEX_CLEAR_RUN = "2026-09-05T07-03-44-495Z";
const DONE_PROMPT = "Reply with exactly DONE and do not use tools.";
const RECALL_TOOL = {
  claude: "Use the Read tool exactly once on NOTES.md.",
  codex: "Use the shell tool exactly once to run: sed -n '1,20p' NOTES.md",
  grok: "Use the read_file tool exactly once on NOTES.md.",
  pi: "Use the read tool exactly once on NOTES.md.",
};
const LIFECYCLE_ASSERTS = {
  resume:
    "The resumed prompt stays in its oboete conversation and context_epoch without repeating its session-start pack.",
  compact:
    "One compaction advances context_epoch once, re-injects repository memory via SessionStart source=compact, and loses no earlier event.",
  fork:
    "The fork is a separate conversation whose ledger includes repository memory without changing the parent ledger.",
  clear:
    `Claude clear injects at SessionStart; Codex /new creates and injects a new root at the first turn's lazy SessionStart source=startup, before UserPromptSubmit, leaving the parent injections unchanged; the parent stays active because /new fires no SessionEnd (run ${CODEX_CLEAR_RUN}).`,
};

export function requireAgentSuccess(result, action) {
  if (result.exitCode === 0) return;
  const diagnostic = `${result.stderr ?? ""}\n${result.stdout ?? ""}`;
  const detail = diagnostic
    .split(/\r?\n/)
    .map((line) => line.trim())
    .find(Boolean);
  const unavailable = result.exitCode === 124 || AGENT_OUTAGE_RE.test(diagnostic);
  const reason = `${action} exited ${result.exitCode}${detail ? `: ${detail.slice(0, 240)}` : ""}.`;
  if (unavailable) throw new PreconditionError(reason);
  throw new Error(reason);
}

function usage() {
  return `Usage: node scripts/e2e/isolated-user.mjs [options]

Options:
  --pairs all|A:B[,A:B...]  Ordered agent pairs (default: all).
  --lifecycle                 Run resume, compact, fork, and clear checks instead of pairs.
  --agents claude,codex       Lifecycle agents (default: claude,codex).
  --no-credentials          Remove oboete provider credentials and use fallback summaries.
  --daily                   Append this run to docs/evidence/m1-dogfood.md.
  --timeout <s>             Agent and summary deadline in seconds (default: 120).
  --run-dir <path>          Store this run at an explicit path.
  -h, --help                Show this help.
`;
}

export function enumerateLifecycleAgents(spec) {
  if (typeof spec !== "string" || spec.trim() === "") {
    throw new Error("The --agents option must be a comma-separated list of claude and/or codex.");
  }
  const agents = spec.split(",").map((value) => value.trim().toLowerCase());
  const seen = new Set();
  for (const agent of agents) {
    if (!LIFECYCLE_AGENT_SET.has(agent)) {
      throw new Error(`Unknown lifecycle agent '${agent}'; use ${LIFECYCLE_AGENTS.join(", ")}.`);
    }
    if (seen.has(agent)) throw new Error(`Duplicate lifecycle agent '${agent}'.`);
    seen.add(agent);
  }
  return agents;
}

export function enumeratePairs(spec) {
  if (spec === "all") {
    return AGENTS.flatMap((from) => AGENTS.filter((to) => to !== from).map((to) => ({ from, to })));
  }
  if (typeof spec !== "string" || spec.trim() === "") {
    throw new Error("--pairs must be 'all' or a comma-separated A:B list");
  }

  const pairs = [];
  const seen = new Set();
  for (const item of spec.split(",")) {
    const parts = item.split(":").map((value) => value.trim().toLowerCase());
    if (parts.length !== 2 || parts.some((value) => value === "")) {
      throw new Error(`invalid pair '${item}'; expected A:B`);
    }
    const [from, to] = parts;
    if (!AGENT_SET.has(from) || !AGENT_SET.has(to)) {
      throw new Error(`unknown agent in pair '${item}'; use ${AGENTS.join(", ")}`);
    }
    if (from === to) throw new Error(`pair '${item}' must name two distinct agents`);
    const key = `${from}:${to}`;
    if (seen.has(key)) throw new Error(`duplicate pair '${key}'`);
    seen.add(key);
    pairs.push({ from, to });
  }
  return pairs;
}

export function parseArguments(argv) {
  // main() turns anything thrown here into the usage message and exit 2, so the message Node's
  // own parseArgs writes for an unknown option is the one the developer sees.
  const { values } = parseNodeArgs({
    args: argv,
    strict: true,
    options: {
      pairs: { type: "string", default: "all" },
      lifecycle: { type: "boolean", default: false },
      agents: { type: "string", default: LIFECYCLE_AGENTS.join(",") },
      "no-credentials": { type: "boolean", default: false },
      daily: { type: "boolean", default: false },
      timeout: { type: "string", default: String(DEFAULT_TIMEOUT_MS / 1000) },
      "run-dir": { type: "string" },
      help: { type: "boolean", short: "h", default: false },
    },
  });

  if (!/^[1-9]\d*$/.test(values.timeout)) {
    throw new Error("--timeout must be a positive integer number of seconds");
  }
  const timeoutMs = Number(values.timeout) * 1000;
  if (!Number.isSafeInteger(timeoutMs)) {
    throw new Error("--timeout must be a positive integer number of seconds");
  }
  if (values["run-dir"] !== undefined && values["run-dir"].trim() === "") {
    throw new Error("--run-dir must not be empty");
  }
  const hasPairs = argv.some((value) => value === "--pairs" || value.startsWith("--pairs="));
  const hasAgents = argv.some((value) => value === "--agents" || value.startsWith("--agents="));
  if (values.lifecycle && hasPairs) throw new Error("The --lifecycle option cannot be combined with --pairs.");
  if (!values.lifecycle && hasAgents) throw new Error("The --agents option requires --lifecycle.");

  return {
    lifecycle: values.lifecycle,
    agents: enumerateLifecycleAgents(values.agents),
    pairs: enumeratePairs(values.pairs),
    noCredentials: values["no-credentials"],
    daily: values.daily,
    timeoutMs,
    runDir: values["run-dir"] ?? null,
    help: values.help,
  };
}

export function buildFactSeedingPrompt(facts) {
  if (!Array.isArray(facts) || facts.length !== 3 || facts.some((fact) => typeof fact !== "string" || fact === "")) {
    throw new TypeError("fact seeding requires exactly three non-empty strings");
  }
  const command = `printf '%s\\n' ${facts.map((fact) => shellQuote(fact)).join(" ")} >> NOTES.md`;
  return [
    "These three exact strings are durable facts about this repository. Preserve them verbatim:",
    ...facts,
    "Use exactly one tool call and no other tools. In that one call, use the shell tool to run:",
    command,
    "After the tool result, reply on one line with the same three exact strings joined by |.",
  ].join("\n");
}

export function assertAgentOutput(output, facts, { requireDegraded = false } = {}) {
  const text = String(output).normalize("NFC");
  const normalize = (value) => String(value).normalize("NFC").replace(/\s+/gu, " ").trim();
  const normalizedOutput = normalize(text);
  const missingFacts = facts.filter((fact) => !normalizedOutput.includes(normalize(fact)));
  const degradedMarker = text
    .split(/\r?\n/u)
    .some((line) => /^>\s*degraded:\s+.*\brule-based notes\.\s*$/iu.test(line.trim()));
  return {
    pass: missingFacts.length === 0 && (!requireDegraded || degradedMarker),
    missingFacts,
    degradedMarker,
  };
}

function factSet(stem) {
  return [
    `${stem}-1: the build token is cedar.`,
    `${stem}-2: the release bird is heron.`,
    `${stem}-3: 配布色は琥珀。`,
  ];
}

function lifecycleRecallPrompt(agent) {
  return [
    "Recall the repository's build token, release bird, and 配布色 only from the oboete memory context.",
    RECALL_TOOL[agent],
    "The file intentionally hides those values; do not derive the answer from it and make no other tool call.",
    "Reply with every matching fact line verbatim, joined by |.",
  ].join(" ");
}

function recallPrompt(agent, noCredentials) {
  const timing =
    agent === "grok"
      ? "The oboete memory context is delivered with that first tool result; use the fact lines inside its markers."
      : "Before the tool call, remember the fact lines already present inside the oboete memory context markers.";
  return [
    timing,
    RECALL_TOOL[agent],
    "Make no other tool call.",
    "After the result, reply with every remembered fact line verbatim, joined by |. Do not derive the answer from NOTES.md.",
    ...(noCredentials
      ? ["Also copy the complete > degraded: line from the oboete memory context onto its own line."]
      : []),
  ].join("\n");
}

export function createReport({
  runId,
  runDir,
  startedAt,
  finishedAt,
  noCredentials,
  timeoutMs,
  daily = false,
  requestedPairs,
  results,
}) {
  const passed = results.filter((result) => result.status === "pass").length;
  const report = {
    runId,
    runDir,
    started_at: startedAt,
    finished_at: finishedAt,
    no_credentials: noCredentials,
    daily,
    timeout_seconds: timeoutMs / 1000,
    requested_pairs: requestedPairs,
    total_pairs: TOTAL_PAIRS,
    // SC-001 is the twelve-pair run, so only a twelve-pair run may report against twelve; a
    // shorter run says so, because this line is what --daily writes into the evidence file.
    summary:
      requestedPairs === TOTAL_PAIRS
        ? `${passed} of ${TOTAL_PAIRS} pairs pass`
        : `${passed} of ${requestedPairs} requested pairs pass (partial run; SC-001 needs all ${TOTAL_PAIRS})`,
    pairs: results.map((result) => ({
      agents: { seed: result.from, receive: result.to },
      elapsed_ms: result.elapsedMs,
      status: result.status,
      missing_facts: result.missingFacts,
      stdout: result.stdout,
      stderr: result.stderr,
      ...(result.degradedMarker === undefined ? {} : { degraded_marker: result.degradedMarker }),
      ...(result.reason === undefined ? {} : { reason: result.reason }),
      ...(result.searchAttempts === undefined ? {} : { search_attempts: result.searchAttempts }),
    })),
  };
  return redactValue(report, runDir, "<run>");
}

export function createLifecycleReport({
  runId,
  runDir,
  startedAt,
  finishedAt,
  noCredentials,
  timeoutMs,
  daily = false,
  agents,
  results,
}) {
  const passed = results.filter((result) => result.status === "pass").length;
  const total = agents.length * LIFECYCLE_CHECKS.length;
  return redactValue(
    {
      mode: "lifecycle",
      runId,
      runDir,
      started_at: startedAt,
      finished_at: finishedAt,
      no_credentials: noCredentials,
      daily,
      timeout_seconds: timeoutMs / 1000,
      requested_agents: agents,
      total_checks: total,
      summary: `${passed} of ${total} lifecycle checks pass.`,
      lifecycle_checks: results.map((result) => ({
        agent: result.agent,
        check: result.check,
        asserts: LIFECYCLE_ASSERTS[result.check],
        elapsed_ms: result.elapsedMs,
        status: result.status,
        assertions: result.assertions ?? [],
        ...(result.stdout === undefined ? {} : { stdout: result.stdout }),
        ...(result.stderr === undefined ? {} : { stderr: result.stderr }),
        ...(result.pane === undefined ? {} : { pane: result.pane }),
        ...(result.argv === undefined ? {} : { argv: result.argv }),
        ...(result.eventDelta === undefined ? {} : { event_delta: result.eventDelta }),
        ...(result.evidence === undefined ? {} : { evidence: result.evidence }),
        ...(result.reason === undefined ? {} : { reason: result.reason }),
      })),
    },
    runDir,
    "<run>",
  );
}

function parsePayload(value) {
  try {
    return JSON.parse(String(value ?? "{}"));
  } catch {
    return {};
  }
}

/** Read-only evidence for the lifecycle assertions. */
export function inspectLifecycle(databasePath, agent) {
  if (!fs.existsSync(databasePath)) throw new PreconditionError(`The oboete database is missing: ${databasePath}.`);
  const db = new DatabaseSync(databasePath, { readOnly: true, timeout: 1000 });
  try {
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

function assertion(asserts, statement, pass, expected, actual) {
  asserts.push({ assertion: statement, pass: Boolean(pass), expected, actual });
}

function session(snapshot, nativeSessionId) {
  return snapshot.sessions.find((row) => row.nativeSessionId === nativeSessionId);
}

function added(before, after, field) {
  const known = new Set(before[field].map((row) => row.id));
  return after[field].filter((row) => !known.has(row.id));
}

function eventDelta(before, after) {
  return added(before, after, "events").map((event) => ({
    id: event.id,
    session_id: event.sessionId,
    native_session_id: event.nativeSessionId,
    kind: event.kind,
    payload: event.payload,
    captured_at: event.capturedAt,
  }));
}

function eventSource(event) {
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

function includedMemoryIds(snapshot, matches) {
  return new Set(
    snapshot.items
      .filter((item) => item.decision === "included" && item.memoryId !== null && matches(item))
      .map((item) => item.memoryId),
  );
}

function includesRepositoryMemory(repository, included) {
  return [...included].some((memoryId) => repository.has(memoryId));
}

function evaluated(assertions) {
  const failed = assertions.filter((item) => !item.pass);
  return {
    status: failed.length === 0 ? "pass" : "fail",
    assertions,
    ...(failed.length === 0 ? {} : { reason: failed.map((item) => item.assertion).join("; ") }),
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

  if (check === "compact") {
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
    const remaining = new Set(after.events.map((event) => event.id));
    const lost = before.events
      .filter((event) => event.sessionId === parentBefore?.id)
      .map((event) => event.id)
      .filter((id) => !remaining.has(id));
    assertion(assertions, "no event captured before compaction is lost", lost.length === 0, [], lost);
    return evaluated(assertions);
  }

  const child = session(after, childNativeSessionId);
  const memories = repoMemoryIds(after, parentBefore?.repoId);

  if (check === "fork") {
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
    return evaluated(assertions);
  }

  if (check === "clear") {
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
    const newEvents = added(before, after, "events").filter((event) => event.sessionId === child?.id);
    if (agent === "codex") {
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
    return {
      ...evaluated(assertions),
      ...(agent === "codex" ? { evidence: {
        parent_session_end_count: added(before, after, "events").filter(
          (event) => event.sessionId === parentBefore?.id && event.kind === "session_end",
        ).length,
      } } : {}),
    };
  }

  throw new Error(`Unknown lifecycle check: ${check}.`);
}

function configuredHome(env, name, fallback) {
  const value = env[name]?.trim();
  return value ? path.resolve(value) : fallback;
}

export function resolveSourceHomes(env, home) {
  const homes = {
    oboete: configuredHome(env, "OBOETE_HOME", path.join(home, ".oboete")),
    claude: configuredHome(env, "CLAUDE_CONFIG_DIR", path.join(home, ".claude")),
    codex: configuredHome(env, "CODEX_HOME", path.join(home, ".codex")),
    grok: configuredHome(env, "GROK_HOME", path.join(home, ".grok")),
    pi: configuredHome(env, "PI_CODING_AGENT_DIR", path.join(home, ".pi", "agent")),
  };
  const realHome = fs.realpathSync(home);
  for (const [name, configured] of Object.entries(homes)) {
    const target = fs.existsSync(configured) ? fs.realpathSync(configured) : path.resolve(configured);
    const relative = path.relative(realHome, target);
    if (relative === ".." || relative.startsWith(`..${path.sep}`) || path.isAbsolute(relative)) {
      throw new PreconditionError(`${name} setup home escapes the isolated account: ${configured}`);
    }
  }
  return homes;
}

/**
 * A Codex trust key is `<absolute hooks.json path>:<snake_case event>:<group>:<handler>` and the
 * hash covers the handler group alone (src/setup/codex-trust.ts), so the copy of the developer's
 * config.toml keeps the trust setup wrote once each key names the copy of hooks.json. Without this
 * every copied row still names the original path, no row can ever match, and Codex skips the oboete
 * hooks in silence (FR-031). That is what --dangerously-bypass-hook-trust used to hide, and hiding
 * it meant the dogfood run could not see a trust regression at all.
 */
export function retargetCodexTrust(configText, sourceHooksPath, destinationHooksPath) {
  // A TOML basic string takes the escapes JSON produces, which is how setup wrote the key.
  const from = JSON.stringify(sourceHooksPath).slice(1, -1);
  const to = JSON.stringify(destinationHooksPath).slice(1, -1);
  let rows = 0;
  const retargeted = configText.replace(
    /^([ \t]*\[hooks\.state\.")(.*)("\][ \t]*)$/gmu,
    (line, head, key, tail) => {
      if (!key.startsWith(`${from}:`)) return line;
      rows += 1;
      return `${head}${to}${key.slice(from.length)}${tail}`;
    },
  );
  if (rows === 0) {
    throw new PreconditionError(
      `no Codex trust row names ${sourceHooksPath}; run oboete setup in the isolated account`,
    );
  }
  return retargeted;
}

function copySetupFile(source, destination, required = false) {
  if (!fs.existsSync(source)) {
    if (required) throw new PreconditionError(`missing setup file: ${source}`);
    return;
  }
  copyMode(source, destination, fs.statSync(source).mode & 0o7777);
}

function prepareOboeteHome(destination, source) {
  fs.mkdirSync(destination, { recursive: true, mode: 0o700 });
  copySetupFile(path.join(source, "config.toml"), path.join(destination, "config.toml"), true);
}

function prepareAgent(agent, directory, homes, prompt, repo, extraArgs = []) {
  const config = path.join(directory, "agent-home");
  switch (agent) {
    case "claude": {
      const settings = path.join(config, "settings.json");
      copySetupFile(path.join(homes.claude, "settings.json"), settings, true);
      return {
        argv: [
          "claude",
          "-p",
          prompt,
          "--settings",
          settings,
          "--dangerously-skip-permissions",
          "--output-format",
          "json",
          ...extraArgs,
        ],
        env: {},
        config,
      };
    }
    case "codex": {
      for (const file of ["auth.json", "config.toml", "hooks.json"]) {
        copySetupFile(path.join(homes.codex, file), path.join(config, file), file !== "auth.json");
      }
      const configToml = path.join(config, "config.toml");
      // The TUI asks "Do you trust the contents of this directory?" for a repository it has not
      // seen, and Escape (the first key tuiSubmit sends) answers "No, quit"; a headless leg never
      // asks. Trust the synthetic repository up front, the way the CLI records a "Yes, continue".
      const trusted = `\n[projects.${JSON.stringify(repo)}]\ntrust_level = "trusted"\n`;
      fs.writeFileSync(
        configToml,
        retargetCodexTrust(
          fs.readFileSync(configToml, "utf8"),
          path.join(homes.codex, "hooks.json"),
          path.join(config, "hooks.json"),
        ) + trusted,
      );
      return {
        argv: [
          "codex",
          "exec",
          "--dangerously-bypass-approvals-and-sandbox",
          "--skip-git-repo-check",
          "--json",
          "-C",
          repo,
          ...extraArgs,
          prompt,
        ],
        env: { CODEX_HOME: config },
        config,
      };
    }
    case "grok": {
      copySetupFile(path.join(homes.grok, "auth.json"), path.join(config, "auth.json"));
      copySetupFile(path.join(homes.grok, "config.toml"), path.join(config, "config.toml"), true);
      copySetupFile(
        path.join(homes.grok, "hooks", "oboete.json"),
        path.join(config, "hooks", "oboete.json"),
        true,
      );
      return {
        argv: ["grok", "-p", prompt, "--always-approve", "--output-format", "json", "--cwd", repo],
        env: { GROK_HOME: config, ...GROK_ISOLATION_ENV },
      };
    }
    case "pi": {
      for (const file of ["auth.json", "settings.json", "models-store.json"]) {
        copySetupFile(path.join(homes.pi, file), path.join(config, file));
      }
      copySetupFile(
        path.join(homes.pi, "extensions", "oboete.js"),
        path.join(config, "extensions", "oboete.js"),
        true,
      );
      const sessions = path.join(directory, "pi-sessions");
      fs.mkdirSync(sessions, { recursive: true });
      return {
        argv: ["pi", "-p", prompt, "--mode", "json", "--session-dir", sessions],
        env: { PI_CODING_AGENT_DIR: config },
      };
    }
    default:
      throw new Error(`unknown agent: ${agent}`);
  }
}

async function launchAgent(
  agent,
  directory,
  repo,
  prompt,
  options,
  homes,
  dependencies,
  oboeteHome,
  launch = {},
) {
  fs.mkdirSync(directory, { recursive: true });
  const prepared = prepareAgent(agent, launch.runtimeDir ?? directory, homes, prompt, repo, launch.extraArgs ?? []);
  const stdoutPath = path.join(directory, "stdout.txt");
  const stderrPath = path.join(directory, "stderr.txt");
  const proc = await dependencies.runTimed(prepared.argv, {
    cwd: repo,
    // An agent CLI runs the developer's shell tools; childEnv keeps the credentials out of it.
    env: dependencies.childEnv({ ...prepared.env, ...(launch.env ?? {}), OBOETE_HOME: oboeteHome }),
    stdoutPath,
    stderrPath,
    timeoutMs: options.timeoutMs,
  });
  return { ...proc, stdoutPath, stderrPath };
}

/** `oboete search --json` always answers with `{ memories: [...] }` (src/memories-cli.ts). */
function searchContainsFacts(output, facts) {
  let parsed;
  try {
    parsed = JSON.parse(output);
  } catch {
    return false;
  }
  return (parsed?.memories ?? []).some((row) => assertAgentOutput(JSON.stringify(row), facts).pass);
}

async function waitForSummary(repo, directory, facts, options, dependencies, env) {
  fs.mkdirSync(directory, { recursive: true });
  const stdoutPath = path.join(directory, "stdout.txt");
  const stderrPath = path.join(directory, "stderr.txt");
  const deadline = dependencies.now() + options.timeoutMs;
  let attempts = 0;
  while (dependencies.now() < deadline) {
    attempts += 1;
    const remaining = deadline - dependencies.now();
    const result = await dependencies.runTimed(["oboete", "search", facts[0], "--json"], {
      cwd: repo,
      env,
      stdoutPath,
      stderrPath,
      timeoutMs: Math.min(15_000, remaining),
    });
    if (result.exitCode === 0 && searchContainsFacts(result.stdout, facts)) {
      return { found: true, attempts };
    }
    if (result.exitCode === 3) return { found: false, attempts };
    const wait = Math.min(1_000, deadline - dependencies.now());
    if (wait > 0) await dependencies.sleep(wait);
  }
  return { found: false, attempts };
}

function resultPaths(pairDir) {
  return {
    stdout: {
      seed: path.join(pairDir, "seed", "stdout.txt"),
      receive: path.join(pairDir, "receive", "stdout.txt"),
    },
    stderr: {
      seed: path.join(pairDir, "seed", "stderr.txt"),
      receive: path.join(pairDir, "receive", "stderr.txt"),
    },
  };
}

async function runPair(pair, context) {
  const { options, runId, runDir, homes, dependencies } = context;
  const started = dependencies.now();
  const pairDir = path.join(runDir, `${pair.from}-to-${pair.to}`);
  const paths = resultPaths(pairDir);
  const facts = factSet(`fact-${runId}-${pair.from}-to-${pair.to}`);
  const finish = (status, missingFacts, details = {}) => ({
    ...pair,
    elapsedMs: Math.max(0, dependencies.now() - started),
    status,
    missingFacts,
    ...paths,
    ...details,
  });

  try {
    fs.mkdirSync(pairDir, { recursive: true, mode: 0o700 });
    const repo = dependencies.gitInit(path.join(pairDir, "repo"));
    const git = await dependencies.runTimed(["git", "config", "remote.origin.url", SYNTHETIC_REMOTE], {
      cwd: repo,
      env: dependencies.childEnv(),
      stdoutPath: path.join(pairDir, "git.stdout.txt"),
      stderrPath: path.join(pairDir, "git.stderr.txt"),
      timeoutMs: Math.min(15_000, options.timeoutMs),
    });
    if (git.exitCode !== 0) return finish("fail", facts, { reason: `git_remote_exit_${git.exitCode}` });

    const oboeteHome = path.join(pairDir, "oboete-home");
    prepareOboeteHome(oboeteHome, homes.oboete);
    // FR-016: `oboete observe` is the one leg that reaches a provider, so it is the one leg that
    // asks for the credentials; --no-credentials is the run that takes them away from it.
    const env = dependencies.childEnv(
      { OBOETE_HOME: oboeteHome },
      { credentials: !options.noCredentials },
    );

    dependencies.log(`[${pair.from}:${pair.to}] seed`);
    const seeded = await launchAgent(
      pair.from,
      path.join(pairDir, "seed"),
      repo,
      buildFactSeedingPrompt(facts),
      options,
      homes,
      dependencies,
      oboeteHome,
    );
    if (seeded.exitCode !== 0) {
      return finish("fail", facts, { reason: `seed_agent_exit_${seeded.exitCode}` });
    }

    const notes = path.join(repo, "NOTES.md");
    // Read and answer "not there" in one step: asking first and reading after is a check the file
    // can outlive, and the agent that writes this file is still exiting.
    const noteCheck = assertAgentOutput(readIfPresent(notes), facts);
    if (!noteCheck.pass) {
      return finish("fail", noteCheck.missingFacts, { reason: "seed_file_missing_facts" });
    }

    const observe = await runObserver(repo, pairDir, oboeteHome, options, dependencies);
    if (![0, 1].includes(observe.exitCode)) {
      return finish("fail", facts, { reason: `observe_exit_${observe.exitCode}` });
    }

    const search = await waitForSummary(repo, path.join(pairDir, "search"), facts, options, dependencies, env);
    if (!search.found) {
      return finish("fail", facts, { reason: "summary_not_found", searchAttempts: search.attempts });
    }

    // B must learn the facts from oboete, not from the required NOTES.md read itself.
    fs.writeFileSync(notes, "The seeded facts are intentionally hidden during the recall check.\n");

    dependencies.log(`[${pair.from}:${pair.to}] receive`);
    const received = await launchAgent(
      pair.to,
      path.join(pairDir, "receive"),
      repo,
      recallPrompt(pair.to, options.noCredentials),
      options,
      homes,
      dependencies,
      oboeteHome,
    );
    if (received.exitCode !== 0) {
      return finish("fail", facts, {
        reason: `receive_agent_exit_${received.exitCode}`,
        searchAttempts: search.attempts,
      });
    }
    const assertion = assertAgentOutput(finalText(pair.to, received, []), facts, {
      requireDegraded: options.noCredentials,
    });
    return finish(assertion.pass ? "pass" : "fail", assertion.missingFacts, {
      ...(assertion.pass
        ? {}
        : {
            reason:
              assertion.missingFacts.length > 0
                ? "facts_missing_from_first_turn"
                : "degraded_marker_missing_from_first_turn",
          }),
      degradedMarker: assertion.degradedMarker,
      searchAttempts: search.attempts,
    });
  } catch (error) {
    return finish(error instanceof PreconditionError ? "skipped" : "fail", facts, {
      reason: error instanceof Error ? error.message : String(error),
    });
  }
}

function databasePath(oboeteHome) {
  return path.join(oboeteHome, "memory.db");
}

export function observerLeaseIsFree(database, now = Date.now()) {
  if (!fs.existsSync(database)) return true;
  const db = new DatabaseSync(database, { readOnly: true, timeout: 1000 });
  try {
    const lease = db.prepare("SELECT owner_token, heartbeat_at FROM worker_lease WHERE id = 1").get();
    // R6: match src/worker/lease.ts without importing TypeScript into this Node 22.16 ESM harness.
    return lease === undefined || lease.owner_token === null || !Number.isFinite(lease.heartbeat_at) ||
      now - lease.heartbeat_at > 6_000 || lease.heartbeat_at - now > 60_000;
  } finally {
    db.close();
  }
}

async function runObserver(repo, directory, oboeteHome, options, dependencies) {
  const idle = await waitUntil(
    () => dependencies.observerLeaseIsFree(databasePath(oboeteHome), dependencies.now()),
    options.timeoutMs, 250, dependencies,
  );
  if (!idle) throw new PreconditionError(`The observer lease was not released within ${options.timeoutMs / 1000}s.`);
  return dependencies.runTimed(["oboete", "observe"], {
    cwd: repo,
    env: dependencies.childEnv({ OBOETE_HOME: oboeteHome }, { credentials: !options.noCredentials }),
    stdoutPath: path.join(directory, "observe.stdout.txt"),
    stderrPath: path.join(directory, "observe.stderr.txt"),
    timeoutMs: options.timeoutMs,
  });
}

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
    .map((event) => `${event.kind}:${event.nativeSessionId}${eventSource(event) ? `:${eventSource(event)}` : ""}`)
    .join(",");
  const TimeoutError = options.contract ? Error : PreconditionError;
  throw new TimeoutError(
    `${label} was not observed within ${options.timeoutMs / 1000}s; latest events=${observed || "none"}.`,
  );
}

/** Only the pane's required overrides may appear in tmux's world-readable -e arguments. */
export function startLifecycleTui({
  agent,
  action,
  directory,
  runtimeDir,
  repo,
  parentNativeSessionId,
  oboeteHome,
  homes,
  dependencies,
}) {
  fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
  const prepared = prepareAgent(agent, runtimeDir, homes, "", repo);
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
  const result = await launchAgent(
    agent,
    directory,
    suite.repo,
    DONE_PROMPT,
    options,
    homes,
    dependencies,
    suite.oboeteHome,
    { runtimeDir: suite.runtimeDir, extraArgs },
  );
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

async function runCodexCompactLifecycle(context) {
  const { agent, suite, options, homes, dependencies } = context;
  const started = dependencies.now();
  const database = databasePath(suite.oboeteHome);
  const directory = path.join(suite.root, "compact");
  const opened = startLifecycleTui({
    agent,
    action: "compact",
    directory,
    runtimeDir: suite.runtimeDir,
    repo: suite.repo,
    parentNativeSessionId: suite.parentNativeSessionId,
    oboeteHome: suite.oboeteHome,
    homes,
    dependencies,
  });
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
    after = await waitForLifecycleState(
      database,
      agent,
      (snapshot) => added(before, snapshot, "events").some(
        (event) => event.nativeSessionId === suite.parentNativeSessionId &&
          event.kind === "session_start" && eventSource(event) === "compact",
      ),
      { ...options, contract: true },
      dependencies,
      "Codex SessionStart source=compact",
    );
  } finally {
    saveTuiPane(directory, opened);
    // A missing hook or incomplete turn must not be interrupted by an exit keystroke.
    if (after !== undefined) await tuiQuit(opened.tui, opened.name, options, dependencies);
    else opened.tui.kill();
  }
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
    const result = await launchAgent(
      agent,
      path.join(suite.root, name),
      suite.repo,
      prompt,
      options,
      homes,
      dependencies,
      suite.oboeteHome,
      { runtimeDir: suite.runtimeDir, ...launch },
    );
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
    const result = await launchAgent(
      agent,
      directory,
      suite.repo,
      lifecycleRecallPrompt(agent),
      options,
      homes,
      dependencies,
      suite.oboeteHome,
      {
        runtimeDir: suite.runtimeDir,
        extraArgs: ["--resume", suite.parentNativeSessionId, "--fork-session"],
      },
    );
    requireAgentSuccess(result, `${agent} fork`);
    after = dependencies.inspectLifecycle(database, agent);
    childNativeSessionId = claudeNativeSessionFromStart(before, after, "fork");
    details = {
      stdout: result.stdoutPath,
      stderr: result.stderrPath,
      eventDelta: eventDelta(before, after),
    };
  } else {
    const opened = startLifecycleTui({
      agent,
      action: "fork",
      directory,
      runtimeDir: suite.runtimeDir,
      repo: suite.repo,
      parentNativeSessionId: suite.parentNativeSessionId,
      oboeteHome: suite.oboeteHome,
      homes,
      dependencies,
    });
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

async function runClearLifecycle(context) {
  const { agent, suite, options, homes, dependencies } = context;
  const started = dependencies.now();
  const database = databasePath(suite.oboeteHome);
  const directory = path.join(suite.root, "clear");
  const opened = startLifecycleTui({
    agent,
    action: "clear",
    directory,
    runtimeDir: suite.runtimeDir,
    repo: suite.repo,
    parentNativeSessionId: suite.parentNativeSessionId,
    oboeteHome: suite.oboeteHome,
    homes,
    dependencies,
  });
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
      await waitForLifecycleState(
        database,
        agent,
        (snapshot) => added(before, snapshot, "events").some(
          (event) => event.nativeSessionId === suite.parentNativeSessionId && event.kind === "session_end",
        ),
        options,
        dependencies,
        "Claude parent SessionEnd",
      );
      // A2 never substitutes an older summary while the ended parent's summary is pending.
      const observe = await runObserver(suite.repo, directory, suite.oboeteHome, options, dependencies);
      if (![0, 1].includes(observe.exitCode)) throw new Error(`Oboete observe exited ${observe.exitCode}.`);
      await waitForLifecycleState(
        database,
        agent,
        (snapshot) => ["done", "no_content"].includes(session(snapshot, suite.parentNativeSessionId)?.summaryState),
        options,
        dependencies,
        `${agent} parent summary done or no_content`,
      );
      await waitForLifecycleState(
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

async function seedLifecycle(agent, context) {
  const { options, runId, runDir, homes, dependencies } = context;
  const root = path.join(runDir, "lifecycle", agent);
  const repo = dependencies.gitInit(path.join(root, "repo"));
  const git = await dependencies.runTimed(["git", "config", "remote.origin.url", SYNTHETIC_REMOTE], {
    cwd: repo,
    env: dependencies.childEnv(),
    stdoutPath: path.join(root, "git.stdout.txt"),
    stderrPath: path.join(root, "git.stderr.txt"),
    timeoutMs: Math.min(15_000, options.timeoutMs),
  });
  if (git.exitCode !== 0) throw new Error(`Git remote setup exited ${git.exitCode}.`);

  const oboeteHome = path.join(root, "oboete-home");
  prepareOboeteHome(oboeteHome, homes.oboete);
  const runtimeDir = path.join(root, "runtime");
  const facts = factSet(`lifecycle-${runId}-${agent}`);
  const seeded = await launchAgent(
    agent,
    path.join(root, "seed"),
    repo,
    buildFactSeedingPrompt(facts),
    options,
    homes,
    dependencies,
    oboeteHome,
    { runtimeDir },
  );
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

  const snapshot = dependencies.inspectLifecycle(databasePath(oboeteHome), agent);
  if (snapshot.sessions.length !== 1) {
    throw new Error(`Expected one seed session, found ${snapshot.sessions.length}.`);
  }
  fs.writeFileSync(path.join(repo, "NOTES.md"), "The lifecycle facts are intentionally hidden.\n");
  // S1 owns the facts. S2 receives them only through its startup pack, so lifecycle actions
  // cannot pass from a replay of the fact-seeding prompt, and S1 remains an ended summary source.
  const parent = await launchAgent(
    agent, path.join(root, "parent"), repo, DONE_PROMPT,
    options, homes, dependencies, oboeteHome, { runtimeDir },
  );
  requireAgentSuccess(parent, `${agent} lifecycle parent`);
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

async function runLifecycleAgent(agent, context) {
  let suite;
  try {
    suite = await seedLifecycle(agent, context);
  } catch (error) {
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
      context.dependencies.log(`[${agent}:${check}] ${result.status}${result.reason ? `: ${result.reason}` : ""}`);
      context.recordResult({ ...result, assertions: [...suite.preconditions, ...result.assertions] });
    } catch (error) {
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
  }
}

function runIdNow(date) {
  return date.toISOString().replace(/[:.]/g, "-");
}

function markdownSection(report) {
  let markdown = `## ${report.started_at.slice(0, 10)} run ${report.runId}\n\n`;
  markdown += `- ${report.summary}\n`;
  markdown += `- No provider credentials: ${report.no_credentials ? "yes" : "no"}\n`;
  markdown += `- Report: ${report.runDir}/report.json\n\n`;
  if (report.mode === "lifecycle") {
    return `${markdown}${lifecycleRows(report)}\n`;
  }
  markdown += "| seed | receive | status | elapsed ms | missing facts |\n|---|---|---:|---:|---|\n";
  for (const pair of report.pairs) {
    markdown += `| ${pair.agents.seed} | ${pair.agents.receive} | ${pair.status} | ${pair.elapsed_ms} | ${pair.missing_facts.join("; ") || "none"} |\n`;
  }
  return `${markdown}\n`;
}

function markdownCell(value) {
  return String(value).replace(/\\/g, "\\\\").replace(/\|/g, "\\|").replace(/\r?\n/g, " ");
}

function evidenceReason(reason) {
  let line = String(reason).split(/[\r\n\u2028\u2029]/u, 1)[0];
  // Keep credential-bearing diagnostics in the private report, including values after the name.
  const credential = [...line.matchAll(/\b[A-Z][A-Z0-9_]*\b/g)].find(([name]) => isCredentialVariable(name));
  if (credential) line = `${line.slice(0, credential.index)}[redacted]`;
  return line.replace(/\bBearer\s+\S+/gi, "Bearer [redacted]").slice(0, 240);
}

function lifecycleRows(report) {
  let output = "| agent | check | status | elapsed ms | asserts | reason |\n|---|---|---:|---:|---|---|\n";
  for (const check of report.lifecycle_checks) {
    output += `| ${check.agent} | ${check.check} | ${check.status} | ${check.elapsed_ms} | ${markdownCell(check.asserts)} | ${markdownCell(evidenceReason(check.reason ?? "none"))} |\n`;
  }
  return output;
}

/** The file's content, or an empty string when it is not there. */
function readIfPresent(file) {
  try {
    return fs.readFileSync(file, "utf8");
  } catch {
    return "";
  }
}

function writeDaily(report, repoRoot) {
  const destination = path.join(repoRoot, "docs", "evidence", "m1-dogfood.md");
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  // One handle answers both questions: whether the file already has content, and where to append.
  // Asking with existsSync and appending afterwards is the same check-then-write race as above.
  const handle = fs.openSync(destination, "a+");
  try {
    const heading =
      fs.fstatSync(handle).size > 0
        ? ""
        : "# oboete M1 dogfood evidence\n\nIsolated-user cross-agent runs for SC-001, SC-004, and SC-007.\n\n";
    fs.writeFileSync(handle, heading + markdownSection(report));
  } finally {
    fs.closeSync(handle);
  }
}

export async function runHarness(options, overrides = {}) {
  const dependencies = {
    runTimed,
    gitInit,
    childEnv,
    inspectLifecycle,
    observerLeaseIsFree,
    tmux,
    tuiSession: tmuxSession,
    sleep,
    now: Date.now,
    env: process.env,
    home: os.homedir(),
    repoRoot: REPO_ROOT,
    log: (message) => console.error(message),
    ...overrides,
  };
  const started = new Date(dependencies.now());
  const runId = runIdNow(started);
  const runDir = path.resolve(options.runDir ?? path.join(dependencies.home, ".cache", "oboete-e2e", runId));
  const homes = resolveSourceHomes(dependencies.env, dependencies.home);
  fs.mkdirSync(runDir, { recursive: true, mode: 0o700 });

  const results = [];
  const reportNow = options.lifecycle
    ? () =>
        createLifecycleReport({
          runId,
          runDir,
          startedAt: started.toISOString(),
          finishedAt: new Date(dependencies.now()).toISOString(),
          noCredentials: options.noCredentials,
          daily: options.daily,
          timeoutMs: options.timeoutMs,
          agents: options.agents,
          results,
        })
    : () =>
        createReport({
          runId,
          runDir,
          startedAt: started.toISOString(),
          finishedAt: new Date(dependencies.now()).toISOString(),
          noCredentials: options.noCredentials,
          daily: options.daily,
          timeoutMs: options.timeoutMs,
          requestedPairs: options.pairs.length,
          results,
        });
  if (options.lifecycle) {
    const recordResult = (result) => {
      results.push(result);
      fs.writeFileSync(path.join(runDir, "report.json"), `${JSON.stringify(reportNow(), null, 2)}\n`);
    };
    for (const agent of options.agents) {
      await runLifecycleAgent(agent, { options, runId, runDir, homes, dependencies, recordResult });
    }
  } else {
    for (const pair of options.pairs) {
      results.push(await runPair(pair, { options, runId, runDir, homes, dependencies }));
      fs.writeFileSync(path.join(runDir, "report.json"), `${JSON.stringify(reportNow(), null, 2)}\n`);
    }
  }

  const report = reportNow();
  if (options.daily) writeDaily(report, dependencies.repoRoot);
  return report;
}

async function main(argv) {
  let options;
  try {
    options = parseArguments(argv);
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n${usage()}`);
    return 2;
  }
  if (options.help) {
    process.stdout.write(usage());
    return 0;
  }
  const account = os.userInfo();
  if (account.username !== "oboete-dogfood") {
    process.stderr.write("Refusing to run outside the isolated oboete-dogfood user.\n");
    return 1;
  }
  if (path.resolve(os.homedir()) !== path.resolve(account.homedir)) {
    process.stderr.write("Refusing to run because HOME is not the oboete-dogfood account home; use sudo -H.\n");
    return 1;
  }

  try {
    const report = await runHarness(options);
    process.stdout.write(report.mode === "lifecycle" ? `${lifecycleRows(report)}${report.summary}\n` : `${report.summary}\n`);
    const results = report.mode === "lifecycle" ? report.lifecycle_checks : report.pairs;
    return results.every((result) => result.status === "pass") ? 0 : 1;
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    return 1;
  }
}

if (path.resolve(process.argv[1] || "") === fileURLToPath(import.meta.url)) {
  process.exitCode = await main(process.argv.slice(2));
}
