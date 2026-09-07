#!/usr/bin/env node
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { parseArgs as parseNodeArgs } from "node:util";
import { fileURLToPath } from "node:url";
import { DatabaseSync } from "node:sqlite";

import {
  AGENT_OUTAGE_RE,
  GROK_ISOLATION_ENV,
  PreconditionError,
  childEnv,
  parseJsonl,
  parseMaybeJson,
  redactValue,
  runTimed,
} from "./probe-lib/agents.mjs";
import { readMcpFrames } from "./probe-lib/mcp-frames.mjs";

export const AGENTS = ["claude", "codex", "grok", "pi"];
export const MCP_AGENTS = ["claude", "codex", "grok"];
export const SUPPORTED_PROTOCOL_VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];
export const LATEST_PROTOCOL = SUPPORTED_PROTOCOL_VERSIONS.at(-1);
export const EXPECTED_TOOLS = ["search", "timeline", "get"];

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

export function messageOf(entry) {
  return entry?.frame && typeof entry.frame === "object" ? entry.frame : entry;
}

function listed(frames) {
  return (frames || []).map((entry) => ({ dir: entry.dir, at: entry.at, msg: messageOf(entry) }));
}

function pairByMethod(frames, method) {
  const rows = listed(frames);
  const inn = rows.find((row) => row.dir === "in" && row.msg?.method === method);
  if (!inn) return { inn: null, out: null };
  const out = rows.find((row) => row.dir === "out" && row.msg?.id === inn.msg.id);
  return { inn, out };
}

export function assertInitialize(frames) {
  const { inn, out } = pairByMethod(frames, "initialize");
  if (!inn) return { ok: false, reason: "no initialize in-frame" };
  if (!out) return { ok: false, reason: "no initialize out-frame" };
  const requested = inn.msg?.params?.protocolVersion;
  const echoed = out.msg?.result?.protocolVersion;
  if (typeof echoed !== "string" || echoed === "") {
    return { ok: false, reason: "initialize out-frame has no protocolVersion" };
  }
  if (SUPPORTED_PROTOCOL_VERSIONS.includes(requested)) {
    if (echoed !== requested) {
      return { ok: false, reason: `protocolVersion ${echoed} != requested ${requested}` };
    }
  } else if (echoed !== LATEST_PROTOCOL) {
    return { ok: false, reason: `unsupported ${requested} expected latest ${LATEST_PROTOCOL}, got ${echoed}` };
  }
  return { ok: true, reason: `protocolVersion=${echoed}`, protocolVersion: echoed };
}

export function assertInitializedNotification(frames) {
  const found = listed(frames).some((row) => row.dir === "in" && row.msg?.method === "notifications/initialized");
  return found
    ? { ok: true, reason: "notifications/initialized" }
    : { ok: false, reason: "no notifications/initialized in-frame" };
}

export function assertToolsList(frames) {
  const { inn, out } = pairByMethod(frames, "tools/list");
  if (!inn) return { ok: false, reason: "no tools/list in-frame" };
  if (!out) return { ok: false, reason: "no tools/list out-frame" };
  const tools = out.msg?.result?.tools;
  if (!Array.isArray(tools)) return { ok: false, reason: "tools/list result.tools is not an array" };
  const names = tools.map((tool) => tool?.name);
  if (names.length !== EXPECTED_TOOLS.length || EXPECTED_TOOLS.some((name, i) => names[i] !== name)) {
    return { ok: false, reason: `tools/list names=[${names.join(",")}] expected [${EXPECTED_TOOLS.join(",")}]` };
  }
  const missingSchema = tools.filter((tool) => !tool?.inputSchema || typeof tool.inputSchema !== "object");
  if (missingSchema.length) {
    return { ok: false, reason: `tools/list missing inputSchema on ${missingSchema.map((t) => t.name).join(",")}` };
  }
  return { ok: true, reason: "tools/list search,timeline,get" };
}

export function assertToolsCallSearch(frames) {
  const rows = listed(frames);
  const inn = rows.find(
    (row) => row.dir === "in" && row.msg?.method === "tools/call" && row.msg?.params?.name === "search",
  );
  if (!inn) return { ok: false, reason: "no tools/call search in-frame" };
  const out = rows.find((row) => row.dir === "out" && row.msg?.id === inn.msg.id);
  if (!out) return { ok: false, reason: "no tools/call search out-frame" };
  const result = out.msg?.result;
  const content0 = Array.isArray(result?.content) ? result.content[0] : null;
  if (content0?.type !== "text") {
    return { ok: false, reason: `tools/call content[0].type=${content0?.type ?? "missing"}` };
  }
  if (!Array.isArray(result?.structuredContent?.memories)) {
    return { ok: false, reason: "tools/call structuredContent.memories is not an array" };
  }
  return { ok: true, reason: `search memories=${result.structuredContent.memories.length}` };
}

export function assertRepoRejected(frame) {
  const msg = messageOf(frame);
  const code = msg?.error?.code;
  if (code !== -32602) return { ok: false, reason: `repo rejection code=${code ?? "missing"}` };
  return { ok: true, reason: "-32602" };
}

export function assertGetMissing(frame) {
  const msg = messageOf(frame);
  if (msg?.result?.isError !== true) {
    return { ok: false, reason: `get missing isError=${msg?.result?.isError ?? "missing"}` };
  }
  return { ok: true, reason: "isError: true" };
}

export function assertAgentFrames(frames) {
  const checks = [
    assertInitialize(frames),
    assertInitializedNotification(frames),
    assertToolsList(frames),
    assertToolsCallSearch(frames),
  ];
  const failed = checks.filter((check) => !check.ok);
  const protocolVersion = checks[0].protocolVersion ?? readMcpFramesFrom(frames);
  return {
    ok: failed.length === 0,
    reason: failed.length ? failed.map((check) => check.reason).join("; ") : checks.map((check) => check.reason).join("; "),
    protocolVersion,
    checks,
  };
}

function readMcpFramesFrom(frames) {
  const inn = listed(frames).find((row) => row.dir === "in" && row.msg?.method === "initialize");
  return inn?.msg?.params?.protocolVersion ?? null;
}

export function buildReportRow({
  agent,
  status,
  protocolVersion = null,
  toolName = "unknown",
  frames = 0,
  reason = "",
}) {
  if (!["pass", "fail", "blocked"].includes(status)) throw new Error(`invalid status '${status}'`);
  return { agent, status, protocolVersion, toolName, frames, reason };
}

export function exitCodeFor(rows, stdioStatus = "pass") {
  if (stdioStatus === "fail") return 1;
  if ((rows || []).some((row) => row.status === "fail")) return 1;
  return 0;
}

function markdownCell(value) {
  return String(value ?? "").replaceAll(/[\\|]/g, (c) => `\\${c}`).replace(/\r?\n/g, " ");
}

export function markdownSection(report) {
  const passed = report.agents.filter((row) => row.status === "pass").length;
  const blocked = report.agents.filter((row) => row.status === "blocked").map((row) => row.agent);
  let markdown = `## ${report.started_at.slice(0, 10)} MCP clients run ${report.runId}\n\n`;
  markdown += `- ${passed} of ${report.agents.length} agents pass`;
  if (blocked.length) markdown += ` (blocked: ${blocked.join(", ")})`;
  markdown += `\n- Report: ${report.runDir}/report.json\n\n`;
  markdown += "| agent | status | protocolVersion | toolName | frames | reason |\n|---|---|---|---|---:|---|\n";
  for (const row of report.agents) {
    markdown += `| ${row.agent} | ${row.status} | ${markdownCell(row.protocolVersion ?? "n/a")} | ${markdownCell(row.toolName)} | ${row.frames} | ${markdownCell(row.reason || "none")} |\n`;
  }
  markdown += "\n";
  for (const line of report.assertions || []) markdown += `- ${line}\n`;
  markdown += "\n";
  return markdown;
}

export function writeDaily(report, cwd) {
  const destination = path.join(cwd, "docs", "evidence", "m1-dogfood.md");
  fs.mkdirSync(path.dirname(destination), { recursive: true });
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

const TOOL_NAME_TOKEN = /mcp__oboete_probe__\w+|oboete_probe__\w+|oboete_search/g;
const TOOL_NAME_EXACT = /^(?:mcp__)?oboete_probe__\w+$|^oboete_search$/;

function collectToolNames(value, acc) {
  if (typeof value === "string") {
    if (TOOL_NAME_EXACT.test(value)) acc.add(value);
    else for (const match of value.matchAll(TOOL_NAME_TOKEN)) acc.add(match[0]);
    return;
  }
  if (Array.isArray(value)) {
    for (const item of value) collectToolNames(item, acc);
    return;
  }
  if (!value || typeof value !== "object") return;
  for (const [key, item] of Object.entries(value)) {
    if (/^(tool_?name|name)$/i.test(key) && typeof item === "string" && TOOL_NAME_EXACT.test(item)) acc.add(item);
    else collectToolNames(item, acc);
  }
}

export function extractToolName({ stdout = "", stderr = "", hookLog = "", dbPayloads = [] } = {}) {
  const names = new Set();
  for (const blob of [stdout, stderr, hookLog, ...dbPayloads]) {
    if (!blob) continue;
    const parsed = parseJsonl(blob);
    const envelope = parseMaybeJson(blob);
    if (envelope) parsed.push(envelope);
    for (const obj of parsed) collectToolNames(obj, names);
    for (const match of String(blob).matchAll(TOOL_NAME_TOKEN)) names.add(match[0]);
  }
  const list = [...names].filter((name) => TOOL_NAME_EXACT.test(name) && name.length < 80);
  const preferred =
    list.find((name) => name === "mcp__oboete_probe__search") ||
    list.find((name) => name === "oboete_probe__search") ||
    list.find((name) => name === "oboete_search") ||
    list[0];
  if (preferred) return { toolName: preferred, reason: null };
  return {
    toolName: "unknown",
    reason: "hook.log records capture outcome without tool_name; agent JSON had no PreToolUse tool_name",
  };
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

export function assertPiJson(stdout) {
  const lines = parseJsonl(stdout);
  const types = [...new Set(lines.map((line) => line.type).filter(Boolean))];
  const named = (line) => line?.toolName || line?.tool_name || line?.name || "";
  const calls = lines.filter((line) => named(line) === "oboete_search" || (line.type === "tool_call" && /oboete_search/.test(JSON.stringify(line))));
  const results = lines.filter((line) => line.type === "tool_result" && /oboete_search/.test(JSON.stringify(line)));
  const texts = [];
  for (const line of results.concat(calls, lines)) {
    const content = line.content || line.message?.content || line.result;
    if (typeof content === "string") texts.push(content);
    if (Array.isArray(content)) {
      for (const block of content) if (typeof block?.text === "string") texts.push(block.text);
    }
  }
  let parsed = null;
  for (const text of texts) {
    const obj = parseMaybeJson(text);
    if (obj && Array.isArray(obj.memories)) {
      parsed = obj;
      break;
    }
  }
  if (!parsed) {
    const obj = parseMaybeJson(stdout);
    if (obj && Array.isArray(obj.memories)) parsed = obj;
  }
  if (calls.length === 0 && !parsed) {
    return {
      ok: false,
      toolName: "unknown",
      reason: `Pi JSON output does not expose tool calls or CLI --json; types=[${types.join(",") || "none"}]`,
      types,
    };
  }
  if (!parsed) {
    return {
      ok: false,
      toolName: calls.length ? "oboete_search" : "unknown",
      reason: `Pi called the tool but the result text is not CLI --json {memories:[]}; types=[${types.join(",")}]`,
      types,
    };
  }
  if (calls.length === 0) {
    return {
      ok: true,
      toolName: "oboete_search",
      reason: `Pi JSONL has no tool_call events (types=[${types.join(",") || "none"}]); CLI --json memories=${parsed.memories.length}`,
      types,
    };
  }
  return { ok: true, toolName: "oboete_search", reason: `oboete_search memories=${parsed.memories.length}`, types };
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
    for (const fd of [outFd, errFd, inFd]) {
      try {
        fs.closeSync(fd);
      } catch {
        /* already closed */
      }
    }
  }
  const proc = {
    exitCode: child?.status == null ? 1 : child.status,
    stdout: readIfPresent(stdoutPath) ?? "",
    stderr: readIfPresent(stderrPath) ?? "",
    elapsedMs: Date.now() - start,
  };
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
    const parsed = readMcpFrames(log);
    const extracted = extractToolName({
      stdout: proc.stdout,
      stderr: proc.stderr,
      hookLog: readHookLog(home),
      dbPayloads: recentDbToolNames(home, agent, started - 5_000),
    });
    if (isUnavailable(proc) && parsed.frames.length === 0) {
      const combined = `${proc.stderr}\n${proc.stdout}`;
      return blockedRow(agent, `${agent} exited ${proc.exitCode}: ${firstLine(combined) || "unavailable"}`);
    }
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
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (error instanceof PreconditionError || AGENT_OUTAGE_RE.test(message)) return blockedRow(agent, message);
    return failRow(agent, message);
  } finally {
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
    const message = error instanceof Error ? error.message : String(error);
    if (error instanceof PreconditionError || AGENT_OUTAGE_RE.test(message)) return blockedRow("pi", message);
    return failRow("pi", message);
  }
}

function frameFiles(runDir, agents) {
  const files = [`${runDir}/stdio/frames.jsonl`];
  for (const agent of agents) {
    if (agent === "pi") files.push(`${runDir}/pi/stdout.txt`);
    else files.push(`${runDir}/${agent}/frames.jsonl`);
  }
  return files;
}

function reportMarkdown(report) {
  let markdown = markdownSection(report);
  markdown += "Frame files:\n";
  for (const file of report.frame_files || []) markdown += `- ${file}\n`;
  markdown += "\n";
  return markdown;
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

  const assertions = [...stdio.assertions];
  for (const row of rows) {
    assertions.push(`${row.agent}: ${row.status} (${row.reason})`);
  }
  const finished = new Date(dependencies.now());
  const report = redactValue(
    {
      runId,
      runDir,
      started_at: started.toISOString(),
      finished_at: finished.toISOString(),
      agents: rows,
      stdio: { status: stdio.status, frames: stdio.frames, reason: stdio.reason },
      assertions,
      frame_files: frameFiles(runDir, options.agents),
      summary: `${rows.filter((row) => row.status === "pass").length} of ${rows.length} agents pass`,
      exit_code: exitCodeFor(rows, stdio.status),
    },
    runDir,
    "<run>",
  );
  fs.writeFileSync(path.join(runDir, "report.json"), `${JSON.stringify(report, null, 2)}\n`);
  fs.writeFileSync(path.join(runDir, "report.md"), reportMarkdown(report));
  if (options.daily) writeDaily(report, dependencies.cwd);
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
