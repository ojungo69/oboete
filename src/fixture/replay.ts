// `oboete fixture replay`: native payloads through the real hook, resource-envelope evidence.
// Never on the hook path (cli.ts loads this command lazily). Sources: contracts/cli.md,
// contracts/agents.md hook SLAs, spec SC-002/003/005/009/010, FR-040, quickstart "Fixture replay".
import { execFile, spawn, spawnSync } from 'node:child_process';
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { hostname, tmpdir, type as osType, cpus, release, arch } from 'node:os';
import { dirname, isAbsolute, join, resolve } from 'node:path';
import { performance } from 'node:perf_hooks';
import { setTimeout as sleep } from 'node:timers/promises';
import { parseArgs } from 'node:util';

import { CAPTURE_DEADLINE_MS, INJECTION_DEADLINE_MS, hookDeadlineMs } from '../capture.js';
import { openDatabase } from '../db/open.js';
import { compareCodeUnits } from '../hash.js';
import { DEGRADED_SENTENCES } from '../injection/pack.js';
import { childEnvironment } from '../log.js';
import { ensureDirectories, oboetePaths } from '../paths.js';
import { isLeaseFree } from '../worker/lease.js';

const ROOT_PH = '__OBOETE_REPLAY_ROOT__';
const FILL_ALPHABET = 'The quick brown fox jumps over the lazy dog. ';
const AT_BOUND = 1_048_576;
const ABOVE_ONE = 1_048_577;
const ABOVE_TWO = 2_097_152;
const READY_BOUND_MS = 300;
// spec.md US2 AC-3 / FR-024 bound the summary *wait* at 1 s; the hook's own deadline is the engine's
// INJECTION_DEADLINE_MS (the 300 ms ready budget plus that wait), and inject.ts caps the wait at the
// remaining budget, so the pending path is judged on wall time against 1300 ms.
const PENDING_BOUND_MS = INJECTION_DEADLINE_MS;
const WORKER_RSS_BOUND_KB = 150 * 1024;
const RECALL_BOUND = 0.9;
const HEADING = '## Fixture replay (T068)';
const SUMMARY_PENDING = DEGRADED_SENTENCES.summary_pending;
const SESSION_END = new Set(['SessionEnd', 'session_shutdown']);
const AGENTS = ['claude', 'codex', 'grok', 'pi'] as const;
const LEASE_TOKEN = 't068-replay';

type Agent = (typeof AGENTS)[number];
type SizeTag = 'at_bound' | 'above_bound';
type Fact = { id: string; lang: 'ja' | 'en'; query: string; expect: string };
type Tags = {
  secret?: string;
  directive?: number;
  fact?: Fact;
  lifecycle?: string;
  size?: SizeTag;
  recall?: string;
};
type Line = {
  seq: number;
  agent: Agent;
  event: string;
  session: string;
  payload: unknown;
  tags?: Tags;
};
type Spawned = {
  status: number | null;
  signal: string | null;
  timedOut: boolean;
  stdout: string;
  stderr: string;
  elapsedMs: number;
};
type Sample = { agent: Agent; event: string; seq: number; session: string; ms: number };
type SizeRow = {
  seq: number;
  agent: Agent;
  event: string;
  tag: SizeTag;
  fillBytes: number;
  ms: number;
  classification: string;
  truncated: number;
};
type RecallHit = { id: string; lang: 'ja' | 'en'; query: string; expect: string; hit: boolean };
type BoundRow = { sc: string; measured: string; bound: string; status: 'pass' | 'fail' };
type HookFailure = {
  seq: number;
  agent: Agent;
  event: string;
  status: string;
  stderr: string;
};
type ResumeCheck = {
  seq: number;
  agent: Agent;
  session: string;
  packPrinted: boolean;
  injectionDelta: number;
};

function fillBytes(n: number): string {
  if (n <= 0) return '';
  const unit = FILL_ALPHABET;
  return unit.repeat(Math.ceil(n / unit.length)).slice(0, n);
}

function isAgent(value: string): value is Agent {
  return (AGENTS as readonly string[]).includes(value);
}

function loadJsonl(path: string): unknown[] {
  return readFileSync(path, 'utf8')
    .trimEnd()
    .split('\n')
    .filter((line) => line !== '')
    .map((line) => JSON.parse(line) as unknown);
}

function walk(value: unknown, map: (text: string) => string): unknown {
  if (typeof value === 'string') return map(value);
  if (Array.isArray(value)) return value.map((item) => walk(item, map));
  if (value !== null && typeof value === 'object') {
    const out: Record<string, unknown> = {};
    for (const [key, item] of Object.entries(value as Record<string, unknown>)) {
      out[key] = walk(item, map);
    }
    return out;
  }
  return value;
}

function expandString(
  text: string,
  parts: { root?: string; secrets?: Map<string, string>; directives?: string[]; fill: boolean },
): string {
  let out = text;
  if (parts.fill) {
    out = out.replace(/__FILL:(\d+)__/g, (_, n) => fillBytes(Number(n)));
  }
  if (parts.secrets !== undefined) {
    out = out.replace(/__SECRET:([a-z0-9-]+)__/g, (_, id: string) => {
      const value = parts.secrets?.get(id);
      if (value === undefined) throw new Error(`unknown secret id ${id}`);
      return value;
    });
  }
  if (parts.directives !== undefined) {
    out = out.replace(/__DIRECTIVE:(\d+)__/g, (_, index: string) => {
      const phrase = parts.directives?.[Number(index)];
      if (phrase === undefined) throw new Error(`unknown directive ${index}`);
      return phrase;
    });
  }
  if (parts.root !== undefined) out = out.split(ROOT_PH).join(parts.root);
  return out;
}

function expandPayload(
  payload: unknown,
  parts: { root?: string; secrets?: Map<string, string>; directives?: string[]; fill: boolean },
): unknown {
  return walk(payload, (text) => expandString(text, parts));
}

function repositoryRoot(): string {
  const fromArgv = process.argv[1] === undefined ? undefined : resolve(process.argv[1]);
  if (fromArgv !== undefined) {
    const dir = dirname(fromArgv);
    const parent = resolve(dir, '..');
    if (existsSync(join(parent, 'package.json')) && existsSync(join(dir, 'oboete.mjs'))) return parent;
  }
  let directory = process.cwd();
  for (;;) {
    if (existsSync(join(directory, 'package.json'))) return directory;
    const parent = dirname(directory);
    if (parent === directory) {
      throw new Error('could not find repository root: no package.json above cwd or the engine bundle');
    }
    directory = parent;
  }
}

function bundlePath(): string {
  if (process.argv[1] !== undefined && existsSync(process.argv[1])) return resolve(process.argv[1]);
  return join(repositoryRoot(), 'dist', 'oboete.mjs');
}

function usage(): string {
  return (
    'Usage: oboete fixture replay <file> [--out <markdown file>] [--json] [--home <dir>] [--keep]\n'
  );
}

function asLine(raw: unknown, index: number): Line {
  if (raw === null || typeof raw !== 'object') throw new Error(`fixture line ${index + 1} is not an object`);
  const row = raw as Record<string, unknown>;
  if (typeof row.seq !== 'number' || typeof row.event !== 'string' || typeof row.session !== 'string') {
    throw new TypeError(`fixture line ${index + 1} is missing seq/event/session`);
  }
  if (typeof row.agent !== 'string' || !isAgent(row.agent)) {
    throw new Error(`fixture line ${index + 1} has unknown agent`);
  }
  return {
    seq: row.seq,
    agent: row.agent,
    event: row.event,
    session: row.session,
    payload: row.payload,
    tags: row.tags as Tags | undefined,
  };
}

function isInjectionHook(agent: Agent, event: string): boolean {
  return hookDeadlineMs(agent, event) === INJECTION_DEADLINE_MS;
}

function sessionStartEvent(agent: Agent, event: string): boolean {
  return agent === 'pi' ? event === 'session_start' : event === 'SessionStart';
}

function nativeSessionId(agent: Agent, payload: unknown): string {
  if (payload === null || typeof payload !== 'object') return '';
  const row = payload as Record<string, unknown>;
  if (agent === 'grok') {
    return typeof row.sessionId === 'string' ? row.sessionId : String(row.session_id ?? '');
  }
  return typeof row.session_id === 'string' ? row.session_id : '';
}

function packText(stdout: string): string {
  const trimmed = stdout.trim();
  if (trimmed.startsWith('{')) {
    try {
      const json = JSON.parse(trimmed) as {
        hookSpecificOutput?: { additionalContext?: unknown };
      };
      const context = json.hookSpecificOutput?.additionalContext;
      if (typeof context === 'string') return context;
    } catch {
      // The hook printed a non-pack JSON document; treat the whole stdout as the pack surface.
    }
  }
  return stdout;
}

function replayEnv(home: string, extra: NodeJS.ProcessEnv = {}): NodeJS.ProcessEnv {
  const env = childEnvironment(process.env);
  delete env.OBOETE_TEST_FAULT;
  delete env.OBOETE_TEST_FAULT_URL;
  delete env.GROK_HOOK_EVENT;
  delete env.GROK_SESSION_ID;
  delete env.NODE_USE_ENV_PROXY;
  env.NODE_ENV = 'test';
  env.OBOETE_HOME = home;
  return { ...env, ...extra };
}

function hookArgs(line: Line): { args: string[]; extra: NodeJS.ProcessEnv } {
  if (line.agent === 'pi') {
    return {
      args: ['capture', '--agent', 'pi', '--event', line.event, '--invocation', `t068-${line.seq}`],
      extra: {},
    };
  }
  if (line.agent === 'codex') {
    return { args: ['hook', '--agent', 'codex', '--event', line.event], extra: {} };
  }
  const extra: NodeJS.ProcessEnv = {};
  if (line.agent === 'grok') {
    extra.GROK_HOOK_EVENT = line.event;
    extra.GROK_SESSION_ID = nativeSessionId('grok', line.payload);
  }
  return { args: ['hook', '--agent', 'claude-or-grok', '--event', line.event], extra };
}

function firstStderrLine(text: string): string {
  const line = text.replaceAll('\r', '').split('\n').find((entry) => entry.trim() !== '') ?? '';
  return line.length > 120 ? `${line.slice(0, 117)}...` : line;
}

function hookStatusCell(spawned: Spawned): string {
  if (spawned.timedOut) return 'timeout';
  if (spawned.signal !== null) return spawned.signal;
  return spawned.status === null ? 'null' : String(spawned.status);
}

function hookViolated(spawned: Spawned): boolean {
  return spawned.timedOut || spawned.signal !== null || spawned.status !== 0;
}

function runChild(
  bundle: string,
  args: string[],
  input: string,
  cwd: string,
  env: NodeJS.ProcessEnv,
  timeoutMs: number,
): Promise<Spawned> {
  const started = performance.now();
  // T068 / SC-003: keep the event loop free for RSS polls while hooks are executing.
  return new Promise((resolvePromise) => {
    const child = execFile(process.execPath, [bundle, ...args], {
      cwd,
      encoding: 'utf8',
      env,
      timeout: timeoutMs,
      maxBuffer: 16 * 1024 * 1024,
      killSignal: 'SIGTERM',
    }, (error, stdout, stderr) => {
      resolvePromise({
        status: child.exitCode,
        signal: child.signalCode,
        timedOut: child.killed && error?.code !== 'ERR_CHILD_PROCESS_STDIO_MAXBUFFER',
        stdout,
        stderr,
        elapsedMs: performance.now() - started,
      });
    });
    // T068 / FR-002: a bounded-input hook can exit before consuming all of a size-tagged payload.
    child.stdin?.on('error', () => {});
    child.stdin?.end(input);
  });
}

function readVmHwm(pid: number): number {
  try {
    const text = readFileSync(`/proc/${pid}/status`, 'utf8');
    const line = text.split('\n').find((entry) => entry.startsWith('VmHWM:'));
    if (line === undefined) return 0;
    const kb = Number(/(\d+)/.exec(line)?.[1] ?? '');
    return Number.isFinite(kb) ? kb : 0;
  } catch {
    return 0;
  }
}

type ObserveProc = {
  pid: number | undefined;
  rssKb: () => number;
  running: () => boolean;
  status: () => number | null;
  exited: Promise<number | null>;
};

function startObserve(bundle: string, cwd: string, env: NodeJS.ProcessEnv): ObserveProc {
  const child = spawn(process.execPath, [bundle, 'observe'], { cwd, env, stdio: 'ignore' });
  let rssKb = 0;
  let running = true;
  let status: number | null = null;
  const tick = (): void => {
    if (child.pid !== undefined) {
      const value = readVmHwm(child.pid);
      if (value > rssKb) rssKb = value;
    }
  };
  const timer = setInterval(tick, 50);
  const exited = new Promise<number | null>((resolvePromise) => {
    const finish = (code: number | null): void => {
      tick();
      clearInterval(timer);
      running = false;
      status = code;
      resolvePromise(code);
    };
    child.on('error', () => finish(null));
    child.on('close', (code) => finish(code));
  });
  return {
    pid: child.pid,
    rssKb: () => rssKb,
    running: () => running,
    status: () => status,
    exited,
  };
}

async function waitLeaseFree(dbPath: string, timeoutMs: number): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    try {
      const opened = openDatabase({ path: dbPath, timeoutMs: 200, hook: true });
      try {
        if (isLeaseFree(opened.db, Date.now())) return;
      } finally {
        opened.db.close();
      }
    } catch {
      // The hook still holds the write lock, or the worker has not released it.
    }
    await sleep(50);
  }
}

function runLeaseSql(dbPath: string, sql: string, params: (string | number | null)[]): void {
  const deadline = Date.now() + 5_000;
  let lastError: unknown;
  while (Date.now() < deadline) {
    try {
      const opened = openDatabase({ path: dbPath, timeoutMs: 500, hook: true });
      try {
        opened.db.prepare(sql).run(...params);
        return;
      } finally {
        opened.db.close();
      }
    } catch (error) {
      lastError = error;
    }
  }
  throw lastError instanceof Error ? lastError : new Error('lease update failed');
}

function holdLease(dbPath: string): void {
  const now = Date.now();
  runLeaseSql(
    dbPath,
    'UPDATE worker_lease SET owner_token = ?, pid = ?, started_at = ?, heartbeat_at = ? WHERE id = 1',
    [LEASE_TOKEN, process.pid, now, now],
  );
}

function releaseHeldLease(dbPath: string): void {
  runLeaseSql(
    dbPath,
    'UPDATE worker_lease SET owner_token = NULL, pid = NULL, started_at = NULL, heartbeat_at = NULL WHERE id = 1',
    [],
  );
}

function endedPendingCount(dbPath: string): number | null {
  try {
    const opened = openDatabase({ path: dbPath, timeoutMs: 200, hook: true });
    try {
      const row = opened.db
        .prepare(
          `SELECT COUNT(*) AS n FROM sessions WHERE status = 'ended' AND summary_state = 'pending'`,
        )
        .get() as { n?: unknown } | undefined;
      return typeof row?.n === 'number' ? row.n : 0;
    } finally {
      opened.db.close();
    }
  } catch {
    return null;
  }
}

function startInjectionCount(dbPath: string, agent: Agent, nativeId: string): number {
  try {
    const opened = openDatabase({ path: dbPath, timeoutMs: 2_000, hook: true });
    try {
      const row = opened.db
        .prepare(
          `SELECT COUNT(*) AS n FROM injections i
           JOIN sessions s ON s.id = i.session_id
           WHERE s.agent = ? AND s.native_session_id = ?
             AND i.kind IN ('session_start', 'grok_deferred') AND i.state <> 'omitted'`,
        )
        .get(agent, nativeId) as { n?: unknown } | undefined;
      return typeof row?.n === 'number' ? row.n : 0;
    } finally {
      opened.db.close();
    }
  } catch {
    return -1;
  }
}

function fileBytes(path: string): number {
  try {
    return statSync(path).size;
  } catch {
    return 0;
  }
}

function walkFiles(root: string): string[] {
  if (!existsSync(root)) return [];
  try {
    return readdirSync(root, { recursive: true, withFileTypes: true })
      .filter((entry) => entry.isFile())
      .map((entry) => join(entry.parentPath, entry.name));
  } catch {
    return [];
  }
}

function bufferHas(haystack: Buffer, needle: string): boolean {
  if (needle === '') return false;
  return haystack.includes(Buffer.from(needle, 'utf8'));
}

function percentile(values: number[], p: number): number {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  const index = (p / 100) * (sorted.length - 1);
  const lo = Math.floor(index);
  const hi = Math.ceil(index);
  const left = sorted[lo];
  const right = sorted[hi];
  if (left === undefined) return 0;
  if (right === undefined || lo === hi) return left;
  return left + (right - left) * (index - lo);
}

function ms(value: number): string {
  return value.toFixed(1);
}

function loadAverage(): string {
  try {
    return readFileSync('/proc/loadavg', 'utf8').trim();
  } catch {
    return 'unavailable';
  }
}

function gitHead(cwd: string): string {
  const result = spawnSync('git', ['-C', cwd, 'rev-parse', '--short', 'HEAD'], { encoding: 'utf8' });
  return result.status === 0 ? result.stdout.trim() : 'unknown';
}

function initRepo(repo: string): void {
  mkdirSync(repo, { recursive: true });
  const init = spawnSync('git', ['init', '--quiet'], { cwd: repo, encoding: 'utf8' });
  if (init.status !== 0) throw new Error(`git init failed: ${init.stderr}`);
  const commit = spawnSync(
    'git',
    [
      '-c',
      'user.email=oboete-replay@invalid',
      '-c',
      'user.name=oboete-replay',
      '-c',
      'commit.gpgsign=false',
      'commit',
      '--allow-empty',
      '-m',
      'init',
      '--quiet',
    ],
    { cwd: repo, encoding: 'utf8' },
  );
  if (commit.status !== 0) throw new Error(`git commit failed: ${commit.stderr}`);
}

function replaceSection(path: string, section: string): void {
  const original = readFileSync(path, 'utf8');
  const start = original.indexOf(HEADING);
  if (start === -1) throw new Error(`${path} is missing ${HEADING}`);
  const rest = original.slice(start);
  const next = rest.slice(HEADING.length).search(/\n## /);
  const prefix = original.slice(0, start);
  const suffix = next === -1 ? '' : rest.slice(HEADING.length + next);
  const body = section.endsWith('\n') ? section : `${section}\n`;
  writeFileSync(path, `${prefix}${body}${suffix}`);
}

function mdTable(headers: string[], right: boolean[], rows: string[][]): string {
  const head = `| ${headers.join(' | ')} |`;
  const rule = `|${right.map((isRight) => (isRight ? '---:' : '---')).join('|')}|`;
  const body = rows.map((row) => `| ${row.join(' | ')} |`).join('\n');
  return `${head}\n${rule}\n${body}`;
}

function mdCell(text: string): string {
  const trimmed = text.trim();
  if (trimmed === '') return '—';
  return trimmed.replaceAll('|', String.raw`\|`);
}

function groupKey(sample: Sample): string {
  return `${sample.agent}\t${sample.event}`;
}

function timingRows(
  samples: Sample[],
  boundFor: (sample: Sample) => number,
  requiredFraction = 0.99,
): { rows: string[][]; pass: boolean; worstGroup: string } {
  const groups = new Map<string, Sample[]>();
  for (const sample of samples) {
    const key = groupKey(sample);
    const list = groups.get(key) ?? [];
    list.push(sample);
    groups.set(key, list);
  }
  const rows: string[][] = [];
  let pass = samples.length > 0;
  let worstP99 = -1;
  let worstGroup = 'n/a';
  const rowOf = (labelAgent: string, labelEvent: string, group: Sample[]): string[] => {
    const values = group.map((sample) => sample.ms);
    const boundMs = Math.max(...group.map(boundFor));
    const p99 = percentile(values, 99);
    const under = group.filter((sample) => sample.ms <= boundFor(sample)).length;
    const passed = under / group.length >= requiredFraction && p99 <= boundMs;
    if (labelAgent !== 'all') {
      pass = pass && passed;
      if (p99 > worstP99) {
        worstP99 = p99;
        worstGroup = `${labelAgent}/${labelEvent} p99 ${ms(p99)} ms`;
      }
    }
    return [
      labelAgent,
      labelEvent,
      String(values.length),
      ms(percentile(values, 50)),
      ms(percentile(values, 95)),
      ms(p99),
      ms(Math.max(...values)),
      `${boundMs} ms`,
      statusOf(passed),
    ];
  };
  for (const [key, group] of [...groups.entries()].sort(([left], [right]) => compareCodeUnits(left, right))) {
    const [agent, event] = key.split('\t');
    rows.push(rowOf(agent ?? '', event ?? '', group));
  }
  if (samples.length > 0) rows.push(rowOf('all', '*', samples));
  return { rows, pass, worstGroup };
}

function countQuery(db: ReturnType<typeof openDatabase>['db'], sql: string): number {
  const row = db.prepare(sql).get() as { n?: unknown } | undefined;
  return typeof row?.n === 'number' ? row.n : 0;
}

function lastSessions(lines: Line[]): {
  lastStartSeq: Record<Agent, number>;
  holdFromSeq: Record<Agent, number>;
} {
  const lastStartSeq = { claude: 0, codex: 0, grok: 0, pi: 0 };
  const ends: Record<Agent, number[]> = { claude: [], codex: [], grok: [], pi: [] };
  const seen = new Set<string>();
  for (const line of lines) {
    const key = `${line.agent}:${line.session}`;
    if (!seen.has(key)) {
      if (!sessionStartEvent(line.agent, line.event)) {
        throw new Error(`fixture seq=${line.seq} event=${line.event}: first line of ${key} must be a session start`);
      }
      seen.add(key);
      lastStartSeq[line.agent] = line.seq;
    }
    if (SESSION_END.has(line.event)) ends[line.agent].push(line.seq);
  }
  const holdFromSeq = { claude: 0, codex: 0, grok: 0, pi: 0 };
  for (const agent of AGENTS) {
    holdFromSeq[agent] = ends[agent].findLast((seq) => seq < lastStartSeq[agent]) ?? 0;
  }
  return { lastStartSeq, holdFromSeq };
}

function skipObserve(line: Line, holdFromSeq: Record<Agent, number>): boolean {
  return SESSION_END.has(line.event) && line.seq === holdFromSeq[line.agent];
}

function parseLines(file: string): Line[] {
  const rows = loadJsonl(file).map((row, index) => asLine(row, index));
  for (let i = 0; i < rows.length; i += 1) {
    if (rows[i]?.seq !== i + 1) throw new Error(`fixture seq must be 1-based strict; line ${i + 1} has seq=${rows[i]?.seq}`);
  }
  return rows;
}

function corpus(root: string): {
  secrets: Map<string, string>;
  secretValues: { id: string; secret: string }[];
  negatives: { id: string; text: string }[];
  directives: string[];
} {
  const secrets = new Map<string, string>();
  const secretValues: { id: string; secret: string }[] = [];
  const negatives: { id: string; text: string }[] = [];
  for (const raw of loadJsonl(join(root, 'test/corpus/secrets.jsonl'))) {
    const row = raw as { id?: unknown; text?: unknown; secret?: unknown };
    if (typeof row.id !== 'string' || typeof row.text !== 'string') continue;
    secrets.set(row.id, row.text);
    if (typeof row.secret === 'string') secretValues.push({ id: row.id, secret: row.secret });
    else negatives.push({ id: row.id, text: row.text });
  }
  const directives = loadJsonl(join(root, 'test/corpus/directives.jsonl')).map((raw) => {
    const row = raw as { phrase?: unknown };
    if (typeof row.phrase !== 'string') throw new Error('directives.jsonl line missing phrase');
    return row.phrase;
  });
  return { secrets, secretValues, negatives, directives };
}

function statusOf(pass: boolean): 'pass' | 'fail' {
  return pass ? 'pass' : 'fail';
}

type ReplayPlan = {
  values: { out?: string; json?: boolean; home?: string; keep?: boolean };
  fixturePath: string;
  outPath: string | undefined;
  root: string;
  bundle: string;
  lines: Line[];
  sessionWindows: ReturnType<typeof lastSessions>;
};

/** `oboete fixture replay <file>` and its flags; a number is the exit code it stops with. */
function replayArgv(argv: string[]): { values: ReplayPlan['values']; fixture: string } | number {
  let parsed;
  try {
    parsed = parseArgs({
      args: argv,
      allowPositionals: true,
      strict: true,
      options: {
        out: { type: 'string' },
        json: { type: 'boolean' },
        home: { type: 'string' },
        keep: { type: 'boolean' },
      },
    });
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n${usage()}`);
    return 2;
  }
  const { values, positionals } = parsed;
  if (positionals[0] !== 'replay' || positionals[1] === undefined || positionals.length !== 2) {
    process.stderr.write(usage());
    return 2;
  }
  return { values, fixture: positionals[1] };
}

/** Reads argv and the files it names. A number is the exit code the replay stops with. */
function replayPlan(argv: string[]): ReplayPlan | number {
  const parsed = replayArgv(argv);
  if (typeof parsed === 'number') return parsed;
  const { values, fixture } = parsed;

  const fixturePath = resolve(fixture);
  if (!existsSync(fixturePath)) {
    process.stderr.write(`fixture file not found: ${fixturePath}\n`);
    return 2;
  }
  const outPath = values.out === undefined ? undefined : resolve(values.out);
  if (outPath !== undefined && !existsSync(outPath)) {
    process.stderr.write(`--out file not found: ${outPath}\n`);
    return 2;
  }

  let root: string;
  try {
    root = repositoryRoot();
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    return 2;
  }
  const bundle = bundlePath();
  if (!existsSync(bundle)) {
    process.stderr.write(`engine bundle not found: ${bundle}\n`);
    return 3;
  }

  let lines: Line[];
  let sessionWindows;
  try {
    lines = parseLines(fixturePath);
    sessionWindows = lastSessions(lines);
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    return 2;
  }
  return { values, fixturePath, outPath, root, bundle, lines, sessionWindows };
}

/**
 * `--home`, then `OBOETE_HOME`, then a fresh temporary directory. `createdHome` marks the last
 * case, the only one where the directory is this run's to remove: an `OBOETE_HOME` that is set but
 * empty also lands there, and used to leak the directory it made.
 */
export function replayHome(values: ReplayPlan['values']): { home: string; createdHome: boolean } {
  if (values.home !== undefined) return { home: resolve(values.home), createdHome: false };
  const fromEnv = process.env.OBOETE_HOME;
  if (fromEnv !== undefined && fromEnv !== '') {
    const home = isAbsolute(fromEnv) ? resolve(fromEnv) : resolve(process.cwd(), fromEnv);
    return { home, createdHome: false };
  }
  return { home: mkdtempSync(join(tmpdir(), 'oboete-t068-home-')), createdHome: true };
}

type ReplayRun = {
  bundle: string;
  repo: string;
  home: string;
  envBase: NodeJS.ProcessEnv;
  paths: ReturnType<typeof oboetePaths>;
  lines: Line[];
  maps: ReturnType<typeof corpus>;
  lastStartSeq: ReturnType<typeof lastSessions>['lastStartSeq'];
  holdFromSeq: ReturnType<typeof lastSessions>['holdFromSeq'];
  pendingHold: Set<Agent>;
  pendingSessions: Set<string>;
  captureSamples: Sample[];
  injectionSamples: Sample[];
  readySamples: Sample[];
  pendingSamples: Sample[];
  sizeRows: SizeRow[];
  packs: { seq: number; agent: Agent; session: string; event: string; text: string }[];
  sessionStartPack: Map<string, string>;
  factsById: Map<string, Fact>;
  recallHits: RecallHit[];
  grokRecallWait: { seq: number; session: string; fact: Fact }[];
  hookFailures: HookFailure[];
  resumeChecks: ResumeCheck[];
  hookCount: number;
  observeRssKb: number;
  observeRuns: number;
  hookWorkerRssKb: number;
  hookWorkerPids: Set<number>;
  observePids: Set<number>;
  storageFailed: boolean;
  leaseHeld: boolean;
  liveObserve: ObserveProc | undefined;
};

/** Every fact the fixture plants, indexed before the first hook so recall can look one up. */
function factsFrom(run: ReplayRun, line: Line): void {
  const fact = line.tags?.fact;
  if (fact !== undefined) run.factsById.set(fact.id, fact);
}

/** Everything a replay accumulates while it runs, empty before its first hook. */
function emptyTables(): Omit<
  ReplayRun,
  'bundle' | 'repo' | 'home' | 'envBase' | 'paths' | 'lines' | 'maps' | 'lastStartSeq' | 'holdFromSeq' | 'pendingSessions'
> {
  return {
    pendingHold: new Set<Agent>(),
    captureSamples: [],
    injectionSamples: [],
    readySamples: [],
    pendingSamples: [],
    sizeRows: [],
    packs: [],
    sessionStartPack: new Map<string, string>(),
    factsById: new Map<string, Fact>(),
    recallHits: [],
    grokRecallWait: [],
    hookFailures: [],
    resumeChecks: [],
    hookCount: 0,
    observeRssKb: 0,
    observeRuns: 0,
    hookWorkerRssKb: 0,
    hookWorkerPids: new Set<number>(),
    observePids: new Set<number>(),
    storageFailed: false,
    leaseHeld: false,
    liveObserve: undefined,
  };
}

/** The mutable state one replay carries from its first hook to its report. */
function createRun(input: {
  bundle: string;
  repo: string;
  home: string;
  envBase: NodeJS.ProcessEnv;
  paths: ReturnType<typeof oboetePaths>;
  lines: Line[];
  maps: ReturnType<typeof corpus>;
  sessionWindows: ReturnType<typeof lastSessions>;
}): ReplayRun {
  const { lastStartSeq, holdFromSeq } = input.sessionWindows;
  const pendingSessions = new Set<string>();
  for (const agent of AGENTS) {
    const start = input.lines.find((line) => line.agent === agent && line.seq === lastStartSeq[agent]);
    if (start !== undefined) pendingSessions.add(`${agent}:${start.session}`);
  }
  const run: ReplayRun = {
    bundle: input.bundle,
    repo: input.repo,
    home: input.home,
    envBase: input.envBase,
    paths: input.paths,
    lines: input.lines,
    maps: input.maps,
    lastStartSeq,
    holdFromSeq,
    pendingSessions,
    ...emptyTables(),
  };
  for (const line of input.lines) factsFrom(run, line);
  return run;
}

/** A pack the hook printed, kept for the recall and directive checks. */
function recordPack(run: ReplayRun, line: Line, stdout: string): void {
  const text = packText(stdout);
  if (text.trim() === '') return;
  run.packs.push({ seq: line.seq, agent: line.agent, session: line.session, event: line.event, text });
  const key = `${line.agent}:${line.session}`;
  if (
    sessionStartEvent(line.agent, line.event) ||
    (line.agent === 'grok' && line.event === 'PreToolUse' && !run.sessionStartPack.has(key))
  ) {
    run.sessionStartPack.set(key, text);
  }
}

/** Counts the hook and keeps the ones that broke the contract. */
function recordHook(run: ReplayRun, line: Line, spawned: Spawned, eventLabel: string): void {
  run.hookCount += 1;
  if (!hookViolated(spawned)) return;
  run.hookFailures.push({
    seq: line.seq,
    agent: line.agent,
    event: eventLabel,
    status: hookStatusCell(spawned),
    stderr: firstStderrLine(spawned.stderr),
  });
}

/** Did the pack this line asked for carry the fact the fixture planted earlier? */
function checkRecall(run: ReplayRun, line: Line, pack: string): void {
  const id = line.tags?.recall;
  if (id === undefined) return;
  const fact = run.factsById.get(id);
  if (fact === undefined) return;
  const start = run.sessionStartPack.get(`${line.agent}:${line.session}`) ?? '';
  const hit = pack.includes(fact.expect) || start.includes(fact.expect);
  run.recallHits.push({ id: fact.id, lang: fact.lang, query: fact.query, expect: fact.expect, hit });
}

/** The live observe run's high-water mark; SC-003 is measured over every run of it. */
function harvestRss(run: ReplayRun): void {
  if (run.liveObserve === undefined) return;
  const value = run.liveObserve.rssKb();
  if (value > run.observeRssKb) run.observeRssKb = value;
  if (run.liveObserve.status() === 3) run.storageFailed = true;
}

/** Replay owns the worker: one run at a time, started here rather than by the hook. */
function startWorker(run: ReplayRun): void {
  if (run.liveObserve?.running() === true) return;
  run.liveObserve = startObserve(run.bundle, run.repo, run.envBase);
  if (run.liveObserve.pid !== undefined) run.observePids.add(run.liveObserve.pid);
  run.observeRuns += 1;
  void run.liveObserve.exited.then(() => harvestRss(run));
}

/** Waits for every ended session to have a summary, restarting the worker while it waits. */
async function waitEndedSummaries(run: ReplayRun, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    harvestRss(run);
    const pending = endedPendingCount(run.paths.db);
    if (pending === 0) return;
    startWorker(run);
    await sleep(50);
  }
}

/** Holds the lease so the hook spawns no worker of its own. */
async function ensureLeaseHeld(run: ReplayRun): Promise<void> {
  if (run.leaseHeld) {
    holdLease(run.paths.db);
    return;
  }
  // A previous observe may still be looping on an active session's unbatched rows (up to 20 min).
  // Steal after a short wait so SessionEnd is not blocked on that idle loop.
  await waitLeaseFree(run.paths.db, 500);
  holdLease(run.paths.db);
  run.leaseHeld = true;
}

function dropLease(run: ReplayRun): void {
  if (!run.leaseHeld) return;
  releaseHeldLease(run.paths.db);
  run.leaseHeld = false;
}

/**
 * One fixture line: the hook, the Pi injection it also drives, and the checks its tags ask
 * for. `null` continues the replay; a number is the exit code the replay stops with.
 */
/**
 * The fixture's placeholders become this run's repository, secrets and directives. A size-tagged
 * line also asserts the byte count its tag promises, because the classification under test is the
 * one the byte count selects.
 */
function expandLine(run: ReplayRun, line: Line): { payload: unknown; fillSize: number } | number {
  try {
    if (line.tags?.size === undefined) {
      const payload = expandPayload(line.payload, {
        root: run.repo,
        secrets: run.maps.secrets,
        directives: run.maps.directives,
        fill: true,
      });
      return { payload, fillSize: 0 };
    }
    const filled = expandPayload(line.payload, { fill: true });
    const fillSize = Buffer.byteLength(JSON.stringify(filled));
    const tag = line.tags.size;
    const ok = tag === 'at_bound' ? fillSize === AT_BOUND : fillSize === ABOVE_ONE || fillSize === ABOVE_TWO;
    if (!ok) {
      const expected = tag === 'at_bound' ? String(AT_BOUND) : `${ABOVE_ONE} or ${ABOVE_TWO}`;
      process.stderr.write(
        `size tag ${tag} seq=${line.seq}: FILL-only JSON is ${fillSize} bytes, expected ${expected}\n`,
      );
      return 2;
    }
    return { payload: expandPayload(filled, { root: run.repo, fill: false }), fillSize };
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    return 2;
  }
}

/** The Pi extension injects in process, so replay drives `oboete inject` for the same events. */
function isPiInjectEvent(line: Line): boolean {
  return (
    line.agent === 'pi' &&
    (line.event === 'session_start' || line.event === 'input') &&
    line.tags?.size === undefined
  );
}

/** Runs `oboete inject` for one Pi line and files its pack and its sample like a hook's. */
/** The stdin of `oboete inject --agent pi`, from the fixture envelope. */
function piInjectInput(run: ReplayRun, payload: unknown): string {
  const envelope = payload as {
    cwd?: unknown;
    session_id?: unknown;
    model?: unknown;
    payload?: { text?: unknown };
  };
  return JSON.stringify({
    cwd: typeof envelope.cwd === 'string' ? envelope.cwd : run.repo,
    session_id: typeof envelope.session_id === 'string' ? envelope.session_id : nativeSessionId('pi', payload),
    prompt: typeof envelope.payload?.text === 'string' ? envelope.payload.text : undefined,
    model: typeof envelope.model === 'string' ? envelope.model : undefined,
  });
}

async function injectPiLine(
  run: ReplayRun,
  line: Line,
  payload: unknown,
  seen: { isPendingStart: boolean; holdActive: boolean },
): Promise<Spawned> {
  const kind = line.event === 'session_start' ? 'start' : 'prompt';
  const injectInput = piInjectInput(run, payload);
  const injected = await runChild(
    run.bundle,
    ['inject', '--agent', 'pi', '--kind', kind],
    injectInput,
    run.repo,
    run.envBase,
    kind === 'start' ? 15_000 : 10_000,
  );
  recordHook(run, line, injected, `inject:${kind}`);
  const injectSample: Sample = {
    agent: 'pi',
    event: line.event,
    seq: line.seq,
    session: line.session,
    ms: injected.elapsedMs,
  };
  recordPack(run, line, injected.stdout);
  if (line.event !== 'session_start') {
    run.injectionSamples.push(injectSample);
  } else if (seen.isPendingStart && seen.holdActive) {
    run.pendingSamples.push(injectSample);
  } else if (!seen.holdActive) {
    run.injectionSamples.push(injectSample);
    if (line.seq > 1) run.readySamples.push(injectSample);
  }
  return injected;
}

/** FR-025: a resumed session prints its pack again and opens no second injection. */
function recordResume(
  run: ReplayRun,
  line: Line,
  seen: { nativeId: string; resumeBefore: number; hooked: Spawned; injected: Spawned | undefined },
): void {
  const after = startInjectionCount(run.paths.db, line.agent, seen.nativeId);
  const printed =
    packText(seen.hooked.stdout).trim() !== '' ||
    (seen.injected !== undefined && packText(seen.injected.stdout).trim() !== '');
  const unreadable = after < 0 || seen.resumeBefore < 0;
  const injectionDelta = unreadable ? 1 : after - seen.resumeBefore;
  run.resumeChecks.push({
    seq: line.seq,
    agent: line.agent,
    session: line.session,
    packPrinted: printed,
    injectionDelta,
  });
}

/** Grok's pack arrives on the next tool call, so its recall checks are settled there. */
function settleGrokRecall(run: ReplayRun, line: Line, key: string, hooked: Spawned): void {
  if (line.agent !== 'grok' || line.event !== 'PreToolUse') return;
  const waiting = run.grokRecallWait.filter((item) => item.session === line.session);
  if (waiting.length === 0) return;
  const pack = packText(hooked.stdout);
  const start = run.sessionStartPack.get(key) ?? '';
  for (const item of waiting) {
    run.recallHits.push({
      id: item.fact.id,
      lang: item.fact.lang,
      query: item.fact.query,
      expect: item.fact.expect,
      hit: pack.includes(item.fact.expect) || start.includes(item.fact.expect),
    });
  }
  run.grokRecallWait = run.grokRecallWait.filter((item) => item.session !== line.session);
}

/** A line tagged with a fact id asks whether that fact came back in this turn's pack. */
function openRecall(run: ReplayRun, line: Line, hooked: Spawned): void {
  if (line.tags?.recall === undefined) return;
  if (line.agent === 'grok') {
    const fact = run.factsById.get(line.tags.recall);
    if (fact !== undefined) run.grokRecallWait.push({ seq: line.seq, session: line.session, fact });
    return;
  }
  const pack =
    run.packs
      .filter((entry) => entry.agent === line.agent && entry.session === line.session && entry.seq >= line.seq)
      .map((entry) => entry.text)
      .join('\n') || packText(hooked.stdout);
  checkRecall(run, line, pack);
}

/** How the engine classified the size-tagged event it just stored, read back for the size table. */
function lastClassification(run: ReplayRun): { classification: string; truncated: number } {
  try {
    const opened = openDatabase({ path: run.paths.db, timeoutMs: 2_000, hook: true });
    try {
      const row = opened.db
        .prepare(
          `SELECT classification_state AS classification_state, truncated AS truncated
           FROM raw_events ORDER BY captured_at DESC, id DESC LIMIT 1`,
        )
        .get() as { classification_state?: unknown; truncated?: unknown } | undefined;
      return {
        classification: typeof row?.classification_state === 'string' ? row.classification_state : 'missing',
        truncated: typeof row?.truncated === 'number' ? row.truncated : 0,
      };
    } finally {
      opened.db.close();
    }
  } catch {
    return { classification: 'unreadable', truncated: 0 };
  }
}

/** One row of the size table: the tag the fixture promised and what the engine made of it. */
function recordSize(run: ReplayRun, line: Line, tag: SizeRow['tag'], fillSize: number, hooked: Spawned): void {
  const { classification, truncated } = lastClassification(run);
  run.sizeRows.push({
    seq: line.seq,
    agent: line.agent,
    event: line.event,
    tag,
    fillBytes: fillSize,
    ms: hooked.elapsedMs,
    classification,
    truncated,
  });
}

/**
 * The worker runs after a session ends, unless the fixture holds that agent open so the next
 * session start finds a pending summary; the hold is released at that start.
 */
async function settleObserve(run: ReplayRun, line: Line): Promise<void> {
  if (SESSION_END.has(line.event)) {
    if (skipObserve(line, run.holdFromSeq)) run.pendingHold.add(line.agent);
    else if (run.pendingHold.size === 0) await observeNow(run);
  }
  if (line.seq === run.lastStartSeq[line.agent]) {
    run.pendingHold.delete(line.agent);
    if (run.pendingHold.size === 0) await observeNow(run);
  }
}

/** Spawns the engine bundle for one line and files its sample under the table it belongs to. */
async function runHookLine(
  run: ReplayRun,
  line: Line,
  payload: unknown,
  kind: { injection: boolean; isPendingStart: boolean },
): Promise<{ hooked: Spawned; holdActive: boolean }> {
  const input = JSON.stringify(payload);
  const { args, extra } = hookArgs({ ...line, payload });
  const env = replayEnv(run.home, extra);
  const timeoutMs = kind.injection ? 15_000 : 10_000;
  const hooked = await runChild(run.bundle, args, input, run.repo, env, timeoutMs);
  recordHook(run, line, hooked, line.event);
  const sample: Sample = {
    agent: line.agent,
    event: line.event,
    seq: line.seq,
    session: line.session,
    ms: hooked.elapsedMs,
  };
  const holdActive = run.pendingHold.size > 0;
  if (kind.injection && !(sessionStartEvent(line.agent, line.event) && holdActive)) {
    run.injectionSamples.push(sample);
  } else if (!kind.injection) {
    run.captureSamples.push(sample);
  }
  recordPack(run, line, hooked.stdout);

  if (sessionStartEvent(line.agent, line.event) && line.agent !== 'pi') {
    if (kind.isPendingStart && holdActive) run.pendingSamples.push(sample);
    else if (line.seq > 1 && !holdActive) run.readySamples.push(sample);
  }
  return { hooked, holdActive };
}

/** A session end, and every line while a hold is open, runs under replay's own lease. */
async function holdForLine(run: ReplayRun, line: Line): Promise<number | null> {
  if (!SESSION_END.has(line.event) && run.pendingHold.size === 0) return null;
  try {
    await ensureLeaseHeld(run);
    return null;
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    return 3;
  }
}

async function replayLine(run: ReplayRun, line: Line): Promise<number | null> {
  if (line.seq % 100 === 0) process.stderr.write(`replay ${line.seq}/${run.lines.length}\n`);
  const key = `${line.agent}:${line.session}`;
  const isPendingStart = run.pendingSessions.has(key) && sessionStartEvent(line.agent, line.event);
  const injection = isInjectionHook(line.agent, line.event);

  const expanded = expandLine(run, line);
  if (typeof expanded === 'number') return expanded;
  const { payload, fillSize } = expanded;

  const nativeId = nativeSessionId(line.agent, payload);
  let resumeBefore = 0;
  if (line.tags?.lifecycle === 'resume') {
    resumeBefore = startInjectionCount(run.paths.db, line.agent, nativeId);
  }

  const held = await holdForLine(run, line);
  if (held !== null) return held;

  const { hooked, holdActive } = await runHookLine(run, line, payload, { injection, isPendingStart });

  const injected = isPiInjectEvent(line)
    ? await injectPiLine(run, line, payload, { isPendingStart, holdActive })
    : undefined;

  if (line.tags?.lifecycle === 'resume') {
    recordResume(run, line, { nativeId, resumeBefore, hooked, injected });
  }
  settleGrokRecall(run, line, key, hooked);
  openRecall(run, line, hooked);

  if (line.tags?.size !== undefined) recordSize(run, line, line.tags.size, fillSize, hooked);
  await settleObserve(run, line);
  return null;
}

/** Runs the worker now and waits for the summaries the ended sessions are owed. */
async function observeNow(run: ReplayRun): Promise<void> {
  dropLease(run);
  startWorker(run);
  await waitEndedSummaries(run, 45_000);
}

/**
 * The last worker run: every ended session is owed a summary, and a run still going gets five more
 * seconds under a held lease so its high-water mark is measured before the report is written.
 */
async function settleWorker(run: ReplayRun): Promise<void> {
  dropLease(run);
  startWorker(run);
  await waitEndedSummaries(run, 60_000);
  harvestRss(run);
  if (run.liveObserve?.running() !== true) return;
  holdLease(run.paths.db);
  const stop = Date.now() + 5_000;
  while (Date.now() < stop && run.liveObserve.running()) await sleep(50);
  harvestRss(run);
  releaseHeldLease(run.paths.db);
}

/** The database is created once, before the first hook; the hook itself never migrates. */
function createDatabase(run: ReplayRun): number | null {
  try {
    openDatabase({ path: run.paths.db, timeoutMs: 5_000 }).db.close();
    return null;
  } catch (error) {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    return 3;
  }
}

/** Workers the hook spawned while the lease was free are found through the lease row's pid. */
function pollHookWorker(run: ReplayRun, workerPid: ReturnType<ReturnType<typeof openDatabase>['db']['prepare']>): void {
  try {
    const pid = workerPid.get()?.pid;
    if (typeof pid === 'number' && pid !== process.pid && !run.observePids.has(pid)) {
      run.hookWorkerPids.add(pid);
      run.hookWorkerRssKb = Math.max(run.hookWorkerRssKb, readVmHwm(pid));
    }
  } catch {
    // T068 / R6: a busy lease read must not interrupt replay; the next poll retries it.
  }
}

/** `--json` prints the machine form, `--out` replaces the evidence section, else stdout. */
function writeReport(
  values: ReplayPlan['values'],
  outPath: string | undefined,
  measured: { markdown: string; json: Record<string, unknown> },
): void {
  if (values.json === true) process.stdout.write(`${JSON.stringify(measured.json, null, 2)}\n`);
  if (outPath !== undefined) replaceSection(outPath, measured.markdown);
  else if (values.json !== true) process.stdout.write(`${measured.markdown}\n`);
}

/** The repository is this run's own; the home is removed only when this run created it. */
function cleanupReplay(input: { home: string; repo: string; keep: boolean; createdHome: boolean }): void {
  if (input.keep) {
    process.stderr.write(`kept home=${input.home} repo=${input.repo}\n`);
    return;
  }
  rmSync(input.repo, { recursive: true, force: true });
  if (input.createdHome) rmSync(input.home, { recursive: true, force: true });
}

/** Reads the finished run out of its own database and renders the evidence section. */
function measureRun(
  run: ReplayRun,
  input: { dbBytesBefore: number; fixturePath: string; startedAt: string; loadAtStart: string },
): ReturnType<typeof measure> {
  const opened = openDatabase({ path: run.paths.db, timeoutMs: 5_000 });
  try {
    return measure(opened, run.paths, {
      ...run,
      ...input,
      hookWorkerRuns: run.hookWorkerPids.size,
    });
  } finally {
    opened.db.close();
  }
}

export async function runFixture(argv: string[]): Promise<number> {
  const plan = replayPlan(argv);
  if (typeof plan === 'number') return plan;
  const { values, fixturePath, outPath, root, bundle, lines, sessionWindows } = plan;

  const maps = corpus(root);
  const { home, createdHome } = replayHome(values);
  const repo = mkdtempSync(join(tmpdir(), 'oboete-t068-repo-'));
  const keep = values.keep === true;
  const paths = oboetePaths(home);
  const envBase = replayEnv(home);
  const startedAt = new Date().toISOString();
  const loadAtStart = loadAverage();
  const run = createRun({ bundle, repo, home, envBase, paths, lines, maps, sessionWindows });
  let workerPollDb: ReturnType<typeof openDatabase>['db'] | undefined;
  let workerPoll: ReturnType<typeof setInterval> | undefined;
  try {
    initRepo(run.repo);
    mkdirSync(home, { recursive: true, mode: 0o700 });
    ensureDirectories(run.paths);
    const created = createDatabase(run);
    if (created !== null) return created;
    const dbBytesBefore = fileBytes(run.paths.db) + fileBytes(`${run.paths.db}-wal`);
    workerPollDb = openDatabase({ path: run.paths.db, timeoutMs: 0, hook: true }).db;
    const workerPid = workerPollDb.prepare('SELECT pid FROM worker_lease WHERE id = 1');
    workerPoll = setInterval(() => pollHookWorker(run, workerPid), 50);
    return await driveRun(run, workerPoll, {
      values,
      outPath,
      fixturePath,
      startedAt,
      loadAtStart,
      dbBytesBefore,
    });
  } finally {
    clearInterval(workerPoll);
    workerPollDb?.close();
    cleanupReplay({ home, repo: run.repo, keep, createdHome });
  }
}

/** The fixture lines, the worker settle, and the report. The caller owns the poll handles. */
async function driveRun(
  run: ReplayRun,
  workerPoll: ReturnType<typeof setInterval>,
  ctx: {
    values: ReplayPlan['values'];
    outPath: string | undefined;
    fixturePath: string;
    startedAt: string;
    loadAtStart: ReturnType<typeof loadAverage>;
    dbBytesBefore: number;
  },
): Promise<number> {
  const { values, outPath, fixturePath, startedAt, loadAtStart, dbBytesBefore } = ctx;

  for (const line of run.lines) {
    const exit = await replayLine(run, line);
    if (exit !== null) return exit;
  }

  await settleWorker(run);
  clearInterval(workerPoll);

  if (run.storageFailed) {
    process.stderr.write('observe reported unusable storage\n');
    return 3;
  }

  const measured = measureRun(run, { dbBytesBefore, fixturePath, startedAt, loadAtStart });
  writeReport(values, outPath, measured);
  return measured.failed ? 1 : 0;
}

function sessionOrder(lines: Line[]): Record<Agent, string[]> {
  const order: Record<Agent, string[]> = { claude: [], codex: [], grok: [], pi: [] };
  const seen = new Set<string>();
  for (const line of lines) {
    const key = `${line.agent}:${line.session}`;
    if (seen.has(key)) continue;
    seen.add(key);
    order[line.agent].push(line.session);
  }
  return order;
}

function neighborSession(order: string[], label: string, offset: number): string | undefined {
  const index = order.indexOf(label);
  if (index < 0) return undefined;
  return order[index + offset];
}

/** The share of samples inside a bound, and the p99 the report prints beside it. */
function shareUnder(values: number[], bound: number): { under: number; p99: number } {
  const under = values.length === 0 ? 1 : values.filter((value) => value <= bound).length / values.length;
  return { under, p99: percentile(values, 99) };
}

function maxMs(samples: Sample[]): number {
  return samples.length === 0 ? 0 : Math.max(...samples.map((sample) => sample.ms));
}

/** A2: a start taken while the previous summary was pending must say so in its own pack. */
function pendingSentence(
  input: { packs: { seq: number; text: string }[]; sessionStartPack: Map<string, string> },
  samples: Sample[],
): { hits: number; text: string } {
  const hits = samples.filter((sample) => {
    const pack =
      input.packs.find((entry) => entry.seq === sample.seq)?.text ??
      input.sessionStartPack.get(`${sample.agent}:${sample.session}`) ??
      '';
    return pack.includes(SUMMARY_PENDING);
  }).length;
  return { hits, text: `${hits}/${samples.length} packs carry summary_pending` };
}

/** Every byte this run wrote anywhere, so a leaked secret is found wherever it landed. */
function writtenSurfaces(paths: ReturnType<typeof oboetePaths>, packBlob: string): Buffer[] {
  const dbBuffers = [paths.db, `${paths.db}-wal`, `${paths.db}-shm`]
    .filter((path) => existsSync(path))
    .map((path) => readFileSync(path));
  const extraFiles = [...walkFiles(paths.spool), ...walkFiles(paths.logs)].map((path) => readFileSync(path));
  return [...dbBuffers, ...extraFiles, Buffer.from(packBlob, 'utf8')];
}

/** SC-005 and FR-021: no planted secret reaches a written surface, no directive reaches a memory. */
function privacyChecks(
  db: ReturnType<typeof openDatabase>['db'],
  paths: ReturnType<typeof oboetePaths>,
  input: { maps: ReturnType<typeof corpus>; packBlob: string },
): { leakedSecrets: string[]; leakedDirectives: string[]; negativesUnredacted: number; rawDirectiveRows: number } {
  const surfaces = writtenSurfaces(paths, input.packBlob);
  const leakedSecrets = input.maps.secretValues
    .filter((row) => surfaces.some((buffer) => bufferHas(buffer, row.secret)))
    .map((row) => row.id);

  const memoryRows = db.prepare('SELECT title AS title, body AS body FROM memories').all() as {
    title: unknown;
    body: unknown;
  }[];
  const memoryText = memoryRows
    .map((row) => `${typeof row.title === 'string' ? row.title : ''}\n${typeof row.body === 'string' ? row.body : ''}`)
    .join('\n');
  const negativesUnredacted = input.maps.negatives.filter((row) => memoryText.includes(row.text)).length;
  const leakedDirectives = input.maps.directives.filter(
    (phrase) => memoryText.includes(phrase) || input.packBlob.includes(phrase),
  );

  const rawContents = db.prepare('SELECT content AS content FROM raw_events WHERE content IS NOT NULL').all() as {
    content: unknown;
  }[];
  const rawDirectiveRows = rawContents.filter(
    (row) => typeof row.content === 'string' && input.maps.directives.some((phrase) => String(row.content).includes(phrase)),
  ).length;

  return { leakedSecrets, leakedDirectives, negativesUnredacted, rawDirectiveRows };
}

type LifeRow = { check: string; n: number; pass: boolean; offenders: string[] };
type LifeResult = { session: string; pass: boolean };
type ConversationOf = (agent: Agent, label: string) => { native: string; conversationId: string; epoch: number } | undefined;

/** One lifecycle row of the report: how many sessions were checked and which ones failed. */
function life(check: string, results: LifeResult[]): LifeRow {
  return {
    check,
    n: results.length,
    pass: results.length > 0 && results.every((row) => row.pass),
    offenders: results.filter((row) => !row.pass).map((row) => row.session),
  };
}

/** Maps a fixture's session label to the conversation the engine actually opened for it. */
function conversationLookup(db: ReturnType<typeof openDatabase>['db'], lines: Line[]): ConversationOf {
  const dbSessions = db
    .prepare(
      `SELECT id AS id, agent AS agent, native_session_id AS native_session_id,
              conversation_id AS conversation_id, context_epoch AS context_epoch
       FROM sessions`,
    )
    .all() as {
    id: unknown;
    agent: unknown;
    native_session_id: unknown;
    conversation_id: unknown;
    context_epoch: unknown;
  }[];
  const sessionByNative = new Map<string, (typeof dbSessions)[number]>();
  const sessionById = new Map<string, (typeof dbSessions)[number]>();
  for (const row of dbSessions) {
    sessionByNative.set(`${String(row.agent)}\t${String(row.native_session_id)}`, row);
    sessionById.set(String(row.id), row);
  }
  const nativeByLabel = new Map<string, string>();
  for (const line of lines) {
    const key = `${line.agent}:${line.session}`;
    if (!nativeByLabel.has(key)) nativeByLabel.set(key, nativeSessionId(line.agent, line.payload));
  }
  return (agent, label) => {
    const native = nativeByLabel.get(`${agent}:${label}`);
    if (native === undefined) return undefined;
    const row = sessionByNative.get(`${agent}\t${native}`);
    if (row === undefined) return undefined;
    const root = sessionById.get(String(row.conversation_id)) ?? row;
    return { native, conversationId: String(row.conversation_id), epoch: Number(root.context_epoch ?? 0) };
  };
}

/** A fork and a clear both open a new conversation; only a clear also opens a new native session. */
function branchResult(
  line: Line,
  order: string[],
  conversationOf: ConversationOf,
  tag: 'fork' | 'clear',
): LifeResult {
  const start = sessionStartEvent(line.agent, line.event);
  const subject = start ? line.session : neighborSession(order, line.session, 1);
  const parent = start ? neighborSession(order, line.session, -1) : line.session;
  const left = subject === undefined ? undefined : conversationOf(line.agent, subject);
  const right = parent === undefined ? undefined : conversationOf(line.agent, parent);
  const pass =
    left !== undefined &&
    right !== undefined &&
    left.conversationId !== right.conversationId &&
    (tag === 'fork' || left.native !== right.native);
  return { session: `${line.agent}:${subject ?? line.session}`, pass };
}

/** A compaction adds one epoch and one classified summary row to the same conversation. */
function compactResult(
  db: ReturnType<typeof openDatabase>['db'],
  line: Line,
  conversationOf: ConversationOf,
): LifeResult {
  const conv = conversationOf(line.agent, line.session);
  const clean =
    conv === undefined
      ? -1
      : (
          db
            .prepare(
              `SELECT COUNT(*) AS n FROM raw_events e
               JOIN sessions s ON s.id = e.session_id
               WHERE e.kind = 'compaction_summary' AND s.conversation_id = ?
                 AND e.classification_state = 'done'`,
            )
            .get(conv.conversationId) as { n?: unknown }
        ).n;
  const count = typeof clean === 'number' ? clean : -1;
  return {
    session: `${line.agent}:${line.session} epoch=${conv?.epoch ?? 'missing'} rows=${count}`,
    pass: conv !== undefined && count >= 1 && conv.epoch === count,
  };
}

/** Every line the fixture tagged with a lifecycle event, grouped by the check it feeds. */
function lifecycleTags(
  db: ReturnType<typeof openDatabase>['db'],
  lines: Line[],
  conversationOf: ConversationOf,
): { fork: LifeResult[]; clear: LifeResult[]; compact: LifeResult[] } {
  const order = sessionOrder(lines);
  const tagged = { fork: [] as LifeResult[], clear: [] as LifeResult[], compact: [] as LifeResult[] };
  for (const line of lines) {
    const tag = line.tags?.lifecycle;
    if (tag === 'fork' || tag === 'clear') {
      tagged[tag].push(branchResult(line, order[line.agent], conversationOf, tag));
    } else if (tag === 'compact') {
      tagged.compact.push(compactResult(db, line, conversationOf));
    }
  }
  return tagged;
}

/** The share of recall probes whose fact came back; an empty set counts as a pass. */
function recallRateOf(rows: RecallHit[]): number {
  return rows.length === 0 ? 1 : rows.filter((row) => row.hit).length / rows.length;
}

type MeasureInput = {
    lines: Line[];
    captureSamples: Sample[];
    injectionSamples: Sample[];
    readySamples: Sample[];
    pendingSamples: Sample[];
    sizeRows: SizeRow[];
    packs: { seq: number; agent: Agent; session: string; event: string; text: string }[];
    sessionStartPack: Map<string, string>;
    recallHits: RecallHit[];
    grokRecallWait: { seq: number; session: string; fact: Fact }[];
    hookFailures: HookFailure[];
    hookCount: number;
    resumeChecks: ResumeCheck[];
    maps: ReturnType<typeof corpus>;
    observeRssKb: number;
    observeRuns: number;
    hookWorkerRssKb: number;
    hookWorkerRuns: number;
    dbBytesBefore: number;
    home: string;
    fixturePath: string;
    bundle: string;
    startedAt: string;
    loadAtStart: string;
};

/** SC-002, SC-003, SC-005, SC-009, SC-010, lifecycle and directives, as one evidence section. */
function measure(
  opened: ReturnType<typeof openDatabase>,
  paths: ReturnType<typeof oboetePaths>,
  input: MeasureInput,
): { markdown: string; json: Record<string, unknown>; failed: boolean } {
  return renderReport(input, computeReport(opened, paths, input));
}

/** The row counts and database growth the report states, read once. */
function dbCounts(
  db: ReturnType<typeof openDatabase>['db'],
  paths: ReturnType<typeof oboetePaths>,
  input: MeasureInput,
) {
  const dbBytesAfter = fileBytes(paths.db) + fileBytes(`${paths.db}-wal`);
  const perThousand = input.lines.length === 0 ? 0 : (dbBytesAfter - input.dbBytesBefore) * (1000 / input.lines.length);
  const rawEvents = countQuery(db, 'SELECT COUNT(*) AS n FROM raw_events');
  const memories = countQuery(db, 'SELECT COUNT(*) AS n FROM memories');
  const injections = countQuery(db, 'SELECT COUNT(*) AS n FROM injections');
  const injectionItems = countQuery(db, 'SELECT COUNT(*) AS n FROM injection_items');
  const duplicateGroups = db
    .prepare(
      `SELECT conversation_id AS conversation_id, context_epoch AS context_epoch, memory_id AS memory_id, COUNT(*) AS n
       FROM injection_items
       WHERE decision = 'included' AND memory_id IS NOT NULL
       GROUP BY conversation_id, context_epoch, memory_id
       HAVING n > 1`,
    )
    .all() as { conversation_id: unknown; context_epoch: unknown; memory_id: unknown; n: unknown }[];

  return { dbBytesAfter, perThousand, rawEvents, memories, injections, injectionItems, duplicateGroups };
}

/** FR-025 and contracts/agents.md: fork, resume, compaction and clear each keep their shape. */
function lifecycleReport(
  db: ReturnType<typeof openDatabase>['db'],
  input: MeasureInput,
): { lifecycleRows: LifeRow[]; lifecyclePass: boolean } {
  const conversationOf = conversationLookup(db, input.lines);
  const tagged = lifecycleTags(db, input.lines, conversationOf);
  const resumeLife = life(
    'resume',
    input.resumeChecks.map((row) => ({
      session: `${row.agent}:${row.session}`,
      pass: !row.packPrinted && row.injectionDelta === 0,
    })),
  );
  const lifecycleRows = [
    life('fork', tagged.fork),
    resumeLife,
    life('compact', tagged.compact),
    life('clear', tagged.clear),
  ];
  const lifecyclePass = lifecycleRows.every((row) => row.pass);
  return { lifecycleRows, lifecyclePass };
}

/** SC-002 and the injection and session-start bounds, from the samples this run took. */
function timingReport(input: MeasureInput) {
  const captureValues = input.captureSamples.map((sample) => sample.ms);
  const { under: captureUnder, p99: captureP99 } = shareUnder(captureValues, CAPTURE_DEADLINE_MS);
  const sc002 = captureValues.length > 0 && captureUnder >= 0.99 && captureP99 <= CAPTURE_DEADLINE_MS;
  const injectionValues = input.injectionSamples.map((sample) => sample.ms);
  const { under: injectionUnder, p99: injectionP99 } = shareUnder(injectionValues, READY_BOUND_MS);
  const injectionTiming = timingRows(input.injectionSamples, () => READY_BOUND_MS, 1);
  const injectionPass = injectionTiming.pass;
  const pending = pendingSentence(input, input.pendingSamples);
  const readyMax = maxMs(input.readySamples);
  const pendingMax = maxMs(input.pendingSamples);
  const readyPass = input.readySamples.every((sample) => sample.ms <= READY_BOUND_MS);
  const pendingPass =
    input.pendingSamples.length > 0 &&
    pending.hits === input.pendingSamples.length &&
    input.pendingSamples.every((sample) => sample.ms <= PENDING_BOUND_MS);
  return {
    captureValues,
    captureUnder,
    captureP99,
    sc002,
    injectionValues,
    injectionUnder,
    injectionP99,
    injectionTiming,
    injectionPass,
    pending,
    readyMax,
    pendingMax,
    readyPass,
    pendingPass,
  };
}

/** Everything the report states about this run, read out of the run's own database. */
function computeReport(
  opened: ReturnType<typeof openDatabase>,
  paths: ReturnType<typeof oboetePaths>,
  input: MeasureInput,
) {
  const { db } = opened;
  const counts = dbCounts(db, paths, input);
  const packBlob = input.packs.map((pack) => pack.text).join('\n');
  const privacy = privacyChecks(db, paths, { maps: input.maps, packBlob });
  const recall = recallTally(input);
  const lifecycle = lifecycleReport(db, input);
  const compactionSummaries = compactionRows(db);
  const timing = timingReport(input);
  const workerRssKb = Math.max(input.observeRssKb, input.hookWorkerRssKb);
  const workerRuns = `observe runs: ${input.observeRuns} spawned by replay, ${input.hookWorkerRuns} hook-spawned (polled via worker_lease.pid)`;
  const verdicts = verdictsOf(input, { ...counts, ...privacy, ...recall, ...lifecycle, ...timing, workerRssKb });
  return {
    ...counts,
    ...privacy,
    ...recall,
    ...lifecycle,
    ...timing,
    ...verdicts,
    compactionSummaries,
    workerRssKb,
    workerRuns,
  };
}

/** Grok's last pending recall probes are settled against the session-start pack, then tallied. */
function recallTally(input: MeasureInput): { recallJa: RecallHit[]; recallEn: RecallHit[]; misses: RecallHit[] } {
  for (const waiting of input.grokRecallWait) {
    const start = input.sessionStartPack.get(`grok:${waiting.session}`) ?? '';
    input.recallHits.push({
      id: waiting.fact.id,
      lang: waiting.fact.lang,
      query: waiting.fact.query,
      expect: waiting.fact.expect,
      hit: start.includes(waiting.fact.expect),
    });
  }
  return {
    recallJa: input.recallHits.filter((row) => row.lang === 'ja'),
    recallEn: input.recallHits.filter((row) => row.lang === 'en'),
    misses: input.recallHits.filter((row) => !row.hit),
  };
}

/** Every compaction summary the worker classified, in the order it saw them. */
function compactionRows(db: ReturnType<typeof openDatabase>['db']) {
  return db
    .prepare(
      `SELECT s.agent AS agent, s.native_session_id AS native_session_id,
              e.classification_state AS classification_state
       FROM raw_events e
       JOIN sessions s ON s.id = e.session_id
       WHERE e.kind = 'compaction_summary'
       ORDER BY s.agent, e.captured_at, e.id`,
    )
    .all() as { agent: unknown; native_session_id: unknown; classification_state: unknown }[];
}

/** What the verdicts are computed from. */
type VerdictInput = {
  duplicateGroups: unknown[];
  leakedSecrets: string[];
  leakedDirectives: string[];
  recallJa: RecallHit[];
  recallEn: RecallHit[];
  lifecyclePass: boolean;
  sc002: boolean;
  injectionPass: boolean;
  readyPass: boolean;
  pendingPass: boolean;
  workerRssKb: number;
};

/** Every printed pass or fail, and the exit code they add up to. */
function verdictsOf(input: MeasureInput, m: VerdictInput) {
  const sc003 = m.workerRssKb < WORKER_RSS_BOUND_KB;
  const sc005 = m.leakedSecrets.length === 0;
  const sc009 =
    recallRateOf(input.recallHits) >= RECALL_BOUND &&
    (m.recallJa.length === 0 || recallRateOf(m.recallJa) >= RECALL_BOUND) &&
    (m.recallEn.length === 0 || recallRateOf(m.recallEn) >= RECALL_BOUND);
  const sc010 = m.duplicateGroups.length === 0;
  const directivesPass = m.leakedDirectives.length === 0;
  const hooksPass = input.hookFailures.length === 0;
  const failed = !(
    m.sc002 &&
    m.injectionPass &&
    m.readyPass &&
    m.pendingPass &&
    sc003 &&
    sc005 &&
    sc009 &&
    sc010 &&
    m.lifecyclePass &&
    directivesPass &&
    hooksPass
  );
  return {
    sc003,
    sc005,
    sc009,
    sc010,
    directivesPass,
    leakedDirectivesEllipsis: m.leakedDirectives.length > 5 ? ' …' : '',
    hooksPass,
    failed,
  };
}

/** The timing rows of the SC table: capture, injection, session start, worker RSS. */
function timingBounds(
  input: MeasureInput,
  computed: ReturnType<typeof computeReport>,
): BoundRow[] {
  const { captureP99, captureUnder, captureValues, injectionP99, injectionPass, injectionTiming, injectionUnder, injectionValues, pending, pendingMax, pendingPass, perThousand, readyMax, readyPass, sc002, sc003, workerRssKb, workerRuns } = computed;
  return [
    {
      sc: 'SC-002',
      measured: `p99 ${ms(captureP99)} ms; ${(captureUnder * 100).toFixed(1)}% ≤ ${CAPTURE_DEADLINE_MS} ms (n=${captureValues.length})`,
      bound: `p99 ≤ ${CAPTURE_DEADLINE_MS} ms and ≥99% of capture events ≤ ${CAPTURE_DEADLINE_MS} ms`,
      status: statusOf(sc002),
    },
    {
      sc: 'injection',
      measured: `p99 ${ms(injectionP99)} ms; ${(injectionUnder * 100).toFixed(1)}% ≤ ${READY_BOUND_MS} ms (n=${injectionValues.length}); worst ${injectionTiming.worstGroup}`,
      bound: `every (agent, event) group passes: every injection hook ≤ ${READY_BOUND_MS} ms (previous summary ready)`,
      status: statusOf(injectionPass),
    },
    {
      sc: 'session start',
      measured: `ready max ${ms(readyMax)} ms (n=${input.readySamples.length}); pending max ${ms(pendingMax)} ms (n=${input.pendingSamples.length}), ${pending.text}`,
      bound: `ready ≤ ${READY_BOUND_MS} ms; pending n > 0, every pack carries summary_pending and every sample ≤ ${PENDING_BOUND_MS} ms (INJECTION_DEADLINE_MS: 300 ms budget + 1 s summary wait)`,
      status: statusOf(readyPass && pendingPass),
    },
    {
      sc: 'SC-003',
      measured: `max VmHWM ${workerRssKb} kB (${(workerRssKb / 1024).toFixed(1)} MB); ${workerRuns}; growth ${Math.round(perThousand)} bytes / 1,000 events`,
      bound: '< 150 MB worker peak RSS; growth recorded',
      status: statusOf(sc003),
    },
  ];
}

/** The content rows of the SC table: secrets, recall, duplicates, lifecycle, directives, hooks. */
function contentBounds(
  input: MeasureInput,
  computed: ReturnType<typeof computeReport>,
): BoundRow[] {
  return [...leakBounds(input, computed), ...sequenceBounds(input, computed)];
}

/** SC-005, SC-009 and SC-010: what leaked, what was recalled, what was duplicated. */
function leakBounds(input: MeasureInput, computed: ReturnType<typeof computeReport>): BoundRow[] {
  const { duplicateGroups, leakedSecrets, rawEvents, recallEn, recallJa, sc005, sc009, sc010 } = computed;
  return [
    {
      sc: 'SC-005',
      measured: leakedSecrets.length === 0 ? '0 secret ids in db/wal/spool/logs/packs' : `leaked ${leakedSecrets.join(', ')}`,
      bound: 'zero secret corpus values in db, wal, spool, logs, packs',
      status: statusOf(sc005),
    },
    {
      sc: 'SC-009',
      measured: `ja ${(recallRateOf(recallJa) * 100).toFixed(1)}% (${recallJa.filter((row) => row.hit).length}/${recallJa.length}); en ${(recallRateOf(recallEn) * 100).toFixed(1)}% (${recallEn.filter((row) => row.hit).length}/${recallEn.length}); overall ${(recallRateOf(input.recallHits) * 100).toFixed(1)}% (${input.recallHits.filter((row) => row.hit).length}/${input.recallHits.length})`,
      bound: '≥ 90% ja, en, and overall',
      status: statusOf(sc009),
    },
    {
      sc: 'SC-010',
      measured: `${duplicateGroups.length} duplicate included (conversation_id, context_epoch, memory_id) groups; raw_events.id=${rawEvents} vs lines piped=${input.lines.length}`,
      bound: 'zero duplicate included memories per (conversation, epoch)',
      status: statusOf(sc010),
    },
  ];
}

/** The lifecycle, directive and hook rows: sequences that must hold across the whole run. */
function sequenceBounds(
  input: MeasureInput,
  computed: ReturnType<typeof computeReport>,
): BoundRow[] {
  const { directivesPass, hooksPass, leakedDirectives, lifecyclePass, lifecycleRows } = computed;
  return [
    {
      sc: 'lifecycle',
      measured: lifecyclePass
        ? 'fork/resume/compact/clear all pass'
        : lifecycleRows
            .filter((row) => !row.pass)
            .map((row) => `${row.check}: ${row.offenders.length === 0 ? 'no tagged sequences' : row.offenders.join(', ')}`)
            .join('; '),
      bound: 'every tagged sequence matches contracts/agents.md',
      status: statusOf(lifecyclePass),
    },
    {
      sc: 'directives',
      measured: leakedDirectives.length === 0 ? '0 directive phrases in memories/packs' : `${leakedDirectives.length} directive phrases in memories/packs`,
      bound: 'zero corpus directive phrases in memories and packs (FR-021)',
      status: statusOf(directivesPass),
    },
    {
      sc: 'hooks',
      measured:
        input.hookFailures.length === 0
          ? `all ${input.hookCount} capture/injection hooks exited 0`
          : `${input.hookFailures.length} of ${input.hookCount} hooks non-zero, killed, or timed out`,
      bound: 'all hooks exit 0',
      status: statusOf(hooksPass),
    },
  ];
}

/** The capture, injection, session-start wait and size tables. */
function timingTables(
  input: MeasureInput,
  computed: ReturnType<typeof computeReport>,
) {
  const { injectionTiming, pending, pendingPass, readyPass } = computed;
  const captureTable = mdTable(
    ['Agent', 'Event', 'n', 'p50 ms', 'p95 ms', 'p99 ms', 'max ms', 'Bound', 'Status'],
    [false, false, true, true, true, true, true, false, false],
    timingRows(input.captureSamples, () => CAPTURE_DEADLINE_MS).rows,
  );
  const injectionTable = mdTable(
    ['Agent', 'Event', 'n', 'p50 ms', 'p95 ms', 'p99 ms', 'max ms', 'Bound', 'Status'],
    [false, false, true, true, true, true, true, false, false],
    injectionTiming.rows,
  );
  const waitRows: string[][] = [];
  const pushWait = (label: string, samples: Sample[], bound: number, sentence: string, status: string): void => {
    if (samples.length === 0) {
      waitRows.push(['all', label, '0', 'n/a', 'n/a', 'n/a', `${bound} ms`, sentence, status]);
      return;
    }
    const values = samples.map((sample) => sample.ms);
    waitRows.push([
      'all',
      label,
      String(values.length),
      ms(percentile(values, 50)),
      ms(percentile(values, 95)),
      ms(Math.max(...values)),
      `${bound} ms`,
      sentence,
      status,
    ]);
  };
  pushWait('ready', input.readySamples, READY_BOUND_MS, pendingSentence(input, input.readySamples).text,
    input.readySamples.length === 0 ? 'n/a' : statusOf(readyPass));
  pushWait('pending', input.pendingSamples, PENDING_BOUND_MS, pending.text, statusOf(pendingPass));
  const waitTable = mdTable(
    ['Agent', 'Path', 'n', 'p50 ms', 'p95 ms', 'max ms', 'Bound', 'summary_pending', 'Status'],
    [false, false, true, true, true, true, false, false, false],
    waitRows,
  );
  const sizeTable = mdTable(
    ['seq', 'Agent', 'Event', 'tag', 'FILL JSON bytes', 'wall ms', 'classification_state', 'truncated'],
    [true, false, false, false, true, true, false, true],
    input.sizeRows.map((row) => [
      String(row.seq),
      row.agent,
      row.event,
      row.tag,
      String(row.fillBytes),
      ms(row.ms),
      row.classification,
      String(row.truncated),
    ]),
  );
  return { captureTable, injectionTable, waitTable, sizeTable };
}

/** The recall misses, SC summary, hook exit, lifecycle and compaction tables. */
/** The recall misses and the SC verdict table. */
function recallTables(computed: ReturnType<typeof computeReport>, bounds: BoundRow[]) {
  const { misses } = computed;
  const missTable =
    misses.length === 0
      ? 'None.'
      : mdTable(
          ['fact id', 'lang', 'query'],
          [false, false, false],
          misses.map((row) => [row.id, row.lang, row.query]),
        );
  const scTable = mdTable(
    ['SC', 'Measured', 'Bound', 'Status'],
    [false, false, false, false],
    bounds.map((row) => [row.sc, row.measured, row.bound, row.status]),
  );
  return { missTable, scTable };
}

/** The hook exits, the lifecycle checks, and the compaction summaries. */
function lifecycleTables(input: MeasureInput, computed: ReturnType<typeof computeReport>) {
  const { compactionSummaries, lifecycleRows } = computed;
  const hookExitTable =
    input.hookFailures.length === 0
      ? `All ${input.hookCount} capture and injection hooks exited 0 (none killed, none timed out).`
      : mdTable(
          ['seq', 'Agent', 'Event', 'status', 'stderr'],
          [true, false, false, false, false],
          input.hookFailures.map((row) => [
            String(row.seq),
            row.agent,
            row.event,
            row.status,
            mdCell(row.stderr),
          ]),
        );
  const lifecycleTable = mdTable(
    ['check', 'n', 'pass/fail', 'offending sessions'],
    [false, true, false, false],
    lifecycleRows.map((row) => [
      row.check,
      String(row.n),
      statusOf(row.pass),
      row.offenders.length === 0 ? '—' : row.offenders.join(', '),
    ]),
  );
  const compactSummaryTable =
    compactionSummaries.length === 0
      ? 'No `compaction_summary` rows.'
      : mdTable(
          ['agent', 'native_session_id', 'classification_state'],
          [false, false, false],
          compactionSummaries.map((row) => [
            String(row.agent),
            String(row.native_session_id),
            String(row.classification_state),
          ]),
        );
  return { hookExitTable, lifecycleTable, compactSummaryTable };
}

function findingTables(
  input: MeasureInput,
  computed: ReturnType<typeof computeReport>,
  bounds: BoundRow[],
) {
  return { ...recallTables(computed, bounds), ...lifecycleTables(input, computed) };
}

/** The heading, the machine this ran on, and the capture and injection tables. */
/** The heading and how this run was set up. */
function setupSection(input: MeasureInput, machine: string, cpu: string): string[] {
  return [
    HEADING,
    '',
    '### Setup',
    '',
    `- Date: ${input.startedAt}`,
    `- Machine: \`${machine}\`.`,
    `- CPU: \`${cpu}\`.`,
    `- Node: \`${process.execPath}\` (${process.version}).`,
    `- Commit: \`${gitHead(repositoryRoot())}\`.`,
    `- Bundle: \`${input.bundle}\`, ${fileBytes(input.bundle)} bytes.`,
    `- Fixture: \`${input.fixturePath}\` (${input.lines.length} lines).`,
    `- \`OBOETE_HOME\`: \`${input.home}\`. Config file absent (schema default preset \`workers-ai\`); child environment has no provider credentials, so summaries are rule-based (\`no_provider\`).`,
    `- Temporary git repository with one empty commit so \`HEAD\` exists. \`NODE_ENV=test\`.`,
    `- Worker RSS: Linux \`/proc/<pid>/status\` \`VmHWM\`, polled every 50 ms on replay's \`observe\` children and, from before the first hook through the final flush, hook-spawned workers found via \`worker_lease.pid\` using one read connection. The lease poll excludes replay's own pid and observe children; a busy read is skipped. A replay-owned \`worker_lease\` token is held across every \`SessionEnd\`/\`session_shutdown\` and the pending windows; hooks can still spawn workers while the lease is free.`,
    '',
    'Commands executed:',
    '',
    '```bash',
    'npm run build',
    `node dist/oboete.mjs fixture replay ${input.fixturePath}`,
    '```',
    '',
    `Load average at the start of the run: \`${input.loadAtStart}\``,
    '',
  ];
}

/** The capture, injection and session-start tables with the text that reads them. */
function hookTimingSection(
  input: MeasureInput,
  computed: ReturnType<typeof computeReport>,
  tables: { captureTable: string; injectionTable: string; waitTable: string; sizeTable: string },
): string[] {
  const { readyMax, readyPass, pendingMax, pendingPass } = computed;
  const { captureTable, injectionTable, waitTable, sizeTable } = tables;
  return [
    '### SC-002 capture time',
    '',
    `Capture-only hooks (\`hookDeadlineMs\` ≠ \`INJECTION_DEADLINE_MS\`). Bound ${CAPTURE_DEADLINE_MS} ms. Row status is informational: p99 ≤ bound and ≥99% of samples ≤ bound. SC-002 is judged on the pooled capture sample.`,
    '',
    captureTable,
    '',
    '### Injection hooks',
    '',
    `Classified by \`hookDeadlineMs(agent, event) === INJECTION_DEADLINE_MS\` (Claude/Codex \`SessionStart\`/\`UserPromptSubmit\`, Grok \`SessionStart\`/\`UserPromptSubmit\`/\`PreToolUse\`/\`PostToolUse\`, Pi \`inject\` for \`session_start\`/\`input\`). Every (agent, event) group must pass: every sample ≤ ${READY_BOUND_MS} ms, with no 99% allowance. The per-agent pending session-start sample is excluded here and reported only in the session-start table at ${PENDING_BOUND_MS} ms. Session-start events that ran while the lease was held for another agent's pending window are also omitted (they are not the ready path). Pi capture of those events stays in the capture table; the inject child is measured here.`,
    '',
    injectionTable,
    '',
    'Size-tagged events (FILL-only JSON byte length, then ROOT substituted). Stdin above the 256 KiB read bound is stored as `partial` / `truncated = 1`.',
    '',
    sizeTable,
    '',
    '### Session-start wait',
    '',
    `Ready path: previous session summarized (bound ${READY_BOUND_MS} ms). Pending path: one sample per agent whose hold window opens, with the lease kept held from that agent's last session end preceding its own last session start through that start (and Pi \`inject --kind start\`) so the hook cannot spawn a worker and the pack must take the pending path (bound ${PENDING_BOUND_MS} ms = the engine's INJECTION_DEADLINE_MS: the 300 ms ready budget plus the 1 s summary wait of FR-024; inject.ts caps the wait at the remaining budget). Passing requires at least one sample and the summary-pending sentence in every sample's pack. The lease hold is what makes the pending path deterministic.`,
    '',
    waitTable,
    '',
    `Ready max ${ms(readyMax)} ms (n=${input.readySamples.length}, ${statusOf(readyPass)}). Pending max ${ms(pendingMax)} ms (n=${input.pendingSamples.length}, ${statusOf(pendingPass)}).`,
    '',
  ];
}

/** Worker memory, database growth, and the secret, directive and duplicate scans. */
function resourceSection(
  input: MeasureInput,
  computed: ReturnType<typeof computeReport>,
): string[] {
  const { dbBytesAfter, duplicateGroups, injectionItems, injections, leakedDirectives, leakedDirectivesEllipsis, leakedSecrets, memories, negativesUnredacted, perThousand, rawDirectiveRows, rawEvents, sc003, workerRssKb, workerRuns } = computed;
  return [
    '### SC-003 worker memory and database growth',
    '',
    `- ${workerRuns}.`,
    `- Max VmHWM: ${workerRssKb} kB = ${(workerRssKb / 1024).toFixed(3)} MB (bound 150 MB, ${statusOf(sc003)}).`,
    `- \`memory.db\` + \`-wal\` before: ${input.dbBytesBefore} bytes; after: ${dbBytesAfter} bytes; delta ${dbBytesAfter - input.dbBytesBefore} bytes; ${Math.round(perThousand)} bytes per 1,000 events.`,
    `- Rows: raw_events=${rawEvents}, memories=${memories}, injections=${injections}, injection_items=${injectionItems}.`,
    '',
    '### SC-005 secret scan',
    '',
    leakedSecrets.length === 0
      ? `All ${input.maps.secretValues.length} non-null corpus secrets are absent from memory.db, memory.db-wal, spool/, logs/, and packs.`
      : `Leaked secret ids: ${leakedSecrets.join(', ')}.`,
    '',
    `Detector precision on the ${input.maps.negatives.length} \`secret = null\` negatives: ${negativesUnredacted} of their \`text\` values survived into \`memories\` unredacted (a redacted negative is a false positive, not a failure of this bound).`,
    '',
    '### Directive scan',
    '',
    leakedDirectives.length === 0
      ? `All ${input.maps.directives.length} directive phrases are absent from memories.title, memories.body, and packs.`
      : `Directive phrases in memories or packs (${leakedDirectives.length}): ${leakedDirectives.slice(0, 5).join(' | ')}${leakedDirectivesEllipsis}.`,
    '',
    `${rawDirectiveRows} raw_events.content rows still carry a directive phrase (allowed; they may remain in raw events and the spool).`,
    '',
    '### SC-010 duplicate injections',
    '',
    duplicateGroups.length === 0
      ? 'Zero `injection_items` rows with `decision = included` share the same `(conversation_id, context_epoch, memory_id)`.'
      : `${duplicateGroups.length} duplicate groups.`,
    '',
    `raw_events.id count ${rawEvents} vs lines piped ${input.lines.length}. Pi \`tool_result\` stores two kinds per line, so the id count can exceed the line count; a re-delivery would collapse onto an existing id.`,
    '',
  ];
}

/** Fact recall, the lifecycle checks, hook exits, and the bounds table. */
function recallSection(
  input: MeasureInput,
  computed: ReturnType<typeof computeReport>,
  tables: { missTable: string; scTable: string; hookExitTable: string; lifecycleTable: string; compactSummaryTable: string },
): string[] {
  const { failed, recallEn, recallJa } = computed;
  const { missTable, scTable, hookExitTable, lifecycleTable, compactSummaryTable } = tables;
  return [
    '### SC-009 fact recall',
    '',
    `Japanese ${(recallRateOf(recallJa) * 100).toFixed(1)}% (${recallJa.filter((row) => row.hit).length}/${recallJa.length}); English ${(recallRateOf(recallEn) * 100).toFixed(1)}% (${recallEn.filter((row) => row.hit).length}/${recallEn.length}); overall ${(recallRateOf(input.recallHits) * 100).toFixed(1)}% (${input.recallHits.filter((row) => row.hit).length}/${input.recallHits.length}). Bound ≥ 90%. Summaries are rule-based (\`preset\` default with no credentials).`,
    '',
    'Misses:',
    '',
    missTable,
    '',
    '### Lifecycle',
    '',
    'Each `tags.lifecycle` sequence checked against contracts/agents.md. `fork`: the forked session\'s `conversation_id` differs from the preceding session of that agent. `resume`: that SessionStart created no `injections` row and printed no pack. `compact`: at least one detector-clean (`classification_state = done`) `compaction_summary` row exists, and the conversation\'s `context_epoch` equals that row count (Claude\'s PostCompact + SessionStart(compact) pair counts once, A16). `clear`: a new session id and a new conversation. A compaction hook that misses the detector deadline stores a `failed` row and by A16 opens no epoch; 0/0 fails this check.',
    '',
    lifecycleTable,
    '',
    '`compaction_summary` rows:',
    '',
    compactSummaryTable,
    '',
    '### Hook exits',
    '',
    'Capture and injection hooks (including Pi `inject`). Bound: every process exits 0; a non-zero status, a kill signal, or a spawn timeout is a contract violation (contracts/cli.md, FR-002). Wall time of these hooks stays in the timing tables above.',
    '',
    hookExitTable,
    '',
    '### Bounds',
    '',
    scTable,
    '',
    failed
      ? 'One or more measured bounds failed. The numbers above are the run, not a softened reading.'
      : 'Every listed bound passed on this run.',
  ];
}

/** The evidence section, in the order the document reads. */
function reportMarkdown(
  input: MeasureInput,
  computed: ReturnType<typeof computeReport>,
  tables: { captureTable: string; injectionTable: string; waitTable: string; sizeTable: string; missTable: string; scTable: string; hookExitTable: string; lifecycleTable: string; compactSummaryTable: string },
): string {
  const cpu = cpus()[0]?.model ?? 'unknown';
  const machine = `${osType()} ${hostname()} ${release()} ${arch()}`;
  const { captureTable, injectionTable, waitTable, sizeTable, missTable, scTable, hookExitTable, lifecycleTable, compactSummaryTable } = tables;
  return [
    ...setupSection(input, machine, cpu),
    ...hookTimingSection(input, computed, { captureTable, injectionTable, waitTable, sizeTable }),
    ...resourceSection(input, computed),
    ...recallSection(input, computed, { missTable, scTable, hookExitTable, lifecycleTable, compactSummaryTable }),
  ].join('\n');
}

/** The same evidence as machine-readable JSON. */
/** The timing, worker and growth halves of the machine report. */
function timingJson(
  input: MeasureInput,
  computed: ReturnType<typeof computeReport>,
): Record<string, unknown> {
  const { captureP99, captureUnder, captureValues, dbBytesAfter, injectionItems, injections, memories, pending, pendingMax, pendingPass, perThousand, rawEvents, readyMax, readyPass, sc002, sc003, workerRssKb } = computed;
  return {
    capture: { n: captureValues.length, p99: captureP99, under: captureUnder, pass: sc002 },
    injection: { n: input.injectionSamples.length, samples: input.injectionSamples },
    sessionStart: {
      ready: { n: input.readySamples.length, max: readyMax, pass: readyPass },
      pending: { n: input.pendingSamples.length, max: pendingMax, summaryPending: pending.hits, pass: pendingPass },
    },
    worker: {
      observeRuns: input.observeRuns,
      hookWorkerRuns: input.hookWorkerRuns,
      hookWorkerRssKb: input.hookWorkerRssKb,
      rssKb: workerRssKb,
      pass: sc003,
    },
    growth: {
      before: input.dbBytesBefore,
      after: dbBytesAfter,
      perThousand,
      rawEvents,
      memories,
      injections,
      injectionItems,
    },
  };
}

function reportJson(
  input: MeasureInput,
  computed: ReturnType<typeof computeReport>,
  bounds: BoundRow[],
): Record<string, unknown> {
  const { duplicateGroups, failed, hooksPass, leakedDirectives, leakedSecrets, lifecycleRows, misses, negativesUnredacted, rawDirectiveRows, rawEvents, recallEn, recallJa, sc009, sc010 } = computed;
  return {
    startedAt: input.startedAt,
    lines: input.lines.length,
    ...timingJson(input, computed),
    secrets: { leaked: leakedSecrets, negativesUnredacted },
    directives: { leaked: leakedDirectives.length, rawRows: rawDirectiveRows },
    recall: {
      ja: recallRateOf(recallJa),
      en: recallRateOf(recallEn),
      overall: recallRateOf(input.recallHits),
      misses: misses.map((row) => ({ id: row.id, query: row.query })),
      pass: sc009,
    },
    duplicates: { groups: duplicateGroups.length, rawEvents, lines: input.lines.length, pass: sc010 },
    hooks: { n: input.hookCount, failures: input.hookFailures.length, pass: hooksPass },
    lifecycle: lifecycleRows,
    bounds,
    failed,
  };
}

/** The evidence section and its machine form, from what computeReport measured. */
function renderReport(
  input: MeasureInput,
  computed: ReturnType<typeof computeReport>,
): { markdown: string; json: Record<string, unknown>; failed: boolean } {
  const bounds: BoundRow[] = [...timingBounds(input, computed), ...contentBounds(input, computed)];
  const timing = timingTables(input, computed);
  const finding = findingTables(input, computed, bounds);
  return {
    markdown: reportMarkdown(input, computed, { ...timing, ...finding }),
    json: reportJson(input, computed, bounds),
    failed: computed.failed,
  };
}

