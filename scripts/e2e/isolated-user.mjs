#!/usr/bin/env node
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { parseArgs as parseNodeArgs } from "node:util";
import { fileURLToPath } from "node:url";

import { finalText, redactValue } from "./probe-lib/agent-events.mjs";
import { childEnv, gitInit, PreconditionError, runTimed } from "./probe-lib/process.mjs";
import { tmux, tmuxSession } from "./probe-lib/tmux.mjs";
import {
  assertAgentOutput,
  buildFactSeedingPrompt,
  configureRemote,
  factSet,
  launchAgent,
  observerLeaseIsFree,
  prepareOboeteHome,
  readIfPresent,
  recallPrompt,
  resolveSourceHomes,
  runObserver,
  waitForSummary,
} from "./probe-lib/isolated-agent.mjs";
import { runLifecycleAgent } from "./probe-lib/isolated-lifecycle.mjs";
import { createLifecycleReport, lifecycleRows } from "./probe-lib/isolated-lifecycle-report.mjs";
import { inspectLifecycle } from "./probe-lib/isolated-lifecycle-state.mjs";
export const AGENTS = ["claude", "codex", "grok", "pi"];
export const LIFECYCLE_AGENTS = ["claude", "codex"];

const AGENT_SET = new Set(AGENTS);
const LIFECYCLE_AGENT_SET = new Set(LIFECYCLE_AGENTS);
const TOTAL_PAIRS = AGENTS.length * (AGENTS.length - 1);
const DEFAULT_TIMEOUT_MS = 120_000;
const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(HERE, "../..");

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
    if (parts.length !== 2 || parts.includes("")) {
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
    throw new TypeError("--timeout must be a positive integer number of seconds");
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

export function createReport(options) {
  const {
    runId,
    runDir,
    startedAt,
    finishedAt,
    noCredentials,
    timeoutMs,
    daily = false,
    requestedPairs,
    results,
  } = options;
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

function evaluatePairRecall(pair, received, facts, options, finish, search) {
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
}

function preparePairHome(pairDir, homes, dependencies, options) {
  const oboeteHome = path.join(pairDir, "oboete-home");
  prepareOboeteHome(oboeteHome, homes.oboete);
  // FR-016: `oboete observe` is the one leg that reaches a provider, so it is the one leg that
  // asks for the credentials; --no-credentials is the run that takes them away from it.
  const env = dependencies.childEnv(
    { OBOETE_HOME: oboeteHome },
    { credentials: !options.noCredentials },
  );

  return { oboeteHome, env };
}

function launchPairSeed(configuration) {
  const { pair, pairDir, repo, facts, options, homes, dependencies, oboeteHome } = configuration;
  return launchAgent({
    agent: pair.from,
    directory: path.join(pairDir, "seed"),
    repo,
    prompt: buildFactSeedingPrompt(facts),
    options,
    homes,
    dependencies,
    oboeteHome,
  });
}

function launchPairRecall(pair, pairDir, repo, options, homes, dependencies, oboeteHome) {
  return launchAgent({
    agent: pair.to,
    directory: path.join(pairDir, "receive"),
    repo,
    prompt: recallPrompt(pair.to, options.noCredentials),
    options,
    homes,
    dependencies,
    oboeteHome,
  });
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
    const git = await configureRemote(repo, pairDir, options, dependencies);
    if (git.exitCode !== 0) return finish("fail", facts, { reason: `git_remote_exit_${git.exitCode}` });

    const { oboeteHome, env } = preparePairHome(pairDir, homes, dependencies, options);

    dependencies.log(`[${pair.from}:${pair.to}] seed`);
    const seeded = await launchPairSeed({ pair, pairDir, repo, facts, options, homes, dependencies, oboeteHome });
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
    const received = await launchPairRecall(pair, pairDir, repo, options, homes, dependencies, oboeteHome);
    if (received.exitCode !== 0) {
      return finish("fail", facts, {
        reason: `receive_agent_exit_${received.exitCode}`,
        searchAttempts: search.attempts,
      });
    }
    return evaluatePairRecall(pair, received, facts, options, finish, search);
  } catch (error) {
    return finish(error instanceof PreconditionError ? "skipped" : "fail", facts, {
      reason: error instanceof Error ? error.message : String(error),
    });
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

function createReportReader(options, runId, runDir, started, dependencies, results) {
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
  return reportNow;
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
  const reportNow = createReportReader(options, runId, runDir, started, dependencies, results);
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
