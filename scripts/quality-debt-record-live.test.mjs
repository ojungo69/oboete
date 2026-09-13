// Exercise live issue searches for Codacy application and public inventory coverage.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

import {
  apiStub, codacyHarnessId, codacyIssues, codacyViewerId, evidence, fixture, publicApiStub, readCalls, readLedger, run, writeJson,
} from './quality-debt-record.test-support.mjs';

const uncoveredCodacyId = '44444444444444444444444444444444';
const observedCodacyIds = [
  codacyHarnessId,
  codacyViewerId,
  '3536c70ee8b5825f074e7598ca528b30',
];

function evidenceText(cwd) {
  return Object.fromEntries(['sonar-main-issues.json', 'codacy-main-issues.json', 'ledger.json', 'allocation.json']
    .map((name) => [name, readFileSync(join(cwd, evidence, name), 'utf8')]));
}

test('--apply-sonar confirms an absent first id and still transitions and comments the present id', (t) => {
  const { cwd, ledger } = fixture(t);
  Object.assign(ledger[0], { state: 'resolved', where: 'Input is bounded.' });
  delete ledger[0].confirmed;
  delete ledger[2].confirmed;
  writeJson(cwd, 'ledger.json', ledger);
  const result = run(cwd, ['--apply-sonar'], apiStub([
    { body: { issues: [{ key: 's-regexp' }], paging: { total: 1 } } },
    { status: 200 }, { status: 204 },
  ]));
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, '');
  const calls = readCalls(cwd).filter((call) => call.url);
  assert.deepEqual(calls.map((call) => [call.method, call.url, call.body]), [
    ['GET', 'https://sonarcloud.io/api/issues/search?componentKeys=ojungo69_free-mem&branch=main&resolved=false&ps=500&p=1', {}],
    ['POST', 'https://sonarcloud.io/api/issues/do_transition', { issue: 's-regexp', transition: 'falsepositive' }],
    ['POST', 'https://sonarcloud.io/api/issues/add_comment', { issue: 's-regexp', text: 'The pattern is a constant.' }],
  ]);
  const saved = readLedger(cwd);
  assert.match(saved[0].confirmed, /^Absent from current Sonar issue search \d{4}-\d\d-\d\dT/);
  assert.equal(saved[0].where, ledger[0].where);
  assert.equal(saved[0].transitioned, undefined);
  assert.match(saved[2].confirmed, /^HTTP 204 /);
  assert.equal(saved[2].transitioned, undefined);
  for (const i of [1, 3, 4]) assert.deepEqual(saved[i], ledger[i]);
});

test('--apply-sonar --dry-run reports skipped ids and previews present calls without credentials or writes', (t) => {
  const { cwd, ledger } = fixture(t);
  Object.assign(ledger[0], { state: 'resolved', where: 'Input is bounded.' });
  delete ledger[0].confirmed;
  delete ledger[2].confirmed;
  writeJson(cwd, 'ledger.json', ledger);
  const before = evidenceText(cwd);
  const result = run(cwd, ['--apply-sonar', '--dry-run'], publicApiStub([
    { body: { issues: [{ key: 's-regexp' }], paging: { total: 1 } } },
  ]));
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, [
    'SKIP Sonar s-worker: absent from current Sonar issue search',
    'POST https://sonarcloud.io/api/issues/do_transition issue=s-regexp&transition=falsepositive',
    'POST https://sonarcloud.io/api/issues/add_comment issue=s-regexp&text=The+pattern+is+a+constant.', '',
  ].join('\n'));
  assert.deepEqual(readCalls(cwd).map((call) => [call.method, call.authMatches]), [['GET', false]]);
  assert.deepEqual(evidenceText(cwd), before);
});

for (const dryRun of [false, true]) {
  test(`--apply-sonar skips an absent transitioned id without changing its progress field (dry-run: ${dryRun})`, (t) => {
    const { cwd, ledger } = fixture(t);
    delete ledger[2].confirmed;
    ledger[2].transitioned = '2026-09-13T00:00:00.000Z';
    writeJson(cwd, 'ledger.json', ledger);
    const stub = dryRun ? publicApiStub : apiStub;
    const result = run(cwd, dryRun ? ['--apply-sonar', '--dry-run'] : ['--apply-sonar'], stub([
      { body: { issues: [], paging: { total: 0 } } },
    ]));
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(readCalls(cwd).map((call) => call.method), ['GET']);
    const saved = readLedger(cwd);
    assert.equal(saved[2].transitioned, ledger[2].transitioned);
    if (dryRun) {
      assert.equal(result.stdout, 'SKIP Sonar s-regexp: absent from current Sonar issue search\n');
      assert.deepEqual(saved, ledger);
    } else {
      assert.match(saved[2].confirmed, /^Absent from current Sonar issue search \d{4}-/);
    }
  });
}

test('--apply-sonar saves an absent first id before a later transition fails', (t) => {
  const { cwd, ledger } = fixture(t);
  ledger[0].state = 'resolved';
  delete ledger[0].confirmed;
  delete ledger[2].confirmed;
  writeJson(cwd, 'ledger.json', ledger);
  const result = run(cwd, ['--apply-sonar'], apiStub([
    { body: { issues: [{ key: 's-regexp' }], paging: { total: 1 } } }, { status: 429 },
  ]));
  assert.equal(result.status, 1);
  assert.equal(result.stderr, 'Sonar s-regexp: do_transition returned HTTP 429\n');
  const saved = readLedger(cwd);
  assert.match(saved[0].confirmed, /^Absent from current Sonar issue search \d{4}-/);
  assert.deepEqual(saved.slice(1), ledger.slice(1));
  assert.deepEqual(readCalls(cwd).filter((call) => call.method === 'POST').map((call) => call.body),
    [{ issue: 's-regexp', transition: 'falsepositive' }]);
});

for (const response of [{ status: 503 }, { body: { issues: [], paging: { total: 1 } } }]) {
  test(`--apply-sonar leaves all rows unchanged when the live search fails: ${JSON.stringify(response)}`, (t) => {
    const { cwd, ledger } = fixture(t);
    ledger[0].state = 'resolved';
    delete ledger[0].confirmed;
    delete ledger[2].confirmed;
    writeJson(cwd, 'ledger.json', ledger);
    const before = evidenceText(cwd);
    const result = run(cwd, ['--apply-sonar'], apiStub([response]));
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Sonar issues search returned (HTTP 503|an invalid page)/);
    assert.equal(result.stdout, '');
    assert.deepEqual(readCalls(cwd).map((call) => call.method), ['GET']);
    assert.deepEqual(evidenceText(cwd), before);
  });
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

test('--check-live reports re-keyed fixed findings separately and fails without file writes', (t) => {
  const { cwd, ledger } = fixture(t);
  // Rule and file belong to the inventory, not hand-edited copies on the disposition row.
  Object.assign(ledger[0], { rule: 'wrong-rule', file: 'wrong-file' });
  Object.assign(ledger[4], { rule: 'wrong-rule', file: 'wrong-file' });
  delete ledger[4].confirmed;
  writeJson(cwd, 'ledger.json', ledger);
  const before = evidenceText(cwd);
  const result = run(cwd, ['--check-live'], publicApiStub([
    { body: { issues: [
      { key: 's-rekeyed', rule: 'typescript:S3776', component: 'ojungo69_free-mem:src/worker/observe.ts', line: 99 },
      { key: 's-new', rule: 'typescript:S3776', component: 'ojungo69_free-mem:src/new.ts' },
    ], paging: { total: 2 } } },
    { body: { data: [{ issueId: uncoveredCodacyId, patternInfo: { id: 'Lizard_nloc-medium' },
      filePath: 'src/viewer/app/main.tsx', lineNumber: 101 }], pagination: { total: 1 } } },
  ]));
  assert.equal(result.status, 1);
  assert.equal(result.stdout, '');
  assert.equal(result.stderr, [
    'sonar uncovered 2: s-rekeyed, s-new', `codacy uncovered 1: ${uncoveredCodacyId}`,
    'sonar re-keyed 1: s-rekeyed', `codacy re-keyed 1: ${uncoveredCodacyId}`, '',
  ].join('\n'));
  assert.deepEqual(evidenceText(cwd), before);
});

test('--check-live still fails for uncovered ids with no fixed triple and prints no re-keyed group', (t) => {
  const { cwd } = fixture(t);
  const before = evidenceText(cwd);
  const result = run(cwd, ['--check-live'], publicApiStub([
    { body: { issues: [
      { key: 's-file', rule: 'typescript:S3776', component: 'ojungo69_free-mem:src/new.ts' },
      { key: 's-rule', rule: 'typescript:S107', component: 'ojungo69_free-mem:src/worker/observe.ts' },
      { key: 's-service', rule: 'Lizard_nloc-medium', component: 'ojungo69_free-mem:src/viewer/app/main.tsx' },
      { key: 's-excluded', rule: 'plsql:S1192', component: 'ojungo69_free-mem:src/db/migrations/001.sql' },
      { key: 's-resolved', rule: 'typescript:S8786', component: 'ojungo69_free-mem:src/worker/observe.ts' },
    ], paging: { total: 5 } } },
    { body: { data: [
      { issueId: 'a1', patternInfo: { id: 'Lizard_nloc-medium' }, filePath: 'src/new.ts' },
      { issueId: 'a2', patternInfo: { id: 'Lizard_file-nloc-medium' }, filePath: 'src/viewer/app/main.tsx' },
      { issueId: 'a3', patternInfo: { id: 'typescript:S3776' }, filePath: 'src/worker/observe.ts' },
      { issueId: 'a4', patternInfo: { id: 'Semgrep_fs' }, filePath: 'scripts/e2e/probe.mjs' },
    ], pagination: { total: 4 } } },
  ]));
  assert.equal(result.status, 1);
  assert.equal(result.stdout, '');
  assert.equal(result.stderr, 'sonar uncovered 5: s-file, s-rule, s-service, s-excluded, s-resolved\ncodacy uncovered 4: a1, a2, a3, a4\n');
  assert.deepEqual(evidenceText(cwd), before);
});

test('--check-live passes with no re-keyed group when all live ids are covered', (t) => {
  const { cwd } = fixture(t);
  const before = evidenceText(cwd);
  const result = run(cwd, ['--check-live'], publicApiStub([
    { body: { issues: [{ key: 's-worker', rule: 'typescript:S3776',
      component: 'ojungo69_free-mem:src/worker/observe.ts' }], paging: { total: 1 } } },
    { body: { data: [{ issueId: codacyViewerId, patternInfo: { id: 'Lizard_nloc-medium' },
      filePath: 'src/viewer/app/main.tsx' }], pagination: { total: 1 } } },
  ]));
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, '');
  assert.equal(result.stderr, '');
  assert.deepEqual(evidenceText(cwd), before);
});

test('--check-live retains re-keyed rule and file evidence from later search pages', (t) => {
  const { cwd, sonar } = fixture(t);
  const firstPage = Array.from({ length: 500 }, (_, index) => ({ key: `s-page-${index}` }));
  writeJson(cwd, 'sonar-main-issues.json', [...sonar, ...firstPage.map(({ key }) => ({ ...sonar[0], id: key }))]);
  const before = evidenceText(cwd);
  const result = run(cwd, ['--check-live'], publicApiStub([
    { body: { issues: firstPage, paging: { total: 501 } } },
    { body: { issues: [{ key: 's-rekeyed', rule: 'typescript:S3776',
      component: 'ojungo69_free-mem:src/worker/observe.ts' }], paging: { total: 501 } } },
    { body: { data: [{ issueId: codacyViewerId }], pagination: { total: 2, cursor: 'next' } } },
    { body: { data: [{ issueId: uncoveredCodacyId, patternInfo: { id: 'Lizard_nloc-medium' },
      filePath: 'src/viewer/app/main.tsx' }], pagination: { total: 2 } } },
  ]));
  assert.equal(result.status, 1);
  assert.equal(result.stderr, [
    'sonar uncovered 1: s-rekeyed', `codacy uncovered 1: ${uncoveredCodacyId}`,
    'sonar re-keyed 1: s-rekeyed', `codacy re-keyed 1: ${uncoveredCodacyId}`, '',
  ].join('\n'));
  assert.equal(readCalls(cwd).length, 4);
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
