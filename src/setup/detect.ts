import { spawnSync } from 'node:child_process';
import type { SpawnSyncOptionsWithStringEncoding, SpawnSyncReturns } from 'node:child_process';
import {
  accessSync,
  constants,
  existsSync,
  readFileSync,
  readdirSync,
  statSync,
} from 'node:fs';
import { homedir } from 'node:os';
import { delimiter, isAbsolute, join, resolve } from 'node:path';
import { parse as parseToml } from 'smol-toml';
import { z } from 'zod';

import type { AgentName } from '../events.js';
import { childEnvironment } from '../log.js';

import { trustedHash, trustKey, type CodexHandler } from './codex-trust.js';

const VERSION_PROBE_TIMEOUT_MS = 2_000;

export type VersionSpawn = (
  command: string,
  args: string[],
  options: SpawnSyncOptionsWithStringEncoding,
) => SpawnSyncReturns<string>;

export type SetupAgent = Exclude<AgentName, 'unknown'>;
export type AgentTrust = 'n/a' | 'trusted' | 'untrusted' | 'wired' | 'absent';
export type NativeMemory =
  | 'codex_memories'
  | 'claude_auto_memory'
  | 'grok_native_memory';

export type AgentDetection = {
  agent: SetupAgent;
  /** The agent home exists or its CLI is on PATH: there is configuration to manage (FR-031). */
  installed: boolean;
  /** Absolute path of the CLI when it is on PATH, else null: only then can a probe run. */
  cliPath: string | null;
  version?: string;
  configPath: string;
  trust: AgentTrust;
  nativeMemory: NativeMemory | null;
};

const handlerSchema = z.looseObject({
  type: z.string().optional(),
  command: z.string().optional(),
  timeout: z.number().optional(),
  additionalContextLimit: z.number().optional(),
  async: z.boolean().optional(),
});

const groupSchema = z.looseObject({
  matcher: z.unknown().optional(),
  hooks: z.array(handlerSchema),
  oboete: z.boolean().optional(),
});

const hooksFileSchema = z.looseObject({
  hooks: z.record(z.string(), z.array(groupSchema)),
});

const trustRowSchema = z.looseObject({ trusted_hash: z.string().optional() });
const codexConfigSchema = z.looseObject({
  hooks: z
    .looseObject({ state: z.record(z.string(), trustRowSchema).optional() })
    .optional(),
  features: z.looseObject({ memories: z.boolean().optional() }).optional(),
  memories: z
    .union([
      z.boolean(),
      z.looseObject({
        generate_memories: z.boolean().optional(),
        use_memories: z.boolean().optional(),
      }),
    ])
    .optional(),
});

type CodexConfig = z.infer<typeof codexConfigSchema>;

function userHome(env: NodeJS.ProcessEnv): string {
  return resolve(env.HOME?.trim() || env.USERPROFILE?.trim() || homedir());
}

function configuredHome(env: NodeJS.ProcessEnv, variable: string, fallback: string): string {
  const override = env[variable]?.trim();
  return resolve(override === undefined || override === '' ? fallback : override);
}

function isExecutable(file: string): boolean {
  try {
    return statSync(file).isFile() && (accessSync(file, constants.X_OK), true);
  } catch {
    return false;
  }
}

// ponytail: `PATH` and a bare name only, because M1 targets Linux/WSL (plan.md "Target Platform");
// the `Path` casing fallback and the PATHEXT suffixes come back with Windows in M5 (README).
function resolveOnPath(command: string, env: NodeJS.ProcessEnv): string | null {
  for (const entry of (env.PATH ?? '').split(delimiter)) {
    const directory = entry.replace(/^"|"$/g, '');
    // An empty or relative entry would resolve against the current working directory, so a script
    // in the repository could be reported as the agent CLI and then executed.
    if (!isAbsolute(directory)) continue;
    const file = join(directory, command);
    if (isExecutable(file)) return file;
  }
  return null;
}

function firstLine(value: string): string | undefined {
  const line = value.trim().split(/\r?\n/, 1)[0]?.trim();
  return line === undefined || line === '' ? undefined : line;
}

function readVersion(
  cliPath: string,
  env: NodeJS.ProcessEnv,
  spawnFn: VersionSpawn,
): string | undefined {
  const result = spawnFn(cliPath, ['--version'], {
    encoding: 'utf8',
    env,
    timeout: VERSION_PROBE_TIMEOUT_MS,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  if (result.error !== undefined || result.status !== 0) return undefined;
  return firstLine(result.stdout) ?? firstLine(result.stderr);
}

function readCodexConfig(path: string): CodexConfig | null {
  if (!existsSync(path)) return null;
  try {
    const parsed = codexConfigSchema.safeParse(parseToml(readFileSync(path, 'utf8')));
    return parsed.success ? parsed.data : null;
  } catch {
    return null;
  }
}

function codexTrust(hooksPath: string, config: CodexConfig | null): AgentTrust {
  if (!existsSync(hooksPath)) return 'absent';
  let hooks: z.infer<typeof hooksFileSchema>;
  try {
    const parsed = hooksFileSchema.safeParse(JSON.parse(readFileSync(hooksPath, 'utf8')));
    if (!parsed.success) return 'untrusted';
    hooks = parsed.data;
  } catch {
    return 'untrusted';
  }

  const expected: [key: string, hash: string][] = [];
  let invalid = false;
  for (const [event, groups] of Object.entries(hooks.hooks)) {
    groups.forEach((group, groupIndex) => {
      // The marker sits on the entry of the event's array, which for Codex is the matcher group:
      // that is the entry src/setup/managed-block.ts finds again when setup removes the wiring.
      if (group.oboete !== true) return;
      group.hooks.forEach((handler, handlerIndex) => {
        const { command, timeout, additionalContextLimit } = handler;
        // Only a command handler carries an identity oboete can rebuild; anything else oboete
        // marked as its own is a file it no longer understands.
        if (command === undefined || handler.type !== 'command') {
          invalid = true;
          return;
        }
        const rebuilt: CodexHandler = { type: 'command', command };
        if (timeout !== undefined) rebuilt.timeout = timeout;
        if (additionalContextLimit !== undefined) rebuilt.additionalContextLimit = additionalContextLimit;
        expected.push([
          trustKey(hooksPath, event, groupIndex, handlerIndex),
          // The installer's rule (src/setup/codex-trust.ts), so detection cannot drift from what
          // src/setup/write-codex.ts writes.
          trustedHash(event, typeof group.matcher === 'string' ? group.matcher : undefined, rebuilt),
        ]);
      });
    });
  }
  if (invalid) return 'untrusted';
  if (expected.length === 0) return 'absent';
  const rows = config?.hooks?.state;
  return expected.every(([key, hash]) => rows?.[key]?.trusted_hash === hash)
    ? 'trusted'
    : 'untrusted';
}

function codexMemoryEnabled(codexHome: string, config: CodexConfig | null): boolean {
  if (existsSync(join(codexHome, 'memories'))) return true;
  if (config?.features?.memories === true || config?.memories === true) return true;
  return (
    typeof config?.memories === 'object' &&
    (config.memories.generate_memories === true || config.memories.use_memories === true)
  );
}

function claudeMemoryEnabled(claudeHome: string): boolean {
  const projects = join(claudeHome, 'projects');
  try {
    return readdirSync(projects).some((project) => existsSync(join(projects, project, 'memory')));
  } catch {
    return false;
  }
}

/** Read-only installed-agent, wiring-trust and native-memory coexistence detection (FR-031/032/043). */
export function detectAgents(
  env: NodeJS.ProcessEnv = process.env,
  spawnFn: VersionSpawn = spawnSync,
): AgentDetection[] {
  const home = userHome(env);
  const claudeHome = configuredHome(env, 'CLAUDE_CONFIG_DIR', join(home, '.claude'));
  const codexHome = configuredHome(env, 'CODEX_HOME', join(home, '.codex'));
  const grokHome = configuredHome(env, 'GROK_HOME', join(home, '.grok'));
  const piHome = configuredHome(env, 'PI_CODING_AGENT_DIR', join(home, '.pi', 'agent'));
  const codexConfig = readCodexConfig(join(codexHome, 'config.toml'));

  const agents: {
    agent: SetupAgent;
    cli: string;
    home: string;
    configPath: string;
    trust: AgentTrust;
    nativeMemory: NativeMemory | null;
  }[] = [
    {
      agent: 'claude',
      cli: 'claude',
      home: claudeHome,
      configPath: join(claudeHome, 'settings.json'),
      trust: 'n/a',
      nativeMemory: claudeMemoryEnabled(claudeHome) ? 'claude_auto_memory' : null,
    },
    {
      agent: 'codex',
      cli: 'codex',
      home: codexHome,
      configPath: join(codexHome, 'hooks.json'),
      trust: codexTrust(join(codexHome, 'hooks.json'), codexConfig),
      nativeMemory: codexMemoryEnabled(codexHome, codexConfig) ? 'codex_memories' : null,
    },
    {
      agent: 'grok',
      cli: 'grok',
      home: grokHome,
      configPath: join(grokHome, 'hooks', 'oboete.json'),
      trust: existsSync(join(grokHome, 'hooks', 'oboete.json')) ? 'wired' : 'absent',
      nativeMemory: existsSync(join(grokHome, 'memory')) ? 'grok_native_memory' : null,
    },
    {
      agent: 'pi',
      cli: 'pi',
      home: piHome,
      configPath: join(piHome, 'extensions', 'oboete.js'),
      trust: existsSync(join(piHome, 'extensions', 'oboete.js')) ? 'wired' : 'absent',
      nativeMemory: null,
    },
  ];

  const cliPaths = agents.map(({ cli }) => resolveOnPath(cli, env));
  const probeEnv = childEnvironment(env);
  const versions = cliPaths.map((cliPath) =>
    cliPath === null ? undefined : readVersion(cliPath, probeEnv, spawnFn),
  );
  return agents.map((entry, index) => ({
    agent: entry.agent,
    installed: cliPaths[index] !== null || existsSync(entry.home),
    cliPath: cliPaths[index] ?? null,
    ...(versions[index] === undefined ? {} : { version: versions[index] }),
    configPath: entry.configPath,
    trust: entry.trust,
    nativeMemory: entry.nativeMemory,
  }));
}
