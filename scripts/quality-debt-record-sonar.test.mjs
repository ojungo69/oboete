// Exercise Sonar application decisions against the service's live status and resolution.
import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  apiStub, evidenceText, fixture, publicApiStub, readCalls, readLedger, run, sonarIssue, writeJson,
} from './quality-debt-record.test-support.mjs';
import { sonarIssueStates } from './quality-debt-services.mjs';

for (const resolution of ['FIXED', 'REMOVED', undefined]) {
  test(`--apply-sonar refuses CLOSED/${resolution} without POSTs or ledger writes`, (t) => {
    const { cwd, ledger } = fixture(t);
    delete ledger[2].confirmed;
    ledger[2].transitioned = '2026-09-13T00:00:00.000Z';
    writeJson(cwd, 'ledger.json', ledger);
    const before = evidenceText(cwd);
    const result = run(cwd, ['--apply-sonar'], apiStub([
      { body: { issues: [sonarIssue('s-regexp', { status: 'CLOSED', resolution })], total: 1 } },
    ]));
    assert.equal(result.status, 1);
    assert.equal(result.stdout, `REFUSE Sonar s-regexp: closed by the service (resolution ${resolution}); re-disposition this row\n`);
    assert.match(result.stderr, /1.*s-regexp/);
    assert.deepEqual(readCalls(cwd).map((call) => call.method), ['GET']);
    assert.deepEqual(evidenceText(cwd), before);
  });
}

test('sonarIssueStates reads 201 ids in chunks and returns their real states', async (t) => {
  const ids = Array.from({ length: 201 }, (_, index) => `s-${index}`);
  const calls = [];
  t.mock.method(globalThis, 'fetch', async (url, options) => {
    const params = new URL(url).searchParams;
    assert.equal(params.get('componentKeys'), 'ojungo69_free-mem');
    assert.equal(params.get('branch'), 'main');
    assert.equal(params.get('ps'), '100');
    assert.equal(params.has('resolved'), false);
    assert.deepEqual(options, { method: 'GET', headers: { Authorization: 'Basic fixture' }, redirect: 'manual' });
    const chunk = params.get('issues').split(',');
    calls.push(chunk);
    const issues = chunk.map((id) => ({ key: id, ...(id === 's-200' ? { status: 'RESOLVED', resolution: 'WONTFIX' } : { status: 'OPEN' }) }));
    return new globalThis.Response(JSON.stringify({ issues, total: issues.length }));
  });
  const states = await sonarIssueStates(ids, 'Basic fixture');
  assert.deepEqual(calls, [ids.slice(0, 100), ids.slice(100, 200), ['s-200']]);
  assert.equal(states.size, 201);
  assert.deepEqual(states.get('s-0'), { status: 'OPEN', resolution: undefined });
  assert.deepEqual(states.get('s-200'), { status: 'RESOLVED', resolution: 'WONTFIX' });
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

for (const missing of [false, true]) {
  for (const refusedIndex of [0, 2]) {
    test(`--apply-sonar persists the appliable row with a ${missing ? 'missing' : 'CLOSED/FIXED'} row at index ${refusedIndex}`, (t) => {
      const { cwd, ledger } = fixture(t);
      ledger[0].state = 'resolved';
      delete ledger[0].confirmed;
      delete ledger[2].confirmed;
      writeJson(cwd, 'ledger.json', ledger);
      const appliedIndex = refusedIndex === 0 ? 2 : 0;
      const refused = ledger[refusedIndex];
      const applied = ledger[appliedIndex];
      const issues = [sonarIssue(applied.id)];
      if (!missing) issues.push(sonarIssue(refused.id, { status: 'CLOSED', resolution: 'FIXED' }));
      const result = run(cwd, ['--apply-sonar'], apiStub([
        { body: { issues, total: issues.length } }, { status: 200 }, { status: 204 },
      ]));
      assert.equal(result.status, 1);
      assert.equal(result.stderr, `Sonar refused 1 row(s): ${refused.id}\n`);
      const reason = missing ? 'the issue search does not report this id' : 'closed by the service (resolution FIXED)';
      assert.ok(result.stdout.includes(`REFUSE Sonar ${refused.id}: ${reason}; re-disposition this row`));
      assert.ok(result.stdout.includes(`APPLY Sonar ${applied.id}: the transition then the comment`));
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
}

for (const [transition, resolution, expected] of [
  ['falsepositive', 'WONTFIX', 'FALSE-POSITIVE'], ['wontfix', 'FALSE-POSITIVE', 'WONTFIX'],
  ['falsepositive', undefined, 'FALSE-POSITIVE'], ['wontfix', 'FIXED', 'WONTFIX'],
]) {
  test(`--apply-sonar refuses RESOLVED/${resolution} for a ${transition} row without writes`, (t) => {
    const { cwd, ledger } = fixture(t);
    delete ledger[2].confirmed;
    Object.assign(ledger[2], { transition, transitioned: '2026-09-13T00:00:00.000Z' });
    writeJson(cwd, 'ledger.json', ledger);
    const before = evidenceText(cwd);
    const result = run(cwd, ['--apply-sonar'], apiStub([
      { body: { issues: [sonarIssue('s-regexp', { status: 'RESOLVED', resolution })], total: 1 } },
    ]));
    assert.equal(result.status, 1);
    assert.equal(result.stdout, `REFUSE Sonar s-regexp: resolved by the service (resolution ${resolution}, expected ${expected}); re-disposition this row\n`);
    assert.equal(result.stderr, 'Sonar refused 1 row(s): s-regexp\n');
    assert.deepEqual(readCalls(cwd).map((call) => call.method), ['GET']);
    assert.deepEqual(evidenceText(cwd), before);
  });
}

for (const status of ['OPEN', 'CONFIRMED', 'REOPENED']) {
  test(`--apply-sonar transitions live ${status} despite a stale transitioned marker`, (t) => {
    const { cwd, ledger } = fixture(t);
    delete ledger[2].confirmed;
    ledger[2].transitioned = '2026-09-13T00:00:00.000Z';
    writeJson(cwd, 'ledger.json', ledger);
    const result = run(cwd, ['--apply-sonar'], apiStub([
      { body: { issues: [sonarIssue('s-regexp', { status, rule: undefined, component: undefined })], total: 1 } },
      { status: 200 }, { status: 204 },
    ]));
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, 'APPLY Sonar s-regexp: the transition then the comment\n');
    assert.deepEqual(readCalls(cwd).filter((call) => call.method === 'POST').map((call) => [call.url, call.body]), [
      ['https://sonarcloud.io/api/issues/do_transition', { issue: 's-regexp', transition: 'falsepositive' }],
      ['https://sonarcloud.io/api/issues/add_comment', { issue: 's-regexp', text: ledger[2].where }],
    ]);
    const saved = readLedger(cwd);
    assert.match(saved[2].confirmed, /^HTTP 204 /);
    assert.equal(Object.hasOwn(saved[2], 'transitioned'), false);
  });
}

test('--apply-sonar aggregates all refusal reasons after persisting an appliable row', (t) => {
  const { cwd, sonar, ledger } = fixture(t);
  for (const row of ledger.slice(0, 3)) {
    row.state = 'resolved';
    delete row.confirmed;
  }
  sonar.push({ ...sonar[0], id: 's-open' });
  ledger.push({ ...ledger[0], id: 's-open' });
  writeJson(cwd, 'sonar-main-issues.json', sonar);
  writeJson(cwd, 'ledger.json', ledger);
  const result = run(cwd, ['--apply-sonar'], apiStub([
    { body: { issues: [sonarIssue('s-worker', { status: 'CLOSED', resolution: 'REMOVED' }),
      sonarIssue('s-regexp', { status: 'RESOLVED', resolution: 'WONTFIX' }), sonarIssue('s-open')], total: 3 } },
    { status: 200 }, { status: 204 },
  ]));
  assert.equal(result.status, 1);
  assert.equal(result.stdout, [
    'REFUSE Sonar s-worker: closed by the service (resolution REMOVED); re-disposition this row',
    'REFUSE Sonar s-sql: the issue search does not report this id; re-disposition this row',
    'REFUSE Sonar s-regexp: resolved by the service (resolution WONTFIX, expected FALSE-POSITIVE); re-disposition this row',
    'APPLY Sonar s-open: the transition then the comment', '',
  ].join('\n'));
  assert.equal(result.stderr, 'Sonar refused 3 row(s): s-worker, s-sql, s-regexp\n');
  assert.deepEqual(readCalls(cwd).filter((call) => call.method === 'POST').map((call) => call.body.issue), ['s-open', 's-open']);
  const saved = readLedger(cwd);
  assert.deepEqual(saved.slice(0, 5), ledger.slice(0, 5));
  assert.match(saved[5].confirmed, /^HTTP 204 /);
  assert.equal(Object.hasOwn(saved[5], 'transitioned'), false);
});

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
    'REFUSE Sonar s-worker: closed by the service (resolution FIXED); re-disposition this row',
    'RESOLVED Sonar s-sql: transition already applied, posting the comment',
    'POST https://sonarcloud.io/api/issues/add_comment issue=s-sql&text=sonar-project.properties%3A2',
    'APPLY Sonar s-regexp: the transition then the comment',
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

for (const response of [
  { status: 503 }, { body: { issues: [], paging: { total: 1 } } },
  { body: { issues: [sonarIssue('s-worker')], total: 2 } },
]) {
  test(`--apply-sonar leaves all rows unchanged when the live search fails: ${JSON.stringify(response)}`, (t) => {
    const { cwd, ledger } = fixture(t);
    ledger[0].state = 'resolved';
    delete ledger[0].confirmed;
    delete ledger[2].confirmed;
    writeJson(cwd, 'ledger.json', ledger);
    const before = evidenceText(cwd);
    const result = run(cwd, ['--apply-sonar'], apiStub([response]));
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Sonar issues search returned (HTTP 503|an invalid page|an incomplete result)/);
    assert.equal(result.stdout, '');
    assert.deepEqual(readCalls(cwd).map((call) => call.method), ['GET']);
    assert.deepEqual(evidenceText(cwd), before);
  });
}
