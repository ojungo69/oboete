import assert from 'node:assert/strict';
import childProcess, { spawnSync } from 'node:child_process';
import { syncBuiltinESMExports } from 'node:module';
import { writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { getMemory, grantVisibility, memoryScope } from '../../src/db/queries.js';
import { buildPromptPack, buildSessionStartPack, type SessionStartInput } from '../../src/injection/pack.js';
import { confirmDelivery } from '../../src/injection/ledger.js';
import { attachOnPreToolUse, confirmOnPostToolUse, storePending } from '../../src/injection/deferred.js';
import { chooseWork, completeWork } from '../../src/work.js';
import { excludeSecretSource } from '../../src/worker/batches.js';
import { NOW, REPO_ID, seedEvent, seedMemory, seedRepo, seedSession, withOpened } from '../helpers/observer-fixture.js';
import { seedWorkBinding } from '../helpers/work.js';
import { withCapture, type Context } from '../helpers/capture.js';
import { openDatabase } from '../../src/db/open.js';
import { runInject } from '../../src/injection/pi.js';
import { stdoutOf } from '../helpers/inject-fixture.js';
import { filterReadOutput } from '../../src/privacy/provenance.js';
import { detectSync } from '../../src/privacy/detect.js';

async function captureChoicePurposes(context: Context) {
  await context.capture('claude', 'SessionStart', { cwd: context.repo, session_id: 'choice-origin', source: 'startup' });
  for (const [id, purpose] of [['one', 'Inspect source choice marker 8712.'], ['two', 'Verify the other work.']]) {
    await context.capture('claude', 'UserPromptSubmit', { cwd: context.repo, session_id: 'choice-origin',
      prompt_id: id, prompt: `New task: ${purpose}` });
  }
  return context.all('SELECT id, purpose, purpose_source_event_id FROM work_items ORDER BY purpose');
}

for (const change of ['missing reference', 'expired source', 'secret source', 'pending source', 'protected path'] as const) {
  test(`choice labels retain opaque selection IDs but withhold a ${change}`, async () => {
    await withCapture(async (context) => {
      const works = await captureChoicePurposes(context);
      const db = openDatabase({ path: context.paths.db, timeoutMs: 1000 }).db;
      try {
        const sourceId = String(works[0].purpose_source_event_id);
        if (change === 'missing reference') db.prepare('UPDATE work_items SET purpose_source_event_id = NULL WHERE id = ?').run(String(works[0].id));
        else if (change === 'expired source') db.prepare('DELETE FROM raw_events WHERE id = ?').run(sourceId);
        else if (change === 'secret source') db.prepare("UPDATE raw_events SET sensitivity = 'secret' WHERE id = ?").run(sourceId);
        else if (change === 'pending source') db.prepare("UPDATE raw_events SET classification_state = 'pending' WHERE id = ?").run(sourceId);
        else {
          db.prepare(`UPDATE raw_events SET payload_json = json_set(payload_json, '$.source_paths', json('["protected/source.ts"]')) WHERE id = ?`).run(sourceId);
          writeFileSync(join(context.repo, '.oboete.toml'), '[privacy]\nsecret_paths = ["protected/**"]\n');
        }
        const read = await filterReadOutput(db, { repoId: String(context.all('SELECT id FROM repos')[0].id),
          bindingId: String(context.all("SELECT id FROM work_bindings WHERE closed_at IS NULL")[0].id) }, [],
        works.map((work) => ({ id: String(work.id), purpose: String(work.purpose) })));
        assert.equal(read.works[0].purpose, null, 'CLI/MCP labels use the same source proof');
      } finally { db.close(); }
      const result = await context.capture('claude', 'SessionStart', { cwd: context.repo,
        session_id: 'choice-reader', source: 'startup', model: 'claude-opus-5[1m]' });
      assert.match(result.stdout ?? '', /Select which work/);
      assert.match(result.stdout ?? '', /Untitled work/);
      assert.ok((result.stdout ?? '').includes(String(works[0].id)));
      assert.ok((result.stdout ?? '').includes(String(works[1].id)));
      assert.match(result.stdout ?? '', /Verify the other work/);
      assert.doesNotMatch(result.stdout ?? '', /source choice marker 8712/);
    });
  });
}

test('a purpose source remap during detection cancels both a read view and a Pi choice pack', async () => {
  await withCapture(async (context) => {
    const works = await captureChoicePurposes(context);
    const db = openDatabase({ path: context.paths.db, timeoutMs: 1000 }).db;
    const remap = () => db.prepare('UPDATE work_items SET purpose_source_event_id = ? WHERE id = ?')
      .run(String(works[1].purpose_source_event_id), String(works[0].id));
    try {
      let changed = false;
      const read = await filterReadOutput(db, { repoId: String(context.all('SELECT id FROM repos')[0].id),
        bindingId: String(context.all("SELECT id FROM work_bindings WHERE closed_at IS NULL")[0].id) }, [],
      works.map((work) => ({ id: String(work.id), purpose: String(work.purpose) })), async (input) => {
        changed = true; remap(); return detectSync(input);
      });
      assert.equal(changed, true);
      assert.deepEqual(read.works.map((work) => work.purpose), [null, null]);
      db.prepare('UPDATE work_items SET purpose_source_event_id = ? WHERE id = ?')
        .run(String(works[0].purpose_source_event_id), String(works[0].id));
      db.prepare('UPDATE raw_events SET content = content || ? WHERE id = ?')
        .run('a'.repeat(70_000), String(works[0].purpose_source_event_id));
      changed = false;
      const output = await stdoutOf(() => runInject(['--agent', 'pi', '--kind', 'start'], {
        readStdin: () => JSON.stringify({ cwd: context.repo, session_id: 'choice-race', model: 'gpt-5.6-luna' }),
        now: () => NOW, elapsedMs: () => 0,
        detect: async (input) => { changed = true; remap(); return detectSync(input); },
      }));
      assert.equal(changed, true);
      assert.equal(output, '');
    } finally { db.close(); }
  });
});

test('Grok choice-only delivery carries and rechecks purpose sources', async () => {
  await withCapture(async (context) => {
    const works = await captureChoicePurposes(context);
    await context.capture('grok', 'SessionStart', { cwd: context.repo, sessionId: 'choice-deferred', source: 'new', model: 'grok-4.6-build' });
    const db = openDatabase({ path: context.paths.db, timeoutMs: 1000 }).db;
    try {
      const conversation = String(db.prepare("SELECT conversation_id FROM sessions WHERE native_session_id = 'choice-deferred'").get()?.conversation_id);
      assert.ok(db.prepare("SELECT 1 FROM injections WHERE state = 'pending'").get());
      db.prepare("UPDATE raw_events SET classification_state = 'pending' WHERE id = ?").run(String(works[0].purpose_source_event_id));
      assert.equal(attachOnPreToolUse(db, { conversationId: conversation, toolCallId: 'choice-carrier', now: NOW }), null);
    } finally { db.close(); }
  });
});

test('one pack validates its source snapshot without resolving the same Git root per item', async () => {
  await withCapture(async (context) => {
    await context.capture('claude', 'SessionStart', { cwd: context.repo, session_id: 'snapshot-work', source: 'startup' });
    for (let i = 0; i < 6; i++) await context.capture('claude', 'UserPromptSubmit', {
      cwd: context.repo, session_id: 'snapshot-work', prompt_id: `activity-${i}`, prompt: `Inspect upload evidence item ${i}.` });
    const original = childProcess.spawnSync;
    let gitCalls = 0;
    childProcess.spawnSync = ((...args: Parameters<typeof original>) => {
      if (args[0] === 'git') gitCalls++;
      return Reflect.apply(original, childProcess, args);
    }) as typeof original;
    syncBuiltinESMExports();
    try {
      const result = await context.capture('claude', 'SessionStart', {
        cwd: context.repo, session_id: 'snapshot-reader', source: 'startup', model: 'claude-opus-5[1m]' });
      assert.match(result.stdout ?? '', /Inspect upload evidence item/);
      assert.ok(gitCalls <= 9, `capture, initial policy and fresh final policy need at most three root resolutions; got ${gitCalls} Git calls`);
    } finally {
      childProcess.spawnSync = original;
      syncBuiltinESMExports();
    }
  });
});

for (const change of ['repository rules', 'credentials'] as const) test(`a prepared source snapshot is cancelled when ${change} change during detection`, async () => {
  await withCapture(async (context) => {
    const marker = 'SnapshotReleaseProbe';
    await context.capture('claude', 'SessionStart', { cwd: context.repo, session_id: 'snapshot-policy', source: 'startup' });
    await context.capture('claude', 'UserPromptSubmit', { cwd: context.repo, session_id: 'snapshot-policy',
      prompt_id: 'long-source', prompt: `${marker} ${'a'.repeat(70_000)}` });
    const { db } = openDatabase({ path: context.paths.db, timeoutMs: 1_000 });
    db.exec(`UPDATE raw_events SET payload_json = json_set(payload_json, '$.source_paths', json('["protected/source.ts"]'))
      WHERE kind = 'prompt'`);
    const source = db.prepare("SELECT id, content, classification_state FROM raw_events WHERE kind = 'prompt'").get();
    db.close();
    const previous = process.env.OBOETE_OPENROUTER_API_KEY;
    let changed = false;
    try {
      const result = await stdoutOf(() => runInject(['--agent', 'pi', '--kind', 'start'], {
        readStdin: () => JSON.stringify({ cwd: context.repo, session_id: 'snapshot-policy-reader', model: 'gpt-5.6-luna' }),
        now: () => NOW, elapsedMs: () => 0,
        detect: async (input) => {
          changed = true;
          if (change === 'repository rules') writeFileSync(join(context.repo, '.oboete.toml'), '[privacy]\nsecret_paths = ["protected/**"]\n');
          else process.env.OBOETE_OPENROUTER_API_KEY = marker;
          return detectSync(input);
        },
      }));
      assert.equal(changed, true, 'the policy changes after assembly captured its source snapshot');
      assert.doesNotMatch(result, new RegExp(marker));
      assert.deepEqual(context.all("SELECT id, content, classification_state FROM raw_events WHERE kind = 'prompt'")[0], source);
    } finally {
      if (previous === undefined) delete process.env.OBOETE_OPENROUTER_API_KEY;
      else process.env.OBOETE_OPENROUTER_API_KEY = previous;
    }
  });
});

test('the injection command bounds expensive retained-path validation without losing its source', async () => {
  await withCapture(async (context) => {
    await context.capture('claude', 'SessionStart', { cwd: context.repo, session_id: 'bounded-path', source: 'startup' });
    await context.capture('claude', 'UserPromptSubmit', { cwd: context.repo, session_id: 'bounded-path', prompt_id: 'purpose', prompt: 'Review the retained upload path.' });
    const path = 'a'.repeat(128 * 1024);
    await context.capture('claude', 'PreToolUse', { cwd: context.repo, session_id: 'bounded-path',
      tool_use_id: 'long-path', tool_name: 'Read', tool_input: { file_path: path } });
    const before = context.all("SELECT id, payload_json, classification_state, processing_state FROM raw_events WHERE kind = 'tool_call'");
    assert.equal(before[0].classification_state, 'done');
    const rules = Array.from({ length: 64 }, (_, index) => '*a'.repeat(100) + String(index) + 'Z');
    writeFileSync(join(context.repo, '.oboete.toml'), `[privacy]\nsecret_paths = ${JSON.stringify(rules)}\n`);
    const started = performance.now();
    const result = spawnSync(process.execPath, [join(process.cwd(), 'dist/oboete.mjs'), 'inject', '--agent', 'pi', '--kind', 'start'], {
      cwd: context.repo, env: process.env, encoding: 'utf8', timeout: 1800,
      input: JSON.stringify({ cwd: context.repo, session_id: 'bounded-reader', model: 'gpt-5.6-luna' }),
    });
    assert.equal(result.status, 0, result.error?.message ?? result.stderr);
    assert.ok(performance.now() - started < 1300, 'the worker cutoff leaves time for the injection command to exit');
    assert.doesNotMatch(result.stdout, /aaaaaa/);
    assert.deepEqual(context.all("SELECT id, payload_json, classification_state, processing_state FROM raw_events WHERE kind = 'tool_call'"), before);
  });
});

test('checkpoint and pending activity are withheld when current rules deny their unrendered source paths', async () => {
  await withCapture(async (context) => {
    await context.capture('claude', 'SessionStart', { cwd: context.repo, session_id: 'policy', source: 'startup' });
    await context.capture('claude', 'UserPromptSubmit', { cwd: context.repo, session_id: 'policy', prompt_id: 'purpose', prompt: 'Check the upload.' });
    const opened = openDatabase({ path: context.paths.db, timeoutMs: 1000 });
    const bound = opened.db.prepare(`SELECT b.work_id, s.repo_id FROM work_bindings b JOIN sessions s ON s.id = b.session_id
      WHERE s.native_session_id = 'policy'`).get()!;
    opened.db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, content_hash, sensitivity,
      review_state, work_id, provenance_complete) VALUES ('private-checkpoint', ?, 'session_summary',
      'Upload progress', 'The protected upload details remain outstanding.', 'policy-checkpoint', 'eligible', 'unreviewed', ?, 1)`)
      .run(bound.repo_id, bound.work_id);
    grantVisibility(opened.db, 'private-checkpoint', { audience: 'work', repoId: String(bound.repo_id), workId: String(bound.work_id) }, 'observer', NOW);
    opened.db.prepare('INSERT INTO memory_sources (memory_id, capture_root, source_paths_json) VALUES (?, ?, ?)')
      .run('private-checkpoint', context.repo, JSON.stringify(['protected/upload.ts']));
    opened.db.prepare('UPDATE work_items SET current_checkpoint_memory_id = ? WHERE id = ?').run('private-checkpoint', bound.work_id);
    opened.db.exec("UPDATE raw_events SET processing_state = 'processed'");
    opened.db.close();
    await context.capture('claude', 'UserPromptSubmit', { cwd: context.repo, session_id: 'policy', prompt_id: 'activity', prompt: 'Inspect the retained activity.' });
    const db = openDatabase({ path: context.paths.db, timeoutMs: 1000 }).db;
    db.prepare(`UPDATE raw_events SET payload_json = json_set(payload_json, '$.source_paths', json(?)) WHERE content = ?`)
      .run(JSON.stringify(['protected/upload.ts']), 'Inspect the retained activity.');
    db.close();
    writeFileSync(join(context.repo, '.oboete.toml'), '[privacy]\nsecret_paths = ["protected/**"]\n');
    const delivered = await context.capture('claude', 'SessionStart', { cwd: context.repo, session_id: 'policy-reader', source: 'startup', model: 'claude-opus-5[1m]' });
    assert.doesNotMatch(delivered.stdout ?? '', /protected upload details|Inspect the retained activity/);
  });
});

for (const change of ['repository rules', 'credentials'] as const) test(`Grok rechecks ${change} before attaching a prepared pack`, async () => {
  await withCapture(async (context) => {
    await context.capture('grok', 'SessionStart', { cwd: context.repo, sessionId: 'deferred-policy', source: 'new' });
    await context.capture('grok', 'UserPromptSubmit', { cwd: context.repo, sessionId: 'deferred-policy', promptId: 'purpose',
      prompt: 'Keep the deployment verification outstanding.' });
    const db = openDatabase({ path: context.paths.db, timeoutMs: 1000 }).db;
    try {
      db.prepare(`UPDATE raw_events SET payload_json = json_set(payload_json, '$.source_paths', json(?)) WHERE kind = 'prompt'`)
        .run(JSON.stringify(['protected/upload.ts']));
      await context.capture('grok', 'SessionStart', { cwd: context.repo, sessionId: 'policy-consumer', source: 'new', model: 'grok-4.6-build' });
      const conversation = String(db.prepare("SELECT conversation_id FROM sessions WHERE native_session_id = 'policy-consumer'").get()?.conversation_id);
      assert.ok(db.prepare("SELECT 1 FROM injections WHERE state = 'pending'").get());
      const prior = process.env.OBOETE_OPENROUTER_API_KEY;
      try {
        if (change === 'repository rules') writeFileSync(join(context.repo, '.oboete.toml'), '[privacy]\nsecret_paths = ["protected/**"]\n');
        else process.env.OBOETE_OPENROUTER_API_KEY = 'Keep the deployment verification outstanding.';
        assert.equal(attachOnPreToolUse(db, { conversationId: conversation, toolCallId: 'changed-policy', now: NOW }), null);
      } finally {
        if (prior === undefined) delete process.env.OBOETE_OPENROUTER_API_KEY;
        else process.env.OBOETE_OPENROUTER_API_KEY = prior;
      }
    } finally { db.close(); }
  });
});

const WORK = `fixture-work:${REPO_ID}`;
const input: SessionStartInput = {
  agent: 'claude', repoId: REPO_ID, repoIdentityDisplay: 'example.invalid/work',
  sessionId: 'sess1', conversationId: 'sess1', epoch: 0, model: 'claude-opus-5[1m]',
  channelCap: 10_000, contextFraction: 0.05, channel: 'claude:SessionStart', now: NOW,
  detect: () => false, directives: [], repoRoot: '/fixture',
};

function seed(db: DatabaseSync): void {
  seedRepo(db);
  seedSession(db, 'sess1');
  seedWorkBinding(db, 'sess1');
  db.prepare(`INSERT INTO work_items (id, repo_id, origin_context_id, purpose, created_at, updated_at)
    VALUES ('other', ?, ?, 'Other investigation', ?, ?)`).run(REPO_ID, `fixture-context:${REPO_ID}`, NOW, NOW);
  for (const [id, work, body] of [
    ['checkpoint', WORK, 'Verify interrupted uploads.'],
    ['foreign', 'other', 'Publish unrelated metrics.'],
    ['legacy', null, 'Old repository-wide session progress.'],
  ] as const) {
    seedMemory(db, { id, title: 'Progress checkpoint', body });
    db.prepare("UPDATE memories SET type = 'session_summary', work_id = ?, pinned_at = ? WHERE id = ?")
      .run(work, NOW, id);
    if (work !== null) db.prepare('UPDATE work_items SET current_checkpoint_memory_id = ? WHERE id = ?').run(id, work);
    if (work !== null) {
      db.prepare('DELETE FROM memory_visibility WHERE memory_id = ?').run(id);
      grantVisibility(db, id, { audience: 'work', repoId: REPO_ID, workId: work }, 'observer', NOW);
    }
  }
  seedMemory(db, { id: 'knowledge', title: 'Upload retry policy', body: 'Uploads use the existing retry helper.' });
  db.prepare('UPDATE memories SET pinned_at = ? WHERE id = ?').run(NOW, 'knowledge');
}

test('both pack lanes include ordinary knowledge of the selected work only', async () => {
  await withOpened(async (db) => {
    seed(db);
    for (const [id, work, body] of [['work-fact', WORK, 'The selected work uses bounded upload retries.'],
      ['other-fact', 'other', 'The unrelated work changes the billing backend.']] as const) {
      seedMemory(db, { id, title: 'Work visibility probe', body });
      db.prepare('DELETE FROM memory_visibility WHERE memory_id = ?').run(id);
      grantVisibility(db, id, { audience: 'work', repoId: REPO_ID, workId: work }, 'observer', NOW);
      db.prepare('UPDATE memories SET pinned_at = ? WHERE id = ?').run(NOW, id);
    }
    const start = await buildSessionStartPack(db, input);
    const prompt = await buildPromptPack(db, { ...input, turnId: null, prompt: 'Work visibility probe' });
    for (const pack of [start, prompt]) {
      assert.match(pack?.text ?? '', /selected work uses bounded upload retries/);
      assert.doesNotMatch(pack?.text ?? '', /unrelated work changes the billing backend/);
    }
  });
});

test('a new tool call can carry the already prepared Grok checkpoint and activity', async () => {
  await withCapture(async (context) => {
    await context.capture('grok', 'SessionStart', { cwd: context.repo, sessionId: 'tool-producer', source: 'new' });
    await context.capture('grok', 'UserPromptSubmit', { cwd: context.repo, sessionId: 'tool-producer', promptId: 'purpose',
      prompt: 'Keep deployment verification outstanding.' });
    await context.capture('grok', 'SessionStart', { cwd: context.repo, sessionId: 'tool-consumer', source: 'new', model: 'grok-4.6-build' });
    const delivered = await context.capture('grok', 'PreToolUse', { cwd: context.repo, sessionId: 'tool-consumer',
      toolUseId: 'first-carrier', toolName: 'run_terminal_command', toolInput: { command: 'pwd' } });
    assert.equal(context.all("SELECT id FROM raw_events WHERE kind = 'tool_call'").length, 1);
    assert.match(delivered.stdout ?? '', /Keep deployment verification outstanding/);
  });
});

test('a policy change while reading a later label also withholds an earlier label', async () => {
  await withCapture(async (context) => {
    const works = await captureChoicePurposes(context);
    const db = openDatabase({ path: context.paths.db, timeoutMs: 1000 }).db;
    const before = process.env.OBOETE_OPENROUTER_API_KEY;
    let changed = false;
    try {
      const { works: labels } = await filterReadOutput(db, { repoId: String(context.all('SELECT id FROM repos')[0].id), bindingId: null }, [],
      works.map((work) => ({ id: String(work.id), purpose: String(work.purpose) })), async (input) => {
        const checked = await detectSync(input);
        if (input.text === works[1].purpose) {
          changed = true;
          process.env.OBOETE_OPENROUTER_API_KEY = String(works[0].purpose);
        }
        return checked;
      });
      assert.equal(changed, true);
      assert.deepEqual(labels.map((label) => label.purpose), [null, null]);
    } finally {
      db.close();
      if (before === undefined) delete process.env.OBOETE_OPENROUTER_API_KEY;
      else process.env.OBOETE_OPENROUTER_API_KEY = before;
    }
  });
});

test('readers detect credentials in unescaped memory and citation fields', async () => {
  await withCapture(async (context) => {
    await context.capture('claude', 'SessionStart', { cwd: context.repo, session_id: 'escaped', source: 'startup' });
    const db = openDatabase({ path: context.paths.db, timeoutMs: 1000 }).db;
    const session = db.prepare('SELECT s.repo_id, b.id FROM sessions s JOIN work_bindings b ON b.session_id = s.id').get()!;
    db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, content_hash, sensitivity)
      VALUES ('escaped-memory', ?, 'discovery', 'Ordinary title', 'Ordinary body', 'escaped', 'eligible')`).run(session.repo_id);
    grantVisibility(db, 'escaped-memory', { audience: 'project', repoId: String(session.repo_id) }, 'migration', NOW);
    const location = { repoId: String(session.repo_id), bindingId: String(session.id), home: context.home };
    const before = process.env.OBOETE_OPENROUTER_API_KEY;
    try {
      const credential = 'rotated"credential\\with\nescaping';
      assert.equal((await filterReadOutput(db, location, [{ id: 'escaped-memory', body: credential }], [])).memories.length, 1);
      process.env.OBOETE_OPENROUTER_API_KEY = credential;
      for (const sensitive of [{ title: credential }, { body: credential }, { sources: [{ citation_value: credential }] }]) {
        const row = { id: 'escaped-memory', ...sensitive };
        const result = await filterReadOutput(db, location, [row], []);
        assert.deepEqual(result.memories, []);
      }
    } finally {
      if (before === undefined) delete process.env.OBOETE_OPENROUTER_API_KEY;
      else process.env.OBOETE_OPENROUTER_API_KEY = before;
      db.close();
    }
  });
});

test('a secret source cannot quarantine a different work checkpoint from the same native session', async () => {
  await withOpened((db) => {
    seed(db);
    seedEvent(db, { id: 'sensitive', content: 'Earlier upload work.' });
    db.exec("UPDATE memories SET source_session_id = 'sess1'");
    db.prepare('INSERT INTO memory_sources (memory_id, raw_event_id) VALUES (?, ?)').run('checkpoint', 'sensitive');
    excludeSecretSource(db, 'sensitive', NOW);
    assert.equal(db.prepare('SELECT sensitivity FROM memories WHERE id = ?').get('foreign')?.sensitivity, 'eligible');
    assert.equal(db.prepare('SELECT sensitivity FROM memories WHERE id = ?').get('checkpoint')?.sensitivity, 'secret');
    assert.equal(db.prepare('SELECT sensitivity FROM memories WHERE id = ?').get('legacy')?.sensitivity, 'secret');
  });
});

test('ordinary readers exclude progress unless its current work or deliberate history is selected', async () => {
  await withOpened((db) => {
    seed(db);
    const ordinary = memoryScope(db, { repoId: REPO_ID, destination: 'injection' });
    assert.equal(getMemory(db, 'legacy', ordinary), null);
    assert.equal(getMemory(db, 'foreign', ordinary), null);
    const selected = memoryScope(db, { repoId: REPO_ID, destination: 'injection', workId: WORK });
    assert.equal(getMemory(db, 'checkpoint', selected)?.id, 'checkpoint');
    assert.equal(getMemory(db, 'foreign', selected), null);
    db.prepare('UPDATE memories SET valid_to = ? WHERE id = ?').run(NOW, 'legacy');
    const history = memoryScope(db, { repoId: REPO_ID, destination: 'injection', history: true });
    assert.equal(getMemory(db, 'legacy', history)?.id, 'legacy');
    db.prepare('UPDATE memories SET deleted_at = ? WHERE id = ?').run(NOW, 'legacy');
    assert.equal(getMemory(db, 'legacy', history), null);
  });
});

test('completed checkpoints require deliberate history rather than an old active binding', async () => {
  await withOpened((db) => {
    seed(db);
    assert.equal(completeWork(db, { repoId: REPO_ID, workId: WORK, now: NOW }), true);
    assert.equal(getMemory(db, 'checkpoint', memoryScope(db, { repoId: REPO_ID, destination: 'injection', workId: WORK })), null);
    assert.equal(getMemory(db, 'checkpoint', memoryScope(db, { repoId: REPO_ID, destination: 'injection', workId: WORK, history: true }))?.id, 'checkpoint');
  });
});

test('explicitly selecting the same completed work resumes it without changing accepted bindings', async () => {
  await withOpened((db) => {
    seed(db);
    completeWork(db, { repoId: REPO_ID, workId: WORK, now: NOW });
    const resumed = chooseWork(db, { repoId: REPO_ID, contextKey: 'fixture', bindingId: 'fixture-binding:sess1', workId: WORK, now: NOW + 1 });
    assert.equal(resumed?.id, 'fixture-binding:sess1');
    assert.equal(db.prepare('SELECT state FROM work_items WHERE id = ?').get(WORK)?.state, 'active');
    assert.equal(db.prepare('SELECT COUNT(*) AS n FROM work_bindings').get()?.n, 1);
  });
});

test('start and prompt injection follow the exact binding and preserve delivery deduplication after a work choice', async () => {
  await withOpened(async (db) => {
    seed(db);
    seedEvent(db, { id: 'pending', content: 'Check the upload timeout.' });
    const pack = await buildSessionStartPack(db, input);
    assert.ok(pack);
    assert.match(pack.text, /work checkpoint.*Progress checkpoint/u);
    assert.match(pack.text, /Verify interrupted uploads/);
    assert.match(pack.text, /Check the upload timeout/);
    assert.doesNotMatch(pack.text, /unrelated metrics|repository-wide/);
    confirmDelivery(db, pack.injectionId, NOW);
    const choice = chooseWork(db, { repoId: REPO_ID, contextKey: 'fixture',
      bindingId: 'fixture-binding:sess1', workId: 'other', now: NOW });
    assert.ok(choice);
    const next = await buildPromptPack(db, { ...input, prompt: 'Continue the progress checkpoint.' });
    assert.ok(next);
    assert.match(next.text, /Publish unrelated metrics/);
    assert.doesNotMatch(next.text, /Verify interrupted uploads|upload timeout|repository-wide/);
    confirmDelivery(db, next.injectionId, NOW);
    assert.equal(await buildPromptPack(db, { ...input, prompt: 'Continue the progress checkpoint.' }), null);
  });
});

test('unresolved selection emits bounded framed choices without candidate progress or raw activity', async () => {
  await withOpened(async (db) => {
    seed(db);
    seedEvent(db, { id: 'pending', content: 'Check the upload timeout.' });
    db.prepare("UPDATE work_bindings SET work_id = NULL, reason = 'ambiguous', candidates_json = ?")
      .run(JSON.stringify([WORK, 'other']));
    db.exec('UPDATE memories SET pinned_at = NULL');
    const pack = await buildSessionStartPack(db, input);
    assert.ok(pack, 'a choice-only pack is useful and gets a delivery receipt');
    assert.match(pack.text, /fixture-binding:sess1/);
    assert.match(pack.text, /> Work other: Untitled work/);
    assert.doesNotMatch(pack.text, /Verify interrupted uploads|unrelated metrics|upload timeout|repository-wide/);
    assert.equal(pack.items.length, 0, 'choices never pretend to be memories or captured activity');
    assert.ok(pack.text.split('\n').slice(1, -1).every((line) => line.startsWith('> ')));
    db.exec(`UPDATE work_items SET purpose_sensitivity = 'private' WHERE id = 'other';
      UPDATE destination_rules SET allowed = 0 WHERE destination = 'injection' AND sensitivity = 'private'`);
    const narrowed = await buildPromptPack(db, { ...input, prompt: 'Continue.' });
    assert.ok(narrowed);
    assert.doesNotMatch(narrowed.text, /Other investigation/);
    assert.equal(await buildPromptPack(db, { ...input, prompt: 'Continue.', detect: () => true }), null);
  });
});

test('masking a short choice purpose keeps the reserved pack within its character limit', async () => {
  await withOpened(async (db) => {
    seed(db);
    seedEvent(db, { id: 'short-purpose', content: 'New task: x' });
    db.exec("UPDATE work_items SET purpose = 'x', purpose_source_event_id = 'short-purpose'; UPDATE memories SET pinned_at = NULL");
    db.prepare("UPDATE work_bindings SET work_id = NULL, reason = 'ambiguous', candidates_json = ?")
      .run(JSON.stringify([WORK, 'other']));
    let displayed = false;
    for (const cap of [220, 250, 280, 400]) {
      const pack = await buildSessionStartPack(db, { ...input, channelCap: cap, epoch: cap,
        detect: (_text, source) => source !== undefined });
      if (pack === null) continue;
      assert.ok(pack.charsUsed <= cap);
      assert.equal(pack.charsUsed, pack.text.length);
      displayed ||= pack.text.includes('Untitled work');
      assert.doesNotMatch(pack.text, /: x\n/);
    }
    assert.equal(displayed, true);
  });
});

test('a changed binding or checkpoint during validation cannot publish stale progress', async () => {
  await withOpened(async (db) => {
    seed(db);
    const pack = await buildSessionStartPack(db, { ...input, detect: () => {
      db.exec("UPDATE work_items SET current_checkpoint_memory_id = NULL");
      return false;
    } });
    assert.equal(pack, null);
    assert.equal(db.prepare("SELECT COUNT(*) AS n FROM injections WHERE state = 'built'").get()?.n, 0);
  });
});

test('path-only calls are retained as pending activity and unrelated delivery metadata cannot invalidate them', async () => {
  await withOpened(async (db) => {
    seed(db);
    seedEvent(db, { id: 'path-only', kind: 'tool_call', content: null,
      payload: { capture_root: '/fixture', source_paths: ['src/config.ts'], tool_name: 'read', input: { paths: ['src/config.ts'] } } });
    const pack = await buildSessionStartPack(db, { ...input, state: 'pending' });
    assert.ok(pack);
    assert.match(pack.text, /read src\/config\.ts/);
    await storePending(db, { conversationId: input.conversationId, epoch: 0, pack, now: NOW,
      validation: { detect: () => false, directives: [] } });
    db.prepare('UPDATE memories SET last_injected_at = ?, citations_head = ?, citations_ok = 1 WHERE id = ?')
      .run(NOW, 'new-citation-cache-head', 'checkpoint');
    assert.equal(attachOnPreToolUse(db, { conversationId: input.conversationId, toolCallId: 'still-valid', now: NOW }), pack.text);
  });
});

test('deferred delivery drops an old work selection and preserves choice-only metadata when merging', async () => {
  await withOpened(async (db) => {
    seed(db);
    const deferred = { ...input, agent: 'grok' as const, model: 'grok-4.6-build', state: 'pending' as const };
    const first = await buildSessionStartPack(db, deferred);
    assert.ok(first);
    await storePending(db, { conversationId: input.conversationId, epoch: 0, pack: first, now: NOW,
      validation: { detect: () => false, directives: [] } });
    chooseWork(db, { repoId: REPO_ID, contextKey: 'fixture', bindingId: 'fixture-binding:sess1', workId: 'other', now: NOW });
    assert.equal(attachOnPreToolUse(db, { conversationId: input.conversationId, toolCallId: 'stale', now: NOW }), null);
    assert.equal(confirmOnPostToolUse(db, { conversationId: input.conversationId, toolCallId: 'stale-post', now: NOW }).text, null);

    db.prepare("UPDATE work_bindings SET work_id = NULL, reason = 'ambiguous', candidates_json = ? WHERE closed_at IS NULL")
      .run(JSON.stringify([WORK, 'other']));
    const choices = await buildPromptPack(db, { ...deferred, prompt: 'Continue.' });
    assert.ok(choices);
    await storePending(db, { conversationId: input.conversationId, epoch: 0, pack: choices, now: NOW,
      validation: { detect: () => false, directives: [] } });
    const related = await buildPromptPack(db, { ...deferred, prompt: 'Upload retry policy' });
    assert.ok(related);
    await storePending(db, { conversationId: input.conversationId, epoch: 0, pack: related, now: NOW,
      validation: { detect: () => false, directives: [] } });
    const text = attachOnPreToolUse(db, { conversationId: input.conversationId, toolCallId: 'choice', now: NOW });
    assert.ok(text);
    assert.match(text, /Binding:/);
    assert.match(text, /> Work other: Untitled work/);
    assert.match(text, /Uploads use the existing retry helper/);
    assert.doesNotMatch(text, /Verify interrupted uploads|Publish unrelated metrics/);
  });
});

test('a printed deferred pack keeps its actual receipt when another hook observes a changed selection', async () => {
  await withOpened(async (db) => {
    seed(db);
    const pack = await buildSessionStartPack(db, { ...input, state: 'pending' });
    assert.ok(pack);
    const store = (next: typeof pack) => storePending(db, { conversationId: input.conversationId, epoch: 0,
      pack: next, now: NOW, validation: { detect: () => false, directives: [] } });
    await store(pack);
    assert.equal(attachOnPreToolUse(db, { conversationId: input.conversationId, toolCallId: 'printed', now: NOW }), pack.text);
    chooseWork(db, { repoId: REPO_ID, contextKey: 'fixture', bindingId: 'fixture-binding:sess1', workId: 'other', now: NOW });
    assert.equal(attachOnPreToolUse(db, { conversationId: input.conversationId, toolCallId: 'later', now: NOW }), null);
    const next = await buildPromptPack(db, { ...input, state: 'pending', prompt: 'Continue the progress.' });
    assert.ok(next);
    await store(next);
    assert.equal(db.prepare('SELECT state FROM injections WHERE id = ?').get(pack.injectionId)?.state, 'attempted');
    assert.equal(confirmOnPostToolUse(db, { conversationId: input.conversationId, toolCallId: 'printed', now: NOW }).status, 'emitted');
    assert.equal(db.prepare('SELECT decision FROM injection_items WHERE injection_id = ? AND memory_id = ?')
      .get(pack.injectionId, 'checkpoint')?.decision, 'included');
    assert.equal(db.prepare('SELECT delivery_count FROM injections WHERE id = ?').get(pack.injectionId)?.delivery_count, 1);
    assert.equal(db.prepare('SELECT state FROM injections WHERE id = ?').get(next.injectionId)?.state, 'omitted');
  });
});

const AGENTS = ['claude', 'codex', 'grok', 'pi'] as const;
const MODELS = { claude: 'claude-opus-5[1m]', codex: 'gpt-5.6-sol', grok: 'grok-4.6-build', pi: 'gpt-5.6-luna' };
for (const from of AGENTS) for (const to of AGENTS) if (from !== to) {
  test(`synthetic ${from} to ${to} continuation delivers the selected checkpoint`, async () => {
    await withCapture(async (context) => {
      const capture = (agent: typeof AGENTS[number], id: string, prompt?: string) => {
        const start = prompt === undefined;
        if (agent === 'pi') {
          const event = start ? 'session_start' : 'input';
          return context.capture(agent, event, { event, session_id: id, cwd: context.repo, model: MODELS[agent],
            prompt_id: start ? undefined : 'purpose', payload: start ? { reason: 'startup' } : { text: prompt, source: 'interactive' } });
        }
        const common = { cwd: context.repo, model: MODELS[agent], source: agent === 'grok' ? 'new' : 'startup', prompt };
        return context.capture(agent, start ? 'SessionStart' : 'UserPromptSubmit', agent === 'grok'
          ? { ...common, sessionId: id, promptId: 'purpose' } : { ...common, session_id: id, prompt_id: 'purpose', turn_id: 'purpose' });
      };
      assert.equal((await capture(from, 'producer')).outcome, 'stored');
      assert.equal((await capture(from, 'producer', 'Verify the interrupted upload.')).outcome, 'stored');
      const before = context.all(`SELECT b.work_id, b.context_id, s.repo_id FROM work_bindings b
        JOIN sessions s ON s.id = b.session_id WHERE s.native_session_id = 'producer' AND b.closed_at IS NULL`)[0];
      const { db } = openDatabase({ path: context.paths.db, timeoutMs: 1000 });
      db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, content_hash, sensitivity, review_state, work_id)
        VALUES ('checkpoint-pair', ?, 'session_summary', 'Upload continuity',
          'The migration landed. Deployment and timeout verification remain outstanding.', 'pair-checkpoint', 'eligible', 'unreviewed', ?)`)
        .run(String(before.repo_id), String(before.work_id));
      grantVisibility(db, 'checkpoint-pair', { audience: 'work', repoId: String(before.repo_id), workId: String(before.work_id) }, 'observer', NOW);
      db.prepare('UPDATE memories SET provenance_complete = 1 WHERE id = ?').run('checkpoint-pair');
      db.prepare(`INSERT INTO memory_sources (memory_id, capture_root, source_paths_json, source_context_id)
        VALUES ('checkpoint-pair', ?, '[]', ?)`).run(context.repo, String(before.context_id));
      db.prepare('UPDATE work_items SET current_checkpoint_memory_id = ? WHERE id = ?').run('checkpoint-pair', String(before.work_id));
      db.exec("UPDATE raw_events SET processing_state = 'processed'");
      db.close();
      const started = await capture(to, 'consumer');
      assert.equal(started.outcome, 'stored');
      const after = context.all(`SELECT b.work_id, s.id AS session_id, s.conversation_id FROM work_bindings b
        JOIN sessions s ON s.id = b.session_id WHERE s.native_session_id = 'consumer' AND b.closed_at IS NULL`)[0];
      assert.equal(after.work_id, before.work_id);
      let text = started.stdout ?? '';
      if (to === 'pi') text = await stdoutOf(() => runInject(['--agent', 'pi', '--kind', 'start'], {
        readStdin: () => JSON.stringify({ cwd: context.repo, session_id: 'consumer', model: MODELS.pi }),
        now: () => NOW, elapsedMs: () => 0,
      }));
      if (to === 'grok') {
        const opened = openDatabase({ path: context.paths.db, timeoutMs: 1000 });
        text = attachOnPreToolUse(opened.db, { conversationId: String(after.conversation_id), toolCallId: 'delivery', now: NOW }) ?? '';
        opened.db.close();
      }
      assert.match(text, /Deployment and timeout verification remain outstanding/);
      assert.equal(context.all('SELECT state FROM work_items')[0].state, 'active');
    });
  });
}
