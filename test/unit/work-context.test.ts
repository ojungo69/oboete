import assert from 'node:assert/strict';
import { grantVisibility } from '../../src/db/queries.js';
import { spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { devNull } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';

import { listSpool } from '../../src/spool.js';
import { chooseWork, readWorkSelection, workStatus } from '../../src/work.js';
import { resolveRepoIdentity } from '../../src/repo-identity.js';
import type { ObserverInput } from '../../src/observer/contract.js';
import { filterReadOutput, readSourcePrivacy } from '../../src/privacy/provenance.js';
import { detectSync } from '../../src/privacy/detect.js';
import { openDatabase } from '../../src/db/open.js';
import { getMemory, memoryScope } from '../../src/db/queries.js';
import { excludeSecretSource } from '../../src/worker/batches.js';

import { NOW, captureEndedSession, cleanEnv, openAiResponse, providerOutput, runObserveForFixture, toggleDatabase, withFixture, writeConfig, type Fixture } from '../helpers/observe.js';

function git(cwd: string, ...args: string[]): void {
  const result = spawnSync('git', ['-C', cwd, ...args], { encoding: 'utf8', env: {
    ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('GIT_'))),
    GIT_CONFIG_GLOBAL: devNull, GIT_CONFIG_SYSTEM: devNull, GIT_TERMINAL_PROMPT: '0',
  } });
  assert.equal(result.status, 0, result.stderr);
}

function worktrees(fixture: Fixture): { main: string; linked: string } {
  const main = join(fixture.home, 'main');
  const linked = join(fixture.home, 'linked');
  mkdirSync(main);
  git(main, 'init', '--quiet', '--initial-branch=main');
  git(main, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
    'commit', '--quiet', '--allow-empty', '-m', 'Fixture');
  git(main, 'worktree', 'add', '--quiet', '-b', 'fixture-work', linked);
  return { main, linked };
}

function binding(fixture: Fixture, native: string) {
  return fixture.withDb((db) => db.prepare(`SELECT b.id, b.work_id, b.context_id, b.candidates_json,
    w.purpose, w.state, c.repo_id FROM work_bindings b
    JOIN sessions s ON s.id = b.session_id JOIN work_contexts c ON c.id = b.context_id
    LEFT JOIN work_items w ON w.id = b.work_id
    WHERE s.native_session_id = ? AND b.closed_at IS NULL`).get(native)!);
}

for (const outputKind of ['update', 'add', 'checkpoint'] as const) for (const revoke of ['path rule', 'source exclusion'] as const) {
  test(`provided memory context retains its privacy after ${outputKind} and ${revoke}`, async () => {
    await withFixture(async (fixture) => {
      fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'context-inheritance-fixture-key' });
      writeConfig(fixture, 'openrouter');
      await captureEndedSession(fixture, { cwd: fixture.home, sessionId: 'context-origin', prompts: ['Record the uploader policy.'],
        tools: [{ id: 'context-file', path: 'protected/upload.ts' }] });
      const originId = fixture.withDb((db) => String(db.prepare("SELECT id FROM raw_events WHERE kind = 'tool_call'").get()!.id));
      const detail = 'The restricted uploader endpoint requires a separate manual approval.';
      let parentId = '';
      const respond: typeof fetch = async (_url, options) => {
        const messages = (JSON.parse(String(options?.body)) as { messages: { role: string; content: string }[] }).messages;
        const request = JSON.parse(messages.find((message) => message.role === 'user')!.content) as ObserverInput;
        const output = providerOutput(request.events[0].id);
        output.observations[0].source_event_ids = request.events.map((row) => row.id);
        output.observations[0].title = parentId === '' ? 'Uploader policy' : 'Updated uploader policy';
        output.observations[0].body = detail + (parentId === '' ? '' : ' Keep it for the follow-up.');
        if (parentId !== '') {
          assert.ok(request.nearby.some((row) => row.id === parentId), 'the old memory was actually sent');
          if (outputKind === 'update') output.observations[0].classification = { decision: 'update', target: parentId, reason: 'The uploader policy was refined.' };
          if (outputKind === 'checkpoint') output.checkpoint = { decision: 'replace', purpose: 'Finish the uploader',
            constraints: [detail], decisions: [], outstanding: ['Verify the new input.'],
            source_event_ids: request.events.map((row) => row.id), reason: 'Progress updated from supplied context.' };
        }
        return openAiResponse(output);
      };
      assert.equal(await runObserveForFixture(fixture, { fetch: respond }), 0);
      parentId = fixture.withDb((db) => String(db.prepare("SELECT id FROM memories WHERE title = 'Uploader policy'").get()!.id));
      await captureEndedSession(fixture, { cwd: fixture.home, sessionId: 'context-update', prompts: ['Update the uploader policy for the next verification.'] });
      assert.equal(await runObserveForFixture(fixture, { fetch: respond }), 0);
      const current = binding(fixture, 'context-update');
      const db = openDatabase({ path: fixture.paths.db, timeoutMs: 1000 }).db;
      try {
        const id = outputKind === 'checkpoint' ? String(db.prepare('SELECT current_checkpoint_memory_id FROM work_items WHERE id = ?').get(current.work_id)?.current_checkpoint_memory_id)
          : String(db.prepare("SELECT id FROM memories WHERE title = 'Updated uploader policy'").get()?.id);
        const memory = getMemory(db, id, memoryScope(db, { repoId: String(current.repo_id), workId: String(current.work_id), destination: 'injection' }));
        assert.ok(memory);
        assert.ok(memory.body?.includes(detail));
        const location = { repoId: String(current.repo_id), bindingId: String(current.id), home: fixture.paths.home };
        assert.equal((await filterReadOutput(db, location, [memory], [])).memories.length, 1);
        if (revoke === 'path rule') writeFileSync(join(fixture.home, '.oboete.toml'), '[privacy]\nsecret_paths = ["protected/**"]\n');
        else excludeSecretSource(db, originId, NOW);
        assert.deepEqual((await filterReadOutput(db, location, [memory], [])).memories, []);
        if (revoke === 'source exclusion') assert.equal(db.prepare('SELECT sensitivity FROM memories WHERE id = ?').get(id)?.sensitivity, 'secret');
      } finally { db.close(); }
    });
  });
}

test('removed contexts preserve saved path restrictions and reject unknown or replaced origins', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const { main, linked } = worktrees(fixture);
    writeFileSync(join(linked, '.oboete.toml'), '[privacy]\nsecret_paths = ["protected/**"]\n');
    await fixture.capture('SessionStart', { cwd: linked, session_id: 'origin-policy', source: 'startup' });
    await fixture.capture('SessionStart', { cwd: main, session_id: 'resume-policy', source: 'startup' });
    const origin = binding(fixture, 'origin-policy');
    const current = binding(fixture, 'resume-policy');
    const identity = resolveRepoIdentity(main);
    const selected = fixture.withDb((db) => chooseWork(db, { repoId: identity.id, contextKey: identity.worktreeKey,
      bindingId: String(current.id), workId: String(origin.work_id), now: NOW }))!;
    git(main, 'worktree', 'remove', '--force', linked);
    const location = { repoId: identity.id, bindingId: selected.id, home: fixture.paths.home };
    const context = { root: linked, contextId: String(origin.context_id), paths: [join(linked, 'protected/config.ts')] };
    const policy = fixture.withDb((db) => readSourcePrivacy(db, location, context, fixture.env));
    assert.ok(policy);
    const detected = await detectSync({ ...policy.detector, text: 'The retained progress body.' });
    assert.equal(detected.ok && detected.sensitivity, 'secret');
    assert.equal(fixture.withDb((db) => readSourcePrivacy(db, location, { ...context, contextId: null }, fixture.env)), null);
    mkdirSync(linked);
    git(linked, 'init', '--quiet', '--initial-branch=unrelated');
    assert.equal(fixture.withDb((db) => readSourcePrivacy(db, location, context, fixture.env)), null);
    assert.equal(fixture.withDb((db) => db.prepare('SELECT root FROM work_contexts WHERE id = ?').get(origin.context_id)?.root), linked);
  });
});

test('recreating a worktree at the same path starts a new context without replacing old privacy rules', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const { main, linked } = worktrees(fixture);
    writeFileSync(join(linked, '.oboete.toml'), '[privacy]\nsecret_paths = ["protected/**"]\n');
    await fixture.capture('SessionStart', { cwd: linked, session_id: 'old-generation', source: 'startup' });
    await fixture.capture('UserPromptSubmit', { cwd: linked, session_id: 'old-generation', prompt_id: 'old-work', prompt: 'Finish the upload retry.' });
    const before = binding(fixture, 'old-generation');
    const oldKey = resolveRepoIdentity(linked).worktreeKey;
    git(main, 'worktree', 'remove', '--force', linked);
    git(main, 'worktree', 'add', '--quiet', linked, 'fixture-work');
    assert.notEqual(resolveRepoIdentity(linked).worktreeKey, oldKey);
    await fixture.capture('SessionStart', { cwd: linked, session_id: 'new-generation', source: 'startup' });
    const after = binding(fixture, 'new-generation');
    assert.notEqual(after.context_id, before.context_id);
    assert.notEqual(after.work_id, before.work_id);
    assert.equal(fixture.withDb((db) => db.prepare('SELECT repo_secret_paths_json FROM work_contexts WHERE id = ?')
      .get(before.context_id)?.repo_secret_paths_json), '["protected/**"]');
  });
});

test('unverified directory identity retains native-session work without inferring cross-session continuity', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const missing = join(fixture.home, 'directory-not-present');
    const identity = resolveRepoIdentity(missing);
    assert.equal(identity.worktreeKey, null);
    await fixture.capture('SessionStart', { cwd: missing, session_id: 'unknown-one', source: 'startup' });
    const first = binding(fixture, 'unknown-one');
    await fixture.capture('SessionStart', { cwd: missing, session_id: 'unknown-one', source: 'resume' });
    assert.equal(binding(fixture, 'unknown-one').id, first.id);
    await fixture.capture('SessionStart', { cwd: missing, session_id: 'unknown-two', source: 'startup' });
    assert.notEqual(binding(fixture, 'unknown-two').work_id, first.work_id);
    fixture.withDb((db) => {
      assert.equal(readWorkSelection(db, { repoId: identity.id, contextKey: null }).workId, null);
      assert.equal(workStatus(db, { repoId: identity.id, contextKey: null }, true).works.length, 2);
    });
  });
});

test('moving the same worktree preserves its binding and checks old paths under its verified new root', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const { main, linked } = worktrees(fixture);
    writeFileSync(join(linked, '.oboete.toml'), '[privacy]\nsecret_paths = ["protected/**"]\n');
    await fixture.capture('SessionStart', { cwd: linked, session_id: 'moved-work', source: 'startup' });
    const before = binding(fixture, 'moved-work');
    const moved = join(fixture.home, 'moved');
    git(main, 'worktree', 'move', linked, moved);
    writeFileSync(join(moved, '.oboete.toml'), `[privacy]\nsecret_paths = [${JSON.stringify(join(moved, 'protected/**'))}]\n`);
    await fixture.capture('SessionStart', { cwd: moved, session_id: 'moved-work', source: 'resume' });
    assert.equal(binding(fixture, 'moved-work').id, before.id);
    const context = { root: linked, contextId: String(before.context_id), paths: [join(linked, 'protected/config.ts')] };
    const policy = fixture.withDb((db) => readSourcePrivacy(db, { repoId: String(before.repo_id), bindingId: String(before.id) }, context, fixture.env));
    assert.ok(policy);
    assert.equal(policy.detector.repoRoot, moved);
    const result = await detectSync({ ...policy.detector, text: 'Retained upload work.' });
    assert.equal(result.ok && result.sensitivity, 'secret');
    assert.equal(context.root, linked);
  });
});

test('another selected work cannot authorize a removed work purpose through its live binding', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const { main, linked } = worktrees(fixture);
    await fixture.capture('SessionStart', { cwd: linked, session_id: 'old-label', source: 'startup' });
    await fixture.capture('UserPromptSubmit', { cwd: linked, session_id: 'old-label', prompt_id: 'old-purpose', prompt: 'Private branch investigation.' });
    await fixture.capture('SessionStart', { cwd: main, session_id: 'different-work', source: 'startup' });
    const old = binding(fixture, 'old-label');
    const current = binding(fixture, 'different-work');
    git(main, 'worktree', 'remove', linked);
    const db = openDatabase({ path: fixture.paths.db, timeoutMs: 1000 }).db;
    try {
      for (const purged of [false, true]) {
        if (purged) db.prepare('DELETE FROM raw_events WHERE id = (SELECT purpose_source_event_id FROM work_items WHERE id = ?)').run(old.work_id);
        const { works: labels } = await filterReadOutput(db, { repoId: String(current.repo_id), bindingId: String(current.id),
          workId: String(current.work_id), home: fixture.paths.home }, [], [{ id: String(old.work_id), purpose: String(old.purpose) }]);
        assert.equal(labels[0].purpose, null, `source purged: ${purged}`);
        assert.equal(labels[0].id, old.work_id);
      }
    } finally { db.close(); }
  });
});

test('current absolute rules apply to an original path even while the other source worktree remains live', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const { main, linked } = worktrees(fixture);
    await fixture.capture('SessionStart', { cwd: linked, session_id: 'live-origin', source: 'startup' });
    await fixture.capture('SessionStart', { cwd: main, session_id: 'live-reader', source: 'startup' });
    const origin = binding(fixture, 'live-origin');
    const current = binding(fixture, 'live-reader');
    writeFileSync(join(main, '.oboete.toml'), `[privacy]\nsecret_paths = [${JSON.stringify(join(main, 'protected/**'))}]\n`);
    const policy = fixture.withDb((db) => readSourcePrivacy(db, { repoId: String(current.repo_id), bindingId: String(current.id) },
      { root: linked, contextId: String(origin.context_id), paths: [join(linked, 'protected/config.ts')] }, fixture.env));
    assert.ok(policy);
    const checked = await detectSync({ ...policy.detector, text: 'Source from the other live worktree.' });
    assert.equal(checked.ok && checked.sensitivity, 'secret');
  });
});

for (const condition of ['clean', 'current rules', 'origin rules', 'removed origin', 'replaced origin'] as const) {
  test(`ambiguous work labels require both current and origin proof: ${condition}`, async () => {
    await withFixture(async (fixture) => {
      writeConfig(fixture, 'none');
      const { main, linked } = worktrees(fixture);
      await fixture.capture('SessionStart', { cwd: linked, session_id: 'choice-origin', source: 'startup' });
      await fixture.capture('UserPromptSubmit', { cwd: linked, session_id: 'choice-origin', prompt_id: 'purpose',
        prompt: 'Inspect the linked work purpose 6821.' });
      await fixture.capture('SessionStart', { cwd: main, session_id: 'choice-current', source: 'startup' });
      const origin = binding(fixture, 'choice-origin'), current = binding(fixture, 'choice-current');
      fixture.withDb((db) => {
        db.prepare("UPDATE work_bindings SET work_id = NULL, reason = 'ambiguous', candidates_json = ? WHERE id = ?")
          .run(JSON.stringify([origin.work_id, current.work_id]), current.id);
        db.prepare(`UPDATE raw_events SET payload_json = json_set(payload_json, '$.source_paths', json(?))
          WHERE id = (SELECT purpose_source_event_id FROM work_items WHERE id = ?)`)
          .run(JSON.stringify([join(linked, 'protected/source.ts')]), origin.work_id);
      });
      if (condition === 'current rules' || condition === 'origin rules') {
        const root = condition === 'current rules' ? main : linked;
        writeFileSync(join(root, '.oboete.toml'), `[privacy]\nsecret_paths = [${JSON.stringify(join(root, 'protected/**'))}]\n`);
      }
      if (condition === 'removed origin' || condition === 'replaced origin') git(main, 'worktree', 'remove', '--force', linked);
      if (condition === 'replaced origin') {
        mkdirSync(linked);
        git(linked, 'init', '--quiet', '--initial-branch=unrelated');
      }
      const db = openDatabase({ path: fixture.paths.db, timeoutMs: 1000 }).db;
      try {
        const labels = await filterReadOutput(db, { repoId: String(current.repo_id), bindingId: String(current.id), home: fixture.home }, [],
          [{ id: String(origin.work_id), purpose: String(origin.purpose) }]);
        assert.equal(labels.works[0].purpose, condition === 'clean' ? origin.purpose : null);
      } finally { db.close(); }
      const injected = await fixture.capture('SessionStart', { cwd: main, session_id: 'choice-current', source: 'startup', model: 'claude-opus-5[1m]' });
      assert.match(injected.stdout ?? '', /Select which work/);
      assert.ok((injected.stdout ?? '').includes(String(origin.work_id)));
      if (condition === 'clean') assert.match(injected.stdout ?? '', /linked work purpose 6821/);
      else assert.doesNotMatch(injected.stdout ?? '', /linked work purpose 6821/);
    });
  });
}

test('an inferred work becomes withheld if another active purpose appears during reading', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const { main } = worktrees(fixture);
    await fixture.capture('SessionStart', { cwd: main, session_id: 'inferred', source: 'startup' });
    const current = binding(fixture, 'inferred');
    const identity = resolveRepoIdentity(main);
    const db = openDatabase({ path: fixture.paths.db, timeoutMs: 1000 }).db;
    try {
      db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, content_hash, sensitivity, work_id, provenance_complete)
        VALUES ('inferred-checkpoint', ?, 'session_summary', 'First work progress', 'Verify the first work.', 'inferred', 'eligible', ?, 1)`)
        .run(identity.id, current.work_id);
      grantVisibility(db, 'inferred-checkpoint', { audience: 'work', repoId: identity.id, workId: String(current.work_id) }, 'observer', 1);
      db.prepare('INSERT INTO memory_sources (memory_id, capture_root, source_paths_json, source_context_id) VALUES (?, ?, ?, ?)')
        .run('inferred-checkpoint', main, '[]', current.context_id);
      db.prepare('UPDATE work_items SET current_checkpoint_memory_id = ? WHERE id = ?').run('inferred-checkpoint', current.work_id);
      const selected = readWorkSelection(db, { repoId: identity.id, contextKey: identity.worktreeKey });
      assert.equal(selected.workId, current.work_id);
      const result = await filterReadOutput(db, { repoId: identity.id, bindingId: null, workId: selected.workId,
        repoRoot: main, contextKey: identity.worktreeKey, home: fixture.paths.home },
      [{ id: 'inferred-checkpoint', body: 'Verify the first work.' }], [], async (input) => {
        const checked = await detectSync(input);
        db.prepare(`INSERT OR IGNORE INTO work_items (id, repo_id, origin_context_id, purpose, created_at, updated_at)
          VALUES ('newly-active', ?, ?, 'A separate purpose', 1, 1)`).run(identity.id, current.context_id);
        return checked;
      });
      assert.equal(result.memories.length, 0);
      assert.equal(readWorkSelection(db, { repoId: identity.id, contextKey: identity.worktreeKey }).choices.length, 2);
    } finally { db.close(); }
  });
});

test('processing an accepted source after a worktree move retains complete original provenance', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'moved-source-fixture-key' });
    writeConfig(fixture, 'openrouter');
    const { main, linked } = worktrees(fixture);
    await fixture.capture('SessionStart', { cwd: linked, session_id: 'before-move', source: 'startup' });
    await fixture.capture('UserPromptSubmit', { cwd: linked, session_id: 'before-move', prompt_id: 'before', prompt: 'Finish verifying the uploader.' });
    await fixture.capture('SessionEnd', { cwd: linked, session_id: 'before-move', reason: 'exit' });
    const origin = binding(fixture, 'before-move');
    const moved = join(fixture.home, 'processing-root');
    git(main, 'worktree', 'move', linked, moved);
    await fixture.capture('SessionStart', { cwd: moved, session_id: 'after-move', source: 'startup' });
    const states: string[] = [];
    const respond: typeof fetch = async (_url, options) => {
      const messages = (JSON.parse(String(options?.body)) as { messages: { role: string; content: string }[] }).messages;
      const input = JSON.parse(messages.find((message) => message.role === 'user')!.content) as ObserverInput;
      states.push(input.checkpoint_context.state);
      return openAiResponse({ ...providerOutput(input.events[0].id), checkpoint: { decision: 'replace', purpose: 'Verify the uploader',
        constraints: [], decisions: [], outstanding: ['Check the deployment.'], source_event_ids: input.events.map((row) => row.id), reason: 'Retained progress.' } });
    };
    assert.equal(await runObserveForFixture(fixture, { fetch: respond }), 0);
    fixture.withDb((db) => {
      const memory = db.prepare('SELECT m.* FROM work_items w JOIN memories m ON m.id = w.current_checkpoint_memory_id WHERE w.id = ?').get(origin.work_id)!;
      assert.ok(memory);
      assert.equal(memory.provenance_complete, 1);
      assert.equal(db.prepare('SELECT capture_root FROM memory_sources WHERE memory_id = ? AND raw_event_id IS NULL').get(memory.id)?.capture_root, linked);
    });
    await fixture.capture('UserPromptSubmit', { cwd: moved, session_id: 'after-move', prompt_id: 'after', prompt: 'Continue deployment verification.' });
    await fixture.capture('SessionEnd', { cwd: moved, session_id: 'after-move', reason: 'exit' });
    assert.equal(await runObserveForFixture(fixture, { fetch: respond }), 0);
    assert.deepEqual(states, ['none', 'provided']);
  });
});

test('explicit continuation survives Git integration and a removed worktree without completing outstanding work', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'removed-worktree-fixture-key' });
    writeConfig(fixture, 'openrouter');
    const { main, linked } = worktrees(fixture);
    await fixture.capture('SessionStart', { cwd: main, session_id: 'parent', source: 'startup' });
    await fixture.capture('SessionStart', { cwd: linked, session_id: 'branch-work', source: 'startup' });
    await fixture.capture('UserPromptSubmit', { cwd: linked, session_id: 'branch-work', prompt_id: 'purpose',
      prompt: 'Keep deployment outstanding after merging the retry fix.' });
    await fixture.capture('SessionEnd', { cwd: linked, session_id: 'branch-work', reason: 'exit' });
    const branch = binding(fixture, 'branch-work');
    const checkpoint = { decision: 'replace' as const, purpose: 'Reliable upload deployment', constraints: ['Preserve public callers.'],
      decisions: ['The retry implementation is ready.'], outstanding: ['Deploy and verify the live timeout.'],
      source_event_ids: ['placeholder'], reason: 'Progress recorded.' };
    let firstId = '';
    const continuationContexts: string[] = [];
    const respond: typeof fetch = async (_url, options) => {
      const messages = (JSON.parse(String(options?.body)) as { messages: { role: string; content: string }[] }).messages;
      const input = JSON.parse(messages.find((message) => message.role === 'user')!.content) as ObserverInput;
      if (firstId !== '') continuationContexts.push(input.checkpoint_context.state);
      const output = { ...providerOutput(input.events[0].id), checkpoint: input.checkpoint_context.state === 'withheld'
        ? { decision: 'unchanged' as const, source_event_ids: input.events.map((event) => event.id), reason: 'The parent is unavailable.' } : {
        ...checkpoint, source_event_ids: input.events.map((event) => event.id),
        outstanding: firstId === '' ? checkpoint.outstanding : ['Verify the live timeout after deployment.'],
      } };
      return openAiResponse(output);
    };
    assert.equal(await runObserveForFixture(fixture, { fetch: respond }), 0);
    firstId = fixture.withDb((db) => String(db.prepare('SELECT current_checkpoint_memory_id FROM work_items WHERE id = ?').get(branch.work_id)?.current_checkpoint_memory_id));
    assert.ok(firstId.startsWith('m_'));
    git(linked, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet', '--allow-empty', '-m', 'Retry fixture');
    git(main, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'merge', '--quiet', '--no-ff', 'fixture-work', '-m', 'Merge retry fixture');
    git(main, 'worktree', 'remove', linked);
    const parent = binding(fixture, 'parent');
    const identity = resolveRepoIdentity(main);
    fixture.withDb((db) => {
      const retained = workStatus(db, { repoId: identity.id, contextKey: identity.worktreeKey }, true)
        .works.find((work) => work.id === branch.work_id);
      assert.equal(retained?.state, 'active');
      assert.match(retained?.checkpoint?.body ?? '', /Deploy and verify/);
      assert.ok(chooseWork(db, { repoId: identity.id, contextKey: identity.worktreeKey,
        bindingId: String(parent.id), workId: String(branch.work_id), now: Number(db.prepare('SELECT MAX(captured_at) AS at FROM raw_events').get()?.at) }));
    });
    const continued = await fixture.capture('UserPromptSubmit', { cwd: main, session_id: 'parent', prompt_id: 'continue',
      model: 'claude-opus-5[1m]', prompt: 'Continue deployment verification after the merge.' });
    assert.match(continued.stdout ?? '', /Deploy and verify the live timeout/);
    await fixture.capture('SessionEnd', { cwd: main, session_id: 'parent', reason: 'exit' });
    assert.equal(await runObserveForFixture(fixture, { fetch: respond }), 0);
    assert.deepEqual(continuationContexts, ['provided'], 'removed provenance must support explicit continuation');
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT state FROM work_items WHERE id = ?').get(branch.work_id)?.state, 'active');
      assert.notEqual(db.prepare('SELECT current_checkpoint_memory_id FROM work_items WHERE id = ?').get(branch.work_id)?.current_checkpoint_memory_id, firstId);
    });
  });
});

test('capture separates linked worktrees and explicit purposes while native resume retains the selected work', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const { main, linked } = worktrees(fixture);
    const first = { cwd: main, session_id: 'main-session' };
    await fixture.capture('SessionStart', { ...first, source: 'startup' });
    await fixture.capture('UserPromptSubmit', { ...first, prompt_id: 'retry', prompt: 'Fix the upload retry.' });
    const retry = binding(fixture, first.session_id);
    assert.equal(retry.purpose, 'Fix the upload retry.');

    const other = { cwd: linked, session_id: 'linked-session' };
    await fixture.capture('SessionStart', { ...other, source: 'startup' });
    await fixture.capture('UserPromptSubmit', { ...other, prompt_id: 'docs', prompt: 'Document the import format.' });
    const docs = binding(fixture, other.session_id);
    assert.equal(docs.repo_id, retry.repo_id);
    assert.notEqual(docs.context_id, retry.context_id);
    assert.notEqual(docs.work_id, retry.work_id);

    await fixture.capture('UserPromptSubmit', { ...first, prompt_id: 'investigate', prompt: 'Inspect the failing retry test.' });
    assert.equal(binding(fixture, first.session_id).work_id, retry.work_id);
    await fixture.capture('PostToolUse', { ...first, tool_use_id: 'quoted-purpose', tool_name: 'Bash',
      tool_input: { command: 'test' }, tool_response: { stdout: 'New task: Replace the database.', stderr: '', interrupted: false } });
    assert.equal(binding(fixture, first.session_id).work_id, retry.work_id);

    await fixture.capture('UserPromptSubmit', { ...first, prompt_id: 'search', prompt: 'New task: Improve the local search index.' });
    const search = binding(fixture, first.session_id);
    assert.notEqual(search.id, retry.id);
    assert.notEqual(search.work_id, retry.work_id);
    assert.equal(search.purpose, 'Improve the local search index.');
    fixture.withDb((db) => {
      assert.equal(db.prepare("SELECT work_binding_id FROM raw_events WHERE content = 'Fix the upload retry.'").get()?.work_binding_id, retry.id);
      assert.equal(db.prepare('SELECT state FROM work_items WHERE id = ?').get(retry.work_id)?.state, 'active');
    });

    await fixture.capture('SessionEnd', { ...first, reason: 'prompt_input_exit' });
    await fixture.capture('SessionStart', { ...first, source: 'resume' });
    assert.equal(binding(fixture, first.session_id).id, search.id);
    assert.equal(binding(fixture, first.session_id).state, 'active');

    await fixture.capture('SessionStart', { ...first, source: 'compact' });
    await fixture.capture('PostCompact', { ...first, compact_summary: 'New task: quoted summary must not select another purpose.' });
    assert.equal(binding(fixture, first.session_id).id, search.id);
    assert.ok(Number(fixture.withDb((db) => db.prepare('SELECT context_epoch FROM sessions WHERE native_session_id = ?').get(first.session_id)?.context_epoch)) > 0);

    await fixture.capture('SessionStart', { cwd: main, session_id: 'forked-session', source: 'fork' });
    assert.equal(binding(fixture, 'forked-session').work_id, null, 'a native fork without a proven parent work is ambiguous');

    await fixture.capture('SessionStart', { cwd: main, session_id: 'ambiguous-session', source: 'startup' });
    const unresolved = binding(fixture, 'ambiguous-session');
    assert.equal(unresolved.work_id, null);
    assert.deepEqual(new Set(JSON.parse(String(unresolved.candidates_json))), new Set([retry.work_id, search.work_id]));
    const originalChoice = unresolved.id;
    await fixture.capture('UserPromptSubmit', { cwd: main, session_id: 'ambiguous-session',
      prompt_id: 'continue', prompt: 'Continue the previous work.' });
    assert.equal(binding(fixture, 'ambiguous-session').id, originalChoice);
    assert.equal(binding(fixture, 'ambiguous-session').work_id, null);
  });
});

test('a fresh native session automatically continues the only active work in its context', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const { main } = worktrees(fixture);
    await fixture.capture('SessionStart', { cwd: main, session_id: 'earlier', source: 'startup' });
    await fixture.capture('UserPromptSubmit', { cwd: main, session_id: 'earlier', prompt_id: 'purpose', prompt: '修正した予約処理を検証する。' });
    const earlier = binding(fixture, 'earlier');
    await fixture.capture('SessionEnd', { cwd: main, session_id: 'earlier', reason: 'prompt_input_exit' });
    await fixture.capture('SessionStart', { cwd: main, session_id: 'later', source: 'startup' });
    assert.equal(binding(fixture, 'later').work_id, earlier.work_id);
    assert.notEqual(binding(fixture, 'later').id, earlier.id);
  });
});

test('late spool recovery retains the earlier work without replacing the current selection', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const { main } = worktrees(fixture);
    const common = { cwd: main, session_id: 'late-recovery' };
    await fixture.capture('SessionStart', { ...common, source: 'startup' });
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'earlier', prompt: 'Fix the upload retry.' });
    const earlier = binding(fixture, common.session_id);
    toggleDatabase(fixture, true);
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'late', prompt: 'Inspect the earlier retry failure.' }, 'spooled');
    toggleDatabase(fixture, false);
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'later', prompt: '別件: 検索の並び順を改善する。' });
    const later = binding(fixture, common.session_id);
    await runObserveForFixture(fixture);
    assert.equal(binding(fixture, common.session_id).id, later.id);
    fixture.withDb((db) => {
      const recovered = db.prepare("SELECT work_binding_id, via_spool FROM raw_events WHERE content = 'Inspect the earlier retry failure.'").get();
      assert.equal(recovered?.via_spool, 1);
      assert.equal(recovered?.work_binding_id, earlier.id);
    });
  });
});

test('batches never mix purposes in one native session and a closed purpose drains before ten turns', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const { main } = worktrees(fixture);
    const common = { cwd: main, session_id: 'batch-purposes' };
    await fixture.capture('SessionStart', { ...common, source: 'startup' });
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'earlier', prompt: 'Fix the upload retry.' });
    const earlier = binding(fixture, common.session_id);
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'later', prompt: 'New task: Improve the local search index.' });
    const later = binding(fixture, common.session_id);
    await runObserveForFixture(fixture);
    fixture.withDb((db) => {
      const attempts = db.prepare('SELECT id, work_binding_id FROM observation_batches').all();
      assert.equal(attempts.length, 1, 'closing the earlier purpose makes its short cohort due');
      assert.equal(attempts[0].work_binding_id, earlier.id);
      assert.equal(db.prepare(`SELECT COUNT(*) AS n FROM observation_batch_sources s JOIN raw_events r ON r.id = s.raw_event_id
        WHERE s.batch_id = ? AND r.work_binding_id <> ?`).get(attempts[0].id, earlier.id)?.n, 0);
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM observation_batches WHERE work_binding_id = ?').get(later.id)?.n, 0);
    });
    await fixture.capture('SessionEnd', { ...common, reason: 'prompt_input_exit' });
    await runObserveForFixture(fixture);
    fixture.withDb((db) => {
      assert.equal(db.prepare(`SELECT COUNT(*) AS n FROM observation_batches b JOIN observation_batch_sources s ON s.batch_id = b.id
        JOIN raw_events r ON r.id = s.raw_event_id WHERE b.work_binding_id IS NOT r.work_binding_id`).get()?.n, 0);
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM observation_batches WHERE work_binding_id = ?').get(later.id)?.n, 1);
    });
  });
});

test('a previously unseen new-purpose declaration in late spool data stays unresolved', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const { main } = worktrees(fixture);
    const common = { cwd: main, session_id: 'late-purpose' };
    await fixture.capture('SessionStart', { ...common, source: 'startup' });
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'first', prompt: 'Fix the upload retry.' });
    const first = binding(fixture, common.session_id);
    toggleDatabase(fixture, true);
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'unseen', prompt: 'New task: Clarify the settings.' }, 'spooled');
    await fixture.capture('PostToolUse', { ...common, prompt_id: 'unseen', tool_use_id: 'late-settings', tool_name: 'Bash',
      tool_input: { command: 'inspect' }, tool_response: { stdout: 'The settings use a local cache.', stderr: '', interrupted: false } }, 'spooled');
    toggleDatabase(fixture, false);
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'current', prompt: 'New task: Improve search.' });
    const current = binding(fixture, common.session_id);
    await runObserveForFixture(fixture);
    fixture.withDb((db) => {
      const held = db.prepare(`SELECT b.id, b.work_id, b.closed_at FROM raw_events r
        JOIN work_bindings b ON b.id = r.work_binding_id WHERE r.content = 'New task: Clarify the settings.'`).get()!;
      assert.notEqual(held.id, first.id);
      assert.notEqual(held.id, current.id);
      assert.equal(held.work_id, null);
      assert.notEqual(held.closed_at, null);
      assert.equal(db.prepare("SELECT work_binding_id FROM raw_events WHERE content = 'The settings use a local cache.'").get()?.work_binding_id, held.id);
    });
    assert.equal(binding(fixture, common.session_id).id, current.id);
  });
});

test('an explicit first purpose fills the empty work created at session start', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const { main } = worktrees(fixture);
    const common = { cwd: main, session_id: 'first-purpose' };
    await fixture.capture('SessionStart', { ...common, source: 'startup' });
    const empty = binding(fixture, common.session_id);
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'purpose', prompt: 'New task: Fix the upload retry.' });
    assert.equal(binding(fixture, common.session_id).work_id, empty.work_id);
    assert.equal(binding(fixture, common.session_id).purpose, 'Fix the upload retry.');
    assert.equal(fixture.withDb((db) => db.prepare('SELECT COUNT(*) AS n FROM work_items').get()?.n), 1);
  });
});

function workCli(fixture: Fixture, cwd: string, ...args: string[]) {
  return spawnSync(process.execPath, [join(process.cwd(), 'dist/oboete.mjs'), 'work', ...args, '--json'],
    { cwd, env: fixture.env, encoding: 'utf8' });
}

test('work choices persist, stale or foreign choices fail, and completion is explicit', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const { main } = worktrees(fixture);
    const common = { cwd: main, session_id: 'two-purposes' };
    await fixture.capture('SessionStart', { ...common, source: 'startup' });
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'one', prompt: 'Fix the upload retry.' });
    const one = binding(fixture, common.session_id);
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'two', prompt: 'New task: Improve search.' });
    const two = binding(fixture, common.session_id);
    await fixture.capture('SessionStart', { cwd: main, session_id: 'choosing', source: 'startup' });
    const choice = binding(fixture, 'choosing');
    const status = workCli(fixture, main, 'status');
    assert.equal(status.status, 0, status.stderr);
    assert.ok(JSON.parse(status.stdout).bindings.some((row: { id: string }) => row.id === choice.id));

    const selected = workCli(fixture, main, 'choose', String(choice.id), String(one.work_id));
    assert.equal(selected.status, 0, selected.stderr);
    assert.equal(binding(fixture, 'choosing').id, choice.id);
    assert.equal(binding(fixture, 'choosing').work_id, one.work_id);
    assert.equal(workCli(fixture, main, 'choose', String(choice.id), String(one.work_id)).status, 0);
    assert.equal(workCli(fixture, main, 'choose', String(choice.id), String(two.work_id)).status, 0);
    const switched = binding(fixture, 'choosing');
    assert.notEqual(switched.id, choice.id);
    assert.equal(workCli(fixture, main, 'choose', String(choice.id), String(one.work_id)).status, 1);
    assert.equal(binding(fixture, 'choosing').id, switched.id);

    const foreign = join(fixture.home, 'foreign');
    mkdirSync(foreign);
    const denied = workCli(fixture, foreign, 'choose', String(switched.id), String(one.work_id));
    const missing = workCli(fixture, foreign, 'choose', 'missing-binding', String(one.work_id));
    assert.equal(denied.status, 1);
    assert.equal(denied.stderr, missing.stderr);
    assert.equal(binding(fixture, 'choosing').id, switched.id);
    assert.equal(workCli(fixture, main, 'complete', String(one.work_id)).status, 0);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT state FROM work_items WHERE id = ?').get(one.work_id)?.state, 'completed');
      assert.equal(db.prepare('SELECT state FROM work_items WHERE id = ?').get(two.work_id)?.state, 'active');
    });
  });
});

test('identical native session and prompt IDs in separate repositories retain both sources and sessions', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const firstRoot = join(fixture.home, 'first-repository');
    const otherRoot = join(fixture.home, 'other-repository');
    mkdirSync(firstRoot);
    mkdirSync(otherRoot);
    for (const cwd of [firstRoot, otherRoot]) {
      await fixture.capture('SessionStart', { cwd, session_id: 'same-native-id', source: 'startup' });
      await fixture.capture('UserPromptSubmit', { cwd, session_id: 'same-native-id', prompt_id: 'same-prompt-id', prompt: 'Inspect the retry behavior.' });
    }
    fixture.withDb((db) => {
      const rows = db.prepare("SELECT id, repo_id, session_id FROM raw_events WHERE kind = 'prompt'").all();
      assert.equal(rows.length, 2);
      assert.equal(new Set(rows.map((row) => row.id)).size, 2);
      assert.equal(new Set(rows.map((row) => row.session_id)).size, 2);
      assert.equal(new Set(rows.map((row) => row.repo_id)).size, 2);
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM raw_events r JOIN sessions s ON s.id = r.session_id WHERE r.repo_id <> s.repo_id').get()?.n, 0);
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM work_bindings WHERE closed_at IS NULL').get()?.n, 2);
    });
  });
});

test('a late spooled session end cannot end a resumed native session or close its current turn', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const { main } = worktrees(fixture);
    const common = { cwd: main, session_id: 'late-session-end' };
    await fixture.capture('SessionStart', { ...common, source: 'startup' });
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'old', prompt: 'Inspect the retry behavior.' });
    toggleDatabase(fixture, true);
    await fixture.capture('SessionEnd', { ...common, reason: 'prompt_input_exit' }, 'spooled');
    toggleDatabase(fixture, false);
    await fixture.capture('SessionStart', { ...common, source: 'resume' });
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'current', prompt: 'Check the current retry behavior.' });
    const before = fixture.withDb((db) => db.prepare('SELECT status, ended_at, turn_count FROM sessions').get());
    await runObserveForFixture(fixture);
    fixture.withDb((db) => {
      assert.deepEqual(db.prepare('SELECT status, ended_at, turn_count FROM sessions').get(), before);
      assert.equal(db.prepare('SELECT ended_at FROM turns ORDER BY ordinal DESC LIMIT 1').get()?.ended_at, null);
    });
  });
});

test('legacy unbound sources and inherited batches stay held until explicit source-to-work selection', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'work-selection-fixture-key' });
    writeConfig(fixture, 'openrouter');
    const { main } = worktrees(fixture);
    const common = { cwd: main, session_id: 'legacy-work' };
    await fixture.capture('SessionStart', { ...common, source: 'startup' });
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'source', prompt: 'Inspect the retry behavior.' });
    await fixture.capture('SessionEnd', { ...common, reason: 'prompt_input_exit' });
    const work = binding(fixture, common.session_id);
    const sourceId = fixture.withDb((db) => {
      const row = db.prepare("SELECT id, repo_id, session_id, turn_id FROM raw_events WHERE kind = 'prompt'").get()!;
      db.prepare(`INSERT INTO observation_batches (id, repo_id, session_id, through_event_id, destination, trigger,
        state, provider_attempts, claimed_at) VALUES ('legacy-attempt', ?, ?, ?, 'remote_observer', 'session_end', 'pending', 0, ?)`)
        .run(row.repo_id, row.session_id, row.id, NOW - 1000);
      db.prepare(`UPDATE raw_events SET work_binding_id = NULL, batch_id = 'legacy-attempt',
        payload_json = json_remove(payload_json, '$.work_context_key') WHERE id = ?`).run(row.id);
      db.prepare(`INSERT INTO observation_batch_sources (batch_id, raw_event_id, turn_id, outcome, recorded_at)
        VALUES ('legacy-attempt', ?, ?, 'assigned', ?)`).run(row.id, row.turn_id, NOW - 1000);
      return String(row.id);
    });
    let requests = 0;
    const fetch: typeof globalThis.fetch = async () => { requests += 1; return openAiResponse(providerOutput(sourceId)); };
    await runObserveForFixture(fixture, { fetch });
    assert.equal(requests, 0);
    fixture.withDb((db) => {
      const source = db.prepare('SELECT content, processing_state, retry_after, batch_id FROM raw_events WHERE id = ?').get(sourceId)!;
      assert.equal(source.content, 'Inspect the retry behavior.');
      assert.equal(source.processing_state, 'waiting');
      assert.equal(source.retry_after, null);
      assert.equal(source.batch_id, null);
      assert.equal(db.prepare("SELECT reason FROM observation_batch_sources WHERE batch_id = 'legacy-attempt'").get()?.reason, 'work_selection_required');
    });
    fixture.withDb((db) => db.prepare("UPDATE raw_events SET classification_state = 'pending' WHERE id = ?").run(sourceId));
    const selected = workCli(fixture, main, 'choose-source', sourceId, String(work.work_id));
    assert.equal(selected.status, 0, selected.stderr);
    await runObserveForFixture(fixture, { fetch });
    assert.equal(requests, 1);
    assert.equal(fixture.withDb((db) => db.prepare('SELECT processing_state FROM raw_events WHERE id = ?').get(sourceId)?.processing_state), 'processed');
  });
});

test('old spool IDs that collide with another repository are preserved under the new repository namespace', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const firstRoot = join(fixture.home, 'old-repository');
    const otherRoot = join(fixture.home, 'spooled-repository');
    mkdirSync(firstRoot);
    mkdirSync(otherRoot);
    const common = { session_id: 'legacy-shared-native', prompt_id: 'shared-prompt', prompt: 'Inspect the retry behavior.' };
    await fixture.capture('UserPromptSubmit', { ...common, cwd: firstRoot });
    const legacyId = fixture.withDb((db) => {
      const row = db.prepare("SELECT id, json_extract(payload_json, '$.legacy_event_id') AS legacy FROM raw_events WHERE kind = 'prompt'").get()!;
      db.prepare(`UPDATE raw_events SET id = ?, payload_json = json_remove(payload_json, '$.legacy_event_id', '$.work_context_key')
        WHERE id = ?`).run(row.legacy, row.id);
      return String(row.legacy);
    });
    toggleDatabase(fixture, true);
    await fixture.capture('UserPromptSubmit', { ...common, cwd: otherRoot }, 'spooled');
    const path = join(fixture.paths.spool, listSpool(fixture.paths)[0]);
    const entry = JSON.parse(readFileSync(path, 'utf8'));
    const payload = JSON.parse(entry.row.payload_json);
    entry.row.id = payload.legacy_event_id;
    delete payload.legacy_event_id;
    delete payload.work_context_key;
    entry.row.payload_json = JSON.stringify(payload);
    writeFileSync(path, JSON.stringify(entry));
    toggleDatabase(fixture, false);
    await runObserveForFixture(fixture);
    fixture.withDb((db) => {
      const rows = db.prepare("SELECT id, repo_id, session_id, via_spool FROM raw_events WHERE kind = 'prompt'").all();
      assert.equal(rows.length, 2);
      assert.ok(rows.some((row) => row.id === legacyId && row.via_spool === 0));
      assert.ok(rows.some((row) => row.id !== legacyId && row.via_spool === 1));
      assert.equal(new Set(rows.map((row) => row.session_id)).size, 2);
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM raw_events r JOIN sessions s ON s.id = r.session_id WHERE r.repo_id <> s.repo_id').get()?.n, 0);
    });
    const why = spawnSync(process.execPath, [join(process.cwd(), 'dist/oboete.mjs'), 'why', common.session_id, '--json'],
      { cwd: otherRoot, env: fixture.env, encoding: 'utf8' });
    assert.equal(why.status, 0, why.stderr);
    assert.ok(why.stdout.includes(common.session_id));
    const storedNative = fixture.withDb((db) => String(db.prepare('SELECT native_session_id FROM sessions WHERE original_native_session_id IS NOT NULL').get()?.native_session_id));
    assert.equal(why.stdout.includes(storedNative), false, 'the internal collision ID is not a user-facing native identity');
  });
});

test('late recovery does not replace a context root observed more recently', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const { main } = worktrees(fixture);
    const common = { cwd: main, session_id: 'late-root' };
    await fixture.capture('SessionStart', { ...common, source: 'startup' });
    toggleDatabase(fixture, true);
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'late', prompt: 'Inspect the retry behavior.' }, 'spooled');
    toggleDatabase(fixture, false);
    const moved = join(fixture.home, 'moved-worktree');
    fixture.withDb((db) => db.prepare('UPDATE work_contexts SET root = ?, last_seen_at = ? WHERE root = ?').run(moved, NOW, main));
    await runObserveForFixture(fixture);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT root FROM work_contexts').get()?.root, moved);
      assert.equal(db.prepare('SELECT last_seen_at FROM work_contexts').get()?.last_seen_at, NOW);
    });
  });
});
