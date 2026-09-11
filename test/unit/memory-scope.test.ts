import assert from 'node:assert/strict';
import test from 'node:test';
import type { DatabaseSync } from 'node:sqlite';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';

import { getMemory, grantVisibility, listMemories, memoryScope } from '../../src/db/queries.js';
import { filterReadOutput } from '../../src/privacy/provenance.js';
import { detectSync } from '../../src/privacy/detect.js';
import { contentHash, materialHash, memoryIdFor } from '../../src/db/identity.js';
import { NOW, REPO_ID, seedRepo, seedSession, withOpened } from '../helpers/observer-fixture.js';
import { insertSession, withFixture, type Fixture } from '../helpers/inject-fixture.js';
import { seedWorkBinding } from '../helpers/work.js';
import { applyObservations, type ApplyInput } from '../../src/observer/apply.js';
import { buildObserverRequest } from '../../src/observer/request.js';
import { loadDestinationRules } from '../../src/privacy/egress.js';
import { claimLease } from '../../src/worker/lease.js';
import type { RawEventRow, SessionRow } from '../../src/worker/batches.js';
import { observation, output } from '../helpers/observer-fixture.js';
import { adoptKnowledge, decideSharing, sharingStatus } from '../../src/sharing.js';
import { runGet, runSearch, runShare } from '../../src/memories-cli.js';
import { resolveRepoIdentity } from '../../src/repo-identity.js';

test('a new ordinary memory without an explicit audience is unavailable to every reader', async () => {
  await withOpened((db) => {
    seedRepo(db);
    db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, content_hash,
      sensitivity, review_state) VALUES ('ungranted', ?, 'discovery', 'Unshared',
      'Only the origin work may use this fact.', 'ungranted-hash', 'eligible', 'reviewed')`).run(REPO_ID);
    for (const history of [false, true]) {
      const scope = memoryScope(db, { repoId: REPO_ID, destination: 'injection', history });
      assert.equal(getMemory(db, 'ungranted', scope), null);
      assert.deepEqual(listMemories(db, scope, { limit: 10 }), []);
    }
  });
});

function memory(db: DatabaseSync, id: string, repoId: string, body = 'Reply in Japanese.'): void {
  db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, content_hash,
    sensitivity, review_state) VALUES (?, ?, 'discovery', 'Preference', ?, ?, 'eligible', 'reviewed')`)
    .run(id, repoId, body, `hash:${id}`);
}

function proposal(db: DatabaseSync, repoId: string, workId: string, projectedId: string): void {
  memory(db, `origin:${projectedId}`, repoId);
  memory(db, projectedId, repoId);
  grantVisibility(db, `origin:${projectedId}`, { audience: 'work', repoId, workId }, 'observer', NOW);
  const material = materialHash('Personal preference', 'Reply in Japanese.');
  db.prepare("UPDATE memories SET title = 'Personal preference', material_hash = ? WHERE id = ?").run(material, projectedId);
  db.prepare(`INSERT INTO sharing_proposals (id, origin_memory_id, origin_repo_id, origin_work_id,
    candidate_title, candidate_body, candidate_material_hash, candidate_sensitivity,
    source_event_ids_json, basis, state, decision_channel, projected_memory_id, created_at, decided_at)
    VALUES (?, ?, ?, ?, 'Personal preference', 'Reply in Japanese.', ?, 'eligible',
      '[]', 'inferred', 'approved', 'cli', ?, 1, 2)`)
    .run(`proposal:${projectedId}`, `origin:${projectedId}`, repoId, workId, material, projectedId);
  grantVisibility(db, projectedId, { audience: 'personal', proposalId: `proposal:${projectedId}` }, 'proposal_approval', NOW);
}

test('the same scope separates work, project and approved personal knowledge including history', async () => {
  await withOpened((db) => {
    seedRepo(db);
    seedSession(db, 'origin');
    seedWorkBinding(db, 'origin');
    const work = `fixture-work:${REPO_ID}`;
    db.prepare(`INSERT INTO work_items (id, repo_id, origin_context_id, created_at, updated_at)
      VALUES ('other-work', ?, ?, 1, 1)`).run(REPO_ID, `fixture-context:${REPO_ID}`);
    db.exec("INSERT INTO repos (id, identity_kind, normalized_identity) VALUES ('foreign', 'common_dir', '/foreign')");
    for (const [id, repoId] of [['work', REPO_ID], ['other-work-memory', REPO_ID], ['project', REPO_ID],
      ['foreign-project', 'foreign'], ['secret', REPO_ID], ['imported', REPO_ID], ['prior', REPO_ID]] as const) {
      memory(db, id, repoId);
      grantVisibility(db, id, id === 'work' || id === 'other-work-memory'
        ? { audience: 'work', repoId, workId: id === 'work' ? work : 'other-work' }
        : { audience: 'project', repoId }, 'observer', NOW);
    }
    proposal(db, REPO_ID, work, 'personal');
    proposal(db, REPO_ID, work, 'pending');
    db.exec(`UPDATE sharing_proposals SET state = 'pending', decision_channel = NULL,
      projected_memory_id = NULL, decided_at = NULL WHERE id = 'proposal:pending';
      UPDATE memories SET sensitivity = 'secret' WHERE id = 'secret';
      UPDATE memories SET review_state = 'imported' WHERE id = 'imported';
      UPDATE memories SET valid_to = 3 WHERE id = 'prior'`);
    const ids = (repoId: string, workId: string | null, history = false) => listMemories(db,
      memoryScope(db, { repoId, workId, history, destination: 'injection' }), { limit: 50 }).map((row) => row.id).sort();
    assert.deepEqual(ids(REPO_ID, work), ['origin:pending', 'origin:personal', 'personal', 'project', 'work']);
    assert.deepEqual(ids(REPO_ID, 'other-work'), ['other-work-memory', 'personal', 'project']);
    assert.deepEqual(ids(REPO_ID, null), ['personal', 'project']);
    assert.deepEqual(ids('foreign', null), ['foreign-project', 'personal']);
    assert.deepEqual(ids(REPO_ID, work, true), ['origin:pending', 'origin:personal', 'personal', 'prior', 'project', 'work']);
    db.exec("UPDATE destination_rules SET allowed = 1 WHERE destination = 'injection' AND sensitivity = 'secret'");
    assert.deepEqual(ids(REPO_ID, work), ['origin:pending', 'origin:personal', 'personal', 'project', 'work'], 'a rule cannot expose a secret');
  });
});

test('personal output contains the statement without origin metadata or source fields', async () => {
  await withFixture(async ({ db, identity, repo, paths }) => {
    insertSession({ db, identity, repo, paths }, { id: 'source-session', agent: 'claude' });
    proposal(db, identity.id, `fixture-work:${identity.id}`, 'personal');
    const row = getMemory(db, 'personal', memoryScope(db, { repoId: identity.id, destination: 'injection' }));
    assert.ok(row);
    const output = await filterReadOutput(db, { repoId: identity.id, bindingId: 'fixture-binding:source-session' },
      [{ ...row, sources: [{ raw_event_id: 'origin-private-id' }], extra_path: '/origin/private/path' }], []);
    assert.equal(output.memories[0]?.body, 'Reply in Japanese.');
    for (const field of ['repo_id', 'work_id', 'source_session_id', 'source_batch_id', 'sources', 'extra_path', 'citations_head']) {
      assert.equal(Object.hasOwn(output.memories[0], field), false, field);
    }
    db.exec("UPDATE memories SET deleted_at = 3 WHERE id = 'personal'");
    assert.equal(getMemory(db, 'personal', memoryScope(db, { repoId: identity.id, destination: 'injection' })), null);
  });
});

for (const change of ['grant', 'approval', 'projection'] as const) test(`an asynchronous read withholds a changed personal ${change}`, async () => {
  await withFixture(async (fixture) => {
    const { db, identity } = fixture;
    insertSession(fixture, { id: 'source-session', agent: 'claude' });
    proposal(db, identity.id, `fixture-work:${identity.id}`, 'personal');
    memory(db, 'replacement', identity.id);
    const row = getMemory(db, 'personal', memoryScope(db, { repoId: identity.id, destination: 'injection' }));
    assert.ok(row);
    let changed = false;
    const output = await filterReadOutput(db, { repoId: identity.id, bindingId: 'fixture-binding:source-session' }, [row], [], async (input) => {
      changed = true;
      if (change === 'grant') db.exec("DELETE FROM memory_visibility WHERE memory_id = 'personal'");
      else if (change === 'approval') db.exec("UPDATE sharing_proposals SET state = 'rejected', projected_memory_id = NULL WHERE id = 'proposal:personal'");
      else db.exec("UPDATE sharing_proposals SET projected_memory_id = 'replacement' WHERE id = 'proposal:personal'");
      return detectSync(input);
    });
    assert.equal(changed, true);
    assert.deepEqual(output.memories, []);
  });
});

function sharingInput(fixture: Fixture, options: { content?: string; kind?: string; source?: string;
  truncated?: boolean; title?: string; body?: string; visibility?: 'work' | 'project' | 'personal_proposal' } = {}) {
  const { db, identity, repo } = fixture;
  insertSession(fixture, { id: 'sharing-session', agent: 'claude' });
  const token = claimLease(db, { pid: 1, now: NOW });
  assert.ok(token);
  db.prepare(`INSERT INTO observation_batches (id, repo_id, session_id, through_event_id, destination,
    state, owner_token, work_binding_id) VALUES ('sharing-batch', ?, 'sharing-session', 'sharing-source',
      'local_observer', 'running', ?, 'fixture-binding:sharing-session')`).run(identity.id, token);
  db.prepare(`INSERT INTO raw_events (id, repo_id, session_id, kind, content, truncated, payload_json,
    sensitivity, classification_state, captured_at, batch_id, work_binding_id)
    VALUES ('sharing-source', ?, 'sharing-session', ?, ?, ?, ?, 'eligible', 'done', ?,
      'sharing-batch', 'fixture-binding:sharing-session')`).run(identity.id, options.kind ?? 'prompt',
    options.content ?? 'Personal preference: Reply in Japanese.', options.truncated ? 1 : 0,
    JSON.stringify({ input_source: options.source ?? 'user', capture_root: repo, source_paths: [] }), NOW);
  const rows = db.prepare("SELECT * FROM raw_events WHERE id = 'sharing-source'").all() as unknown as RawEventRow[];
  const request = buildObserverRequest({ rows, repoId: identity.id, destination: 'local_observer', turns: [], nearby: [],
    session: db.prepare("SELECT * FROM sessions WHERE id = 'sharing-session'").get() as unknown as SessionRow,
    rules: loadDestinationRules(db) });
  assert.equal(request.coverage.length, 1);
  db.prepare("UPDATE raw_events SET processing_hash = ? WHERE id = 'sharing-source'").run(request.coverage[0].sourceHash);
  const input: ApplyInput = { batchId: 'sharing-batch', repoId: identity.id, sessionId: 'sharing-session', rows,
    output: output(observation({ visibility: options.visibility ?? 'personal_proposal',
      title: options.title ?? 'The uploader retries three times', body: options.body ?? 'Reply in Japanese.',
      source_event_ids: ['sharing-source'] })), nearby: [], fallbackReason: null, coverage: request.coverage,
    detect: async (text) => detectSync({ text, repoRoot: repo, paths: [], secretPaths: [] }), now: NOW };
  const location = { repoId: identity.id, bindingId: 'fixture-binding:sharing-session', workId: `fixture-work:${identity.id}` };
  return { input, token, location };
}

test('a fully accounted direct personal declaration creates one source-free projection', async () => {
  await withFixture(async (fixture) => {
    const { db } = fixture;
    const { input, token, location } = sharingInput(fixture);
    const before = db.prepare('SELECT * FROM work_items').all();
    const result = await applyObservations(db, token, input);
    assert.equal(result.leaseLost, false);
    const saved = db.prepare('SELECT * FROM sharing_proposals').get();
    assert.equal(saved?.state, 'approved');
    assert.equal(saved?.decision_channel, 'automatic_direct');
    assert.equal(saved?.basis, 'direct_declaration');
    const projectedId = String(saved?.projected_memory_id);
    assert.notEqual(projectedId, result.applied[0].memoryId);
    assert.deepEqual(db.prepare('SELECT * FROM memory_sources WHERE memory_id = ?').all(projectedId), []);
    assert.deepEqual(db.prepare('SELECT audience FROM memory_visibility WHERE memory_id = ?').all(projectedId)
      .map((row) => row.audience), ['personal']);
    const projected = getMemory(db, projectedId, memoryScope(db, { repoId: 'another-project', destination: 'injection' }));
    assert.equal(projected?.body, 'Reply in Japanese.');
    assert.deepEqual(db.prepare('SELECT * FROM work_items').all(), before, 'sharing never completes or rewrites work');
    assert.deepEqual(await decideSharing(db, location, { id: String(saved?.id), decision: 'approve', channel: 'cli', now: NOW + 1 }),
      { id: saved?.id, state: 'approved', projectedMemoryId: projectedId });
    assert.equal(db.prepare('SELECT COUNT(*) AS n FROM sharing_proposals').get()?.n, 1);
  });
});

test('direct sharing uses the exact prepared statement when origin identity reuses normalized content', async () => {
  await withFixture(async (fixture) => {
    const { db, identity } = fixture;
    const { input, token, location } = sharingInput(fixture);
    const title = input.output!.observations[0].title;
    const material = materialHash(title, 'reply in japanese.');
    const hash = contentHash(identity.id, material);
    const id = memoryIdFor(hash);
    db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, material_hash, content_hash, sensitivity, review_state)
      VALUES (?, ?, 'discovery', ?, 'reply in japanese.', ?, ?, 'eligible', 'reviewed')`).run(id, identity.id, title, material, hash);
    grantVisibility(db, id, { audience: 'work', repoId: identity.id, workId: location.workId }, 'observer', NOW);
    const result = await applyObservations(db, token, input);
    assert.equal(result.applied[0].memoryId, id);
    const proposal = db.prepare('SELECT state, candidate_body, projected_memory_id FROM sharing_proposals').get();
    assert.equal(proposal?.state, 'approved');
    assert.equal(proposal?.candidate_body, 'Reply in Japanese.');
    assert.equal(db.prepare('SELECT body FROM memories WHERE id = ?').get(proposal!.projected_memory_id)?.body, 'Reply in Japanese.');
  });
});

test('an exact direct declaration cannot auto-approve an earlier normalized-equivalent inferred candidate', async () => {
  await withFixture(async (fixture) => {
    const { input, token } = sharingInput(fixture, { content: 'Personal preference: Use FOO.', title: 'Personal preference', body: 'use foo.' });
    input.output!.observations.push({ ...input.output!.observations[0], body: 'Use FOO.' });
    await applyObservations(fixture.db, token, input);
    const proposals = fixture.db.prepare('SELECT candidate_body, basis, state FROM sharing_proposals ORDER BY candidate_body').all();
    assert.deepEqual(proposals.map((row) => ({ ...row })), [
      { candidate_body: 'Use FOO.', basis: 'direct_declaration', state: 'approved' },
      { candidate_body: 'use foo.', basis: 'inferred', state: 'pending' },
    ]);
    assert.equal(fixture.db.prepare(`SELECT m.body FROM memories m JOIN memory_visibility v ON v.memory_id = m.id
      WHERE v.audience = 'personal'`).get()?.body, 'Use FOO.');
  });
});

for (const [name, options] of [
  ['tool output', { kind: 'tool_result' }], ['assistant', { kind: 'last_assistant_message' }],
  ['RPC', { source: 'rpc' }], ['extension', { source: 'extension' }],
  ['quoted', { content: '> Personal preference: Reply in Japanese.' }],
  ['truncated', { truncated: true }], ['paraphrased', { body: 'Answer using Japanese.' }],
  ['multiline', { content: 'Personal preference: Reply in Japanese.\nAlso inspect uploads.' }],
  ['prefix newline', { content: 'Personal preference:\nReply in Japanese.' }],
  ['trailing newline', { content: 'Personal preference: Reply in Japanese.\n' }],
  ['Unicode line separator', { content: 'Personal preference: Reply in Japanese.\u2028' }],
] as const) test(`${name} cannot authorize its own personal sharing`, async () => {
  await withFixture(async (fixture) => {
    const { input, token } = sharingInput(fixture, options);
    await applyObservations(fixture.db, token, input);
    assert.equal(fixture.db.prepare('SELECT state FROM sharing_proposals').get()?.state, 'pending');
    assert.equal(fixture.db.prepare("SELECT COUNT(*) AS n FROM memory_visibility WHERE audience = 'personal'").get()?.n, 0);
  });
});

test('pending sharing is repository-scoped, human approval is idempotent and work is unchanged', async () => {
  await withFixture(async (fixture) => {
    const { db } = fixture;
    const { input, token, location } = sharingInput(fixture, { source: 'rpc' });
    await applyObservations(db, token, input);
    const before = db.prepare('SELECT * FROM work_items').all();
    const status = await sharingStatus(db, location);
    assert.equal(status.proposals.length, 1);
    const id = status.proposals[0].id;
    assert.equal(await decideSharing(db, { ...location, repoId: 'another-project' }, { id, decision: 'approve', channel: 'cli', now: NOW }), null);
    const approved = await decideSharing(db, location, { id, decision: 'approve', channel: 'cli', now: NOW });
    assert.equal(approved?.state, 'approved');
    assert.deepEqual(await decideSharing(db, location, { id, decision: 'approve', channel: 'cli', now: NOW }), approved);
    assert.equal(await decideSharing(db, location, { id, decision: 'reject', channel: 'viewer', now: NOW }), null);
    assert.deepEqual(db.prepare('SELECT * FROM work_items').all(), before);
    db.prepare('UPDATE memories SET deleted_at = ? WHERE id = ?').run(NOW, approved!.projectedMemoryId);
    await decideSharing(db, location, { id, decision: 'approve', channel: 'cli', now: NOW + 1 });
    assert.equal(db.prepare('SELECT deleted_at FROM memories WHERE id = ?').get(approved!.projectedMemoryId)?.deleted_at, NOW);
  });
});

test('inferred sharing preserves the exact title and body the human approves', async () => {
  await withFixture(async (fixture) => {
    const { input, token, location } = sharingInput(fixture, { source: 'rpc', title: 'Preferred test framework', body: 'Vitest.' });
    await applyObservations(fixture.db, token, input);
    const status = await sharingStatus(fixture.db, location);
    assert.equal(status.proposals[0]?.candidate_title, 'Preferred test framework');
    assert.equal(status.proposals[0]?.candidate_body, 'Vitest.');
    const approved = await decideSharing(fixture.db, location, { id: status.proposals[0].id, decision: 'approve', channel: 'cli', now: NOW });
    assert.ok(approved?.projectedMemoryId);
    const projected = fixture.db.prepare('SELECT title, body FROM memories WHERE id = ?').get(approved.projectedMemoryId);
    assert.deepEqual({ ...projected }, { title: 'Preferred test framework', body: 'Vitest.' });
  });
});

test('older pending proposals remain reachable after fifty newer decisions', async () => {
  await withFixture(async (fixture) => {
    const { db } = fixture;
    const { input, token, location } = sharingInput(fixture, { source: 'rpc' });
    await applyObservations(db, token, input);
    const pendingId = String(db.prepare("SELECT id FROM sharing_proposals WHERE state = 'pending'").get()?.id);
    for (let index = 0; index < 50; index++) db.prepare(`INSERT INTO sharing_proposals (
      id, origin_memory_id, origin_repo_id, origin_work_id, candidate_title, candidate_body,
      candidate_material_hash, candidate_sensitivity, source_event_ids_json, basis, state,
      decision_channel, created_at, decided_at)
      SELECT ?, origin_memory_id, origin_repo_id, origin_work_id, candidate_title, candidate_body,
        candidate_material_hash, candidate_sensitivity, source_event_ids_json, basis, 'rejected',
        'cli', ?, ? FROM sharing_proposals WHERE id = ?`)
      .run(`decided-${index}`, NOW + index + 1, NOW + index + 1, pendingId);
    const status = await sharingStatus(db, location);
    assert.equal(status.proposals[0]?.id, pendingId);
    assert.equal(status.proposals.length, 50);
    assert.equal(status.hasMore, true);
  });
});

test('older pending proposal remains reachable when fifty newer pending origins are deleted', async () => {
  await withFixture(async (fixture) => {
    const { db, identity } = fixture;
    const { input, token, location } = sharingInput(fixture, { source: 'rpc' });
    await applyObservations(db, token, input);
    const pendingId = String(db.prepare("SELECT id FROM sharing_proposals WHERE state = 'pending'").get()?.id);
    const workId = `fixture-work:${identity.id}`;
    for (let index = 0; index < 50; index++) {
      const originId = `deleted-origin-${index}`;
      const body = `Preference ${index}.`;
      memory(db, originId, identity.id, body);
      db.prepare('UPDATE memories SET title = ? WHERE id = ?').run('Personal preference', originId);
      grantVisibility(db, originId, { audience: 'work', repoId: identity.id, workId }, 'observer', NOW);
      db.prepare(`INSERT INTO sharing_proposals (id, origin_memory_id, origin_repo_id, origin_work_id,
        candidate_title, candidate_body, candidate_material_hash, candidate_sensitivity, source_event_ids_json,
        basis, state, created_at) VALUES (?, ?, ?, ?, 'Personal preference', ?, ?, 'eligible', '[]', 'inferred', 'pending', ?)`)
        .run(`deleted-proposal-${index}`, originId, identity.id, workId, body, materialHash('Personal preference', body), NOW + index + 1);
      db.prepare('UPDATE memories SET deleted_at = ? WHERE id = ?').run(NOW, originId);
    }
    const status = await sharingStatus(db, location);
    assert.deepEqual(status.proposals.map((candidate) => candidate.id), [pendingId]);
    assert.equal(status.hasMore, false);
  });
});

for (const statements of [['Use FOO.', 'use foo.'], ['Use  tabs.', 'Use tabs.'], ['Use Ａ.', 'Use A.']] as const) {
  test(`personal approval preserves distinct exact strings: ${JSON.stringify(statements)}`, async () => {
    await withFixture(async (fixture) => {
      const { db } = fixture;
      const otherRoot = join(fixture.paths.home, 'second-project');
      mkdirSync(otherRoot);
      const other = { ...fixture, repo: otherRoot, identity: resolveRepoIdentity(otherRoot) };
      db.prepare('INSERT INTO repos (id, identity_kind, normalized_identity, display_root) VALUES (?, ?, ?, ?)')
        .run(other.identity.id, other.identity.identityKind, other.identity.normalizedIdentity, other.repo);
      const ids = [];
      for (const [index, current] of [fixture, other].entries()) {
        const title = 'Build preference', body = statements[index];
        insertSession(current, { id: `exact-session-${index}`, agent: 'claude' });
        const workId = `fixture-work:${current.identity.id}`;
        const originId = `exact-origin-${index}`, id = `exact-proposal-${index}`;
        memory(db, originId, current.identity.id, body);
        db.prepare('UPDATE memories SET title = ? WHERE id = ?').run(title, originId);
        grantVisibility(db, originId, { audience: 'work', repoId: current.identity.id, workId }, 'observer', NOW);
        db.prepare(`INSERT INTO sharing_proposals (id, origin_memory_id, origin_repo_id, origin_work_id,
          candidate_title, candidate_body, candidate_material_hash, candidate_sensitivity, source_event_ids_json,
          basis, state, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, 'eligible', '[]', 'inferred', 'pending', ?)`)
          .run(id, originId, current.identity.id, workId, title, body, materialHash(title, body), NOW);
        const approved = await decideSharing(db, { repoId: current.identity.id, bindingId: `fixture-binding:exact-session-${index}` },
          { id, decision: 'approve', channel: 'cli', now: NOW });
        assert.ok(approved?.projectedMemoryId);
        const projected = db.prepare('SELECT title, body FROM memories WHERE id = ?').get(approved.projectedMemoryId);
        assert.deepEqual({ ...projected }, { title, body });
        assert.equal(db.prepare("SELECT proposal_id FROM memory_visibility WHERE memory_id = ? AND audience = 'personal'")
          .get(approved.projectedMemoryId)?.proposal_id, id);
        ids.push(approved.projectedMemoryId);
      }
      assert.notEqual(ids[0], ids[1]);
    });
  });
}

for (const changed of ['candidate', 'origin', 'grant', 'decision', 'credentials'] as const) test(`approval cancels when ${changed} changes during detection`, async () => {
  await withFixture(async (fixture) => {
    const { db } = fixture;
    const { input, token, location } = sharingInput(fixture, { source: 'rpc' });
    const result = await applyObservations(db, token, input);
    const id = String(db.prepare('SELECT id FROM sharing_proposals').get()?.id);
    const prior = process.env.OBOETE_OPENROUTER_API_KEY;
    let acted = false;
    try {
      const approved = await decideSharing(db, location, { id, decision: 'approve', channel: 'cli', now: NOW }, async (text) => {
        if (!acted) {
          acted = true;
          if (changed === 'candidate') db.prepare('UPDATE sharing_proposals SET candidate_body = ? WHERE id = ?').run('A changed preference.', id);
          if (changed === 'origin') db.prepare('UPDATE memories SET deleted_at = ? WHERE id = ?').run(NOW, result.applied[0].memoryId);
          if (changed === 'grant') db.prepare('DELETE FROM memory_visibility WHERE memory_id = ?').run(result.applied[0].memoryId);
          if (changed === 'decision') await decideSharing(db, location, { id, decision: 'reject', channel: 'viewer', now: NOW });
          if (changed === 'credentials') process.env.OBOETE_OPENROUTER_API_KEY = 'Reply in Japanese.';
        }
        return detectSync(text);
      });
      assert.equal(acted, true);
      assert.equal(approved, null);
      assert.equal(db.prepare("SELECT COUNT(*) AS n FROM memory_visibility WHERE audience = 'personal'").get()?.n, 0);
    } finally {
      if (prior === undefined) delete process.env.OBOETE_OPENROUTER_API_KEY;
      else process.env.OBOETE_OPENROUTER_API_KEY = prior;
    }
  });
});

test('project adoption preserves source evidence and work state and rejects a wrong work', async () => {
  await withFixture(async (fixture) => {
    const { db } = fixture;
    const { input, token, location } = sharingInput(fixture, { visibility: 'work' });
    const result = await applyObservations(db, token, input);
    const id = result.applied[0].memoryId!;
    const before = { sources: db.prepare('SELECT * FROM memory_sources ORDER BY id').all(), work: db.prepare('SELECT * FROM work_items').all() };
    assert.equal(await adoptKnowledge(db, { ...location, workId: 'another-work' }, id, NOW), false);
    assert.equal(await adoptKnowledge(db, location, id, NOW), true);
    assert.equal(await adoptKnowledge(db, location, id, NOW), true);
    assert.deepEqual(db.prepare('SELECT * FROM memory_sources ORDER BY id').all(), before.sources);
    assert.deepEqual(db.prepare('SELECT * FROM work_items').all(), before.work);
    assert.deepEqual(db.prepare('SELECT audience FROM memory_visibility WHERE memory_id = ? ORDER BY audience').all(id)
      .map((row) => row.audience), ['project', 'work']);
  });
});

test('accepted generation retains its closed, completed origin work', async () => {
  await withFixture(async (fixture) => {
    const { db } = fixture;
    const { input, token, location } = sharingInput(fixture, { visibility: 'work' });
    db.prepare('UPDATE work_bindings SET closed_at = ? WHERE id = ?').run(NOW, location.bindingId);
    db.prepare("UPDATE work_items SET state = 'completed', completed_at = ? WHERE id = ?").run(NOW, location.workId);
    const before = db.prepare('SELECT * FROM work_items').all();
    const result = await applyObservations(db, token, input);
    assert.equal(result.applied.length, 1);
    assert.equal(db.prepare('SELECT work_id FROM memory_visibility WHERE memory_id = ?').get(result.applied[0].memoryId)?.work_id, location.workId);
    assert.deepEqual(db.prepare('SELECT * FROM work_items').all(), before);
  });
});

test('CLI sharing decisions and personal reads preserve the public projection shape', async () => {
  await withFixture(async (fixture) => {
    const { input, token } = sharingInput(fixture, { source: 'rpc' });
    await applyObservations(fixture.db, token, input);
    let stdout = '';
    const runtime = { cwd: fixture.repo, now: () => NOW, writeOut: (text: string) => { stdout += text; }, writeError: () => {} };
    assert.equal(await runShare(['status', '--json'], runtime), 0);
    const id = (JSON.parse(stdout) as { proposals: { id: string }[] }).proposals[0].id;
    assert.equal(await runShare(['approve', id, '--repo', 'elsewhere'], runtime), 2);
    stdout = '';
    assert.equal(await runShare(['approve', id, '--json'], runtime), 0);
    const projectionId = String(JSON.parse(stdout).projectedMemoryId);
    for (const history of [false, true]) {
      stdout = '';
      assert.equal(await runGet([projectionId, '--json', ...(history ? ['--history'] : [])], runtime), 0);
      const shown = JSON.parse(stdout);
      assert.equal(shown.body, 'Reply in Japanese.');
      for (const field of ['repo_id', 'sources', 'source_session_id', 'source_batch_id', 'work_id']) assert.equal(Object.hasOwn(shown, field), false);
    }
    stdout = '';
    assert.equal(await runSearch(['Japanese'], runtime), 0);
    assert.match(stdout, /Reply in Japanese/);
    assert.match(stdout, /relevance score/);
  });
});

for (const denied of ['partial range', 'duplicate citation', 'lost lease', 'detector failure'] as const) test(`personal sharing stays unapproved after ${denied}`, async () => {
  await withFixture(async (fixture) => {
    const { input, token } = sharingInput(fixture);
    if (denied === 'partial range') {
      input.coverage![0].state = 'partial';
      input.coverage![0].end -= 1;
      input.coverage![0].text = input.coverage![0].text.slice(0, -1);
    }
    if (denied === 'duplicate citation') input.output!.observations[0].source_event_ids.push('sharing-source');
    if (denied === 'detector failure') input.detect = async () => ({ ok: false, reason: 'detector_error' });
    const result = await applyObservations(fixture.db, denied === 'lost lease' ? 'foreign-token' : token, input);
    assert.equal(result.leaseLost, denied === 'lost lease');
    assert.equal(fixture.db.prepare("SELECT COUNT(*) AS n FROM memory_visibility WHERE audience = 'personal'").get()?.n, 0);
  });
});
