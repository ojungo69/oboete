// Shared fixtures for the quality-debt record tests: an isolated evidence directory, the CLI runner, and the API stub.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

export const script = fileURLToPath(new URL('./quality-debt-record.mjs', import.meta.url));
export const evidence = 'docs/evidence/quality-debt-2026-09';
export const codacyIssues = 'https://app.codacy.com/api/v3/analysis/organizations/gh/ojungo69/repositories/oboete/issues';
export const sonarAnalyses = 'https://sonarcloud.io/api/project_analyses/search';
export const codacyRepository = 'https://app.codacy.com/api/v3/analysis/organizations/gh/ojungo69/repositories/oboete';
export const confirmArgs = ['--confirm', '--sonar-analysis', 'analysis-key', '--codacy-commit', 'commit-sha'];
export const codacyHarnessId = 'cdcb204be8f7e0941ec2d1eca871d4';
export const codacyViewerId = '7001b39120b60918527820572a47897';

export function fixture(t) {
  const cwd = mkdtempSync(join(tmpdir(), 'quality-debt-record-'));
  t.after(() => rmSync(cwd, { recursive: true, force: true }));
  mkdirSync(join(cwd, evidence), { recursive: true });
  for (const service of ['SONAR', 'CODACY']) {
    writeFileSync(join(cwd, `${service}_TOKEN.md`), '# Test credentials\r\nfixture-token\r\nignored\r\n');
  }
  const sonar = [
    { id: 's-worker', rule: 'typescript:S3776', sev: 'CRITICAL', file: 'src/worker/observe.ts', line: 20, msg: 'Complexity' },
    { id: 's-sql', rule: 'plsql:S1192', sev: 'MAJOR', file: 'src/db/migrations/001.sql', line: 3, msg: 'Literal' },
    { id: 's-regexp', rule: 'typescript:S8786', sev: 'MAJOR', file: 'src/worker/observe.ts', line: 3, msg: 'Regular expression' },
  ];
  const codacy = [
    { id: codacyHarnessId, tool: 'Opengrep', pattern: 'Semgrep_fs', cat: 'Security', sub: 'FileAccess', sev: 'Error', file: 'scripts/e2e/probe.mjs', line: 7, msg: 'Path', text: '' },
    { id: codacyViewerId, tool: 'Lizard', pattern: 'Lizard_nloc-medium', cat: 'Complexity', sub: '', sev: 'Warning', file: 'src/viewer/app/main.tsx', line: 12, msg: 'Length', text: '' },
  ];
  const ledger = [
    { service: 'sonar', id: 's-worker', state: 'fixed', where: '#123' },
    { service: 'sonar', id: 's-sql', state: 'excluded', where: 'sonar-project.properties:2' },
    { service: 'sonar', id: 's-regexp', state: 'resolved', where: 'The pattern is a constant.', transition: 'falsepositive', verdict: 'Constant pattern — not applicable' },
    { service: 'codacy', id: codacyHarnessId, state: 'excluded', where: '.codacy.yml:3', verdict: 'Test fixture paths — not applicable' },
    { service: 'codacy', id: codacyViewerId, state: 'fixed', where: '#124' },
  ];
  for (const row of ledger) row.confirmed = 'fixture-confirmation';
  writeJson(cwd, 'sonar-main-issues.json', sonar);
  writeJson(cwd, 'codacy-main-issues.json', codacy);
  writeJson(cwd, 'ledger.json', ledger);
  const allocation = {
    A: ['s-sql', codacyHarnessId], E: ['s-regexp'], B1: [], B2: [], B3: [],
    C1: ['s-worker'], C2: [], C3: [codacyViewerId], C4: [], D: [],
    counts: { A: 2, E: 1, B1: 0, B2: 0, B3: 0, C1: 1, C2: 0, C3: 1, C4: 0, D: 0 },
  };
  writeJson(cwd, 'allocation.json', allocation);
  return { cwd, sonar, codacy, ledger, allocation };
}

export function writeJson(cwd, name, rows) {
  writeFileSync(join(cwd, evidence, name), JSON.stringify(rows));
}

export function readLedger(cwd) {
  return JSON.parse(readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8'));
}

export function readCalls(cwd) {
  const path = join(cwd, 'calls.jsonl');
  return existsSync(path) ? readFileSync(path, 'utf8').trim().split('\n').map(JSON.parse) : [];
}

export const dryRunStub = `
  import os from 'node:os';
  import { syncBuiltinESMExports } from 'node:module';
  os.homedir = () => { throw new Error('credentials accessed during dry-run'); };
  syncBuiltinESMExports();
  globalThis.fetch = () => { throw new Error('request made during dry-run'); };
`;

export function run(cwd, args = [], preload = '') {
  const imports = preload ? ['--import', `data:text/javascript,${encodeURIComponent(preload)}`] : [];
  const result = spawnSync(process.execPath, [...imports, script, ...args], {
    cwd, encoding: 'utf8', timeout: 10_000,
  });
  assert.equal(result.error, undefined);
  return result;
}


// expected is the credential the recorded `authMatches` is compared against. A test that writes a
// token file of its own passes the token it wrote, so the boolean says the reader sent that exact
// value rather than merely something other than the default fixture.
export function apiStub(responses, verification = {}, expected = 'fixture-token') {
  return `
    import { appendFileSync } from 'node:fs';
    import os from 'node:os';
    import timers from 'node:timers/promises';
    import { syncBuiltinESMExports } from 'node:module';
    const record = (event) => appendFileSync('calls.jsonl', JSON.stringify(event) + '\\n');
    os.homedir = () => process.cwd();
    timers.setTimeout = async (ms) => { record({ sleep: ms }); };
    syncBuiltinESMExports();
    const responses = ${JSON.stringify(responses)};
    const verification = ${JSON.stringify({
      sonar: { body: { analyses: [{ key: 'analysis-key', revision: 'commit-sha' }] } },
      codacy: { body: { data: { lastAnalysedCommit: { sha: 'commit-sha', endedAnalysis: '2026-09-07T00:00:00Z' } } } },
      ...verification,
    })};
    globalThis.fetch = (url, options) => {
      const headers = new Headers(options.headers);
      record({
        url: String(url), method: options.method,
        body: headers.get('content-type') === 'application/json' ? JSON.parse(options.body ?? '{}')
          : Object.fromEntries(new URLSearchParams(options.body)),
        authMatches: String(url).startsWith('https://sonarcloud.io/')
          ? headers.get('authorization') === 'Basic ' + Buffer.from(${JSON.stringify(expected)} + ':').toString('base64')
          : headers.get('api-token') === ${JSON.stringify(expected)},
        contentType: headers.get('content-type'), redirect: options.redirect,
      });
      let response;
      if (String(url).startsWith('${sonarAnalyses}')) response = verification.sonar;
      else if (String(url) === '${codacyRepository}') {
        if (headers.has('authorization') || headers.has('api-token')) throw new Error('expected anonymous request');
        response = verification.codacy;
      } else response = responses.shift();
      if (!response) throw new Error('unexpected request');
      if (response.error) {
        const error = new TypeError('Invalid header: ' + (headers.get('authorization') ?? headers.get('api-token')));
        if (response.error === 'reject') return Promise.reject(error);
        throw error;
      }
      if (response.brokenBody) {
        const stream = new ReadableStream({ start(controller) { controller.error(new Error('body lost')); } });
        return new Response(stream, { status: response.status ?? 200 });
      }
      return new Response(response.body === undefined ? null : JSON.stringify(response.body), { status: response.status ?? 200 });
    };
  `;
}
