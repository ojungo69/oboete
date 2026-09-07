import assert from 'node:assert/strict';
import { existsSync, rmSync } from 'node:fs';
import { isAbsolute, resolve } from 'node:path';
import { test } from 'node:test';

import { replayHome } from '../../src/fixture/replay.js';

function withEnv(value: string | undefined, run: () => void): void {
  const before = process.env.OBOETE_HOME;
  if (value === undefined) delete process.env.OBOETE_HOME;
  else process.env.OBOETE_HOME = value;
  try {
    run();
  } finally {
    if (before === undefined) delete process.env.OBOETE_HOME;
    else process.env.OBOETE_HOME = before;
  }
}

test('--home and a set OBOETE_HOME name a directory the replay does not own', () => {
  withEnv('/tmp/oboete-replay-env-home', () => {
    const flag = replayHome({ home: 'relative-home' });
    assert.equal(flag.home, resolve('relative-home'));
    assert.equal(flag.createdHome, false);

    const env = replayHome({});
    assert.equal(env.home, '/tmp/oboete-replay-env-home');
    assert.equal(env.createdHome, false);
  });
});

test('an unset or empty OBOETE_HOME makes a temporary home this run owns and removes', () => {
  for (const value of [undefined, '']) {
    withEnv(value, () => {
      const made = replayHome({});
      assert.equal(isAbsolute(made.home), true);
      assert.equal(existsSync(made.home), true);
      // The empty case used to report false here, so the directory it made was left behind.
      assert.equal(made.createdHome, true);
      rmSync(made.home, { recursive: true, force: true });
    });
  }
});
