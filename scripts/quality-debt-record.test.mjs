// Exercise the quality-debt record CLI's local modes (check, allocate, generate) against isolated inventories and ledgers.
import assert from 'node:assert/strict';
import { existsSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';

import {
  codacyHarnessId, codacyViewerId, confirmArgs, evidence, fixture, run, writeJson,
} from './quality-debt-record.test-support.mjs';

test('--check accepts a complete ledger', (t) => {
  const { cwd } = fixture(t);
  const result = run(cwd, ['--check']);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.trim(), '5 ids: 0 missing, 0 duplicate, 0 open, 0 unconfirmed, 0 resolved-without-reason, 0 unknown, 0 without-where, 0 without-verdict');
  assert.equal(result.stderr, '');
});

for (const [name, change, counts, problem] of [
  ['missing row', (ledger) => ledger.pop(), '1 missing, 0 duplicate, 1 open, 0 unconfirmed, 0 resolved-without-reason',
    `missing 1: codacy:${codacyViewerId}`],
  ['duplicate row', (ledger) => ledger.push(ledger[0]), '0 missing, 1 duplicate, 0 open, 0 unconfirmed, 0 resolved-without-reason', 'duplicate 1: sonar:s-worker'],
  ['empty resolved reason', (ledger) => { ledger[2].where = ' \t'; }, '0 missing, 0 duplicate, 0 open, 0 unconfirmed, 1 resolved-without-reason', 'resolved-without-reason 1: sonar:s-regexp'],
  ['explicit open state', (ledger) => { ledger[0].state = 'open'; }, '0 missing, 0 duplicate, 1 open, 0 unconfirmed, 0 resolved-without-reason', 'open 1: sonar:s-worker'],
  ['missing state', (ledger) => { delete ledger[0].state; }, '0 missing, 0 duplicate, 1 open, 0 unconfirmed, 0 resolved-without-reason', 'open 1: sonar:s-worker'],
  ['unknown state', (ledger) => { ledger[0].state = 'unknown'; }, '0 missing, 0 duplicate, 1 open, 0 unconfirmed, 0 resolved-without-reason', 'open 1: sonar:s-worker'],
]) {
  test(`--check rejects a ${name}`, (t) => {
    const { cwd, ledger } = fixture(t);
    change(ledger);
    writeJson(cwd, 'ledger.json', ledger);
    const result = run(cwd, ['--check']);
    assert.equal(result.status, 1);
    assert.ok(result.stderr.includes(problem), result.stderr);
    const withoutWhere = name === 'empty resolved reason' ? 1 : 0;
    assert.equal(result.stdout.trim(), `5 ids: ${counts}, 0 unknown, ${withoutWhere} without-where, 0 without-verdict`);
    if (name.endsWith('state')) {
      assert.equal(run(cwd).status, 0);
      const text = readFileSync(join(cwd, `${evidence}.md`), 'utf8');
      assert.match(text, /\| s-worker \| typescript:S3776 \| src\/worker\/observe.ts:20 \| open \|/);
    }
  });
}

for (const state of ['fixed', 'resolved', 'excluded']) {
  test(`--check requires where for confirmed ${state} rows, even with --planned`, (t) => {
    const { cwd, ledger } = fixture(t);
    ledger[0].state = state;
    for (const where of [undefined, '', ' \t', null, 123]) {
      ledger[0].where = where;
      writeJson(cwd, 'ledger.json', ledger);
      for (const args of [['--check'], ['--check', '--planned']]) {
        const result = run(cwd, args);
        assert.equal(result.status, 1);
        assert.match(result.stderr, /without-where 1: sonar:s-worker/);
        assert.match(result.stdout, /0 open, 0 unconfirmed/);
        assert.match(result.stdout, /1 without-where/);
      }
    }
  });

  test(`--check requires confirmation of ${state}, but --planned only reports it`, (t) => {
    const { cwd, ledger } = fixture(t);
    ledger[0].state = state;
    for (const confirmed of [undefined, '', ' \t', 200]) {
      ledger[0].confirmed = confirmed;
      writeJson(cwd, 'ledger.json', ledger);
      const result = run(cwd, ['--check']);
      assert.equal(result.status, 1);
      assert.match(result.stdout, /1 open, 1 unconfirmed/);
      const planned = run(cwd, ['--check', '--planned']);
      assert.equal(planned.status, 0, planned.stderr);
      assert.match(planned.stdout, /0 open, 1 unconfirmed/);
    }
  });
}

for (const [service, id, rule, file, state, required] of [
  ['codacy', codacyViewerId, 'Semgrep_fs', 'src/viewer/app/main.tsx', 'fixed', true],
  ['codacy', codacyHarnessId, 'Semgrep_key', '.github/workflows/ci.yml', 'fixed', true],
  ['codacy', codacyHarnessId, 'Semgrep_regex', 'scripts/dco-check.test.mjs', 'resolved', true],
  ['codacy', codacyHarnessId, 'shellcheck_SC2024', 'scripts/e2e/dogfood.sh', 'fixed', true],
  ['sonar', 's-regexp', 'typescript:S8786', 'src/worker/observe.ts', 'resolved', true],
  ['sonar', 's-regexp', 'javascript:S8786', 'scripts/e2e/probe.mjs', 'fixed', true],
  ['codacy', codacyHarnessId, 'Semgrep_fs', 'scripts/e2e/probe.mjs', 'excluded', false],
  ['sonar', 's-worker', 'typescript:S3776', 'src/worker/observe.ts', 'fixed', false],
  ['codacy', codacyViewerId, 'Lizard_nloc-medium', 'src/viewer/app/main.tsx', 'fixed', false],
]) {
  test(`--check requires a verdict only for fixed or resolved rows of the security population: ${rule} ${file} ${state}`, (t) => {
    const { cwd, ledger, sonar, codacy } = fixture(t);
    const inventory = service === 'sonar' ? sonar : codacy;
    const finding = inventory.find((row) => row.id === id);
    finding[service === 'sonar' ? 'rule' : 'pattern'] = rule;
    finding.file = file;
    writeJson(cwd, `${service}-main-issues.json`, inventory);
    const row = ledger.find((entry) => entry.id === id);
    Object.assign(row, { state, rule: required ? 'Not_security' : 'Semgrep_fs' });
    if (state === 'resolved') Object.assign(row, { where: 'Bounded constant.', reason: 'FalsePositive', transition: 'falsepositive' });
    for (const verdict of [undefined, '', ' \t', 123, 'Bounded constant — not applicable']) {
      row.verdict = verdict;
      writeJson(cwd, 'ledger.json', ledger);
      const count = required && (typeof verdict !== 'string' || !verdict.trim()) ? 1 : 0;
      for (const args of [['--check'], ['--check', '--planned']]) {
        const result = run(cwd, args);
        assert.equal(result.status, count, result.stderr);
        assert.ok(result.stdout.includes(`${count} without-verdict`), result.stdout);
        if (count) assert.ok(result.stderr.includes(`without-verdict 1: ${service}:${id}`), result.stderr);
      }
    }
    row.state = 'open';
    delete row.verdict;
    writeJson(cwd, 'ledger.json', ledger);
    for (const args of [['--check'], ['--check', '--planned']]) {
      const result = run(cwd, args);
      assert.equal(result.status, 1);
      assert.match(result.stdout, /0 without-verdict/);
    }
  });
}

test('--check requires a valid Codacy reason even with --planned', (t) => {
  const { cwd, ledger } = fixture(t);
  ledger[4].state = 'resolved';
  for (const reason of [undefined, '', 'accepteduse', 'Invalid', 'AcceptedUse', 'FalsePositive', 'NotExploitable', 'TestCode', 'ExternalCode']) {
    ledger[4].reason = reason;
    writeJson(cwd, 'ledger.json', ledger);
    const invalid = !reason || ['accepteduse', 'Invalid'].includes(reason);
    for (const args of [['--check'], ['--check', '--planned']]) {
      const result = run(cwd, args);
      assert.equal(result.status, invalid ? 1 : 0, result.stderr);
      assert.ok(result.stdout.includes(`${invalid ? 1 : 0} resolved-without-reason`), result.stdout);
    }
    if (invalid) {
      // The apply path only looks at unconfirmed rows; the --check loop above needs the confirmed row back.
      const { confirmed } = ledger[4];
      delete ledger[4].confirmed;
      writeJson(cwd, 'ledger.json', ledger);
      ledger[4].confirmed = confirmed;
      const result = run(cwd, ['--apply-codacy', '--dry-run']);
      assert.equal(result.status, 1);
      assert.equal(result.stderr, `codacy ${codacyViewerId}: resolved without a valid reason\n`);
      assert.equal(result.stdout, '');
    }
  }
});

test('default mode distinguishes planned and confirmed end states', (t) => {
  const { cwd, ledger } = fixture(t);
  delete ledger[0].confirmed;
  for (const state of ['fixed', 'resolved', 'excluded']) {
    ledger[0].state = state;
    ledger[1].state = state;
    writeJson(cwd, 'ledger.json', ledger);
    assert.equal(run(cwd).status, 0);
    const text = readFileSync(join(cwd, `${evidence}.md`), 'utf8');
    assert.ok(text.includes(`| ${state} (planned) |`));
    assert.ok(text.includes(`| ${state} ✓ |`));
  }
});

test('--allocate gives exclusions priority and respects worker and viewer ownership', (t) => {
  const { cwd } = fixture(t);
  const result = run(cwd, ['--allocate']);
  assert.equal(result.status, 0, result.stderr);
  const allocation = JSON.parse(readFileSync(join(cwd, evidence, 'allocation.json'), 'utf8'));
  assert.deepEqual(allocation, {
    A: ['s-sql', codacyHarnessId], E: ['s-regexp'], B1: [], B2: [], B3: [],
    C1: ['s-worker'], C2: [], C3: [codacyViewerId], C4: [], D: [],
    counts: { A: 2, E: 1, B1: 0, B2: 0, B3: 0, C1: 1, C2: 0, C3: 1, C4: 0, D: 0 },
  });
  assert.deepEqual(JSON.parse(result.stdout), allocation.counts);
  assert.equal(run(cwd, ['--check']).status, 0);
});

// Every allocation rule of the record script, one row per (rule, file, expected batch); an
// expected batch of undefined means the finding stays unallocated for a person to look at.
const sonarCases = [
  ['plsql:S8786', 'scripts/query.sql', 'A'],
  ['javascript:S8786', 'scripts/check.mjs', 'E'],
  ['javascript:S3776', 'scripts/check.mjs', 'C4'],
  ['javascript:S107', 'scripts/check.mjs', 'C4'],
  ['typescript:S3776', 'src/observer/run.ts', 'C1'],
  ['typescript:S107', 'src/capture.ts', 'C2'],
  ['typescript:S3776', 'src/capture.ts', 'C2'],
  ['javascript:S3358', 'scripts/check.mjs', 'B1'],
  ['typescript:S3358', 'src/capture.ts', 'B2'],
  ['typescript:S9999', 'src/capture.ts', undefined],
  ['javascript:S9999', 'scripts/check.mjs', undefined],
  ...['src/injection/pack.ts', 'src/mcp.ts', 'src/viewer/server.ts', 'src/transfer.ts',
    'src/db/queries.ts', 'src/privacy/detect.ts', 'src/fixture/replay.ts'].flatMap((file) => [
    ['typescript:S3776', file, 'C3'], ['typescript:S3358', file, 'B3'],
  ]),
];
const codacyCases = [
  ...['legacy/old.ts', 'package-lock.json', 'build/app.js', 'dist/app.js', 'coverage/report.js']
    .map((file) => ['Lizard_file-nloc-medium', file, 'A']),
  ['TSQLLint_schema', 'src/db/migrations/001.sql', 'A'],
  ['SQLint_allIssues', 'src/db/migrations/001.sql', 'A'],
  ['Lizard_nloc-medium', 'test/unit/check.ts', 'A'],
  ['Lizard_file-nloc-medium', 'test/unit/check.ts', 'A'],
  ['Semgrep_ssrf', 'test/unit/check.ts', 'A'],
  ['markdownlint_MD024', 'docs/evidence.md', 'A'],
  ['Stylelint_scss_function-disallowed-list', 'src/viewer/app/app.css', 'A'],
  ['Stylelint_color-no-invalid-hex', 'src/viewer/app/app.css', undefined],
  ['Semgrep_ssrf', 'src/mcp.ts', 'E'],
  ['Semgrep_key', '.github/workflows/ci.yml', 'E'],
  ['shellcheck_SC2024', 'scripts/e2e/dogfood.sh', 'E'],
  ['Lizard_nloc-medium', 'scripts/e2e/check.test.mjs', 'C4'],
  ['Lizard_parameter-count-medium', 'scripts/check.mjs', 'C4'],
  ['Lizard_nloc-medium', 'src/worker/observe.ts', 'C1'],
  ['Lizard_parameter-count-medium', 'src/observer/run.ts', 'C1'],
  ['Lizard_nloc-medium', 'src/capture.ts', 'C2'],
  ['Lizard_parameter-count-medium', 'src/capture.ts', 'C2'],
  ['Lizard_nloc-medium', 'src/fixture/replay.ts', 'C3'],
  ['Lizard_parameter-count-medium', 'src/mcp.ts', 'C3'],
  ['Lizard_file-nloc-medium', 'src/viewer/app/main.tsx', 'D'],
  ['Lizard_file-nloc-medium', 'scripts/check.mjs', 'D'],
  ['TSQLLint_schema', 'src/query.sql', undefined],
  ['markdownlint_MD024', 'README.md', undefined],
  ['Unknown_rule', 'src/capture.ts', undefined],
];

test('--allocate follows all batch scope rules and leaves unknown patterns unallocated', (t) => {
  const { cwd } = fixture(t);
  writeJson(cwd, 'sonar-main-issues.json', sonarCases.map(([rule, file], i) => ({ id: `s-${i}`, rule, file, line: i })));
  writeJson(cwd, 'codacy-main-issues.json', codacyCases.map(([pattern, file], i) => ({ id: `c-${i}`, pattern, file, line: i })));
  assert.equal(run(cwd, ['--allocate']).status, 0);
  const { counts, ...allocation } = JSON.parse(readFileSync(join(cwd, evidence, 'allocation.json'), 'utf8'));
  for (const [prefix, cases] of [['s', sonarCases], ['c', codacyCases]]) {
    cases.forEach(([, , expected], i) => {
      const batches = Object.keys(allocation).filter((batch) => allocation[batch].includes(`${prefix}-${i}`));
      assert.deepEqual(batches, expected ? [expected] : [], `${prefix}-${i}`);
    });
  }
  assert.equal(Object.values(counts).reduce((sum, count) => sum + count, 0), sonarCases.length + codacyCases.length - 6);
});

test('--check rejects missing or repeated allocations, even with a complete ledger', (t) => {
  const { cwd, allocation } = fixture(t);
  allocation.C3 = ['stale-id'];
  allocation.B1.push('s-worker');
  writeJson(cwd, 'allocation.json', allocation);
  const result = run(cwd, ['--check']);
  assert.equal(result.status, 1);
  assert.ok(result.stderr.includes(`allocation missing: codacy:${codacyViewerId}`), result.stderr);
  assert.match(result.stderr, /allocation duplicate: sonar:s-worker.*C1/);
  assert.match(result.stderr, /allocation unknown: stale-id/);
  assert.equal(result.stdout.trim(), '5 ids: 0 missing, 0 duplicate, 0 open, 0 unconfirmed, 0 resolved-without-reason, 0 unknown, 0 without-where, 0 without-verdict');
});

for (const args of [['--check'], ['--check', '--planned']]) {
  test(`${args.join(' ')} rejects a missing allocation file`, (t) => {
    const { cwd } = fixture(t);
    rmSync(join(cwd, evidence, 'allocation.json'));
    const result = run(cwd, args);
    assert.equal(result.status, 1);
    assert.equal(result.stderr, 'allocation.json missing\n');
  });

  test(`${args.join(' ')} reports ledger ids outside the service's inventory`, (t) => {
    const { cwd, ledger } = fixture(t);
    ledger.push({ ...ledger[0], id: codacyViewerId });
    writeJson(cwd, 'ledger.json', ledger);
    const before = readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8');
    const result = run(cwd, args);
    assert.equal(result.status, 1);
    assert.equal(result.stderr, `unknown 1: sonar:${codacyViewerId}\n`);
    assert.match(result.stdout, /0 resolved-without-reason, 1 unknown/);
    assert.equal(readFileSync(join(cwd, evidence, 'ledger.json'), 'utf8'), before);
  });
}

test('default mode keeps the manual prefix and writes sorted findings, verdicts, and history', (t) => {
  const { cwd, ledger } = fixture(t);
  const prefix = '# Frozen findings\r\n\r\nKeep this exact text.\r\n<!-- generated below -->\r\n';
  const markdown = join(cwd, `${evidence}.md`);
  writeFileSync(markdown, `${prefix}\nOld generated content\n`);
  ledger.pop();
  ledger[2].verdict = 'constant | pattern — not applicable';
  ledger[2].history = [{ from: 'open', to: 'resolved', when: '2026-09-07', why: 'Reviewed | literal\npattern' }];
  writeJson(cwd, 'ledger.json', ledger);
  assert.equal(run(cwd).status, 0);
  const text = readFileSync(markdown, 'utf8');
  assert.ok(text.startsWith(prefix));
  assert.doesNotMatch(text, /Old generated content/);
  assert.equal((text.match(/^\| (?:s-|[0-9a-f]+ \|)/gm) ?? []).length, 5);
  assert.ok(text.indexOf('## SonarCloud') < text.indexOf('## Codacy'));
  assert.ok(text.indexOf('| s-sql |') < text.indexOf('| s-regexp |'));
  assert.ok(text.indexOf('| s-regexp |') < text.indexOf('| s-worker |'));
  assert.match(text, /open 0 \/ fixed 1 \/ resolved 1 \/ excluded 1/);
  assert.match(text, /open 1 \/ fixed 0 \/ resolved 0 \/ excluded 1/);
  assert.ok(text.includes('| s-regexp | typescript:S8786 | src/worker/observe.ts:3 | resolved ✓ | The pattern is a constant. — constant \\| pattern — not applicable |'));
  assert.ok(text.includes(`| ${codacyViewerId} | Lizard_nloc-medium | src/viewer/app/main.tsx:12 | open |  |`));
  assert.ok(text.includes('## History\n\n| service | id | from | to | when | why |'));
  assert.ok(text.includes('| sonar | s-regexp | open | resolved | 2026-09-07 | Reviewed \\| literal<br>pattern |'));
  assert.equal(run(cwd).status, 0);
  assert.equal(readFileSync(markdown, 'utf8'), text);
});

test('default mode creates a header and an empty History table when the file is missing', (t) => {
  const { cwd } = fixture(t);
  assert.equal(run(cwd).status, 0);
  const text = readFileSync(join(cwd, `${evidence}.md`), 'utf8');
  assert.match(text, /^# .+\n\n<!-- generated below -->\n/);
  assert.match(text, /## History\n\n\| service \| id \| from \| to \| when \| why \|\n\| --- \| --- \| --- \| --- \| --- \| --- \|\n$/);
});


test('invalid mode combinations fail without generating a record', (t) => {
  const { cwd } = fixture(t);
  const allocation = readFileSync(join(cwd, evidence, 'allocation.json'), 'utf8');
  for (const args of [['--dry-run'], ['--check', '--dry-run'], ['--allocate', '--check'],
    ['--planned'], ['--apply-codacy', '--planned'], ['--confirm'], ['--confirm', '--sonar-analysis', 'id'],
    [...confirmArgs, '--dry-run'], ['--sonar-analysis', 'id'], [...confirmArgs.slice(0, -1), '']]) {
    const result = run(cwd, args);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Usage:/);
    assert.equal(result.stdout, '');
    assert.equal(existsSync(join(cwd, `${evidence}.md`)), false);
    assert.equal(readFileSync(join(cwd, evidence, 'allocation.json'), 'utf8'), allocation);
  }
});

test('usage explains the caller verification required before recording confirmation labels', (t) => {
  const { cwd } = fixture(t);
  const result = run(cwd, ['--confirm']);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /--confirm records the analysis key and commit SHA as labels/);
  assert.match(result.stderr, /caller must verify beforehand that both services finished analysing that revision/);
  assert.match(result.stderr, /quickstart\.md.*Final analysis confirmation/);
  assert.match(result.stderr, /<sha> is Codacy's last analysed commit with a finished analysis/);
});

test('the quality-debt CLI participates in Sonar and Engine coverage', () => {
  const sonar = readFileSync(new URL('../sonar-project.properties', import.meta.url), 'utf8');
  const exclusions = sonar.match(/^sonar\.coverage\.exclusions=(.*)$/m)[1].split(',');
  for (const module of ['record', 'ledger', 'services']) assert.equal(exclusions.includes(`scripts/quality-debt-${module}.mjs`), false);
  const workflow = readFileSync(new URL('../.github/workflows/ci.yml', import.meta.url), 'utf8');
  const coverage = workflow.split('- name: Engine coverage\n')[1].split('- name:')[0];
  assert.match(coverage, /NODE_V8_COVERAGE="\$PWD\/coverage\/v8" node --test/);
  assert.match(coverage, /'scripts\/quality-debt-record\*\.test\.mjs'/);
  assert.match(coverage, /--exclude 'scripts\/\*\*\/\*\.test\.mjs'/);
});
