import { randomBytes } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { trustedHashToml } from "./trusthash.mjs";
import { packResult } from "./agent-events.mjs";
import { childEnv, gitInit, runTimed } from "./process.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const HOOK_SRC = path.join(HERE, "hook.mjs");
const PI_EXT_SRC = path.join(HERE, "pi-extension.ts");
const HOME = os.homedir();

export const DONE_PROMPT = "Reply with exactly the word DONE.";

export const CLAUDE_EVENTS = [
  "SessionStart",
  "UserPromptSubmit",
  "PreToolUse",
  "PostToolUse",
  "PostToolUseFailure",
  "Stop",
  "PreCompact",
  "PostCompact",
  "SessionEnd",
];
export const CODEX_EVENTS = [
  "SessionStart",
  "UserPromptSubmit",
  "PreToolUse",
  "PostToolUse",
  "Stop",
  "PreCompact",
  "PostCompact",
  "SessionEnd",
];
export const GROK_EVENTS = [
  "SessionStart",
  "UserPromptSubmit",
  "PreToolUse",
  "PostToolUse",
  "PostToolUseFailure",
  "PermissionDenied",
  "Stop",
  "PreCompact",
  "PostCompact",
  "SessionEnd",
];

export const GROK_ISOLATION_ENV = {
  GROK_CLAUDE_HOOKS_ENABLED: "0",
  GROK_CLAUDE_MCPS_ENABLED: "0",
  GROK_CURSOR_HOOKS_ENABLED: "0",
  GROK_CURSOR_MCPS_ENABLED: "0",
};


const grokSeeds = new Map();


export const CLAUDE_COMPACT_PROMPT = [
  "Use the Read tool on big.txt, then Use the Read tool on big.txt again, then reply with exactly the word DONE.",
  "Do not substitute the Bash tool for Read.",
  "A whole-file Read exceeds the 256KB / 25000-token cap; you MUST pass offset and limit.",
  "Use limit 200. Do Reads at offsets 0, 200, 400, 600, 800, 1000, 1200, then the same seven offsets again (14 Reads).",
  "Do not stop after a few chunks.",
].join(" ");

export function writeCompactFixture(file) {
  const text = randomBytes(600_000).toString("base64").replace(/(.{76})/g, "$1\n");
  fs.writeFileSync(file, `${text}\n`);
  return fs.statSync(file).size;
}

export function copyMode(src, dest, mode = 0o600) {
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.copyFileSync(src, dest);
  fs.chmodSync(dest, mode);
}



function resolveRepo(dir, opts) {
  return gitInit(opts.repo || path.join(dir, "repo"));
}


export function toolUsePrompt(agent) {
  const names = {
    claude: { read: "Read", write: "Write", edit: "Edit", shell: "Bash" },
    grok: { read: "read_file", write: "write", edit: "search_replace", shell: "run_terminal_command" },
    pi: { read: "read", write: "write", edit: "edit", shell: "bash" },
  };
  if (agent === "codex") {
    return [
      "Do these five steps in order, without asking for confirmation. Each step names the exact tool to use; use that tool and no other. Do not substitute shell commands for the file tools.",
      "1. Read README.md in the current directory with a shell command.",
      "2. Create notes.txt whose entire content is the single word: alpha",
      "3. Edit notes.txt to replace alpha with beta",
      "4. Use the shell to run: echo probe-ok",
      "5. Reply with exactly the word DONE.",
    ].join(" ");
  }
  const t = names[agent];
  return [
    "Do these five steps in order, without asking for confirmation. Each step names the exact tool to use; use that tool and no other. Do not substitute shell commands for the file tools.",
    `1. Use the ${t.read} tool on README.md in the current directory.`,
    `2. Use the ${t.write} tool to create notes.txt whose entire content is the single word: alpha`,
    `3. Use the ${t.edit} tool on notes.txt to replace alpha with beta`,
    `4. Use the ${t.shell} tool to run: echo probe-ok`,
    "5. Reply with exactly the word DONE.",
  ].join(" ");
}

export function oversizedPrompt(shellName) {
  return `Use the ${shellName} tool to run exactly this command: head -c 1200000 /dev/zero | base64 ; then reply with exactly the word DONE`;
}


function hookCommand(hookPath, eventsPath, label, flags = []) {
  const extra = flags.length ? " " + flags.join(" ") : "";
  return `PROBE_EVENTS=${shellQuote(eventsPath)} node ${shellQuote(hookPath)} ${label}${extra}`;
}

export function shellQuote(s) {
  return "'" + String(s).replaceAll("'", String.raw`'\''`) + "'";
}

function normalizeSpecs(defaults, opts) {
  let list = opts.hooks ?? defaults;
  list = list.map((h) => (typeof h === "string" ? { event: h } : h));
  if (opts.hookFlags) {
    list = list.map((h) => ({
      ...h,
      flags: [...(h.flags || []), ...(opts.hookFlags[h.event] || [])],
    }));
  }
  return list;
}

function buildHookGroup(agent, event, spec, handler) {
  const group = { hooks: [handler] };
  if (spec.matcher) group.matcher = spec.matcher;
  else if (agent === "codex" && event === "SessionStart") {
    group.matcher = "startup|resume|clear|compact";
  }
  return group;
}

function buildHooksJson(agent, hookPath, eventsPath, specs) {
  const hooks = {};
  for (const spec of specs) {
    const event = spec.event;
    const label = spec.label || event;
    let timeout = 20;
    if (event === "SessionEnd") timeout = agent === "codex" ? 3 : 10;
    const handler = { type: "command", command: hookCommand(hookPath, eventsPath, label, spec.flags || []), timeout };
    if (!hooks[event]) {
      hooks[event] = [buildHookGroup(agent, event, spec, handler)];
    } else {
      hooks[event][0].hooks.push(handler);
    }
  }
  return { hooks };
}

function writeHookTree(dir, agent, opts) {
  const hookPath = path.join(dir, "hook.mjs");
  fs.copyFileSync(HOOK_SRC, hookPath);
  const eventsPath = path.join(dir, "events.jsonl");
  fs.writeFileSync(eventsPath, "");
  let defaults;
  if (agent === "claude") defaults = CLAUDE_EVENTS;
  else if (agent === "codex") defaults = CODEX_EVENTS;
  else defaults = GROK_EVENTS;
  const specs = normalizeSpecs(defaults, opts);
  const json = buildHooksJson(agent, hookPath, eventsPath, specs);
  return { hookPath, eventsPath, json };
}


export async function seedGrokHome(runRoot) {
  if (grokSeeds.has(runRoot)) return grokSeeds.get(runRoot);
  const seed = path.join(runRoot, "_grok-seed");
  fs.mkdirSync(path.join(seed, "hooks"), { recursive: true });
  copyMode(path.join(HOME, ".grok/auth.json"), path.join(seed, "auth.json"));
  await runTimed(["grok", "inspect", "--json"], {
    cwd: seed,
    env: childEnv({ GROK_HOME: seed, GROK_CLAUDE_HOOKS_ENABLED: "0" }),
    stdoutPath: path.join(seed, "inspect.json"),
    stderrPath: path.join(seed, "inspect.err"),
    timeoutMs: 60_000,
  });
  grokSeeds.set(runRoot, seed);
  return seed;
}

function copyGrokHome(src, dest, { wipeSessions = true } = {}) {
  fs.cpSync(src, dest, { recursive: true });
  fs.mkdirSync(path.join(dest, "hooks"), { recursive: true });
  for (const name of fs.readdirSync(path.join(dest, "hooks"))) {
    fs.rmSync(path.join(dest, "hooks", name), { force: true });
  }
  if (wipeSessions) fs.rmSync(path.join(dest, "sessions"), { recursive: true, force: true });
}

function deepMerge(a, b) {
  if (!b || typeof b !== "object" || Array.isArray(b)) return b;
  const out = { ...(a && typeof a === "object" && !Array.isArray(a) ? a : {}) };
  for (const [k, v] of Object.entries(b)) {
    out[k] =
      v && typeof v === "object" && !Array.isArray(v) && out[k] && typeof out[k] === "object" && !Array.isArray(out[k])
        ? deepMerge(out[k], v)
        : v;
  }
  return out;
}

export function writeClaudeSettings(dir, opts = {}) {
  fs.mkdirSync(dir, { recursive: true });
  const { eventsPath, json } = writeHookTree(dir, "claude", opts);
  const settingsPath = path.join(dir, "settings.json");
  fs.writeFileSync(settingsPath, JSON.stringify(json, null, 2));
  return { settingsPath, eventsPath };
}

export async function claude(dir, opts = {}) {
  fs.mkdirSync(dir, { recursive: true });
  const repo = resolveRepo(dir, opts);
  const { settingsPath, eventsPath } = writeClaudeSettings(dir, opts);
  const proc = await runTimed(
    [
      "claude",
      "-p",
      opts.prompt || toolUsePrompt("claude"),
      "--settings",
      settingsPath,
      "--dangerously-skip-permissions",
      "--output-format",
      "json",
      ...(opts.extraArgs || []),
    ],
    {
      cwd: repo,
      env: childEnv(opts.env),
      stdoutPath: path.join(dir, "stdout.txt"),
      stderrPath: path.join(dir, "stderr.txt"),
    },
  );
  return packResult("claude", dir, repo, settingsPath, proc, eventsPath);
}

export async function codex(dir, opts = {}) {
  fs.mkdirSync(dir, { recursive: true });
  const repo = resolveRepo(dir, opts);
  const home = path.join(dir, "codex-home");
  fs.mkdirSync(home, { recursive: true });
  copyMode(path.join(HOME, ".codex/auth.json"), path.join(home, "auth.json"));
  const { eventsPath, json } = writeHookTree(home, "codex", opts);
  const hooksPath = path.join(home, "hooks.json");
  const trust = opts.trust === true;
  if (trust) {
    const file = structuredClone(json);
    const toml = trustedHashToml(hooksPath, file);
    fs.writeFileSync(hooksPath, JSON.stringify(file, null, 2));
    fs.writeFileSync(path.join(home, "config.toml"), toml);
  } else {
    fs.writeFileSync(hooksPath, JSON.stringify(json, null, 2));
  }
  const flags = [];
  if (!trust) flags.push("--dangerously-bypass-hook-trust");
  const proc = await runTimed(
    [
      "codex",
      "exec",
      ...flags,
      "--dangerously-bypass-approvals-and-sandbox",
      "--skip-git-repo-check",
      "--json",
      "-C",
      repo,
      ...(opts.extraArgs || []),
      opts.prompt || toolUsePrompt("codex"),
    ],
    {
      cwd: repo,
      env: childEnv({ CODEX_HOME: home, ...opts.env }),
      stdoutPath: path.join(dir, "stdout.txt"),
      stderrPath: path.join(dir, "stderr.txt"),
    },
  );
  return packResult("codex", dir, repo, home, proc, eventsPath);
}

export function prepareGrokHome(dir, opts = {}) {
  fs.mkdirSync(dir, { recursive: true });
  const repo = resolveRepo(dir, opts);
  const src = opts.homeFrom || opts.grokSeed;
  if (!src) throw new Error("prepareGrokHome: grokSeed or homeFrom required");
  const home = path.join(dir, "grok-home");
  copyGrokHome(src, home, { wipeSessions: !opts.homeFrom });
  const { eventsPath, json } = writeHookTree(home, "grok", opts);
  fs.writeFileSync(path.join(home, "hooks", "probe.json"), JSON.stringify(json, null, 2));
  if (opts.configToml) {
    const cfg = path.join(home, "config.toml");
    const prev = fs.existsSync(cfg) ? fs.readFileSync(cfg, "utf8") : "";
    const extra = String(opts.configToml);
    fs.writeFileSync(cfg, prev + (prev && !prev.endsWith("\n") ? "\n" : "") + extra + (extra.endsWith("\n") ? "" : "\n"));
  }
  return { home, repo, eventsPath };
}

export async function grok(dir, opts = {}) {
  const grokSeed = opts.homeFrom ? opts.grokSeed : opts.grokSeed || (await seedGrokHome(path.dirname(dir)));
  const { home, repo, eventsPath } = prepareGrokHome(dir, { ...opts, grokSeed });
  const argv = ["grok", "-p", opts.prompt || toolUsePrompt("grok")];
  if (!opts.noApprove) argv.push("--always-approve");
  argv.push("--output-format", "json", "--cwd", repo, ...(opts.extraArgs || []));
  const proc = await runTimed(argv, {
    cwd: repo,
    env: childEnv({ GROK_HOME: home, ...GROK_ISOLATION_ENV, ...opts.env }),
    stdoutPath: path.join(dir, "stdout.txt"),
    stderrPath: path.join(dir, "stderr.txt"),
    timeoutMs: opts.timeoutMs,
  });
  return packResult("grok", dir, repo, home, proc, eventsPath);
}

export async function pi(dir, opts = {}) {
  fs.mkdirSync(dir, { recursive: true });
  const repo = resolveRepo(dir, opts);
  const tmp = path.join(dir, "piagent");
  const sessions = path.join(tmp, "sessions");
  fs.mkdirSync(path.join(tmp, "extensions"), { recursive: true });
  fs.mkdirSync(sessions, { recursive: true });
  const agentDir = path.join(HOME, ".pi/agent");
  copyMode(path.join(agentDir, "auth.json"), path.join(tmp, "auth.json"));
  for (const name of ["settings.json", "models-store.json"]) {
    const src = path.join(agentDir, name);
    if (fs.existsSync(src)) fs.copyFileSync(src, path.join(tmp, name));
  }
  if (opts.settings) {
    const p = path.join(tmp, "settings.json");
    const cur = fs.existsSync(p) ? JSON.parse(fs.readFileSync(p, "utf8")) : {};
    fs.writeFileSync(p, JSON.stringify(deepMerge(cur, opts.settings), null, 2));
  }
  fs.copyFileSync(PI_EXT_SRC, path.join(tmp, "extensions", "oboete-probe.ts"));
  const eventsPath = path.join(dir, "events.jsonl");
  fs.writeFileSync(eventsPath, "");
  const env = childEnv({
    PI_CODING_AGENT_DIR: tmp,
    PROBE_EVENTS: eventsPath,
    ...(opts.marker ? { PROBE_MARKER: opts.marker } : {}),
    ...opts.env,
  });
  const proc = await runTimed(
    ["pi", "-p", opts.prompt || toolUsePrompt("pi"), "--mode", "json", "--session-dir", sessions, ...(opts.extraArgs || [])],
    {
      cwd: repo,
      env,
      stdoutPath: path.join(dir, "stdout.txt"),
      stderrPath: path.join(dir, "stderr.txt"),
    },
  );
  return packResult("pi", dir, repo, tmp, proc, eventsPath);
}
