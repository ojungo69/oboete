// US6 device sync bounds and crash safety (contracts/sync.md "Verification", "Bounds" and the
// "Push"/"Pull" steps). Bundles here are written by hand so a single bound can be put one over its
// value: the reader recomputes every id, so a synthetic bundle is only accepted when it is exactly
// as well formed as one `buildSnapshot` writes.
import assert from 'node:assert/strict';
import { createHash, randomBytes } from 'node:crypto';
import { closeSync, existsSync, openSync, readdirSync, readSync, rmSync, statSync, truncateSync, writeFileSync, writeSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import type { DatabaseSync } from 'node:sqlite';

import { materialHash } from '../../src/db/identity.js';
import { loadSqlite, openDatabase } from '../../src/db/open.js';
import { oboetePaths } from '../../src/paths.js';
import { applyStaged, resolveRow, type ApplyResult } from '../../src/sync/apply.js';
import { captureLocalChanges } from '../../src/sync/capture.js';
import { BundleError, decryptBundle, encryptBundle, MAX_CIPHERTEXT_BYTES, MAX_PLAINTEXT_BYTES } from '../../src/sync/envelope.js';
import { BOUNDS } from '../../src/sync/format.js';
import { canonicalJson, payloadHash, revisionId, snapshotId, SNAPSHOT_FORMAT, type SyncKind } from '../../src/sync/identity.js';
import {
  initSpace, joinSpace, pullSpace, pushSpace, readKey, spaceDirectory, SyncError, syncPaths, withSpaceLock,
} from '../../src/sync/space.js';
import { buildSnapshot } from '../../src/sync/publish.js';
import { BundleRejected, stageBundle } from '../../src/sync/stage.js';
import {
  canonicalOf, effectiveControl, headsOf, originsOfRow, readOrigin, readRevision, replicaOriginId, repoKeyFor, type Row,
} from '../../src/sync/store.js';
import {
  insertMemory, memoryOf, openHome, publish, pull, REMOTE, REPO, revisionCount, SPACE, withHomes, withReplicas,
} from '../helpers/sync.js';

const SENDER = 'a'.repeat(32);
const OTHER = 'e'.repeat(32);
const CONTROL = { tombstone: false, sensitivity_floor: 'eligible' } as const;
const REPO_KEY = repoKeyFor(SENDER, 'remote', REMOTE);
const hex = (value: number, width: number): string => value.toString(16).padStart(width, '0');
const ordinary = (material: number): Row => ({ domain: 'ordinary', repo: REPO_KEY, material_hash: hex(material, 64) });

type Line = Record<string, unknown>;

/** One revision line whose `revision_id` is the one the reader recomputes from its identity fields. */
function line(input: {
  origin_id: string; kind?: SyncKind; author?: string; parents?: string[]; natural: Row;
  payload?: Row | null; payload_hash?: string | null; head?: boolean;
}): Line {
  const payload = input.payload ?? null;
  const identity = {
    origin_id: input.origin_id, kind: input.kind ?? 'memory', author: input.author ?? SENDER,
    parents: input.parents ?? [], control: CONTROL, natural: input.natural,
    payload_hash: input.payload_hash !== undefined ? input.payload_hash : (payload === null ? null : payloadHash(payload)),
  };
  return { ...identity, revision_id: revisionId(identity), head: input.head ?? true, payload };
}

/**
 * Writes a plaintext bundle: body lines first (hashed and counted as they go, so a million-line
 * bundle never sits in memory), then the header the body forces. `header` overrides one field.
 */
function writeBundle(path: string, sender: string, produce: (emit: (value: Line | string) => void) => void,
  header: Record<string, unknown> = {}): void {
  const bodyPath = `${path}.body`;
  const digest = createHash('sha256');
  let revisionLines = 0;
  let heads = 0;
  const body = openSync(bodyPath, 'w', 0o600);
  try {
    produce((value) => {
      const buffer = Buffer.from(`${typeof value === 'string' ? value : canonicalJson(value)}\n`, 'utf8');
      writeSync(body, buffer);
      digest.update(buffer);
      if (typeof value === 'string' || value.kind === 'repo') return;
      revisionLines += 1;
      if (value.head === true) heads += 1;
    });
  } finally { closeSync(body); }
  const full: Record<string, unknown> = {
    format: SNAPSHOT_FORMAT, space_id: SPACE, replica_origin_id: sender, revision_lines: revisionLines, heads,
    revisions_sha256: digest.digest('hex'), withheld: { works: 0, memories: 0, sources: 0, contexts: 0, proposals: 0 },
    produced_at: 1, ...header,
  };
  full.snapshot_id = snapshotId(String(full.space_id), String(full.replica_origin_id), String(full.revisions_sha256));
  const out = openSync(path, 'w', 0o600);
  try {
    writeSync(out, `${canonicalJson(full)}\n`);
    const input = openSync(bodyPath, 'r');
    try {
      const chunk = Buffer.alloc(1 << 20);
      for (;;) {
        const read = readSync(input, chunk, 0, chunk.length, null);
        if (read === 0) break;
        writeSync(out, chunk, 0, read);
      }
    } finally { closeSync(input); }
  } finally { closeSync(out); rmSync(bodyPath, { force: true }); }
}

/** Stage and apply one plaintext bundle exactly as `pullSpace` does, in one transaction. */
function applyBundle(db: DatabaseSync, sender: string, path: string, now = 200): ApplyResult {
  const staged = stageBundle(db, { plaintextPath: path, scratchPath: `${path}.scratch`, spaceId: SPACE, senderOriginId: sender });
  try {
    db.exec('BEGIN IMMEDIATE');
    try {
      const result = applyStaged(db, staged, { senderOriginId: sender, now });
      db.exec('COMMIT');
      return result;
    } catch (error) { db.exec('ROLLBACK'); throw error; }
  } finally { staged.close(); }
}

const counts = (db: DatabaseSync): Record<string, number> => Object.fromEntries(
  ['sync_revisions', 'sync_origins', 'sync_conflicts', 'memories'].map((table) => [table,
    Number(db.prepare(`SELECT COUNT(*) AS n FROM ${table}`).get()?.n)]));

const rejected = (code: string) => (error: unknown): boolean => error instanceof BundleRejected && error.code === code;

// --- Heads per origin and per row (contracts/sync.md "Bounds", "Verification") ---

test('64 sibling heads apply and one resolve closes them with every head as a parent', async () => {
  await withReplicas(1, ([replica], dir) => {
    const { db } = replica!;
    const origin = `${SENDER}:m_wide`;
    const path = join(dir, `${SENDER}.plain`);
    writeBundle(path, SENDER, (emit) => {
      for (let i = 0; i < BOUNDS.headsPerOrigin; i += 1) emit(line({ origin_id: origin, author: hex(i, 32), natural: ordinary(1) }));
    });
    const result = applyBundle(db, SENDER, path);
    assert.equal(result.stored, 64);
    assert.equal(result.conflicts, 1);
    assert.equal(headsOf(db, origin).length, 64);
    assert.equal(db.prepare('SELECT status FROM sync_conflicts WHERE id = ?').get(`sync:${origin}`)?.status, 'open');
    // `resolve` cites every current head, which is why parents per revision equals heads per origin.
    const keep = headsOf(db, origin)[7]!;
    db.exec('BEGIN IMMEDIATE');
    const { revision_id } = resolveRow(db, origin, keep, 300);
    db.exec('COMMIT');
    assert.deepEqual(headsOf(db, origin), [revision_id]);
    assert.equal(readRevision(db, revision_id)!.parents.length, 64);
    assert.equal(db.prepare('SELECT status FROM sync_conflicts WHERE id = ?').get(`sync:${origin}`)?.status, 'resolved');
  });
});

test('an origin with 65 heads is rejected and leaves the database untouched', async () => {
  await withReplicas(1, ([replica], dir) => {
    const { db } = replica!;
    const before = counts(db);
    const path = join(dir, `${SENDER}.plain`);
    writeBundle(path, SENDER, (emit) => {
      for (let i = 0; i <= BOUNDS.headsPerOrigin; i += 1) emit(line({ origin_id: `${SENDER}:m_over`, author: hex(i, 32), natural: ordinary(2) }));
    });
    assert.throws(() => applyBundle(db, SENDER, path), rejected('heads_per_row'));
    assert.deepEqual(counts(db), before);
  });
});

test('two origins aliased onto one row are counted together: 64 heads apply, 66 are rejected', async () => {
  await withReplicas(2, ([a, b], dir) => {
    insertMemory(a!.db, 'm_one', 'Title', 'Body text');
    pull(b!, a!, publish(a!, dir));
    const first = `${a!.id}:m_one`;
    const second = `${OTHER}:m_alias`;
    const natural = readOrigin(b!.db, first)!.natural;
    const local = memoryOf(b!, a!, 'm_one').id as string;
    // 1 head from the pulled payload + 31 siblings on its origin + 32 on a second origin with the
    // same natural key: `aliasTarget` binds both to the one local row, so the row holds 64 heads.
    const path = join(dir, `${OTHER}.plain`);
    writeBundle(path, OTHER, (emit) => {
      for (let i = 0; i < 31; i += 1) emit(line({ origin_id: first, author: hex(i, 32), natural }));
      for (let i = 0; i < 32; i += 1) emit(line({ origin_id: second, author: hex(100 + i, 32), natural }));
    });
    applyBundle(b!.db, OTHER, path);
    const row = canonicalOf(b!.db, first).origin_id;
    assert.deepEqual(originsOfRow(b!.db, 'memory', local).map((origin) => origin.origin_id).sort(), [first, second].sort());
    assert.equal(headsOf(b!.db, row).length, 64);
    const before = counts(b!.db);
    const over = join(dir, `${OTHER}.more.plain`);
    writeBundle(over, OTHER, (emit) => {
      for (let i = 0; i < 2; i += 1) emit(line({ origin_id: second, author: hex(200 + i, 32), natural }));
    });
    assert.throws(() => applyBundle(b!.db, OTHER, over, 400), rejected('heads_per_row'));
    assert.deepEqual(counts(b!.db), before);
  });
});

test('a bundle from a store of 2,000 single-head origins is accepted', async () => {
  await withReplicas(2, ([a, b], dir) => {
    for (let i = 0; i < 2000; i += 1) insertMemory(a!.db, `m_${i}`, `Title ${i}`, `Body ${i}`);
    const result = pull(b!, a!, publish(a!, dir));
    assert.equal(result.stored, 2000);
    assert.equal(result.materialized, 2000);
    assert.equal(revisionCount(b!.db), 2000);
    assert.equal(Number(b!.db.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n), 2000);
    assert.equal(Number(b!.db.prepare("SELECT COUNT(*) AS n FROM sync_conflicts WHERE status = 'open'").get()?.n), 0);
    assert.equal(headsOf(b!.db, `${a!.id}:m_1999`).length, 1);
  });
});

// --- Graph bounds: one over the value in "Bounds" is rejected before apply ---

test('every graph bound one over its value is rejected before apply, with the database unchanged', async () => {
  await withReplicas(1, ([replica], dir) => {
    const { db } = replica!;
    const before = counts(db);
    const cases: { name: string; code: string; write: (path: string) => void }[] = [
      {
        name: 'header bytes', code: 'header_too_long',
        write: (path) => { writeFileSync(path, `${'x'.repeat(BOUNDS.headerBytes + 1)}\n`); },
      },
      {
        name: 'line bytes', code: 'line_too_long',
        write: (path) => { writeBundle(path, SENDER, (emit) => { emit('y'.repeat(BOUNDS.lineBytes + 1)); }); },
      },
      {
        name: 'revision lines', code: 'invalid_header',
        write: (path) => { writeBundle(path, SENDER, () => undefined, { revision_lines: BOUNDS.revisionLines + 1 }); },
      },
      {
        name: 'parents per revision', code: 'invalid_revision_line',
        write: (path) => {
          const parents = Array.from({ length: BOUNDS.parentsPerRevision + 1 }, (_unused, i) => hex(i, 64));
          writeBundle(path, SENDER, (emit) => { emit(line({ origin_id: `${SENDER}:m_p`, parents, natural: ordinary(3) })); });
        },
      },
      {
        name: 'revisions per origin', code: 'revisions_per_origin',
        write: (path) => writeBundle(path, SENDER, (emit) => {
          let parent: string[] = [];
          for (let i = 0; i <= BOUNDS.revisionsPerOrigin; i += 1) {
            const next = line({ origin_id: `${SENDER}:m_deep`, author: hex(i, 32), parents: parent, natural: ordinary(4),
              head: i === BOUNDS.revisionsPerOrigin });
            parent = [String(next.revision_id)];
            emit(next);
          }
        }),
      },
    ];
    for (const bound of cases) {
      const path = join(dir, `${SENDER}.${bound.name.replace(/ /gu, '-')}.plain`);
      bound.write(path);
      assert.throws(() => applyBundle(db, SENDER, path), rejected(bound.code), bound.name);
      assert.deepEqual(counts(db), before, bound.name);
      rmSync(path, { force: true });
    }
  });
});

// --- payload integrity: a line cannot borrow another row's identity while carrying other content ---

test('a payload whose text, hashes or natural key disagree is rejected before apply, so no line can alias onto a row it does not describe', async () => {
  await withReplicas(1, ([replica], dir) => {
    const { db, id: me } = replica!;
    const victim = insertMemory(db, 'm_victim', 'Victim title', 'Victim body');
    db.exec('BEGIN IMMEDIATE'); captureLocalChanges(db, 100); db.exec('COMMIT');
    const victimOrigin = readOrigin(db, `${me}:m_victim`)!;
    const victimHead = readRevision(db, victimOrigin.selected_head!)!;
    const before = counts(db);
    const foreign = `${OTHER}:m_foreign`;
    const base: Row = { ...victimHead.payload!, id: foreign };
    const cases: { name: string; code: string; payload: Row; natural?: Row }[] = [
      { name: 'other text under the victim material hash', code: 'material_hash_mismatch', payload: { ...base, title: 'Unrelated', body: 'Unrelated body' } },
      { name: 'own material hash under the victim natural key', code: 'natural_mismatch',
        payload: { ...base, title: 'Unrelated', body: 'Unrelated body', material_hash: materialHash('Unrelated', 'Unrelated body') } },
      { name: 'a checkpoint claiming an ordinary natural key', code: 'natural_mismatch', payload: { ...base, work_id: `${OTHER}:w_x` } },
      { name: 'text on a deleted memory', code: 'redacted_memory_text', payload: { ...base, deleted_at: 5 } },
      { name: 'text on a secret memory', code: 'redacted_memory_text', payload: { ...base, sensitivity: 'secret' } },
    ];
    for (const item of cases) {
      const path = join(dir, `${OTHER}.integrity.plain`);
      writeBundle(path, OTHER, (emit) => {
        emit({ kind: 'repo', origin_id: String(base.repo_id), identity_kind: 'remote', normalized_identity: REMOTE });
        emit(line({ origin_id: foreign, author: OTHER, parents: [victimHead.revision_id], natural: item.natural ?? victimOrigin.natural, payload: item.payload }));
      });
      assert.throws(() => applyBundle(db, OTHER, path), rejected(item.code), item.name);
      assert.deepEqual(counts(db), before, item.name);
      assert.equal(db.prepare('SELECT body FROM memories WHERE id = ?').get('m_victim')?.body, 'Victim body', item.name);
      rmSync(path, { force: true });
    }
    assert.ok(victim.material);
  });
});

test('a space directory holds at most 32 replicas: the 33rd bundle stops the pull by name', async () => {
  await withHomes(1, (homes, shared) => {
    const db = openHome(homes[0]!);
    try {
      const paths = oboetePaths(homes[0]!);
      const { spaceId } = initSpace(db, paths, { directory: shared, classes: ['eligible'], now: 1 });
      const space = spaceDirectory(shared, spaceId);
      pushSpace(db, paths, { now: 2 });
      const names: string[] = [];
      for (let i = 0; i < BOUNDS.replicasPerSpace; i += 1) {
        names.push(`${hex(i, 31)}f.osb`);
        writeFileSync(join(space, names[i]!), '');
      }
      assert.equal(pullSpace(db, paths, { now: 3 }).bundles.length, BOUNDS.replicasPerSpace, 'at the bound every bundle is read');
      writeFileSync(join(space, `${'b'.repeat(32)}.osb`), '');
      assert.throws(() => pullSpace(db, paths, { now: 4 }), (error: unknown) => error instanceof SyncError
        && error.code === 'too_many_replicas' && (error.detail as { count: number }).count === BOUNDS.replicasPerSpace + 1);
      assert.equal(revisionCount(db), 0);
    } finally { db.close(); }
  });
});

test('validation time and RSS stay bounded on a long chain, a wide fan-out and a repeated merge DAG', async () => {
  await withReplicas(1, ([replica], dir) => {
    const { db } = replica!;
    // contracts/sync.md asks for a 1,000,000-line chain. This machine validates ~3,700 lines/s, so
    // a million lines needs ~270 s, far over the 90 s ceiling the bullet sets: the shapes run at the
    // largest size under 60 s with OBOETE_SYNC_HEAVY=1 and at a suite-sized fraction of it by default.
    const LINES = process.env.OBOETE_SYNC_HEAVY === undefined ? 30_000 : 180_000;
    const perOrigin = BOUNDS.revisionsPerOrigin;
    const shapes: Record<string, { produce: (emit: (value: Line) => void) => void; heads: number }> = {
      // A chain, split at the revisions-per-origin bound so the chain itself is not what is rejected.
      chain: {
        heads: Math.ceil(LINES / perOrigin),
        produce: (emit) => {
          let parents: string[] = [];
          for (let i = 0; i < LINES; i += 1) {
            if (i % perOrigin === 0) parents = [];
            const next = line({ origin_id: `${SENDER}:chain_${Math.floor(i / perOrigin)}`, author: hex(i, 32),
              parents, natural: ordinary(i - (i % perOrigin)), head: i % perOrigin === perOrigin - 1 || i === LINES - 1 });
            parents = [String(next.revision_id)];
            emit(next);
          }
        },
      },
      // Wide fan-out: one root per origin carrying the maximum sibling head count.
      fanOut: {
        heads: Math.floor(LINES / (BOUNDS.headsPerOrigin + 1)) * BOUNDS.headsPerOrigin,
        produce: (emit) => {
          for (let origin = 0; origin < Math.floor(LINES / (BOUNDS.headsPerOrigin + 1)); origin += 1) {
            const id = `${SENDER}:fan_${origin}`;
            const root = line({ origin_id: id, author: hex(origin, 32), natural: ordinary(origin), head: false });
            emit(root);
            for (let i = 0; i < BOUNDS.headsPerOrigin; i += 1) {
              emit(line({ origin_id: id, author: hex(1000 + i, 32), parents: [String(root.revision_id)], natural: ordinary(origin) }));
            }
          }
        },
      },
      // Repeated merge DAG: diamonds stacked so every third revision has two parents.
      merge: {
        heads: Math.floor(LINES / (3 * 400 + 1)),
        produce: (emit) => {
          for (let origin = 0; origin < Math.floor(LINES / (3 * 400 + 1)); origin += 1) {
            const id = `${SENDER}:dag_${origin}`;
            const natural = ordinary(origin);
            let tip = line({ origin_id: id, author: hex(origin, 32), natural, head: false });
            emit(tip);
            for (let round = 0; round < 400; round += 1) {
              const left = line({ origin_id: id, author: hex(2 * round, 32), parents: [String(tip.revision_id)], natural, head: false });
              const right = line({ origin_id: id, author: hex(2 * round + 1, 32), parents: [String(tip.revision_id)], natural, head: false });
              emit(left); emit(right);
              tip = line({ origin_id: id, author: hex(9000 + round, 32), natural, head: round === 399,
                parents: [String(left.revision_id), String(right.revision_id)] });
              emit(tip);
            }
          }
        },
      },
    };
    const measured: string[] = [];
    for (const [name, shape] of Object.entries(shapes)) {
      const path = join(dir, `${SENDER}.${name}.plain`);
      writeBundle(path, SENDER, shape.produce);
      const rssBefore = process.memoryUsage.rss();
      const started = performance.now();
      const staged = stageBundle(db, { plaintextPath: path, scratchPath: `${path}.scratch`, spaceId: SPACE, senderOriginId: SENDER });
      const elapsed = performance.now() - started;
      const rss = process.memoryUsage.rss() - rssBefore;
      try {
        // An accepted bundle proves the reader counted exactly these heads (`heads_mismatch`) and
        // parsed every line (`revisions_sha256_mismatch`), so the shape reached the validator whole.
        assert.equal(staged.header.heads, shape.heads, name);
        assert.equal(Number(staged.scratch.prepare('SELECT COUNT(*) AS n FROM lines').get()?.n), staged.header.revision_lines, name);
      } finally { staged.close(); }
      assert.ok(elapsed < 60_000, `${name} validated in ${Math.round(elapsed)} ms`);
      assert.ok(rss < 512 * 1024 * 1024, `${name} grew RSS by ${Math.round(rss / 1048576)} MiB`);
      measured.push(`${name}: ${statSync(path).size} bytes, ${Math.round(elapsed)} ms, RSS ${rss >= 0 ? '+' : ''}${Math.round(rss / 1048576)} MiB`);
      rmSync(path, { force: true });
    }
    console.log(`  graph bounds (${LINES} lines each) — ${measured.join('; ')}`);
  });
});

// --- Crash safety: the pull transaction, the push lock and the staging fence ---

test('a pull whose cursor write fails rolls back every row and the next pull re-applies the bundle', async () => {
  await withHomes(2, (homes, shared) => {
    const a = openHome(homes[0]!);
    const b = openHome(homes[1]!);
    try {
      const pathsA = oboetePaths(homes[0]!);
      const pathsB = oboetePaths(homes[1]!);
      const { keyLine: key } = initSpace(a, pathsA, { directory: shared, classes: ['eligible'], now: 1 });
      joinSpace(b, pathsB, { directory: shared, keyLine: key, classes: ['eligible'], now: 1 });
      insertMemory(a, 'm_one', 'Title', 'Body text');
      assert.equal(pushSpace(a, pathsA, { now: 10 }).outcome, 'published');
      const before = counts(b);
      // The kill lands inside step 6: rows are written, then the cursor insert never returns.
      b.exec("CREATE TRIGGER crash BEFORE INSERT ON sync_cursors BEGIN SELECT RAISE(ABORT, 'killed'); END");
      // An unexpected failure is that bundle's outcome (Pull step 7), labelled by code, never by message.
      const crashed = pullSpace(b, pathsB, { now: 20 }).bundles[0]!;
      assert.equal(crashed.outcome, 'rejected');
      assert.equal(crashed.reason, 'apply_failed:sqlite:1811', 'SQLITE_CONSTRAINT_TRIGGER, by extended result code');
      assert.deepEqual(counts(b), before, 'one transaction: the rows roll back with the cursor');
      assert.equal(Number(b.prepare('SELECT COUNT(*) AS n FROM sync_cursors').get()?.n), 0);
      b.exec('DROP TRIGGER crash');
      assert.deepEqual(pullSpace(b, pathsB, { now: 21 }).bundles.map((entry) => entry.outcome), ['applied']);
      assert.equal(b.prepare('SELECT body FROM memories').get()?.body, 'Body text');
      assert.equal(Number(b.prepare('SELECT COUNT(*) AS n FROM sync_cursors').get()?.n), 1);
    } finally { a.close(); b.close(); }
  });
});

test('a push killed while holding the space lock leaves nothing to reclaim', async () => {
  await withHomes(1, (homes, shared) => {
    const db = openHome(homes[0]!);
    try {
      const paths = oboetePaths(homes[0]!);
      const { spaceId } = initSpace(db, paths, { directory: shared, classes: ['eligible'], now: 1 });
      // The lock is a SQLite file held with BEGIN IMMEDIATE; a killed push is a connection that
      // closes without COMMIT, which is the operating system releasing it.
      const holder = new (loadSqlite().DatabaseSync)(syncPaths(paths).lock(spaceId));
      holder.exec('CREATE TABLE IF NOT EXISTS lock (id INTEGER PRIMARY KEY); BEGIN IMMEDIATE');
      assert.throws(() => pushSpace(db, paths, { now: 2 }), (error: unknown) => error instanceof SyncError && error.code === 'busy');
      assert.equal(existsSync(join(shared, 'oboete-sync')), false, 'the busy push wrote nothing');
      holder.close();
      assert.equal(withSpaceLock(paths, spaceId, () => 'taken'), 'taken');
      assert.equal(pushSpace(db, paths, { now: 3 }).outcome, 'published');
    } finally { db.close(); }
  });
});

test('the space directory receives no file before the step 4 check, even with a secret marking during encryption', async () => {
  await withHomes(2, (homes, shared) => {
    const a = openHome(homes[0]!);
    const b = openHome(homes[1]!);
    const other = openDatabase({ path: oboetePaths(homes[0]!).db, timeoutMs: 1000 }).db;
    try {
      const pathsA = oboetePaths(homes[0]!);
      const pathsB = oboetePaths(homes[1]!);
      const { spaceId, keyLine: key } = initSpace(a, pathsA, { directory: shared, classes: ['eligible'], now: 1 });
      joinSpace(b, pathsB, { directory: shared, keyLine: key, classes: ['eligible'], now: 1 });
      insertMemory(a, 'm_one', 'Title', 'Body text');
      assert.equal(pushSpace(a, pathsA, { now: 10 }).outcome, 'published');
      const space = spaceDirectory(shared, spaceId);
      const published = readdirSync(space);
      insertMemory(a, 'm_two', 'Second', 'Second body');
      let duringEncrypt: string[] = [];
      let fired = false;
      const push = pushSpace(a, pathsA, { now: 20, probe: (at) => {
        if (at !== 'after_encrypt' || fired) return;
        fired = true;
        other.prepare("UPDATE memories SET sensitivity = 'secret' WHERE id = 'm_two'").run();
        duringEncrypt = readdirSync(space);
      } });
      assert.deepEqual(duringEncrypt, published, 'encryption wrote nothing into the space directory');
      assert.equal(push.restarts, 1);
      assert.equal(push.withheld.memories, 1);
      assert.deepEqual(readdirSync(space), published, 'the rename leaves one bundle and no temporary file');
      pullSpace(b, pathsB, { now: 30 });
      const secret = `${replicaOriginId(a)}:m_two`;
      assert.equal(readOrigin(b, secret)?.local_id, null, 'no text of the row marked secret reached B');
      assert.equal(effectiveControl(b, secret).sensitivity_floor, 'secret');
    } finally { other.close(); a.close(); b.close(); }
  });
});

// --- What pull reads from the space directory, and what it refuses ---

test('pull ignores an interrupted push temporary file and every foreign name in the space directory', async () => {
  await withHomes(2, (homes, shared) => {
    const a = openHome(homes[0]!);
    const b = openHome(homes[1]!);
    try {
      const pathsA = oboetePaths(homes[0]!);
      const pathsB = oboetePaths(homes[1]!);
      const { spaceId, keyLine: key } = initSpace(a, pathsA, { directory: shared, classes: ['eligible'], now: 1 });
      joinSpace(b, pathsB, { directory: shared, keyLine: key, classes: ['eligible'], now: 1 });
      insertMemory(a, 'm_one', 'Title', 'Body text');
      pushSpace(a, pathsA, { now: 10 });
      assert.deepEqual(pullSpace(b, pathsB, { now: 20 }).bundles.map((entry) => entry.outcome), ['applied']);
      const space = spaceDirectory(shared, spaceId);
      const temporary = join(space, `${OTHER}.osb.tmp-${randomBytes(6).toString('hex')}`);
      for (const name of [temporary, join(space, 'notes.txt'), join(space, 'README'), join(space, `${OTHER}.osb.bak`),
        join(space, `${OTHER.toUpperCase()}.osb`)]) writeFileSync(name, 'not a bundle');
      assert.deepEqual(pullSpace(b, pathsB, { now: 30 }).bundles,
        [{ replica: replicaOriginId(a), outcome: 'skipped', reason: 'cursor' }]);
      assert.equal(Number(b.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n), 1);
      assert.ok(existsSync(temporary), 'pull never writes into the space directory');
    } finally { a.close(); b.close(); }
  });
});

test('a store bound reached by a local capture surfaces as a sync error under the space lock, not a crash', async () => {
  await withHomes(1, (homes, shared) => {
    const db = openHome(homes[0]!);
    try {
      const paths = oboetePaths(homes[0]!);
      initSpace(db, paths, { directory: shared, classes: ['eligible'], now: 1 });
      insertMemory(db, 'm_full', 'Title', 'Body text');
      assert.equal(pushSpace(db, paths, { now: 2 }).outcome, 'published');
      // The origin's log is at the bound (a device that has pulled that much of it): the next local edit cannot be recorded.
      const origin = db.prepare("SELECT origin_id, natural_json FROM sync_origins WHERE kind = 'memory' AND local_id = 'm_full'").get()!;
      const insert = db.prepare(`INSERT INTO sync_revisions (revision_id, origin_id, kind, author, parents_json, control_json, natural_json, payload_hash, payload_json, stored_at)
        VALUES (?, ?, 'memory', ?, '[]', '{"sensitivity_floor":"eligible","tombstone":false}', ?, NULL, NULL, 1)`);
      for (let i = 1; i < BOUNDS.revisionsPerOrigin; i += 1) insert.run(hex(i, 64), origin.origin_id, SENDER, origin.natural_json);
      db.prepare("UPDATE memories SET body = 'Edited body' WHERE id = 'm_full'").run();
      assert.throws(() => pushSpace(db, paths, { now: 3 }), (error: unknown) => error instanceof SyncError && error.code === 'revisions_per_origin');
      assert.equal(Number(db.prepare('SELECT COUNT(*) AS n FROM sync_revisions WHERE origin_id = ?').get(origin.origin_id)?.n), BOUNDS.revisionsPerOrigin, 'nothing was recorded');
    } finally { db.close(); }
  });
});

test('bundles rejected at the space level: key id, size, a payload hash and a parent of another kind', async () => {
  await withHomes(1, (homes, shared) => {
    const db = openHome(homes[0]!);
    try {
      const paths = oboetePaths(homes[0]!);
      const { spaceId } = initSpace(db, paths, { directory: shared, classes: ['eligible'], now: 1 });
      pushSpace(db, paths, { now: 2 });
      const space = spaceDirectory(shared, spaceId);
      const key = readKey(paths, spaceId);
      const put = (sender: string, produce: (emit: (value: Line) => void) => void, spaceKey: Buffer = key): void => {
        const plain = join(syncPaths(paths).staging, `${sender}.build`);
        writeBundle(plain, sender, produce, { space_id: spaceId });
        encryptBundle(spaceKey, plain, join(space, `${sender}.osb`));
        rmSync(plain, { force: true });
      };
      const wrongKey = '1'.repeat(32);
      put(wrongKey, (emit) => { emit(line({ origin_id: `${wrongKey}:m`, natural: ordinary(5) })); }, randomBytes(32));
      const badHash = '2'.repeat(32);
      put(badHash, (emit) => {
        emit(line({ origin_id: `${badHash}:m`, natural: ordinary(6), payload: { id: `${badHash}:m`, kind: 'memory' },
          payload_hash: '9'.repeat(64) }));
      });
      const badParent = '3'.repeat(32);
      put(badParent, (emit) => {
        const context = line({ origin_id: `${badParent}:c`, kind: 'context', natural: { repo: REPO_KEY, local_key: 'k' } });
        emit(context);
        emit(line({ origin_id: `${badParent}:m`, parents: [String(context.revision_id)], natural: ordinary(7) }));
      });
      const huge = '4'.repeat(32);
      writeFileSync(join(space, `${huge}.osb`), '');
      truncateSync(join(space, `${huge}.osb`), MAX_CIPHERTEXT_BYTES + 1);
      const before = counts(db);
      const outcomes = new Map(pullSpace(db, paths, { now: 3 }).bundles.map((entry) => [entry.replica, `${entry.outcome}:${String(entry.reason)}`]));
      assert.equal(outcomes.get(wrongKey), 'rejected:key_mismatch');
      assert.equal(outcomes.get(badHash), 'rejected:payload_hash_mismatch');
      assert.equal(outcomes.get(badParent), 'rejected:unresolvable_parent');
      assert.equal(outcomes.get(huge), 'rejected:oversize');
      assert.deepEqual(counts(db), before);
    } finally { db.close(); }
  });
});

// --- Envelope bounds (contracts/sync.md "Bounds": 256 MiB plaintext, 268,501,036 bytes ciphertext) ---

test('one byte over the 256 MiB plaintext bound is rejected before any ciphertext is written', async () => {
  await withHomes(1, (homes) => {
    const plain = join(homes[0]!, 'over.plain');
    const cipher = join(homes[0]!, 'over.osb');
    writeFileSync(plain, '');
    truncateSync(plain, MAX_PLAINTEXT_BYTES + 1);
    assert.throws(() => encryptBundle(randomBytes(32), plain, cipher),
      (error: unknown) => error instanceof BundleError && error.code === 'plaintext_too_large');
    assert.equal(existsSync(cipher), false);
  });
});

test('a 256 MiB plaintext round-trips to exactly the ciphertext bound',
  { skip: process.env.OBOETE_SYNC_HEAVY === undefined && 'set OBOETE_SYNC_HEAVY=1' }, async () => {
    await withHomes(1, (homes) => {
      const key = randomBytes(32);
      const plain = join(homes[0]!, 'max.plain');
      const cipher = join(homes[0]!, 'max.osb');
      const out = join(homes[0]!, 'max.out');
      writeFileSync(plain, '');
      truncateSync(plain, MAX_PLAINTEXT_BYTES);
      assert.equal(encryptBundle(key, plain, cipher).size, 268_501_036);
      assert.equal(statSync(cipher).size, MAX_CIPHERTEXT_BYTES);
      assert.equal(decryptBundle(key, cipher, out).plaintextBytes, MAX_PLAINTEXT_BYTES);
      assert.equal(statSync(out).size, MAX_PLAINTEXT_BYTES);
      for (const path of [plain, cipher, out]) rmSync(path, { force: true });
    });
  });

test('a push at exactly 256 MiB of plaintext round-trips through the space, and one byte more writes nothing',
  { skip: process.env.OBOETE_SYNC_HEAVY === undefined && 'set OBOETE_SYNC_HEAVY=1' }, async () => {
    await withHomes(2, (homes, shared) => {
      const [homeA, homeB] = homes as [string, string];
      const a = openHome(homeA);
      const b = openHome(homeB);
      try {
        const pathsA = oboetePaths(homeA);
        const pathsB = oboetePaths(homeB);
        const { keyLine, spaceId } = initSpace(a, pathsA, { directory: shared, classes: ['eligible'], now: 1 });
        joinSpace(b, pathsB, { directory: shared, keyLine, classes: ['eligible'], now: 1 });
        const BODY = 65_000;
        const insert = a.prepare(`INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash, sensitivity,
          review_state, created_at) VALUES (?, ?, 'discovery', ?, ?, '', ?, ?, 'eligible', 'reviewed', 1)`);
        const add = (index: number, body: string): void => {
          const material = createHash('sha256').update(`m${index}`).digest('hex');
          insert.run(`m_${index.toString().padStart(6, '0')}`, REPO, `Memory ${index}`, body, material, createHash('sha256').update(`c${index}`).digest('hex'));
        };
        // Measured with the clock the push will use: `produced_at` is part of the header.
        const measure = (): number => {
          a.exec('BEGIN IMMEDIATE'); captureLocalChanges(a, 10); a.exec('COMMIT');
          const out = join(homeA, 'measure.plain');
          const bytes = buildSnapshot(a, { spaceId, classes: ['eligible'], now: 10, outputPath: out }).bytes;
          rmSync(out, { force: true });
          return bytes;
        };
        // Every memory line has the same size apart from its body, so the last body is sized to land on the bound.
        for (let index = 0; index < 4_000; index += 1) add(index, 'x'.repeat(BODY));
        const before = measure();
        add(4_000, 'x'.repeat(1_000));
        const perLine = measure() - before - 1_000;
        const remaining = MAX_PLAINTEXT_BYTES - measure();
        let index = 4_001;
        for (let left = remaining; left > 0; index += 1) {
          const body = Math.min(BODY, left - perLine);
          add(index, 'x'.repeat(body));
          left -= body + perLine;
        }
        assert.equal(measure(), MAX_PLAINTEXT_BYTES, 'the plaintext sits exactly on the bound');
        const pushed = pushSpace(a, pathsA, { now: 10 });
        assert.equal(pushed.outcome, 'published');
        assert.equal(pushed.bytes, MAX_PLAINTEXT_BYTES);
        const bundle = join(spaceDirectory(shared, spaceId), `${replicaOriginId(a)}.osb`);
        assert.equal(statSync(bundle).size, MAX_CIPHERTEXT_BYTES);
        const pulled = pullSpace(b, pathsB, { now: 11 });
        assert.equal(pulled.bundles[0]!.outcome, 'applied');
        assert.equal(b.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n, index);
        assert.equal(b.prepare('SELECT SUM(LENGTH(body)) AS n FROM memories').get()?.n, a.prepare('SELECT SUM(LENGTH(body)) AS n FROM memories').get()?.n);
        // One byte more: the push fails before anything reaches the space directory.
        a.prepare("UPDATE memories SET body = body || 'y' WHERE id = 'm_000000'").run();
        const bytesBefore = statSync(bundle);
        assert.throws(() => pushSpace(a, pathsA, { now: 12 }), (error: unknown) => error instanceof SyncError && error.code === 'plaintext_too_large');
        assert.deepEqual(readdirSync(spaceDirectory(shared, spaceId)), [`${replicaOriginId(a)}.osb`]);
        assert.equal(statSync(bundle).mtimeMs, bytesBefore.mtimeMs);
        assert.deepEqual(readdirSync(syncPaths(pathsA).staging), [], 'the staging directory is clean');
      } finally { a.close(); b.close(); }
    });
  });
