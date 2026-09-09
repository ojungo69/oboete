import fs from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";

import { GROK_ISOLATION_ENV, copyMode, shellQuote } from "./agents.mjs";
import { AGENT_OUTAGE_RE, PreconditionError, waitUntil } from "./process.mjs";

const SYNTHETIC_REMOTE = "https://example.invalid/oboete-e2e.git";
export const DONE_PROMPT = "Reply with exactly DONE and do not use tools.";
const RECALL_TOOL = {
  claude: "Use the Read tool exactly once on NOTES.md.",
  codex: "Use the shell tool exactly once to run: sed -n '1,20p' NOTES.md",
  grok: "Use the read_file tool exactly once on NOTES.md.",
  pi: "Use the read tool exactly once on NOTES.md.",
};

export function requireAgentSuccess(result, action) {
  if (result.exitCode === 0) return;
  const diagnostic = `${result.stderr ?? ""}\n${result.stdout ?? ""}`;
  const detail = diagnostic
    .split(/\r?\n/)
    .map((line) => line.trim())
    .find(Boolean);
  const unavailable = result.exitCode === 124 || AGENT_OUTAGE_RE.test(diagnostic);
  const detailSuffix = detail ? `: ${detail.slice(0, 240)}` : "";
  const reason = `${action} exited ${result.exitCode}${detailSuffix}.`;
  if (unavailable) throw new PreconditionError(reason);
  throw new Error(reason);
}

export function buildFactSeedingPrompt(facts) {
  if (!Array.isArray(facts) || facts.length !== 3 || facts.some((fact) => typeof fact !== "string" || fact === "")) {
    throw new TypeError("fact seeding requires exactly three non-empty strings");
  }
  const command = String.raw`printf '%s\n' ${facts.map((fact) => shellQuote(fact)).join(" ")} >> NOTES.md`;
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
    .some((line) => {
      // Two anchored tests instead of one `\s+.*` pattern, which re-split a run of spaces; the
      // accepted lines are the same: any whitespace run after the colon, then a line without a
      // line terminator that ends in the phrase.
      const trimmed = line.trim();
      const head = /^>\s*degraded:\s+/iu.exec(trimmed);
      return head !== null && /^.*\brule-based notes\.$/iu.test(trimmed.slice(head[0].length));
    });
  return {
    pass: missingFacts.length === 0 && (!requireDegraded || degradedMarker),
    missingFacts,
    degradedMarker,
  };
}

export function factSet(stem) {
  return [
    `${stem}-1: the build token is cedar.`,
    `${stem}-2: the release bird is heron.`,
    `${stem}-3: 配布色は琥珀。`,
  ];
}

export function lifecycleRecallPrompt(agent) {
  return [
    "Recall the repository's build token, release bird, and 配布色 only from the oboete memory context.",
    RECALL_TOOL[agent],
    "The file intentionally hides those values; do not derive the answer from it and make no other tool call.",
    "Reply with every matching fact line verbatim, joined by |.",
  ].join(" ");
}

export function recallPrompt(agent, noCredentials) {
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

export function prepareOboeteHome(destination, source) {
  fs.mkdirSync(destination, { recursive: true, mode: 0o700 });
  copySetupFile(path.join(source, "config.toml"), path.join(destination, "config.toml"), true);
}

function prepareClaudeAgent(config, homes, prompt, extraArgs) {
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

function prepareCodexAgent(config, homes, prompt, repo, extraArgs) {
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

function prepareGrokAgent(config, homes, prompt, repo) {
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

function preparePiAgent(config, directory, homes, prompt) {
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

export function prepareAgent(agent, directory, homes, prompt, repo, extraArgs = []) {
  const config = path.join(directory, "agent-home");
  switch (agent) {
    case "claude": {
      return prepareClaudeAgent(config, homes, prompt, extraArgs);
    }
    case "codex": {
      return prepareCodexAgent(config, homes, prompt, repo, extraArgs);
    }
    case "grok": {
      return prepareGrokAgent(config, homes, prompt, repo);
    }
    case "pi": {
      return preparePiAgent(config, directory, homes, prompt);
    }
    default:
      throw new Error(`unknown agent: ${agent}`);
  }
}

export async function launchAgent(configuration
) {
  const { agent, directory, repo, prompt, options, homes, dependencies, oboeteHome, launch = {} } = configuration;
  fs.mkdirSync(directory, { recursive: true });
  const prepared = prepareAgent(agent, launch.runtimeDir ?? directory, homes, prompt, repo, launch.extraArgs ?? []);
  const stdoutPath = path.join(directory, "stdout.txt");
  const stderrPath = path.join(directory, "stderr.txt");
  const proc = await dependencies.runTimed(prepared.argv, {
    cwd: repo,
    // An agent CLI runs the developer's shell tools; childEnv keeps the credentials out of it.
    env: dependencies.childEnv({ ...prepared.env, ...launch.env, OBOETE_HOME: oboeteHome }),
    stdoutPath,
    stderrPath,
    timeoutMs: options.timeoutMs,
  });
  return { ...proc, stdoutPath, stderrPath };
}

function searchContainsFacts(output, facts) {
  let parsed;
  try {
    parsed = JSON.parse(output);
  } catch {
    return false;
  }
  return (parsed?.memories ?? []).some((row) => assertAgentOutput(JSON.stringify(row), facts).pass);
}

export async function waitForSummary(repo, directory, facts, options, dependencies, env) {
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

export function configureRemote(repo, directory, options, dependencies) {
  return dependencies.runTimed(["git", "config", "remote.origin.url", SYNTHETIC_REMOTE], {
    cwd: repo,
    env: dependencies.childEnv(),
    stdoutPath: path.join(directory, "git.stdout.txt"),
    stderrPath: path.join(directory, "git.stderr.txt"),
    timeoutMs: Math.min(15_000, options.timeoutMs),
  });
}

export function databasePath(oboeteHome) {
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

export async function runObserver(repo, directory, oboeteHome, options, dependencies) {
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

/** The file's content, or an empty string when it is not there. */
export function readIfPresent(file) {
  try {
    return fs.readFileSync(file, "utf8");
  } catch {
    return "";
  }
}
