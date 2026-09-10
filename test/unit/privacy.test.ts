import { grantVisibility } from '../../src/db/queries.js';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { appendFileSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';
import { Worker } from 'node:worker_threads';
import { setImmediate as nextTurn } from 'node:timers/promises';

import type { SecretLintCoreConfig } from '@secretlint/types';
import { secretLintProfiler } from '@secretlint/profiler';

import {
  MAX_REPO_SECRET_PATHS,
  MAX_REPO_SECRET_PATH_LENGTH,
  RepoConfigError,
  loadRepoRules,
} from '../../src/config.js';
import { openDatabase } from '../../src/db/open.js';
import { resolveRepoIdentity } from '../../src/repo-identity.js';
import { chooseWork } from '../../src/work.js';
import { PACK_FOOTER, PACK_HEADER, packHash, stripRecognizedPacks } from '../../src/injection/recognize.js';
import { credentialValues } from '../../src/log.js';
import { DIRECTIVE_PHRASES, sessionSummary } from '../../src/observer/classify.js';
import { filterReadOutput } from '../../src/privacy/provenance.js';
import { claimLease } from '../../src/worker/lease.js';
import { promoteSensitivity, reclassifyImportedRow, strictest } from '../../src/privacy/classify.js';
import {
  detectInWorker,
  detectSync,
  compileGlob,
  globRuleError,
  matchSecretPath,
  redactSecrets,
  stripPrivate,
} from '../../src/privacy/detect.js';
import type { DetectorInput, DetectorResult } from '../../src/privacy/detect.js';
import { filterEgress, isAllowed, loadDestinationRules } from '../../src/privacy/egress.js';
import type { Destination, Sensitivity } from '../../src/privacy/egress.js';
import { WALL_CLOCK_IS_MEASURED, withTempHome } from '../helpers/home.js';
import {
  NOW,
  DAY,
  captureEndedSession,
  catalogResponse,
  cleanEnv,
  eventBase,
  eventId,
  providerOutput,
  runObserveForFixture,
  toggleDatabase,
  withFixture,
  workersResponse,
  writeConfig,
  type Fixture,
} from '../helpers/observe.js';

type CorpusLine = { id: string; kind: string; text: string; secret: string | null };

test('repeated secret checks retain no library profiling records while still applying the rules', async () => {
  const sample = corpus.find((row) => row.secret !== null);
  assert.ok(sample?.secret);
  for (let i = 0; i < 10; i++) {
    const result = await detectSync({ text: sample.text,
      fields: [`safe fixture ${i}`], paths: [], repoRoot: null, secretPaths: [], credentialValues: [] });
    assert.equal(result.ok, true);
    if (result.ok) {
      assert.equal(result.sensitivity, 'secret');
      assert.equal(result.text.includes(sample.secret), false);
    }
  }
  await nextTurn();
  assert.equal((await secretLintProfiler.getEntries()).length, 0);
  assert.equal((await secretLintProfiler.getMeasures()).length, 0);
});

// The corpus is the SC-005 fixture: it is data, so the test reads it instead of restating it.
const corpus: CorpusLine[] = readFileSync(resolve(process.cwd(), 'test/corpus/secrets.jsonl'), 'utf8')
  .split('\n')
  .filter((line) => line.trim() !== '')
  .map((line) => JSON.parse(line) as CorpusLine);

const SENSITIVITIES: Sensitivity[] = ['eligible', 'local_only', 'private', 'secret'];
const DESTINATIONS: Destination[] = ['remote_observer', 'local_observer', 'injection', 'sync'];

// data-model.md "destination_rules (seeded)", written out here so the test fails when the seed changes.
const EXPECTED_EGRESS: { destination: Destination; sameRepo: Sensitivity[]; otherRepo: Sensitivity[] }[] = [
  { destination: 'remote_observer', sameRepo: ['eligible'], otherRepo: ['eligible'] },
  { destination: 'local_observer', sameRepo: ['eligible', 'local_only', 'private'], otherRepo: [] },
  { destination: 'injection', sameRepo: ['eligible', 'local_only', 'private'], otherRepo: [] },
  {
    destination: 'sync',
    sameRepo: ['eligible', 'local_only', 'private'],
    otherRepo: ['eligible', 'local_only', 'private'],
  },
];

const CLEAN_TEXT = [
  'export function loadUser(id: string) {',
  '  return fetch(`https://api.example.com/v1/users/${id}`).then((response) => response.json());',
  '}',
  'The regression appeared in commit 9c1f4b7e2a8d3f60b5c7e9a1d2f4b6c8e0a3d5f7 and the run identifier',
  'was 3f2504e0-4f89-11d3-9a0c-0305e82c3301 on the second attempt.',
  '{"status":"ok","items":[1,2,3],"note":"nothing confidential in this payload"}',
].join('\n');

function detectorInput(text: string): DetectorInput {
  // No configured credentials: the detector must not read the developer's shell in these tests.
  return { text, paths: [], repoRoot: null, secretPaths: [], credentialValues: [] };
}

/** Secret-shaped literals live in the corpus, never in this file, so the secret scanners stay useful. */
function corpusLine(id: string): CorpusLine {
  const line = corpus.find((entry) => entry.id === id);
  if (line === undefined) assert.fail(`the corpus is missing the line ${id}`);
  return line;
}

/**
 * A run of eight secret characters is already enough to be worth stealing, so a partial redaction
 * counts as a leak: checking only the whole secret would pass while most of it survived.
 */
function assertNoSecretRun(text: string, secret: string, message: string): void {
  for (let start = 0; start + 8 <= secret.length; start += 1) {
    const run = secret.slice(start, start + 8);
    assert.ok(!text.includes(run), `${message} survived redaction at offset ${start}: ${text}`);
  }
}

function assertDetected(result: DetectorResult, message: string): Extract<DetectorResult, { ok: true }> {
  if (!result.ok) assert.fail(`${message}: the detector failed with reason ${result.reason}`);
  return result;
}

test('fail-closed: every secret of the corpus is redacted and classified secret', async () => {
  assert.ok(corpus.length >= 30, 'the corpus is the SC-005 fixture and stays at 30 lines or more');
  for (const line of corpus) {
    if (line.secret === null) continue;
    const detected = assertDetected(await detectSync(detectorInput(line.text)), line.id);
    assert.equal(detected.sensitivity, 'secret', `${line.id} (${line.kind}) must be classified secret`);
    assert.ok(detected.redactions.length > 0, `${line.id} (${line.kind}) must record a redaction`);
    assert.ok(
      !detected.text.includes(line.secret),
      `${line.id} (${line.kind}) survived redaction: ${detected.text}`,
    );
    assertNoSecretRun(detected.text, line.secret, `${line.id} (${line.kind})`);
  }
});

test('fail-open: the corpus negatives stay local_only and byte-identical', async () => {
  const negatives = corpus.filter((line) => line.secret === null);
  assert.ok(negatives.length >= 5, 'the corpus keeps at least five negative lines');
  for (const line of negatives) {
    const detected = assertDetected(await detectSync(detectorInput(line.text)), line.id);
    assert.equal(detected.sensitivity, 'local_only', `${line.id} (${line.kind}) must stay local_only`);
    assert.equal(detected.text, line.text, `${line.id} (${line.kind}) must not be rewritten`);
    assert.deepEqual(detected.redactions, []);
  }
});

test('fail-open: clean content comes back byte-identical with no redaction', async () => {
  const detected = assertDetected(await detectSync(detectorInput(CLEAN_TEXT)), 'clean text');
  assert.equal(detected.text, CLEAN_TEXT);
  assert.equal(detected.sensitivity, 'local_only');
  assert.deepEqual(detected.redactions, []);
  assert.equal(detected.privateRemoved, 0);
  assert.equal(detected.pathRule, null);
});

test('fail-closed: stripPrivate removes every private span, including an unclosed tag', () => {
  assert.deepEqual(stripPrivate('<private>a</private>b'), { text: 'b', removed: 1 });
  assert.deepEqual(stripPrivate('before <PRIVATE >x</ Private >after'), {
    text: 'before after',
    removed: 1,
  });
  // FR-019: an unclosed tag removes everything after it, so nothing unreviewed is kept.
  assert.deepEqual(stripPrivate('keep this <private>drop the rest'), {
    text: 'keep this ',
    removed: 1,
  });
  // Nested tags are one span from the first open tag to its matching close tag.
  assert.deepEqual(stripPrivate('<private>a<private>b</private>c</private>d'), { text: 'd', removed: 1 });
  assert.deepEqual(stripPrivate('one <private>x</private> two <private>y</private> three'), {
    text: 'one  two  three',
    removed: 2,
  });
  assert.deepEqual(stripPrivate('no tag here'), { text: 'no tag here', removed: 0 });
});

test('stripPrivate reads an unclosed tag prefix once, not once per space', () => {
  // Captured text is agent output up to 1 MiB. The tag itself was always fast; the shape that
  // backtracked (1.4 s on 50,000 spaces) is the second one below: spaces after `<` and no tag.
  const text = `<${' '.repeat(1024 * 1024)}private>`;
  const started = performance.now();
  const result = stripPrivate(text);
  const elapsed = performance.now() - started;
  assert.deepEqual(result, { text: '', removed: 1 });
  if (WALL_CLOCK_IS_MEASURED) assert.ok(elapsed < 200, `took ${elapsed.toFixed(0)} ms`);
  // The non-matching shape is the one that backtracked: a run of spaces after `<` and then not a tag.
  const noTag = `<${' '.repeat(1024 * 1024)}x`;
  const startedNoTag = performance.now();
  assert.deepEqual(stripPrivate(noTag), { text: noTag, removed: 0 });
  const elapsedNoTag = performance.now() - startedNoTag;
  if (WALL_CLOCK_IS_MEASURED) assert.ok(elapsedNoTag < 200, `took ${elapsedNoTag.toFixed(0)} ms`);
  assert.deepEqual(stripPrivate(`< ${'x'.repeat(1024 * 1024)}`), {
    text: `< ${'x'.repeat(1024 * 1024)}`,
    removed: 0,
  });
});

test('fail-closed: a private span never reaches the detector result', async () => {
  const wrapped = corpusLine('github-classic-pat').secret;
  const detected = assertDetected(
    await detectSync(detectorInput(`public note <private>${wrapped}</private> end`)),
    'private span',
  );
  assert.equal(detected.text, 'public note  end');
  assert.equal(detected.privateRemoved, 1);
  assert.equal(detected.sensitivity, 'local_only');
});

test('compileGlob follows gitignore semantics and keeps a single star inside one path segment', () => {
  // A rule without a slash matches the file name at any depth, which is how the same rule is
  // written in .gitignore and how an agent sends the path (FR-039, R4).
  assert.equal(compileGlob('*.pem').test('server.pem'), true);
  assert.equal(compileGlob('*.pem').test('config/server.pem'), true);
  assert.equal(compileGlob('*.pem').test('a/x.pem'), true);
  assert.equal(compileGlob('*.pem').test('x.pem.bak'), false);
  assert.equal(compileGlob('**/.env').test('.env'), true);
  assert.equal(compileGlob('**/.env').test('app/.env'), true);
  assert.equal(compileGlob('**/*.pem').test('x.pem'), true);
  assert.equal(compileGlob('deploy/**/key.pem').test('deploy/key.pem'), true);
  assert.equal(compileGlob('deploy/**/key.pem').test('deploy/eu/west/key.pem'), true);
  assert.equal(compileGlob('secrets/**').test('secrets/k.txt'), true);
  assert.equal(compileGlob('secrets/**').test('secrets/aws/key.json'), true);
  assert.equal(compileGlob('secrets/**').test('other/secrets/key.json'), false);
  assert.equal(compileGlob('.env*').test('.env.local'), true);
  assert.equal(compileGlob('.env*').test('app.env'), false);
  assert.equal(compileGlob('key?.pem').test('key1.pem'), true);
  assert.equal(compileGlob('key?.pem').test('key10.pem'), false);
  assert.equal(compileGlob('key[0-9].pem').test('key7.pem'), true);
  assert.equal(compileGlob('key[0-9].pem').test('keyx.pem'), false);
  // Regular expression metacharacters in a glob are literal text.
  assert.equal(compileGlob('note(1)+.txt').test('note(1)+.txt'), true);
  assert.equal(compileGlob('note(1)+.txt').test('note(1).txt'), false);
});

test('fail-closed: a path rule matches the repository-relative and the raw form', () => {
  const rules = ['secrets/**', '*.pem', '.env*'];
  const root = '/home/dev/repo';
  assert.equal(matchSecretPath('/home/dev/repo/secrets/aws.json', rules, root), 'secrets/**');
  assert.equal(matchSecretPath('/home/dev/repo/server.pem', rules, root), '*.pem');
  assert.equal(matchSecretPath('/home/dev/repo/.env.local', rules, root), '.env*');
  assert.equal(matchSecretPath('/home/dev/repo/src/app.ts', rules, root), null);
  // A relative path is what the agents actually send, and it is the form a rule is written in.
  assert.equal(matchSecretPath('.env', ['**/.env'], root), '**/.env');
  assert.equal(matchSecretPath('a/x.pem', ['*.pem'], root), '*.pem');
  assert.equal(matchSecretPath('secrets/k.txt', ['secrets/**'], root), 'secrets/**');
  assert.equal(matchSecretPath('other/secrets/k.txt', ['secrets/**'], root), null);
  assert.equal(matchSecretPath('README.md', ['*.pem', '**/.env'], root), null);
  // Without a repository root only the raw form is tested.
  assert.equal(matchSecretPath('/etc/ssl/server.pem', ['/etc/**'], null), '/etc/**');
  assert.equal(matchSecretPath('/etc/ssl/server.pem', ['*.pem'], null), '*.pem');
  assert.equal(matchSecretPath('server.pem', [], root), null);
});

test('a rule of unclosed brackets is tokenized once, not once per bracket', () => {
  // An operator rule (~/.oboete/config.toml) has no length bound, and each `[` used to search the
  // rest of the rule for a `]` that was never there: 1 MiB of `[` took over 4 s, now under 200 ms,
  // so the bound below holds on a loaded runner and fails on the old code by a wide margin.
  const started = performance.now();
  assert.equal(globRuleError('['.repeat(1024 * 1024)), null);
  const elapsed = performance.now() - started;
  if (WALL_CLOCK_IS_MEASURED) assert.ok(elapsed < 1500, `took ${elapsed.toFixed(0)} ms`);
  assert.equal(compileGlob('[ab]c[').test('bc['), true);
  assert.equal(compileGlob('[ab]c[').test('cc['), false);
  assert.equal(compileGlob('a[[]b').test('a[b'), true);
  assert.equal(compileGlob('[a[b]c').test('[c'), true);
});

test('a full repository rule list keeps one path match bounded', () => {
  // R4: `.oboete.toml` is repository-supplied, so its rules may be written to be expensive. The
  // list is bounded by loadRepoRules and the matcher sweeps the path once per token, so the work
  // stays linear instead of backtracking over every way to split the path.
  // The shape is the declared bound, so raising either constant is measured here rather than
  // silently spending more of the capture deadline (CAPTURE_DEADLINE_MS is 300 ms).
  const stars = MAX_REPO_SECRET_PATH_LENGTH - 1;
  const rules = Array.from({ length: MAX_REPO_SECRET_PATHS }, (_, index) =>
    index % 2 === 0 ? `${'*'.repeat(stars)}x` : `${'a*'.repeat(Math.floor(stars / 2))}x`,
  );
  // A path with no slash is the expensive shape: every `*` can reach every position of it. 1 KiB
  // keeps the sweep (rules x rule length x path length) short enough for coverage-instrumented CI.
  const path = `${'a'.repeat(1_023)}b`;
  const started = process.hrtime.bigint();
  assert.equal(matchSecretPath(path, rules, null), null);
  const elapsedMs = Number(process.hrtime.bigint() - started) / 1e6;
  // What is measured is linear against exponential: the same rules compiled into one backtracking
  // expression do not finish at all. Measured at about 70 ms here and about twenty times slower
  // under coverage instrumentation, so the bound only has to fail a return to backtracking.
  assert.ok(
    elapsedMs < 10_000,
    `matching ${MAX_REPO_SECRET_PATHS} rules against a 1 KiB path took ${elapsedMs.toFixed(0)} ms`,
  );
});

test('fail-closed: a path rule hit keeps metadata only, even for harmless text', async () => {
  const detected = assertDetected(
    await detectSync({
      text: 'the deployment notes mention nothing confidential',
      paths: ['/home/dev/repo/secrets/aws.json'],
      repoRoot: '/home/dev/repo',
      secretPaths: ['secrets/**'],
    }),
    'path rule',
  );
  assert.equal(detected.sensitivity, 'secret');
  assert.equal(detected.pathRule, 'secrets/**');
  // R4 and data-model raw_events.content: a path-rule hit stores no content at all.
  assert.equal(detected.text, '');
});

test('fail-closed: a detector failure returns detector_error and carries no text', async () => {
  const throwingRules = {
    rules: [
      {
        id: 'oboete-detector-failure',
        rule: {
          meta: {
            id: 'oboete-detector-failure',
            recommended: false,
            type: 'scanner',
            supportedContentTypes: ['text'],
          },
          messages: {},
          create: () => ({
            file: () => {
              throw new Error('the detector failed on purpose');
            },
          }),
        },
      },
    ],
  } as unknown as SecretLintCoreConfig;

  const line = corpusLine('github-classic-pat');
  const result = await detectSync(detectorInput(line.text), { rules: throwingRules });
  assert.deepEqual(result, { ok: false, reason: 'detector_error' });
  assert.ok(!JSON.stringify(result).includes(line.secret ?? ''));
});

test('fail-closed: a malformed .oboete.toml yields no rule set, so capture cannot classify', async () => {
  await withTempHome((home) => {
    writeFileSync(join(home, '.oboete.toml'), '[privacy\nsecret_paths = ["secrets/**"]\n');
    assert.throws(() => loadRepoRules(home), RepoConfigError);
  });
  await withTempHome((home) => {
    writeFileSync(join(home, '.oboete.toml'), '[observer]\npreset = "workers-ai"\n');
    assert.throws(() => loadRepoRules(home), RepoConfigError);
  });
});

test('fail-closed: redaction is idempotent', async () => {
  for (const line of corpus) {
    const once = await redactSecrets(line.text);
    const twice = await redactSecrets(once.text);
    assert.equal(twice.text, once.text, `${line.id} changed on the second pass`);
    if (line.secret !== null) assert.deepEqual(twice.hits, [], `${line.id} reported a hit on redacted text`);
  }
});

test('fail-closed: a detector that never answers is cut off at the deadline', async (t) => {
  const directory = mkdtempSync(join(tmpdir(), 'oboete-detector-'));
  try {
    const script = join(directory, 'never-answers.mjs');
    writeFileSync(
      script,
      [
        "import { workerData } from 'node:worker_threads';",
        'setInterval(() => {',
        '  Atomics.add(workerData.input.beat, 0, 1);',
        '}, 5);',
        '',
      ].join('\n'),
    );

    const beat = new Int32Array(new SharedArrayBuffer(4));
    const input = { ...detectorInput(''), beat } as unknown as DetectorInput;
    const started = Date.now();
    const result = await detectInWorker(input, { cutoffMs: 50, workerScript: script });
    const elapsed = Date.now() - started;

    assert.deepEqual(result, { ok: false, reason: 'deadline' });
    if (WALL_CLOCK_IS_MEASURED) {
      assert.ok(elapsed < 200, `the call returned after ${elapsed} ms`);
    }

    // The Worker is terminated, so nothing in it keeps running once the deadline has passed. How
    // many times it ticked before that is machine-dependent and only reported.
    await new Promise((done) => setTimeout(done, 60));
    const afterTermination = Atomics.load(beat, 0);
    await new Promise((done) => setTimeout(done, 120));
    assert.equal(Atomics.load(beat, 0), afterTermination, 'the worker kept running after the cutoff');
    t.diagnostic(`the worker ticked ${afterTermination} times before it was terminated`);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('fail-open: the engine bundle worker returns what detectSync returns', async () => {
  const bundle = resolve(process.cwd(), 'dist/oboete.mjs');
  for (const text of [corpusLine('github-classic-pat').text, CLEAN_TEXT]) {
    const inWorker = await detectInWorker(detectorInput(text), { cutoffMs: 5_000, workerScript: bundle });
    assert.deepEqual(inWorker, await detectSync(detectorInput(text)));
  }
});

test('fail-closed: the engine bundle stays silent in a worker it does not recognize', async () => {
  // FR-021: the hook writes nothing but the pack to stdout, so a worker carrying a role this build
  // does not know must not fall through to the CLI dispatch.
  const worker = new Worker(resolve(process.cwd(), 'dist/oboete.mjs'), {
    workerData: { role: 'a role from a newer build' },
    stdout: true,
  });
  const chunks: Buffer[] = [];
  worker.stdout.on('data', (chunk: Buffer) => chunks.push(chunk));
  const code = await new Promise<number>((resolveExit, rejectExit) => {
    worker.on('exit', resolveExit);
    worker.on('error', rejectExit);
  });
  assert.equal(code, 0);
  assert.equal(Buffer.concat(chunks).toString('utf8'), '');
});

test('egress: the seeded destination_rules table decides every combination', async () => {
  await withTempHome(async (home) => {
    const opened = openDatabase({ path: join(home, 'memory.db'), timeoutMs: 2_000 });
    try {
      const rules = loadDestinationRules(opened.db);
      for (const expected of EXPECTED_EGRESS) {
        for (const sensitivity of SENSITIVITIES) {
          assert.equal(
            isAllowed(rules, expected.destination, sensitivity, true),
            expected.sameRepo.includes(sensitivity),
            `${expected.destination} / ${sensitivity} / same repository`,
          );
          assert.equal(
            isAllowed(rules, expected.destination, sensitivity, false),
            expected.otherRepo.includes(sensitivity),
            `${expected.destination} / ${sensitivity} / other repository`,
          );
        }
      }
      // FR-020: a secret row is refused at every destination.
      for (const destination of DESTINATIONS) {
        assert.equal(isAllowed(rules, destination, 'secret', true), false);
        assert.equal(isAllowed(rules, destination, 'secret', false), false);
      }
    } finally {
      opened.db.close();
    }
  });
});

test('fail-closed: secret stays refused even when the table says it is allowed', async () => {
  await withTempHome(async (home) => {
    const opened = openDatabase({ path: join(home, 'memory.db'), timeoutMs: 2_000 });
    try {
      opened.db.exec("UPDATE destination_rules SET allowed = 1 WHERE sensitivity = 'secret'");
      const tampered = loadDestinationRules(opened.db);
      for (const destination of DESTINATIONS) {
        assert.equal(isAllowed(tampered, destination, 'secret', true), false, destination);
      }
      const { allowed, blocked } = filterEgress(
        tampered,
        'local_observer',
        [{ sensitivity: 'secret', repoId: 'repo-a' }],
        'repo-a',
      );
      assert.deepEqual(allowed, []);
      assert.equal(blocked.length, 1);
      assert.equal(blocked[0]?.reason, 'sensitivity');
    } finally {
      opened.db.close();
    }
  });
});

test('egress: filterEgress reports the blocked rows with their reason', async () => {
  await withTempHome(async (home) => {
    const opened = openDatabase({ path: join(home, 'memory.db'), timeoutMs: 2_000 });
    try {
      const rules = loadDestinationRules(opened.db);
      const rows = [
        { sensitivity: 'eligible' as Sensitivity, repoId: 'repo-a' },
        { sensitivity: 'local_only' as Sensitivity, repoId: 'repo-a' },
        { sensitivity: 'secret' as Sensitivity, repoId: 'repo-a' },
        { sensitivity: 'eligible' as Sensitivity, repoId: 'repo-b' },
      ];

      const injection = filterEgress(rules, 'injection', rows, 'repo-a');
      assert.deepEqual(injection.allowed, [rows[0], rows[1]]);
      assert.deepEqual(injection.blocked, [
        { row: rows[2], reason: 'sensitivity' },
        { row: rows[3], reason: 'repository' },
      ]);

      const remote = filterEgress(rules, 'remote_observer', rows, 'repo-a');
      // FR-020: the remote observer receives eligible rows from any repository and nothing else.
      assert.deepEqual(remote.allowed, [rows[0], rows[3]]);
      assert.deepEqual(remote.blocked, [
        { row: rows[1], reason: 'sensitivity' },
        { row: rows[2], reason: 'sensitivity' },
      ]);
    } finally {
      opened.db.close();
    }
  });
});

test('promotion: the worker promotes only a clean, complete, local-only row', () => {
  const clean: DetectorResult = {
    ok: true,
    text: 'note',
    texts: [],
    redactions: [],
    privateRemoved: 0,
    sensitivity: 'local_only',
    pathRule: null,
  };
  const hit: DetectorResult = {
    ...clean,
    redactions: [{ rule: 'github', count: 1 }],
    sensitivity: 'secret',
  };
  const failed: DetectorResult = { ok: false, reason: 'detector_error' };
  const cutOff: DetectorResult = { ok: false, reason: 'deadline' };

  const table: { current: Sensitivity; detector: DetectorResult; state: 'done' | 'partial' | 'failed'; expected: Sensitivity }[] = [
    { current: 'local_only', detector: clean, state: 'done', expected: 'eligible' },
    { current: 'local_only', detector: hit, state: 'done', expected: 'secret' },
    { current: 'local_only', detector: failed, state: 'done', expected: 'local_only' },
    { current: 'local_only', detector: cutOff, state: 'done', expected: 'local_only' },
    { current: 'local_only', detector: clean, state: 'partial', expected: 'local_only' },
    { current: 'local_only', detector: clean, state: 'failed', expected: 'local_only' },
    { current: 'local_only', detector: hit, state: 'partial', expected: 'local_only' },
    { current: 'eligible', detector: clean, state: 'done', expected: 'eligible' },
    { current: 'eligible', detector: hit, state: 'done', expected: 'secret' },
    { current: 'private', detector: clean, state: 'done', expected: 'private' },
    { current: 'private', detector: hit, state: 'done', expected: 'private' },
    { current: 'secret', detector: clean, state: 'done', expected: 'secret' },
    { current: 'secret', detector: hit, state: 'done', expected: 'secret' },
  ];

  for (const row of table) {
    assert.equal(
      promoteSensitivity(row.current, row.detector, row.state),
      row.expected,
      `${row.current} / ${row.state} / ${row.detector.ok ? row.detector.sensitivity : row.detector.reason}`,
    );
  }
});

test('strictest keeps the stricter class of the lattice', () => {
  assert.equal(strictest('eligible', 'local_only'), 'local_only');
  assert.equal(strictest('local_only', 'private'), 'private');
  assert.equal(strictest('private', 'secret'), 'secret');
  assert.equal(strictest('eligible'), 'eligible');
  assert.equal(strictest('eligible', 'eligible', 'eligible'), 'eligible');
  assert.equal(strictest('secret', 'eligible', 'private'), 'secret');
});

// SC-006: the producing agent is provenance only. `filterEgress` takes rows without an agent field,
// so a compile error here is the assertion that no egress decision can read one.
export type EgressRowKeys = keyof Parameters<typeof filterEgress>[2][number];
export type NoAgentInEgressRow = [EgressRowKeys] extends ['sensitivity' | 'repoId'] ? true : false;
const noAgentInEgressRow: NoAgentInEgressRow = true;

test('SC-006: changing only the producing agent changes no decision', async () => {
  assert.equal(noAgentInEgressRow, true);
  const line = corpusLine('generic-hex-entropy');

  const forClaude = { ...detectorInput(line.text), agent: 'claude' } as unknown as DetectorInput;
  const forGrok = { ...detectorInput(line.text), agent: 'grok' } as unknown as DetectorInput;

  const claudeDetected = await detectSync(forClaude);
  const grokDetected = await detectSync(forGrok);
  assert.deepEqual(claudeDetected, grokDetected);

  assert.equal(
    promoteSensitivity('local_only', claudeDetected, 'done'),
    promoteSensitivity('local_only', grokDetected, 'done'),
  );

  await withTempHome(async (home) => {
    const opened = openDatabase({ path: join(home, 'memory.db'), timeoutMs: 2_000 });
    try {
      const rules = loadDestinationRules(opened.db);
      const claudeRows = [{ sensitivity: 'eligible' as Sensitivity, repoId: 'repo-a', agent: 'claude' }];
      const grokRows = [{ sensitivity: 'eligible' as Sensitivity, repoId: 'repo-a', agent: 'grok' }];
      const claudeEgress = filterEgress(rules, 'remote_observer', claudeRows, 'repo-a');
      const grokEgress = filterEgress(rules, 'remote_observer', grokRows, 'repo-a');
      assert.deepEqual(
        claudeEgress.allowed.map((row) => row.sensitivity),
        grokEgress.allowed.map((row) => row.sensitivity),
      );
      assert.deepEqual(claudeEgress.blocked, []);
      assert.deepEqual(grokEgress.blocked, []);
    } finally {
      opened.db.close();
    }
  });
});

test('the detector wall time on large clean payloads is reported for the capture budget', async (t) => {
  const paragraph = `${CLEAN_TEXT}\n`;
  for (const size of [200_000, 1_000_000]) {
    const text = paragraph.repeat(Math.ceil(size / paragraph.length)).slice(0, size);
    const started = performance.now();
    const detected = assertDetected(await detectSync(detectorInput(text)), `${size} bytes`);
    const elapsed = performance.now() - started;
    assert.equal(detected.sensitivity, 'local_only');
    t.diagnostic(
      `detectSync on ${size} characters took ${elapsed.toFixed(1)} ms on Node ${process.versions.node}`,
    );
  }
});

// ---------------------------------------------------------------------------
// US3 end to end (T064): the real hook, the real detector, the real worker and the real pack.
// ---------------------------------------------------------------------------

/** The repository the fixture captures into: whatever identity the hook derived for `process.cwd()`. */
function repoIdOf(fixture: Fixture): string {
  return fixture.withDb((db) => {
    const row = db.prepare('SELECT id FROM repos').get();
    if (row === undefined) assert.fail('no repository row was created by capture');
    return String(row.id);
  });
}

function ensureRepo(db: DatabaseSync, id: string): void {
  db.prepare(
    `INSERT OR IGNORE INTO repos (id, identity_kind, normalized_identity, display_root, created_at, last_seen_at)
     VALUES (?, 'remote', ?, '/tmp/other-repository', 1, 1)`,
  ).run(id, `example.invalid/${id}.git`);
}

function seedMemory(
  db: DatabaseSync,
  memory: {
    id: string;
    repoId: string;
    title: string;
    body: string;
    sensitivity?: Sensitivity;
    pinned?: boolean;
    reviewState?: 'unreviewed' | 'imported';
  },
): void {
  db.prepare(
    `INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash, content_hash,
       sensitivity, review_state, pinned_at, pin_order, created_at)
     VALUES (?, ?, 'discovery', ?, ?, '', ?, ?, ?, ?, ?, ?, ?)`,
  ).run(
    memory.id,
    memory.repoId,
    memory.title,
    memory.body,
    `material_${memory.id}`,
    `content_${memory.id}`,
    memory.sensitivity ?? 'eligible',
    memory.reviewState ?? 'unreviewed',
    memory.pinned === true ? NOW - 1_000 : null,
    memory.pinned === true ? 1 : null,
    NOW - 2_000,
  );
  grantVisibility(db, memory.id, { audience: 'project', repoId: memory.repoId }, 'migration', NOW);
}

function storedEvents(fixture: Fixture, nativeSessionId: string, kind: string): { content: string | null; payload: Record<string, unknown>; id: string }[] {
  return fixture.withDb((db) =>
    db
      .prepare(
        `SELECT e.id, e.content, e.payload_json FROM raw_events e
         JOIN sessions s ON s.id = e.session_id
         WHERE s.native_session_id = ? AND e.kind = ? ORDER BY e.captured_at, e.id`,
      )
      .all(nativeSessionId, kind)
      .map((row) => ({
        id: String(row.id),
        content: row.content === null ? null : String(row.content),
        payload: JSON.parse(String(row.payload_json)) as Record<string, unknown>,
      })),
  );
}

/**
 * The observer input travels as the user message of the provider's own request shape (llm.ts), so
 * it is found by walking the request body and parsing the string that carries `repo_ref`.
 */
function observerInputOf(body: string): { repo_ref?: string; nearby?: { id: string }[] } {
  const queue: unknown[] = [JSON.parse(body)];
  while (queue.length > 0) {
    const value = queue.shift();
    if (typeof value === 'string') {
      try {
        queue.push(JSON.parse(value));
      } catch {
        // an ordinary string
      }
      continue;
    }
    if (value !== null && typeof value === 'object') {
      if ('repo_ref' in value) return value as { repo_ref?: string; nearby?: { id: string }[] };
      queue.push(...Object.values(value));
    }
  }
  assert.fail('the provider body carries no observer input');
}

test('FR-021: a pack that comes back through capture is recognized by its hash and never summarized', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await fixture.capture('SessionStart', { ...eventBase('seed'), source: 'startup' });
    const repoId = repoIdOf(fixture);
    fixture.withDb((db) =>
      seedMemory(db, {
        id: 'm_pinned',
        repoId,
        title: 'Uploader retry policy',
        body: 'PINNEDBODY the uploader retries three times with backoff.',
        pinned: true,
      }),
    );

    const common = eventBase('s-recapture');
    const start = await fixture.capture('SessionStart', { ...common, source: 'startup' });
    const pack = start.stdout ?? '';
    assert.ok(pack.startsWith(PACK_HEADER) && pack.includes('PINNEDBODY'), `the session-start pack was not built: ${pack}`);
    const packHash = fixture.withDb((db) =>
      String(db.prepare("SELECT pack_hash FROM injections WHERE pack_hash IS NOT NULL ORDER BY created_at DESC LIMIT 1").get()?.pack_hash),
    );
    assert.equal(packHash.length, 64);

    // The whole message is the pack: nothing of it is new activity.
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'p1', prompt: 'Repeat your notes.' });
    await fixture.capture('Stop', { ...common, prompt_id: 'p1', last_assistant_message: pack });
    // The pack embedded in real output: the output stays, the pack does not.
    const before = 'The tests pass on the retry branch.';
    const after = 'Nothing else changed.';
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'p2', prompt: 'Summarize the branch.' });
    await fixture.capture('Stop', {
      ...common,
      prompt_id: 'p2',
      last_assistant_message: `${before}\n${pack}\n${after}`,
    });
    await fixture.capture('SessionEnd', { ...common, reason: 'prompt_input_exit' });

    const messages = storedEvents(fixture, 's-recapture', 'last_assistant_message');
    assert.equal(messages.length, 2);
    assert.equal(messages[0]?.content, '');
    assert.deepEqual(messages[0]?.payload.recognized_packs, [packHash]);
    assert.equal(messages[1]?.content, `${before}\n\n${after}`);
    assert.deepEqual(messages[1]?.payload.recognized_packs, [packHash]);

    await runObserveForFixture(fixture);
    fixture.withDb((db) => {
      const session = db.prepare("SELECT id FROM sessions WHERE native_session_id = 's-recapture'").get();
      const produced = db
        .prepare('SELECT id, title, body FROM memories WHERE source_session_id = ?')
        .all(String(session?.id))
        .map((row) => ({ id: String(row.id), text: `${String(row.title)}\n${String(row.body)}` }));
      assert.ok(produced.length > 0, 'the remainder of the session is still summarized');
      for (const memory of produced) {
        assert.equal(memory.text.includes(PACK_HEADER), false, memory.text);
        assert.equal(memory.text.includes('PINNEDBODY'), false, memory.text);
      }
      // The pack-only message contributed nothing: no memory cites it as a source.
      const cited = db
        .prepare('SELECT COUNT(*) AS n FROM memory_sources WHERE raw_event_id = ?')
        .get(messages[0]?.id ?? '')?.n;
      assert.equal(cited, 0);
    });
  });
});

test('fail-open: a text that only looks like a pack is ordinary content', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const common = eventBase('s-lookalike');
    const lookalike = [PACK_HEADER, '> repository: example.invalid/other.git', '> a line nobody injected', 'end of oboete memory context'].join('\n');
    await fixture.capture('SessionStart', { ...common, source: 'startup' });
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'p1', prompt: 'What is this?' });
    await fixture.capture('Stop', { ...common, prompt_id: 'p1', last_assistant_message: lookalike });

    const [message] = storedEvents(fixture, 's-lookalike', 'last_assistant_message');
    assert.equal(message?.content, lookalike);
    assert.equal('recognized_packs' in (message?.payload ?? {}), false);
  });
});

test('FR-020: a memory of another repository is refused at the injection destination, this repository is delivered', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await fixture.capture('SessionStart', { ...eventBase('seed'), source: 'startup' });
    const repoId = repoIdOf(fixture);
    fixture.withDb((db) => {
      ensureRepo(db, 'repo-other');
      seedMemory(db, {
        id: 'm_other',
        repoId: 'repo-other',
        title: 'Uploader retry policy of the other project',
        body: 'OTHERREPO the uploader retries five times.',
        pinned: true,
      });
      seedMemory(db, {
        id: 'm_ours',
        repoId,
        title: 'Uploader retry policy',
        body: 'OURREPO the uploader retries three times.',
        pinned: true,
      });
    });

    const common = eventBase('s-cross');
    const start = await fixture.capture('SessionStart', { ...common, source: 'startup' });
    assert.ok((start.stdout ?? '').includes('OURREPO'), 'fail open: the memory of this repository is delivered');
    assert.equal((start.stdout ?? '').includes('OTHERREPO'), false);

    const prompt = await fixture.capture('UserPromptSubmit', {
      ...common,
      prompt_id: 'p1',
      prompt: 'How many times does the uploader retry in the other project?',
    });
    assert.equal((prompt.stdout ?? '').includes('OTHERREPO'), false);

    fixture.withDb((db) => {
      // Not omitted for a reason: never a candidate. The repository scope is applied before ranking.
      assert.equal(db.prepare("SELECT COUNT(*) AS n FROM injection_items WHERE memory_id = 'm_other'").get()?.n, 0);
      assert.equal(
        db.prepare("SELECT COUNT(*) AS n FROM injection_items WHERE memory_id = 'm_ours' AND decision = 'included'").get()?.n,
        1,
      );
    });
  });
});

test('SC-005/SC-006: the outbound body of a mixed session carries the eligible rows, an opaque repo_ref and nothing else', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, {
      OBOETE_CF_API_TOKEN: 'worker-test-token',
      OBOETE_CF_ACCOUNT_ID: 'worker-test-account',
    });
    writeConfig(fixture, 'workers-ai', fixture.env);
    appendFileSync(fixture.paths.config, '\n[privacy]\nsecret_paths = ["secrets/**"]\n');
    const secret = corpusLine('github-classic-pat');
    const eligiblePrompt = 'Add a retry to the uploader.';
    const eligibleAnswer = 'The uploader now retries three times.';

    const common = { ...eventBase('s-mixed'), cwd: fixture.home };
    await fixture.capture('SessionStart', { ...common, source: 'startup' });
    const repoId = repoIdOf(fixture);
    fixture.withDb((db) => {
      ensureRepo(db, 'repo-other');
      seedMemory(db, { id: 'm_eligible', repoId, title: 'Uploader retry count', body: 'The uploader retry count was one.' });
      db.prepare(`INSERT INTO memory_sources (memory_id, raw_event_id, capture_root, source_paths_json)
        VALUES ('m_eligible', 'retained-uploader-source', ?, '[]')`).run(fixture.home);
      seedMemory(db, {
        id: 'm_unprovenanced', repoId, title: 'Uploader retry history',
        body: 'UNPROVENANCED the uploader retry history has no retained origin.',
      });
      seedMemory(db, {
        id: 'm_local',
        repoId,
        title: 'Uploader retry notes',
        body: 'LOCALONLYMEMORY the uploader retry notes stay on this machine.',
        sensitivity: 'local_only',
      });
      seedMemory(db, {
        id: 'm_other',
        repoId: 'repo-other',
        title: 'Uploader retry in the other project',
        body: 'OTHERREPO the uploader retry of another repository.',
      });
    });
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'p1', prompt: eligiblePrompt });
    await fixture.capture('PreToolUse', {
      ...common,
      prompt_id: 'p1',
      tool_name: 'Edit',
      tool_use_id: 't1',
      tool_input: { file_path: 'src/uploader.ts', old_string: 'retries = 1', new_string: 'retries = 3' },
    });
    await fixture.capture('PreToolUse', {
      ...common,
      prompt_id: 'p1',
      tool_name: 'Edit',
      tool_use_id: 't2',
      tool_input: { file_path: 'src/SECRETPATH.ts', old_string: 'token = ""', new_string: secret.text },
    });
    await fixture.capture('UserPromptSubmit', {
      ...common,
      prompt_id: 'p2',
      prompt: 'Keep <private>PRIVATEMARKER the customer list</private> out of it and continue.',
    });
    await fixture.capture('PreToolUse', {
      ...common,
      prompt_id: 'p2',
      tool_name: 'Read',
      tool_use_id: 't3',
      tool_input: { file_path: 'secrets/aws.json' },
    });
    await fixture.capture('Stop', { ...common, prompt_id: 'p2', last_assistant_message: eligibleAnswer });
    await fixture.capture('SessionEnd', { ...common, reason: 'prompt_input_exit' });

    const sourceId = eventId(fixture, eligiblePrompt);
    let providerBody = '';
    let providerCalls = 0;
    const fetchImpl: typeof fetch = async (input, init) => {
      if (String(input).includes('/models/search')) {
        return catalogResponse(Number(new URL(String(input)).searchParams.get('page') ?? '1'));
      }
      providerCalls += 1;
      providerBody = String(init?.body ?? '');
      return workersResponse(providerOutput(sourceId));
    };
    await runObserveForFixture(fixture, { fetch: fetchImpl });
    assert.equal(providerCalls, 1);

    // Fail open (FR-023): the eligible rows travel.
    assert.equal(providerBody.includes(eligiblePrompt), true);
    assert.equal(providerBody.includes(eligibleAnswer), true);
    assert.equal(providerBody.includes('src/uploader.ts'), true);
    assert.equal(providerBody.includes('The uploader retry count was one.'), true);

    // Fail closed (SC-005, SC-006, FR-020): the secret row, the private span, the path-rule row,
    // the local-only memory and the other repository do not.
    assertNoSecretRun(providerBody, secret.secret ?? '', secret.id);
    assert.equal(providerBody.includes('SECRETPATH'), false, 'a secret row is dropped whole, not redacted');
    assert.equal(providerBody.includes('PRIVATEMARKER'), false);
    assert.equal(providerBody.includes('secrets/aws.json'), false);
    assert.equal(providerBody.includes('LOCALONLYMEMORY'), false);
    assert.equal(providerBody.includes('UNPROVENANCED'), false);
    assert.equal(providerBody.includes('OTHERREPO'), false);

    const body = observerInputOf(providerBody);
    assert.equal(body.repo_ref, repoId);
    assert.deepEqual((body.nearby ?? []).map((item) => item.id), ['m_eligible']);
    // R10: the identity that would leak is the normalized remote (or path); neither travels.
    const identity = fixture.withDb((db) => String(db.prepare('SELECT normalized_identity FROM repos WHERE id = ?').get(repoId)?.normalized_identity));
    assert.notEqual(identity, '');
    assert.equal(providerBody.includes(identity), false, 'the repository travels as an opaque id');
    assert.equal(providerBody.includes(process.cwd()), false);
    assert.equal(/\b(claude|codex|grok|pi)\b/i.test(providerBody), false, 'the producing agent is provenance only');
  });
});

type ProducedUnderAgent = {
  memories: Record<string, unknown>[];
  pack: string;
  decisions: Record<string, unknown>[];
};

test('derived nearby knowledge retains inherited paths before any provider send', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_CF_API_TOKEN: 'worker-test-token', OBOETE_CF_ACCOUNT_ID: 'worker-test-account' });
    writeConfig(fixture, 'workers-ai', fixture.env);
    const common = { ...eventBase('derived-context'), cwd: fixture.home };
    await fixture.capture('SessionStart', { ...common, source: 'startup' });
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'p1', prompt: 'Explain the uploader retry policy.' });
    const sourceId = eventId(fixture, 'Explain the uploader retry policy.');
    fixture.withDb((db) => {
      const repoId = repoIdOf(fixture);
      seedMemory(db, { id: 'm-derived', repoId, title: 'Uploader retry policy', body: 'Derived protected detail 9713.' });
      seedMemory(db, { id: 'm-ancestor', repoId, title: 'Internal constraint', body: 'Protected earlier context.' });
      db.exec("UPDATE memories SET provenance_complete = 1 WHERE id IN ('m-derived', 'm-ancestor')");
      db.prepare("INSERT INTO memory_sources (memory_id, raw_event_id, capture_root, source_paths_json) VALUES ('m-derived', ?, ?, '[]')")
        .run(sourceId, fixture.home);
      db.exec("INSERT INTO memory_sources (memory_id, source_memory_id, context_only) VALUES ('m-derived', 'm-ancestor', 1)");
      for (const id of ['m-derived', 'm-ancestor']) db.prepare(`INSERT INTO memory_sources
        (memory_id, capture_root, source_paths_json, context_only) VALUES (?, ?, '["protected/ancestor.ts"]', 1)`)
        .run(id, fixture.home);
    });
    await fixture.capture('SessionEnd', { ...common, reason: 'normal' });
    writeFileSync(join(fixture.home, '.oboete.toml'), '[privacy]\nsecret_paths = ["protected/**"]\n');
    const bodies: string[] = [];
    await runObserveForFixture(fixture, { fetch: (async (url, init) => {
      if (String(url).includes('/ai/models/search')) return catalogResponse();
      bodies.push(String(init?.body ?? ''));
      return workersResponse(providerOutput(sourceId));
    }) as typeof fetch });
    assert.ok(bodies.length > 0, 'the benign captured prompt still reaches the selected provider');
    for (const body of bodies) assert.doesNotMatch(body, /Derived protected detail 9713/);
  });
});

test('a session summary inherits privacy from every learned title it copies', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const common = { ...eventBase('learned-summary'), cwd: fixture.home };
    await fixture.capture('SessionStart', { ...common, source: 'startup' });
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'p1', prompt: 'Review the release checklist.' });
    await fixture.capture('SessionEnd', { ...common, reason: 'normal' });
    const { db } = openDatabase({ path: fixture.paths.db, timeoutMs: 1000 });
    try {
      const session = db.prepare('SELECT s.id, s.repo_id, b.id AS binding_id FROM sessions s JOIN work_bindings b ON b.session_id = s.id').get()!;
      seedMemory(db, { id: 'm-learned-protected', repoId: String(session.repo_id), title: 'Protected learned marker 9013', body: 'A prior release decision.' });
      db.prepare('UPDATE memories SET source_session_id = ?, provenance_complete = 1 WHERE id = ?').run(session.id, 'm-learned-protected');
      db.prepare(`INSERT INTO memory_sources (memory_id, capture_root, source_paths_json, context_only)
        VALUES ('m-learned-protected', ?, '["protected/learned.ts"]', 1)`).run(fixture.home);
      db.exec("UPDATE raw_events SET processing_state = 'processed'; UPDATE sessions SET summary_state = 'pending'");
      writeFileSync(join(fixture.home, '.oboete.toml'), '[privacy]\nsecret_paths = ["protected/**"]\n');
      const location = { repoId: String(session.repo_id), bindingId: String(session.binding_id), home: fixture.home, history: true };
      assert.deepEqual((await filterReadOutput(db, location, [{ id: 'm-learned-protected', title: 'Protected learned marker 9013' }], [])).memories, []);
      const token = claimLease(db, { pid: 1, now: NOW });
      assert.ok(token);
      const summary = sessionSummary(db, token, String(session.id), NOW);
      assert.ok(summary.memoryId);
      const body = String(db.prepare('SELECT body FROM memories WHERE id = ?').get(summary.memoryId)?.body);
      assert.match(body, /Protected learned marker 9013/);
      const shown = await filterReadOutput(db, location, [{ id: summary.memoryId, body }], []);
      assert.deepEqual(shown.memories, []);
    } finally { db.close(); }
  });
});

for (const [projectGrant, protectedPath] of [[true, false], [false, false], [true, true]] as const) {
  test(`observer reuses removed-origin knowledge only with its project grant and retained rules: ${projectGrant}/${protectedPath}`, async () => {
    await withFixture(async (fixture) => {
      const git = (...args: string[]) => {
        const result = spawnSync('git', ['-C', fixture.home, '-c', 'core.hooksPath=/dev/null', '-c', 'commit.gpgSign=false', ...args], { encoding: 'utf8' });
        assert.equal(result.status, 0, result.stderr);
      };
      git('init', '--quiet');
      git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '--quiet', '--allow-empty', '-m', 'Fixture');
      const oldRoot = join(fixture.home, 'old-worktree');
      git('worktree', 'add', '--quiet', '-b', 'old-work', oldRoot);
      const oldIdentity = resolveRepoIdentity(oldRoot);
      fixture.env = cleanEnv(fixture.home, { OBOETE_CF_API_TOKEN: 'worker-test-token', OBOETE_CF_ACCOUNT_ID: 'worker-test-account' });
      writeConfig(fixture, 'workers-ai', fixture.env);
      const common = { ...eventBase('adopted-context'), cwd: fixture.home };
      await fixture.capture('SessionStart', { ...common, source: 'startup' });
      await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'p1', prompt: 'Explain the adopted release policy.' });
      const sourceId = eventId(fixture, 'Explain the adopted release policy.');
      fixture.withDb((db) => {
        const repoId = repoIdOf(fixture);
        assert.equal(repoId, oldIdentity.id);
        db.prepare(`INSERT INTO work_contexts (id, repo_id, local_key, root, repo_secret_paths_json, created_at, last_seen_at)
          VALUES ('old-context', ?, ?, ?, '["protected/**"]', 1, 1)`).run(repoId, oldIdentity.worktreeKey, oldRoot);
        db.prepare(`INSERT INTO work_items (id, repo_id, origin_context_id, created_at, updated_at)
          VALUES ('old-work', ?, 'old-context', 1, 1)`).run(repoId);
        const source = db.prepare(`SELECT r.session_id, b.context_id FROM raw_events r
          JOIN work_bindings b ON b.id = r.work_binding_id WHERE r.id = ?`).get(sourceId)!;
        db.prepare(`INSERT INTO work_bindings (id, session_id, context_id, work_id, created_at, closed_at, reason)
          VALUES ('late-binding', ?, ?, NULL, ?, ?, 'late_source')`)
          .run(source.session_id, source.context_id, NOW - DAY - 100, NOW - DAY - 50);
        const chosen = chooseWork(db, { repoId, contextKey: resolveRepoIdentity(fixture.home).worktreeKey,
          bindingId: 'late-binding', workId: 'old-work', now: NOW });
        assert.equal(chosen?.reason, 'late_source', 'a historical choice retains its historical binding reason');
        db.prepare('UPDATE raw_events SET work_binding_id = ?, captured_at = ? WHERE id = ?')
          .run('late-binding', NOW - DAY - 75, sourceId);
        seedMemory(db, { id: 'm-adopted', repoId, title: 'Adopted release policy', body: 'Adopted release detail 6284.' });
        if (!projectGrant) db.prepare('DELETE FROM memory_visibility WHERE memory_id = ?').run('m-adopted');
        grantVisibility(db, 'm-adopted', { audience: 'work', repoId, workId: 'old-work' }, 'observer', NOW);
        db.exec("UPDATE memories SET provenance_complete = 1 WHERE id = 'm-adopted'");
        db.prepare(`INSERT INTO memory_sources (memory_id, capture_root, source_paths_json, source_context_id, context_only)
          VALUES ('m-adopted', ?, ?, 'old-context', 1)`)
          .run(oldRoot, JSON.stringify([protectedPath ? 'protected/rules.ts' : 'src/rules.ts']));
      });
      git('worktree', 'remove', '--force', oldRoot);
      await fixture.capture('SessionEnd', { ...common, reason: 'normal' });
      const bodies: string[] = [];
      await runObserveForFixture(fixture, { fetch: (async (url, init) => {
        if (String(url).includes('/ai/models/search')) return catalogResponse();
        bodies.push(String(init?.body ?? ''));
        return workersResponse(providerOutput(sourceId));
      }) as typeof fetch });
      assert.ok(bodies.length > 0);
      assert.equal(bodies.some((body) => body.includes('Adopted release detail 6284')), projectGrant && !protectedPath);
    });
  });
}

/** The same session, produced by one agent, then injected into a fresh Claude session. */
async function produceUnderAgent(agent: 'claude' | 'grok'): Promise<ProducedUnderAgent> {
  let produced: ProducedUnderAgent | null = null;
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await captureEndedSession(fixture, {
      sessionId: 'swap',
      prompts: ['Add a retry to the uploader.', 'Document the retry count in README.md.'],
      tools: [{ id: 't1', path: 'src/uploader.ts', text: 'retries = 1' }],
      assistant: 'The uploader now retries three times.',
    });
    // This comparison varies only the producer; keep the newly persisted work identity fixed too.
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM work_items').get()?.n, 1);
      db.exec(`BEGIN; PRAGMA defer_foreign_keys = ON;
        UPDATE work_items SET id = 'agent-neutral-work';
        UPDATE work_bindings SET work_id = 'agent-neutral-work'; COMMIT;`);
      assert.deepEqual(db.prepare('PRAGMA foreign_key_check').all(), []);
    });
    if (agent === 'grok') {
      // Only the producing agent changes; every byte of content stays.
      fixture.withDb((db) => {
        db.exec("UPDATE raw_events SET agent = 'grok'");
        db.exec("UPDATE sessions SET agent = 'grok'");
      });
    }
    await runObserveForFixture(fixture);
    const start = await fixture.capture('SessionStart', { ...eventBase('s-after-swap'), source: 'startup' });
    produced = fixture.withDb((db) => ({
      memories: db
        .prepare(
          `SELECT type, title, body, material_hash, content_hash, sensitivity, review_state, degraded_reason
           FROM memories ORDER BY content_hash`,
        )
        .all()
        .map((row) => ({ ...row })),
      pack: start.stdout ?? '',
      decisions: db
        .prepare(
          `SELECT i.source_kind, i.decision, i.reason, m.content_hash
           FROM injection_items i LEFT JOIN memories m ON m.id = i.memory_id
           ORDER BY m.content_hash, i.decision`,
        )
        .all()
        .map((row) => ({ ...row })),
    }));
  });
  if (produced === null) assert.fail('the fixture did not run');
  return produced;
}

test('SC-006: changing only the producing agent changes no memory hash, no body, no pack and no injection decision', async () => {
  const byClaude = await produceUnderAgent('claude');
  const byGrok = await produceUnderAgent('grok');
  assert.ok(byClaude.memories.length > 0, 'the worker produced memories');
  assert.ok(byClaude.pack.startsWith(PACK_HEADER), `a pack was injected: ${byClaude.pack}`);
  assert.deepEqual(byGrok.memories, byClaude.memories);
  // The pack dates its items against the real clock; the two runs may straddle a boundary of it.
  const undated = (pack: string) => pack.replace(/\b(\d+ (minute|hour|day)s? ago|yesterday|just now)\b/g, 'AGO');
  assert.equal(undated(byGrok.pack), undated(byClaude.pack));
  assert.deepEqual(byGrok.decisions, byClaude.decisions);
});

test('FR-021: a pack captured while the database is unavailable is recognized when the spool is recovered', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await fixture.capture('SessionStart', { ...eventBase('seed'), source: 'startup' });
    const repoId = repoIdOf(fixture);
    fixture.withDb((db) =>
      seedMemory(db, {
        id: 'm_pinned',
        repoId,
        title: 'Uploader retry policy',
        body: 'PINNEDBODY the uploader retries three times with backoff.',
        pinned: true,
      }),
    );
    const common = eventBase('s-spooled');
    const pack = (await fixture.capture('SessionStart', { ...common, source: 'startup' })).stdout ?? '';
    assert.ok(pack.includes('PINNEDBODY'), `the session-start pack was not built: ${pack}`);
    const packHash = fixture.withDb((db) =>
      String(db.prepare("SELECT pack_hash FROM injections WHERE pack_hash IS NOT NULL ORDER BY created_at DESC LIMIT 1").get()?.pack_hash),
    );
    await fixture.capture('UserPromptSubmit', { ...common, prompt_id: 'p1', prompt: 'Repeat your notes.' });

    // The database is gone for one hook: the message is spooled with the pack still inside it,
    // because without the database the hook has no hash to recognize it by.
    toggleDatabase(fixture, true);
    try {
      await fixture.capture('Stop', { ...common, prompt_id: 'p1', last_assistant_message: pack }, 'spooled');
    } finally {
      toggleDatabase(fixture, false);
    }
    // Stop yields the message and the turn end, so two entries; the message entry still holds the pack.
    const spooled = readdirSync(fixture.paths.spool)
      .filter((name) => name.endsWith('.json'))
      .map((name) => readFileSync(join(fixture.paths.spool, name), 'utf8'));
    assert.equal(spooled.length, 2);
    assert.equal(spooled.some((entry) => entry.includes('PINNEDBODY')), true);
    await fixture.capture('SessionEnd', { ...common, reason: 'prompt_input_exit' });

    await runObserveForFixture(fixture);
    const [message] = storedEvents(fixture, 's-spooled', 'last_assistant_message');
    assert.equal(message?.content, '');
    assert.deepEqual(message?.payload.recognized_packs, [packHash]);
  });
});

// ---------------------------------------------------------------------------
// Imported rows (T066): quarantined until the worker's detector and directive check let them out.
// ---------------------------------------------------------------------------

test('quarantine: an imported row leaves quarantine only on a clean, complete detector run without a directive', () => {
  const clean: DetectorResult = {
    ok: true,
    text: 'note',
    texts: [],
    redactions: [],
    privateRemoved: 0,
    sensitivity: 'local_only',
    pathRule: null,
  };
  const hit: DetectorResult = { ...clean, redactions: [{ rule: 'github', count: 1 }], sensitivity: 'secret' };
  const failed: DetectorResult = { ok: false, reason: 'detector_error' };

  const stripped: DetectorResult = { ...clean, text: 'note ', privateRemoved: 1 };

  assert.deepEqual(reclassifyImportedRow(clean, clean, false), { decision: 'unreviewed', title: 'note', body: 'note' });
  // What the detector removed is what gets stored, on release as much as on a tombstone.
  assert.deepEqual(reclassifyImportedRow(clean, stripped, false), { decision: 'unreviewed', title: 'note', body: 'note ' });
  assert.equal(reclassifyImportedRow(hit, clean, false).decision, 'secret');
  assert.equal(reclassifyImportedRow(clean, hit, false).decision, 'secret');
  assert.equal(reclassifyImportedRow(clean, clean, true).decision, 'secret');
  // A detector that did not finish decides nothing: the row stays quarantined for the next run.
  assert.deepEqual(reclassifyImportedRow(failed, clean, false), { decision: 'retry' });
  assert.deepEqual(reclassifyImportedRow(clean, failed, false), { decision: 'retry' });
  assert.deepEqual(reclassifyImportedRow(failed, hit, true), { decision: 'retry' });
});

test('quarantine: imported memories reach no pack before the worker classifies them, and only the clean one after', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await fixture.capture('SessionStart', { ...eventBase('seed'), source: 'startup' });
    const repoId = repoIdOf(fixture);
    const secret = corpusLine('github-classic-pat');
    fixture.withDb((db) => {
      seedMemory(db, { id: 'm_plain', repoId, title: 'Plain pinned note', body: 'PLAINPINNED the uploader retries.', pinned: true });
      seedMemory(db, {
        id: 'm_import_clean',
        repoId,
        title: 'Imported retry note',
        body: 'IMPORTEDCLEAN the uploader retries three times.',
        sensitivity: 'local_only',
        reviewState: 'imported',
        pinned: true,
      });
      seedMemory(db, {
        id: 'm_import_secret',
        repoId,
        title: 'Imported token note',
        body: `IMPORTEDSECRET ${secret.text}`,
        sensitivity: 'local_only',
        reviewState: 'imported',
        pinned: true,
      });
      seedMemory(db, {
        id: 'm_import_directive',
        repoId,
        title: 'Imported instruction',
        body: `IMPORTEDDIRECTIVE ${DIRECTIVE_PHRASES[0]}`,
        sensitivity: 'local_only',
        reviewState: 'imported',
        pinned: true,
      });
      seedMemory(db, {
        id: 'm_import_private',
        repoId,
        title: 'Imported note with a private span',
        body: 'IMPORTEDPRIVATE keep this <private>IMPORTEDPRIVATESPAN drop this</private> end.',
        sensitivity: 'local_only',
        reviewState: 'imported',
        pinned: true,
      });
    });

    const before = (await fixture.capture('SessionStart', { ...eventBase('s-before'), source: 'startup' })).stdout ?? '';
    assert.equal(before.includes('PLAINPINNED'), true, `a pack was built: ${before}`);
    assert.equal(before.includes('IMPORTED'), false, `quarantined rows reached a pack: ${before}`);

    await runObserveForFixture(fixture);

    fixture.withDb((db) => {
      const rows = db
        .prepare("SELECT id, review_state, sensitivity, deleted_at, body FROM memories WHERE id LIKE 'm_import_%' ORDER BY id")
        .all()
        .map((row) => ({ ...row }));
      assert.deepEqual(
        rows.map((row) => ({ id: row.id, review_state: row.review_state, sensitivity: row.sensitivity, deleted: row.deleted_at !== null })),
        [
          { id: 'm_import_clean', review_state: 'unreviewed', sensitivity: 'local_only', deleted: false },
          { id: 'm_import_directive', review_state: 'imported', sensitivity: 'secret', deleted: true },
          { id: 'm_import_private', review_state: 'imported', sensitivity: 'local_only', deleted: true },
          { id: 'm_import_secret', review_state: 'imported', sensitivity: 'secret', deleted: true },
        ],
      );
      // FR-018: the secret does not stay in the tombstone either.
      const tombstone = rows.find((row) => row.id === 'm_import_secret');
      assertNoSecretRun(String(tombstone?.body), secret.secret ?? '', secret.id);
      // FR-019: a released row stores what the detector left, not what was imported.
      const sanitized = db.prepare("SELECT id, body, review_state FROM memories WHERE title = ? AND deleted_at IS NULL")
        .get('Imported note with a private span');
      assert.equal(sanitized?.body, 'IMPORTEDPRIVATE keep this  end.');
      assert.equal(sanitized?.review_state, 'unreviewed');
      assert.notEqual(sanitized?.id, 'm_import_private', 'sanitized material has a new identity');
    });

    const after = (await fixture.capture('SessionStart', { ...eventBase('s-after'), source: 'startup' })).stdout ?? '';
    assert.equal(after.includes('IMPORTEDCLEAN'), true, `the classified row is injectable: ${after}`);
    assert.equal(after.includes('IMPORTEDSECRET'), false);
    assert.equal(after.includes('IMPORTEDDIRECTIVE'), false);
    assert.equal(after.includes('IMPORTEDPRIVATESPAN'), false);
  });
});

test('FR-021: recognition survives a pack line that quotes the footer, two packs in one text, and a header mid-line', async () => {
  await withTempHome(async (home) => {
    const opened = openDatabase({ path: join(home, 'memory.db'), timeoutMs: 2_000 });
    try {
      const quoting = [PACK_HEADER, '> repository: example.invalid/one.git', `> pinned: the marker line is ${PACK_FOOTER}`, PACK_FOOTER].join('\n');
      const plain = [PACK_HEADER, '> repository: example.invalid/one.git', '> pinned: another note', PACK_FOOTER].join('\n');
      const insert = opened.db.prepare(
        "INSERT INTO injections (id, kind, state, pack_hash, created_at) VALUES (?, 'session_start', 'emitted', ?, 1)",
      );
      insert.run('i1', packHash(quoting));
      insert.run('i2', packHash(plain));

      assert.deepEqual(stripRecognizedPacks(opened.db, `before\n${quoting}\nmiddle\n${plain}\nafter`), {
        text: 'before\n\nmiddle\n\nafter',
        hashes: [packHash(quoting), packHash(plain)],
      });
      // The header inside a sentence is not a pack; the issued pack after it still is.
      assert.deepEqual(stripRecognizedPacks(opened.db, `the ${PACK_HEADER} marker, then\n${plain}`), {
        text: `the ${PACK_HEADER} marker, then\n`,
        hashes: [packHash(plain)],
      });
      // An unterminated header is content.
      const unterminated = `${PACK_HEADER}\n> repository: nothing closes this`;
      assert.deepEqual(stripRecognizedPacks(opened.db, unterminated), { text: unterminated, hashes: [] });
    } finally {
      opened.db.close();
    }
  });
});

test('FR-016: a configured credential value is redacted whatever it looks like, and a short one is not a credential', async () => {
  const value = 'plain-words-credential-000';
  const text = `the key ${value} selects the preset; ${value.slice(0, 5)} alone is a word`;
  const redacted = assertDetected(
    await detectSync({ ...detectorInput(text), credentialValues: [value] }),
    'configured credential',
  );
  assert.equal(redacted.text, 'the key [REDACTED:oboete-credential] selects the preset; plain alone is a word');
  assert.equal(redacted.sensitivity, 'secret');
  assert.deepEqual(redacted.redactions, [{ rule: 'oboete-credential', count: 1 }]);
  // Fail open: the same text with no configured credential is byte-identical and local_only.
  const untouched = assertDetected(await detectSync(detectorInput(text)), 'no credential');
  assert.equal(untouched.text, text);
  assert.equal(untouched.sensitivity, 'local_only');
  // A placeholder shorter than a real credential never becomes one (log.ts credentialValues).
  assert.deepEqual(credentialValues({ OBOETE_OPENAI_API_KEY: 'test', OBOETE_CF_ACCOUNT_ID: ' ' }), []);
  assert.deepEqual(credentialValues({ OBOETE_OPENROUTER_API_KEY: value, OBOETE_CF_ACCOUNT_ID: 'acct-1234' }), [value, 'acct-1234']);
});
