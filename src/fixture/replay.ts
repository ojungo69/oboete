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
    throw new Error(`fixture line ${index + 1} is missing seq/event/session`);
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
  const line = text.replace(/\r/g, '').split('\n').find((entry) => entry.trim() !== '') ?? '';
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
  return trimmed.replace(/\|/g, '\\|');
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
    holdFromSeq[agent] = ends[agent].filter((seq) => seq < lastStartSeq[agent]).pop() ?? 0;
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

export async function runFixture(argv: string[]): Promise<number> {
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

  const fixturePath = resolve(positionals[1]);
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

  const maps = corpus(root);
  const createdHome = values.home === undefined && process.env.OBOETE_HOME === undefined;
  const home =
    values.home !== undefined
      ? resolve(values.home)
      : process.env.OBOETE_HOME !== undefined && process.env.OBOETE_HOME !== ''
        ? isAbsolute(process.env.OBOETE_HOME)
          ? resolve(process.env.OBOETE_HOME)
          : resolve(process.cwd(), process.env.OBOETE_HOME)
        : mkdtempSync(join(tmpdir(), 'oboete-t068-home-'));
  const repo = mkdtempSync(join(tmpdir(), 'oboete-t068-repo-'));
  const keep = values.keep === true;
  const paths = oboetePaths(home);
  const envBase = replayEnv(home);
  const startedAt = new Date().toISOString();
  const loadAtStart = loadAverage();
  const { lastStartSeq, holdFromSeq } = sessionWindows;
  const pendingHold = new Set<Agent>();
  const pendingSessions = new Set<string>();
  for (const agent of AGENTS) {
    const start = lines.find((line) => line.agent === agent && line.seq === lastStartSeq[agent]);
    if (start !== undefined) pendingSessions.add(`${agent}:${start.session}`);
  }

  const captureSamples: Sample[] = [];
  const injectionSamples: Sample[] = [];
  const readySamples: Sample[] = [];
  const pendingSamples: Sample[] = [];
  const sizeRows: SizeRow[] = [];
  const packs: { seq: number; agent: Agent; session: string; event: string; text: string }[] = [];
  const sessionStartPack = new Map<string, string>();
  const factsById = new Map<string, Fact>();
  const recallHits: RecallHit[] = [];
  let grokRecallWait: { seq: number; session: string; fact: Fact }[] = [];
  const hookFailures: HookFailure[] = [];
  const resumeChecks: ResumeCheck[] = [];
  let hookCount = 0;
  let observeRssKb = 0;
  let observeRuns = 0;
  let hookWorkerRssKb = 0;
  const hookWorkerPids = new Set<number>();
  const observePids = new Set<number>();
  let workerPollDb: ReturnType<typeof openDatabase>['db'] | undefined;
  let workerPoll: ReturnType<typeof setInterval> | undefined;
  let storageFailed = false;
  let leaseHeld = false;

  const factsFrom = (line: Line): void => {
    const fact = line.tags?.fact;
    if (fact !== undefined) factsById.set(fact.id, fact);
  };
  for (const line of lines) factsFrom(line);

  const recordPack = (line: Line, stdout: string): void => {
    const text = packText(stdout);
    if (text.trim() === '') return;
    packs.push({ seq: line.seq, agent: line.agent, session: line.session, event: line.event, text });
    const key = `${line.agent}:${line.session}`;
    if (
      sessionStartEvent(line.agent, line.event) ||
      (line.agent === 'grok' && line.event === 'PreToolUse' && !sessionStartPack.has(key))
    ) {
      sessionStartPack.set(key, text);
    }
  };

  const recordHook = (line: Line, spawned: Spawned, eventLabel: string): void => {
    hookCount += 1;
    if (!hookViolated(spawned)) return;
    hookFailures.push({
      seq: line.seq,
      agent: line.agent,
      event: eventLabel,
      status: hookStatusCell(spawned),
      stderr: firstStderrLine(spawned.stderr),
    });
  };

  const checkRecall = (line: Line, pack: string): void => {
    const id = line.tags?.recall;
    if (id === undefined) return;
    const fact = factsById.get(id);
    if (fact === undefined) return;
    const start = sessionStartPack.get(`${line.agent}:${line.session}`) ?? '';
    const hit = pack.includes(fact.expect) || start.includes(fact.expect);
    recallHits.push({ id: fact.id, lang: fact.lang, query: fact.query, expect: fact.expect, hit });
  };

  let liveObserve: ObserveProc | undefined;

  const harvestRss = (): void => {
    if (liveObserve === undefined) return;
    const value = liveObserve.rssKb();
    if (value > observeRssKb) observeRssKb = value;
    if (liveObserve.status() === 3) storageFailed = true;
  };

  const startWorker = (): void => {
    if (liveObserve?.running() === true) return;
    liveObserve = startObserve(bundle, repo, envBase);
    if (liveObserve.pid !== undefined) observePids.add(liveObserve.pid);
    observeRuns += 1;
    void liveObserve.exited.then(() => harvestRss());
  };

  const waitEndedSummaries = async (timeoutMs: number): Promise<void> => {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      harvestRss();
      const pending = endedPendingCount(paths.db);
      if (pending === 0) return;
      if (liveObserve === undefined || !liveObserve.running()) startWorker();
      await sleep(50);
    }
  };

  const ensureLeaseHeld = async (): Promise<void> => {
    if (leaseHeld) {
      holdLease(paths.db);
      return;
    }
    // A previous observe may still be looping on an active session's unbatched rows (up to 20 min).
    // Steal after a short wait so SessionEnd is not blocked on that idle loop.
    await waitLeaseFree(paths.db, 500);
    holdLease(paths.db);
    leaseHeld = true;
  };

  const dropLease = (): void => {
    if (!leaseHeld) return;
    releaseHeldLease(paths.db);
    leaseHeld = false;
  };

  const observeNow = async (): Promise<void> => {
    dropLease();
    startWorker();
    await waitEndedSummaries(45_000);
  };

  try {
    initRepo(repo);
    mkdirSync(home, { recursive: true, mode: 0o700 });
    ensureDirectories(paths);
    try {
      openDatabase({ path: paths.db, timeoutMs: 5_000 }).db.close();
    } catch (error) {
      process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
      return 3;
    }
    const dbBytesBefore = fileBytes(paths.db) + fileBytes(`${paths.db}-wal`);
    workerPollDb = openDatabase({ path: paths.db, timeoutMs: 0, hook: true }).db;
    const workerPid = workerPollDb.prepare('SELECT pid FROM worker_lease WHERE id = 1');
    workerPoll = setInterval(() => {
      try {
        const pid = workerPid.get()?.pid;
        if (typeof pid === 'number' && pid !== process.pid && !observePids.has(pid)) {
          hookWorkerPids.add(pid);
          hookWorkerRssKb = Math.max(hookWorkerRssKb, readVmHwm(pid));
        }
      } catch {
        // T068 / R6: a busy lease read must not interrupt replay; the next poll retries it.
      }
    }, 50);

    for (const line of lines) {
      if (line.seq % 100 === 0) process.stderr.write(`replay ${line.seq}/${lines.length}\n`);
      const key = `${line.agent}:${line.session}`;
      const isPendingStart = pendingSessions.has(key) && sessionStartEvent(line.agent, line.event);
      const injection = isInjectionHook(line.agent, line.event);

      let payload = line.payload;
      let fillSize = 0;
      try {
        if (line.tags?.size !== undefined) {
          const filled = expandPayload(payload, { fill: true });
          fillSize = Buffer.byteLength(JSON.stringify(filled));
          const tag = line.tags.size;
          const ok = tag === 'at_bound' ? fillSize === AT_BOUND : fillSize === ABOVE_ONE || fillSize === ABOVE_TWO;
          if (!ok) {
            process.stderr.write(
              `size tag ${tag} seq=${line.seq}: FILL-only JSON is ${fillSize} bytes, expected ${tag === 'at_bound' ? AT_BOUND : `${ABOVE_ONE} or ${ABOVE_TWO}`}\n`,
            );
            return 2;
          }
          payload = expandPayload(filled, { root: repo, fill: false });
        } else {
          payload = expandPayload(payload, {
            root: repo,
            secrets: maps.secrets,
            directives: maps.directives,
            fill: true,
          });
        }
      } catch (error) {
        process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
        return 2;
      }

      const nativeId = nativeSessionId(line.agent, payload);
      let resumeBefore = 0;
      if (line.tags?.lifecycle === 'resume') {
        resumeBefore = startInjectionCount(paths.db, line.agent, nativeId);
      }

      if (SESSION_END.has(line.event) || pendingHold.size > 0) {
        try {
          await ensureLeaseHeld();
        } catch (error) {
          process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
          return 3;
        }
      }

      const input = JSON.stringify(payload);
      const { args, extra } = hookArgs({ ...line, payload });
      const env = replayEnv(home, extra);
      const timeoutMs = injection ? 15_000 : 10_000;
      const hooked = await runChild(bundle, args, input, repo, env, timeoutMs);
      recordHook(line, hooked, line.event);
      const sample: Sample = {
        agent: line.agent,
        event: line.event,
        seq: line.seq,
        session: line.session,
        ms: hooked.elapsedMs,
      };
      const holdActive = pendingHold.size > 0;
      if (injection && !(sessionStartEvent(line.agent, line.event) && holdActive)) {
        injectionSamples.push(sample);
      } else if (!injection) {
        captureSamples.push(sample);
      }
      recordPack(line, hooked.stdout);

      if (sessionStartEvent(line.agent, line.event) && line.agent !== 'pi') {
        if (isPendingStart && holdActive) pendingSamples.push(sample);
        else if (line.seq > 1 && !holdActive) readySamples.push(sample);
      }

      let injected: Spawned | undefined;
      if (line.agent === 'pi' && (line.event === 'session_start' || line.event === 'input') && line.tags?.size === undefined) {
        const envelope = payload as {
          cwd?: unknown;
          session_id?: unknown;
          model?: unknown;
          payload?: { text?: unknown };
        };
        const kind = line.event === 'session_start' ? 'start' : 'prompt';
        const injectInput = JSON.stringify({
          cwd: typeof envelope.cwd === 'string' ? envelope.cwd : repo,
          session_id: typeof envelope.session_id === 'string' ? envelope.session_id : nativeSessionId('pi', payload),
          prompt: typeof envelope.payload?.text === 'string' ? envelope.payload.text : undefined,
          model: typeof envelope.model === 'string' ? envelope.model : undefined,
        });
        injected = await runChild(
          bundle,
          ['inject', '--agent', 'pi', '--kind', kind],
          injectInput,
          repo,
          envBase,
          kind === 'start' ? 15_000 : 10_000,
        );
        recordHook(line, injected, `inject:${kind}`);
        const injectSample: Sample = {
          agent: 'pi',
          event: line.event,
          seq: line.seq,
          session: line.session,
          ms: injected.elapsedMs,
        };
        recordPack(line, injected.stdout);
        if (line.event === 'session_start') {
          if (isPendingStart && holdActive) pendingSamples.push(injectSample);
          else if (!holdActive) {
            injectionSamples.push(injectSample);
            if (line.seq > 1) readySamples.push(injectSample);
          }
        } else {
          injectionSamples.push(injectSample);
        }
      }

      if (line.tags?.lifecycle === 'resume') {
        const after = startInjectionCount(paths.db, line.agent, nativeId);
        const printed =
          packText(hooked.stdout).trim() !== '' || (injected !== undefined && packText(injected.stdout).trim() !== '');
        resumeChecks.push({
          seq: line.seq,
          agent: line.agent,
          session: line.session,
          packPrinted: printed,
          injectionDelta: after < 0 || resumeBefore < 0 ? 1 : after - resumeBefore,
        });
      }

      if (line.agent === 'grok' && line.event === 'PreToolUse') {
        const waiting = grokRecallWait.filter((item) => item.session === line.session);
        if (waiting.length > 0) {
          const pack = packText(hooked.stdout);
          const start = sessionStartPack.get(key) ?? '';
          for (const item of waiting) {
            recallHits.push({
              id: item.fact.id,
              lang: item.fact.lang,
              query: item.fact.query,
              expect: item.fact.expect,
              hit: pack.includes(item.fact.expect) || start.includes(item.fact.expect),
            });
          }
          grokRecallWait = grokRecallWait.filter((item) => item.session !== line.session);
        }
      }

      if (line.tags?.recall !== undefined) {
        if (line.agent === 'grok') {
          const fact = factsById.get(line.tags.recall);
          if (fact !== undefined) grokRecallWait.push({ seq: line.seq, session: line.session, fact });
        } else {
          const pack =
            packs
              .filter((entry) => entry.agent === line.agent && entry.session === line.session && entry.seq >= line.seq)
              .map((entry) => entry.text)
              .join('\n') || packText(hooked.stdout);
          checkRecall(line, pack);
        }
      }

      if (line.tags?.size !== undefined) {
        let classification = 'missing';
        let truncated = 0;
        try {
          const opened = openDatabase({ path: paths.db, timeoutMs: 2_000, hook: true });
          try {
            const row = opened.db
              .prepare(
                `SELECT classification_state AS classification_state, truncated AS truncated
                 FROM raw_events ORDER BY captured_at DESC, id DESC LIMIT 1`,
              )
              .get() as { classification_state?: unknown; truncated?: unknown } | undefined;
            classification = typeof row?.classification_state === 'string' ? row.classification_state : 'missing';
            truncated = typeof row?.truncated === 'number' ? row.truncated : 0;
          } finally {
            opened.db.close();
          }
        } catch {
          classification = 'unreadable';
        }
        sizeRows.push({
          seq: line.seq,
          agent: line.agent,
          event: line.event,
          tag: line.tags.size,
          fillBytes: fillSize,
          ms: hooked.elapsedMs,
          classification,
          truncated,
        });
      }

      if (SESSION_END.has(line.event)) {
        if (skipObserve(line, holdFromSeq)) pendingHold.add(line.agent);
        else if (pendingHold.size === 0) await observeNow();
      }

      if (line.seq === lastStartSeq[line.agent]) {
        pendingHold.delete(line.agent);
        if (pendingHold.size === 0) await observeNow();
      }
    }

    dropLease();
    startWorker();
    await waitEndedSummaries(60_000);
    harvestRss();
    if (liveObserve?.running() === true) {
      holdLease(paths.db);
      const stop = Date.now() + 5_000;
      while (Date.now() < stop && liveObserve.running()) await sleep(50);
      harvestRss();
      releaseHeldLease(paths.db);
    }
    clearInterval(workerPoll);

    if (storageFailed) {
      process.stderr.write('observe reported unusable storage\n');
      return 3;
    }

    const opened = openDatabase({ path: paths.db, timeoutMs: 5_000 });
    let measured;
    try {
      measured = measure(opened, paths, {
        lines,
        captureSamples,
        injectionSamples,
        readySamples,
        pendingSamples,
        sizeRows,
        packs,
        sessionStartPack,
        recallHits,
        grokRecallWait,
        hookFailures,
        hookCount,
        resumeChecks,
        maps,
        observeRssKb,
        observeRuns,
        hookWorkerRssKb,
        hookWorkerRuns: hookWorkerPids.size,
        dbBytesBefore,
        home,
        fixturePath,
        bundle,
        startedAt,
        loadAtStart,
      });
    } finally {
      opened.db.close();
    }

    if (values.json === true) process.stdout.write(`${JSON.stringify(measured.json, null, 2)}\n`);
    if (outPath !== undefined) replaceSection(outPath, measured.markdown);
    else if (values.json !== true) process.stdout.write(`${measured.markdown}\n`);

    return measured.failed ? 1 : 0;
  } finally {
    clearInterval(workerPoll);
    workerPollDb?.close();
    if (!keep) {
      rmSync(repo, { recursive: true, force: true });
      if (createdHome) rmSync(home, { recursive: true, force: true });
    } else {
      process.stderr.write(`kept home=${home} repo=${repo}\n`);
    }
  }
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

function measure(
  opened: ReturnType<typeof openDatabase>,
  paths: ReturnType<typeof oboetePaths>,
  input: {
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
  },
): { markdown: string; json: Record<string, unknown>; failed: boolean } {
  const { db } = opened;
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

  const packBlob = input.packs.map((pack) => pack.text).join('\n');
  const dbBuffers = [paths.db, `${paths.db}-wal`, `${paths.db}-shm`].filter((path) => existsSync(path)).map((path) => readFileSync(path));
  const extraFiles = [...walkFiles(paths.spool), ...walkFiles(paths.logs)].map((path) => readFileSync(path));
  const surfaces = [...dbBuffers, ...extraFiles, Buffer.from(packBlob, 'utf8')];

  const leakedSecrets: string[] = [];
  for (const row of input.maps.secretValues) {
    if (surfaces.some((buffer) => bufferHas(buffer, row.secret))) leakedSecrets.push(row.id);
  }

  const memoryRows = db.prepare('SELECT title AS title, body AS body FROM memories').all() as {
    title: unknown;
    body: unknown;
  }[];
  const memoryText = memoryRows
    .map((row) => `${typeof row.title === 'string' ? row.title : ''}\n${typeof row.body === 'string' ? row.body : ''}`)
    .join('\n');
  let negativesUnredacted = 0;
  for (const row of input.maps.negatives) {
    if (memoryText.includes(row.text)) negativesUnredacted += 1;
  }

  const leakedDirectives: string[] = [];
  for (const phrase of input.maps.directives) {
    if (memoryText.includes(phrase) || packBlob.includes(phrase)) leakedDirectives.push(phrase);
  }
  let rawDirectiveRows = 0;
  const rawContents = db.prepare('SELECT content AS content FROM raw_events WHERE content IS NOT NULL').all() as {
    content: unknown;
  }[];
  for (const row of rawContents) {
    const content = row.content;
    if (typeof content !== 'string') continue;
    if (input.maps.directives.some((phrase) => content.includes(phrase))) rawDirectiveRows += 1;
  }

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

  const recallJa = input.recallHits.filter((row) => row.lang === 'ja');
  const recallEn = input.recallHits.filter((row) => row.lang === 'en');
  const recallRate = (rows: RecallHit[]): number =>
    rows.length === 0 ? 1 : rows.filter((row) => row.hit).length / rows.length;
  const misses = input.recallHits.filter((row) => !row.hit);

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
  for (const line of input.lines) {
    const key = `${line.agent}:${line.session}`;
    if (!nativeByLabel.has(key)) nativeByLabel.set(key, nativeSessionId(line.agent, line.payload));
  }
  const order = sessionOrder(input.lines);
  const conversationOf = (
    agent: Agent,
    label: string,
  ): { native: string; conversationId: string; epoch: number } | undefined => {
    const native = nativeByLabel.get(`${agent}:${label}`);
    if (native === undefined) return undefined;
    const row = sessionByNative.get(`${agent}\t${native}`);
    if (row === undefined) return undefined;
    const root = sessionById.get(String(row.conversation_id)) ?? row;
    return {
      native,
      conversationId: String(row.conversation_id),
      epoch: Number(root.context_epoch ?? 0),
    };
  };

  type LifeRow = { check: string; n: number; pass: boolean; offenders: string[] };
  const life = (check: string, results: { session: string; pass: boolean }[]): LifeRow => ({
    check,
    n: results.length,
    pass: results.length > 0 && results.every((row) => row.pass),
    offenders: results.filter((row) => !row.pass).map((row) => row.session),
  });

  const forkRows: { session: string; pass: boolean }[] = [];
  const clearRows: { session: string; pass: boolean }[] = [];
  const compactRows: { session: string; pass: boolean }[] = [];
  for (const line of input.lines) {
    const tag = line.tags?.lifecycle;
    if (tag === undefined) continue;
    const start = sessionStartEvent(line.agent, line.event);
    if (tag === 'fork' || tag === 'clear') {
      const subject = start ? line.session : neighborSession(order[line.agent], line.session, 1);
      const parent = start ? neighborSession(order[line.agent], line.session, -1) : line.session;
      const left = subject === undefined ? undefined : conversationOf(line.agent, subject);
      const right = parent === undefined ? undefined : conversationOf(line.agent, parent);
      const pass =
        left !== undefined &&
        right !== undefined &&
        left.conversationId !== right.conversationId &&
        (tag === 'fork' || left.native !== right.native);
      const label = `${line.agent}:${subject ?? line.session}`;
      if (tag === 'fork') forkRows.push({ session: label, pass });
      else clearRows.push({ session: label, pass });
    }
    if (tag === 'compact') {
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
      compactRows.push({
        session: `${line.agent}:${line.session} epoch=${conv?.epoch ?? 'missing'} rows=${count}`,
        pass: conv !== undefined && count >= 1 && conv.epoch === count,
      });
    }
  }
  const resumeLife = life(
    'resume',
    input.resumeChecks.map((row) => ({
      session: `${row.agent}:${row.session}`,
      pass: !row.packPrinted && row.injectionDelta === 0,
    })),
  );
  const lifecycleRows = [
    life('fork', forkRows),
    resumeLife,
    life('compact', compactRows),
    life('clear', clearRows),
  ];
  const lifecyclePass = lifecycleRows.every((row) => row.pass);

  const compactionSummaries = db
    .prepare(
      `SELECT s.agent AS agent, s.native_session_id AS native_session_id,
              e.classification_state AS classification_state
       FROM raw_events e
       JOIN sessions s ON s.id = e.session_id
       WHERE e.kind = 'compaction_summary'
       ORDER BY s.agent, e.captured_at, e.id`,
    )
    .all() as { agent: unknown; native_session_id: unknown; classification_state: unknown }[];

  const captureValues = input.captureSamples.map((sample) => sample.ms);
  const captureUnder =
    captureValues.length === 0
      ? 1
      : captureValues.filter((value) => value <= CAPTURE_DEADLINE_MS).length / captureValues.length;
  const captureP99 = percentile(captureValues, 99);
  const sc002 = captureValues.length > 0 && captureUnder >= 0.99 && captureP99 <= CAPTURE_DEADLINE_MS;
  const injectionValues = input.injectionSamples.map((sample) => sample.ms);
  const injectionUnder =
    injectionValues.length === 0
      ? 1
      : injectionValues.filter((value) => value <= READY_BOUND_MS).length / injectionValues.length;
  const injectionP99 = percentile(injectionValues, 99);
  const injectionTiming = timingRows(input.injectionSamples, () => READY_BOUND_MS, 1);
  const injectionPass = injectionTiming.pass;
  const pendingSentence = (samples: Sample[]): { hits: number; text: string } => {
    const hits = samples.filter((sample) => {
      const pack =
        input.packs.find((entry) => entry.seq === sample.seq)?.text ??
        input.sessionStartPack.get(`${sample.agent}:${sample.session}`) ??
        '';
      return pack.includes(SUMMARY_PENDING);
    }).length;
    return { hits, text: `${hits}/${samples.length} packs carry summary_pending` };
  };
  const pending = pendingSentence(input.pendingSamples);
  const readyMax = input.readySamples.length === 0 ? 0 : Math.max(...input.readySamples.map((sample) => sample.ms));
  const pendingMax = input.pendingSamples.length === 0 ? 0 : Math.max(...input.pendingSamples.map((sample) => sample.ms));
  const readyPass = input.readySamples.every((sample) => sample.ms <= READY_BOUND_MS);
  const pendingPass =
    input.pendingSamples.length > 0 &&
    pending.hits === input.pendingSamples.length &&
    input.pendingSamples.every((sample) => sample.ms <= PENDING_BOUND_MS);
  const workerRssKb = Math.max(input.observeRssKb, input.hookWorkerRssKb);
  const workerRuns = `observe runs: ${input.observeRuns} spawned by replay, ${input.hookWorkerRuns} hook-spawned (polled via worker_lease.pid)`;
  const sc003 = workerRssKb < WORKER_RSS_BOUND_KB;
  const sc005 = leakedSecrets.length === 0;
  const sc009 =
    recallRate(input.recallHits) >= RECALL_BOUND &&
    (recallJa.length === 0 || recallRate(recallJa) >= RECALL_BOUND) &&
    (recallEn.length === 0 || recallRate(recallEn) >= RECALL_BOUND);
  const sc010 = duplicateGroups.length === 0;
  const directivesPass = leakedDirectives.length === 0;
  const hooksPass = input.hookFailures.length === 0;
  const failed = !(
    sc002 &&
    injectionPass &&
    readyPass &&
    pendingPass &&
    sc003 &&
    sc005 &&
    sc009 &&
    sc010 &&
    lifecyclePass &&
    directivesPass &&
    hooksPass
  );

  const cpu = cpus()[0]?.model ?? 'unknown';
  const machine = `${osType()} ${hostname()} ${release()} ${arch()}`;
  const bounds: BoundRow[] = [
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
    {
      sc: 'SC-005',
      measured: leakedSecrets.length === 0 ? '0 secret ids in db/wal/spool/logs/packs' : `leaked ${leakedSecrets.join(', ')}`,
      bound: 'zero secret corpus values in db, wal, spool, logs, packs',
      status: statusOf(sc005),
    },
    {
      sc: 'SC-009',
      measured: `ja ${(recallRate(recallJa) * 100).toFixed(1)}% (${recallJa.filter((row) => row.hit).length}/${recallJa.length}); en ${(recallRate(recallEn) * 100).toFixed(1)}% (${recallEn.filter((row) => row.hit).length}/${recallEn.length}); overall ${(recallRate(input.recallHits) * 100).toFixed(1)}% (${input.recallHits.filter((row) => row.hit).length}/${input.recallHits.length})`,
      bound: '≥ 90% ja, en, and overall',
      status: statusOf(sc009),
    },
    {
      sc: 'SC-010',
      measured: `${duplicateGroups.length} duplicate included (conversation_id, context_epoch, memory_id) groups; raw_events.id=${rawEvents} vs lines piped=${input.lines.length}`,
      bound: 'zero duplicate included memories per (conversation, epoch)',
      status: statusOf(sc010),
    },
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
  pushWait('ready', input.readySamples, READY_BOUND_MS, pendingSentence(input.readySamples).text,
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

  const markdown = [
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
      : `Directive phrases in memories or packs (${leakedDirectives.length}): ${leakedDirectives.slice(0, 5).join(' | ')}${leakedDirectives.length > 5 ? ' …' : ''}.`,
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
    '### SC-009 fact recall',
    '',
    `Japanese ${(recallRate(recallJa) * 100).toFixed(1)}% (${recallJa.filter((row) => row.hit).length}/${recallJa.length}); English ${(recallRate(recallEn) * 100).toFixed(1)}% (${recallEn.filter((row) => row.hit).length}/${recallEn.length}); overall ${(recallRate(input.recallHits) * 100).toFixed(1)}% (${input.recallHits.filter((row) => row.hit).length}/${input.recallHits.length}). Bound ≥ 90%. Summaries are rule-based (\`preset\` default with no credentials).`,
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
  ].join('\n');

  const json = {
    startedAt: input.startedAt,
    lines: input.lines.length,
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
    secrets: { leaked: leakedSecrets, negativesUnredacted },
    directives: { leaked: leakedDirectives.length, rawRows: rawDirectiveRows },
    recall: {
      ja: recallRate(recallJa),
      en: recallRate(recallEn),
      overall: recallRate(input.recallHits),
      misses: misses.map((row) => ({ id: row.id, query: row.query })),
      pass: sc009,
    },
    duplicates: { groups: duplicateGroups.length, rawEvents, lines: input.lines.length, pass: sc010 },
    hooks: { n: input.hookCount, failures: input.hookFailures.length, pass: hooksPass },
    lifecycle: lifecycleRows,
    bounds,
    failed,
  };

  return { markdown, json, failed };
}
