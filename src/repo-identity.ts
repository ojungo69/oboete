import { spawnSync } from 'node:child_process';
import type { SpawnSyncOptionsWithStringEncoding, SpawnSyncReturns } from 'node:child_process';
import { realpathSync } from 'node:fs';
import { resolve } from 'node:path';

import { sha256Hex } from './hash.js';

export type RepoIdentity = {
  id: string;
  identityKind: 'remote' | 'common_dir';
  normalizedIdentity: string;
  root: string;
};

// A hook must return within 300 ms (FR-002), so git gets a slice of it: one call may take
// GIT_TIMEOUT_MS and the whole identity may take GIT_BUDGET_MS, after which the remaining calls are
// skipped and the identity falls back to the git common directory or the working directory. The
// caller lowers the budget to what is left of its own deadline (`options.budgetMs`), because a slow
// git (cold disk, WSL /mnt/c, NFS) would otherwise leave the detector nothing to run in.
const GIT_TIMEOUT_MS = 120;
const GIT_BUDGET_MS = 250;

/** The one child process this module starts; a test injects its own to count the calls. */
export type GitSpawn = (
  file: string,
  args: string[],
  options: SpawnSyncOptionsWithStringEncoding,
) => SpawnSyncReturns<string>;

// `C:\path\repo` is a local path on Windows, not a host called `c` (R8 normalizes to host/path).
const WINDOWS_DRIVE = /^[A-Za-z]:[\\/]/;

const DEFAULT_PORTS: Record<string, string> = {
  'http:': '80',
  'https:': '443',
  'ssh:': '22',
  'git:': '9418',
};

const REMOTE_SCHEMES = ['https:', 'http:', 'ssh:', 'git:', 'file:'];

function git(spawn: GitSpawn, cwd: string, args: string[], timeout: number): string | null {
  // FR-004: the identity comes from the repository at `cwd` and nothing else, so git's own
  // repository-discovery and configuration variables (GIT_DIR, GIT_COMMON_DIR, GIT_WORK_TREE,
  // GIT_CONFIG_*) are dropped; a localized message must not change it either.
  const env: Record<string, string | undefined> = {};
  for (const [key, value] of Object.entries(process.env)) {
    if (!key.startsWith('GIT_')) env[key] = value;
  }
  env.LC_ALL = 'C';

  const result = spawn('git', ['-C', cwd, ...args], {
    encoding: 'utf8',
    timeout,
    stdio: ['ignore', 'pipe', 'pipe'],
    env,
  });
  if (result.error !== undefined || result.status !== 0) return null;
  return result.stdout.trim();
}

function trimPath(path: string): string {
  const withoutSlashes = path.replace(/\/+$/, '');
  return withoutSlashes.endsWith('.git')
    ? withoutSlashes.slice(0, -'.git'.length).replace(/\/+$/, '')
    : withoutSlashes;
}

/**
 * `host/path` with userinfo, query and fragment removed, so a credential embedded in a remote URL
 * never reaches the database or a pack (R8). Returns null when the URL is not one oboete knows.
 */
function normalizeRemote(url: string): string | null {
  const raw = url.trim();
  if (WINDOWS_DRIVE.test(raw)) return null;
  const hasScheme = /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(raw);
  const scpLike = /^(?:[^@/]+@)?([^@/:]+):(.*)$/.exec(raw);
  if (!hasScheme && scpLike !== null) {
    const path = trimPath(scpLike[2]).replace(/^\/+/, '');
    return path === '' ? null : `${scpLike[1].toLowerCase()}/${path}`;
  }

  let parsed: URL;
  try {
    parsed = new URL(raw);
  } catch {
    return null;
  }
  if (!REMOTE_SCHEMES.includes(parsed.protocol)) return null;

  // Reading only hostname, port and pathname drops the userinfo, the query and the fragment.
  const host = parsed.hostname.toLowerCase();
  const port = parsed.port !== '' && parsed.port !== DEFAULT_PORTS[parsed.protocol] ? `:${parsed.port}` : '';
  const path = trimPath(parsed.pathname);
  if (host === '' && path === '') return null;
  return `${host}${port}${path.startsWith('/') ? path : `/${path}`}`;
}

function realpath(path: string): string {
  try {
    return realpathSync(path);
  } catch {
    return resolve(path);
  }
}

function identity(
  identityKind: RepoIdentity['identityKind'],
  normalizedIdentity: string,
  root: string,
): RepoIdentity {
  return {
    // data-model.md repos: first 16 hex of sha256 over the normalized identity.
    id: sha256Hex(normalizedIdentity).slice(0, 16),
    identityKind,
    normalizedIdentity,
    root,
  };
}

/**
 * FR-004: the repository identity is derived here from the repository's remote or its location.
 * Nothing is ever taken from an event payload or an environment variable.
 */
export function resolveRepoIdentity(
  cwd: string,
  options?: { spawn?: GitSpawn; budgetMs?: number },
): RepoIdentity {
  const spawn = options?.spawn ?? spawnSync;
  const budget = Math.min(GIT_BUDGET_MS, options?.budgetMs ?? GIT_BUDGET_MS);
  const startedAt = performance.now();
  const run = (args: string[]): string | null => {
    // The budget bounds the whole identity, so the last call gets whatever is left of it.
    // performance.now() is fractional and spawnSync demands an unsigned integer timeout, so the
    // remainder is floored: a fractional value raises RangeError instead of falling back.
    const remaining = Math.floor(budget - (performance.now() - startedAt));
    return remaining < 1 ? null : git(spawn, cwd, args, Math.min(GIT_TIMEOUT_MS, remaining));
  };

  // One call for both locations: the repository root and the git common directory, in that order.
  const [top, common] = (run(['rev-parse', '--show-toplevel', '--git-common-dir']) ?? '').split('\n');
  const root = top === undefined || top === '' ? cwd : top;

  // A repository with an origin costs this one call; only a repository without one pays for the
  // listing, which is rare enough to keep the common case at two calls inside the budget.
  const url = run(['remote', 'get-url', 'origin']) ?? otherRemoteUrl(run);
  const normalized = url === null ? null : normalizeRemote(url);
  if (normalized !== null) return identity('remote', normalized, root);

  // ponytail: without a usable remote the identity is this machine's path, so the same repository
  // on another machine gets another id; `oboete import --map-repo` maps the two.
  return identity('common_dir', realpath(common === undefined ? cwd : resolve(cwd, common)), root);
}

/** The first remote of a repository that has no `origin`. */
function otherRemoteUrl(run: (args: string[]) => string | null): string | null {
  const names = (run(['remote']) ?? '')
    .split('\n')
    .map((name) => name.trim())
    .filter((name) => name !== '');
  const name = names[0];
  return name === undefined ? null : run(['remote', 'get-url', name]);
}
