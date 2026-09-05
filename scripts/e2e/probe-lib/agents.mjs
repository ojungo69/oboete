import { spawn, spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { trustedHashToml } from "./trusthash.mjs";

const SESSION_MS = 240_000;
const HERE = path.dirname(fileURLToPath(import.meta.url));
const HOOK_SRC = path.join(HERE, "hook.mjs");
const PI_EXT_SRC = path.join(HERE, "pi-extension.ts");
const HOME = os.homedir();
const USERNAME = os.userInfo().username;
function reEscape(s) {
  return String(s).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
const HOME_RE = new RegExp(`(/home/|%2[Ff]home%2[Ff]|-home-)${reEscape(USERNAME)}-?`, "g");
const RUN_ID_RE = String.raw`\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}Z`;
const RUN_RE = new RegExp(
  String.raw`~(?:/\.cache/oboete-probes/|%2[Ff]\.cache%2[Ff]oboete-probes%2[Ff]|-cache-oboete-probes-)${RUN_ID_RE}`,
  "g",
);
const COMPACTION_KEY_EXACT =
  /^(compaction_?id|compact_?id|id|counter|seq(uence)?|ordinal|epoch|generation|timestamp|count)$/i;
const COMPACTION_KEY_SUFFIX = /(_id|Id)$/;

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

export const SUMMARY_KEYS = [
  "compact_summary",
  "compactSummary",
  "compaction_summary",
  "summary",
  "summary_text",
  "summaryText",
  "compactedSummary",
  "text",
];

const COMPACTION_EXCLUDE = new Set([
  "session_id",
  "sessionId",
  "transcript_path",
  "transcriptPath",
  "cwd",
  "hook_event_name",
  "hookEventName",
  "permission_mode",
  "permissionMode",
  "workspaceRoot",
  "prompt_id",
  "turn_id",
  "model",
  "trigger",
  "matcher",
  "source",
  "compact_summary",
  "compaction_summary",
  "summary",
]);

const grokSeeds = new Map();

export class PreconditionError extends Error {
  constructor(message) {
    super(message);
    this.name = "PreconditionError";
  }
}

// An API error alone may be a bad request; only availability/auth signatures block a run.
export const AGENT_OUTAGE_RE = /529 Overloaded|overloaded_error|rate.?limit|quota (?:exhausted|exceeded)|HTTP (?:402|429)\b|stream (?:disconnected|error)|ECONNRESET|ETIMEDOUT|fetch failed|authentication required|please .*login/i;

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

export function agentPath(env = process.env) {
  return [
    path.join(HOME, ".local/bin"),
    path.join(HOME, ".npm-global/bin"),
    env.PATH || "/usr/bin:/bin",
  ].join(path.delimiter);
}

/**
 * The oboete credential variables of contracts/cli.md. The engine states the same rule in
 * src/log.ts `isCredentialVariable`; this is the harness's one copy of it, not a fourth.
 */
export function isCredentialVariable(name) {
  if (name === "OBOETE_CF_ACCOUNT_ID") return true;
  return name.startsWith("OBOETE_") && (name.endsWith("_API_KEY") || name.endsWith("_API_TOKEN"));
}

/**
 * The environment a probe hands to a child. FR-016: a probe runs from the developer's shell, and
 * oboete's provider credentials are the developer's, so a child that is an agent CLI never receives
 * them. Only a caller that runs `oboete` itself asks for them with `{ credentials: true }`. A pane
 * takes its environment from the tmux server rather than from its caller, so `probe-lib/tmux.mjs`
 * applies the same rule where that server is started.
 */
export function childEnv(extra = {}, { credentials = false } = {}) {
  const env = { ...process.env, PATH: agentPath(), ...extra };
  if (credentials) return env;
  for (const name of Object.keys(env)) if (isCredentialVariable(name)) delete env[name];
  return env;
}

export function copyMode(src, dest, mode = 0o600) {
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.copyFileSync(src, dest);
  fs.chmodSync(dest, mode);
}

export function parseEvents(file) {
  if (!fs.existsSync(file)) return [];
  return fs
    .readFileSync(file, "utf8")
    .split("\n")
    .filter(Boolean)
    .map((l) => {
      try {
        return JSON.parse(l);
      } catch {
        return { event: "parse_error", raw: l.slice(0, 200) };
      }
    });
}

export function gitInit(repo) {
  fs.mkdirSync(repo, { recursive: true });
  const readme = path.join(repo, "README.md");
  if (!fs.existsSync(readme)) {
    fs.writeFileSync(readme, "oboete probe repository\nsecond line\n");
  }
  if (fs.existsSync(path.join(repo, ".git"))) return repo;
  const run = (args) =>
    spawnSync("git", args, { cwd: repo, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  let r = run(["init", "-q"]);
  if (r.status !== 0) throw new Error("git init: " + (r.stderr || r.stdout));
  run(["-c", "user.email=probe@example.invalid", "-c", "user.name=probe", "add", "README.md"]);
  r = run(["-c", "user.email=probe@example.invalid", "-c", "user.name=probe", "commit", "-qm", "init"]);
  if (r.status !== 0) throw new Error("git commit: " + (r.stderr || r.stdout));
  return repo;
}

function resolveRepo(dir, opts) {
  return gitInit(opts.repo || path.join(dir, "repo"));
}

export function runTimed(argv, { cwd, env, stdoutPath, stderrPath, timeoutMs = SESSION_MS } = {}) {
  fs.mkdirSync(path.dirname(stdoutPath), { recursive: true });
  const start = Date.now();
  return new Promise((resolve) => {
    const outFd = fs.openSync(stdoutPath, "w");
    const errFd = fs.openSync(stderrPath, "w");
    const secs = Math.max(1, Math.ceil(timeoutMs / 1000));
    const child = spawn("timeout", ["--signal=TERM", String(secs), ...argv], {
      cwd,
      env: env || childEnv(),
      stdio: ["ignore", outFd, errFd],
    });
    const done = (code, signal) => {
      try {
        fs.closeSync(outFd);
      } catch {
        /* already closed */
      }
      try {
        fs.closeSync(errFd);
      } catch {
        /* already closed */
      }
      resolve({
        exitCode: code == null ? 124 : code,
        signal: signal || null,
        elapsedMs: Date.now() - start,
        stdout: fs.existsSync(stdoutPath) ? fs.readFileSync(stdoutPath, "utf8") : "",
        stderr: fs.existsSync(stderrPath) ? fs.readFileSync(stderrPath, "utf8") : "",
      });
    };
    child.on("close", done);
    child.on("error", (err) => done(1, err.message));
  });
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

export function redactValue(value, repoPath, label = "<repo>") {
  const walk = (v) => {
    if (typeof v === "string") {
      let s = v;
      if (repoPath) s = s.split(repoPath).join(label);
      return s.replace(HOME_RE, "~").replace(RUN_RE, "<run>");
    }
    if (Array.isArray(v)) return v.map(walk);
    if (v && typeof v === "object") {
      const o = {};
      for (const [k, val] of Object.entries(v)) o[k] = walk(val);
      return o;
    }
    return v;
  };
  return walk(value);
}

export function toolUseIdOf(ev) {
  const s = ev?.stdin || {};
  return s.tool_use_id || s.toolUseId || s.toolCallId || null;
}

export function pairFor(events, name, preEvent = "PreToolUse", postEvent = "PostToolUse") {
  const pres = events.filter((e) => e.event === preEvent && toolNameOf(e) === name);
  const posts = events.filter((e) => e.event === postEvent && toolNameOf(e) === name);
  for (const pre of pres) {
    const id = toolUseIdOf(pre);
    const post = id ? posts.find((p) => toolUseIdOf(p) === id) : posts[0];
    if (post) return { pre, post };
  }
  return { pre: pres[0] || null, post: posts[0] || null };
}

export function keyDiff(obs, exp) {
  const missing = (exp || []).filter((k) => !obs.includes(k));
  const extra = obs.filter((k) => !(exp || []).includes(k));
  const bits = [];
  if (missing.length) bits.push("missing " + missing.join(","));
  if (extra.length) bits.push("extra " + extra.join(","));
  return bits.length ? bits.join("; ") : "match recon";
}

export function toolNameOf(ev) {
  const s = ev?.stdin || {};
  return s.toolName || s.tool_name || s.tool || null;
}

export function toolInputOf(ev) {
  const s = ev?.stdin || {};
  return s.toolInput || s.tool_input || s.input || null;
}

export function toolOutputOf(ev) {
  const s = ev?.stdin || {};
  if ("toolResult" in s) return s.toolResult;
  if ("tool_response" in s) return s.tool_response;
  if ("content" in s) return s.content;
  return undefined;
}

function hookCommand(hookPath, eventsPath, label, flags = []) {
  const extra = flags.length ? " " + flags.join(" ") : "";
  return `PROBE_EVENTS=${shellQuote(eventsPath)} node ${shellQuote(hookPath)} ${label}${extra}`;
}

export function shellQuote(s) {
  return "'" + String(s).replace(/'/g, `'\\''`) + "'";
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

function buildHooksJson(agent, hookPath, eventsPath, specs) {
  const hooks = {};
  for (const spec of specs) {
    const event = spec.event;
    const label = spec.label || event;
    const timeout = event === "SessionEnd" ? (agent === "codex" ? 3 : 10) : 20;
    const handler = { type: "command", command: hookCommand(hookPath, eventsPath, label, spec.flags || []), timeout };
    if (!hooks[event]) {
      const group = { hooks: [handler] };
      if (spec.matcher) group.matcher = spec.matcher;
      else if (agent === "codex" && event === "SessionStart") {
        group.matcher = "startup|resume|clear|compact";
      }
      hooks[event] = [group];
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
  const defaults = agent === "claude" ? CLAUDE_EVENTS : agent === "codex" ? CODEX_EVENTS : GROK_EVENTS;
  const specs = normalizeSpecs(defaults, opts);
  const json = buildHooksJson(agent, hookPath, eventsPath, specs);
  return { hookPath, eventsPath, json };
}

export function parseMaybeJson(text) {
  const t = (text || "").trim();
  if (!t) return null;
  try {
    return JSON.parse(t);
  } catch {
    /* fall through */
  }
  const start = t.indexOf("{");
  const end = t.lastIndexOf("}");
  if (start >= 0 && end > start) {
    try {
      return JSON.parse(t.slice(start, end + 1));
    } catch {
      return null;
    }
  }
  return null;
}

export function parseJsonl(text) {
  return (text || "")
    .split("\n")
    .filter((l) => l.trim().startsWith("{"))
    .map((l) => {
      try {
        return JSON.parse(l);
      } catch {
        return null;
      }
    })
    .filter(Boolean);
}

function envelopeMeta(agent, proc, events, envelope) {
  let sessionId = null;
  let model = null;
  if (agent === "claude") {
    const env = envelope;
    sessionId = env?.session_id || null;
    const mu = env?.modelUsage || {};
    let best = -1;
    for (const [id, u] of Object.entries(mu)) {
      const n = (u && (u.output_tokens ?? u.outputTokens)) || 0;
      if (n > best) {
        best = n;
        model = id;
      }
    }
  } else if (agent === "codex") {
    for (const line of parseJsonl(proc.stdout)) {
      if (line.type === "thread.started") sessionId = line.thread_id || line.threadId || sessionId;
    }
    for (const ev of events) {
      const s = ev.stdin || {};
      if (s.model) model = s.model;
      if (s.session_id) sessionId = sessionId || s.session_id;
    }
  } else if (agent === "grok") {
    const env = envelope;
    sessionId = env?.sessionId || env?.session_id || null;
    const mu = env?.modelUsage || {};
    model = Object.keys(mu)[0] || null;
    for (const ev of events) {
      const s = ev.stdin || {};
      sessionId = sessionId || s.sessionId || s.session_id;
    }
  } else if (agent === "pi") {
    const lines = parseJsonl(proc.stdout);
    for (let i = lines.length - 1; i >= 0; i--) {
      if (lines[i].type === "turn_end" && lines[i].message?.model) {
        model = lines[i].message.model;
        break;
      }
    }
    for (const ev of events) {
      if (ev.sessionId) sessionId = ev.sessionId;
    }
  }
  if (!sessionId) {
    for (const ev of events) {
      const s = ev.stdin || {};
      sessionId = s.session_id || s.sessionId || sessionId;
    }
  }
  return { sessionId, model };
}

export function piContentText(content) {
  if (!Array.isArray(content)) return typeof content === "string" ? content : "";
  return content
    .filter((b) => b && b.type === "text")
    .map((b) => b.text || "")
    .join("");
}

export function finalText(agent, proc, events) {
  if (agent === "claude") {
    const env = parseMaybeJson(proc.stdout);
    if (typeof env?.result === "string") return env.result;
  }
  if (agent === "codex") {
    for (const line of parseJsonl(proc.stdout).reverse()) {
      if (line.type === "item.completed" && line.item?.text) return line.item.text;
    }
  }
  if (agent === "grok") {
    for (const ev of events) {
      if (ev.event === "Stop") {
        const s = ev.stdin || {};
        if (s.reason === "end_turn" && s.lastAssistantMessage) return s.lastAssistantMessage;
      }
    }
    const env = parseMaybeJson(proc.stdout);
    if (typeof env?.text === "string") return env.text;
  }
  if (agent === "pi") {
    const lines = parseJsonl(proc.stdout);
    for (let i = lines.length - 1; i >= 0; i--) {
      const l = lines[i];
      if (l.type === "turn_end" || l.type === "message_end") {
        const t = piContentText(l.message?.content);
        if (t) return t;
      }
    }
    return proc.stdout.trim();
  }
  for (const ev of events) {
    if (ev.event === "Stop") {
      const s = ev.stdin || {};
      return s.last_assistant_message || s.lastAssistantMessage || "";
    }
  }
  return proc.stdout.slice(-2000);
}

function packResult(agent, dir, repo, tree, proc, eventsPath) {
  const events = parseEvents(eventsPath);
  const envelope = agent === "claude" || agent === "grok" ? parseMaybeJson(proc.stdout) : null;
  const { sessionId, model } = envelopeMeta(agent, proc, events, envelope);
  return {
    agent,
    exitCode: proc.exitCode,
    stdout: proc.stdout,
    stderr: proc.stderr,
    events,
    elapsedMs: proc.elapsedMs,
    sessionId,
    model,
    envelope,
    tree,
    repo,
    dir,
  };
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
      env: childEnv({ CODEX_HOME: home, ...(opts.env || {}) }),
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
    env: childEnv({ GROK_HOME: home, ...GROK_ISOLATION_ENV, ...(opts.env || {}) }),
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
    ...(opts.env || {}),
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

export function topKeys(v) {
  if (v == null) return [];
  if (typeof v === "string") return ["(string)"];
  if (Array.isArray(v)) return ["(array)"];
  if (typeof v === "object") return Object.keys(v);
  return [typeof v];
}

export function named(events, name) {
  return (events || []).filter((e) => e.event === name);
}

export async function waitUntil(fn, ms, stepMs = 250, { now = Date.now, sleep: pause = sleep } = {}) {
  const deadline = now() + ms;
  let last;
  while (now() < deadline) {
    last = await fn();
    if (last) return last;
    const remaining = deadline - now();
    if (remaining > 0) await pause(Math.min(stepMs, remaining));
  }
  return last;
}

export function summaryOf(stdin) {
  if (!stdin || typeof stdin !== "object") return { field: null, length: 0 };
  for (const k of SUMMARY_KEYS) {
    if (!(k in stdin) || stdin[k] == null || stdin[k] === "") continue;
    const v = stdin[k];
    return { field: k, length: typeof v === "string" ? v.length : JSON.stringify(v).length };
  }
  return { field: null, length: 0 };
}

function flattenOneLevel(stdin) {
  const s = stdin && typeof stdin === "object" && !Array.isArray(stdin) ? stdin : {};
  const out = { ...s };
  for (const v of Object.values(s)) {
    if (!v || typeof v !== "object" || Array.isArray(v)) continue;
    for (const [ik, iv] of Object.entries(v)) {
      if (!(ik in out)) out[ik] = iv;
    }
  }
  return out;
}

export function compactionIdentity(posts) {
  const payloads = (posts || []).map((e) => flattenOneLevel(e?.stdin));
  const n = payloads.length;
  const keySet = new Set();
  for (const p of payloads) {
    for (const k of Object.keys(p)) {
      if (COMPACTION_EXCLUDE.has(k)) continue;
      if (COMPACTION_KEY_EXACT.test(k) || COMPACTION_KEY_SUFFIX.test(k)) keySet.add(k);
    }
  }
  const candidates = [...keySet];
  const values = payloads.map((p) => Object.fromEntries(candidates.map((k) => [k, p[k]])));
  if (n === 1) {
    return { ok: false, candidates, values, n, note: `single observation; candidate keys = [${candidates.join(", ")}]` };
  }
  if (n < 2) return { ok: false, candidates, values, n, note: "no observations" };
  const sigs = values.map((v) => JSON.stringify(v));
  const unique = new Set(sigs).size === n;
  return {
    ok: unique,
    candidates,
    values,
    n,
    note: unique ? undefined : candidates.length ? "no distinguishing candidate" : "no candidate",
  };
}

export function grepLines(text, patterns) {
  const re = new RegExp(patterns.map((p) => p.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|"), "i");
  return (text || "")
    .split("\n")
    .filter((l) => re.test(l))
    .slice(0, 30);
}

export function oversizedOutcome(r, dir) {
  const text = finalText(r.agent, r, r.events);
  const done = /\bDONE\b/.test(text);
  const stop = r.events.some((e) => e.event === "Stop");
  const end = r.events.some((e) => e.event === "SessionEnd");
  const unread = r.events.filter((e) => e.event === "PostToolUse-noread");
  const read = r.events.filter((e) => e.event === "PostToolUse");
  const sizes = read.map((e) => e.stdinBytes ?? JSON.stringify(e.stdin || {}).length);
  const hookHits = grepLines(r.stderr, [dir, "hook.mjs", "hook", "EPIPE", "SIGPIPE", "failed"]);
  const failedHook = /hook.*fail|EPIPE|SIGPIPE/i.test(r.stderr || "");
  return {
    status: done && stop && end && !failedHook ? "pass" : "fail",
    evidence: [
      `exit=${r.exitCode}`,
      `DONE=${done}`,
      `Stop=${stop}`,
      `SessionEnd=${end}`,
      `unread_handlers=${unread.length}`,
      `read_PostToolUse=${read.length} sizes=${sizes.join(",") || "none"}`,
      `hook_lines=${hookHits.length ? hookHits.slice(0, 5).join(" | ") : "none"}`,
      `elapsed_s=${(r.elapsedMs / 1000).toFixed(1)}`,
    ],
    data: { sizes, hookHits },
  };
}

export function stripFences(text) {
  const t = String(text || "").trim();
  const m = t.match(/^```(?:json)?\s*\n?([\s\S]*?)\n?```$/);
  return (m ? m[1] : t).trim();
}

export function parseObservationsJson(text) {
  const stripped = stripFences(text);
  let obj;
  try {
    obj = JSON.parse(stripped);
  } catch {
    const start = stripped.indexOf("{");
    const end = stripped.lastIndexOf("}");
    if (start < 0 || end <= start) return null;
    try {
      obj = JSON.parse(stripped.slice(start, end + 1));
    } catch {
      return null;
    }
  }
  if (!obj || typeof obj !== "object") return null;
  if (!Array.isArray(obj.observations) || typeof obj.summary !== "string") return null;
  return obj;
}

export function writeFixture(repoRoot, rel, obj) {
  const dest = path.join(repoRoot, rel);
  const body = JSON.stringify(redactValue(obj, null), null, 2) + "\n";
  if (USERNAME && body.includes(USERNAME)) throw new Error("unredacted path in fixture");
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.writeFileSync(dest, body);
  return dest;
}

export function shapeProbe({ agent, expected, launch, preEvent = "PreToolUse", postEvent = "PostToolUse", fixtureDir }) {
  return async (ctx) => {
    const r = await launch(ctx);
    const evidence = [];
    const missing = [];
    const version = binVersion(agent);
    const captured_at = new Date().toISOString();
    for (const [native, exp] of Object.entries(expected)) {
      const { pre, post } = pairFor(r.events, native, preEvent, postEvent);
      if (!pre || !post) {
        missing.push(native);
        evidence.push(`${native}: missing ${!pre ? preEvent : ""}${!post ? postEvent : ""}`);
        continue;
      }
      const inKeys = topKeys(toolInputOf(pre) || toolInputOf(post));
      let outKeys = topKeys(toolOutputOf(post));
      if (post.stdin && "details" in post.stdin && !outKeys.includes("details")) outKeys = [...outKeys, "details"];
      const diff = exp.output
        ? `${keyDiff(inKeys, exp.input)}; out ${keyDiff(outKeys, exp.output)}`
        : keyDiff(inKeys, exp.input);
      evidence.push(`${native} input=[${inKeys.join(",")}] output=[${outKeys.join(",")}] path=${exp.path} (${diff})`);
      const events = {
        [preEvent]: redactValue(pre.stdin, r.repo),
        [postEvent]: redactValue(post.stdin, r.repo),
      };
      writeFixture(ctx.repoRoot, `test/contracts/${fixtureDir}/${exp.file}`, {
        agent,
        agent_version: version,
        captured_at,
        native_tool: native,
        normalized_tool: exp.normalized,
        events,
        notes: exp.path,
      });
    }
    evidence.push(
      `exit=${r.exitCode} elapsed_s=${(r.elapsedMs / 1000).toFixed(1)} session=${r.sessionId || "none"} model=${r.model || "none"}`,
    );
    return { status: missing.length ? "fail" : "pass", evidence, data: { missing, sessionId: r.sessionId, model: r.model } };
  };
}

export function binVersion(bin) {
  try {
    const r = spawnSync(bin, ["--version"], { encoding: "utf8", env: childEnv(), timeout: 15_000 });
    const t = ((r.stdout || "") + (r.stderr || "")).trim();
    return t.split("\n")[0] || "unknown";
  } catch {
    return "unknown";
  }
}
