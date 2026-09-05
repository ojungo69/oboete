import { mkdirSync } from 'node:fs';
import { homedir } from 'node:os';
import { isAbsolute, join, resolve } from 'node:path';

export type OboetePaths = {
  home: string;
  config: string;
  db: string;
  spool: string;
  spoolFailed: string;
  piAck: string;
  logs: string;
  hookLog: string;
  observeLog: string;
  paused: string;
};

/** The one data directory (FR-039, amendment A4): `OBOETE_HOME`, else `~/.oboete`. */
export function resolveHome(env: NodeJS.ProcessEnv = process.env): string {
  const override = env.OBOETE_HOME?.trim();
  if (!override) return join(homedir(), '.oboete');
  // A hook runs in the agent's working directory and the worker in its own, so resolving a relative
  // override against the current directory would give each process a different data directory --
  // and FR-039 has exactly one. A relative override is therefore anchored to the home directory,
  // which every process agrees on.
  return isAbsolute(override) ? resolve(override) : resolve(homedir(), override);
}

/** Every path under the data directory. No other module composes one (conventions, "Data directory and files"). */
export function oboetePaths(home: string): OboetePaths {
  const spool = join(home, 'spool');
  const logs = join(home, 'logs');
  return {
    home,
    config: join(home, 'config.toml'),
    db: join(home, 'memory.db'),
    spool,
    spoolFailed: join(spool, 'failed'),
    piAck: join(spool, 'pi-ack'),
    logs,
    hookLog: join(logs, 'hook.log'),
    observeLog: join(logs, 'observe.log'),
    paused: join(home, 'paused'),
  };
}

export function ensureDirectories(paths: OboetePaths): void {
  // The database, the spool and the logs hold captured content, so the tree stays owner-only.
  for (const directory of [paths.home, paths.spool, paths.spoolFailed, paths.piAck, paths.logs]) {
    mkdirSync(directory, { recursive: true, mode: 0o700 });
  }
}
