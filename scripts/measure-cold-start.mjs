#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import {
  closeSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { performance } from 'node:perf_hooks';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

const WARM_UPS = 3;
const RUNS = 30;
const ATTEMPTS = 2;
const BIG_LINE_COUNT = 1_272;
const ROOT = resolve(fileURLToPath(new URL('..', import.meta.url)));
const FIXTURE = JSON.parse(
  readFileSync(join(ROOT, 'test', 'contracts', 'claude', 'read.json'), 'utf8'),
).events.PostToolUse;

const { values } = parseArgs({
  options: {
    bundle: { type: 'string' },
    markdown: { type: 'boolean', default: false },
    node: { type: 'string', multiple: true },
  },
  strict: true,
});

const bundle = resolve(values.bundle ?? join(ROOT, 'dist', 'oboete.mjs'));
const nodes = [...new Set((values.node ?? [process.execPath]).map((path) => resolve(path)))];

for (const path of [bundle, ...nodes]) {
  if (!existsSync(path)) throw new Error(`not found: ${path}`);
}

function run(file, args, options = {}) {
  const capture = mkdtempSync(join(tmpdir(), 'oboete-command-'));
  const stdoutPath = join(capture, 'stdout');
  const stderrPath = join(capture, 'stderr');
  const stdout = openSync(stdoutPath, 'w');
  const stderr = openSync(stderrPath, 'w');
  let result;
  try {
    result = spawnSync(file, args, {
      encoding: 'utf8',
      timeout: 10_000,
      ...options,
      stdio: ['ignore', stdout, stderr],
    });
  } finally {
    closeSync(stdout);
    closeSync(stderr);
  }
  const output = {
    ...result,
    stdout: readFileSync(stdoutPath, 'utf8'),
    stderr: readFileSync(stderrPath, 'utf8'),
  };
  rmSync(capture, { recursive: true, force: true });
  if (output.status !== 0) {
    const detail = output.error?.message ?? output.stderr.trim() ?? `exit ${String(output.status)}`;
    throw new Error(`${file} ${args.join(' ')} failed: ${detail}`);
  }
  return output;
}

function loadAverage() {
  const raw = readFileSync('/proc/loadavg', 'utf8').trim();
  return { raw, oneMinute: Number(raw.split(/\s+/, 1)[0]) };
}

function displayPath(path) {
  if (path.startsWith(`${ROOT}/`)) return path.slice(ROOT.length + 1);
  const home = process.env.HOME;
  return home !== undefined && path.startsWith(`${home}/`)
    ? `$HOME/${path.slice(home.length + 1)}`
    : path;
}

function percentile(valuesToSort, fraction) {
  const sorted = [...valuesToSort].sort((left, right) => left - right);
  const position = (sorted.length - 1) * fraction;
  const lower = Math.floor(position);
  const upper = Math.ceil(position);
  const weight = position - lower;
  return (sorted[lower] ?? 0) * (1 - weight) + (sorted[upper] ?? 0) * weight;
}

function stats(samples) {
  return {
    p50: percentile(samples, 0.5),
    p95: percentile(samples, 0.95),
    max: Math.max(...samples),
  };
}

function measuredSpawn(node, args, options) {
  const { stdinFile, ...spawnOptions } = options ?? {};
  const stdin = stdinFile === undefined ? undefined : openSync(stdinFile, 'r');
  const started = performance.now();
  let result;
  try {
    result = spawnSync(node, args, {
      encoding: 'utf8',
      maxBuffer: 16 * 1024 * 1024,
      timeout: 5_000,
      ...spawnOptions,
      stdio: stdin === undefined ? ['ignore', 'pipe', 'pipe'] : [stdin, 'pipe', 'pipe'],
    });
  } finally {
    if (stdin !== undefined) closeSync(stdin);
  }
  const elapsed = performance.now() - started;
  if (result.status !== 0) {
    const detail = result.error?.message ?? result.stderr.trim() ?? `exit ${String(result.status)}`;
    throw new Error(`${node} ${args.join(' ')} failed: ${detail}`);
  }
  return elapsed;
}

function measureVersion(node) {
  const samples = [];
  for (let index = 0; index < WARM_UPS + RUNS; index += 1) {
    const elapsed = measuredSpawn(node, [bundle, '--version']);
    if (index >= WARM_UPS) samples.push(elapsed);
  }
  return {
    scenario: '`--version`',
    stdinBytes: 0,
    ...stats(samples),
    hookP50: 'n/a',
    landed: 'n/a',
    budget: 100,
  };
}

function line(prefix, index) {
  return `${prefix} ${String(index).padStart(4, '0')}`.padEnd(160, '.');
}

const cleanContent =
  Array.from({ length: BIG_LINE_COUNT }, (_, index) => line('clean measurement text', index)).join(
    '\n',
  ) + '\n';
const secretContent =
  Array.from({ length: BIG_LINE_COUNT }, (_, index) => {
    const hex = `${index.toString(16).padStart(8, '0')}${'0123456789abcdef'.repeat(3)}`.slice(0, 48);
    return line(`api_key=${hex}`, index);
  }).join('\n') + '\n';

function payload(repo, content, sample) {
  const value = JSON.parse(JSON.stringify(FIXTURE));
  value.cwd = repo;
  value.transcript_path = join(repo, 'transcript.jsonl');
  value.tool_use_id = `toolu_oboete_measure_${String(sample).padStart(4, '0')}`;
  value.tool_input.file_path = join(repo, 'README.md');
  value.tool_response.file.filePath = join(repo, 'README.md');
  value.tool_response.file.content = content;
  return JSON.stringify(value);
}

function hookLogP50(home) {
  const path = join(home, 'logs', 'hook.log');
  if (!existsSync(path)) return 'not recorded by hook.log';
  const samples = readFileSync(path, 'utf8')
    .trimEnd()
    .split('\n')
    .slice(WARM_UPS)
    .map((entry) =>
      /\b(?:wall_ms|elapsed_ms|duration_ms)=([0-9]+(?:\.[0-9]+)?)(?:\s|$)/.exec(entry),
    )
    .filter((match) => match !== null)
    .map((match) => Number(match[1]));
  return samples.length === RUNS ? `${percentile(samples, 0.5).toFixed(1)} ms` : 'not recorded by hook.log';
}

function rawEventCount(node, home) {
  const code =
    "import { DatabaseSync } from 'node:sqlite';" +
    "const db = new DatabaseSync(process.argv[1], { readOnly: true });" +
    "process.stdout.write(String(db.prepare('SELECT count(*) AS count FROM raw_events').get().count));" +
    'db.close();';
  return Number(
    run(node, ['--no-warnings', '--input-type=module', '--eval', code, join(home, 'memory.db')]).stdout,
  );
}

function spoolCount(home) {
  const spool = join(home, 'spool');
  return existsSync(spool)
    ? readdirSync(spool, { withFileTypes: true }).filter(
        (entry) => entry.isFile() && entry.name.endsWith('.json'),
      ).length
    : 0;
}

function measureHook(node, parent, key, description, content, databasePresent) {
  const home = join(parent, `${key}-home`);
  const repo = join(parent, `${key}-repo`);
  mkdirSync(home);
  mkdirSync(repo);
  run('git', ['-C', repo, 'init', '--quiet']);

  const env = { ...process.env, OBOETE_HOME: home };
  delete env.GROK_SESSION_ID;
  if (databasePresent) run(node, [bundle, 'observe'], { cwd: repo, env });

  const inputs = Array.from({ length: WARM_UPS + RUNS }, (_, index) => payload(repo, content, index));
  const inputFiles = inputs.map((input, index) => {
    const path = join(parent, `${key}-input-${String(index).padStart(2, '0')}.json`);
    writeFileSync(path, input);
    return path;
  });
  const samples = [];
  for (let index = 0; index < inputs.length; index += 1) {
    const elapsed = measuredSpawn(
      node,
      [bundle, 'hook', '--agent', 'claude-or-grok', '--event', 'PostToolUse'],
      { cwd: repo, env, stdinFile: inputFiles[index] },
    );
    if (index >= WARM_UPS) samples.push(elapsed);
  }

  const landed = databasePresent
    ? `raw_events=${rawEventCount(node, home)}`
    : `spool files=${spoolCount(home)}; memory.db absent=${existsSync(join(home, 'memory.db')) ? 'no' : 'yes'}`;
  return {
    scenario: description,
    stdinBytes: Buffer.byteLength(inputs[0]),
    ...stats(samples),
    hookP50: hookLogP50(home),
    landed,
    budget: 300,
  };
}

function measureNode(node, parent, attempt) {
  const version = run(node, ['--version']).stdout.trim();
  const safeVersion = version.replace(/[^A-Za-z0-9_.-]/g, '_');
  const nodeRoot = join(parent, safeVersion);
  mkdirSync(nodeRoot);
  const scenarios = [
    measureVersion(node),
    measureHook(node, nodeRoot, 'small-db', 'hook small, DB present', FIXTURE.tool_response.file.content, true),
    measureHook(node, nodeRoot, 'clean-200kb', 'hook clean 200 KB, DB present', cleanContent, true),
    measureHook(
      node,
      nodeRoot,
      'secret-200kb',
      'hook secret-dense 200 KB, DB present',
      secretContent,
      true,
    ),
    measureHook(node, nodeRoot, 'small-spool', 'hook small, DB absent (spool)', FIXTURE.tool_response.file.content, false),
  ];
  for (const scenario of scenarios) {
    process.stderr.write(`attempt ${attempt}/${ATTEMPTS} ${version} ${scenario.scenario}\n`);
  }
  return { node, version, scenarios };
}

function measurementAttempt(index) {
  const load = loadAverage();
  const temporary = mkdtempSync(join(tmpdir(), 'oboete-cold-start-'));
  try {
    return {
      index,
      load,
      results: nodes.map((node) => measureNode(node, temporary, index)),
    };
  } finally {
    if (!temporary.startsWith(join(tmpdir(), 'oboete-cold-start-'))) {
      throw new Error(`refusing to remove unexpected path: ${temporary}`);
    }
    rmSync(temporary, { recursive: true, force: true });
  }
}

const measuredAt = new Date().toISOString();
const nodeVersions = nodes.map((node) => ({ node, version: run(node, ['--version']).stdout.trim() }));
const commit = run('git', ['-C', ROOT, 'rev-parse', '--short', 'HEAD']).stdout.trim();
const attempts = Array.from({ length: ATTEMPTS }, (_, index) => measurementAttempt(index + 1));
const kept = attempts.reduce((best, candidate) =>
  candidate.load.oneMinute < best.load.oneMinute ? candidate : best,
);

const lines = [];
if (values.markdown) lines.push('<!-- measure:start -->');
lines.push(`- Date: ${measuredAt}`);
lines.push(
  `- Node versions: ${nodeVersions.map(({ node, version }) => `\`${displayPath(node)}\` (${version})`).join('; ')}`,
);
lines.push(`- Commit: \`${commit}\``);
lines.push(`- Bundle: \`${displayPath(bundle)}\` (${statSync(bundle).size} bytes)`);
lines.push(`- Samples: ${RUNS} measured runs after ${WARM_UPS} warm-up runs per scenario`);
lines.push(
  `- Measurement attempts: ${attempts.map((attempt) => `run ${attempt.index} load \`${attempt.load.raw}\``).join('; ')}; kept run ${kept.index} (lower 1-minute load average)`,
);
lines.push('- Percentiles: linear interpolation over the 30 measured runs; status is `max <= budget`');
lines.push('');
lines.push(`Load average next to this table (kept run ${kept.index}, before the measurement set): \`${kept.load.raw}\``);
lines.push('');
lines.push('| Node | Scenario | stdin bytes | p50 ms | p95 ms | max ms | hook.log wall p50 | Landed | Budget | Status |');
lines.push('|---|---|---:|---:|---:|---:|---|---|---:|---|');
for (const result of kept.results) {
  for (const scenario of result.scenarios) {
    lines.push(
      `| ${result.version} | ${scenario.scenario} | ${scenario.stdinBytes} | ${scenario.p50.toFixed(1)} | ${scenario.p95.toFixed(1)} | ${scenario.max.toFixed(1)} | ${scenario.hookP50} | ${scenario.landed} | ${scenario.budget} ms | ${scenario.max <= scenario.budget ? 'pass' : 'fail'} |`,
    );
  }
}
if (values.markdown) lines.push('<!-- measure:end -->');

process.stdout.write(`${lines.join('\n')}\n`);
