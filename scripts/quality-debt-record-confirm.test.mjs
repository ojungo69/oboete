// Exercise the quality-debt record CLI's --confirm mode against a stubbed fetch: revision checks, paging, and labels.
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';

import {
  apiStub, codacyIssues, codacyRepository, codacyViewerId, confirmArgs, evidence, fixture, readCalls, run,
  sonarAnalyses, writeJson,
} from './quality-debt-record.test-support.mjs';

const codacyOpenId = '33333333333333333333333333333333';
const codacyOtherId = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const codacyLastId = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
const generatedCodacyId = (index) => (0x1000 + index).toString(16).padStart(30 + index % 3, '0');

test('--confirm reads all pages once per service and only confirms absent planned ids', (t) => {
  const { cwd, ledger } = fixture(t);
  for (const row of ledger) delete row.confirmed;
  ledger.push({ service: 'sonar', id: 's-confirmed', state: 'fixed', confirmed: 'older-analysis' });
  ledger.push({ service: 'codacy', id: codacyOpenId, state: 'open' });
  writeJson(cwd, 'ledger.json', ledger);
  const result = run(cwd, confirmArgs, apiStub([
    { body: { issues: Array.from({ length: 500 }, (_, i) => ({ key: `s-other-${i}` })), paging: { total: 501 } } },
    { body: { issues: [{ key: 's-sql' }], total: 501 } },
    { body: { data: Array.from({ length: 100 }, (_, i) => ({ issueId: generatedCodacyId(i) })), pagination: { cursor: 'next/+=', total: 101 } } },
    { body: { data: [{ issueId: codacyViewerId }], pagination: { total: 101 } } },
  ]));
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /sonar: 1 confirmed, 1 still open/);
  assert.match(result.stdout, /codacy: 1 confirmed, 1 still open/);
  assert.match(result.stdout, /sonar s-sql: still open/);
  assert.ok(result.stdout.includes(`codacy ${codacyViewerId}: still open`), result.stdout);
  const text = readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8');
  const saved = JSON.parse(text);
  assert.equal(saved[0].confirmed, 'analysis-key');
  assert.equal(saved[3].confirmed, 'commit-sha');
  for (const i of [1, 2, 4, 5, 6]) assert.deepEqual(saved[i], ledger[i]);
  assert.equal(text, `${JSON.stringify(saved, null, 1)}\n`);
  const calls = readFileSync(join(cwd, 'calls.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
  assert.equal(calls.length, 6);
  for (const [i, call] of calls.slice(2, 4).entries()) {
    assert.equal(call.url, `https://sonarcloud.io/api/issues/search?componentKeys=ojungo69_free-mem&branch=main&resolved=false&ps=500&p=${i + 1}`);
    assert.equal(call.method, 'GET');
    assert.equal(call.authMatches, true);
    assert.equal(call.redirect, 'manual');
  }
  for (const [i, call] of calls.slice(4).entries()) {
    assert.equal(call.url, `${codacyIssues}/search?limit=100${i ? '&cursor=next%2F%2B%3D' : ''}`);
    assert.equal(call.method, 'POST');
    assert.deepEqual(call.body, {});
    assert.equal(call.contentType, 'application/json');
    assert.equal(call.redirect, 'manual');
  }
});

for (const service of ['sonar', 'codacy']) {
  for (const failure of [{ status: 503 }, { body: {} }, { body: service === 'sonar'
    ? { issues: [{}], total: 502 } : { data: [{}], pagination: {} } }, { body: service === 'sonar'
    ? { issues: [{ key: 'last' }], total: 502 } : { data: [{ issueId: codacyLastId }], pagination: {} } }, { body: service === 'sonar'
    ? { issues: [{ key: 'last' }], total: 501 } : { data: [{ issueId: codacyLastId }], pagination: { total: 2 } } }]) {
    test(`--confirm leaves ${service} unchanged after an incomplete or invalid page: ${JSON.stringify(failure)}`, (t) => {
      const { cwd, ledger } = fixture(t);
      delete ledger[service === 'sonar' ? 0 : 4].confirmed;
      writeJson(cwd, 'ledger.json', ledger);
      const before = readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8');
      const first = service === 'sonar'
        ? { issues: Array.from({ length: 500 }, (_, i) => ({ key: `other-${i}` })), total: 502 }
        : { data: [{ issueId: codacyOtherId }], pagination: { cursor: 'next', total: 3 } };
      const result = run(cwd, confirmArgs, apiStub([{ body: first }, failure]));
      assert.equal(result.status, 1);
      assert.ok(result.stderr.toLowerCase().includes(service), result.stderr);
      assert.equal(readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8'), before);
      const calls = readFileSync(join(cwd, 'calls.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
      assert.equal(calls.length, 3);
      assert.ok(calls.every((call) => call.url.startsWith(service === 'sonar' ? 'https://sonarcloud.io/' : 'https://app.codacy.com/')));
    });
  }
}

test('--confirm accepts empty open sets for both services', (t) => {
  const { cwd, ledger } = fixture(t);
  for (const row of ledger) delete row.confirmed;
  writeJson(cwd, 'ledger.json', ledger);
  const result = run(cwd, confirmArgs, apiStub([
    { body: { issues: [], paging: { total: 0 } } }, { body: { data: [], pagination: {} } },
  ]));
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, 'sonar: 2 confirmed, 0 still open\ncodacy: 2 confirmed, 0 still open\n');
  const saved = JSON.parse(readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8'));
  for (const i of [0, 1]) assert.equal(saved[i].confirmed, 'analysis-key');
  for (const i of [3, 4]) assert.equal(saved[i].confirmed, 'commit-sha');
  assert.deepEqual(saved[2], ledger[2]);
  const calls = readCalls(cwd);
  assert.deepEqual(calls.slice(0, 2), [
    { url: `${sonarAnalyses}?project=ojungo69_free-mem&branch=main&ps=1`, method: 'GET',
      body: {}, authMatches: true, contentType: null, redirect: 'manual' },
    { url: codacyRepository, method: 'GET', body: {}, authMatches: false, contentType: null, redirect: 'manual' },
  ]);
  assert.equal(calls.length, 4);
});

for (const [service, response, message] of [
  ['sonar', { body: { analyses: [{ key: 'latest-key', revision: 'unrelated-revision' }, { key: 'analysis-key' }] } },
    'Sonar: the latest analysis is not analysis-key'],
  ['sonar', { body: { analyses: [] } }, 'Sonar: the latest analysis is not analysis-key'],
  ['sonar', { body: { analyses: [{ key: 'fixture-token' }] } }, 'Sonar: the latest analysis is not analysis-key'],
  ['sonar', { body: { analyses: [{ key: 'analysis-key', revision: 'other-sha' }] } }, 'Sonar: analysis analysis-key is not of commit commit-sha'],
  ['sonar', { body: { analyses: [{ key: 'analysis-key' }] } }, 'Sonar: analysis analysis-key is not of commit commit-sha'],
  // The issue search has no commit selector, so its answer is only evidence for the last analysed commit.
  ['codacy', { body: { data: { lastAnalysedCommit: { sha: 'newer-sha', endedAnalysis: '2026-09-07T00:00:00Z' } } } },
    'Codacy: the last analysed commit is not commit-sha'],
  ['codacy', { body: { data: { lastAnalysedCommit: { sha: 'fixture-token' } } } }, 'Codacy: the last analysed commit is not commit-sha'],
  ['codacy', { body: {} }, 'Codacy: the last analysed commit is not commit-sha'],
  ...[undefined, '', ' \t'].map((endedAnalysis) => ['codacy',
    { body: { data: { lastAnalysedCommit: { sha: 'commit-sha', startedAnalysis: '2026-09-07T00:00:00Z', endedAnalysis } } } },
    'Codacy: analysis of commit-sha has not finished']),
  ...['Sonar', 'Codacy'].flatMap((name) => [
    [name.toLowerCase(), { status: 503 }, `${name} ${name === 'Sonar' ? 'analyses search' : 'repository'} returned HTTP 503`],
    [name.toLowerCase(), { brokenBody: true }, `${name} returned a body that is not JSON`],
    ...['throw', 'reject'].map((error) => [name.toLowerCase(), { error }, `${name} request failed: TypeError`]),
  ]),
]) {
  test(`--confirm rejects ${service} verification before issue queries or writes: ${JSON.stringify(response)}`, (t) => {
    const { cwd, ledger } = fixture(t);
    delete ledger[0].confirmed;
    delete ledger[4].confirmed;
    writeJson(cwd, 'ledger.json', ledger);
    const before = readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8');
    const result = run(cwd, confirmArgs, apiStub([], { [service]: response }));
    assert.equal(result.status, 1);
    assert.equal(result.stderr, `${message}\n`);
    assert.equal(result.stdout, '');
    const expected = [`${sonarAnalyses}?project=ojungo69_free-mem&branch=main&ps=1`];
    if (service === 'codacy') expected.push(codacyRepository);
    assert.deepEqual(readCalls(cwd).map((call) => call.url), expected);
    assert.equal(readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8'), before);
  });
}

for (const [name, responses, message] of [
  ['repeated id across pages without a total', [
    { body: { data: [{ issueId: codacyOtherId }], pagination: { cursor: 'next' } } },
    { body: { data: [{ issueId: codacyOtherId }], pagination: {} } },
  ], 'Codacy issues search returned a repeated id'],
  ['null total on the first page', [{ body: { data: [], pagination: { total: null } } }],
    'Codacy issues search returned an invalid total'],
  ['null total after a known total', [
    { body: { data: [{ issueId: codacyOtherId }], pagination: { cursor: 'next', total: 2 } } },
    { body: { data: [], pagination: { total: null } } },
  ], 'Codacy issues search total changed between pages'],
]) {
  test(`--confirm rejects a Codacy ${name} before any confirmation or ledger write`, (t) => {
    const { cwd, ledger } = fixture(t);
    delete ledger[4].confirmed;
    writeJson(cwd, 'ledger.json', ledger);
    const before = readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8');
    const result = run(cwd, confirmArgs, apiStub(responses));
    assert.equal(result.status, 1);
    assert.equal(result.stderr, `${message}\n`);
    assert.equal(result.stdout, '');
    assert.equal(readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8'), before);
  });
}

test('--confirm avoids requests when every planned row is already confirmed', (t) => {
  const { cwd } = fixture(t);
  const result = run(cwd, confirmArgs, apiStub([]));
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, 'sonar: 0 confirmed, 0 still open\ncodacy: 0 confirmed, 0 still open\n');
  assert.equal(existsSync(join(cwd, 'calls.jsonl')), false);
});
