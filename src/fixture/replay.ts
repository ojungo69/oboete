// `oboete fixture replay`: native payloads through the real hook, resource-envelope evidence.
// Never on the hook path (cli.ts loads this command lazily). Sources: contracts/cli.md,
// contracts/agents.md hook SLAs, spec SC-002/003/005/009/010, FR-040, quickstart "Fixture replay".
import { execFile, spawn, spawnSync } from 'node:child_process';
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { isAbsolute, join, resolve } from 'node:path';
import { performance } from 'node:perf_hooks';
import { setTimeout as sleep } from 'node:timers/promises';
import { parseArgs } from 'node:util';

import { INJECTION_DEADLINE_MS, hookDeadlineMs } from '../capture.js';
import { isBusyError, openDatabase } from '../db/open.js';
import { deliveredFactItems, measure, nativeSessionId, sessionStartEvent } from './replay-evaluate.js';
import { HEADING, fileBytes, repositoryRoot } from './replay-report.js';
import { childEnvironment } from '../log.js';
import { ensureDirectories, oboetePaths } from '../paths.js';
import { claimLease, heartbeat, releaseLease } from '../worker/lease.js';
import { SUMMARIZABLE_ROW_SQL } from '../worker/batches.js';
import { resolveRepoIdentity } from '../repo-identity.js';
import { stripRecognizedPacks } from '../injection/recognize.js';

const ROOT_PH = '__OBOETE_REPLAY_ROOT__';
const FILL_ALPHABET = 'The quick brown fox jumps over the lazy dog. ';
const AT_BOUND = 1_048_576;
const ABOVE_ONE = 1_048_577;
const ABOVE_TWO = 2_097_152;
const SESSION_END = new Set(['SessionEnd', 'session_shutdown']);
const AGENTS = ['claude', 'codex', 'grok', 'pi'] as const;
const WORKER_SETTLE_MS = 5 * 60_000;

class ReplayFailure extends Error {
  constructor(readonly exit: 1 | 3, reason: string) { super(`replay invalid: ${reason}`); }
}

type WaitOptions = { timeoutMs: number; now: () => number; sleep: (ms: number) => Promise<void> };
const waitOptions = (timeoutMs: number): WaitOptions => ({ timeoutMs, now: Date.now, sleep });

export type Agent = (typeof AGENTS)[number];
type SizeTag = 'at_bound' | 'above_bound';
type Fact = { id: string; lang: 'ja' | 'en'; query: string; expect: string };
type CapturedFact = Fact & { seq: number; sourceSessionId: string | null; sourceIds: string[]; factSourceIds: string[] };
export type DeliveredFactItem = { injectionId: string; memoryId: string | null; rawEventId: string | null };
export type RecallProbe = Fact & {
  factSeq: number; querySeq: number; repoId: string; sourceSessionId: string | null;
  sourceIds: string[]; factSourceIds: string[]; sessionId: string | null;
  conversationId: string | null; epoch: number | null; priorInjectionIds: string[];
  priorDelivery: DeliveredFactItem[]; currentInjectionIds: string[]; currentTextHit: boolean;
};
type Tags = {
  secret?: string;
  directive?: number;
  fact?: Fact;
  lifecycle?: string;
  size?: SizeTag;
  recall?: string;
};
export type Line = {
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
export type Sample = { agent: Agent; event: string; seq: number; session: string; ms: number; injectionId?: string };
export type InjectionSnapshot = { id: string; state: string | null; degradedReason: string | null; hash: string | null };
export type StartSample = Sample & { classification: 'ready' | 'pending' | 'unclassified'; state: string | null; degradedReason: string | null };
type ReplayPack = { seq: number; agent: Agent; session: string; event: string; text: string; injectionIds: string[] };
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
export type RecallHit = { id: string; lang: 'ja' | 'en'; query: string; expect: string; hit: boolean };
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

/** The file to run: `dist/oboete.mjs`, the launcher, because that is what `bin` and the installed
 *  hook commands name and its compile cache is part of what the replay measures. The engine next to
 *  it, when there is one, is what `siblingEngine` names alongside it, with its own size: the
 *  launcher is a couple of kilobytes, most of it comment, so the report gives both numbers rather
 *  than one that could be either. */
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
  stop: () => Promise<void>;
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
    stop: async () => {
      if (!running) return;
      child.kill('SIGTERM');
      await Promise.race([exited, sleep(1_000)]);
      if (running) child.kill('SIGKILL');
      await exited;
    },
  };
}

/** Claims only a free/stale lease; an acquisition timeout cannot displace a live worker. */
export async function holdLease(
  dbPath: string,
  options: WaitOptions = waitOptions(5_000),
): Promise<string | null> {
  const deadline = options.now() + options.timeoutMs;
  do {
    try {
      const { db } = openDatabase({ path: dbPath, timeoutMs: 200, hook: true });
      try {
        const token = claimLease(db, { pid: process.pid, now: options.now() });
        if (token !== null) return token;
      } finally { db.close(); }
    } catch (error) { if (!isBusyError(error)) throw new ReplayFailure(3, 'storage_error'); }
    if (options.now() >= deadline) return null;
    await options.sleep(Math.min(50, deadline - options.now()));
  } while (options.now() <= deadline);
  return null;
}

export function releaseHeldLease(dbPath: string, token: string): 'released' | 'kept' | 'lost' {
  const { db } = openDatabase({ path: dbPath, timeoutMs: 2_000, hook: true });
  try { return releaseLease(db, token, () => true); }
  finally { db.close(); }
}

/** A current degraded summary settles this invocation without claiming successful generation. */
export function replayTargetsSettled(
  db: ReturnType<typeof openDatabase>['db'], repoId: string, sessionIds: readonly string[],
): boolean {
  const target = db.prepare(`SELECT 1 FROM sessions s WHERE s.id = ? AND s.repo_id = ? AND s.status = 'ended'
    AND NOT EXISTS (SELECT 1 FROM observation_batches b WHERE b.session_id = s.id AND b.state IN ('pending', 'running'))
    AND NOT EXISTS (SELECT 1 FROM observation_batch_sources bs JOIN observation_batches b ON b.id = bs.batch_id
      WHERE b.session_id = s.id AND bs.outcome = 'assigned')
    AND NOT EXISTS (SELECT 1 FROM raw_events WHERE session_id = s.id AND batch_id IS NULL
      AND processing_state = 'pending' AND ${SUMMARIZABLE_ROW_SQL})
    AND (s.summary_state = 'no_content' OR (s.summary_updated_at IS NOT NULL
      AND NOT EXISTS (SELECT 1 FROM observation_batches b
        WHERE b.session_id = s.id AND b.completed_at > s.summary_updated_at)))`);
  return sessionIds.every((id) => target.get(id, repoId) !== undefined);
}

export async function waitForReplaySettlement(
  dbPath: string, repoId: string, sessionIds: readonly string[],
  worker: Pick<ObserveProc, 'pid' | 'running' | 'status'>,
  options: WaitOptions = waitOptions(WORKER_SETTLE_MS),
): Promise<void> {
  const deadline = options.now() + options.timeoutMs;
  do {
    if (worker.status() === 3) throw new ReplayFailure(3, 'worker_storage_error');
    if (!worker.running() && worker.status() !== 0 && worker.status() !== 1) throw new ReplayFailure(1, 'worker_failed');
    try {
      const { db } = openDatabase({ path: dbPath, timeoutMs: 200, hook: true });
      try {
        const owner = db.prepare('SELECT pid FROM worker_lease WHERE id = 1').get()?.pid;
        if (!worker.running() && owner !== worker.pid && replayTargetsSettled(db, repoId, sessionIds)) return;
      } finally { db.close(); }
    } catch (error) { if (!isBusyError(error)) throw new ReplayFailure(3, 'storage_error'); }
    if (options.now() >= deadline) throw new ReplayFailure(1, 'worker_settle_timeout');
    await options.sleep(Math.min(50, deadline - options.now()));
  } while (options.now() <= deadline);
  throw new ReplayFailure(1, 'worker_settle_timeout');
}

function startInjectionCount(dbPath: string, repoId: string, agent: Agent, nativeId: string): number {
  try {
    const opened = openDatabase({ path: dbPath, timeoutMs: 2_000, hook: true });
    try {
      const row = opened.db
        .prepare(
          `SELECT COUNT(*) AS n FROM injections i
           JOIN sessions s ON s.id = i.session_id
           WHERE s.repo_id = ? AND s.agent = ? AND COALESCE(s.original_native_session_id, s.native_session_id) = ?
             AND i.kind IN ('session_start', 'grok_deferred') AND i.state <> 'omitted'`,
        )
        .get(repoId, agent, nativeId) as { n?: unknown } | undefined;
      return typeof row?.n === 'number' ? row.n : 0;
    } finally {
      opened.db.close();
    }
  } catch {
    return -1;
  }
}

export function replaySession(dbPath: string, repoId: string, agent: Agent, nativeId: string): {
  id: string; conversationId: string; epoch: number;
} | undefined {
  const { db } = openDatabase({ path: dbPath, timeoutMs: 2_000, hook: true });
  try {
    const rows = db.prepare(`SELECT s.id, s.conversation_id, COALESCE(root.context_epoch, s.context_epoch) AS epoch
      FROM sessions s LEFT JOIN sessions root ON root.id = s.conversation_id AND root.repo_id = s.repo_id
      WHERE s.repo_id = ? AND s.agent = ? AND COALESCE(s.original_native_session_id, s.native_session_id) = ? LIMIT 2`)
      .all(repoId, agent, nativeId);
    if (rows.length > 1) throw new ReplayFailure(1, 'ambiguous_session');
    const row = rows[0];
    return row === undefined ? undefined : { id: String(row.id), conversationId: String(row.conversation_id), epoch: Number(row.epoch) };
  } finally { db.close(); }
}

function injectionSnapshot(run: ReplayRun, agent: Agent, nativeId: string): InjectionSnapshot[] {
  const session = replaySession(run.paths.db, run.repoId, agent, nativeId);
  if (session === undefined) return [];
  const { db } = openDatabase({ path: run.paths.db, timeoutMs: 2_000, hook: true });
  try {
    return db.prepare(`SELECT id, state, degraded_reason AS degradedReason, pack_hash AS hash FROM injections
      WHERE repo_id = ? AND conversation_id = ? AND context_epoch = ? ORDER BY created_at, id`)
      .all(run.repoId, session.conversationId, session.epoch) as InjectionSnapshot[];
  } finally { db.close(); }
}

export function classifyStartSample(sample: Sample, rows: InjectionSnapshot[]): StartSample {
  if (rows.length !== 1) return { ...sample, classification: 'unclassified', state: null, degradedReason: null };
  const row = rows[0];
  return { ...sample, injectionId: row.id, state: row.state, degradedReason: row.degradedReason,
    classification: row.degradedReason === 'summary_pending' ? 'pending' : 'ready' };
}

export function startInjectionExpected(line: Line): boolean {
  if (!sessionStartEvent(line.agent, line.event) || line.tags?.lifecycle === 'resume') return false;
  const source = line.payload !== null && typeof line.payload === 'object'
    ? (line.payload as { source?: unknown }).source : undefined;
  return !((line.agent === 'claude' || line.agent === 'codex') && source === 'resume')
    && !(line.agent === 'claude' && source === 'fork');
}

function loadAverage(): string {
  try {
    return readFileSync('/proc/loadavg', 'utf8').trim();
  } catch {
    return 'unavailable';
  }
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
  repoId: string;
  home: string;
  envBase: NodeJS.ProcessEnv;
  paths: ReturnType<typeof oboetePaths>;
  lines: Line[];
  maps: ReturnType<typeof corpus>;
  lastStartSeq: ReturnType<typeof lastSessions>['lastStartSeq'];
  holdFromSeq: ReturnType<typeof lastSessions>['holdFromSeq'];
  pendingHold: Set<Agent>;
  captureSamples: Sample[];
  injectionSamples: Sample[];
  readySamples: Sample[];
  pendingSamples: Sample[];
  startSamples: StartSample[];
  sizeRows: SizeRow[];
  packs: ReplayPack[];
  factsById: Map<string, CapturedFact>;
  recallProbes: RecallProbe[];
  grokRecallWait: { session: string; probe: RecallProbe }[];
  hookFailures: HookFailure[];
  resumeChecks: ResumeCheck[];
  hookCount: number;
  observeRssKb: number;
  observeRuns: number;
  hookWorkerRssKb: number;
  hookWorkerPids: Set<number>;
  observePids: Set<number>;
  storageFailed: boolean;
  leaseToken: string | null;
  leaseFailure: ReplayFailure | null;
  endedTargets: Set<string>;
  fixtureSessions: Set<string>;
  liveObserve: ObserveProc | undefined;
};

/** Everything a replay accumulates while it runs, empty before its first hook. */
function emptyTables(): Omit<
  ReplayRun,
  'bundle' | 'repo' | 'home' | 'envBase' | 'paths' | 'lines' | 'maps' | 'lastStartSeq' | 'holdFromSeq'
> {
  return {
    pendingHold: new Set<Agent>(),
    captureSamples: [],
    injectionSamples: [],
    readySamples: [],
    pendingSamples: [],
    startSamples: [],
    sizeRows: [],
    packs: [],
    factsById: new Map<string, CapturedFact>(),
    recallProbes: [],
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
    repoId: '',
    leaseToken: null,
    leaseFailure: null,
    endedTargets: new Set<string>(),
    fixtureSessions: new Set<string>(),
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
    ...emptyTables(),
  };
  // Every fact the fixture plants, indexed before the first hook so recall can look one up.
  for (const line of input.lines) {
    const fact = line.tags?.fact;
    if (fact !== undefined) run.factsById.set(fact.id, { ...fact, seq: line.seq,
      sourceSessionId: null, sourceIds: [], factSourceIds: [] });
  }
  return run;
}

/** A pack the hook printed, kept for the recall and directive checks. */
function recordPack(run: ReplayRun, line: Line, stdout: string): void {
  const text = packText(stdout);
  if (text.trim() === '') return;
  const { db } = openDatabase({ path: run.paths.db, timeoutMs: 2_000, hook: true });
  let hashes: string[];
  try { hashes = stripRecognizedPacks(db, text).hashes; } finally { db.close(); }
  const injectionIds = injectionSnapshot(run, line.agent, nativeSessionId(line.agent, line.payload))
    .filter((row) => row.hash !== null && hashes.includes(row.hash)).map((row) => row.id);
  run.packs.push({ seq: line.seq, agent: line.agent, session: line.session, event: line.event, text, injectionIds });

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

function factSourceRows(run: ReplayRun, agent: Agent, nativeId: string) {
  const session = replaySession(run.paths.db, run.repoId, agent, nativeId);
  if (session === undefined) return [];
  const { db } = openDatabase({ path: run.paths.db, timeoutMs: 2_000, hook: true });
  try {
    return db.prepare('SELECT id, content, payload_json FROM raw_events WHERE repo_id = ? AND session_id = ?')
      .all(run.repoId, session.id);
  } finally { db.close(); }
}

function recordFact(run: ReplayRun, line: Line, nativeId: string, before: Set<string>): void {
  const fact = line.tags?.fact === undefined ? undefined : run.factsById.get(line.tags.fact.id);
  if (fact === undefined) return;
  const rows = factSourceRows(run, line.agent, nativeId).filter((row) => !before.has(String(row.id)));
  fact.sourceSessionId = replaySession(run.paths.db, run.repoId, line.agent, nativeId)?.id ?? null;
  fact.sourceIds = rows.map((row) => String(row.id));
  fact.factSourceIds = rows.filter((row) => {
    const texts = [String(row.content ?? '')];
    try {
      JSON.parse(String(row.payload_json), (_key, value: unknown) => {
        if (typeof value === 'string') texts.push(value);
        return value;
      });
    } catch { /* Partial or rejected payloads may retain no fact-bearing text. */ }
    return texts.some((text) => text.includes(fact.expect));
  }).map((row) => String(row.id));
}

function recallBefore(run: ReplayRun, line: Line, nativeId: string, priorIds: string[]): RecallProbe | undefined {
  const fact = line.tags?.recall === undefined ? undefined : run.factsById.get(line.tags.recall);
  if (fact === undefined) return undefined;
  const session = replaySession(run.paths.db, run.repoId, line.agent, nativeId);
  const probe: RecallProbe = { id: fact.id, lang: fact.lang, query: fact.query, expect: fact.expect,
    factSeq: fact.seq, querySeq: line.seq, repoId: run.repoId, sourceSessionId: fact.sourceSessionId,
    sourceIds: fact.sourceIds, factSourceIds: fact.factSourceIds, sessionId: session?.id ?? null,
    conversationId: session?.conversationId ?? null, epoch: session?.epoch ?? null,
    priorInjectionIds: priorIds, priorDelivery: [], currentInjectionIds: [], currentTextHit: false };
  const printed = run.packs.filter((pack) => pack.seq < line.seq && pack.text.includes(fact.expect))
    .flatMap((pack) => pack.injectionIds);
  const { db } = openDatabase({ path: run.paths.db, timeoutMs: 2_000, hook: true });
  try { probe.priorDelivery = deliveredFactItems(db, probe, priorIds.filter((id) => printed.includes(id))); }
  finally { db.close(); }
  return probe;
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

/** The replay holds ownership through capture and refreshes it while child hooks run. */
async function ensureLeaseHeld(run: ReplayRun): Promise<void> {
  if (run.leaseFailure !== null) throw run.leaseFailure;
  if (run.leaseToken === null) {
    run.leaseToken = await holdLease(run.paths.db);
    if (run.leaseToken === null) throw new ReplayFailure(1, 'lease_acquire_timeout');
  }
  const { db } = openDatabase({ path: run.paths.db, timeoutMs: 2_000, hook: true });
  try {
    if (!heartbeat(db, run.leaseToken, Date.now())) throw new ReplayFailure(1, 'lease_lost');
  } finally { db.close(); }
}

function dropLease(run: ReplayRun): void {
  if (run.leaseToken === null) return;
  const token = run.leaseToken;
  run.leaseToken = null;
  if (releaseHeldLease(run.paths.db, token) !== 'released') throw new ReplayFailure(1, 'lease_lost');
}

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

/** The stdin of `oboete inject --agent pi`, from the fixture envelope. */
function piInjectInput(run: ReplayRun, payload: unknown): string {
  const envelope = payload as {
    cwd?: unknown;
    session_id?: unknown;
    model?: unknown;
    payload?: { text?: unknown };
    prompt_id?: unknown;
  };
  return JSON.stringify({
    cwd: typeof envelope.cwd === 'string' ? envelope.cwd : run.repo,
    session_id: typeof envelope.session_id === 'string' ? envelope.session_id : nativeSessionId('pi', payload),
    prompt: typeof envelope.payload?.text === 'string' ? envelope.payload.text : undefined,
    prompt_id: typeof envelope.prompt_id === 'string' ? envelope.prompt_id : undefined,
    model: typeof envelope.model === 'string' ? envelope.model : undefined,
  });
}

/** Runs `oboete inject` for one Pi line and files its pack and its sample like a hook's. */
async function injectPiLine(
  run: ReplayRun,
  line: Line,
  payload: unknown,
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
  if (line.event !== 'session_start') run.injectionSamples.push(injectSample);
  return injected;
}

/** FR-025: a resumed session prints its pack again and opens no second injection. */
function recordResume(
  run: ReplayRun,
  line: Line,
  seen: { nativeId: string; resumeBefore: number; hooked: Spawned; injected: Spawned | undefined },
): void {
  const after = startInjectionCount(run.paths.db, run.repoId, line.agent, seen.nativeId);
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

/** Keep a Grok query open until its correlated carrier prints; final ledger state proves delivery. */
function settleGrokRecall(run: ReplayRun, line: Line): void {
  if (line.agent !== 'grok' || (line.event !== 'PreToolUse' && line.event !== 'PostToolUse')) return;
  const packs = run.packs.filter((pack) => pack.seq === line.seq);
  if (packs.length === 0) return;
  for (const waiting of run.grokRecallWait.filter((item) => item.session === line.session)) {
    waiting.probe.currentInjectionIds = [...new Set([...waiting.probe.currentInjectionIds,
      ...packs.flatMap((pack) => pack.injectionIds)])];
    waiting.probe.currentTextHit ||= packs.some((pack) => pack.text.includes(waiting.probe.expect));
  }
  run.grokRecallWait = run.grokRecallWait.filter((item) => item.session !== line.session);
}

function openRecall(run: ReplayRun, line: Line, nativeId: string, probe: RecallProbe | undefined): void {
  if (probe === undefined) return;
  const session = replaySession(run.paths.db, run.repoId, line.agent, nativeId);
  if (probe.conversationId !== session?.conversationId || probe.epoch !== session?.epoch) probe.priorDelivery = [];
  probe.sessionId = session?.id ?? null;
  probe.conversationId = session?.conversationId ?? null;
  probe.epoch = session?.epoch ?? null;
  probe.currentInjectionIds = injectionSnapshot(run, line.agent, nativeId)
    .filter((row) => !probe.priorInjectionIds.includes(row.id)).map((row) => row.id);
  const packs = run.packs.filter((pack) => pack.seq === line.seq);
  probe.currentInjectionIds = [...new Set([...probe.currentInjectionIds, ...packs.flatMap((pack) => pack.injectionIds)])];
  probe.currentTextHit = packs.some((pack) => pack.text.includes(probe.expect));
  run.recallProbes.push(probe);
  if (line.agent === 'grok') run.grokRecallWait.push({ session: line.session, probe });
}

/** How the engine classified the size-tagged event it just stored, read back for the size table. */
function lastClassification(run: ReplayRun): { classification: string; truncated: number } {
  let classification: string;
  let truncated = 0;
  try {
    const opened = openDatabase({ path: run.paths.db, timeoutMs: 2_000, hook: true });
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
  return { classification, truncated };
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
  injection: boolean,
): Promise<Spawned> {
  const input = JSON.stringify(payload);
  const { args, extra } = hookArgs({ ...line, payload });
  const env = replayEnv(run.home, extra);
  const timeoutMs = injection ? 15_000 : 10_000;
  const hooked = await runChild(run.bundle, args, input, run.repo, env, timeoutMs);
  recordHook(run, line, hooked, line.event);
  const sample: Sample = {
    agent: line.agent,
    event: line.event,
    seq: line.seq,
    session: line.session,
    ms: hooked.elapsedMs,
  };
  if (!injection) run.captureSamples.push(sample);
  else if (!sessionStartEvent(line.agent, line.event)) run.injectionSamples.push(sample);
  recordPack(run, line, hooked.stdout);
  return hooked;
}


/**
 * One fixture line: the hook, the Pi injection it also drives, and the checks its tags ask
 * for. `null` continues the replay; a number is the exit code the replay stops with.
 */
async function replayLine(run: ReplayRun, line: Line): Promise<number | null> {
  if (line.seq % 100 === 0) process.stderr.write(`replay ${line.seq}/${run.lines.length}\n`);
  const injection = isInjectionHook(line.agent, line.event);

  const expanded = expandLine(run, line);
  if (typeof expanded === 'number') return expanded;
  const { payload, fillSize } = expanded;

  const nativeId = nativeSessionId(line.agent, payload);
  let resumeBefore = 0;
  if (line.tags?.lifecycle === 'resume') {
    resumeBefore = startInjectionCount(run.paths.db, run.repoId, line.agent, nativeId);
  }

  await ensureLeaseHeld(run);

  const beforeIds = new Set(sessionStartEvent(line.agent, line.event) || line.tags?.recall !== undefined
    ? injectionSnapshot(run, line.agent, nativeId).map((row) => row.id) : []);
  const factBefore = new Set(line.tags?.fact === undefined ? [] : factSourceRows(run, line.agent, nativeId).map((row) => String(row.id)));
  const probe = recallBefore(run, line, nativeId, [...beforeIds]);
  const hooked = await runHookLine(run, line, payload, injection);

  const injected = isPiInjectEvent(line)
    ? await injectPiLine(run, line, payload)
    : undefined;

  if (sessionStartEvent(line.agent, line.event)) {
    const session = replaySession(run.paths.db, run.repoId, line.agent, nativeId);
    if (session === undefined) throw new ReplayFailure(1, 'missing_started_session');
    run.fixtureSessions.add(session.id);
  }
  if (startInjectionExpected(line)) {
    const rows = injectionSnapshot(run, line.agent, nativeId).filter((row) => !beforeIds.has(row.id));
    const sample = classifyStartSample({ agent: line.agent, event: line.event, seq: line.seq,
      session: line.session, ms: (injected ?? hooked).elapsedMs }, rows);
    run.startSamples.push(sample);
    if (sample.classification === 'pending') run.pendingSamples.push(sample);
    if (sample.classification === 'ready') {
      run.readySamples.push(sample);
      run.injectionSamples.push(sample);
    }
  } else if (sessionStartEvent(line.agent, line.event)) {
    run.injectionSamples.push({ agent: line.agent, event: line.event, seq: line.seq,
      session: line.session, ms: (injected ?? hooked).elapsedMs });
  }
  if (line.tags?.lifecycle === 'resume') {
    recordResume(run, line, { nativeId, resumeBefore, hooked, injected });
  }
  recordFact(run, line, nativeId, factBefore);
  settleGrokRecall(run, line);
  openRecall(run, line, nativeId, probe);

  if (line.tags?.size !== undefined) recordSize(run, line, line.tags.size, fillSize, hooked);
  if (SESSION_END.has(line.event)) {
    const ended = replaySession(run.paths.db, run.repoId, line.agent, nativeId);
    if (ended === undefined) throw new ReplayFailure(1, 'missing_ended_session');
    run.endedTargets.add(ended.id);
  }
  await settleObserve(run, line);
  return null;
}

/** Drain ended targets only when no still-active fixture session needs a later end event. */
async function observeNow(run: ReplayRun): Promise<void> {
  if (run.endedTargets.size === 0) return;
  const { db } = openDatabase({ path: run.paths.db, timeoutMs: 2_000, hook: true });
  try {
    const active = db.prepare(`SELECT DISTINCT session_id FROM raw_events WHERE batch_id IS NULL AND processing_state = 'pending'
      AND ${SUMMARIZABLE_ROW_SQL} AND session_id IN (SELECT id FROM sessions WHERE status = 'active')`).all();
    if (active.some((row) => !run.fixtureSessions.has(String(row.session_id)))) throw new ReplayFailure(1, 'foreign_active_sources');
    if (active.length > 0) return;
  } finally { db.close(); }
  dropLease(run);
  startWorker(run);
  if (run.liveObserve === undefined) throw new ReplayFailure(1, 'worker_missing');
  await waitForReplaySettlement(run.paths.db, run.repoId, [...run.endedTargets], run.liveObserve);
  harvestRss(run);
  run.endedTargets.clear();
  await ensureLeaseHeld(run);
}

async function settleWorker(run: ReplayRun): Promise<void> {
  if (run.leaseFailure !== null) throw run.leaseFailure;
  await observeNow(run);
  if (run.endedTargets.size > 0) throw new ReplayFailure(1, 'active_sources_unsettled');
  dropLease(run);
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
    run.repoId = resolveRepoIdentity(run.repo).id;
    mkdirSync(home, { recursive: true, mode: 0o700 });
    ensureDirectories(run.paths);
    const created = createDatabase(run);
    if (created !== null) return created;
    const dbBytesBefore = fileBytes(run.paths.db) + fileBytes(`${run.paths.db}-wal`);
    workerPollDb = openDatabase({ path: run.paths.db, timeoutMs: 0, hook: true }).db;
    const workerPid = workerPollDb.prepare('SELECT pid FROM worker_lease WHERE id = 1');
    let heartbeatAt = 0;
    workerPoll = setInterval(() => {
      pollHookWorker(run, workerPid);
      if (run.leaseToken === null || run.leaseFailure !== null || workerPollDb === undefined) return;
      if (Date.now() - heartbeatAt < 1_000) return;
      try {
        if (!heartbeat(workerPollDb, run.leaseToken, Date.now())) run.leaseFailure = new ReplayFailure(1, 'lease_lost');
        heartbeatAt = Date.now();
      } catch (error) { if (!isBusyError(error)) run.leaseFailure = new ReplayFailure(3, 'storage_error'); }
    }, 50);
    return await driveRun(run, workerPoll, {
      values,
      outPath,
      fixturePath,
      startedAt,
      loadAtStart,
      dbBytesBefore,
    });
  } catch (error) {
    const failure = error instanceof ReplayFailure ? error : new ReplayFailure(3, 'storage_error');
    process.stderr.write(`${failure.message}\n`);
    return failure.exit;
  } finally {
    clearInterval(workerPoll);
    await run.liveObserve?.stop();
    if (run.leaseToken !== null) {
      try { dropLease(run); } catch { /* A successor owns its lease; cleanup must leave it intact. */ }
    }
    workerPollDb?.close();
    cleanupReplay({ home, repo: run.repo, keep, createdHome });
  }
}

/**
 * The fixture lines, the worker settle, and the report. Stops the 50 ms lease poll before the
 * measurement reads the worker's high-water mark; `runFixture`'s `finally` clears it on every
 * other path and closes the database the poll reads.
 */
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
  if (run.leaseFailure !== null) throw run.leaseFailure;

  if (run.storageFailed) {
    process.stderr.write('observe reported unusable storage\n');
    return 3;
  }

  const measured = measureRun(run, { dbBytesBefore, fixturePath, startedAt, loadAtStart });
  writeReport(values, outPath, measured);
  return measured.failed ? 1 : 0;
}

export type MeasureInput = {
    repoId: string;
    lines: Line[];
    captureSamples: Sample[];
    injectionSamples: Sample[];
    readySamples: Sample[];
    pendingSamples: Sample[];
    startSamples: StartSample[];
    sizeRows: SizeRow[];
    packs: ReplayPack[];
      recallProbes: RecallProbe[];
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
