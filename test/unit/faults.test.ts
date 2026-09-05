// The gate of the test-only fault seam (contracts/cli.md "Environment"): `OBOETE_TEST_FAULT` and
// `OBOETE_TEST_FAULT_URL` do nothing unless `NODE_ENV=test`, and a redirect never leaves loopback.
import assert from 'node:assert/strict';
import { test } from 'node:test';

import { faultFetch, testFault } from '../../src/testing/faults.js';

const VARIABLES = [
  'NODE_ENV',
  'OBOETE_TEST_FAULT',
  'OBOETE_TEST_FAULT_URL',
  'NODE_USE_ENV_PROXY',
  'NODE_OPTIONS',
] as const;

/** Runs `fn` with exactly `env` set (others unset), then restores the process environment. */
function withEnv<T>(env: Partial<Record<(typeof VARIABLES)[number], string>>, fn: () => T): T {
  const saved = VARIABLES.map((name) => [name, process.env[name]] as const);
  try {
    for (const name of VARIABLES) {
      const value = env[name];
      // `process.env.X = undefined` stores the string 'undefined'; unset means delete.
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
    return fn();
  } finally {
    for (const [name, value] of saved) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
  }
}

test('a fault name is honoured only under NODE_ENV=test', () => {
  withEnv({}, () => assert.equal(testFault('pi-throw'), false));
  withEnv({ OBOETE_TEST_FAULT: 'pi-throw' }, () => assert.equal(testFault('pi-throw'), false));
  withEnv({ NODE_ENV: 'production', OBOETE_TEST_FAULT: 'pi-throw' }, () =>
    assert.equal(testFault('pi-throw'), false),
  );
  withEnv({ NODE_ENV: 'test', OBOETE_TEST_FAULT: 'pi-throw' }, () =>
    assert.equal(testFault('pi-throw'), true),
  );
  withEnv({ NODE_ENV: 'test', OBOETE_TEST_FAULT: 'provider-hang' }, () =>
    assert.equal(testFault('pi-throw'), false),
  );
});

test('the provider redirect needs NODE_ENV=test and a loopback target', async () => {
  const seen: [string, RequestInit | undefined][] = [];
  const inner: typeof globalThis.fetch = async (input, init) => {
    seen.push([String(input), init]);
    return new Response('');
  };
  withEnv({ OBOETE_TEST_FAULT_URL: 'http://127.0.0.1:1' }, () =>
    assert.equal(faultFetch(inner), inner),
  );
  withEnv({ NODE_ENV: 'test', OBOETE_TEST_FAULT_URL: 'https://example.com' }, () =>
    assert.equal(faultFetch(inner), inner),
  );
  withEnv({ NODE_ENV: 'test', OBOETE_TEST_FAULT_URL: 'not a url' }, () =>
    assert.equal(faultFetch(inner), inner),
  );
  // A name would let a resolver decide where the credential goes; only numeric loopback passes.
  withEnv({ NODE_ENV: 'test', OBOETE_TEST_FAULT_URL: 'http://localhost:1' }, () =>
    assert.equal(faultFetch(inner), inner),
  );
  withEnv({ NODE_ENV: 'test', OBOETE_TEST_FAULT_URL: 'ftp://127.0.0.1:1' }, () =>
    assert.equal(faultFetch(inner), inner),
  );
  // An environment proxy would carry the rewritten clear-text request off the machine.
  withEnv({ NODE_ENV: 'test', OBOETE_TEST_FAULT_URL: 'http://127.0.0.1:1', NODE_USE_ENV_PROXY: '1' }, () =>
    assert.equal(faultFetch(inner), inner),
  );
  withEnv(
    { NODE_ENV: 'test', OBOETE_TEST_FAULT_URL: 'http://127.0.0.1:1', NODE_OPTIONS: '--use_env-proxy' },
    () => assert.equal(faultFetch(inner), inner),
  );
  const redirected = withEnv({ NODE_ENV: 'test', OBOETE_TEST_FAULT_URL: 'http://127.0.0.1:1' }, () =>
    faultFetch(inner),
  );
  assert.notEqual(redirected, inner);
  await redirected('https://api.cloudflare.com/client/v4/accounts/a/ai/run/m?x=1', {
    method: 'POST',
    headers: { authorization: 'Bearer key', 'content-type': 'application/json' },
    body: '{"a":1}',
  });
  // Path, query, method, body and the content type survive; the credential does not, and a
  // redirect answer from the loopback server is an error, so nothing can reach a third host.
  const [[url, init]] = seen;
  assert.equal(url, 'http://127.0.0.1:1/client/v4/accounts/a/ai/run/m?x=1');
  assert.equal(init?.method, 'POST');
  assert.equal(init?.body, '{"a":1}');
  assert.equal(init?.redirect, 'error');
  const headers = new Headers(init?.headers);
  assert.equal(headers.has('authorization'), false);
  assert.equal(headers.get('content-type'), 'application/json');
});
