import assert from 'node:assert/strict';
import { grantVisibility } from '../../src/db/queries.js';
import { randomUUID } from 'node:crypto';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { setTimeout as sleep } from 'node:timers/promises';
import { test } from 'node:test';

import { runInject } from '../../src/injection/pi.js';
import { oboetePaths } from '../../src/paths.js';
import { withTempHome } from '../helpers/home.js';
import { withCapture } from '../helpers/capture.js';
import { openDatabase } from '../../src/db/open.js';
import { detectSync } from '../../src/privacy/detect.js';
import { NOW, insertMemory, insertSession, seedSummary, stdoutOf, withFixture } from '../helpers/inject-fixture.js';

for (const captured of ['before', 'during', 'missing', 'failed', 'secret', 'old-input'] as const) {
  test(`Pi waits for its current prompt before choosing progress: ${captured}`, async () => {
    await withCapture(async (context) => {
      const envelope = { cwd: context.repo, session_id: 'pi-race' };
      await context.capture('pi', 'session_start', { ...envelope, event: 'session_start', payload: { reason: 'startup' } });
      await context.capture('pi', 'input', { ...envelope, event: 'input', prompt_id: randomUUID(),
        payload: { text: 'Finish the original upload.', source: 'interactive' } });
      const db = openDatabase({ path: context.paths.db, timeoutMs: 1000 }).db;
      const origin = db.prepare('SELECT s.repo_id, b.work_id FROM sessions s JOIN work_bindings b ON b.session_id = s.id').get()!;
      db.prepare(`INSERT INTO memories (id, repo_id, type, title, body, content_hash, sensitivity, pinned_at)
        VALUES ('pi-knowledge', ?, 'discovery', 'Project upload policy', 'Use the existing upload helper.', 'pi-knowledge', 'eligible', 1)`).run(origin.repo_id);
      grantVisibility(db, 'pi-knowledge', { audience: 'project', repoId: String(origin.repo_id) }, 'migration', NOW);
      db.close();
      const promptId = randomUUID();
      const prompt = 'New task: Verify the replacement work.';
      const capture = async () => {
        await context.capture('pi', 'input', { ...envelope, event: 'input', prompt_id: promptId,
          payload: { text: prompt, source: 'interactive' } }, captured === 'failed'
          ? { deps: { detect: async () => ({ ok: false, reason: 'detector_error' }) } }
          : captured === 'secret' ? { deps: { detect: (input) => detectSync({ ...input, credentialValues: [prompt] }) } } : {});
      };
      if (captured === 'before' || captured === 'failed' || captured === 'secret') await capture();
      const pending = captured === 'during' ? sleep(10).then(capture) : Promise.resolve();
      const output = await stdoutOf(() => runInject(['--agent', 'pi', '--kind', 'start'], {
        readStdin: () => JSON.stringify({ ...envelope, prompt, ...(captured === 'old-input' ? {} : { prompt_id: promptId }), model: 'gpt-5.6-luna' }),
        now: () => NOW, elapsedMs: () => captured === 'missing' || captured === 'failed' || captured === 'secret' ? 1130 : 0,
      }));
      await pending;
      assert.match(output, /Use the existing upload helper/);
      assert.doesNotMatch(output, /Finish the original upload/);
      if (captured === 'before' || captured === 'during') assert.match(output, /Verify the replacement work/);
      else assert.doesNotMatch(output, /Verify the replacement work|Hidden purpose/);
    });
  });
}

test('Pi start and prompt run through the strict inject child', async () => {
  await withFixture(async (fixture) => {
    seedSummary(fixture);
    insertSession(fixture, {
      id: 's-pi-start',
      agent: 'pi',
      nativeId: 'pi-start',
      model: 'gpt-5.6-luna',
    });
    insertMemory(fixture, {
      id: 'm-pi',
      title: 'SQLite busy timeout',
      body: 'Pi can retrieve the same note.',
    });
    const runtime = (body: Record<string, unknown>) => ({
      readStdin: () => JSON.stringify(body),
      now: () => NOW,
      elapsedMs: () => 0,
    });

    const start = await stdoutOf(() =>
      runInject(
        ['--agent', 'pi', '--kind', 'start'],
        runtime({ cwd: fixture.repo, session_id: 'pi-start', model: 'gpt-5.6-luna' }),
      ),
    );
    assert.ok(start.startsWith('oboete memory context'));

    const prompt = await stdoutOf(() =>
      runInject(
        ['--agent', 'pi', '--kind', 'prompt'],
        runtime({
          cwd: fixture.repo,
          session_id: 'pi-prompt',
          prompt: 'SQLite busy timeout',
          model: 'gpt-5.6-luna',
        }),
      ),
    );
    assert.ok(prompt.includes('SQLite busy timeout'));
    const created = fixture.db
      .prepare("SELECT id, conversation_id FROM sessions WHERE agent = 'pi' AND native_session_id = ?")
      .get('pi-prompt');
    assert.notEqual(created, undefined);
    assert.equal(created?.conversation_id, created?.id);
    assert.equal(
      fixture.db
        .prepare('SELECT session_id FROM injections WHERE session_id = ? LIMIT 1')
        .get(created?.id as string)?.session_id,
      created?.id,
      'the capture child will reuse the same persisted root',
    );

    const invalid = await stdoutOf(() =>
      runInject(
        ['--agent', 'pi', '--kind', 'prompt'],
        runtime({ cwd: fixture.repo, session_id: 'pi-prompt', prompt: 'SQLite', extra: true }),
      ),
    );
    assert.equal(invalid, '', 'the stdin schema is strict');
  });
});

test('paused Pi injection returns zero before reading stdin or creating storage', async () => {
  await withTempHome(async (home) => {
    const paths = oboetePaths(home);
    writeFileSync(paths.paused, 'paused');
    assert.equal(await stdoutOf(() => runInject(['--agent', 'pi', '--kind', 'prompt'], {
      readStdin: () => assert.fail('paused injection must not read stdin'),
    })), '');
    assert.equal(existsSync(paths.db), false);
    assert.equal(existsSync(paths.hookLog), false);
  });
});

test('Pi injection with an expired deadline leaves storage unopened', async () => {
  await withTempHome(async (home) => {
    const paths = oboetePaths(home);
    assert.equal(await stdoutOf(() => runInject(['--agent', 'pi', '--kind', 'prompt'], {
      readStdin: () => JSON.stringify({ cwd: home, session_id: 'too-late', prompt: 'private input' }),
      elapsedMs: () => 301,
    })), '');
    assert.equal(existsSync(paths.db), false);
    const log = readFileSync(paths.hookLog, 'utf8');
    assert.match(log, /inject failed agent=pi reason=Error/);
    assert.doesNotMatch(log, /private input/);
  });
});

test('Pi injection reports an older schema without migrating it', async () => {
  await withFixture(async (fixture) => {
    fixture.db.exec('PRAGMA user_version = 1');
    assert.equal(await stdoutOf(() => runInject(['--agent', 'pi', '--kind', 'start'], {
      readStdin: () => JSON.stringify({ cwd: fixture.repo, session_id: 'old-schema' }),
      elapsedMs: () => 0,
    })), '');
    assert.equal(fixture.db.prepare('PRAGMA user_version').get()?.user_version, 1);
    assert.equal(fixture.db.prepare('SELECT count(*) AS n FROM sessions').get()?.n, 0);
    assert.match(readFileSync(fixture.paths.hookLog, 'utf8'), /agent=pi event=start degraded=index_unavailable/);
  });
});

test('Pi fills a missing model and reuses the persisted conversation epoch and latest turn', async () => {
  await withFixture(async (fixture) => {
    insertSession(fixture, { id: 'pi-root', agent: 'pi', nativeId: 'native-root', epoch: 2 });
    insertSession(fixture, { id: 'pi-resume', agent: 'pi', nativeId: 'native-resume', conversationId: 'pi-root' });
    fixture.db.prepare("INSERT INTO turns (id, session_id, ordinal) VALUES ('pi-turn-1', 'pi-resume', 1), ('pi-turn-2', 'pi-resume', 2)").run();
    insertMemory(fixture, { id: 'm-resumed', title: 'SQLite busy timeout', body: 'Reuse the existing session.' });
    const output = await stdoutOf(() => runInject(['--agent', 'pi', '--kind', 'prompt'], {
      readStdin: () => JSON.stringify({ cwd: fixture.repo, session_id: 'native-resume', prompt: 'SQLite busy timeout', model: 'gpt-5.6-luna' }),
      now: () => NOW, elapsedMs: () => 0,
    }));
    assert.match(output, /SQLite busy timeout/);
    assert.deepEqual(
      { ...fixture.db.prepare("SELECT model, conversation_id FROM sessions WHERE id = 'pi-resume'").get() },
      { model: 'gpt-5.6-luna', conversation_id: 'pi-root' },
    );
    assert.deepEqual(
      { ...fixture.db.prepare('SELECT session_id, conversation_id, context_epoch, turn_id, state FROM injections').get() },
      { session_id: 'pi-resume', conversation_id: 'pi-root', context_epoch: 2, turn_id: 'pi-turn-2', state: 'emitted' },
    );
    assert.equal(fixture.db.prepare("SELECT last_injected_at FROM memories WHERE id = 'm-resumed'").get()?.last_injected_at, NOW);
  });
});
