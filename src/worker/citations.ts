import type { DatabaseSync } from 'node:sqlite';

import {
  checkCommits,
  checkPaths,
  type AncestorCache,
  updateCitationState,
} from '../injection/staleness.js';
import { assertLease, transactionImmediate } from './lease.js';
import type { ObserveDeps } from './observe.js';

function gitEnvironment(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
  const clean: NodeJS.ProcessEnv = { ...env, LC_ALL: 'C' };
  for (const key of Object.keys(clean)) {
    if (key.startsWith('GIT_')) delete clean[key];
  }
  return clean;
}

async function repositoryHead(
  root: string,
  deps: ObserveDeps,
): Promise<string | null> {
  return await new Promise((resolve) => {
    let settled = false;
    const finish = (head: string | null): void => {
      if (settled) return;
      settled = true;
      resolve(head);
    };
    try {
      const child = deps.spawn('git', ['-C', root, 'rev-parse', 'HEAD'], {
        env: gitEnvironment(deps.env),
        stdio: ['ignore', 'pipe', 'ignore'],
        signal: AbortSignal.timeout(500),
      });
      let stdout = '';
      child.stdout?.setEncoding('utf8');
      child.stdout?.on('data', (chunk: string) => {
        if (stdout.length < 256) stdout += chunk;
      });
      child.once('error', () => finish(null));
      child.once('close', (code) => {
        const head = stdout.trim();
        finish(code === 0 && /^[0-9a-f]{40,64}$/iu.test(head) ? head : null);
      });
    } catch {
      finish(null);
    }
  });
}

export async function updateBatchCitations(
  db: DatabaseSync,
  token: string,
  repoId: string,
  memoryIds: string[],
  ancestorCache: AncestorCache,
  deps: ObserveDeps,
): Promise<boolean> {
  if (memoryIds.length === 0) return true;
  const root = db.prepare('SELECT display_root FROM repos WHERE id = ?').get(repoId)?.display_root;
  if (typeof root !== 'string' || root === '') return true;
  const head = await repositoryHead(root, deps);
  if (head === null) return true;
  const citationState = new Map<string, boolean>();
  for (const memoryId of new Set(memoryIds)) {
    const citations = db
      .prepare(
        `SELECT citation_kind, citation_value FROM memory_sources
         WHERE memory_id = ? AND context_only = 0 AND citation_kind IS NOT NULL AND citation_value IS NOT NULL`,
      )
      .all(memoryId);
    const paths = citations.flatMap((row) =>
      (row.citation_kind === 'file_read' || row.citation_kind === 'file_modified') &&
      typeof row.citation_value === 'string'
        ? [row.citation_value]
        : [],
    );
    const commits = citations.flatMap((row) =>
      row.citation_kind === 'commit' && typeof row.citation_value === 'string'
        ? [row.citation_value]
        : [],
    );
    citationState.set(
      memoryId,
      [...checkPaths(paths, root).values(), ...checkCommits(commits, root, ancestorCache).values()].every(
        Boolean,
      ),
    );
  }
  return transactionImmediate(db, () => {
    if (!assertLease(db, token, deps.now())) {
      db.exec('ROLLBACK');
      return false;
    }
    for (const [memoryId, ok] of citationState) updateCitationState(db, memoryId, head, ok);
    return true;
  });
}
