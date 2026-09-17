import { existsSync, mkdirSync, realpathSync } from 'node:fs';
import { homedir } from 'node:os';
import { isAbsolute, join, parse, resolve, sep } from 'node:path';

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
  workerStop: string;
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
    workerStop: join(home, 'worker-stop'),
  };
}

export function ensureDirectories(paths: OboetePaths): void {
  // The database, the spool and the logs hold captured content, so the tree stays owner-only.
  for (const directory of [paths.home, paths.spool, paths.spoolFailed, paths.piAck, paths.logs]) {
    mkdirSync(directory, { recursive: true, mode: 0o700 });
  }
}

/**
 * `path` with every symbolic link in the part that exists resolved and the rest appended as written,
 * so two spellings of one file compare equal, including a file that was removed. Git reports a
 * repository root in this physical form, and a macOS temporary directory is always a link. The
 * existing part is found from the top down, so a deep path that does not exist costs one lookup
 * past its last existing directory rather than one per missing segment.
 */
export function physicalPath(path: string): string {
  const absolute = resolve(path);
  const { root } = parse(absolute);
  const segments = absolute.slice(root.length).split(sep).filter((segment) => segment !== '');
  let existing = 0;
  while (existing < segments.length && existsSync(join(root, ...segments.slice(0, existing + 1)))) existing += 1;
  try {
    return join(realpathSync(join(root, ...segments.slice(0, existing))), ...segments.slice(existing));
  } catch {
    return absolute;
  }
}

const GLOB_CHARACTER = /[*?[]/u;

/**
 * The user's own `secret_paths` rules and, for each absolute one, the same rule with its literal
 * directory prefix made physical, so a rule written through a symbolic link still names the file
 * Git reports. Only rules from the user's configuration are resolved: a repository's
 * `.oboete.toml` arrives with a clone, and resolving its paths would let a commit make every hook
 * touch a path of its choosing, an automounted network one included (`/net/<host>` on macOS), so
 * repository rules keep matching as written. A physical prefix that itself contains a glob
 * character is not added, because the rule syntax has no escape for it.
 */
export function withPhysicalRules(rules: readonly string[]): string[] {
  const expanded = [...rules];
  for (const rule of rules) {
    if (!isAbsolute(rule)) continue;
    const segments = rule.split(/[\\/]/u);
    const literal = segments.findIndex((segment) => GLOB_CHARACTER.test(segment));
    // A glob in the first segment leaves no literal prefix; resolving the empty one would name the cwd.
    if (literal === 1) continue;
    const prefix = physicalPath(literal === -1 ? rule : segments.slice(0, literal).join('/'));
    if (GLOB_CHARACTER.test(prefix)) continue;
    const physical = literal === -1 ? prefix : [prefix, ...segments.slice(literal)].join('/');
    if (!expanded.includes(physical)) expanded.push(physical);
  }
  return expanded;
}
