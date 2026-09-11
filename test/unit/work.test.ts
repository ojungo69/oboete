import assert from 'node:assert/strict';
import { test } from 'node:test';

import { bindCapturedWork, completeWork, currentWorkBinding } from '../../src/work.js';
import { NOW, REPO_ID, seedRepo, seedSession, withOpened } from '../helpers/observer-fixture.js';

test('an ordinary capture after work completion replaces the current binding', async () => {
  await withOpened((db) => {
    seedRepo(db);
    seedSession(db, 'sess1');
    const input = { repoId: REPO_ID, root: '/work/uploader', contextKey: 'fixture', sessionId: 'sess1',
      sourceId: 'first-prompt', kind: 'prompt', content: 'Inspect the uploader.', inputSource: 'user',
      sensitivity: 'local_only' as const, admissible: true, capturedAt: NOW };
    const first = bindCapturedWork(db, input);
    const previous = currentWorkBinding(db, input.sessionId)!;
    assert.ok(previous.work_id);
    assert.equal(completeWork(db, { repoId: REPO_ID, workId: previous.work_id, now: NOW + 1 }), true);
    assert.equal(currentWorkBinding(db, input.sessionId)?.id, first.bindingId);

    const next = bindCapturedWork(db, { ...input, sourceId: 'next-prompt',
      content: 'Check the retry behavior.', capturedAt: NOW + 2 });
    assert.notEqual(next.bindingId, first.bindingId);
    assert.equal(next.closedPrevious, true);
    const current = currentWorkBinding(db, input.sessionId)!;
    assert.equal(current.id, next.bindingId);
    assert.equal(current.context_id, previous.context_id);
    assert.ok(current.work_id);
    assert.notEqual(current.work_id, previous.work_id);
    assert.equal(db.prepare('SELECT closed_at FROM work_bindings WHERE id = ?').get(first.bindingId)?.closed_at, NOW + 2);
    assert.equal(db.prepare('SELECT state FROM work_items WHERE id = ?').get(previous.work_id)?.state, 'completed');
    assert.equal(db.prepare('SELECT state FROM work_items WHERE id = ?').get(current.work_id)?.state, 'active');
  });
});
