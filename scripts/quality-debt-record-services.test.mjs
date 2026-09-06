// Exercise the quality-debt record CLI's service modes (apply-sonar, apply-codacy, confirm) against a stubbed fetch.
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';

import {
  apiStub, codacyIssues, confirmArgs, dryRunStub, evidence, fixture, readCalls, readLedger, run, writeJson,
} from './quality-debt-record.test-support.mjs';

test('--apply-sonar --dry-run prints both calls without reading credentials or sending requests', (t) => {
  const { cwd, ledger } = fixture(t);
  delete ledger[2].confirmed;
  writeJson(cwd, 'ledger.json', ledger);
  const result = run(cwd, ['--apply-sonar', '--dry-run'], dryRunStub);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, [
    'POST https://sonarcloud.io/api/issues/do_transition issue=s-regexp&transition=falsepositive',
    'POST https://sonarcloud.io/api/issues/add_comment issue=s-regexp&text=The+pattern+is+a+constant.', '',
  ].join('\n'));
  assert.deepEqual(readLedger(cwd), ledger);
});


test('--apply-sonar sends authenticated forms in order with a 200 ms pause between calls', (t) => {
  const { cwd, ledger } = fixture(t);
  ledger[0] = { service: 'sonar', id: 's-worker', state: 'resolved', where: 'Input is bounded & constant.' };
  delete ledger[2].confirmed;
  ledger[4].state = 'resolved';
  writeJson(cwd, 'ledger.json', ledger);
  const result = run(cwd, ['--apply-sonar'], apiStub([200, 204, 200, 204].map((status) => ({ status }))));
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, '');
  const events = readFileSync(join(cwd, 'calls.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
  const expectedForms = [
    { issue: 's-worker', transition: 'wontfix' }, { issue: 's-worker', text: 'Input is bounded & constant.' },
    { issue: 's-regexp', transition: 'falsepositive' }, { issue: 's-regexp', text: 'The pattern is a constant.' },
  ];
  assert.equal(events.length, 7);
  events.forEach((event, i) => {
    if (i % 2) assert.deepEqual(event, { sleep: 200 });
    else assert.deepEqual(event, {
      url: `https://sonarcloud.io/api/issues/${i % 4 === 0 ? 'do_transition' : 'add_comment'}`,
      method: 'POST', body: expectedForms[i / 2], authMatches: true,
      contentType: 'application/x-www-form-urlencoded', redirect: 'manual',
    });
  });
  const saved = JSON.parse(readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8'));
  // The recorded status is the comment call's real status (the stub answers 204 to add_comment).
  for (const i of [0, 2]) assert.match(saved[i].confirmed, /^HTTP 204 \d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/);
  for (const i of [1, 3, 4]) assert.deepEqual(saved[i], ledger[i]);
});

for (const statuses of [[403], [200, 429], [302]]) {
  test(`--apply-sonar stops at HTTP ${statuses.at(-1)} without sending later calls`, (t) => {
    const { cwd, ledger, sonar } = fixture(t);
    delete ledger[2].confirmed;
    ledger.push({ service: 'sonar', id: 's-later', state: 'resolved', where: 'This must never be sent.', verdict: 'Bounded — not applicable' });
    writeJson(cwd, 'sonar-main-issues.json', [...sonar, { ...sonar[2], id: 's-later' }]);
    writeJson(cwd, 'ledger.json', ledger);
    const result = run(cwd, ['--apply-sonar'], apiStub(statuses.map((status) => ({ status }))));
    assert.equal(result.status, 1);
    assert.ok(result.stderr.includes('s-regexp'));
    assert.ok(result.stderr.includes(String(statuses.at(-1))));
    assert.doesNotMatch(result.stdout + result.stderr, /fixture-token|Zml4dHVyZS10b2tlbjo=/);
    const events = readFileSync(join(cwd, 'calls.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
    assert.equal(events.filter((event) => event.url).length, statuses.length);
    const saved = readLedger(cwd);
    if (statuses.length > 1) {
      assert.match(saved[2].transitioned, /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/);
      delete saved[2].transitioned;
    }
    assert.deepEqual(saved, ledger);
  });
}

test('--apply-sonar rejects empty reasons before changing any issue', (t) => {
  const { cwd, ledger } = fixture(t);
  delete ledger[2].confirmed;
  ledger[2].where = '';
  writeJson(cwd, 'ledger.json', ledger);
  const result = run(cwd, ['--apply-sonar', '--dry-run']);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /s-regexp.*reason/);
  assert.equal(result.stdout, '');
});

test('--apply-codacy --dry-run prints only resolved Codacy PATCH calls and writes nothing', (t) => {
  const { cwd, ledger } = fixture(t);
  for (const row of ledger.slice(3)) {
    Object.assign(row, { state: 'resolved', reason: 'TestCode' });
    delete row.confirmed;
  }
  writeJson(cwd, 'ledger.json', ledger);
  const before = readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8');
  const result = run(cwd, ['--apply-codacy', '--dry-run'], dryRunStub);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, ledger.slice(3).map((row) =>
    `PATCH ${codacyIssues}/${row.id} ${JSON.stringify({ ignored: true, reason: 'TestCode', comment: row.where })}\n`).join(''));
  assert.equal(readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8'), before);
});

for (const status of [401, 302]) {
  test(`--apply-codacy rejects the token at HTTP ${status} before any PATCH`, (t) => {
    const { cwd, ledger } = fixture(t);
    Object.assign(ledger[4], { state: 'resolved', reason: 'AcceptedUse' });
    delete ledger[4].confirmed;
    writeJson(cwd, 'ledger.json', ledger);
    const result = run(cwd, ['--apply-codacy'], apiStub([{ status }]));
    assert.equal(result.status, 1);
    assert.equal(result.stderr.trim(), 'Codacy token rejected');
    const calls = readFileSync(join(cwd, 'calls.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
    assert.equal(calls.length, 1);
    assert.equal(calls[0].url, 'https://app.codacy.com/api/v3/user');
    assert.equal(calls[0].method, 'GET');
    assert.equal(calls[0].authMatches, true);
    assert.equal(calls[0].redirect, 'manual');
    assert.deepEqual(JSON.parse(readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8')), ledger);
  });
}

for (const status of [204, 429]) {
  test(`--apply-codacy saves each success and handles a later HTTP ${status}`, (t) => {
    const { cwd, ledger, codacy } = fixture(t);
    for (const row of ledger.slice(3)) {
      Object.assign(row, { state: 'resolved', reason: 'AcceptedUse', where: 'A cohesive module & bounded input.' });
      delete row.confirmed;
    }
    if (status === 429) {
      ledger.push({ ...ledger[4], id: 'c-later' });
      writeJson(cwd, 'codacy-main-issues.json', [...codacy, { ...codacy[1], id: 'c-later' }]);
    }
    writeJson(cwd, 'ledger.json', ledger);
    const result = run(cwd, ['--apply-codacy'], apiStub([{ status: 200 }, { status: 200 }, { status }]));
    assert.equal(result.status, status === 429 ? 1 : 0, result.stderr);
    if (status === 429) assert.match(result.stderr, /c-viewer.*429/);
    assert.doesNotMatch(result.stdout + result.stderr, /fixture-token/);
    const events = readFileSync(join(cwd, 'calls.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
    assert.equal(events.length, 5);
    for (const i of [1, 3]) assert.deepEqual(events[i], { sleep: 200 });
    for (const [i, row] of ledger.slice(3, 5).entries()) {
      assert.deepEqual(events[2 + i * 2], {
        url: `${codacyIssues}/${row.id}`, method: 'PATCH',
        body: { ignored: true, reason: row.reason, comment: row.where },
        authMatches: true, contentType: 'application/json', redirect: 'manual',
      });
    }
    const text = readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8');
    const saved = JSON.parse(text);
    assert.match(saved[3].confirmed, /^HTTP 200 \d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/);
    if (status === 204) assert.match(saved[4].confirmed, /^HTTP 204 /);
    else assert.deepEqual(saved.slice(4), ledger.slice(4));
    assert.deepEqual(saved.slice(0, 3), ledger.slice(0, 3));
    assert.equal(text, `${JSON.stringify(saved, null, 1)}\n`);
  });
}

test('--apply-sonar resumes only the second row after its comment fails', (t) => {
  const { cwd, ledger, sonar } = fixture(t);
  delete ledger[2].confirmed;
  ledger.push({ ...ledger[2], id: 's-later' });
  writeJson(cwd, 'sonar-main-issues.json', [...sonar, { ...sonar[2], id: 's-later' }]);
  writeJson(cwd, 'ledger.json', ledger);
  const result = run(cwd, ['--apply-sonar'], apiStub([200, 204, 200, 429].map((status) => ({ status }))));
  assert.equal(result.status, 1);
  assert.match(result.stderr, /s-later.*429/);
  const text = readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8');
  const saved = JSON.parse(text);
  assert.match(saved[2].confirmed, /^HTTP 204 /);
  assert.match(saved.at(-1).transitioned, /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/);
  assert.equal(saved.at(-1).confirmed, undefined);
  assert.equal(text, `${JSON.stringify(saved, null, 1)}\n`);
  const events = readCalls(cwd).length;
  const rerun = run(cwd, ['--apply-sonar'], apiStub([{ status: 204 }]));
  assert.equal(rerun.status, 0, rerun.stderr);
  assert.deepEqual(readCalls(cwd).slice(events), [{
    url: 'https://sonarcloud.io/api/issues/add_comment', method: 'POST',
    body: { issue: 's-later', text: 'The pattern is a constant.' }, authMatches: true,
    contentType: 'application/x-www-form-urlencoded', redirect: 'manual',
  }]);
  const resumed = readLedger(cwd);
  assert.deepEqual(resumed.slice(0, -1), saved.slice(0, -1));
  // A completed row keeps only `confirmed`: clearing it is enough to run both calls again.
  assert.equal(resumed.at(-1).transitioned, undefined);
  assert.match(resumed.at(-1).confirmed, /^HTTP 204 /);
});

test('--apply-sonar records a transition whose success body cannot be discarded', (t) => {
  // HTTP 200 means the service applied the transition; a body stream that errors afterwards must
  // not lose that fact, or the rerun would repeat a transition SonarCloud refuses on a RESOLVED issue.
  const { cwd, ledger } = fixture(t);
  delete ledger[2].confirmed;
  writeJson(cwd, 'ledger.json', ledger);
  const result = run(cwd, ['--apply-sonar'], apiStub([{ status: 200, brokenBody: true }, { status: 429 }]));
  assert.equal(result.status, 1);
  assert.match(result.stderr, /s-regexp: add_comment returned HTTP 429/);
  assert.match(readLedger(cwd)[2].transitioned, /^\d{4}-/);
  const events = readCalls(cwd).length;
  const rerun = run(cwd, ['--apply-sonar'], apiStub([{ status: 200 }]));
  assert.equal(rerun.status, 0, rerun.stderr);
  assert.equal(readCalls(cwd).slice(events).length, 1);
  assert.equal(readCalls(cwd).at(-1).url, 'https://sonarcloud.io/api/issues/add_comment');
  assert.equal(readLedger(cwd)[2].transitioned, undefined);
  assert.match(readLedger(cwd)[2].confirmed, /^HTTP 200 /);
});

test('--apply-sonar persists a successful transition and retries only the failed comment', (t) => {
  const { cwd, ledger } = fixture(t);
  delete ledger[2].confirmed;
  writeJson(cwd, 'ledger.json', ledger);
  const failed = run(cwd, ['--apply-sonar'], apiStub([{ status: 200 }, { status: 429 }]));
  assert.equal(failed.status, 1);
  assert.match(failed.stderr, /s-regexp: add_comment returned HTTP 429/);
  const saved = readLedger(cwd);
  assert.match(saved[2].transitioned, /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/);
  assert.equal(saved[2].confirmed, undefined);
  const preview = run(cwd, ['--apply-sonar', '--dry-run'], dryRunStub);
  assert.equal(preview.status, 0, preview.stderr);
  assert.equal(preview.stdout, 'POST https://sonarcloud.io/api/issues/add_comment issue=s-regexp&text=The+pattern+is+a+constant.\n');
  assert.deepEqual(readLedger(cwd), saved);
  const events = readCalls(cwd).length;
  const resumed = run(cwd, ['--apply-sonar'], apiStub([{ status: 200 }]));
  assert.equal(resumed.status, 0, resumed.stderr);
  assert.deepEqual(readCalls(cwd).slice(events), [{
    url: 'https://sonarcloud.io/api/issues/add_comment', method: 'POST',
    body: { issue: 's-regexp', text: 'The pattern is a constant.' }, authMatches: true,
    contentType: 'application/x-www-form-urlencoded', redirect: 'manual',
  }]);
  assert.equal(readLedger(cwd)[2].transitioned, undefined);
  assert.match(readLedger(cwd)[2].confirmed, /^HTTP 200 /);
});

for (const args of [['--apply-sonar'], ['--apply-sonar', '--dry-run']]) {
  test(`${args.join(' ')} skips confirmed rows without accessing credentials`, (t) => {
    const { cwd, ledger } = fixture(t);
    const result = run(cwd, args, dryRunStub);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, '');
    assert.deepEqual(readLedger(cwd), ledger);
  });
}


const serviceModes = [
  { args: ['--apply-sonar'], service: 'sonar', index: 2 },
  { args: ['--apply-sonar', '--dry-run'], service: 'sonar', index: 2 },
  { args: ['--apply-codacy'], service: 'codacy', index: 4 },
  { args: ['--apply-codacy', '--dry-run'], service: 'codacy', index: 4 },
  { args: confirmArgs, service: 'sonar', index: 0 },
  { args: confirmArgs, service: 'codacy', index: 4 },
];

for (const { args, service, index } of serviceModes) {
  for (const problem of ['unknown', 'duplicate']) {
    test(`${args.join(' ')} rejects a later ${problem} ${service} id before all requests and writes`, (t) => {
      const { cwd, ledger } = fixture(t);
      for (const row of ledger) delete row.confirmed;
      if (args[0] === '--apply-codacy') {
        for (const row of ledger.slice(3)) Object.assign(row, { state: 'resolved', reason: 'AcceptedUse' });
      }
      const invalid = { ...ledger[index] };
      if (problem === 'unknown') invalid.id = service === 'sonar' ? 'c-viewer' : 's-worker';
      ledger.push(invalid);
      writeJson(cwd, 'ledger.json', ledger);
      const path = join(cwd, evidence, 'ledger.json');
      const before = readFileSync(path, 'utf8');
      const result = run(cwd, args, apiStub([]));
      assert.equal(result.status, 1);
      assert.ok(result.stderr.includes(`${service}:${invalid.id}`));
      assert.match(result.stderr, problem === 'unknown' ? /not in inventory/ : /duplicate/);
      assert.equal(result.stdout, '');
      assert.deepEqual(readCalls(cwd), []);
      assert.equal(readFileSync(path, 'utf8'), before);
    });
  }
}

for (const [service, args] of [['SONAR', ['--apply-sonar']], ['CODACY', ['--apply-codacy']], ['SONAR', confirmArgs]]) {
  for (const [name, token] of [['carriage return', 'fixture\rtoken'], ['NUL', 'fixture\0token'], ['trailing space', 'fixture-token ']]) {
    test(`${args[0]} rejects a ${name} in the token without disclosing it`, (t) => {
      const { cwd, ledger } = fixture(t);
      for (const row of ledger) delete row.confirmed;
      Object.assign(ledger[4], { state: 'resolved', reason: 'AcceptedUse' });
      writeJson(cwd, 'ledger.json', ledger);
      writeFileSync(join(cwd, `${service}_TOKEN.md`), `# Test credentials\n${token}\n`);
      const before = readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8');
      const result = run(cwd, args, apiStub([]));
      assert.equal(result.status, 1);
      assert.equal(result.stderr, `${service}_TOKEN.md contains an unexpected character\n`);
      assert.equal(result.stdout, '');
      assert.equal((result.stdout + result.stderr).includes(token), false);
      assert.deepEqual(readCalls(cwd), []);
      assert.equal(readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8'), before);
    });
  }
}

for (const [service, args, preceding] of [
  ['Sonar', ['--apply-sonar'], []], ['Sonar', confirmArgs, []],
  ['Codacy', ['--apply-codacy'], []], ['Codacy', ['--apply-codacy'], [{ status: 200 }]],
]) {
  for (const error of ['throw', 'reject']) {
    test(`${args[0]} sanitizes a request ${error} after ${preceding.length} successful calls`, (t) => {
      const { cwd, ledger } = fixture(t);
      for (const row of ledger) delete row.confirmed;
      Object.assign(ledger[4], { state: 'resolved', reason: 'AcceptedUse' });
      writeJson(cwd, 'ledger.json', ledger);
      const before = readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8');
      const result = run(cwd, args, apiStub([...preceding, { error }]));
      assert.equal(result.status, 1);
      assert.equal(result.stderr, `${service} request failed: TypeError\n`);
      assert.doesNotMatch(result.stdout + result.stderr, /fixture-token|Zml4dHVyZS10b2tlbjo=|Invalid header/);
      assert.equal(readCalls(cwd).filter((call) => call.url).length, preceding.length + (args[0] === '--confirm' ? 3 : 1));
      assert.equal(readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8'), before);
    });
  }
}

test('--apply-codacy resumes at the first unconfirmed row and keeps the earlier confirmation', (t) => {
  const { cwd, ledger } = fixture(t);
  for (const row of ledger.slice(3)) {
    Object.assign(row, { state: 'resolved', reason: 'AcceptedUse' });
    delete row.confirmed;
  }
  writeJson(cwd, 'ledger.json', ledger);
  const first = run(cwd, ['--apply-codacy'], apiStub([{ status: 200 }, { status: 200 }, { status: 429 }]));
  assert.equal(first.status, 1);
  const saved = readLedger(cwd);
  assert.match(saved[3].confirmed, /^HTTP 200 /);
  assert.equal(saved[4].confirmed, undefined);
  const events = readCalls(cwd).length;
  const rerun = run(cwd, ['--apply-codacy'], apiStub([{ status: 200 }, { status: 204 }]));
  assert.equal(rerun.status, 0, rerun.stderr);
  const patches = readCalls(cwd).slice(events).filter((call) => call.method === 'PATCH');
  assert.deepEqual(patches.map((call) => call.url), [`${codacyIssues}/${ledger[4].id}`]);
  const resumed = readLedger(cwd);
  assert.equal(resumed[3].confirmed, saved[3].confirmed);
  assert.match(resumed[4].confirmed, /^HTTP 204 /);
});

for (const error of ['throw', 'reject']) {
  test(`--confirm reports a Codacy issue search ${error} without transport text`, (t) => {
    const { cwd, ledger } = fixture(t);
    delete ledger[4].confirmed;
    writeJson(cwd, 'ledger.json', ledger);
    const before = readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8');
    const result = run(cwd, confirmArgs, apiStub([{ error }]));
    assert.equal(result.status, 1);
    assert.equal(result.stderr, 'Codacy request failed: TypeError\n');
    assert.equal(readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8'), before);
  });
}

test('--apply-codacy ignores confirmed rows entirely: no validation, no credentials, no request', (t) => {
  const { cwd, ledger } = fixture(t);
  Object.assign(ledger[4], { state: 'resolved', reason: 'Invalid' });
  writeJson(cwd, 'ledger.json', ledger);
  const result = run(cwd, ['--apply-codacy'], dryRunStub);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, '');
});

test('--apply-sonar processes an unconfirmed row next to a confirmed row with an invalid transition', (t) => {
  const { cwd, ledger } = fixture(t);
  ledger[2].transition = 'resolve';
  Object.assign(ledger[0], { state: 'resolved', where: 'Bounded input.', transition: 'wontfix' });
  delete ledger[0].confirmed;
  writeJson(cwd, 'ledger.json', ledger);
  const result = run(cwd, ['--apply-sonar', '--dry-run'], dryRunStub);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /issue=s-worker&transition=wontfix/);
  assert.doesNotMatch(result.stdout, /s-regexp/);
});

for (const [service, args, mutate, message] of [
  ['sonar', ['--apply-sonar'], (ledger) => { ledger[2].where = 'Two\nlines'; }, 'sonar s-regexp: resolved without a valid reason'],
  ['sonar', ['--apply-sonar'], (ledger) => { delete ledger[2].verdict; }, 'sonar s-regexp: resolved without a verdict'],
  ['codacy', ['--apply-codacy'], (ledger) => {
    Object.assign(ledger[3], { state: 'resolved', reason: 'TestCode' }); delete ledger[3].verdict;
  }, 'codacy c-harness: resolved without a verdict'],
]) {
  test(`${args[0]} refuses a ${service} row before any credential read or request: ${message}`, (t) => {
    const { cwd, ledger } = fixture(t);
    for (const row of ledger) delete row.confirmed;
    mutate(ledger);
    writeJson(cwd, 'ledger.json', ledger);
    const before = readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8');
    const result = run(cwd, args, dryRunStub);
    assert.equal(result.status, 1);
    assert.equal(result.stderr, `${message}\n`);
    assert.equal(readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8'), before);
  });
}

test('--apply-sonar rejects a transition outside wontfix and falsepositive', (t) => {
  const { cwd, ledger } = fixture(t);
  delete ledger[2].confirmed;
  ledger[2].transition = 'resolve';
  writeJson(cwd, 'ledger.json', ledger);
  const result = run(cwd, ['--apply-sonar'], dryRunStub);
  assert.equal(result.status, 1);
  assert.equal(result.stderr, 'Sonar s-regexp: invalid transition\n');
});

for (const [name, responses, message] of [
  ['Codacy array pagination', [
    { body: { issues: [], total: 0 } }, { body: { data: [], pagination: [] } },
  ], 'Codacy issues search returned an invalid page'],
  ['conflicting Sonar totals', [
    { body: { issues: [], paging: { total: 0 }, total: 1 } },
  ], 'Sonar issues search returned inconsistent totals'],
]) {
  test(`--confirm rejects ${name} before any confirmation or ledger write`, (t) => {
    const { cwd, ledger } = fixture(t);
    for (const row of ledger) delete row.confirmed;
    writeJson(cwd, 'ledger.json', ledger);
    const path = join(cwd, evidence, 'ledger.json');
    const before = readFileSync(path, 'utf8');
    const result = run(cwd, confirmArgs, apiStub(responses));
    assert.equal(result.status, 1);
    assert.equal(result.stderr, `${message}\n`);
    assert.equal(result.stdout, '');
    assert.equal(readCalls(cwd).length, responses.length + 2);
    assert.equal(readFileSync(path, 'utf8'), before);
  });
}
