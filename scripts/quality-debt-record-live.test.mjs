// Exercise live issue searches for Codacy application and public inventory coverage.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

import {
  apiStub, codacyHarnessId, codacyIssue, codacyIssues, codacyViewerId, evidence, evidenceText, fixture, publicApiStub, readCalls, readLedger, run,
  sonarIssue, writeJson,
} from './quality-debt-record.test-support.mjs';

const uncoveredCodacyId = '44444444444444444444444444444444';
const observedCodacyIds = [
  codacyHarnessId,
  codacyViewerId,
  '3536c70ee8b5825f074e7598ca528b30',
];

test('--apply-codacy PATCHes pending ids even when absent from the current issue search', (t) => {
  const { cwd, ledger } = fixture(t);
  for (const row of ledger.slice(3)) {
    Object.assign(row, { state: 'resolved', reason: 'AcceptedUse' });
    delete row.confirmed;
  }
  writeJson(cwd, 'ledger.json', ledger);
  const searched = run(cwd, ['--check-live'], publicApiStub([
    { body: { issues: [], total: 0 } },
    { body: { data: [codacyIssue(codacyViewerId)], pagination: { total: 1 } } },
  ]));
  assert.equal(searched.status, 0, searched.stderr);
  const searches = readCalls(cwd).length;
  const result = run(cwd, ['--apply-codacy'], apiStub([
    { status: 200 }, { status: 204 }, { status: 204 },
  ]));
  assert.equal(result.status, 0, result.stderr);
  const calls = readCalls(cwd).slice(searches).filter((call) => call.url);
  assert.deepEqual(calls.map((call) => call.url), [
    'https://app.codacy.com/api/v3/user', `${codacyIssues}/${codacyHarnessId}`, `${codacyIssues}/${codacyViewerId}`,
  ]);
  assert.deepEqual(calls.map((call) => call.method), ['GET', 'PATCH', 'PATCH']);
  const saved = readLedger(cwd);
  assert.equal(saved[3].where, ledger[3].where);
  assert.match(saved[3].confirmed, /^HTTP 204 \d{4}-\d\d-\d\dT/);
  assert.match(saved[4].confirmed, /^HTTP 204 /);
});

test('--apply-codacy refuses a missing record and confirms the later row before reporting refusals', (t) => {
  const { cwd, ledger } = fixture(t);
  for (const row of ledger.slice(3)) {
    Object.assign(row, { state: 'resolved', reason: 'AcceptedUse' });
    delete row.confirmed;
  }
  writeJson(cwd, 'ledger.json', ledger);
  const result = run(cwd, ['--apply-codacy'], apiStub([{ status: 200 }, { status: 404 }, { status: 204 }]));
  assert.equal(result.status, 1);
  assert.equal(result.stdout,
    `REFUSE Codacy ${codacyHarnessId}: the service does not hold this issue record; re-disposition this row\n`);
  assert.equal(result.stderr, `Codacy refused 1 row(s): ${codacyHarnessId}\n`);
  assert.deepEqual(readCalls(cwd).filter((call) => call.method === 'PATCH').map((call) => call.url),
    [`${codacyIssues}/${codacyHarnessId}`, `${codacyIssues}/${codacyViewerId}`]);
  const saved = readLedger(cwd);
  assert.deepEqual(saved.slice(0, 4), ledger.slice(0, 4));
  assert.match(saved[4].confirmed, /^HTTP 204 /);
});

for (const status of [401, 403, 503]) {
  test(`--apply-codacy aborts at HTTP ${status} before attempting the later row`, (t) => {
    const { cwd, ledger } = fixture(t);
    for (const row of ledger.slice(3)) {
      Object.assign(row, { state: 'resolved', reason: 'AcceptedUse' });
      delete row.confirmed;
    }
    writeJson(cwd, 'ledger.json', ledger);
    const path = join(cwd, evidence, 'ledger.json');
    const before = readFileSync(path, 'utf8');
    const result = run(cwd, ['--apply-codacy'], apiStub([{ status: 200 }, { status }]));
    assert.equal(result.status, 1);
    assert.equal(result.stderr, `Codacy ${codacyHarnessId}: PATCH returned HTTP ${status}\n`);
    assert.equal(result.stdout, '');
    assert.deepEqual(readCalls(cwd).filter((call) => call.url).map((call) => call.url), [
      'https://app.codacy.com/api/v3/user', `${codacyIssues}/${codacyHarnessId}`,
    ]);
    assert.equal(readFileSync(path, 'utf8'), before);
  });
}

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
  test(`--check-live rejects ${name} without credentials or local writes`, (t) => {
    const { cwd } = fixture(t);
    const path = join(cwd, evidence, 'ledger.json');
    const before = readFileSync(path, 'utf8');
    const result = run(cwd, ['--check-live'], publicApiStub([
      { body: { issues: [], total: 0 } }, { body: { data: [{ issueId }], pagination: { total: 1 } } },
    ]));
    assert.equal(result.status, 1);
    assert.equal(result.stderr, 'Codacy issues search returned an invalid id\n');
    assert.equal(result.stdout, '');
    assert.deepEqual(readCalls(cwd).filter((call) => call.url).map((call) => call.url), [
      'https://sonarcloud.io/api/issues/search?componentKeys=ojungo69_free-mem&branch=main&resolved=false&ps=500&p=1',
      `${codacyIssues}/search?limit=100`,
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

for (const problem of ['duplicate', 'unknown']) {
  test(`--check-live rejects ${problem} ledger ids before deriving claims or making requests`, (t) => {
    const { cwd, ledger } = fixture(t);
    const row = { ...ledger[0], state: 'open', confirmed: undefined };
    if (problem === 'unknown') row.id = 's-unknown';
    ledger.push(row);
    writeJson(cwd, 'ledger.json', ledger);
    const before = evidenceText(cwd);
    const result = run(cwd, ['--check-live'], publicApiStub([
      { body: { issues: [sonarIssue('s-worker')], total: 1 } },
      { body: { data: [], pagination: { total: 0 } } },
    ]));
    assert.equal(result.status, 1);
    const reason = problem === 'duplicate' ? 'duplicate in ledger' : 'not in inventory';
    assert.equal(result.stderr, `sonar:${row.id} ${reason}\n`);
    assert.equal(result.stdout, '');
    assert.deepEqual(readCalls(cwd), []);
    assert.deepEqual(evidenceText(cwd), before);
  });
}

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

test('--check-live keeps uncovered and contradicted groups around invalid issues from both services', (t) => {
  const { cwd } = fixture(t);
  const before = evidenceText(cwd);
  const result = run(cwd, ['--check-live'], publicApiStub([
    { body: { issues: [
      sonarIssue('s-new'),
      sonarIssue('s-worker', { component: 'ojungo69_free-mem' }),
      sonarIssue('s-unreadable', { component: 'ojungo69_free-mem' }),
      sonarIssue('s-regexp', { rule: null }),
      sonarIssue('s-regrown', { component: 'ojungo69_free-mem:src/worker/observe.ts' }),
    ], total: 5 } },
    { body: { data: [
      codacyIssue(uncoveredCodacyId, { filePath: 'src/viewer/app/main.tsx' }),
      codacyIssue('bad', { filePath: null }), codacyIssue('a1'),
    ], pagination: { total: 3 } } },
  ]));
  assert.equal(result.status, 1);
  assert.equal(result.stdout, '');
  assert.equal(result.stderr, [
    'sonar uncovered 3: s-new, s-unreadable, s-regrown', `codacy uncovered 3: ${uncoveredCodacyId}, bad, a1`,
    'sonar contradicted 3: s-worker, s-regexp, s-regrown', `codacy contradicted 1: ${uncoveredCodacyId}`,
    'sonar invalid 3: s-worker: Sonar issues search returned an invalid component, '
      + 's-unreadable: Sonar issues search returned an invalid component, s-regexp: Sonar issues search returned an invalid rule',
    'codacy invalid 1: bad: Codacy issues search returned an invalid filePath', '',
  ].join('\n'));
  assert.deepEqual(evidenceText(cwd), before);
});

test('--check-live reports contradicted fixed findings separately and fails without file writes', (t) => {
  const { cwd, ledger } = fixture(t);
  // Rule and file belong to the inventory, not hand-edited copies on the disposition row.
  Object.assign(ledger[0], { rule: 'wrong-rule', file: 'wrong-file' });
  Object.assign(ledger[4], { rule: 'wrong-rule', file: 'wrong-file' });
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

test('--check-live contradicts a reopened Sonar resolution and never a returned Codacy ignore', (t) => {
  const { cwd, ledger } = fixture(t);
  // `openSonarIssues` passes `resolved=false`, so a confirmed `resolved` Sonar id coming back means
  // the resolution was reopened. Codacy's search takes no ignore filter and returns ignored issues,
  // so the same shape there proves nothing and must stay out of the group.
  Object.assign(ledger[3], { state: 'resolved', where: 'The path is operator-owned.', reason: 'FalsePositive' });
  writeJson(cwd, 'ledger.json', ledger);
  const before = evidenceText(cwd);
  const result = run(cwd, ['--check-live'], publicApiStub([
    { body: { issues: [
      { key: 's-regexp', rule: 'typescript:S8786', component: 'ojungo69_free-mem:src/worker/observe.ts', line: 3 },
    ], paging: { total: 1 } } },
    { body: { data: [{ issueId: codacyHarnessId, patternInfo: { id: 'Semgrep_fs' },
      filePath: 'scripts/e2e/probe.mjs', lineNumber: 7 }], pagination: { total: 1 } } },
  ]));
  assert.equal(result.status, 1);
  assert.equal(result.stdout, '');
  assert.equal(result.stderr, 'sonar contradicted 1: s-regexp\n');
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
  test(`--check-live reports confirmed ${state} ids even after their code moves`, (t) => {
    const { cwd, ledger } = fixture(t);
    for (const index of [0, 4]) {
      Object.assign(ledger[index], { state, rule: 'wrong-rule', file: 'wrong-file' });
    }
    writeJson(cwd, 'ledger.json', ledger);
    const before = evidenceText(cwd);
    const result = run(cwd, ['--check-live'], publicApiStub([
      { body: { issues: [{ key: 's-worker', rule: 'typescript:S3776',
        component: 'ojungo69_free-mem:src/moved.ts' }], total: 1 } },
      { body: { data: [{ issueId: codacyViewerId, patternInfo: { id: 'Lizard_nloc-medium' },
        filePath: 'src/moved.ts' }], pagination: { total: 1 } } },
    ]));
    assert.equal(result.status, 1);
    assert.equal(result.stdout, '');
    assert.equal(result.stderr, `sonar contradicted 1: s-worker\ncodacy contradicted 1: ${codacyViewerId}\n`);
    assert.deepEqual(evidenceText(cwd), before);
  });

  test(`--check-live makes no contradiction claim for unconfirmed ${state} rows`, (t) => {
    const { cwd, ledger } = fixture(t);
    for (const index of [0, 4]) {
      ledger[index].state = state;
      ledger[index].confirmed = ' \t';
    }
    writeJson(cwd, 'ledger.json', ledger);
    const before = evidenceText(cwd);
    const result = run(cwd, ['--check-live'], publicApiStub([
      { body: { issues: [sonarIssue('s-worker', { component: 'ojungo69_free-mem:src/worker/observe.ts' })], total: 1 } },
      { body: { data: [codacyIssue(codacyViewerId, { filePath: 'src/viewer/app/main.tsx' })], pagination: { total: 1 } } },
    ]));
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, '');
    assert.equal(result.stderr, '');
    assert.deepEqual(evidenceText(cwd), before);
  });
}

for (const state of ['open', 'fixed', 'excluded', 'resolved']) {
  test(`--check-live makes no triple claim when a confirmed fixed row has a planned ${state} sibling`, (t) => {
    const { cwd, sonar, codacy, ledger } = fixture(t);
    sonar.push({ ...sonar[0], id: 's-sibling' });
    codacy.push({ ...codacy[1], id: uncoveredCodacyId });
    for (const [index, id] of [[0, 's-sibling'], [4, uncoveredCodacyId]]) {
      ledger.push({ ...ledger[index], id, state, confirmed: undefined });
    }
    writeJson(cwd, 'sonar-main-issues.json', sonar);
    writeJson(cwd, 'codacy-main-issues.json', codacy);
    writeJson(cwd, 'ledger.json', ledger);
    const before = evidenceText(cwd);
    const result = run(cwd, ['--check-live'], publicApiStub([
      { body: { issues: [sonarIssue('s-sibling', { component: 'ojungo69_free-mem:src/worker/observe.ts' })], total: 1 } },
      { body: { data: [codacyIssue(uncoveredCodacyId, { filePath: 'src/viewer/app/main.tsx' })], pagination: { total: 1 } } },
    ]));
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, '');
    assert.equal(result.stderr, '');
    assert.deepEqual(evidenceText(cwd), before);
  });
}

test('--check-live retains contradicted rule and file evidence from later search pages', (t) => {
  const { cwd, sonar } = fixture(t);
  const firstPage = Array.from({ length: 500 }, (_, index) => sonarIssue(`s-page-${index}`));
  writeJson(cwd, 'sonar-main-issues.json', [...sonar, ...firstPage.map(({ key }) => ({ ...sonar[0], id: key, file: 'src/fixture.ts' }))]);
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
    'sonar contradicted 1: s-rekeyed', `codacy contradicted 2: ${codacyViewerId}, ${uncoveredCodacyId}`, '',
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
  // A blank field would build a triple that matches no claim, so the contradiction would go unreported.
  ['Sonar', 'rule', { rule: '   ' }],
  ['Sonar', 'component', { component: undefined }], ['Sonar', 'component', { component: 42 }],
  ['Sonar', 'component', { component: 'other-project:src/fixture.ts' }],
  ['Sonar', 'component', { component: 'ojungo69_free-mem-wrong:src/fixture.ts' }],
  ['Sonar', 'component', { component: 'ojungo69_free-mem:' }],
  ['Sonar', 'component', { component: 'ojungo69_free-mem:   ' }],
  ['Codacy', 'patternInfo.id', { patternInfo: undefined }],
  ['Codacy', 'patternInfo.id', { patternInfo: { id: '' } }], ['Codacy', 'patternInfo.id', { patternInfo: { id: 42 } }],
  ['Codacy', 'patternInfo.id', { patternInfo: { id: '  ' } }],
  ['Codacy', 'filePath', { filePath: undefined }], ['Codacy', 'filePath', { filePath: '' }], ['Codacy', 'filePath', { filePath: 42 }],
  ['Codacy', 'filePath', { filePath: '\t' }],
]) {
  test(`--check-live rejects invalid ${service} ${field}: ${JSON.stringify(invalid)}`, (t) => {
    const { cwd } = fixture(t);
    const before = evidenceText(cwd);
    const result = run(cwd, ['--check-live'], publicApiStub([
      { body: { issues: service === 'Sonar' ? [sonarIssue('s-worker', invalid)] : [], total: service === 'Sonar' ? 1 : 0 } },
      { body: { data: service === 'Codacy' ? [codacyIssue(codacyViewerId, invalid)] : [], pagination: {} } },
    ]));
    assert.equal(result.status, 1);
    const id = service === 'Sonar' ? 's-worker' : codacyViewerId;
    assert.equal(result.stderr, `${service.toLowerCase()} contradicted 1: ${id}\n`
      + `${service.toLowerCase()} invalid 1: ${id}: ${service} issues search returned an invalid ${field}\n`);
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
