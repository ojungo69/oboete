import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

const SESSION_MS = 240_000;
const HOME = os.homedir();

export class PreconditionError extends Error {
  constructor(message) {
    super(message);
    this.name = "PreconditionError";
  }
}

// An API error alone may be a bad request; only availability/auth signatures block a run.
export const AGENT_OUTAGE_RE = /529 Overloaded|overloaded_error|rate.?limit|quota (?:exhausted|exceeded)|HTTP (?:402|429)\b|stream (?:disconnected|error)|ECONNRESET|ETIMEDOUT|fetch failed|authentication required|please .*login/i;

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

export function binVersion(bin) {
  try {
    const r = spawnSync(bin, ["--version"], { encoding: "utf8", env: childEnv(), timeout: 15_000 });
    const t = ((r.stdout || "") + (r.stderr || "")).trim();
    return t.split("\n")[0] || "unknown";
  } catch {
    return "unknown";
  }
}
