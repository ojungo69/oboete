import { spawnSync } from "node:child_process";
import { setTimeout as sleep } from "node:timers/promises";

import { PreconditionError, isCredentialVariable, waitUntil } from "./agents.mjs";

export const TMUX_SOCKET = "oboete-probes";

export const TRUST_PANE_RE = /hook needs review|review required|Trust to trust|New hook|untrusted hook/i;
const CLAUDE_LOGIN = /Select login method|Paste code here if prompted|Browser didn't open|oauth\/authorize/i;
// Two Claude dialogs default to "No, exit": folder trust and the bypass-permissions acceptance.
const CLAUDE_TRUST = /Do you trust|Yes, I trust|Bypass Permissions mode|Yes, I accept/i;
const CLAUDE_ONBOARDING = /Choose the text style|Let's get started|looks best with your terminal|Press enter to continue/i;
const CLAUDE_READY = /\/help|\/compact|\/clear|\/exit|Ask Claude|Write a message|Type a (task|message)|shift\+tab/i;

export function tuiCmd(extra = []) {
  return [
    "codex", "--sandbox", "danger-full-access", "--ask-for-approval", "never",
    "-c", "tui.animations=false", "-c", "tui.disable_paste_burst=true", "-c", "mcp_servers={}",
    ...extra,
  ];
}

export async function readyTui(agent, tui, { timeoutMs = 90_000 } = {}, dependencies = {}) {
  const pause = dependencies.sleep ?? sleep;
  const now = dependencies.now ?? Date.now;
  const deadline = now() + timeoutMs;
  let enters = 0;
  const ready = await waitUntil(async () => {
    const pane = tui.capture();
    if (agent === "codex") {
      if (TRUST_PANE_RE.test(pane)) throw new Error(`Codex hook trust/approval regression (FR-031): ${pane}`);
      // Directory trust for a repository the config has not recorded: "1. Yes, continue" is the
      // default, so Enter accepts it (Escape would quit).
      if (/Do you trust the contents of this directory/.test(pane)) {
        tui.send("");
        await pause(Math.min(1_000, Math.max(0, deadline - now())));
        return null;
      }
      return /› Ask Codex/.test(pane) ? pane : null;
    }
    if (CLAUDE_LOGIN.test(pane)) throw new PreconditionError(`Claude TUI login required: ${pane.slice(-400)}.`);
    if (CLAUDE_TRUST.test(pane) || CLAUDE_ONBOARDING.test(pane)) {
      if (enters < 14) {
        // send appends Enter: both CLAUDE_TRUST dialogs default to "No, exit".
        tui.send(CLAUDE_TRUST.test(pane) ? "Down" : "");
        enters += 1;
        await pause(Math.min(1_500, Math.max(0, deadline - now())));
      }
      return null;
    }
    return CLAUDE_READY.test(pane) ? pane : null;
  }, timeoutMs, 200, dependencies);
  if (!ready) throw new PreconditionError(`${agent} TUI composer unavailable: ${tui.capture().slice(-400)}.`);
  return ready;
}

export async function settleTui(tui, { timeoutMs = 30_000 } = {}, dependencies = {}) {
  const now = dependencies.now ?? Date.now;
  let previous = tui.capture();
  let changedAt = now();
  // The composer can precede MCP startup completion; type only after a quiet second.
  const stable = await waitUntil(() => {
    const pane = tui.capture();
    if (pane !== previous) changedAt = now();
    previous = pane;
    return pane && now() - changedAt >= 1_000;
  }, timeoutMs, 200, dependencies);
  if (!stable) throw new PreconditionError(`TUI did not settle within ${timeoutMs / 1000}s; pane=${previous}`);
  return previous;
}

export async function tuiSubmit(name, tui, text, options = {}, dependencies = {}) {
  const drive = dependencies.tmux ?? tmux;
  const pause = dependencies.sleep ?? sleep;
  const send = (...keys) => {
    const result = drive(["send-keys", "-t", name, ...keys]);
    if (result.status !== 0) {
      throw new PreconditionError(`Tmux could not drive ${name}: ${result.stderr || result.stdout || "send-keys failed"}; pane=${tui.capture()}`);
    }
  };
  await settleTui(tui, options, dependencies);
  send("Escape");
  await pause(80);
  send("-l", text);
  await pause(250);
  const prefix = text.slice(0, 40);
  options.beforeSubmit?.();
  for (let attempt = 0; attempt < 3; attempt += 1) {
    send("C-m");
    const submitted = await waitUntil(() => {
      const pane = tui.capture();
      if (!pane) return text === "/quit";
      const lines = pane.split("\n");
      const composer = lines.findLastIndex((line) => /^\s*[│]?\s*[›❯>]\s/u.test(line));
      return composer < 0 || !lines[composer].includes(prefix) ||
        lines.slice(composer + 1).some((line) => /^\s*[•●]/u.test(line));
    }, 5_000, 200, dependencies);
    if (submitted) return tui.capture();
  }
  throw new Error(`TUI did not submit ${prefix} after 3 Enter attempts; pane=${tui.capture()}`);
}

export async function tuiQuit(tui, name, options = {}, dependencies = {}) {
  try {
    await tuiSubmit(name, tui, "/quit", options, dependencies);
    await (dependencies.sleep ?? sleep)(300);
  } catch {
    // Cleanup must also close a crashed or unresponsive TUI.
  } finally {
    try {
      tui.kill();
    } catch {
      // The process may already have exited after /quit.
    }
  }
}

/**
 * FR-016: a tmux pane inherits the environment of the server, not of the client that asked for the
 * session, so filtering the launcher's own environment is not enough -- the server is started here
 * and the credentials have to be gone from it. `agents.mjs childEnv` states the same rule for the
 * children a probe spawns directly.
 */
function withoutCredentials(env) {
  const clean = { ...env };
  for (const name of Object.keys(clean)) if (isCredentialVariable(name)) delete clean[name];
  return clean;
}

export function tmux(args, opts = {}) {
  const { env, ...rest } = opts;
  return spawnSync("tmux", ["-L", TMUX_SOCKET, ...args], {
    encoding: "utf8",
    ...rest,
    env: withoutCredentials({ ...process.env, ...(env ?? {}) }),
  });
}

/**
 * A pane also inherits the environment of a server that was already running, which the filter
 * above cannot reach: that server was started by whoever ran the first probe. Panes read the
 * server's global environment, so the credentials are removed from it before a session is created.
 */
function scrubServerEnvironment(drive) {
  const shown = drive(["show-environment", "-g"]);
  // No server yet: the next tmux command starts one from the filtered environment above.
  if (shown.status !== 0) return;
  for (const line of (shown.stdout || "").split("\n")) {
    const name = (line.startsWith("-") ? line.slice(1) : line.split("=")[0]).trim();
    if (name && isCredentialVariable(name)) drive(["set-environment", "-g", "-u", name]);
  }
}

export function tmuxSession({ name, command, cwd, env } = {}, { tmux: drive = tmux } = {}) {
  if (!name) throw new Error("tmuxSession: name required");
  scrubServerEnvironment(drive);
  const args = ["new-session", "-d", "-s", name, "-x", "200", "-y", "50"];
  if (cwd) args.push("-c", cwd);
  const extraEnv = withoutCredentials(env || {});
  for (const [k, v] of Object.entries(extraEnv)) {
    args.push("-e", `${k}=${v}`);
  }
  args.push(command || "bash");
  // One command queue sets the policy before tmux handles even an immediate startup exit.
  args.push(";", "set-option", "-t", name, "remain-on-exit", "on");
  const r = drive(args, { env: extraEnv });
  if (r.status !== 0) throw new PreconditionError("tmux new-session: " + (r.stderr || r.stdout || "fail"));
  return {
    send(keys) {
      drive(["send-keys", "-t", name, String(keys), "Enter"]);
    },
    capture() {
      const c = drive(["capture-pane", "-p", "-t", name]);
      const panes = drive(["list-panes", "-t", name, "-F", "#{pane_dead}"]);
      if ((panes.stdout || "").split(/\r?\n/).includes("1")) {
        const history = drive(["capture-pane", "-p", "-S", "-200", "-t", name]);
        throw new PreconditionError(`Tmux pane ${name} exited; pane=${history.stdout || c.stdout || ""}`);
      }
      return c.stdout || "";
    },
    async waitFor(regex, ms = 5000) {
      const re = regex instanceof RegExp ? regex : new RegExp(regex);
      const start = Date.now();
      while (Date.now() - start < ms) {
        if (re.test(this.capture())) return true;
        await new Promise((res) => setTimeout(res, 80));
      }
      throw new Error("waitFor timeout " + re + " pane=" + this.capture().slice(-200));
    },
    kill() {
      drive(["kill-session", "-t", name]);
    },
  };
}
