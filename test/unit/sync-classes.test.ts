// US6 publish classes, the reference closure and the approval binding (contracts/sync.md
// "What a replica publishes", the approval paragraph in "Merge rules", "Verification"). Every
// case reads the plaintext lines a push writes, so "identity-only" and "no repo line" are
// observed on the wire and not inferred from the store.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { materialHash } from '../../src/db/identity.js';
import { sha256Json } from '../../src/hash.js';
import { ENTITY_REFERENCES } from '../../src/sync/format.js';
import { BundleRejected } from '../../src/sync/stage.js';
import type { SyncKind } from '../../src/sync/identity.js';
import { readOrigin } from '../../src/sync/store.js';
import {
  insertMemory, insertSource, memoryOf, publish, pull, REMOTE, REPO, revisions, withReplicas, type Replica,
} from '../helpers/sync.js';

type Line = Record<string, unknown>;
type Bundle = { header: Line; repos: Line[]; revisions: Line[]; text: string };
type Counters = { works: number; memories: number; sources: number; contexts: number; proposals: number };

const NOTHING_WITHHELD: Counters = { works: 0, memories: 0, sources: 0, contexts: 0, proposals: 0 };

/** The identity approveProjection gives a projection (src/sharing.ts). */
const projectionHash = (title: string, body: string): string => sha256Json(['personal-projection-v1', title, body]);

/** The plaintext a push writes: the header line, then repo lines and revision lines. */
function bundleOf(path: string): Bundle {
  const text = readFileSync(path, 'utf8');
  const lines = text.split('\n').filter((line) => line !== '').map((line) => JSON.parse(line) as Line);
  const [header, ...rest] = lines;
  return {
    header: header!, text,
    repos: rest.filter((line) => line.kind === 'repo'),
    revisions: rest.filter((line) => line.kind !== 'repo'),
  };
}

function withheldOf(bundle: Bundle): Counters {
  return bundle.header.withheld as Counters;
}

/** The single head line of an origin; `payload === null` is the identity-only (control) form. */
function headLine(bundle: Bundle, originId: string): Line {
  const found = bundle.revisions.filter((line) => line.origin_id === originId && line.head === true);
  assert.equal(found.length, 1, `exactly one head line for ${originId}`);
  return found[0]!;
}

/** "no bundle contains a dangling payload reference": every reference of a shipped payload ships. */
function assertNoDanglingReferences(bundle: Bundle): void {
  const shipped = new Set(bundle.revisions.filter((line) => line.payload !== null).map((line) => String(line.origin_id)));
  const repos = new Set(bundle.repos.map((line) => String(line.origin_id)));
  for (const line of bundle.revisions) {
    if (line.payload === null) continue;
    const payload = line.payload as Line;
    for (const { field, kind } of ENTITY_REFERENCES[line.kind as SyncKind]) {
      const value = payload[field];
      if (value === null || value === undefined) continue;
      const where = `${String(line.origin_id)}.${field}`;
      if (kind === 'repo') assert.ok(repos.has(String(value)), `${where} has a repo line`);
      else assert.ok(shipped.has(String(value)), `${where} -> ${String(value)} ships a payload`);
    }
  }
}

function insertContext(db: DatabaseSync, id: string, secretPaths: string | null = null): void {
  db.prepare(`INSERT INTO work_contexts (id, repo_id, local_key, root, repo_secret_paths_json, created_at, last_seen_at)
    VALUES (?, ?, ?, '/work/sync', ?, 1, 1)`).run(id, REPO, id, secretPaths);
}

function insertWork(db: DatabaseSync, id: string, contextId: string,
  extra: { sensitivity?: string; checkpoint?: string | null } = {}): void {
  db.prepare(`INSERT INTO work_items (id, repo_id, origin_context_id, purpose, purpose_sensitivity, state,
    created_at, updated_at, current_checkpoint_memory_id) VALUES (?, ?, ?, 'Ship US6', ?, 'active', 1, 1, ?)`)
    .run(id, REPO, contextId, extra.sensitivity ?? 'eligible', extra.checkpoint ?? null);
}

/** A raw event a source can point at; `waiting` is what "unfinished" means for the content filter. */
function insertRawEvent(db: DatabaseSync, id: string, classification: string, processing: string): void {
  db.prepare(`INSERT OR IGNORE INTO sessions (id, repo_id, agent, native_session_id, conversation_id, status)
    VALUES ('s_one', ?, 'claude', 'n_one', 'c_one', 'active')`).run(REPO);
  db.prepare(`INSERT INTO raw_events (id, repo_id, session_id, kind, classification_state, processing_state, captured_at)
    VALUES (?, ?, 's_one', 'prompt', ?, ?, 1)`).run(id, REPO, classification, processing);
}

function sourceOrigins(db: DatabaseSync): string[] {
  return db.prepare("SELECT origin_id FROM sync_origins WHERE kind = 'source' ORDER BY origin_id").all()
    .map((row) => String(row.origin_id));
}

// --- work purpose: a private purpose withholds the whole work, checkpoint or not ---

test('a private purpose withholds the work and its checkpoint, and ships the control revision', async () => {
  await withReplicas(1, (replicas, dir) => {
    const [a] = replicas as [Replica];
    insertContext(a.db, 'ctx');
    insertWork(a.db, 'w_one', 'ctx', { sensitivity: 'private' });
    const noCheckpoint = bundleOf(publish(a, dir, ['eligible', 'local_only']));
    assert.deepEqual(withheldOf(noCheckpoint), { ...NOTHING_WITHHELD, works: 1, contexts: 1 });
    const control = headLine(noCheckpoint, `${a.id}:w_one`);
    assert.equal(control.payload, null, 'the work ships identity-only');
    assert.deepEqual(control.control, { tombstone: false, sensitivity_floor: 'private' });
    assert.equal(noCheckpoint.repos.length, 0, 'nothing was exported from the repository');

    // With a checkpoint the work is still withheld, and the checkpoint follows it through the closure.
    insertMemory(a.db, 'm_cp', 'Checkpoint', 'Progress so far');
    a.db.prepare("UPDATE memories SET work_id = 'w_one' WHERE id = 'm_cp'").run();
    a.db.prepare("UPDATE work_items SET current_checkpoint_memory_id = 'm_cp' WHERE id = 'w_one'").run();
    const withCheckpoint = bundleOf(publish(a, dir, ['eligible', 'local_only']));
    assert.equal(withheldOf(withCheckpoint).works, 1);
    assert.equal(withheldOf(withCheckpoint).memories, 1);
    assert.equal(headLine(withCheckpoint, `${a.id}:w_one`).payload, null);
    assert.equal(headLine(withCheckpoint, `${a.id}:m_cp`).payload, null, 'an eligible checkpoint of a withheld work is withheld');

    // Selecting private is the passing side of the same gate.
    const selected = bundleOf(publish(a, dir, ['eligible', 'local_only', 'private']));
    assert.deepEqual(withheldOf(selected), NOTHING_WITHHELD);
    assert.notEqual(headLine(selected, `${a.id}:w_one`).payload, null);
    assert.notEqual(headLine(selected, `${a.id}:m_cp`).payload, null);
    assertNoDanglingReferences(selected);
  });
});

test('a context ships identity-only when the closure withheld every payload of its repository',
  async () => {
  await withReplicas(1, (replicas, dir) => {
    const [a] = replicas as [Replica];
    insertContext(a.db, 'ctx', '["/work/sync/.env"]');
    insertWork(a.db, 'w_one', 'ctx', { sensitivity: 'private' });
    insertMemory(a.db, 'm_cp', 'Checkpoint', 'Progress so far');
    a.db.prepare("UPDATE memories SET work_id = 'w_one' WHERE id = 'm_cp'").run();
    a.db.prepare("UPDATE work_items SET current_checkpoint_memory_id = 'm_cp' WHERE id = 'w_one'").run();

    // The private work and, through the closure, its eligible checkpoint are both withheld, so no
    // payload of this repository ships: `root` and `repo_secret_paths_json` must stay on the device.
    const bundle = bundleOf(publish(a, dir, ['eligible', 'local_only']));
    assert.equal(bundle.revisions.filter((line) => line.payload !== null && line.kind !== 'context').length, 0);
    assert.equal(headLine(bundle, `${a.id}:ctx`).payload, null);
    assert.deepEqual(bundle.repos, []);
    assert.equal(bundle.text.includes('.env'), false);
  });
});

// --- proposal class: the candidate's own sensitivity gates the proposal, not its memory or work ---

test('a private pending candidate on an eligible memory and work is withheld with its control revision', async () => {
  await withReplicas(1, (replicas, dir) => {
    const [a] = replicas as [Replica];
    insertContext(a.db, 'ctx');
    insertWork(a.db, 'w_one', 'ctx');
    insertMemory(a.db, 'm_origin', 'Origin', 'Origin body');
    a.db.prepare(`INSERT INTO sharing_proposals (id, origin_memory_id, origin_repo_id, origin_work_id, candidate_title,
      candidate_body, candidate_material_hash, candidate_sensitivity, source_event_ids_json, basis, state, created_at)
      VALUES ('p_one', 'm_origin', ?, 'w_one', 'Candidate', 'Candidate body', ?, 'private', '[]', 'inferred', 'pending', 1)`)
      .run(REPO, materialHash('Candidate', 'Candidate body'));

    const withoutPrivate = bundleOf(publish(a, dir, ['eligible', 'local_only']));
    assert.deepEqual(withheldOf(withoutPrivate), { ...NOTHING_WITHHELD, proposals: 1 });
    const control = headLine(withoutPrivate, `${a.id}:p_one`);
    assert.equal(control.payload, null);
    assert.deepEqual(control.control, { tombstone: false, sensitivity_floor: 'private' });
    assert.equal(withoutPrivate.text.includes('Candidate body'), false, 'no candidate text on the wire');
    // The eligible memory and work the candidate hangs off are unaffected by its class.
    assert.notEqual(headLine(withoutPrivate, `${a.id}:m_origin`).payload, null);
    assert.notEqual(headLine(withoutPrivate, `${a.id}:w_one`).payload, null);
    assertNoDanglingReferences(withoutPrivate);

    const withPrivate = bundleOf(publish(a, dir, ['eligible', 'local_only', 'private']));
    assert.deepEqual(withheldOf(withPrivate), NOTHING_WITHHELD);
    assert.equal((headLine(withPrivate, `${a.id}:p_one`).payload as Line).candidate_body, 'Candidate body');
  });
});

// --- approval binding: an approval keeps a projection only for the candidate approved here ---

test('a pulled approval binds to the local record only: a new candidate lands pending and is re-published verbatim', async () => {
  await withReplicas(3, (replicas, dir) => {
    const [a, b, c] = replicas as [Replica, Replica, Replica];
    const classes = ['eligible', 'local_only'] as const;
    insertContext(a.db, 'ctx');
    insertWork(a.db, 'w_one', 'ctx');
    insertMemory(a.db, 'm_origin', 'Origin', 'Origin body');
    const candidate0 = materialHash('Candidate zero', 'Body zero');
    a.db.prepare(`INSERT INTO sharing_proposals (id, origin_memory_id, origin_repo_id, origin_work_id, candidate_title,
      candidate_body, candidate_material_hash, candidate_sensitivity, source_event_ids_json, basis, state, created_at)
      VALUES ('p_one', 'm_origin', ?, 'w_one', 'Candidate zero', 'Body zero', ?, 'eligible', '[]', 'inferred', 'pending', 1)`)
      .run(REPO, candidate0);
    pull(b, a, publish(a, dir, classes));
    const onB = readOrigin(b.db, `${a.id}:p_one`)!.local_id!;

    // B's user approves candidate C0 here: the local record stores that candidate and scope.
    const c0 = insertMemory(a.db, 'm_c0', 'Candidate zero', 'Body zero', { content: projectionHash('Candidate zero', 'Body zero') });
    a.db.prepare(`INSERT INTO memory_visibility (id, memory_id, audience, repo_id, work_id, proposal_id, grant_kind, created_at)
      VALUES ('v_c0', 'm_c0', 'personal', NULL, NULL, 'p_one', 'proposal_approval', 2)`).run();
    b.db.prepare(`INSERT INTO sync_approvals (proposal_id, candidate_hash, projection_hash, scope_json, approved_at)
      VALUES (?, ?, ?, '{"audience":"personal"}', 5)`).run(onB, candidate0, c0.content);
    a.db.prepare(`UPDATE sharing_proposals SET state = 'approved', decision_channel = 'cli',
      projected_memory_id = 'm_c0', decided_at = 2 WHERE id = 'p_one'`).run();
    pull(b, a, publish(a, dir, classes));
    assert.deepEqual({ ...b.db.prepare('SELECT state, projected_memory_id FROM sharing_proposals').get() },
      { state: 'approved', projected_memory_id: memoryOf(b, a, 'm_c0').id }, 'the matching candidate keeps the projection');

    // B re-publishes A's approved revision verbatim: the approval is local, the revision is A's.
    const fromA = bundleOf(`${dir}/${a.id}.plain`);
    const fromB = bundleOf(publish(b, dir, classes));
    assert.deepEqual(headLine(fromB, `${a.id}:p_one`), headLine(fromA, `${a.id}:p_one`));
    assert.deepEqual(revisions(b.db, `${a.id}:p_one`).map((revision) => revision.author), [a.id, a.id]);

    // A device that never had an approval record holds the decision pending and adds no revision of its own.
    pull(c, a, `${dir}/${a.id}.plain`);
    assert.deepEqual({ ...c.db.prepare('SELECT state, projected_memory_id FROM sharing_proposals').get() },
      { state: 'pending', projected_memory_id: null });
    publish(c, dir, classes);
    assert.deepEqual(revisions(c.db, `${a.id}:p_one`).map((revision) => revision.author), [a.id, a.id]);

    // A candidate is the proposal's identity: a later revision that swaps it for C1 contradicts
    // the origin's natural key and the whole bundle is rejected before anything is stored.
    const c1 = insertMemory(a.db, 'm_c1', 'Candidate one', 'Body one');
    a.db.prepare(`UPDATE sharing_proposals SET candidate_material_hash = ?, candidate_title = 'Candidate one',
      candidate_body = 'Body one', projected_memory_id = 'm_c1', decided_at = 3 WHERE id = 'p_one'`).run(c1.material);
    const swapped = publish(a, dir, classes);
    assert.throws(() => pull(b, a, swapped), (error: unknown) => error instanceof BundleRejected && error.code === 'natural_mismatch');
    assert.deepEqual({ ...b.db.prepare('SELECT state, projected_memory_id FROM sharing_proposals').get() },
      { state: 'approved', projected_memory_id: memoryOf(b, a, 'm_c0').id }, 'B keeps the approval it holds');
    assert.equal(b.db.prepare('SELECT candidate_hash FROM sync_approvals WHERE proposal_id = ?').get(onB)?.candidate_hash,
      candidate0, 'the local approval stays bound to C0');
  });
});

// --- secret after sync: the control revision carries "now secret" and "now private" to a holder ---

test('marking a synced memory secret strips the copy, and a raise to private keeps the old text', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    const classes = ['eligible', 'local_only'] as const;
    insertMemory(a.db, 'm_sec', 'Secret later', 'Body to erase');
    insertMemory(a.db, 'm_priv', 'Private later', 'Body to keep');
    pull(b, a, publish(a, dir, classes));
    assert.equal(memoryOf(b, a, 'm_sec').body, 'Body to erase');
    assert.equal(memoryOf(b, a, 'm_priv').body, 'Body to keep');

    a.db.prepare("UPDATE memories SET sensitivity = 'secret' WHERE id = 'm_sec'").run();
    a.db.prepare("UPDATE memories SET sensitivity = 'private' WHERE id = 'm_priv'").run();
    assert.equal(a.db.prepare("SELECT review_state FROM memories WHERE id = 'm_sec'").get()?.review_state, 'imported',
      'the 0005 trigger quarantines the row the contract says it quarantines');
    const bundle = bundleOf(publish(a, dir, classes));
    assert.deepEqual(withheldOf(bundle), { ...NOTHING_WITHHELD, memories: 2 });
    assert.equal(headLine(bundle, `${a.id}:m_sec`).payload, null);
    assert.deepEqual(headLine(bundle, `${a.id}:m_sec`).control, { tombstone: false, sensitivity_floor: 'secret' });
    assert.deepEqual(headLine(bundle, `${a.id}:m_priv`).control, { tombstone: false, sensitivity_floor: 'private' });
    assert.equal(bundle.text.includes('Body to erase'), false);

    pull(b, a, `${dir}/${a.id}.plain`);
    const secret = memoryOf(b, a, 'm_sec');
    assert.equal(secret.sensitivity, 'secret');
    assert.equal(secret.title, '');
    assert.equal(secret.body, '');
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM sync_revisions WHERE origin_id = ? AND payload_json IS NOT NULL')
      .get(`${a.id}:m_sec`)?.n, 0, 'every stored payload of a secret origin is erased');
    const raised = memoryOf(b, a, 'm_priv');
    assert.equal(raised.sensitivity, 'private');
    assert.equal(raised.body, 'Body to keep', 'the raise arrives without new text');
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM sync_revisions WHERE origin_id = ? AND payload_json IS NOT NULL')
      .get(`${a.id}:m_priv`)?.n, 1, 'a private floor erases nothing');
  });
});

// --- withheld closure: every withholding reason ships control only, is counted, and leaves no dangling reference ---

test('unprocessed sources, quarantine and unselected classes withhold payloads, their works, and nothing else', async () => {
  await withReplicas(1, (replicas, dir) => {
    const [a] = replicas as [Replica];
    insertContext(a.db, 'ctx');
    insertMemory(a.db, 'm_ok', 'Shipped', 'Shipped body');
    insertMemory(a.db, 'm_quar', 'Quarantined', 'Imported body');
    a.db.prepare("UPDATE memories SET review_state = 'imported' WHERE id = 'm_quar'").run();
    insertMemory(a.db, 'm_priv', 'Unselected', 'Private body', { sensitivity: 'private' });
    insertMemory(a.db, 'm_unproc', 'Unprocessed', 'Waiting body');
    const rowid = insertSource(a.db, 'm_unproc', 'src/waiting.ts');
    insertRawEvent(a.db, 'e_waiting', 'done', 'waiting');
    a.db.prepare("UPDATE memory_sources SET raw_event_id = 'e_waiting' WHERE id = ?").run(rowid);
    insertWork(a.db, 'w_ref', 'ctx', { checkpoint: 'm_unproc' });

    const bundle = bundleOf(publish(a, dir, ['eligible', 'local_only']));
    assert.deepEqual(withheldOf(bundle), { works: 1, memories: 3, sources: 1, contexts: 0, proposals: 0 });
    for (const local of ['m_quar', 'm_priv', 'm_unproc']) {
      assert.equal(headLine(bundle, `${a.id}:${local}`).payload, null, `${local} ships identity-only`);
    }
    assert.deepEqual(headLine(bundle, `${a.id}:m_priv`).control, { tombstone: false, sensitivity_floor: 'private' });
    assert.deepEqual(headLine(bundle, `${a.id}:m_quar`).control, { tombstone: false, sensitivity_floor: 'eligible' });
    assert.equal(headLine(bundle, `${a.id}:w_ref`).payload, null, 'the work referencing a withheld memory is withheld');
    assert.equal(headLine(bundle, sourceOrigins(a.db)[0]!).payload, null, 'the source of a withheld memory is withheld');
    assert.equal(bundle.text.includes('Waiting body'), false);
    assert.equal(bundle.text.includes('src/waiting.ts'), false);
    // The one eligible memory still ships, with its repository and context.
    assert.notEqual(headLine(bundle, `${a.id}:m_ok`).payload, null);
    assert.equal(bundle.repos.length, 1);
    assertNoDanglingReferences(bundle);

    // The raw event finishing is the only change needed to release the whole group.
    a.db.prepare("UPDATE raw_events SET processing_state = 'processed' WHERE id = 'e_waiting'").run();
    const released = bundleOf(publish(a, dir, ['eligible', 'local_only']));
    assert.deepEqual(withheldOf(released), { ...NOTHING_WITHHELD, memories: 2 });
    assert.notEqual(headLine(released, `${a.id}:w_ref`).payload, null);
    assertNoDanglingReferences(released);

    // Selecting the private class ships the private memory itself; the quarantined one stays withheld.
    const widened = bundleOf(publish(a, dir, ['eligible', 'local_only', 'private']));
    assert.deepEqual(withheldOf(widened), { ...NOTHING_WITHHELD, memories: 1 });
    assert.equal((headLine(widened, `${a.id}:m_priv`).payload as Line).body, 'Private body');
    assert.equal(headLine(widened, `${a.id}:m_quar`).payload, null);
  });
});

test('a pulled approval whose projection differs from the local record does not bind', async () => {
  await withReplicas(2, (replicas, dir) => {
    const [a, b] = replicas as [Replica, Replica];
    const classes = ['eligible', 'local_only'] as const;
    insertContext(a.db, 'ctx');
    insertWork(a.db, 'w_one', 'ctx');
    insertMemory(a.db, 'm_origin', 'Origin', 'Origin body');
    const candidate0 = materialHash('Candidate zero', 'Body zero');
    a.db.prepare(`INSERT INTO sharing_proposals (id, origin_memory_id, origin_repo_id, origin_work_id, candidate_title,
      candidate_body, candidate_material_hash, candidate_sensitivity, source_event_ids_json, basis, state, created_at)
      VALUES ('p_one', 'm_origin', ?, 'w_one', 'Candidate zero', 'Body zero', ?, 'eligible', '[]', 'inferred', 'pending', 1)`)
      .run(REPO, candidate0);
    pull(b, a, publish(a, dir, classes));
    const onB = readOrigin(b.db, `${a.id}:p_one`)!.local_id!;
    // A approves with projection P1 (personal from birth: approveProjection grants in the same
    // transaction); B's user approved the same candidate but as projection P0.
    insertMemory(a.db, 'm_p1', 'Candidate zero', 'Body zero variant', { content: projectionHash('Candidate zero', 'Body zero variant') });
    a.db.prepare(`INSERT INTO memory_visibility (id, memory_id, audience, repo_id, work_id, proposal_id, grant_kind, created_at)
      VALUES ('v_pers', 'm_p1', 'personal', NULL, NULL, 'p_one', 'proposal_approval', 2)`).run();
    a.db.prepare(`UPDATE sharing_proposals SET state = 'approved', decision_channel = 'cli', projected_memory_id = 'm_p1',
      decided_at = 2 WHERE id = 'p_one'`).run();
    const p0 = insertMemory(a.db, 'm_p0', 'Candidate zero', 'Body zero', { content: projectionHash('Candidate zero', 'Body zero') });
    a.db.prepare("UPDATE memories SET deleted_at = 1 WHERE id = 'm_p0'").run();
    b.db.prepare(`INSERT INTO sync_approvals (proposal_id, candidate_hash, projection_hash, scope_json, approved_at)
      VALUES (?, ?, ?, '{"audience":"personal"}', 5)`).run(onB, candidate0, p0.content);
    a.db.prepare("DELETE FROM memories WHERE id = 'm_p0'").run();
    pull(b, a, publish(a, dir, classes));
    assert.deepEqual({ ...b.db.prepare('SELECT state, projected_memory_id FROM sharing_proposals').get() },
      { state: 'pending', projected_memory_id: null }, 'a different projection does not bind');
    assert.equal(readOrigin(b.db, `${a.id}:m_p1`)!.withheld_reason, 'approval_missing', 'the unapproved projection is withheld');

    // The approved projection binds; the personal grant (the only scope a proposal grant can carry,
    // 0006 CHECK) is written once the proposal is approved here.
    insertMemory(a.db, 'm_p0', 'Candidate zero', 'Body zero', { content: projectionHash('Candidate zero', 'Body zero') });
    a.db.prepare(`INSERT INTO memory_visibility (id, memory_id, audience, repo_id, work_id, proposal_id, grant_kind, created_at)
      VALUES ('v_pers0', 'm_p0', 'personal', NULL, NULL, 'p_one', 'proposal_approval', 4)`).run();
    a.db.prepare("UPDATE sharing_proposals SET projected_memory_id = 'm_p0', decided_at = 3 WHERE id = 'p_one'").run();
    pull(b, a, publish(a, dir, classes));
    assert.deepEqual({ ...b.db.prepare('SELECT state, projected_memory_id FROM sharing_proposals').get() },
      { state: 'approved', projected_memory_id: memoryOf(b, a, 'm_p0').id }, 'the matching projection binds');
    const projection = memoryOf(b, a, 'm_p0').id as string;
    assert.deepEqual(b.db.prepare('SELECT audience, proposal_id FROM memory_visibility WHERE memory_id = ?').all(projection)
      .map((row) => ({ ...row })), [{ audience: 'personal', proposal_id: onB }]);
    assert.equal(b.db.prepare('SELECT COUNT(*) AS n FROM memory_visibility').get()?.n, 1, 'the unapproved projection still has no grant here');
  });
});

// --- context payloads: identity-only until the repository ships something ---

test('a repository with nothing shipped keeps its contexts identity-only and emits no repo line', async () => {
  await withReplicas(1, (replicas, dir) => {
    const [a] = replicas as [Replica];
    insertContext(a.db, 'ctx', '["/work/sync/.env"]');
    insertMemory(a.db, 'm_priv', 'Unselected', 'Private body', { sensitivity: 'private' });

    const withheld = bundleOf(publish(a, dir, ['eligible', 'local_only']));
    assert.deepEqual(withheldOf(withheld), { ...NOTHING_WITHHELD, memories: 1, contexts: 1 });
    assert.equal(headLine(withheld, `${a.id}:ctx`).payload, null);
    assert.deepEqual(withheld.repos, [], 'no repo line for a repository nothing was exported from');
    assert.equal(withheld.text.includes('.env'), false, 'repo_secret_paths_json stays on the device');
    assert.equal(withheld.text.includes('/work/sync'), false, 'root stays on the device');

    // The first shipped memory of the repository brings both the context payload and the repo line.
    insertMemory(a.db, 'm_ok', 'Shipped', 'Shipped body');
    const shipped = bundleOf(publish(a, dir, ['eligible', 'local_only']));
    assert.deepEqual(withheldOf(shipped), { ...NOTHING_WITHHELD, memories: 1 });
    const context = headLine(shipped, `${a.id}:ctx`).payload as Line;
    assert.equal(context.root, '/work/sync');
    assert.equal(context.repo_secret_paths_json, '["/work/sync/.env"]');
    assert.equal(shipped.repos.length, 1);
    assert.equal(shipped.repos[0]!.identity_kind, 'remote');
    assert.equal(shipped.repos[0]!.normalized_identity, REMOTE);
    assertNoDanglingReferences(shipped);
  });
});

// --- work-only repository: a work and its context need no memory to ship ---

test('a repository with an eligible work, its context and no memories ships both payloads', async () => {
  await withReplicas(1, (replicas, dir) => {
    const [a] = replicas as [Replica];
    insertContext(a.db, 'ctx');
    insertWork(a.db, 'w_one', 'ctx');

    const bundle = bundleOf(publish(a, dir, ['eligible']));
    assert.deepEqual(withheldOf(bundle), NOTHING_WITHHELD);
    const work = headLine(bundle, `${a.id}:w_one`).payload as Line;
    assert.equal(work.purpose, 'Ship US6');
    assert.equal(work.current_checkpoint_memory_id, null);
    assert.equal((headLine(bundle, `${a.id}:ctx`).payload as Line).local_key, 'ctx');
    assert.equal(bundle.repos.length, 1, 'the work alone brings the repo line');
    assert.equal(bundle.revisions.filter((line) => line.kind === 'memory').length, 0);
    assertNoDanglingReferences(bundle);
  });
});

// --- sources: identity by content, tombstones for removed rows, purged evidence still ships ---

test('a re-inserted source keeps its origin, a removed one ships a tombstone, and a purged raw event still ships', async () => {
  await withReplicas(1, (replicas, dir) => {
    const [a] = replicas as [Replica];
    insertMemory(a.db, 'm_one', 'Sourced', 'Sourced body');
    const first = insertSource(a.db, 'm_one', 'src/a.ts');
    insertSource(a.db, 'm_one', 'src/keep.ts');
    const before = bundleOf(publish(a, dir, ['eligible']));
    const origins = sourceOrigins(a.db);
    assert.equal(origins.length, 2);
    const origin = origins.find((id) => (headLine(before, id).payload as Line).citation_value === 'src/a.ts')!;
    assert.equal(revisions(a.db, origin).length, 1);

    // A row deleted and re-inserted identically is the same source: its id is its content, not its rowid.
    a.db.prepare('DELETE FROM memory_sources WHERE id = ?').run(first);
    const second = insertSource(a.db, 'm_one', 'src/a.ts');
    assert.notEqual(second, first, 'a fresh rowid');
    const reinserted = bundleOf(publish(a, dir, ['eligible']));
    assert.deepEqual(sourceOrigins(a.db), origins, 'the re-inserted row keeps the origin it had');
    assert.equal(revisions(a.db, origin).length, 1, 'no revision for an unchanged source');
    assert.deepEqual(headLine(reinserted, origin).control, { tombstone: false, sensitivity_floor: 'eligible' });

    // A removed source ships a tombstone under the same origin; its sibling is untouched.
    a.db.prepare('DELETE FROM memory_sources WHERE id = ?').run(second);
    const removed = bundleOf(publish(a, dir, ['eligible']));
    assert.equal(revisions(a.db, origin).length, 2);
    assert.equal((headLine(removed, origins.find((id) => id !== origin)!).payload as Line).citation_value, 'src/keep.ts');
    const tombstone = headLine(removed, origin);
    assert.deepEqual(tombstone.control, { tombstone: true, sensitivity_floor: 'eligible' });
    assert.equal(tombstone.payload, null);
    assert.equal(tombstone.payload_hash, null);
    assert.deepEqual(withheldOf(removed), NOTHING_WITHHELD, 'a tombstone is not a withheld payload');

    // A summary whose raw event was purged keeps its evidence-less source rows and still ships.
    insertMemory(a.db, 'm_sum', 'Summary', 'Summary body');
    a.db.prepare("UPDATE memories SET type = 'session_summary' WHERE id = 'm_sum'").run();
    const purged = insertSource(a.db, 'm_sum', 'src/purged.ts');
    a.db.prepare("UPDATE memory_sources SET raw_event_id = 'e_purged' WHERE id = ?").run(purged);
    const summary = bundleOf(publish(a, dir, ['eligible']));
    assert.deepEqual(withheldOf(summary), NOTHING_WITHHELD);
    assert.notEqual(headLine(summary, `${a.id}:m_sum`).payload, null);
    const shipped = sourceOrigins(a.db).filter((id) => !origins.includes(id));
    assert.equal(shipped.length, 1);
    const row = headLine(summary, shipped[0]!).payload as Line;
    assert.equal(row.raw_event_id, 'e_purged', 'the dangling raw event id travels as provenance');
    assert.equal(row.evidence, null);
    assert.equal(row.citation_value, 'src/purged.ts');
    assertNoDanglingReferences(summary);
  });
});
