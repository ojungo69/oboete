/**
 * Test-only failure injection (contracts/cli.md "Environment"): `OBOETE_TEST_FAULT` and
 * `OBOETE_TEST_FAULT_URL` are honoured only when `NODE_ENV=test`, and that comparison lives here
 * once. Only a failure nothing outside the process can stage gets a seam; a missing, corrupt or
 * read-only database, a held write lock, `kill -9`, the paused marker, a `worker_lease` row, a
 * broken CLI on PATH or an oversized payload is induced for real by the test.
 */

function gateOpen(): boolean {
  return process.env.NODE_ENV === 'test';
}

/** True when the named fault is armed; every call site is one inline `if`. */
export function testFault(name: string): boolean {
  return gateOpen() && process.env.OBOETE_TEST_FAULT === name;
}

/** 127.0.0.0/8 and `[::1]`, numeric only, so no resolver decides where the request goes. */
function isLoopback(hostname: string): boolean {
  return hostname === '[::1]' || /^127\.\d+\.\d+\.\d+$/.test(hostname);
}

/** Node routes fetch through HTTP_PROXY when any of these is set; a loopback request would then leave the machine. */
function envProxyEnabled(): boolean {
  return (
    Boolean(process.env.NODE_USE_ENV_PROXY) ||
    [...process.execArgv, process.env.NODE_OPTIONS ?? ''].some((arg) =>
      // Node accepts the flag with `_` in place of `-` in any position.
      arg.replaceAll('_', '-').includes('--use-env-proxy'),
    )
  );
}

/** Header names that carry a provider credential; the loopback stand-in never needs one. */
const CREDENTIAL_HEADERS = ['authorization', 'x-api-key', 'api-key', 'x-goog-api-key'];

/**
 * Sends provider requests to `OBOETE_TEST_FAULT_URL` instead of the preset's host, keeping path,
 * method, body and non-credential headers, so a real 401, 429/3036, length stop, unparsable body,
 * refused connection or silent socket can come from an ephemeral loopback port. Consent is bound
 * to the preset's fixed host (`consentTuple`), not to the runtime destination, so the rewrite
 * refuses anything but numeric loopback over HTTP(S), refuses redirects, drops the credential
 * headers, and stays off entirely when Node's environment proxy is on.
 */
export function faultFetch(inner: typeof globalThis.fetch): typeof globalThis.fetch {
  const base = gateOpen() && !envProxyEnabled() ? process.env.OBOETE_TEST_FAULT_URL : undefined;
  if (base === undefined || !URL.canParse(base)) return inner;
  const target = new URL(base);
  if (!/^https?:$/.test(target.protocol) || !isLoopback(target.hostname)) return inner;
  return async (input, init) => {
    const url = new URL(String(input));
    url.protocol = target.protocol;
    url.host = target.host;
    const headers = new Headers(init?.headers);
    for (const name of CREDENTIAL_HEADERS) headers.delete(name);
    return await inner(url.href, { ...init, headers, redirect: 'error' });
  };
}
