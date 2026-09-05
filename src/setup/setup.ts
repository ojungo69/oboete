// `oboete setup`: detect the installed agents, show what a remote destination would receive and
// take consent for it (FR-022), write the four hook installations through the managed-block
// writers (FR-031), verify each one with a headless probe, and report wiring, probe, trust and
// native-memory coexistence per agent (FR-032, FR-043). Nothing here is on the hook path.
import { spawn, spawnSync } from 'node:child_process';
import { homedir } from 'node:os';
import { dirname, resolve } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

import { loadConfig, PRESET_CATALOG, type OboeteConfig, type PresetName } from '../config.js';
import { openDatabase } from '../db/open.js';
import { appendLog, childEnvironment } from '../log.js';
import { ensureDirectories, oboetePaths, resolveHome } from '../paths.js';
import { runtimeStateSet } from '../worker/purge.js';

import {
  CONSENT_STATE_KEY,
  consentDisplay,
  credentialGuidance,
  decideConsent,
  saveConsent,
  saveProviderPreset,
} from './consent.js';
import {
  detectAgents,
  type AgentDetection,
  type AgentTrust,
  type NativeMemory,
  type SetupAgent,
  type VersionSpawn,
} from './detect.js';
import { ManagedFileError } from './managed-block.js';
import { probeEventStored, runProbes, type ProbeResult } from './probe.js';
import { MCP_REMOVE_ARGS, removeClaude, writeClaude } from './write-claude.js';
import { removeCodex, writeCodex } from './write-codex.js';
import { removeGrok, writeGrok } from './write-grok.js';
import { piLoaderPath, removePi, writePi } from './write-pi.js';

/** Where `oboete doctor` (T069) reads the last setup result from (data-model.md runtime_state). */
export const SETUP_RESULT_KEY = 'setup.last_result';

const AGENTS: readonly SetupAgent[] = ['claude', 'codex', 'grok', 'pi'];
type Preset = PresetName | 'none';
const PRESETS = [...Object.keys(PRESET_CATALOG), 'none'] as Preset[];
const VIEW_LINE = 'Open the memory viewer with `oboete view --open`.';
const DATABASE_TIMEOUT_MS = 5_000;

export type SetupDeps = {
  env: NodeJS.ProcessEnv;
  /** Detection's `--version` probe. */
  versionSpawn: VersionSpawn;
  /** Child process spawner for the wiring probes. */
  spawn: typeof spawn;
  /** Runs one agent command line (`claude mcp add`); never through a shell. */
  runCli(command: readonly string[]): { ok: boolean; reason: string };
  write(text: string): void;
  /** Absolute paths written into the developer's hook handlers. */
  node: string;
  bundle: string;
};

type AgentRow = {
  agent: SetupAgent;
  wired: 'yes' | 'failed' | 'not installed' | 'removed';
  probe: ProbeResult['status'] | 'skipped';
  trust: AgentTrust;
  native_memory: NativeMemory | null;
};

type Options = {
  agents: SetupAgent[] | null;
  provider: Preset | null;
  acceptEgress: boolean;
  yes: boolean;
  remove: boolean;
  json: boolean;
};

function defaults(): SetupDeps {
  return {
    env: process.env,
    versionSpawn: spawnSync,
    spawn,
    runCli: (command) => {
      const env = childEnvironment(process.env);
      const result = spawnSync(command[0] ?? '', command.slice(1), {
        encoding: 'utf8',
        env,
        // The agent CLI explains its own refusals ("already exists in user config", "exists in
        // multiple scopes"); discarding that leaves the developer with an exit status to guess at.
        stdio: ['ignore', 'pipe', 'pipe'],
        timeout: 60_000,
      });
      if (result.error !== undefined) {
        const missing = (result.error as NodeJS.ErrnoException).code === 'ENOENT';
        return {
          ok: false,
          reason: missing ? `${command[0]} is not there any more` : describe(result.error),
        };
      }
      const said = `${result.stdout ?? ''}\n${result.stderr ?? ''}`
        .split('\n')
        .map((line) => line.trim())
        .find((line) => line !== '');
      return {
        ok: result.status === 0,
        reason: said ?? `it exited with status ${result.status}`,
      };
    },
    write: (text) => process.stdout.write(text),
    node: process.execPath,
    // Inside dist/oboete.mjs this module is the bundle; the handlers must name that file.
    bundle: fileURLToPath(import.meta.url),
  };
}

function parseOptions(argv: string[]): Options {
  const { values } = parseArgs({
    args: argv,
    allowPositionals: false,
    strict: true,
    options: {
      agents: { type: 'string' },
      provider: { type: 'string' },
      'accept-egress': { type: 'boolean' },
      yes: { type: 'boolean' },
      remove: { type: 'boolean' },
      json: { type: 'boolean' },
    },
  });

  let agents: SetupAgent[] | null = null;
  if (values.agents !== undefined) {
    const names = values.agents.split(',').map((name) => name.trim());
    for (const name of names) {
      if (!AGENTS.includes(name as SetupAgent)) {
        throw new Error(`--agents takes ${AGENTS.join(', ')}; "${name}" is not one of them`);
      }
    }
    agents = names as SetupAgent[];
  }
  if (values.provider !== undefined && !PRESETS.includes(values.provider as Preset)) {
    throw new Error(`--provider takes ${PRESETS.join(', ')}; "${values.provider}" is not one of them`);
  }

  return {
    agents,
    provider: (values.provider ?? null) as Preset | null,
    acceptEgress: values['accept-egress'] === true,
    yes: values.yes === true,
    remove: values.remove === true,
    json: values.json === true,
  };
}

export async function runSetup(argv: string[], overrides: Partial<SetupDeps> = {}): Promise<number> {
  const deps: SetupDeps = { ...defaults(), ...overrides };

  let options: Options;
  try {
    options = parseOptions(argv);
  } catch (error) {
    deps.write(`${describe(error)}\n`);
    return 2;
  }

  const paths = oboetePaths(resolveHome(deps.env));
  ensureDirectories(paths);

  const notes: string[] = [];
  const note = (...lines: string[]): void => {
    notes.push(...lines);
  };
  const finish = (rows: AgentRow[], code: number): number => {
    report(deps, options.json, rows, notes);
    appendLog(paths.hookLog, code === 0 ? 'info' : 'warn', 'setup', {
      agents: rows.map((row) => row.agent).join(',') || 'none',
      remove: options.remove,
      exit: code,
    });
    return code;
  };
  let config: OboeteConfig;
  try {
    config = loadConfig(paths);
  } catch (error) {
    note(describe(error));
    return finish([], 2);
  }
  // `--provider` names the destination this run would settle on. It is applied in memory so the
  // consent screen shows that destination, and written only once the run is past the gate: a
  // refused run must not leave the destination it refused enabled (contracts/cli.md, FR-022).
  const provider = options.remove ? null : options.provider;
  if (provider !== null) config = { ...config, observer: { ...config.observer, preset: provider } };

  const detected = detectAgents(deps.env, deps.versionSpawn);
  // Default: every agent that is installed. Named agents are reported even when they are not, so
  // a typo in `--agents` is visible instead of silently wiring nothing.
  const names = options.agents;
  const selected =
    names === null
      ? detected.filter((agent) => agent.installed)
      : detected.filter((agent) => names.includes(agent.agent));

  if (!options.remove) {
    note(...consentDisplay(config, deps.env));
    const decision = decideConsent({
      config,
      env: deps.env,
      acceptEgress: options.acceptEgress,
      yes: options.yes,
    });
    if (decision.state === 'missing') {
      note(
        'Setup changed nothing: this destination has not been consented to.',
        'Accept it with `oboete setup --accept-egress`, or with `oboete setup --yes` once a stored',
        'record matches the tuple above. `oboete setup --provider ollama` keeps everything on this machine.',
      );
      return finish([], 2);
    }
    // The stored record is what the observer recomputes before every reservation and every send
    // (R8, contracts/observer.md call policy 6), so every run that gets past the gate records the
    // tuple it displayed -- a local preset included, whose record would otherwise stay the remote
    // hash of an earlier run and degrade every batch with `consent_changed` with no way back.
    if (provider !== null) saveProviderPreset(paths, provider);
    if (config.consent.hash !== decision.hash) saveConsent(paths, decision.hash, Date.now());
    config = loadConfig(paths);
    note(...credentialGuidance(config, deps.env));
  }

  let database: DatabaseSync | null = null;
  try {
    database = openDatabase({ path: paths.db, timeoutMs: DATABASE_TIMEOUT_MS }).db;
  } catch (error) {
    note(
      `The memory database ${paths.db} could not be opened (${describe(error)}), so no probe ran and`,
      'nothing was recorded for doctor. The agent configuration files below were still updated.',
    );
  }

  try {
    if (!options.remove && database !== null && config.consent.hash !== undefined) {
      runtimeStateSet(
        database,
        CONSENT_STATE_KEY,
        JSON.stringify({ hash: config.consent.hash, accepted_at: config.consent.accepted_at }),
        Date.now(),
      );
    }

    const wiring = new Map<SetupAgent, AgentRow['wired']>();
    for (const agent of selected) {
      wiring.set(
        agent.agent,
        options.remove ? unwire(agent, deps, note) : wire(agent, detected, deps, note),
      );
    }

    // Trust and native memory are read after the write, so the report states the file as it now
    // stands rather than the state setup found (FR-031: report trust before reporting success).
    const after = detectAgents(deps.env, deps.versionSpawn);
    const probes = await probeSelected(selected, wiring, deps, database, options.remove);

    const rows: AgentRow[] = selected.map((agent) => {
      const current = after.find((entry) => entry.agent === agent.agent) ?? agent;
      if (current.nativeMemory !== null) {
        note(
          `${agent.agent}: its own memory feature (${current.nativeMemory}) is enabled. oboete neither reads`,
          'it nor changes it; the two run side by side.',
        );
      }
      return {
        agent: agent.agent,
        wired: wiring.get(agent.agent) ?? 'not installed',
        probe: probes.get(agent.agent) ?? 'skipped',
        trust: current.trust,
        native_memory: current.nativeMemory,
      };
    });

    if (rows.length === 0) {
      note(
        'No supported agent was found on this machine, so setup changed no agent configuration.',
        'Install Claude Code, Codex, Grok Build or Pi and run `oboete setup` again.',
      );
    }
    if (options.remove) {
      note('The agent configuration files hold nothing of oboete any more; the consent record is kept.');
    }
    if (database !== null) {
      runtimeStateSet(database, SETUP_RESULT_KEY, JSON.stringify({ at: Date.now(), agents: rows }), Date.now());
    }

    const failed =
      database === null ||
      rows.some(
        (row) =>
          row.wired === 'failed' ||
          row.probe === 'fail' ||
          row.probe === 'timeout',
      );
    return finish(rows, failed ? 1 : 0);
  } finally {
    database?.close();
  }
}

/** The agent home each writer takes, derived from the configuration file detection reported. */
function agentHome(detection: AgentDetection, env: NodeJS.ProcessEnv): string {
  switch (detection.agent) {
    case 'claude':
    case 'codex':
      return dirname(detection.configPath);
    case 'grok':
      return dirname(dirname(detection.configPath));
    case 'pi':
      // write-pi.ts composes `.pi/agent/extensions` from the user's home itself.
      return resolve(env.HOME?.trim() || env.USERPROFILE?.trim() || homedir());
  }
}

/**
 * One agent command line, run by the absolute path detection resolved on PATH rather than the bare
 * name: `spawn` resolves a bare name through PATH itself, and a PATH holding '.' or an empty entry
 * would run a file of that name out of the developer's repository -- the reason src/setup/detect.ts
 * refuses a relative PATH entry when it resolves the CLI. An agent with no CLI on PATH is reported
 * per agent and nothing is spawned for it.
 */
function runAgentCli(
  detection: AgentDetection,
  args: readonly string[],
  deps: SetupDeps,
): { ok: boolean; reason: string; command: readonly string[] } {
  if (detection.cliPath === null) {
    return {
      ok: false,
      reason: `the ${detection.agent} executable is not on the PATH`,
      command: [detection.agent, ...args],
    };
  }
  const command = [detection.cliPath, ...args];
  return { ...deps.runCli(command), command };
}

function wire(
  detection: AgentDetection,
  detected: readonly AgentDetection[],
  deps: SetupDeps,
  note: (...lines: string[]) => void,
): AgentRow['wired'] {
  if (!detection.installed) {
    note(`${detection.agent}: not installed, so nothing was written for it.`);
    return 'not installed';
  }
  const home = agentHome(detection, deps.env);
  try {
    switch (detection.agent) {
      case 'claude': {
        const result = writeClaude(home, { nodePath: deps.node, bundlePath: deps.bundle });
        // `claude mcp add` refuses a name it already holds, and setup is a command a developer runs
        // again (after an upgrade, or to repair a file). The removal makes the pair idempotent and
        // repoints an entry left by an older bundle path; it fails harmlessly when there is none.
        runAgentCli(detection, MCP_REMOVE_ARGS, deps);
        const mcp = runAgentCli(detection, result.mcpArgs, deps);
        if (!mcp.ok) {
          note(
            `claude: the memory tools could not be registered because ${mcp.reason}. Capture and injection`,
            `are wired; run \`${mcp.command.join(' ')}\` yourself to add the tools.`,
          );
        }
        return 'yes';
      }
      case 'codex':
        writeCodex(home, { node: deps.node, bundle: deps.bundle });
        return 'yes';
      case 'grok': {
        const result = writeGrok(home, {
          nodePath: deps.node,
          bundlePath: deps.bundle,
          claudeSettingsPath: detected.find((entry) => entry.agent === 'claude')?.configPath,
        });
        if (result.duplicateClaudeEvents.length > 0) {
          note(
            `grok: it also reads the Claude Code settings, so ${result.duplicateClaudeEvents.join(', ')} would`,
            'fire twice per turn. Set GROK_CLAUDE_HOOKS_ENABLED=0 in the shell that starts Grok Build.',
          );
        }
        return 'yes';
      }
      case 'pi': {
        if (piLoaderPath(home) !== detection.configPath) {
          note(
            `pi: PI_CODING_AGENT_DIR points at ${dirname(dirname(detection.configPath))}, and setup can only`,
            `write the loader at ${piLoaderPath(home)}. Unset the variable and run setup again.`,
          );
          return 'failed';
        }
        writePi(home, { node: deps.node, bundle: deps.bundle });
        return 'yes';
      }
    }
  } catch (error) {
    note(...failure(detection.agent, error));
    return 'failed';
  }
}

function unwire(
  detection: AgentDetection,
  deps: SetupDeps,
  note: (...lines: string[]) => void,
): AgentRow['wired'] {
  const home = agentHome(detection, deps.env);
  try {
    switch (detection.agent) {
      case 'claude': {
        removeClaude(home);
        const mcp = runAgentCli(detection, MCP_REMOVE_ARGS, deps);
        if (!mcp.ok) {
          note(`claude: the memory tools could not be unregistered because ${mcp.reason}.`);
        }
        break;
      }
      case 'codex':
        removeCodex(home);
        break;
      case 'grok':
        removeGrok(home);
        break;
      case 'pi':
        removePi(home);
        break;
    }
    return 'removed';
  } catch (error) {
    note(...failure(detection.agent, error));
    return 'failed';
  }
}

/** One shared 90-second deadline over the agents that were wired in this run (FR-031, R12). */
async function probeSelected(
  selected: readonly AgentDetection[],
  wiring: ReadonlyMap<SetupAgent, AgentRow['wired']>,
  deps: SetupDeps,
  database: DatabaseSync | null,
  remove: boolean,
): Promise<Map<SetupAgent, AgentRow['probe']>> {
  const probes = new Map<SetupAgent, AgentRow['probe']>();
  for (const agent of selected) {
    probes.set(agent.agent, agent.installed ? 'skipped' : 'not_installed');
  }
  if (remove || database === null) return probes;

  const targets = selected
    .filter((agent) => wiring.get(agent.agent) === 'yes')
    .map(({ agent, cliPath, configPath }) => ({ agent, cliPath, configPath }));
  const db = database;
  const results = await runProbes(targets, {
    spawn: deps.spawn,
    lookupProbe: (agent, marker) => probeEventStored(db, agent, marker),
    env: deps.env,
  });
  for (const result of results) probes.set(result.agent, result.status);
  return probes;
}

function failure(agent: SetupAgent, error: unknown): string[] {
  const lines = [`${agent}: ${describe(error)}`];
  if (error instanceof ManagedFileError && error.code === 'reparse_failed') {
    lines.push(
      `  Remove the \`[mcp_servers.oboete]\` table that is already in ${error.file} (\`grok mcp add\` and`,
      '  `codex mcp add` write one) and run setup again; oboete keeps its own copy in a managed block.',
    );
  }
  return lines;
}

function report(deps: SetupDeps, json: boolean, rows: readonly AgentRow[], notes: readonly string[]): void {
  if (json) {
    deps.write(`${JSON.stringify({ agents: rows, notes, view: VIEW_LINE }, null, 2)}\n`);
    return;
  }
  for (const line of notes) deps.write(`${line}\n`);
  if (rows.length > 0) deps.write(table(rows));
  deps.write(`${VIEW_LINE}\n`);
}

function table(rows: readonly AgentRow[]): string {
  const header = ['agent', 'wired', 'probe', 'trust', 'native memory'];
  const cells = rows.map((row) => [
    row.agent,
    row.wired,
    row.probe,
    row.trust,
    row.native_memory ?? 'none',
  ]);
  const widths = header.map((title, column) =>
    Math.max(title.length, ...cells.map((cell) => cell[column]?.length ?? 0)),
  );
  const line = (cell: readonly string[]): string =>
    `${cell.map((value, column) => value.padEnd(widths[column] ?? 0)).join('  ').trimEnd()}\n`;
  return `\n${line(header)}${cells.map(line).join('')}\n`;
}

function describe(error: unknown): string {
  return error instanceof Error ? error.message.split('\n')[0] ?? error.name : String(error);
}
