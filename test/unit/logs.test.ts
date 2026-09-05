import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { test } from 'node:test';

import { cleanEnv, eventBase, runObserveForFixture, toggleDatabase, withFixture, writeConfig } from '../helpers/observe.js';

// Not secret-shaped on purpose: what makes it a credential is the environment variable, not the
// look of the value (FR-016), so the detector cannot pass this test by pattern alone.
const CREDENTIAL = 'oboete-test-credential-value-7f3a9c1e5b2d';

/** Every non-empty regular file under the data directory: logs, spool, the database and its journal. */
function filesUnder(directory: string): string[] {
  return readdirSync(directory, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile())
    .map((entry) => join(entry.parentPath, entry.name))
    .filter((path) => statSync(path).size > 0);
}

function assertNowhereUnder(home: string, value: string): void {
  const files = filesUnder(home);
  assert.ok(files.length > 0, 'the data directory has files to scan');
  for (const file of files) {
    assert.equal(readFileSync(file).includes(value), false, `${file} carries the credential value`);
  }
}

test('FR-016: a provider credential pasted into a session reaches no stored row, spool file, log, pack, memory or doctor output', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: CREDENTIAL });
    writeConfig(fixture, 'openrouter', fixture.env);
    const common = eventBase('s-credential');
    await fixture.capture('SessionStart', { ...common, source: 'startup' });
    await fixture.capture('UserPromptSubmit', {
      ...common,
      prompt_id: 'p1',
      prompt: `Use OBOETE_OPENROUTER_API_KEY=${CREDENTIAL} for the run and tell me what it configures.`,
    });
    await fixture.capture('PreToolUse', {
      ...common,
      prompt_id: 'p1',
      tool_name: 'Bash',
      tool_use_id: 't1',
      tool_input: { command: `OBOETE_OPENROUTER_API_KEY=${CREDENTIAL} node dist/oboete.mjs observe` },
    });
    // One hook without storage: the spool file must be as clean as the database row.
    toggleDatabase(fixture, true);
    try {
      await fixture.capture(
        'Stop',
        { ...common, prompt_id: 'p1', last_assistant_message: `The key ${CREDENTIAL} selects the OpenRouter preset.` },
        'spooled',
      );
    } finally {
      toggleDatabase(fixture, false);
    }
    // A turn without the credential: every row above is secret and summarizes nothing, so this is
    // what gives the worker a batch, the provider a request, and the next session a pack.
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'p2', prompt: 'Explain the retry policy of the uploader.' });
    await fixture.capture('Stop', { ...common, prompt_id: 'p2', last_assistant_message: 'The uploader retries three times.' });
    await fixture.capture('SessionEnd', { ...common, reason: 'prompt_input_exit' });

    fixture.withDb((db) => {
      const contents = db.prepare('SELECT content FROM raw_events WHERE content IS NOT NULL').all().map((row) => String(row.content));
      assert.ok(contents.some((content) => content.includes('OBOETE_OPENROUTER_API_KEY')), 'the events themselves are stored');
      for (const content of contents) assert.equal(content.includes(CREDENTIAL), false, content);
    });
    const spooled = readdirSync(fixture.paths.spool)
      .filter((name) => name.endsWith('.json'))
      .map((name) => readFileSync(join(fixture.paths.spool, name), 'utf8'));
    assert.ok(spooled.some((entry) => entry.includes('OpenRouter preset')), 'the message was spooled');
    for (const entry of spooled) assert.equal(entry.includes(CREDENTIAL), false, entry);

    // The provider fails with a message that quotes the credential. The worker logs an error code and
    // never a message (T063), and appendLog scrubs the process's credentials; this pins the sum.
    await runObserveForFixture(fixture, {
      fetch: async () => {
        throw new Error(`connect ECONNREFUSED while sending Authorization: Bearer ${CREDENTIAL}`);
      },
    });
    const observeLog = readFileSync(fixture.paths.observeLog, 'utf8');
    assert.ok(observeLog.includes('batch'), 'the worker logged its batch');
    assert.equal(observeLog.includes(CREDENTIAL), false, observeLog);

    const pack = (await fixture.capture('SessionStart', { ...eventBase('s-next'), source: 'startup' })).stdout ?? '';
    assert.ok(pack !== '', 'the next session receives a pack');
    assert.equal(pack.includes(CREDENTIAL), false, pack);

    const doctor = spawnSync(process.execPath, [resolve(process.cwd(), 'dist/oboete.mjs'), 'doctor'], {
      env: fixture.env,
      encoding: 'utf8',
    });
    // The command ran and said something (T069 makes it a report; until then it says it is not implemented).
    assert.equal(doctor.error, undefined);
    assert.notEqual(doctor.status, null);
    assert.notEqual(`${doctor.stdout}${doctor.stderr}`, '');
    assert.equal(`${doctor.stdout}${doctor.stderr}`.includes(CREDENTIAL), false);

    assertNowhereUnder(fixture.home, CREDENTIAL);
  });
});
