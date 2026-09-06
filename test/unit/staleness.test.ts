import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { after, test } from 'node:test';

import { openDatabase } from '../../src/db/open.js';
import {
  checkCommits,
  checkPaths,
  createAncestorCache,
  updateCitationState,
} from '../../src/injection/staleness.js';
import { oboetePaths } from '../../src/paths.js';
import { withTempHome } from '../helpers/home.js';

const gitAvailable = spawnSync('git', ['--version'], { encoding: 'utf8' }).status === 0;
const skip = gitAvailable ? false : 'git is not installed, so the commit staleness tests cannot run.';

const temporaryRoots: string[] = [];

after(() => {
  // A git wrapper on PATH (git-ai writes .git/ai after the command returned) can still be adding to
  // the directory for a moment, so the removal is retried.
  for (const root of temporaryRoots) {
    rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 50 });
  }
});

function temporaryRoot(): string {
  const root = mkdtempSync(join(tmpdir(), 'oboete-stale-'));
  temporaryRoots.push(root);
  return root;
}

function git(cwd: string, ...args: string[]): string {
  const result = spawnSync('git', args, { cwd, encoding: 'utf8' });
  assert.equal(result.status, 0, `git ${args.join(' ')}: ${result.stderr}`);
  return result.stdout.trim();
}

function repositoryWithTwoCommits(): { root: string; first: string; second: string } {
  const root = join(temporaryRoot(), 'work');
  mkdirSync(root);
  git(root, 'init', '--quiet', '--initial-branch', 'main');
  // `git commit` may start a background `git gc --auto`, which would still be writing into the
  // directory while the cleanup hook removes it.
  git(root, 'config', 'gc.auto', '0');
  git(root, 'config', 'user.email', 'tester@example.test');
  git(root, 'config', 'user.name', 'Tester');
  writeFileSync(join(root, 'first.txt'), 'first\n');
  git(root, 'add', '.');
  git(root, 'commit', '--quiet', '-m', 'first');
  const first = git(root, 'rev-parse', 'HEAD');
  writeFileSync(join(root, 'second.txt'), 'second\n');
  git(root, 'add', '.');
  git(root, 'commit', '--quiet', '-m', 'second');
  const second = git(root, 'rev-parse', 'HEAD');
  return { root, first, second };
}

test('a cited path is fresh while the file exists and stale once it is gone', () => {
  const root = temporaryRoot();
  writeFileSync(join(root, 'present.ts'), 'export const value = 1;\n');
  const result = checkPaths(['present.ts', 'gone.ts', join(root, 'present.ts')], root);
  assert.equal(result.get('present.ts'), true);
  assert.equal(result.get('gone.ts'), false);
  assert.equal(result.get(join(root, 'present.ts')), true, 'an absolute citation is used as it is');
});

test('a cited commit is fresh while it is an ancestor of HEAD', { skip }, () => {
  const { root, first, second } = repositoryWithTwoCommits();
  const cache = createAncestorCache();
  const result = checkCommits([first, second, '0'.repeat(40)], root, cache);
  assert.equal(result.get(first), true);
  assert.equal(result.get(second), true);
  assert.equal(result.get('0'.repeat(40)), false, 'an unknown commit id is stale');
});

test('the ancestor cache is keyed by HEAD and is dropped when HEAD moves', { skip }, () => {
  const { root, first } = repositoryWithTwoCommits();
  const cache = createAncestorCache();
  assert.equal(checkCommits([first], root, cache).get(first), true);
  const headBefore = cache.head;
  assert.equal(cache.ancestors.size, 1);

  // A commit that is only on the other branch is not an ancestor of the new HEAD.
  git(root, 'checkout', '--quiet', '-b', 'side', 'HEAD~1');
  writeFileSync(join(root, 'side.txt'), 'side\n');
  git(root, 'add', '.');
  git(root, 'commit', '--quiet', '-m', 'side');
  const { second } = { second: git(root, 'rev-parse', 'main') };

  const after = checkCommits([first, second], root, cache);
  assert.notEqual(cache.head, headBefore, 'the cache follows the new HEAD');
  assert.equal(after.get(first), true);
  assert.equal(after.get(second), false);
});

test('an option-shaped citation is a commit argument, never a git option', { skip }, () => {
  const { root } = repositoryWithTwoCommits();

  // `--version` would make git print its version and exit 0; read as a commit it is simply not an
  // ancestor, so the pack marks the citation stale instead of claiming it is current (FR-029).
  const answers = checkCommits(['--version', '--all'], root, createAncestorCache());

  assert.equal(answers.get('--version'), false);
  assert.equal(answers.get('--all'), false);
});

test('a git failure counts as stale, so a citation is never claimed to be fresh', () => {
  const root = temporaryRoot();
  const cache = createAncestorCache();
  const result = checkCommits(['0'.repeat(40)], root, cache);
  assert.equal(result.get('0'.repeat(40)), false);
  assert.equal(cache.head, null, 'nothing is cached when HEAD cannot be read');
});

test('the worker records the citation state on the memory row', async () => {
  await withTempHome(async (home) => {
    const { db } = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1_000 });
    db.prepare(
      `INSERT INTO repos (id, identity_kind, normalized_identity, created_at, last_seen_at)
       VALUES ('r1', 'remote', 'example.test/one', 1, 1)`,
    ).run();
    db.prepare(
      `INSERT INTO memories (id, repo_id, type, title, body, content_hash, sensitivity,
         review_state, created_at)
       VALUES ('m1', 'r1', 'discovery', 'Title', 'Body', 'c1', 'eligible', 'unreviewed', 1)`,
    ).run();

    updateCitationState(db, 'm1', 'abc123', false);
    const stale = db.prepare('SELECT citations_head, citations_ok FROM memories WHERE id = ?').get('m1');
    assert.equal(stale?.citations_head, 'abc123');
    assert.equal(stale?.citations_ok, 0);

    updateCitationState(db, 'm1', 'def456', true);
    const fresh = db.prepare('SELECT citations_head, citations_ok FROM memories WHERE id = ?').get('m1');
    assert.equal(fresh?.citations_head, 'def456');
    assert.equal(fresh?.citations_ok, 1);
    db.close();
  });
});
