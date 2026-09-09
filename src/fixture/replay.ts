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
import { openDatabase } from '../db/open.js';
import { measure, nativeSessionId, sessionStartEvent } from './replay-evaluate.js';
import { HEADING, fileBytes, repositoryRoot } from './replay-report.js';
import { childEnvironment } from '../log.js';
import { ensureDirectories, oboetePaths } from '../paths.js';
import { isLeaseFree } from '../worker/lease.js';

const ROOT_PH = '__OBOETE_REPLAY_ROOT__';
const FILL_ALPHABET = 'The quick brown fox jumps over the lazy dog. ';
const AT_BOUND = 1_048_576;
const ABOVE_ONE = 1_048_577;
const ABOVE_TWO = 2_097_152;
const SESSION_END = new Set(['SessionEnd', 'session_shutdown']);
const AGENTS = ['claude', 'codex', 'grok', 'pi'] as const;
const LEASE_TOKEN = 't068-replay';

export type Agent = (typeof AGENTS)[number];
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
export type Sample = { agent: Agent; event: string; seq: number; session: string; ms: number };
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
  // Every fact the fixture plants, indexed before the first hook so recall can look one up.
  for (const line of input.lines) {
    const fact = line.tags?.fact;
    if (fact !== undefined) run.factsById.set(fact.id, fact);
  }
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
  };
  return JSON.stringify({
    cwd: typeof envelope.cwd === 'string' ? envelope.cwd : run.repo,
    session_id: typeof envelope.session_id === 'string' ? envelope.session_id : nativeSessionId('pi', payload),
    prompt: typeof envelope.payload?.text === 'string' ? envelope.payload.text : undefined,
    model: typeof envelope.model === 'string' ? envelope.model : undefined,
  });
}

/** Runs `oboete inject` for one Pi line and files its pack and its sample like a hook's. */
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

/**
 * One fixture line: the hook, the Pi injection it also drives, and the checks its tags ask
 * for. `null` continues the replay; a number is the exit code the replay stops with.
 */
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

  if (run.storageFailed) {
    process.stderr.write('observe reported unusable storage\n');
    return 3;
  }

  const measured = measureRun(run, { dbBytesBefore, fixturePath, startedAt, loadAtStart });
  writeReport(values, outPath, measured);
  return measured.failed ? 1 : 0;
}

export type MeasureInput = {
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
