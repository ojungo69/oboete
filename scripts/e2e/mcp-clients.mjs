#!/usr/bin/env node
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { parseArgs as parseNodeArgs } from "node:util";
import { fileURLToPath } from "node:url";
import { DatabaseSync } from "node:sqlite";

import {
  GROK_ISOLATION_ENV,
} from "./probe-lib/agents.mjs";
import {
  AGENT_OUTAGE_RE,
  PreconditionError,
  childEnv,
  runTimed,
} from "./probe-lib/process.mjs";
import { readMcpFrames } from "./probe-lib/mcp-frames.mjs";
import { buildReportRow, finishHarnessReport, reportMarkdown } from "./probe-lib/mcp-report.mjs";
import {
  assertAgentFrames,
  assertGetMissing,
  assertPiJson,
  assertRepoRejected,
  extractToolName,
} from "./probe-lib/mcp-assertions.mjs";

export const AGENTS = ["claude", "codex", "grok", "pi"];
export const MCP_AGENTS = ["claude", "codex", "grok"];

const AGENT_SET = new Set(AGENTS);
const HERE = path.dirname(fileURLToPath(import.meta.url));
const TEE = path.join(HERE, "probe-lib", "mcp-tee.mjs");
const PROBE_NAME = "oboete_probe";
const DEFAULT_TIMEOUT_MS = 180_000;
const STDIO_TIMEOUT_MS = 15_000;
const SEARCH_PROMPT =
  'Use the oboete_probe search tool with the query "wiring probe" and reply with the number of memories it returned; do not use any other tool.';
const PI_PROMPT =
  'Use the oboete_search tool with the query "wiring probe" and reply with the number of memories it returned; do not use any other tool.';

function usage() {
  return `Usage: node scripts/e2e/mcp-clients.mjs [options]

Options:
  --agents claude,codex,grok,pi  Agents to run (default: all four).
  --out <dir>                    Run directory (default: ~/.cache/oboete-mcp-clients/<runId>/).
  --daily                        Append this run to docs/evidence/m1-dogfood.md in the cwd.
  -h, --help                     Show this help.
`;
}

export function enumerateAgents(spec) {
  if (typeof spec !== "string" || spec.trim() === "") {
    throw new Error("The --agents option must be a comma-separated list of claude, codex, grok, and/or pi.");
  }
  const agents = spec.split(",").map((value) => value.trim().toLowerCase());
  const seen = new Set();
  for (const agent of agents) {
    if (!AGENT_SET.has(agent)) throw new Error(`Unknown agent '${agent}'; use ${AGENTS.join(", ")}.`);
    if (seen.has(agent)) throw new Error(`Duplicate agent '${agent}'.`);
    seen.add(agent);
  }
  return agents;
}

export function parseArguments(argv) {
  const { values } = parseNodeArgs({
    args: argv,
    strict: true,
    options: {
      agents: { type: "string", default: AGENTS.join(",") },
      out: { type: "string" },
      daily: { type: "boolean", default: false },
      help: { type: "boolean", short: "h", default: false },
    },
  });
  if (values.out !== undefined && values.out.trim() === "") throw new Error("--out must not be empty");
  return {
    agents: enumerateAgents(values.agents),
    out: values.out ?? null,
    daily: values.daily,
    help: values.help,
  };
}

export function removeTomlServerTables(text, name) {
  const header = new RegExp(String.raw`^\[mcp_servers\.${name}(?:\.[^\]]+)?\]\s*$`);
  const out = [];
  let skipping = false;
  for (const line of String(text).split("\n")) {
    if (header.test(line)) {
      skipping = true;
      continue;
    }
    if (skipping && /^\s*\[/.test(line)) skipping = false;
    if (!skipping) out.push(line);
  }
  return out.join("\n").replace(/\n{3,}/g, "\n\n");
}

export function probeServerToml({ node, tee, bundle, log, startupTimeoutSec = 15 }) {
  return [
    "",
    `[mcp_servers.${PROBE_NAME}]`,
    `command = ${JSON.stringify(node)}`,
    `args = [${JSON.stringify(tee)}, ${JSON.stringify(bundle)}]`,
    `startup_timeout_sec = ${startupTimeoutSec}`,
    "",
    `[mcp_servers.${PROBE_NAME}.env]`,
    `PROBE_MCP_LOG = ${JSON.stringify(log)}`,
    "",
  ].join("\n");
}

/** The file's text, or undefined when it does not exist (read, never check-then-read). */
function readIfPresent(file) {
  try {
    return fs.readFileSync(file, "utf8");
  } catch (error) {
    if (error?.code === "ENOENT") return undefined;
    throw error;
  }
}

function appendProbeToml(file, block) {
  const text = readIfPresent(file);
  const kept = text === undefined ? "" : `${removeTomlServerTables(text, PROBE_NAME).trimEnd()}\n`;
  // mode applies only when the file is created; an existing file keeps its own mode.
  fs.writeFileSync(file, `${kept}${block.trim()}\n`, { mode: 0o600 });
}

function stripProbeToml(file) {
  const text = readIfPresent(file);
  if (text !== undefined) fs.writeFileSync(file, removeTomlServerTables(text, PROBE_NAME));
}

function whichBin(name, env) {
  const result = spawnSync("which", [name], { encoding: "utf8", env: env || childEnv() });
  const found = (result.stdout || "").trim();
  return result.status === 0 && found ? found : null;
}

export function resolveBundle(env = process.env, home = os.homedir()) {
  if (env.OBOETE_BUNDLE && fs.existsSync(env.OBOETE_BUNDLE)) return fs.realpathSync(env.OBOETE_BUNDLE);
  const bin = whichBin("oboete", childEnv(env));
  if (bin) {
    try {
      const real = fs.realpathSync(bin);
      if (real.endsWith("oboete.mjs")) return real;
    } catch {
      /* fall through */
    }
  }
  const fallback = path.join(home, ".npm-global/lib/node_modules/oboete/dist/oboete.mjs");
  if (fs.existsSync(fallback)) return fallback;
  throw new PreconditionError("oboete bundle not found; install the package for this account");
}

function runIdNow(date) {
  return date.toISOString().replace(/[:.]/g, "-");
}

function writeLog(file, dir, frame) {
  fs.appendFileSync(file, `${JSON.stringify({ dir, at: new Date().toISOString(), frame })}\n`);
}

function isUnavailable(proc) {
  const diagnostic = `${proc.stderr ?? ""}\n${proc.stdout ?? ""}`;
  return proc.exitCode === 124 || AGENT_OUTAGE_RE.test(diagnostic);
}

function firstLine(text) {
  return String(text || "")
    .split(/\r?\n/u)
    .map((line) => line.trim())
    .find(Boolean);
}

function readHookLog(home) {
  const file = path.join(home, ".oboete", "logs", "hook.log");
  try {
    return fs.readFileSync(file, "utf8");
  } catch {
    return "";
  }
}

function recentDbToolNames(home, agent, sinceMs) {
  const dbPath = path.join(home, ".oboete", "memory.db");
  if (!fs.existsSync(dbPath)) return [];
  try {
    const db = new DatabaseSync(dbPath, { readOnly: true });
    try {
      const rows = db
        .prepare(
          `SELECT payload_json FROM raw_events
           WHERE agent = ? AND kind IN ('tool_call', 'tool_result') AND captured_at >= ?
           ORDER BY captured_at DESC LIMIT 30`,
        )
        .all(agent, Math.floor(sinceMs / 1000));
      return rows.map((row) => row.payload_json).filter(Boolean);
    } finally {
      db.close();
    }
  } catch {
    return [];
  }
}

async function runCli(argv, { cwd, env, dir, timeoutMs }) {
  fs.mkdirSync(dir, { recursive: true });
  return runTimed(argv, {
    cwd,
    env,
    stdoutPath: path.join(dir, "stdout.txt"),
    stderrPath: path.join(dir, "stderr.txt"),
    timeoutMs,
  });
}

function blockedRow(agent, reason) {
  return buildReportRow({ agent, status: "blocked", reason });
}

function failRow(agent, reason, extra = {}) {
  return buildReportRow({ agent, status: "fail", reason, ...extra });
}

async function registerClaude({ node, tee, bundle, log, env, dir }) {
  await runCli(["claude", "mcp", "remove", PROBE_NAME, "--scope", "user"], {
    cwd: dir,
    env,
    dir: path.join(dir, "_reg"),
    timeoutMs: 30_000,
  });
  const add = await runCli(
    ["claude", "mcp", "add", PROBE_NAME, "--scope", "user", "-e", `PROBE_MCP_LOG=${log}`, "--", node, tee, bundle],
    { cwd: dir, env, dir: path.join(dir, "_reg"), timeoutMs: 30_000 },
  );
  if (add.exitCode !== 0) {
    const combined = `${add.stderr}\n${add.stdout}`;
    throw new Error(`claude mcp add oboete_probe exited ${add.exitCode}: ${firstLine(combined) || "no output"}`);
  }
}

async function unregisterClaude({ env, dir }) {
  await runCli(["claude", "mcp", "remove", PROBE_NAME, "--scope", "user"], {
    cwd: dir,
    env,
    dir: path.join(dir, "_unreg"),
    timeoutMs: 30_000,
  });
}

function registerCodex({ node, tee, bundle, log, configPath }) {
  appendProbeToml(configPath, probeServerToml({ node, tee, bundle, log }));
}

function unregisterCodex(configPath) {
  stripProbeToml(configPath);
}

async function registerGrok({ node, tee, bundle, log, env, dir }) {
  await runCli(["grok", "mcp", "remove", "--scope", "user", PROBE_NAME], {
    cwd: dir,
    env,
    dir: path.join(dir, "_reg"),
    timeoutMs: 30_000,
  });
  const add = await runCli(
    ["grok", "mcp", "add", "--scope", "user", PROBE_NAME, "-e", `PROBE_MCP_LOG=${log}`, "--", node, tee, bundle],
    { cwd: dir, env, dir: path.join(dir, "_reg"), timeoutMs: 30_000 },
  );
  if (add.exitCode !== 0) {
    const combined = `${add.stderr}\n${add.stdout}`;
    throw new Error(`grok mcp add oboete_probe exited ${add.exitCode}: ${firstLine(combined) || "no output"}`);
  }
}

async function unregisterGrok({ env, dir, configPath }) {
  await runCli(["grok", "mcp", "remove", "--scope", "user", PROBE_NAME], {
    cwd: dir,
    env,
    dir: path.join(dir, "_unreg"),
    timeoutMs: 30_000,
  });
  stripProbeToml(configPath);
}

function agentArgv(agent, prompt, { repo, tee, bundle, log }) {
  if (agent === "claude") {
    return ["claude", "-p", prompt, "--output-format", "json", "--dangerously-skip-permissions"];
  }
  if (agent === "codex") {
    const override = `mcp_servers.${PROBE_NAME}={command=${JSON.stringify(process.execPath)}, args=[${JSON.stringify(tee)}, ${JSON.stringify(bundle)}], env={PROBE_MCP_LOG=${JSON.stringify(log)}}, startup_timeout_sec=15}`;
    return [
      "codex",
      "exec",
      "--dangerously-bypass-approvals-and-sandbox",
      "--skip-git-repo-check",
      "--json",
      "-C",
      repo,
      "-c",
      override,
      prompt,
    ];
  }
  if (agent === "grok") {
    return ["grok", "-p", prompt, "--always-approve", "--output-format", "json", "--cwd", repo];
  }
  return ["pi", "-p", prompt, "--mode", "json", "--no-builtin-tools", "--session-dir", path.join(path.dirname(log), "sessions")];
}

function agentEnv(agent, extra) {
  if (agent === "grok") return childEnv({ ...GROK_ISOLATION_ENV, ...extra });
  return childEnv(extra);
}

function assertStdioOutput(proc, log) {
  const outFrames = (proc.stdout || "")
    .split("\n")
    .filter((line) => line.trim())
    .map((line) => {
      try {
        return JSON.parse(line);
      } catch {
        return { parse_error: line.slice(0, 120) };
      }
    });
  for (const frame of outFrames) writeLog(log, "out", frame);
  const repoFrame = outFrames.find((frame) => frame?.id === 1) || outFrames[0];
  const getFrame = outFrames.find((frame) => frame?.id === 2) || outFrames[1];
  const repoAssert = assertRepoRejected(repoFrame);
  const getAssert = assertGetMissing(getFrame);
  const ok = repoAssert.ok && getAssert.ok;
  return {
    status: ok ? "pass" : "fail",
    frames: readMcpFrames(log).frames.length,
    assertions: [
      `repo -32602: ${repoAssert.ok ? "pass" : "fail"} (${repoAssert.reason})`,
      `get missing isError: ${getAssert.ok ? "pass" : "fail"} (${getAssert.reason})`,
    ],
    reason: ok ? "stdio repo rejection and missing get" : [repoAssert, getAssert].filter((a) => !a.ok).map((a) => a.reason).join("; "),
    proc,
  };
}

function closeStdioFiles(outFd, errFd, inFd) {
  for (const fd of [outFd, errFd, inFd]) {
    try {
      fs.closeSync(fd);
    } catch {
      /* already closed */
    }
  }
}

async function runStdio({ repo, bundle, log, env }) {
  fs.mkdirSync(path.dirname(log), { recursive: true });
  fs.writeFileSync(log, "");
  const search = {
    jsonrpc: "2.0",
    id: 1,
    method: "tools/call",
    params: { name: "search", arguments: { query: "x", repo: "/elsewhere" } },
  };
  const get = {
    jsonrpc: "2.0",
    id: 2,
    method: "tools/call",
    params: { name: "get", arguments: { id: "m_missing" } },
  };
  const stdin = `${JSON.stringify(search)}\n${JSON.stringify(get)}\n`;
  const stdinPath = path.join(path.dirname(log), "stdin.jsonl");
  const stdoutPath = path.join(path.dirname(log), "stdout.txt");
  const stderrPath = path.join(path.dirname(log), "stderr.txt");
  fs.writeFileSync(stdinPath, stdin);
  writeLog(log, "in", search);
  writeLog(log, "in", get);
  const start = Date.now();
  const outFd = fs.openSync(stdoutPath, "w");
  const errFd = fs.openSync(stderrPath, "w");
  const inFd = fs.openSync(stdinPath, "r");
  let child;
  try {
    child = spawnSync(process.execPath, [bundle, "mcp"], {
      cwd: repo,
      env,
      timeout: STDIO_TIMEOUT_MS,
      stdio: [inFd, outFd, errFd],
    });
  } finally {
    closeStdioFiles(outFd, errFd, inFd);
  }
  const proc = {
    exitCode: child?.status == null ? 1 : child.status,
    stdout: readIfPresent(stdoutPath) ?? "",
    stderr: readIfPresent(stderrPath) ?? "",
    elapsedMs: Date.now() - start,
  };
  return assertStdioOutput(proc, log);
}

function readMcpAgentEvidence(log, proc, home, agent, started) {
  const parsed = readMcpFrames(log);
  const extracted = extractToolName({
    stdout: proc.stdout,
    stderr: proc.stderr,
    hookLog: readHookLog(home),
    dbPayloads: recentDbToolNames(home, agent, started - 5_000),
  });
  return { parsed, extracted };
}

function mcpAgentReport(agent, proc, parsed, extracted) {
  const asserted = assertAgentFrames(parsed.frames);
  const toolName = extracted.toolName;
  const reasonParts = [];
  if (!asserted.ok) reasonParts.push(asserted.reason);
  if (extracted.reason && toolName === "unknown") reasonParts.push(extracted.reason);
  if (proc.exitCode !== 0 && asserted.ok) reasonParts.push(`exit=${proc.exitCode}`);
  const status = asserted.ok ? "pass" : "fail";
  return buildReportRow({
    agent,
    status,
    protocolVersion: asserted.protocolVersion || parsed.protocolVersion,
    toolName,
    frames: parsed.frames.length,
    reason: reasonParts.join("; ") || asserted.reason,
  });
}

async function cleanupMcpAgent(registered, agent, env, dir, codexConfig, grokConfig) {
  if (registered || agent === "claude" || agent === "grok") {
    try {
      if (agent === "claude") await unregisterClaude({ env, dir });
      else if (agent === "codex") unregisterCodex(codexConfig);
      else if (agent === "grok") await unregisterGrok({ env, dir, configPath: grokConfig });
    } catch {
      /* still leave oboete itself alone */
    }
  }
}

function agentErrorMessage(error) {
  return error instanceof Error ? error.message : String(error);
}

async function runMcpAgent(agent, context) {
  const { repo, node, tee, bundle, runDir, home, timeoutMs, log } = context;
  const dir = path.join(runDir, agent);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(log, "");
  const env = agentEnv(agent);
  const bin = whichBin(agent, env);
  if (!bin) return blockedRow(agent, `${agent} CLI is not on PATH`);

  const codexConfig = path.join(home, ".codex", "config.toml");
  const grokConfig = path.join(home, ".grok", "config.toml");
  let registered = false;
  try {
    if (agent === "claude") await registerClaude({ node, tee, bundle, log, env, dir });
    else if (agent === "codex") {
      if (!fs.existsSync(codexConfig)) return blockedRow(agent, "missing ~/.codex/config.toml");
      registerCodex({ node, tee, bundle, log, configPath: codexConfig });
    } else if (agent === "grok") {
      await registerGrok({ node, tee, bundle, log, env, dir });
    }
    registered = true;
    const started = Date.now();
    const proc = await runCli(agentArgv(agent, SEARCH_PROMPT, { repo, tee, bundle, log }), {
      cwd: repo,
      env,
      dir,
      timeoutMs,
    });
    const { parsed, extracted } = readMcpAgentEvidence(log, proc, home, agent, started);
    if (isUnavailable(proc) && parsed.frames.length === 0) {
      const combined = `${proc.stderr}\n${proc.stdout}`;
      return blockedRow(agent, `${agent} exited ${proc.exitCode}: ${firstLine(combined) || "unavailable"}`);
    }
    return mcpAgentReport(agent, proc, parsed, extracted);
  } catch (error) {
    const message = agentErrorMessage(error);
    if (error instanceof PreconditionError || AGENT_OUTAGE_RE.test(message)) return blockedRow(agent, message);
    return failRow(agent, message);
  } finally {
    await cleanupMcpAgent(registered, agent, env, dir, codexConfig, grokConfig);
  }
}

async function runPi(context) {
  const { repo, runDir, timeoutMs } = context;
  const dir = path.join(runDir, "pi");
  fs.mkdirSync(dir, { recursive: true });
  const env = childEnv();
  if (!whichBin("pi", env)) return blockedRow("pi", "pi CLI is not on PATH");
  const sessions = path.join(dir, "sessions");
  fs.mkdirSync(sessions, { recursive: true });
  try {
    const proc = await runCli(["pi", "-p", PI_PROMPT, "--mode", "json", "--no-builtin-tools", "--session-dir", sessions], {
      cwd: repo,
      env,
      dir,
      timeoutMs,
    });
    if (isUnavailable(proc)) {
      const combined = `${proc.stderr}\n${proc.stdout}`;
      return blockedRow("pi", `pi exited ${proc.exitCode}: ${firstLine(combined) || "unavailable"}`);
    }
    const asserted = assertPiJson(proc.stdout);
    fs.writeFileSync(path.join(dir, "pi-assert.json"), `${JSON.stringify({ types: asserted.types, reason: asserted.reason }, null, 2)}\n`);
    return buildReportRow({
      agent: "pi",
      status: asserted.ok ? "pass" : "fail",
      protocolVersion: null,
      toolName: asserted.toolName,
      frames: 0,
      reason: asserted.reason,
    });
  } catch (error) {
    const message = agentErrorMessage(error);
    if (error instanceof PreconditionError || AGENT_OUTAGE_RE.test(message)) return blockedRow("pi", message);
    return failRow("pi", message);
  }
}

export async function runHarness(options, overrides = {}) {
  const dependencies = {
    now: Date.now,
    env: process.env,
    home: os.homedir(),
    cwd: process.cwd(),
    log: (message) => console.error(message),
    ...overrides,
  };
  const started = new Date(dependencies.now());
  const runId = runIdNow(started);
  const runDir = path.resolve(options.out ?? path.join(dependencies.home, ".cache", "oboete-mcp-clients", runId));
  fs.mkdirSync(runDir, { recursive: true, mode: 0o700 });
  const repo = dependencies.cwd;
  const node = process.execPath;
  const tee = TEE;
  const bundle = resolveBundle(dependencies.env, dependencies.home);
  const timeoutMs = DEFAULT_TIMEOUT_MS;
  const env = childEnv();

  const stdioLog = path.join(runDir, "stdio", "frames.jsonl");
  dependencies.log("[stdio] tools/call repo rejection and missing get");
  const stdio = await runStdio({ repo, bundle, log: stdioLog, env });
  dependencies.log(`[stdio] ${stdio.status}: ${stdio.reason}`);

  const rows = [];
  const context = { repo, node, tee, bundle, runDir, home: dependencies.home, timeoutMs };
  for (const agent of options.agents) {
    dependencies.log(`[${agent}] start`);
    const row =
      agent === "pi"
        ? await runPi(context)
        : await runMcpAgent(agent, { ...context, log: path.join(runDir, agent, "frames.jsonl") });
    rows.push(row);
    dependencies.log(`[${agent}] ${row.status}: ${row.reason}`);
    fs.writeFileSync(path.join(runDir, "report.json"), `${JSON.stringify({ agents: rows }, null, 2)}\n`);
  }

  return finishHarnessReport(options, dependencies, stdio, rows, runId, runDir, started);
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
  try {
    const report = await runHarness(options);
    process.stdout.write(reportMarkdown(report));
    process.stdout.write(`${report.summary}\n`);
    return report.exit_code;
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    return 1;
  }
}

if (path.resolve(process.argv[1] || "") === fileURLToPath(import.meta.url)) {
  process.exitCode = await main(process.argv.slice(2));
}
