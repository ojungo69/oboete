import assert from 'node:assert/strict';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { contentHash, materialHash, memoryIdFor } from '../../src/db/identity.js';
import { openDatabase } from '../../src/db/open.js';
import { sha256Hex } from '../../src/hash.js';
import { getMemory, memoryScope } from '../../src/db/queries.js';
import { oboetePaths } from '../../src/paths.js';
import { cjkBigrams } from '../../src/retrieval/fts.js';
import {
  EXPORT_FORMAT,
  exportMemories,
  importMemories,
  runExport,
  runImport,
  type ImportResult,
} from '../../src/transfer.js';
import { withTempHome } from '../helpers/home.js';

const NOW = 1_800_000_000_000;
// data-model "repos": the id is the first 16 hex of sha256 over the normalized identity, which
// import recomputes before it trusts a header row.
const REMOTE = { id: sha256Hex('github.com/example/one').slice(0, 16), kind: 'remote' as const, identity: 'github.com/example/one' };
const LOCAL = { id: sha256Hex('/home/someone/work/.git').slice(0, 16), kind: 'common_dir' as const, identity: '/home/someone/work/.git' };

function insertRepo(db: DatabaseSync, repo: { id: string; kind: string; identity: string }): void {
  db.prepare(
    `INSERT INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
     VALUES (?, ?, ?, '/tmp/x', 1, 1)`,
  ).run(repo.id, repo.kind, repo.identity);
}

type Seed = {
  repoId: string;
  title: string;
  body: string;
  sensitivity?: string;
  deletedAt?: number | null;
  pinOrder?: number;
  sources?: { kind: string; value: string; agent: string }[];
};

function insertMemory(db: DatabaseSync, seed: Seed): string {
  const material = materialHash(seed.title, seed.body);
  const content = contentHash(seed.repoId, material);
  const id = memoryIdFor(content);
  db.prepare(
    `INSERT INTO memories
       (id, repo_id, type, title, body, concepts, cjk_bigrams, material_hash, content_hash,
        sensitivity, review_state, degraded_reason, source_session_id, valid_from, pinned_at, pin_order, deleted_at, created_at)
     VALUES (?, ?, 'discovery', ?, ?, '["sqlite"]', ?, ?, ?, ?, 'unreviewed', 'rule_based', 's_src', 1, ?, ?, ?, ?)`,
  ).run(
    id,
    seed.repoId,
    seed.deletedAt ? '' : seed.title,
    seed.deletedAt ? '' : seed.body,
    cjkBigrams(`${seed.title} ${seed.body}`),
    material,
    content,
    seed.sensitivity ?? 'eligible',
    seed.pinOrder === undefined ? null : NOW - 1,
    seed.pinOrder ?? null,
    seed.deletedAt ?? null,
    NOW - 10,
  );
  for (const source of seed.sources ?? []) {
    db.prepare(
      `INSERT INTO memory_sources (memory_id, raw_event_id, citation_kind, citation_value, source_agent)
       VALUES (?, NULL, ?, ?, ?)`,
    ).run(id, source.kind, source.value, source.agent);
  }
  return id;
}

function open(home: string): DatabaseSync {
  return openDatabase({ path: oboetePaths(home).db, timeoutMs: 2_000 }).db;
}

function exportOf(db: DatabaseSync): { lines: string[]; counts: { memories: number; tombstones: number } } {
  const lines: string[] = [];
  const counts = exportMemories(db, (line) => lines.push(line), NOW);
  return { lines, counts };
}

async function importInto(
  home: string,
  lines: string[],
  options: { dryRun?: boolean; mapRepo?: Record<string, string> } = {},
): Promise<ImportResult> {
  const db = open(home);
  try {
    return await importMemories(db, lines.join('\n') + '\n', { now: NOW, ...options });
  } finally {
    db.close();
  }
}

test('export writes the header and one line per memory; tombstones and secret rows carry hashes but no text', async () => {
  await withTempHome(async (home) => {
    const db = open(home);
    insertRepo(db, REMOTE);
    const active = insertMemory(db, {
      repoId: REMOTE.id,
      title: 'Busy timeout',
      body: 'Set busy_timeout to 2000 ms.',
      pinOrder: 1,
      sources: [{ kind: 'file_read', value: 'src/db/open.ts', agent: 'codex' }],
    });
    const gone = insertMemory(db, { repoId: REMOTE.id, title: 'Old', body: 'Deleted content.', deletedAt: NOW - 5 });
    insertMemory(db, { repoId: REMOTE.id, title: 'Token', body: 'sk-live-secret', sensitivity: 'secret' });
    const { lines, counts } = exportOf(db);
    db.close();

    assert.deepEqual(counts, { memories: 2, tombstones: 1 });
    assert.equal(lines.length, 4);
    const header = JSON.parse(lines[0]) as { format: string; exported_at: number; repos: { id: string; identity_kind: string; normalized_identity: string }[] };
    assert.equal(header.format, EXPORT_FORMAT);
    assert.equal(header.exported_at, NOW);
    assert.deepEqual(header.repos, [{ id: REMOTE.id, identity_kind: 'remote', normalized_identity: REMOTE.identity }]);

    const rows = lines.slice(1).map((line) => JSON.parse(line) as Record<string, unknown>);
    const activeRow = rows.find((row) => row.id === active)!;
    assert.equal(activeRow.body, 'Set busy_timeout to 2000 ms.');
    assert.equal(activeRow.material_hash, materialHash('Busy timeout', 'Set busy_timeout to 2000 ms.'));
    assert.equal(activeRow.content_hash, contentHash(REMOTE.id, String(activeRow.material_hash)));
    assert.equal(activeRow.pin_order, 1);
    assert.deepEqual(activeRow.sources, [{ citation_kind: 'file_read', citation_value: 'src/db/open.ts', source_agent: 'codex' }]);
    assert.equal(activeRow.source_agent, 'codex');

    const tomb = rows.find((row) => row.id === gone)!;
    assert.equal(tomb.deleted_at, NOW - 5);
    assert.equal(tomb.body, '');
    assert.equal(tomb.material_hash, materialHash('Old', 'Deleted content.'));

    const secret = rows.find((row) => row.sensitivity === 'secret')!;
    assert.equal(secret.title, '');
    assert.equal(secret.body, '');
    assert.ok(!lines.join('\n').includes('sk-live-secret'));
    for (const line of lines) assert.ok(Buffer.byteLength(line) < 65_536);
  });
});

test('a round trip into an empty installation keeps counts, tombstones, sources and quarantines active rows', async () => {
  await withTempHome(async (source) => {
    const db = open(source);
    insertRepo(db, REMOTE);
    const active = insertMemory(db, {
      repoId: REMOTE.id,
      title: 'Busy timeout',
      body: 'Set busy_timeout to 2000 ms.',
      sensitivity: 'eligible',
      sources: [{ kind: 'file_read', value: 'src/db/open.ts', agent: 'codex' }],
    });
    const gone = insertMemory(db, { repoId: REMOTE.id, title: 'Old', body: 'Deleted content.', deletedAt: NOW - 5 });
    const { lines } = exportOf(db);
    db.close();

    await withTempHome(async (target) => {
      const result = await importInto(target, lines);
      assert.deepEqual(result.rejected, []);
      assert.equal(result.inserted, 1);
      assert.equal(result.tombstones, 1);
      const targetDb = open(target);
      try {
        const repo = targetDb.prepare('SELECT id, identity_kind, normalized_identity FROM repos WHERE id = ?').get(REMOTE.id);
        assert.equal(repo?.normalized_identity, REMOTE.identity);
        const row = targetDb.prepare('SELECT * FROM memories WHERE id = ?').get(active) as Record<string, unknown>;
        assert.equal(row.sensitivity, 'local_only');
        assert.equal(row.review_state, 'imported');
        assert.equal(row.content_hash, contentHash(REMOTE.id, materialHash('Busy timeout', 'Set busy_timeout to 2000 ms.')));
        assert.equal(row.cjk_bigrams, cjkBigrams('Busy timeout Set busy_timeout to 2000 ms.'));
        assert.equal(row.source_session_id, 's_src');
        const sources = targetDb.prepare('SELECT citation_kind, citation_value, source_agent FROM memory_sources WHERE memory_id = ?').all(active);
        assert.deepEqual(JSON.parse(JSON.stringify(sources)), [{ citation_kind: 'file_read', citation_value: 'src/db/open.ts', source_agent: 'codex' }]);
        const tomb = targetDb.prepare('SELECT deleted_at, body FROM memories WHERE id = ?').get(gone) as Record<string, unknown>;
        assert.equal(tomb.deleted_at, NOW - 5);
        assert.equal(tomb.body, '');
        // R12: imported rows stay out of search and injection until the worker classifies them.
        const scope = memoryScope(targetDb, { repoId: REMOTE.id, destination: 'injection' });
        assert.equal(getMemory(targetDb, active, scope), null);
        // Importing the same file again changes nothing.
        const again = await importMemories(targetDb, lines.join('\n') + '\n', { now: NOW });
        assert.equal(again.inserted, 0);
        assert.equal(again.tombstones, 0);
        assert.equal(again.unchanged, 2);
      } finally {
        targetDb.close();
      }
    });
  });
});

test('the sensitivity lattice never lowers a row and tombstones win in both directions', async () => {
  await withTempHome(async (source) => {
    const db = open(source);
    insertRepo(db, REMOTE);
    insertMemory(db, { repoId: REMOTE.id, title: 'Stricter', body: 'Comes in as private.', sensitivity: 'private' });
    insertMemory(db, { repoId: REMOTE.id, title: 'Looser', body: 'Comes in as eligible.', sensitivity: 'eligible' });
    insertMemory(db, { repoId: REMOTE.id, title: 'Gone there', body: 'Deleted at the source.', deletedAt: NOW - 5 });
    insertMemory(db, { repoId: REMOTE.id, title: 'Gone here', body: 'Deleted at the target.' });
    const { lines } = exportOf(db);
    db.close();

    await withTempHome(async (target) => {
      const targetDb = open(target);
      insertRepo(targetDb, REMOTE);
      const stricter = insertMemory(targetDb, { repoId: REMOTE.id, title: 'Stricter', body: 'Comes in as private.', sensitivity: 'eligible' });
      const looser = insertMemory(targetDb, { repoId: REMOTE.id, title: 'Looser', body: 'Comes in as eligible.', sensitivity: 'private' });
      const goneThere = insertMemory(targetDb, { repoId: REMOTE.id, title: 'Gone there', body: 'Deleted at the source.' });
      const goneHere = insertMemory(targetDb, { repoId: REMOTE.id, title: 'Gone here', body: 'Deleted at the target.', deletedAt: NOW - 7 });
      const result = await importMemories(targetDb, lines.join('\n') + '\n', { now: NOW });
      assert.deepEqual(result.rejected, []);
      const sensitivity = (id: string): unknown => targetDb.prepare('SELECT sensitivity FROM memories WHERE id = ?').get(id)?.sensitivity;
      const deleted = (id: string): unknown => targetDb.prepare('SELECT deleted_at FROM memories WHERE id = ?').get(id)?.deleted_at;
      assert.equal(sensitivity(stricter), 'private', 'raised to the stricter class');
      assert.equal(sensitivity(looser), 'private', 'never lowered');
      assert.equal(deleted(goneThere), NOW - 5, 'a tombstone in the file deletes the local row');
      assert.equal(deleted(goneHere), NOW - 7, 'a local tombstone is not resurrected');
      assert.equal(result.updated, 1);
      assert.equal(result.tombstones, 1);
      targetDb.close();
    });
  });
});

test('a hash mismatch, an oversized line, a malformed line or a bad header rejects the import with nothing written', async () => {
  await withTempHome(async (source) => {
    const db = open(source);
    insertRepo(db, REMOTE);
    insertMemory(db, { repoId: REMOTE.id, title: 'Fine', body: 'This one is fine.' });
    const { lines } = exportOf(db);
    db.close();
    const forged = { ...(JSON.parse(lines[1]) as Record<string, unknown>), body: 'Another body.', id: 'm_forged', content_hash: 'x'.repeat(64) };

    await withTempHome(async (target) => {
      const cases: [string, string[]][] = [
        ['material_hash', [lines[0], JSON.stringify(forged)]],
        ['line size', [lines[0], JSON.stringify({ ...JSON.parse(lines[1]), body: 'x'.repeat(70_000) })]],
        ['not JSON', [lines[0], '{not json']],
        ['header', ['{"format":"something-else/9","repos":[]}', lines[1]]],
        ['missing field', [lines[0], JSON.stringify({ id: 'm_1' })]],
      ];
      for (const [label, file] of cases) {
        const result = await importInto(target, file);
        assert.ok(result.rejected.length > 0, label);
        assert.equal(result.inserted, 0, label);
        const targetDb = open(target);
        assert.equal(targetDb.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n, 0, label);
        targetDb.close();
      }
      // A valid line next to a rejected one is not written either: the file is applied as a whole.
      const mixed = await importInto(target, [lines[0], lines[1], '{not json']);
      assert.equal(mixed.rejected.length, 1);
      assert.equal(mixed.inserted, 0);
      assert.equal(mixed.applied, false);
    });
  });
});

test('--map-repo moves a machine-local repository and a mapped tombstone still suppresses the content', async () => {
  await withTempHome(async (source) => {
    const db = open(source);
    insertRepo(db, LOCAL);
    insertMemory(db, { repoId: LOCAL.id, title: 'Moved', body: 'Comes from a common_dir repository.' });
    insertMemory(db, { repoId: LOCAL.id, title: 'Buried', body: 'Deleted before the move.', deletedAt: NOW - 5 });
    const { lines } = exportOf(db);
    db.close();

    await withTempHome(async (target) => {
      const targetDb = open(target);
      insertRepo(targetDb, { id: 'r_here', kind: 'common_dir', identity: '/home/other/work/.git' });
      const unmapped = await importMemories(targetDb, lines.join('\n') + '\n', { now: NOW });
      assert.equal(unmapped.inserted, 0);
      assert.match(unmapped.rejected[0]?.reason ?? '', /map-repo/);

      const mapped = await importMemories(targetDb, lines.join('\n') + '\n', { now: NOW, mapRepo: { [LOCAL.id]: 'r_here' } });
      assert.deepEqual(mapped.rejected, []);
      const material = materialHash('Moved', 'Comes from a common_dir repository.');
      const moved = targetDb.prepare('SELECT id, repo_id FROM memories WHERE content_hash = ?').get(contentHash('r_here', material));
      assert.equal(moved?.repo_id, 'r_here');
      assert.equal(moved?.id, memoryIdFor(contentHash('r_here', material)));
      const buried = targetDb
        .prepare('SELECT deleted_at FROM memories WHERE content_hash = ?')
        .get(contentHash('r_here', materialHash('Buried', 'Deleted before the move.')));
      assert.equal(buried?.deleted_at, NOW - 5);
      targetDb.close();
    });
  });
});

test('--dry-run reports the counts and writes nothing; the file limits are enforced', async () => {
  await withTempHome(async (source) => {
    const db = open(source);
    insertRepo(db, REMOTE);
    insertMemory(db, { repoId: REMOTE.id, title: 'Fine', body: 'This one is fine.' });
    const { lines } = exportOf(db);
    db.close();
    await withTempHome(async (target) => {
      const dry = await importInto(target, lines, { dryRun: true });
      assert.equal(dry.inserted, 1);
      assert.equal(dry.applied, false);
      const targetDb = open(target);
      assert.equal(targetDb.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n, 0);
      const tooBig = await importMemories(targetDb, lines.join('\n') + '\n', { now: NOW, maxFileBytes: 10 });
      assert.match(tooBig.rejected[0]?.reason ?? '', /256 MB|file size/i);
      targetDb.close();
    });
  });
});

test('oboete export and oboete import wire the module with the exit codes of contracts/cli.md', async () => {
  await withTempHome(async (home) => {
    const db = open(home);
    insertRepo(db, REMOTE);
    insertMemory(db, { repoId: REMOTE.id, title: 'Fine', body: 'This one is fine.' });
    db.close();
    const file = join(home, 'out', 'memories.jsonl');
    mkdirSync(join(home, 'out'));
    let stdout = '';
    let stderr = '';
    const io = {
      writeOut: (text: string) => {
        stdout += text;
      },
      writeError: (text: string) => {
        stderr += text;
      },
    };
    assert.equal(await runExport([file], io), 0);
    assert.match(stdout, /1 memor(y|ies)/);

    stdout = '';
    assert.equal(await runExport(['-'], io), 0);
    assert.equal(stdout.split('\n').filter((line) => line !== '').length, 2, 'stdout carries the file itself');

    await withTempHome(async (target) => {
      stdout = '';
      assert.equal(await runImport([file, '--dry-run'], io), 0);
      assert.match(stdout, /dry run/i);
      assert.equal(await runImport([file], io), 0);
      const targetDb = open(target);
      assert.equal(targetDb.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n, 1);
      targetDb.close();

      const broken = join(home, 'out', 'broken.jsonl');
      writeFileSync(broken, '{"format":"nope"}\n');
      stderr = '';
      assert.equal(await runImport([broken], io), 2);
      assert.notEqual(stderr, '');
      assert.equal(await runImport([file, '--map-repo', 'nonsense'], io), 2);
      assert.equal(await runImport([join(home, 'missing.jsonl')], io), 2);
    });
  });
});
