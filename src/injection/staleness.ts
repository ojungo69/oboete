// Citation staleness for injection packs (FR-029, research.md R12 "Citation staleness",
// contracts/agents.md "Injection policy shared by all agents"). A memory with a stale citation is
// still injected; the pack marks it and the ledger records why.
import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { isAbsolute, resolve } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';

// An injection hook must return within 300 ms (contracts/agents.md SLAs), so no git call may hang.
const GIT_TIMEOUT_MS = 500;

function git(
  cwd: string,
  args: string[],
  timeoutMs: number = GIT_TIMEOUT_MS,
): { status: number; stdout: string } | null {
  // FR-004: the answer comes from the repository at `cwd` and nothing else, so git's own
  // repository-discovery and configuration variables are dropped and the messages stay in C.
  const env: Record<string, string | undefined> = {};
  for (const [key, value] of Object.entries(process.env)) {
    if (!key.startsWith('GIT_')) env[key] = value;
  }
  env.LC_ALL = 'C';

  const result = spawnSync('git', ['-C', cwd, ...args], {
    encoding: 'utf8',
    timeout: timeoutMs,
    stdio: ['ignore', 'pipe', 'pipe'],
    env,
  });
  if (result.error !== undefined || result.status === null) return null;
  return { status: result.status, stdout: result.stdout.trim() };
}

/**
 * The repository's current `HEAD`, or null when it cannot be read. This is the one git call the
 * injection path makes: it compares `HEAD` against the worker's `memories.citations_head` instead
 * of asking git per cited commit (contracts/agents.md "Injection policy shared by all agents").
 * `budgetMs` is what is left of the hook's deadline; the call never outlives the git timeout.
 */
export function repositoryHead(repoRoot: string, budgetMs: number = GIT_TIMEOUT_MS): string | null {
  const timeout = Math.min(GIT_TIMEOUT_MS, Math.floor(budgetMs));
  if (!(timeout > 0)) return null;
  const head = git(repoRoot, ['rev-parse', 'HEAD'], timeout);
  return head === null || head.status !== 0 || head.stdout === '' ? null : head.stdout;
}

/**
 * The commit answers of one `HEAD`, built per process: an injection hook is short-lived, so the
 * cache exists to spare the repeated `git` calls of one pack (R12).
 */
export type AncestorCache = { head: string | null; ancestors: Map<string, boolean> };

export function createAncestorCache(): AncestorCache {
  return { head: null, ancestors: new Map() };
}

/** `true` when the cited path is still present. A relative citation is read against `repoRoot`. */
export function checkPaths(paths: string[], repoRoot: string): Map<string, boolean> {
  const result = new Map<string, boolean>();
  for (const path of paths) {
    if (result.has(path)) continue;
    result.set(path, existsSync(isAbsolute(path) ? path : resolve(repoRoot, path)));
  }
  return result;
}

/**
 * `true` when the cited commit is an ancestor of the repository's current `HEAD`. A git failure
 * leaves the answer unknown, and unknown counts as stale: a pack never claims that a citation it
 * could not check is still current (FR-029).
 */
export function checkCommits(
  commits: string[],
  repoRoot: string,
  cache: AncestorCache,
): Map<string, boolean> {
  const result = new Map<string, boolean>();
  if (commits.length === 0) return result;

  const head = git(repoRoot, ['rev-parse', 'HEAD']);
  if (head === null || head.status !== 0 || head.stdout === '') {
    for (const commit of commits) result.set(commit, false);
    return result;
  }
  if (cache.head !== head.stdout) {
    cache.head = head.stdout;
    cache.ancestors.clear();
  }

  for (const commit of commits) {
    if (result.has(commit)) continue;
    const cached = cache.ancestors.get(commit);
    if (cached !== undefined) {
      result.set(commit, cached);
      continue;
    }
    // FR-004: a citation is a value, never an option. `--end-of-options` (git 2.24) makes git read
    // the commit as an operand even when a provider wrote something option-shaped into it.
    const answer = git(repoRoot, ['merge-base', '--is-ancestor', '--end-of-options', commit, 'HEAD']);
    const ancestor = answer !== null && answer.status === 0;
    cache.ancestors.set(commit, ancestor);
    result.set(commit, ancestor);
  }
  return result;
}

/** The worker's record of the last citation check (data-model.md memories). Hooks only read it. */
export function updateCitationState(
  db: DatabaseSync,
  memoryId: string,
  head: string,
  ok: boolean,
): void {
  db.prepare('UPDATE memories SET citations_head = ?, citations_ok = ? WHERE id = ?').run(
    head,
    ok ? 1 : 0,
    memoryId,
  );
}
