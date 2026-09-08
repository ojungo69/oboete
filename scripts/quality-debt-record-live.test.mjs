// Exercise live issue searches for Codacy application and public inventory coverage.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

import {
  apiStub, codacyHarnessId, codacyIssues, codacyViewerId, evidence, fixture, readCalls, readLedger, run, writeJson,
} from './quality-debt-record.test-support.mjs';

const uncoveredCodacyId = '44444444444444444444444444444444';
const observedCodacyIds = [
  codacyHarnessId,
  codacyViewerId,
  '3536c70ee8b5825f074e7598ca528b30',
];

function publicApiStub(responses) {
  return `${apiStub(responses)}
    os.homedir = () => { throw new Error('credentials accessed during --check-live'); };
    syncBuiltinESMExports();
    const publicFetch = globalThis.fetch;
    globalThis.fetch = (url, init) => {
      const headers = new Headers(init.headers);
      if (headers.has('authorization') || headers.has('api-token')) throw new Error('authentication sent during --check-live');
      return publicFetch(url, init);
    };
  `;
}

function evidenceText(cwd) {
  return Object.fromEntries(['sonar-main-issues.json', 'codacy-main-issues.json', 'ledger.json', 'allocation.json']
    .map((name) => [name, readFileSync(join(cwd, evidence, name), 'utf8')]));
}

test('--apply-codacy confirms snapshot-absent ids and PATCHes present ids', (t) => {
  const { cwd, ledger } = fixture(t);
  for (const row of ledger.slice(3)) {
    Object.assign(row, { state: 'resolved', reason: 'AcceptedUse' });
    delete row.confirmed;
  }
  writeJson(cwd, 'ledger.json', ledger);
  const result = run(cwd, ['--apply-codacy'], apiStub([
    { status: 200 },
    { body: { data: [{ issueId: codacyViewerId }], pagination: { total: 1 } } },
    { status: 204 },
  ]));
  assert.equal(result.status, 0, result.stderr);
  const calls = readCalls(cwd).filter((call) => call.url);
  assert.deepEqual(calls.map((call) => call.url), [
    'https://app.codacy.com/api/v3/user', `${codacyIssues}/search?limit=100`, `${codacyIssues}/${codacyViewerId}`,
  ]);
  assert.deepEqual(calls.map((call) => call.method), ['GET', 'POST', 'PATCH']);
  const saved = readLedger(cwd);
  assert.equal(saved[3].where, ledger[3].where);
  assert.match(saved[3].confirmed, /^Absent from current Codacy issue search \d{4}-\d\d-\d\dT/);
  assert.match(saved[4].confirmed, /^HTTP 204 /);
});

test('--apply-codacy leaves the ledger unchanged when the live search fails before PATCH', (t) => {
  const { cwd, ledger } = fixture(t);
  for (const row of ledger.slice(3)) {
    Object.assign(row, { state: 'resolved', reason: 'AcceptedUse' });
    delete row.confirmed;
  }
  writeJson(cwd, 'ledger.json', ledger);
  const path = join(cwd, evidence, 'ledger.json');
  const before = readFileSync(path, 'utf8');
  const result = run(cwd, ['--apply-codacy'], apiStub([{ status: 200 }, { status: 503 }]));
  assert.equal(result.status, 1);
  assert.equal(result.stderr, 'Codacy issues search returned HTTP 503\n');
  assert.equal(result.stdout, '');
  assert.deepEqual(readCalls(cwd).filter((call) => call.url).map((call) => call.url), [
    'https://app.codacy.com/api/v3/user', `${codacyIssues}/search?limit=100`,
  ]);
  assert.equal(readFileSync(path, 'utf8'), before);
});

test('--apply-codacy preserves and PATCHes current ids at observed 30, 31, and 32 character widths', (t) => {
  const { cwd, codacy, ledger } = fixture(t);
  const rows = observedCodacyIds.map((id, index) => ({
    ...ledger[4], id, state: 'resolved', reason: 'AcceptedUse', where: `Reviewed row ${index + 1}.`,
  }));
  for (const row of rows) delete row.confirmed;
  ledger.splice(3, 2, ...rows);
  writeJson(cwd, 'codacy-main-issues.json', observedCodacyIds.map((id) => ({ ...codacy[1], id })));
  writeJson(cwd, 'ledger.json', ledger);
  const result = run(cwd, ['--apply-codacy'], apiStub([
    { status: 200 },
    { body: { data: observedCodacyIds.map((issueId) => ({ issueId })), pagination: { total: 3 } } },
    { status: 204 }, { status: 204 }, { status: 204 },
  ]));
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(readCalls(cwd).filter((call) => call.method === 'PATCH').map((call) => call.url),
    observedCodacyIds.map((id) => `${codacyIssues}/${id}`));
  for (const [index, row] of readLedger(cwd).slice(3).entries()) {
    assert.equal(row.id, observedCodacyIds[index]);
    assert.match(row.confirmed, /^HTTP 204 /);
  }
});

for (const [name, issueId] of [
  ['a non-hex id', 'g'.repeat(32)],
  ['an uppercase id', 'A'.repeat(32)],
  ['a hexadecimal id with a trailing newline', `${'a'.repeat(32)}\n`],
  ['an empty id', ''],
]) {
  test(`--apply-codacy rejects ${name} before PATCH or local writes`, (t) => {
    const { cwd, ledger } = fixture(t);
    for (const row of ledger.slice(3)) {
      Object.assign(row, { state: 'resolved', reason: 'AcceptedUse' });
      delete row.confirmed;
    }
    writeJson(cwd, 'ledger.json', ledger);
    const path = join(cwd, evidence, 'ledger.json');
    const before = readFileSync(path, 'utf8');
    const result = run(cwd, ['--apply-codacy'], apiStub([
      { status: 200 }, { body: { data: [{ issueId }], pagination: { total: 1 } } },
    ]));
    assert.equal(result.status, 1);
    assert.equal(result.stderr, 'Codacy issues search returned an invalid id\n');
    assert.equal(result.stdout, '');
    assert.deepEqual(readCalls(cwd).filter((call) => call.url).map((call) => call.url), [
      'https://app.codacy.com/api/v3/user', `${codacyIssues}/search?limit=100`,
    ]);
    assert.equal(readFileSync(path, 'utf8'), before);
  });
}

test('--check-live accepts covered and empty public issue sets without credentials or file writes', (t) => {
  const { cwd } = fixture(t);
  const before = evidenceText(cwd);
  const result = run(cwd, ['--check-live'], publicApiStub([
    { body: { issues: [{ key: 's-worker' }], paging: { total: 1 } } },
    { body: { data: [], pagination: { total: 0 } } },
  ]));
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, '');
  assert.equal(result.stderr, '');
  const calls = readCalls(cwd);
  assert.deepEqual(calls.map((call) => call.url), [
    'https://sonarcloud.io/api/issues/search?componentKeys=ojungo69_free-mem&branch=main&resolved=false&ps=500&p=1',
    `${codacyIssues}/search?limit=100`,
  ]);
  assert.deepEqual(calls.map((call) => call.authMatches), [false, false]);
  assert.deepEqual(evidenceText(cwd), before);
});

test('--check-live reports uncovered ids from both services without file writes', (t) => {
  const { cwd } = fixture(t);
  const before = evidenceText(cwd);
  const result = run(cwd, ['--check-live'], publicApiStub([
    { body: { issues: [{ key: 's-new' }], paging: { total: 1 } } },
    { body: { data: [{ issueId: uncoveredCodacyId }], pagination: { total: 1 } } },
  ]));
  assert.equal(result.status, 1);
  assert.equal(result.stdout, '');
  assert.equal(result.stderr, `sonar uncovered 1: s-new\ncodacy uncovered 1: ${uncoveredCodacyId}\n`);
  assert.deepEqual(evidenceText(cwd), before);
});

test('--check-live rejects malformed pages without credentials or file writes', (t) => {
  const { cwd } = fixture(t);
  const before = evidenceText(cwd);
  const result = run(cwd, ['--check-live'], publicApiStub([
    { body: { issues: [], paging: { total: 1 } } },
  ]));
  assert.equal(result.status, 1);
  assert.equal(result.stderr, 'Sonar issues search returned an invalid page\n');
  assert.deepEqual(readCalls(cwd).map((call) => call.url), [
    'https://sonarcloud.io/api/issues/search?componentKeys=ojungo69_free-mem&branch=main&resolved=false&ps=500&p=1',
  ]);
  assert.deepEqual(evidenceText(cwd), before);
});

test('--check-live rejects malformed Codacy pages without file writes', (t) => {
  const { cwd } = fixture(t);
  const before = evidenceText(cwd);
  const result = run(cwd, ['--check-live'], publicApiStub([
    { body: { issues: [], paging: { total: 0 } } },
    { body: { data: [], pagination: [] } },
  ]));
  assert.equal(result.status, 1);
  assert.equal(result.stderr, 'Codacy issues search returned an invalid page\n');
  assert.deepEqual(evidenceText(cwd), before);
});

test('--check-live rejects incompatible mode flags without file writes', (t) => {
  const { cwd } = fixture(t);
  const before = evidenceText(cwd);
  for (const args of [['--check-live', '--check'], ['--check-live', '--planned'], ['--check-live', '--dry-run']]) {
    const result = run(cwd, args);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Usage:/);
    assert.equal(result.stdout, '');
  }
  assert.deepEqual(evidenceText(cwd), before);
});
