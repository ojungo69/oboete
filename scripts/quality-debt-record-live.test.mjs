// Exercise live issue searches for Codacy application and public inventory coverage.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

import {
  apiStub, codacyHarnessId, codacyIssue, codacyIssues, codacyViewerId, confirmArgs, evidence, fixture, publicApiStub, readCalls, readLedger, run,
  sonarIssue, sonarOpen, writeJson,
} from './quality-debt-record.test-support.mjs';
import { sonarIssueStates } from './quality-debt-services.mjs';

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

test('--apply-sonar refuses a service FIXED row without requests or ledger writes', (t) => {
  const { cwd, ledger } = fixture(t);
  delete ledger[2].confirmed;
  ledger[2].transitioned = '2026-09-13T00:00:00.000Z';
  writeJson(cwd, 'ledger.json', ledger);
  const before = evidenceText(cwd);
  const result = run(cwd, ['--apply-sonar'], apiStub([
    { body: { issues: [sonarIssue('s-regexp', { status: 'CLOSED', resolution: 'FIXED' })], total: 1 } },
  ]));
  assert.equal(result.status, 1);
  assert.equal(result.stdout, 'REFUSE Sonar s-regexp: closed by the service as FIXED; re-disposition this row as fixed\n');
  assert.match(result.stderr, /1.*s-regexp/);
  assert.deepEqual(readCalls(cwd).map((call) => call.method), ['GET']);
  assert.deepEqual(evidenceText(cwd), before);
});

test('sonarIssueStates reads 501 ids in chunks and returns their real states', async (t) => {
  const ids = Array.from({ length: 501 }, (_, index) => `s-${index}`);
  const calls = [];
  t.mock.method(globalThis, 'fetch', async (url, options) => {
    const params = new URL(url).searchParams;
    assert.equal(params.get('componentKeys'), 'ojungo69_free-mem');
    assert.equal(params.get('branch'), 'main');
    assert.equal(params.get('ps'), '500');
    assert.equal(params.has('resolved'), false);
    assert.deepEqual(options, { method: 'GET', headers: { Authorization: 'Basic fixture' }, redirect: 'manual' });
    const chunk = params.get('issues').split(',');
    calls.push(chunk);
    const issues = chunk.map((id) => sonarIssue(id, id === 's-500' ? { status: 'RESOLVED', resolution: 'WONTFIX' } : {}));
    return new globalThis.Response(JSON.stringify({ issues, total: issues.length }));
  });
  const states = await sonarIssueStates(ids, 'Basic fixture');
  assert.deepEqual(calls, [ids.slice(0, 500), ['s-500']]);
  assert.equal(states.size, 501);
  assert.deepEqual(states.get('s-0'), { status: 'OPEN', resolution: undefined });
  assert.deepEqual(states.get('s-500'), { status: 'RESOLVED', resolution: 'WONTFIX' });
});

for (const [field, value] of [
  ['status', undefined], ['status', null], ['status', 42], ['status', ''],
  ['resolution', null], ['resolution', 42], ['resolution', ''],
]) {
  test(`--apply-sonar rejects invalid ${field}: ${JSON.stringify(value)} before any write`, (t) => {
    const { cwd, ledger } = fixture(t);
    ledger[0].state = 'resolved';
    delete ledger[0].confirmed;
    delete ledger[2].confirmed;
    writeJson(cwd, 'ledger.json', ledger);
    const before = evidenceText(cwd);
    const result = run(cwd, ['--apply-sonar'], apiStub([
      { body: { issues: [sonarIssue('s-worker'), sonarIssue('s-regexp', { [field]: value })], total: 2 } },
    ]));
    assert.equal(result.status, 1);
    assert.equal(result.stderr, `Sonar issues search returned an invalid ${field}\n`);
    assert.deepEqual(readCalls(cwd).map((call) => call.method), ['GET']);
    assert.deepEqual(evidenceText(cwd), before);
  });
}

for (const refusedIndex of [0, 2]) {
  test(`--apply-sonar persists the appliable row with a FIXED row at index ${refusedIndex}`, (t) => {
    const { cwd, ledger } = fixture(t);
    ledger[0].state = 'resolved';
    delete ledger[0].confirmed;
    delete ledger[2].confirmed;
    writeJson(cwd, 'ledger.json', ledger);
    const appliedIndex = refusedIndex === 0 ? 2 : 0;
    const refused = ledger[refusedIndex];
    const applied = ledger[appliedIndex];
    const result = run(cwd, ['--apply-sonar'], apiStub([
      { body: { issues: [sonarIssue(refused.id, { status: 'CLOSED', resolution: 'FIXED' }),
        sonarIssue(applied.id)], total: 2 } }, { status: 200 }, { status: 204 },
    ]));
    assert.equal(result.status, 1);
    assert.equal(result.stderr, `Sonar refused 1 row(s): ${refused.id}\n`);
    assert.ok(result.stdout.includes(`REFUSE Sonar ${refused.id}: closed by the service as FIXED; re-disposition this row as fixed`));
    assert.ok(result.stdout.includes(`APPLY Sonar ${applied.id}: 2 call(s)`));
    const calls = readCalls(cwd).filter((call) => call.url);
    assert.deepEqual(calls.slice(1).map((call) => [call.method, call.url, call.body]), [
      ['POST', 'https://sonarcloud.io/api/issues/do_transition', { issue: applied.id, transition: applied.transition ?? 'wontfix' }],
      ['POST', 'https://sonarcloud.io/api/issues/add_comment', { issue: applied.id, text: applied.where }],
    ]);
    const saved = readLedger(cwd);
    assert.deepEqual(saved[refusedIndex], refused);
    assert.match(saved[appliedIndex].confirmed, /^HTTP 204 /);
    assert.equal(Object.hasOwn(saved[appliedIndex], 'transitioned'), false);
    for (const i of [1, 3, 4]) assert.deepEqual(saved[i], ledger[i]);
  });
}

test('--apply-sonar --dry-run prints all three decisions and only applicable POSTs without writes', (t) => {
  const { cwd, ledger } = fixture(t);
  for (const row of ledger.slice(0, 3)) {
    row.state = 'resolved';
    delete row.confirmed;
  }
  writeJson(cwd, 'ledger.json', ledger);
  const before = evidenceText(cwd);
  const result = run(cwd, ['--apply-sonar', '--dry-run'], publicApiStub([
    { body: { issues: [sonarIssue('s-worker', { status: 'CLOSED', resolution: 'FIXED' }),
      sonarIssue('s-sql', { status: 'RESOLVED', resolution: 'WONTFIX' }), sonarIssue('s-regexp')], total: 3 } },
  ]));
  assert.equal(result.status, 1);
  assert.equal(result.stdout, [
    'REFUSE Sonar s-worker: closed by the service as FIXED; re-disposition this row as fixed',
    'RESOLVED Sonar s-sql: transition already applied, posting the comment',
    'POST https://sonarcloud.io/api/issues/add_comment issue=s-sql&text=sonar-project.properties%3A2',
    'APPLY Sonar s-regexp: 2 call(s)',
    'POST https://sonarcloud.io/api/issues/do_transition issue=s-regexp&transition=falsepositive',
    'POST https://sonarcloud.io/api/issues/add_comment issue=s-regexp&text=The+pattern+is+a+constant.', '',
  ].join('\n'));
  assert.equal(result.stderr, 'Sonar refused 1 row(s): s-worker\n');
  assert.deepEqual(readCalls(cwd).map((call) => [call.method, call.authMatches]), [['GET', false]]);
  assert.deepEqual(evidenceText(cwd), before);
});

for (const transitioned of [false, true]) {
  for (const resolution of ['WONTFIX', 'FALSE-POSITIVE']) {
    test(`--apply-sonar comments a RESOLVED/${resolution} row with transitioned: ${transitioned}`, (t) => {
      const { cwd, ledger } = fixture(t);
      delete ledger[2].confirmed;
      ledger[2].transition = resolution === 'WONTFIX' ? 'wontfix' : 'falsepositive';
      if (transitioned) ledger[2].transitioned = '2026-09-13T00:00:00.000Z';
      writeJson(cwd, 'ledger.json', ledger);
      const result = run(cwd, ['--apply-sonar'], apiStub([
        { body: { issues: [sonarIssue('s-regexp', { status: 'RESOLVED', resolution })], total: 1 } }, { status: 204 },
      ]));
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, 'RESOLVED Sonar s-regexp: transition already applied, posting the comment\n');
      const calls = readCalls(cwd);
      assert.deepEqual(calls.map((call) => call.method), ['GET', 'POST']);
      assert.equal(calls[1].url, 'https://sonarcloud.io/api/issues/add_comment');
      assert.deepEqual(calls[1].body, { issue: 's-regexp', text: ledger[2].where });
      const saved = readLedger(cwd);
      assert.match(saved[2].confirmed, /^HTTP 204 /);
      assert.equal(Object.hasOwn(saved[2], 'transitioned'), false);
    });
  }
}

test('--apply-sonar fails before any write when the search omits a pending id', (t) => {
  const { cwd, ledger } = fixture(t);
  ledger[0].state = 'resolved';
  delete ledger[0].confirmed;
  delete ledger[2].confirmed;
  writeJson(cwd, 'ledger.json', ledger);
  const before = evidenceText(cwd);
  const result = run(cwd, ['--apply-sonar'], apiStub([
    { body: { issues: [sonarIssue('s-worker')], total: 1 } },
  ]));
  assert.equal(result.status, 1);
  assert.equal(result.stderr, 'Sonar issues search did not return s-regexp\n');
  assert.deepEqual(readCalls(cwd).map((call) => call.method), ['GET']);
  assert.deepEqual(evidenceText(cwd), before);
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
    Object.assign(row, { state: 'resolved', reason: 'AcceptedUse', transitioned: '2026-09-13T00:00:00.000Z' });
    delete row.confirmed;
  }
  writeJson(cwd, 'ledger.json', ledger);
  const result = run(cwd, ['--apply-codacy'], apiStub([
    { status: 200 },
    { body: { data: [codacyIssue(codacyViewerId)], pagination: { total: 1 } } },
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
  for (const row of saved.slice(3)) assert.equal(Object.hasOwn(row, 'transitioned'), false);
});

test('--confirm clears transition progress after a row is re-dispositioned as fixed', (t) => {
  const { cwd, ledger } = fixture(t);
  Object.assign(ledger[2], { state: 'fixed', where: '#125', transitioned: '2026-09-13T00:00:00.000Z' });
  delete ledger[2].confirmed;
  writeJson(cwd, 'ledger.json', ledger);
  const result = run(cwd, confirmArgs, apiStub([sonarOpen()]));
  assert.equal(result.status, 0, result.stderr);
  const saved = readLedger(cwd);
  assert.equal(saved[2].confirmed, 'analysis-key');
  assert.equal(Object.hasOwn(saved[2], 'transitioned'), false);
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
    { body: { data: observedCodacyIds.map((id) => codacyIssue(id)), pagination: { total: 3 } } },
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
  const { cwd, ledger } = fixture(t);
  ledger[0].state = 'open';
  writeJson(cwd, 'ledger.json', ledger);
  const before = evidenceText(cwd);
  const result = run(cwd, ['--check-live'], publicApiStub([
    { body: { issues: [sonarIssue('s-worker')], paging: { total: 1 } } },
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
    { body: { issues: [sonarIssue('s-new')], paging: { total: 1 } } },
    { body: { data: [codacyIssue(uncoveredCodacyId)], pagination: { total: 1 } } },
  ]));
  assert.equal(result.status, 1);
  assert.equal(result.stdout, '');
  assert.equal(result.stderr, `sonar uncovered 1: s-new\ncodacy uncovered 1: ${uncoveredCodacyId}\n`);
  assert.deepEqual(evidenceText(cwd), before);
});

test('--check-live reports contradicted fixed findings separately and fails without file writes', (t) => {
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
    'sonar contradicted 1: s-rekeyed', `codacy contradicted 1: ${uncoveredCodacyId}`, '',
  ].join('\n'));
  assert.deepEqual(evidenceText(cwd), before);
});

test('--check-live still fails for uncovered ids with no fixed or excluded triple and prints no contradicted group', (t) => {
  const { cwd } = fixture(t);
  const before = evidenceText(cwd);
  const result = run(cwd, ['--check-live'], publicApiStub([
    { body: { issues: [
      { key: 's-file', rule: 'typescript:S3776', component: 'ojungo69_free-mem:src/new.ts' },
      { key: 's-rule', rule: 'typescript:S107', component: 'ojungo69_free-mem:src/worker/observe.ts' },
      { key: 's-service', rule: 'Lizard_nloc-medium', component: 'ojungo69_free-mem:src/viewer/app/main.tsx' },
      { key: 's-resolved', rule: 'typescript:S8786', component: 'ojungo69_free-mem:src/worker/observe.ts' },
    ], paging: { total: 4 } } },
    { body: { data: [
      { issueId: 'a1', patternInfo: { id: 'Lizard_nloc-medium' }, filePath: 'src/new.ts' },
      { issueId: 'a2', patternInfo: { id: 'Lizard_file-nloc-medium' }, filePath: 'src/viewer/app/main.tsx' },
      { issueId: 'a3', patternInfo: { id: 'typescript:S3776' }, filePath: 'src/worker/observe.ts' },
    ], pagination: { total: 3 } } },
  ]));
  assert.equal(result.status, 1);
  assert.equal(result.stdout, '');
  assert.equal(result.stderr, 'sonar uncovered 4: s-file, s-rule, s-service, s-resolved\ncodacy uncovered 3: a1, a2, a3\n');
  assert.deepEqual(evidenceText(cwd), before);
});

for (const state of ['fixed', 'excluded']) {
  test(`--check-live reports covered ids contradicting ${state} dispositions from frozen inventory`, (t) => {
    const { cwd, ledger } = fixture(t);
    for (const index of [0, 4]) {
      Object.assign(ledger[index], { state, rule: 'wrong-rule', file: 'wrong-file' });
    }
    delete ledger[4].confirmed;
    writeJson(cwd, 'ledger.json', ledger);
    const before = evidenceText(cwd);
    const result = run(cwd, ['--check-live'], publicApiStub([
      { body: { issues: [{ key: 's-worker', rule: 'typescript:S3776',
        component: 'ojungo69_free-mem:src/worker/observe.ts' }], total: 1 } },
      { body: { data: [{ issueId: codacyViewerId, patternInfo: { id: 'Lizard_nloc-medium' },
        filePath: 'src/viewer/app/main.tsx' }], pagination: { total: 1 } } },
    ]));
    assert.equal(result.status, 1);
    assert.equal(result.stdout, '');
    assert.equal(result.stderr, `sonar contradicted 1: s-worker\ncodacy contradicted 1: ${codacyViewerId}\n`);
    assert.deepEqual(evidenceText(cwd), before);
  });
}

test('--check-live retains contradicted rule and file evidence from later search pages', (t) => {
  const { cwd, sonar } = fixture(t);
  const firstPage = Array.from({ length: 500 }, (_, index) => sonarIssue(`s-page-${index}`));
  writeJson(cwd, 'sonar-main-issues.json', [...sonar, ...firstPage.map(({ key }) => ({ ...sonar[0], id: key }))]);
  const before = evidenceText(cwd);
  const result = run(cwd, ['--check-live'], publicApiStub({ sonar: [
    { body: { issues: firstPage, paging: { total: 501 } } },
    { body: { issues: [{ key: 's-rekeyed', rule: 'typescript:S3776',
      component: 'ojungo69_free-mem:src/worker/observe.ts' }], paging: { total: 501 } } },
  ], codacy: [
    { body: { data: [codacyIssue(codacyViewerId)], pagination: { total: 2, cursor: 'next' } } },
    { body: { data: [{ issueId: uncoveredCodacyId, patternInfo: { id: 'Lizard_nloc-medium' },
      filePath: 'src/viewer/app/main.tsx' }], pagination: { total: 2 } } },
  ] }));
  assert.equal(result.status, 1);
  assert.equal(result.stderr, [
    'sonar uncovered 1: s-rekeyed', `codacy uncovered 1: ${uncoveredCodacyId}`,
    'sonar contradicted 1: s-rekeyed', `codacy contradicted 1: ${uncoveredCodacyId}`, '',
  ].join('\n'));
  assert.equal(readCalls(cwd).length, 4);
  assert.deepEqual(evidenceText(cwd), before);
});

test('--check-live rejects malformed pages without credentials or file writes', (t) => {
  const { cwd } = fixture(t);
  const before = evidenceText(cwd);
  const result = run(cwd, ['--check-live'], publicApiStub([
    { body: { issues: [], paging: { total: 1 } } },
    { body: { data: [], pagination: {} } },
  ]));
  assert.equal(result.status, 1);
  assert.equal(result.stderr, 'Sonar issues search returned an invalid page\n');
  assert.deepEqual(readCalls(cwd).map((call) => call.url), [
    'https://sonarcloud.io/api/issues/search?componentKeys=ojungo69_free-mem&branch=main&resolved=false&ps=500&p=1',
    `${codacyIssues}/search?limit=100`,
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

for (const [service, field, invalid] of [
  ['Sonar', 'rule', { rule: undefined }], ['Sonar', 'rule', { rule: '' }], ['Sonar', 'rule', { rule: 42 }],
  ['Sonar', 'component', { component: undefined }], ['Sonar', 'component', { component: 42 }],
  ['Sonar', 'component', { component: 'other-project:src/fixture.ts' }],
  ['Sonar', 'component', { component: 'ojungo69_free-mem-wrong:src/fixture.ts' }],
  ['Codacy', 'patternInfo.id', { patternInfo: undefined }],
  ['Codacy', 'patternInfo.id', { patternInfo: { id: '' } }], ['Codacy', 'patternInfo.id', { patternInfo: { id: 42 } }],
  ['Codacy', 'filePath', { filePath: undefined }], ['Codacy', 'filePath', { filePath: '' }], ['Codacy', 'filePath', { filePath: 42 }],
]) {
  test(`--check-live rejects invalid ${service} ${field}: ${JSON.stringify(invalid)}`, (t) => {
    const { cwd } = fixture(t);
    const before = evidenceText(cwd);
    const result = run(cwd, ['--check-live'], publicApiStub([
      { body: { issues: service === 'Sonar' ? [sonarIssue('s-worker', invalid)] : [], total: service === 'Sonar' ? 1 : 0 } },
      { body: { data: service === 'Codacy' ? [codacyIssue(codacyViewerId, invalid)] : [], pagination: {} } },
    ]));
    assert.equal(result.status, 1);
    assert.equal(result.stderr, `${service} issues search returned an invalid ${field}\n`);
    assert.equal(result.stdout, '');
    assert.deepEqual(evidenceText(cwd), before);
  });
}

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
