// `oboete doctor`: one report of every pipeline part with reason, consequence and recovery
// (FR-033, FR-032, contracts/cli.md). Nothing here is on the hook path.
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, statSync } from 'node:fs';
import type { DatabaseSync } from 'node:sqlite';
import { parseArgs } from 'node:util';

import {
  ConfigError,
  configSchema,
  isPaused,
  loadConfig,
  type OboeteConfig,
} from './config.js';
import { isBusyError, sqliteErrorInfo } from './db/open.js';
import { agentItems, piItem, unrecognizedItem } from './doctor/agents.js';
import { allowanceItem, catalogItems, providerItem } from './doctor/provider.js';
import { ftsItem, migrationItem, openStorage, spoolItem, workerItem } from './doctor/storage.js';
import { appendLog, errorCode } from './log.js';
import { LEXICAL_NOTE } from './memories-cli.js';
import { ensureDirectories, oboetePaths, resolveHome, type OboetePaths } from './paths.js';
import type { VersionSpawn } from './setup/detect.js';
import { describe, renderTable, VIEW_LINE } from './setup/setup.js';

const INTEGRITY_UNVERIFIED =
  'The database failed its integrity check, so this item could not be verified.';
const CONFIG_UNREADABLE =
  'The configuration could not be read, so the provider was not checked.';

export type DoctorStatus = 'healthy' | 'warning' | 'unverified' | 'degraded';

export type DoctorItem = {
  item: string;
  status: DoctorStatus;
  reason: string;
  consequence: string;
  recovery: string;
};

export type DoctorDeps = {
  env: NodeJS.ProcessEnv;
  versionSpawn: VersionSpawn;
  spawn: typeof spawn;
  write(text: string): void;
  now(): number;
  fetch: typeof globalThis.fetch;
};

export type DoctorOptions = {
  probeProvider: boolean;
  noProbeAgents: boolean;
  json: boolean;
};

function defaults(): DoctorDeps {
  return {
    env: process.env,
    versionSpawn: spawnSync,
    spawn,
    write: (text) => {
      process.stdout.write(text);
    },
    now: () => Date.now(),
    fetch: globalThis.fetch,
  };
}

function parseOptions(argv: string[]): DoctorOptions {
  const { values } = parseArgs({
    args: argv,
    allowPositionals: false,
    strict: true,
    options: {
      'probe-provider': { type: 'boolean' },
      'no-probe-agents': { type: 'boolean' },
      json: { type: 'boolean' },
    },
  });
  return {
    probeProvider: values['probe-provider'] === true,
    noProbeAgents: values['no-probe-agents'] === true,
    json: values.json === true,
  };
}

export async function runDoctor(argv: string[], overrides: Partial<DoctorDeps> = {}): Promise<number> {
  const deps: DoctorDeps = { ...defaults(), ...overrides };

  let options: DoctorOptions;
  try {
    options = parseOptions(argv);
  } catch (error) {
    deps.write(`${describe(error)}\n`);
    return 2;
  }

  const paths = oboetePaths(resolveHome(deps.env));
  ensureDirectories(paths);
  const now = deps.now();
  const items: DoctorItem[] = [];
  let db: DatabaseSync | null = null;
  let integrityFailed = false;

  try {
    let loaded: { item: DoctorItem; config: OboeteConfig | null };
    try {
      loaded = loadConfigItem(paths);
    } catch (error) {
      loaded = { item: failedItem('config', error), config: null };
    }
    items.push(loaded.item);
    const config = loaded.config;

    items.push(guardItem('paused', () => pausedItem(paths)));

    let storage;
    try {
      storage = openStorage(paths);
    } catch (error) {
      storage = {
        item: failedItem('storage', error),
        db: null,
        schemaVersion: null,
        schemaAhead: false,
        integrityFailed: false,
      };
    }
    items.push(storage.item);
    db = storage.db;
    integrityFailed = storage.integrityFailed;

    items.push(guardItem('fts', () => ftsItem(db, integrityFailed)));
    items.push(
      guardItem('migration', () =>
        migrationItem(storage.schemaVersion, storage.schemaAhead, integrityFailed),
      ),
    );
    items.push(guardItem('worker', () => workerItem(db, now, integrityFailed)));
    items.push(guardItem('spool', () => spoolItem(paths)));
    items.push(
      await guardItemAsync('provider', () =>
        providerItem({ config, paths, db, integrityFailed, deps, options, now }),
      ),
    );
    items.push(guardItem('allowance', () => allowanceItem(config, db, integrityFailed, now)));
    items.push(
      ...guardList('catalog', () => catalogItems(config, db, integrityFailed, deps.env, now)),
    );
    items.push(
      ...(await guardListAsync('agent:claude', () => agentItems(db, integrityFailed, deps, options))),
    );
    items.push(guardItem('unrecognized-agents', () => unrecognizedItem(db, integrityFailed)));
    items.push(guardItem('pi', () => piItem(paths, db, integrityFailed, now)));
  } finally {
    try {
      db?.close();
    } catch {
      // The report still has to print.
    }
  }

  const degradedCount = items.filter((entry) => entry.status === 'degraded').length;
  const exit = integrityFailed ? 3 : degradedCount > 0 ? 1 : 0;
  report(deps, options.json, items);
  appendLog(paths.hookLog, exit === 0 ? 'info' : 'warn', 'doctor', { exit, degraded: degradedCount });
  return exit;
}

export function healthy(name: string, reason: string): DoctorItem {
  return { item: name, status: 'healthy', reason, consequence: '', recovery: '' };
}

export function warning(name: string, reason: string, consequence: string, recovery: string): DoctorItem {
  return { item: name, status: 'warning', reason, consequence, recovery };
}

export function unverified(name: string, reason: string, consequence: string, recovery: string): DoctorItem {
  return { item: name, status: 'unverified', reason, consequence, recovery };
}

export function degraded(name: string, reason: string, consequence: string, recovery: string): DoctorItem {
  return { item: name, status: 'degraded', reason, consequence, recovery };
}

export function configUnread(name: string): DoctorItem {
  return unverified(
    name,
    CONFIG_UNREADABLE,
    'The observer destination cannot be checked until the configuration file is valid.',
    'Fix the configuration file or delete it and run `oboete setup` again.',
  );
}

export function dbUnread(
  name: string,
  integrityFailed: boolean,
  unavailable: string,
  consequence: string,
  recovery: string,
): DoctorItem {
  return unverified(
    name,
    integrityFailed ? INTEGRITY_UNVERIFIED : unavailable,
    consequence,
    recovery,
  );
}

export function failedItem(name: string, error: unknown): DoctorItem {
  if (name === 'provider' && isBusyError(error)) {
    return degraded(
      'provider',
      'The worker holds the write lock; the provider was not probed.',
      'Summaries fall back to rule-based until the provider answers.',
      'Wait for the worker to finish, then run `oboete doctor --probe-provider`.',
    );
  }
  return degraded(
    name,
    `${name} could not be checked: ${itemLabel(error)}.`,
    'This part of the pipeline was not verified because the check failed.',
    'Resolve the error and run `oboete doctor` again.',
  );
}

function itemLabel(error: unknown): string {
  const sqlite = sqliteErrorInfo(error);
  if (sqlite.errcode !== undefined) return String(sqlite.errcode);
  return errorCode(error);
}

function guardItem(name: string, fn: () => DoctorItem): DoctorItem {
  try {
    return fn();
  } catch (error) {
    return failedItem(name, error);
  }
}

async function guardItemAsync(name: string, fn: () => Promise<DoctorItem>): Promise<DoctorItem> {
  try {
    return await fn();
  } catch (error) {
    return failedItem(name, error);
  }
}

function guardList(name: string, fn: () => DoctorItem[]): DoctorItem[] {
  try {
    return fn();
  } catch (error) {
    return [failedItem(name, error)];
  }
}

async function guardListAsync(name: string, fn: () => Promise<DoctorItem[]>): Promise<DoctorItem[]> {
  try {
    return await fn();
  } catch (error) {
    return [failedItem(name, error)];
  }
}

function loadConfigItem(paths: OboetePaths): { item: DoctorItem; config: OboeteConfig | null } {
  const fallback = configSchema.parse({});
  if (!existsSync(paths.config)) {
    return {
      item: healthy('config', `No configuration file at ${paths.config}; defaults apply.`),
      config: fallback,
    };
  }

  let config: OboeteConfig;
  try {
    config = loadConfig(paths);
  } catch (error) {
    const reason = error instanceof ConfigError ? firstLine(error.message) : describe(error);
    return {
      item: degraded(
        'config',
        reason,
        'Capture still works, but the observer destination and consent record cannot be trusted until the file is fixed.',
        `Fix \`${paths.config}\` or delete it and run \`oboete setup\` again.`,
      ),
      config: null,
    };
  }

  try {
    const mode = statSync(paths.config).mode;
    if ((mode & 0o077) !== 0) {
      const octal = `0o${(mode & 0o777).toString(8).padStart(3, '0')}`;
      return {
        item: degraded(
          'config',
          `Configuration file ${paths.config} has mode ${octal}.`,
          'Other users of this machine can read the provider destination and the consent record.',
          `\`chmod 600 ${paths.config}\``,
        ),
        config,
      };
    }
  } catch (error) {
    return {
      item: degraded(
        'config',
        describe(error),
        'Capture still works, but the observer destination and consent record cannot be trusted until the file is fixed.',
        `Fix \`${paths.config}\` or delete it and run \`oboete setup\` again.`,
      ),
      config: null,
    };
  }

  const mode = statSync(paths.config).mode & 0o777;
  return {
    item: healthy(
      'config',
      `Configuration at ${paths.config} loaded (mode 0o${mode.toString(8).padStart(3, '0')}).`,
    ),
    config,
  };
}

function pausedItem(paths: OboetePaths): DoctorItem {
  if (!isPaused(paths)) return healthy('paused', 'Not paused.');
  return warning(
    'paused',
    `Capture and injection are paused by \`${paths.paused}\`.`,
    'Nothing is captured or injected until resume; existing memories are untouched.',
    '`oboete resume`',
  );
}

function report(deps: DoctorDeps, json: boolean, items: readonly DoctorItem[]): void {
  if (json) {
    deps.write(`${JSON.stringify({ items, notes: [LEXICAL_NOTE], view: VIEW_LINE }, null, 2)}\n`);
    return;
  }
  deps.write(table(items));
  deps.write(`${LEXICAL_NOTE}\n`);
  deps.write(`${VIEW_LINE}\n`);
}

function table(items: readonly DoctorItem[]): string {
  return renderTable(
    ['item', 'status', 'reason'],
    items.map((entry) => [entry.item, entry.status, entry.reason]),
    items.map((entry) =>
      entry.status === 'healthy'
        ? undefined
        : `  consequence: ${entry.consequence}\n  recovery: ${entry.recovery}\n`,
    ),
  );
}

export function countOf(row: unknown, key = 'n'): number {
  if (typeof row !== 'object' || row === null) return 0;
  return asNumber((row as Record<string, unknown>)[key]) ?? 0;
}

export function asNumber(value: unknown): number | null {
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  if (typeof value === 'bigint') return Number(value);
  return null;
}

export function iso(ms: number): string {
  return new Date(ms).toISOString();
}

function firstLine(message: string): string {
  return message.split('\n')[0] ?? message;
}
