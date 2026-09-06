#!/usr/bin/env node
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import * as agents from "./probe-lib/agents.mjs";
import { tmux } from "./probe-lib/tmux.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PROBE_MS = 20 * 60 * 1000;
const HOME = os.homedir();

function usage(code = 2) {
  console.error(`Usage:
  node scripts/e2e/probe-contracts.mjs --list
  node scripts/e2e/probe-contracts.mjs --run <id>[,<id>...] [--report]
  node scripts/e2e/probe-contracts.mjs --agent <claude|codex|grok|pi|providers> [--report]
  node scripts/e2e/probe-contracts.mjs --all [--report]`);
  process.exit(code);
}

async function loadProbes() {
  const dir = path.join(HERE, "probes");
  const files = fs.readdirSync(dir).filter((f) => f.endsWith(".mjs")).sort();
  const out = [];
  for (const f of files) {
    const mod = await import(pathToFileURL(path.join(dir, f)).href);
    if (!Array.isArray(mod.probes)) throw new Error(f + " missing probes[]");
    out.push(...mod.probes);
  }
  return out;
}

function parseArgs(argv) {
  const opts = { list: false, report: false, run: null, agent: null, all: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--list") opts.list = true;
    else if (a === "--report") opts.report = true;
    else if (a === "--all") opts.all = true;
    else if (a === "--run") opts.run = argv[++i];
    else if (a === "--agent") opts.agent = argv[++i];
    else if (a === "--help" || a === "-h") usage(0);
    else usage(2);
  }
  return opts;
}

function runIdNow() {
  return new Date().toISOString().replace(/[:.]/g, "-");
}

function envBlock() {
  return {
    date: new Date().toISOString(),
    claude: agents.binVersion("claude"),
    codex: agents.binVersion("codex"),
    grok: agents.binVersion("grok"),
    pi: agents.binVersion("pi"),
    node: agents.binVersion("node"),
  };
}

function pipe(s) {
  // The backslash goes first: escaping only the bar turns an evidence string that already holds
  // `\\|` into `\\\\|`, which renders as a literal backslash followed by a live column separator.
  return String(s || "")
    .replace(/\\/g, "\\\\")
    .replace(/\|/g, "\\|")
    .replace(/\s+/g, " ")
    .trim();
}

function markdownSection(runId, env, results) {
  const date = env.date.slice(0, 10);
  let md = `## ${date} run ${runId}\n\n`;
  md += "| tool | version |\n|---|---|\n";
  for (const k of ["date", "claude", "codex", "grok", "pi", "node"]) {
    md += `| ${k} | ${pipe(env[k])} |\n`;
  }
  md += "\n| id | R13 row | agent | status |\n|---|---|---|---|\n";
  for (const r of results) {
    md += `| ${r.id} | ${pipe(r.row)} | ${r.agent} | ${r.status} |\n`;
  }
  md += "\n";
  for (const r of results) {
    const ev = (r.evidence || []).map((s) => pipe(agents.redactValue(String(s), null))).join("; ");
    md += `- **${r.id}**: ${ev}\n`;
  }
  md += "\n";
  return md;
}

function writeReports(repoRoot, runRoot, runId, env, results) {
  const section = markdownSection(runId, env, results);
  fs.writeFileSync(path.join(runRoot, "report.md"), section);
  const dest = path.join(repoRoot, "docs/research/oboete-contracts-probes.md");
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  let prefix = "";
  if (!fs.existsSync(dest)) {
    prefix =
      "# oboete contract probes\n\nVerification-gate (R13) runs under the isolated dogfood user. Statuses: pass / fail / blocked / skipped.\n\n";
  }
  fs.appendFileSync(dest, prefix + section);
}

function withTimeout(promise, ms, label) {
  let t;
  const timeout = new Promise((_, rej) => {
    t = setTimeout(() => rej(new Error("probe timeout " + label)), ms);
  });
  return Promise.race([promise, timeout]).finally(() => clearTimeout(t));
}

export function blockAgentApiFailure(dir, result) {
  if (!result || !["fail", "blocked"].includes(result.status)) return result;
  let entries;
  try {
    entries = fs.readdirSync(dir, { recursive: true, withFileTypes: true });
  } catch {
    return result;
  }
  for (const entry of entries) {
    if (!entry.isFile() || !/^(?:stdout|stderr).*\.txt$/i.test(entry.name)) continue;
    const file = path.join(entry.parentPath || entry.path || dir, entry.name);
    let text;
    let fd;
    try {
      fd = fs.openSync(file, "r");
      if (fs.fstatSync(fd).size > 5 * 1024 * 1024) continue;
      text = fs.readFileSync(fd, "utf8");
    } catch {
      continue;
    } finally {
      if (fd !== undefined) fs.closeSync(fd);
    }
    const line = text.split(/\r?\n/).find((value) => agents.AGENT_OUTAGE_RE.test(value));
    if (!line) continue;
    result.status = "blocked";
    result.evidence = [...(result.evidence || []), `agent API error: ${line.trim().slice(0, 200)} (${path.relative(dir, file)})`];
    return result;
  }
  return result;
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const all = await loadProbes();
  if (opts.list) {
    for (const p of all) console.log(`${p.id}\t${p.agent}\t${p.row}`);
    return;
  }
  let selected = all;
  if (opts.run) {
    const ids = opts.run.split(",").map((s) => s.trim()).filter(Boolean);
    selected = ids.map((id) => {
      const p = all.find((x) => x.id === id);
      if (!p) throw new Error("unknown probe id: " + id);
      return p;
    });
  } else if (opts.agent) {
    selected = all.filter((p) => p.agent === opts.agent);
    if (!selected.length) throw new Error("no probes for agent " + opts.agent);
  } else if (!opts.all) {
    usage(2);
  }

  const repoRoot = path.resolve(HERE, "../..");
  const runId = runIdNow();
  const runRoot = path.join(HOME, ".cache/oboete-probes", runId);
  fs.mkdirSync(runRoot, { recursive: true });
  const env = envBlock();
  const needGrok = selected.some((p) => p.agent === "grok" || p.id === "agent-cli-json");
  let grokSeed = null;
  let grokSeedError = null;
  if (needGrok) {
    try {
      grokSeed = await agents.seedGrokHome(runRoot);
    } catch (e) {
      grokSeedError = String(e && e.message ? e.message : e);
    }
  }

  const results = [];
  for (const probe of selected) {
    const dir = path.join(runRoot, probe.id);
    fs.mkdirSync(dir, { recursive: true });
    const ctx = {
      dir,
      runRoot,
      runId,
      repoRoot,
      grokSeed,
      grokSeedError,
      log: (...a) => console.error(`[${probe.id}]`, ...a),
      claude: agents.claude,
      codex: agents.codex,
      grok: agents.grok,
      pi: agents.pi,
      seedGrokHome: agents.seedGrokHome,
    };
    ctx.log("start");
    const started = Date.now();
    let result;
    const needsGrok = probe.agent === "grok" || probe.id === "agent-cli-json";
    if (needsGrok && grokSeedError) {
      result = { status: "blocked", evidence: ["grok seed failed: " + grokSeedError] };
    } else {
      try {
        result = await withTimeout(probe.run(ctx), PROBE_MS, probe.id);
        if (!result || !["pass", "fail", "blocked", "skipped"].includes(result.status)) {
          result = { status: "fail", evidence: ["invalid result status"], data: result };
        }
      } catch (e) {
        const evidence = [String(e && e.stack ? e.stack : e)];
        if (/probe timeout /.test(String(e && e.message ? e.message : e))) {
          tmux(["kill-server"]);
          evidence.push("tmux server oboete-probes killed");
        }
        result = { status: "fail", evidence };
      }
    }
    blockAgentApiFailure(dir, result);
    result.id = probe.id;
    result.agent = probe.agent;
    result.row = probe.row;
    result.elapsed_ms = Date.now() - started;
    result.evidence = result.evidence || [];
    results.push(result);
    ctx.log(result.status, (result.evidence || []).join("; ").slice(0, 200));
    fs.writeFileSync(
      path.join(runRoot, "report.json"),
      JSON.stringify(agents.redactValue({ env, runId, results }, null), null, 2) + "\n",
    );
  }

  if (opts.report) writeReports(repoRoot, runRoot, runId, env, results);
  const failed = results.some((r) => r.status === "fail");
  process.exit(failed ? 1 : 0);
}

if (path.resolve(process.argv[1] || "") === fileURLToPath(import.meta.url)) {
  main().catch((e) => {
    console.error(e);
    process.exit(1);
  });
}
