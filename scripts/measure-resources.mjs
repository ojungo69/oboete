#!/usr/bin/env node
// T042 / SC-008: retained-history resource run. Product checkpoints only; doctor generation predicate.
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { setTimeout as sleep } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
const ROOT = resolve(fileURLToPath(new URL('..', import.meta.url)));
const BUNDLE = join(ROOT, 'dist', 'oboete.mjs');
const CONFIG = '[observer]\npreset = "none"\n\n[worker]\nidle_exit_ms = 60000\n';
const SAMPLE_MS = 250, RSS_BOUND_KIB = 150 * 1024, WAL_FRACTION = 0.25;
const HOOK_TIMEOUT_MS = 15_000, REPLAY_TIMEOUT_MS = 40 * 60_000;
const PENDING_TIMEOUT_MS = 3 * 60_000, STOPPED_TIMEOUT_MS = 2 * 60_000, BATCH_WAIT_MS = 60_000, DOCTOR_POLL_MS = 5_000;
const GATED = new Set(['hooks', 'SC-010', 'lifecycle', 'SC-003']);
const NO_MODEL = new Set(['SC-009', 'session start']);
const STAGES = ['hold', 'drain', 'stop', 'final'];
const environHit = new Map();
class HarnessError extends Error { constructor(message) { super(message); this.name = 'HarnessError'; } }
function usage() {
  return 'Usage: node scripts/measure-resources.mjs [--fixture test/fixtures/events-1000.jsonl] [--json-out <path>] [--sessions 20] [--prompts 9] [--hold-ms 20000] [--keep] [--self-check]\n';
}
function intOption(values, name, fallback) {
  const n = Number(values[name] ?? fallback);
  if (!Number.isInteger(n) || n <= 0) throw new HarnessError(`invalid --${name}: ${values[name] ?? fallback}`);
  return n;
}
function parseCli(argv) {
  let v;
  try {
    v = parseArgs({
      args: argv, strict: true,
      options: {
        fixture: { type: 'string' }, 'json-out': { type: 'string' },
        sessions: { type: 'string', default: '20' }, prompts: { type: 'string', default: '9' },
        'hold-ms': { type: 'string', default: '20000' }, keep: { type: 'boolean', default: false },
        'self-check': { type: 'boolean', default: false },
      },
    }).values;
  } catch (error) { throw new HarnessError(`${error instanceof Error ? error.message : String(error)}\n${usage()}`); }
  return {
    fixture: resolve(v.fixture ?? join(ROOT, 'test', 'fixtures', 'events-1000.jsonl')),
    jsonOut: v['json-out'] === undefined ? undefined : resolve(v['json-out']),
    sessions: intOption(v, 'sessions', 20), prompts: intOption(v, 'prompts', 9),
    holdMs: intOption(v, 'hold-ms', 20_000), keep: v.keep === true, selfCheck: v['self-check'] === true,
  };
}
function childEnv(paths) {
  return { PATH: `${paths.bin}:/usr/bin:/bin`, HOME: paths.userHome, OBOETE_HOME: paths.oboeteHome, TMPDIR: paths.tmp, NODE_ENV: 'test' };
}
function gitHead() {
  const r = spawnSync('git', ['-C', ROOT, 'rev-parse', '--short', 'HEAD'], { encoding: 'utf8', env: { PATH: '/usr/bin:/bin', HOME: ROOT, GIT_CONFIG_NOSYSTEM: '1' } });
  return r.status === 0 ? r.stdout.trim() : 'unknown';
}
function loadAverage() { try { return readFileSync('/proc/loadavg', 'utf8').trim(); } catch { return 'unavailable'; } }
function fileBytes(path) { try { return statSync(path).size; } catch { return 0; } }
function spoolCount(dir) {
  try { return readdirSync(dir, { withFileTypes: true }).filter((e) => e.isFile() && e.name.endsWith('.json')).length; } catch { return 0; }
}
function isBusy(error) {
  if (error === null || typeof error !== 'object') return false;
  return error.errcode === 5 || error.errcode === 6 || /database is locked|SQLITE_BUSY|SQLITE_LOCKED|\bbusy\b/i.test(`${error.errstr ?? ''} ${error instanceof Error ? error.message : error}`);
}
function readVm(pid) {
  try {
    const text = readFileSync(`/proc/${pid}/status`, 'utf8');
    const n = (key) => Number((text.match(new RegExp(`${key}:\\s+(\\d+)\\s+kB`)) ?? [])[1] ?? 0);
    return { rssKb: n('VmRSS'), hwmKb: n('VmHWM') };
  } catch { return null; }
}
function walkHome(home, fn) {
  const needle = `OBOETE_HOME=${home}`;
  let names = [];
  try { names = readdirSync('/proc'); } catch { return; }
  const live = new Set();
  for (const name of names) {
    if (!/^[0-9]+$/.test(name)) continue;
    const pid = Number(name);
    if (pid === process.pid) continue;
    live.add(pid);
    let hit = environHit.get(pid);
    if (hit === undefined) {
      try { hit = readFileSync(`/proc/${pid}/environ`).toString().split('\0').includes(needle); } catch { continue; }
      // Only a hit is cached: a child read before it adopted the temp environment must be retried.
      if (hit) environHit.set(pid, hit);
    }
    if (hit) fn(pid);
  }
  for (const pid of [...environHit.keys()]) { if (!live.has(pid)) environHit.delete(pid); }
}
function pidsInHome(home) { const pids = []; walkHome(home, (pid) => pids.push(pid)); return pids; }
function sampleHome(paths, at = Date.now()) {
  const processes = [];
  walkHome(paths.oboeteHome, (pid) => {
    const vm = readVm(pid);
    if (vm === null) return;
    let observe = false;
    try { observe = readFileSync(`/proc/${pid}/cmdline`).toString().split('\0').includes('observe'); } catch { observe = false; }
    processes.push({ pid, rssKb: vm.rssKb, hwmKb: vm.hwmKb, observe });
  });
  return {
    at, dbBytes: fileBytes(paths.db), walBytes: fileBytes(`${paths.db}-wal`), spool: spoolCount(paths.spool),
    rssKb: processes.reduce((m, p) => Math.max(m, p.rssKb), 0), hwmKb: processes.reduce((m, p) => Math.max(m, p.hwmKb), 0), processes,
  };
}
function startSampler(paths, sink, stageOf) {
  const tick = () => { sink.push({ ...sampleHome(paths), stage: stageOf() }); };
  tick();
  const timer = setInterval(tick, SAMPLE_MS);
  return { stop() { clearInterval(timer); tick(); } };
}
function median(values) {
  if (values.length === 0) return 0;
  const s = [...values].sort((a, b) => a - b); const mid = Math.floor(s.length / 2);
  return s.length % 2 === 0 ? (s[mid - 1] + s[mid]) / 2 : s[mid];
}
function seriesStats(samples, key) {
  const values = samples.map((s) => s[key]).filter((v) => typeof v === 'number');
  return values.length === 0 ? { min: 0, median: 0, max: 0, n: 0 } : { min: Math.min(...values), median: median(values), max: Math.max(...values), n: values.length };
}
function pidStats(samples) {
  const map = new Map();
  for (const sample of samples) {
    for (const proc of sample.processes ?? []) {
      const cur = map.get(proc.pid);
      if (cur === undefined) map.set(proc.pid, { pid: proc.pid, firstRss: proc.rssKb, lastRss: proc.rssKb, n: 1, maxHwm: proc.hwmKb, observe: proc.observe === true });
      else { cur.lastRss = proc.rssKb; cur.n += 1; cur.maxHwm = Math.max(cur.maxHwm, proc.hwmKb); cur.observe = cur.observe || proc.observe === true; }
    }
  }
  return [...map.values()];
}
function writeConfig(home) {
  for (const dir of [home, join(home, 'spool'), join(home, 'logs')]) mkdirSync(dir, { recursive: true, mode: 0o700 });
  writeFileSync(join(home, 'config.toml'), CONFIG, { mode: 0o600 });
}
function spawnWait(file, args, { env, cwd, timeoutMs, stdin, inheritStderr }) {
  const t0 = Date.now();
  return new Promise((resolvePromise, reject) => {
    const child = spawn(file, args, {
      cwd, env, stdio: [stdin === undefined ? 'ignore' : 'pipe', 'pipe', 'pipe'],
    });
    let stdout = ''; let stderr = '';
    child.stdout?.setEncoding('utf8');
    child.stdout?.on('data', (chunk) => { stdout += chunk; });
    child.stderr?.setEncoding('utf8');
    child.stderr?.on('data', (chunk) => { stderr += chunk; if (inheritStderr === true) process.stderr.write(chunk); });
    const timer = setTimeout(() => { child.kill('SIGTERM'); setTimeout(() => child.kill('SIGKILL'), 2_000).unref(); }, timeoutMs);
    child.on('error', (error) => { clearTimeout(timer); reject(error); });
    child.stdin?.on('error', () => {});
    if (stdin !== undefined) child.stdin?.end(stdin);
    child.on('close', (status, signal) => {
      clearTimeout(timer);
      resolvePromise({ status, signal, stdout, stderr, timedOut: status === null && signal !== null, ms: Date.now() - t0 });
    });
  });
}
function parseJsonStdout(text) {
  const trimmed = text.trim();
  if (trimmed === '') return null;
  try { return JSON.parse(trimmed); } catch { /* slice */ }
  const start = trimmed.indexOf('{'); const end = trimmed.lastIndexOf('}');
  if (start >= 0 && end > start) { try { return JSON.parse(trimmed.slice(start, end + 1)); } catch { return null; } }
  return null;
}
function claudePayload(repo, sessionId, event, extra) {
  return { session_id: sessionId, transcript_path: join(repo, '.oboete-t042', `${sessionId}.jsonl`), cwd: repo, permission_mode: 'bypassPermissions', hook_event_name: event, ...extra };
}
async function runHook(env, repo, event, payload) {
  return spawnWait(process.execPath, [BUNDLE, 'hook', '--agent', 'claude-or-grok', '--event', event], { env, cwd: repo, timeoutMs: HOOK_TIMEOUT_MS, stdin: JSON.stringify(payload) });
}
function hookFailed(row) { return row.timedOut || row.signal !== null || row.status !== 0; }
async function runSessions(env, repo, cli, runId) {
  const origin = Date.now();
  const gap = cli.sessions <= 1 ? 0 : cli.holdMs / cli.sessions;
  const hooks = []; const sessionIds = []; const markers = [];
  await Promise.all(Array.from({ length: cli.sessions }, async (_, s) => {
    const wait = origin + gap * s - Date.now();
    if (wait > 0) await sleep(wait);
    const sessionId = `t042-${runId}-s${s}`;
    sessionIds[s] = sessionId;
    const push = async (event, extra) => {
      hooks.push({ event, session: s, ...(await runHook(env, repo, event, claudePayload(repo, sessionId, event, extra))) });
    };
    await push('SessionStart', { source: 'startup' });
    for (let i = 0; i < cli.prompts; i += 1) {
      const marker = `t042:${runId}:${s}:${i};`;
      markers.push(marker);
      await push('UserPromptSubmit', { prompt_id: marker, prompt: `${marker} concurrent capture` });
    }
    await push('Stop', { prompt_id: `${sessionId}-stop`, stop_hook_active: false, last_assistant_message: 'done' });
  }));
  return { hooks, sessionIds, markers };
}
function holdReader(dbPath) {
  const db = new DatabaseSync(dbPath, { readOnly: true, timeout: 2_000 });
  db.exec('BEGIN');
  db.prepare('SELECT 1 AS ok FROM raw_events LIMIT 1').get();
  let open = true;
  return {
    release() {
      if (!open) return;
      open = false;
      try { if (db.isTransaction) db.exec('COMMIT'); } catch { try { if (db.isTransaction) db.exec('ROLLBACK'); } catch { /* close drops snapshot */ } }
      db.close();
    },
  };
}
function openRo(path, timeout) { return new DatabaseSync(path, { readOnly: true, timeout }); }
function copyObserveLog(home, jsonOut) {
  if (jsonOut === undefined) return;
  try { writeFileSync(`${jsonOut.replace(/\.json$/i, '')}.observe.log`, readFileSync(join(home, 'logs', 'observe.log'))); } catch { /* no log */ }
}
function findReplayRepo(json, stderr, tmp) {
  const fromJson = typeof json?.repo === 'string' ? json.repo : undefined;
  const fromKeep = (stderr.match(/^kept home=\S+ repo=(\S+)\s*$/m) ?? [])[1];
  const reported = fromJson ?? fromKeep;
  if (reported !== undefined && existsSync(reported)) return { repo: reported, source: fromJson !== undefined ? 'replay-json' : 'replay-keep-stderr' };
  let names = [];
  try { names = readdirSync(tmp, { withFileTypes: true }); } catch { /* empty */ }
  const dirs = names.filter((e) => e.isDirectory() && e.name.startsWith('oboete-t068-repo-')).map((e) => join(tmp, e.name));
  if (dirs.length === 1) return { repo: dirs[0], source: 'replay-tmpdir (JSON has repoId only; replayArgv has --keep and no --repo)' };
  throw new HarnessError(`phase A --keep did not yield a repository path (replayArgv has no --repo); tmp=${dirs.join('|') || 'none'}`);
}
function endReasonFrom(text, afterMs = 0) {
  let last = null, lastAfter = null;
  for (const m of text.matchAll(/^(\S+) \S+ run end\b.*\breason=(\S+)/gm)) {
    last = m[2];
    const at = Date.parse(m[1]);
    if (Number.isFinite(at) && at >= afterMs) lastAfter = m[2];
  }
  return lastAfter ?? last;
}
// `error batch ... state=error` is a failure, not the overlap this looks for (src/worker/observe.ts).
function countBatchLines(text, fromMs, toMs) {
  let n = 0;
  for (const m of text.matchAll(/^(\S+) info batch\b/gm)) {
    const at = Date.parse(m[1]);
    if (Number.isFinite(at) && at >= fromMs && at <= toMs) n += 1;
  }
  return n;
}
function countErrorLines(text, fromMs) {
  let n = 0;
  for (const m of text.matchAll(/^(\S+) error\b/gm)) {
    const at = Date.parse(m[1]);
    if (Number.isFinite(at) && at >= fromMs) n += 1;
  }
  return n;
}
/** A run that ended for any other reason after the hold began is a worker failure this must fail on. */
function badEndReasons(text, fromMs) {
  const benign = new Set(['stopped', 'idle_exit', 'another_worker', 'empty']);
  const bad = [];
  for (const m of text.matchAll(/^(\S+) \S+ run end\b.*\breason=(\S+)/gm)) {
    const at = Date.parse(m[1]);
    if (Number.isFinite(at) && at >= fromMs && !benign.has(m[2])) bad.push(m[2]);
  }
  return bad;
}
async function waitHeldBatch(logPath, fromMs) {
  const deadline = Date.now() + BATCH_WAIT_MS;
  while (Date.now() < deadline) {
    let text = '';
    try { text = readFileSync(logPath, 'utf8'); } catch { /* missing */ }
    if (countBatchLines(text, fromMs, Date.now()) > 0) return;
    await sleep(SAMPLE_MS);
  }
  throw new HarnessError('held reader never overlapped a worker batch');
}
async function readDoctor(env, cwd) {
  const items = parseJsonStdout((await spawnWait(process.execPath, [BUNDLE, 'doctor', '--json'], { env, cwd, timeoutMs: 60_000 })).stdout)?.items ?? [];
  const generation = items.find((item) => item.item === 'generation');
  const worker = items.find((item) => item.item === 'worker');
  if (generation === undefined) throw new HarnessError('doctor --json missing generation item');
  const stuck = parseGeneration(generation.reason ?? '');
  if (!Number.isFinite(stuck.pending)) throw new HarnessError('doctor generation item missing pending count');
  return { generation, worker, stuck };
}
async function waitPending(env, cwd, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let last = 'unread';
  while (Date.now() < deadline) {
    try {
      const doc = await readDoctor(env, cwd);
      last = doc.generation.reason ?? '';
      if (doc.stuck.pending === 0) return;
    } catch (error) { last = error instanceof Error ? error.message : String(error); }
    await sleep(DOCTOR_POLL_MS);
  }
  throw new HarnessError(`timed out waiting for doctor generation pending=0; last=${last}`);
}
async function waitGone(home, dbPath, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (pidsInHome(home).length === 0) {
      try {
        const db = openRo(dbPath, 50);
        try {
          const row = db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get();
          if (row === undefined || row.owner_token == null) return;
        } finally { db.close(); }
      } catch (error) { if (!isBusy(error)) throw error; }
    }
    await sleep(SAMPLE_MS);
  }
  throw new HarnessError(`timed out waiting for home processes to exit and worker_lease to have no live owner after observe --stop; leftover=${pidsInHome(home).join('|') || 'none'}`);
}
async function stopHomeProcesses(home) {
  for (const pid of pidsInHome(home)) { try { process.kill(pid, 'SIGTERM'); } catch { /* gone */ } }
  const deadline = Date.now() + 5_000;
  while (Date.now() < deadline && pidsInHome(home).length > 0) await sleep(100);
  for (const pid of pidsInHome(home)) { try { process.kill(pid, 'SIGKILL'); } catch { /* gone */ } }
}
function liveBatches(dbPath) {
  try { const db = openRo(dbPath, 2_000); try { return Number(db.prepare("SELECT COUNT(*) AS n FROM observation_batches WHERE state IN ('pending', 'running')").get()?.n ?? 0); } finally { db.close(); } } catch { return -1; }
}
function loadHits(dbPath, spoolDir, markers, sessionIds) {
  const db = openRo(dbPath, 2_000);
  let rows;
  try {
    rows = db.prepare(
      `SELECT r.content AS content, r.classification_state AS classification_state, r.kind AS kind, s.native_session_id AS native_session_id, json_extract(r.payload_json, '$.prompt_id') AS prompt_id FROM raw_events r JOIN sessions s ON s.id = r.session_id`,
    ).all();
  } finally { db.close(); }
  const spoolTexts = [];
  if (existsSync(spoolDir)) {
    for (const name of readdirSync(spoolDir)) {
      if (!name.endsWith('.json')) continue;
      try { spoolTexts.push(readFileSync(join(spoolDir, name), 'utf8')); } catch { /* skip */ }
    }
  }
  return {
    markers: markers.map((id) => ({
      id,
      hits: rows.filter((row) => (typeof row.content === 'string' && row.content.includes(id)) || (row.classification_state === 'failed' && row.prompt_id === id)).map((row) => ({ classification_state: row.classification_state })),
      spool: spoolTexts.some((text) => text.includes(id)),
    })),
    sessions: sessionIds.map((id) => ({
      id, starts: rows.filter((row) => row.native_session_id === id && row.kind === 'session_start').length,
      ends: rows.filter((row) => row.native_session_id === id && row.kind === 'session_end').length,
    })),
    spoolFiles: spoolTexts.length,
  };
}
function parseGeneration(reason) {
  const n = (re) => { const m = reason.match(re); return m === null ? Number.NaN : Number(m[1]); };
  return { pending: n(/(\d+) pending/), waiting: n(/(\d+) waiting/), parked: n(/(\d+) parked/), legacy: n(/(\d+) legacy sources held/), processed: n(/(\d+) processed/) };
}
function checkRetained(input) {
  const missing = []; const duplicate = []; const failed = [];
  for (const marker of input.markers) {
    const good = marker.hits.filter((hit) => hit.classification_state !== 'failed');
    const bad = marker.hits.filter((hit) => hit.classification_state === 'failed');
    if (good.length === 1) continue;
    if (good.length > 1) duplicate.push(marker.id);
    else if (marker.spool) continue;
    else if (bad.length > 0) failed.push(marker.id);
    else missing.push(marker.id);
  }
  const sessionFail = input.sessions.filter((s) => s.starts !== 1 || s.ends !== 1).map((s) => s.id);
  return { name: 'retained', pass: missing.length === 0 && duplicate.length === 0 && failed.length === 0 && sessionFail.length === 0, missing, duplicate, failed, sessionFail, spoolFiles: input.spoolFiles };
}
function checkNotStuck(input) {
  return { name: 'not-stuck', pass: input.pending === 0 && input.spoolFiles === 0 && input.liveBatches === 0 && input.workerErrors === 0 && (input.badEnds ?? []).length === 0 && (input.endReason === 'stopped' || input.endReason === 'idle_exit'), ...input, note: 'with preset none, work moves to waiting as deferred no_provider' };
}
function checkWal(input) {
  const grew = input.peak > input.start;
  return { name: 'wal-recycled', pass: grew && input.final <= input.peak * WAL_FRACTION, ...input, grew, fraction: WAL_FRACTION };
}
function checkRss(input) {
  const maxHwm = Math.max(input.phaseAHwmKb, input.phaseB.reduce((m, p) => Math.max(m, p.maxHwm), 0));
  return { name: 'rss-bound', pass: maxHwm <= RSS_BOUND_KIB, maxHwm, ...input, boundKib: RSS_BOUND_KIB, note: "20-second window cannot show long-run growth; that is issue #268's seven-day run" };
}
function selfCheck() {
  const hit = (id, state, n = 1, spool = false) => ({ id, hits: Array.from({ length: n }, () => ({ classification_state: state })), spool });
  const sess = [{ id: 's0', starts: 1, ends: 1 }];
  const stuckOk = { pending: 0, waiting: 4, parked: 0, legacy: 0, processed: 1, spoolFiles: 0, liveBatches: 0, endReason: 'stopped', workerErrors: 0, badEnds: [] };
  assert.equal(checkRetained({ markers: [hit('a', 'done'), hit('b', 'done')], sessions: [...sess, { id: 's1', starts: 1, ends: 1 }], spoolFiles: 0 }).pass, true);
  const mixed = checkRetained({ markers: [hit('miss', 'done', 0), hit('dup', 'done', 2)], sessions: sess, spoolFiles: 0 });
  assert.equal(mixed.pass, false); assert.deepEqual(mixed.missing, ['miss']); assert.deepEqual(mixed.duplicate, ['dup']);
  const classified = checkRetained({ markers: [hit('fail', 'failed')], sessions: sess, spoolFiles: 0 });
  assert.equal(classified.pass, false); assert.deepEqual(classified.failed, ['fail']);
  assert.equal(checkRetained({ markers: [hit('a', 'done')], sessions: [{ id: 's0', starts: 1, ends: 0 }], spoolFiles: 0 }).pass, false);
  assert.equal(checkRetained({ markers: [hit('a', 'done')], sessions: [{ id: 's0', starts: 2, ends: 1 }], spoolFiles: 0 }).pass, false);
  assert.equal(checkNotStuck(stuckOk).pass, true);
  assert.equal(checkNotStuck({ ...stuckOk, pending: 1 }).pass, false);
  assert.equal(checkNotStuck({ ...stuckOk, workerErrors: 1 }).pass, false);
  assert.equal(checkNotStuck({ ...stuckOk, badEnds: ['batch_error'] }).pass, false);
  assert.equal(countBatchLines('2026-01-01T00:00:00.000Z error batch id=x state=error\n', 0, Date.now()), 0);
  assert.equal(countErrorLines('2026-01-01T00:00:00.000Z error batch id=x state=error\n', 0), 1);
  assert.deepEqual(badEndReasons('2026-01-01T00:00:00.000Z info run end exit=1 reason=batch_error\n', 0), ['batch_error']);
  assert.deepEqual(badEndReasons('2026-01-01T00:00:00.000Z info run end exit=0 reason=stopped\n', 0), []);
  assert.equal(checkWal({ start: 100, peak: 800, final: 0 }).pass, true);
  assert.equal(checkWal({ start: 100, peak: 100, final: 0 }).pass, false);
  assert.equal(checkWal({ start: 100, peak: 100, final: 100 }).pass, false);
  assert.equal(checkWal({ start: 0, peak: 1000, final: 300 }).pass, false);
  const pid = (hwm) => [{ pid: 1, firstRss: 1, lastRss: 1, n: 1, maxHwm: hwm }];
  assert.equal(checkRss({ phaseAHwmKb: 12_000, phaseB: pid(13_000) }).pass, true);
  assert.equal(checkRss({ phaseAHwmKb: 151 * 1024, phaseB: pid(100) }).pass, false);
  assert.equal(checkRss({ phaseAHwmKb: 100, phaseB: pid(151 * 1024) }).pass, false);
  assert.equal(endReasonFrom('2026-01-01T00:00:00.000Z info run end exit=0 reason=empty\n2026-01-01T00:00:01.000Z info run end exit=0 reason=stopped\n'), 'stopped');
  assert.equal((('kept home=/h repo=/tmp/oboete-t068-repo-abc\n').match(/^kept home=\S+ repo=(\S+)\s*$/m) ?? [])[1], '/tmp/oboete-t068-repo-abc');
  assert.throws(() => replayGates({ worker: { rssKb: '1' } }), HarnessError);
  const a0 = { gated: { hooks: false, duplicates: false, lifecycle: false, worker: false }, failed: [], notGated: [], bounds: [], rssKb: 0, dbBytes: 0, walBytes: 0, repo: '', repoSource: '' };
  const md = renderMarkdown({ checks: null, error: 'held-batch', failed: true, startedAt: '', node: '', commit: '', fixture: '', fixtureLines: 0, loadAtStart: '', sessions: 1, prompts: 1, holdMs: 1, phaseA: a0, phaseB: { samples: [], hooks: [], hookErrors: [], stopMarker: false, exitReason: null } });
  assert.match(md, /\| retained \| not run \| held-batch \|/);
  assert.match(md, /\| not-stuck \| not run \| held-batch \|/);
  assert.match(md, /\| wal-recycled \| not run \| held-batch \|/);
  assert.match(md, /\| rss-bound \| not run \| held-batch \|/);
  assert.doesNotMatch(md, /\| retained \| (pass|fail) \|/);
}
function replayGates(json) {
  if (json === null || typeof json !== 'object') throw new HarnessError('phase A replay produced no JSON');
  const lifecyclePass = Array.isArray(json.lifecycle)
    ? json.lifecycle.length > 0 && json.lifecycle.every((row) => row.pass === true)
    : (json.bounds ?? []).find((row) => row.sc === 'lifecycle')?.status === 'pass';
  const gated = {
    hooks: json.hooks?.pass === true, duplicates: json.duplicates?.pass === true,
    lifecycle: lifecyclePass === true, worker: json.worker?.pass === true,
  };
  const failed = Object.entries(gated).filter(([, pass]) => !pass).map(([name]) => name);
  const notGated = [];
  for (const row of json.bounds ?? []) {
    if (row.status !== 'fail' || GATED.has(row.sc)) continue;
    notGated.push({ sc: row.sc, measured: row.measured, bound: row.bound, reason: NO_MODEL.has(row.sc) ? 'no model (preset none)' : 'not gated' });
  }
  const rssKb = json.worker?.rssKb;
  if (typeof rssKb !== 'number' || !Number.isFinite(rssKb)) throw new HarnessError('phase A replay JSON worker.rssKb is not a number');
  return { gated, failed, notGated, rssKb, bounds: json.bounds ?? [] };
}
function mdTable(headers, rows) {
  return `| ${headers.join(' | ')} |\n|${headers.map(() => '---').join('|')}|\n${rows.map((row) => `| ${row.join(' | ')} |`).join('\n')}`;
}
function kibToMib(kib) { return (kib / 1024).toFixed(3); }
function checkRows(report) {
  const c = report.checks, yn = (v) => (v ? 'pass' : 'fail');
  if (c == null) {
    const err = report.error ?? 'unread';
    return ['retained', 'not-stuck', 'wal-recycled', 'rss-bound'].map((name) => [name, 'not run', err]);
  }
  return [
    ['retained', yn(c.retained.pass), `missing=${c.retained.missing.join(',') || 'none'} duplicate=${c.retained.duplicate.join(',') || 'none'} failed-classification=${c.retained.failed.join(',') || 'none'} sessionFail=${c.retained.sessionFail.join(',') || 'none'} spoolFiles=${c.retained.spoolFiles}`],
    ['not-stuck', yn(c.notStuck.pass), `pending=${c.notStuck.pending} waiting=${c.notStuck.waiting} parked=${c.notStuck.parked} legacy=${c.notStuck.legacy} processed=${c.notStuck.processed} spoolFiles=${c.notStuck.spoolFiles} liveBatches=${c.notStuck.liveBatches} endReason=${c.notStuck.endReason ?? 'unread'} workerErrors=${c.notStuck.workerErrors} badEnds=${(c.notStuck.badEnds ?? []).join(',') || 'none'}. ${c.notStuck.note}`],
    ['wal-recycled', yn(c.wal.pass), `start=${c.wal.start} peak=${c.wal.peak} final=${c.wal.final} grew=${c.wal.grew} fraction=${c.wal.fraction} batchesHeld=${c.wal.batchesHeld ?? 0}`],
    ['rss-bound', yn(c.rss.pass), `maxHwm=${c.rss.maxHwm} KiB (${kibToMib(c.rss.maxHwm)} MiB) bound=${c.rss.boundKib} KiB; phase A ${c.rss.phaseAHwmKb} KiB. ${c.rss.note}`],
  ];
}
function renderMarkdown(report) {
  const { checks: c, phaseA: a, phaseB: b } = report;
  const hookMs = b.hooks.map((h) => h.ms), failedHooks = b.hooks.filter(hookFailed);
  const holdSamples = b.samples.filter((s) => s.stage === 'hold');
  const series = [['memory.db bytes', 'dbBytes'], ['memory.db-wal bytes', 'walBytes'], ['spool files', 'spool'], ['VmRSS KiB', 'rssKb'], ['VmHWM KiB', 'hwmKb']].map(([name, key]) => {
    const s = seriesStats(holdSamples, key); return [name, String(s.n), String(s.min), String(s.median), String(s.max)];
  });
  const stageMax = STAGES.map((stage) => { const ss = b.samples.filter((s) => s.stage === stage); return [stage, String(ss.length), String(ss.reduce((m, s) => Math.max(m, s.hwmKb ?? 0), 0)), String(ss.reduce((m, s) => Math.max(m, s.walBytes ?? 0), 0))]; });
  const pidRows = (c == null ? pidStats(b.samples) : c.rss.phaseB).map((p) => [String(p.pid), String(p.firstRss), String(p.lastRss), String(p.n), String(p.maxHwm), p.observe ? 'observe' : 'other']);
  const notGated = a.notGated.length === 0 ? 'None.' : a.notGated.map((row) => `- ${row.sc}: ${row.measured} (${row.reason}).`).join('\n');
  const bounds = a.bounds.length === 0 ? 'No bounds table in replay JSON.' : mdTable(['SC', 'Measured', 'Bound', 'Status'], a.bounds.map((row) => [row.sc, String(row.measured), String(row.bound), row.status]));
  const hookFail = failedHooks.length === 0 ? '' : mdTable(['event', 'session', 'status', 'ms'], failedHooks.map((h) => [h.event, String(h.session ?? ''), h.timedOut ? 'timeout' : String(h.status), String(h.ms)]));
  const pids = pidRows.length === 0 ? 'No processes sampled.' : mdTable(['pid', 'first VmRSS', 'last VmRSS', 'n', 'max VmHWM', 'role'], pidRows);
  const stopNote = b.stopMarker ? `still present; doctor worker: ${report.workerReason ?? 'unread'}; harness did not clear it` : 'cleared by the product resident on stopped';
  const closing = report.error !== undefined ? `Harness error: ${report.error} Exit 2.` : report.failed ? 'One or more checks failed. Exit 1.' : 'Every listed check passed on this run.';
  return `## Resource measurement (T042 / SC-008)\n\n### Setup\n\n- Date: ${report.startedAt}\n- Node: \`${report.node}\`.\n- Commit: \`${report.commit}\`.\n- Fixture: \`${report.fixture}\` (${report.fixtureLines} lines).\n- Load average at the start of the run: \`${report.loadAtStart}\`.\n- Config:\n\`\`\`toml\n${CONFIG.trim()}\n\`\`\`\n- Phase B: ${report.sessions} sessions, ${report.prompts} prompts, hold ${report.holdMs} ms.\n- Phase A repository: \`${a.repo}\` (${a.repoSource}). Replay JSON reports repoId, not a filesystem path; replayArgv accepts --keep and has no --repo, so a harness-created repository cannot be passed in.\n- Worker exit reason: \`${b.exitReason ?? 'unread'}\` (from logs/observe.log). Expected stopped: observe --stop writes the product sentinel; the resident exits stopped; shutdownResident runs the product's releaseForExit and wal_checkpoint(TRUNCATE).\n- Stop marker after that release: ${stopNote}.\n\n### Phase A replay bounds\n\n${bounds}\n\nGated (must pass): hooks=${a.gated.hooks} duplicates=${a.gated.duplicates} lifecycle=${a.gated.lifecycle} worker=${a.gated.worker}.\nPhase A worker.rssKb (VmHWM): ${a.rssKb} KiB (${kibToMib(a.rssKb)} MiB). memory.db=${a.dbBytes} -wal=${a.walBytes}.\n\nReported-not-gated:\n${notGated}\n\n### Phase B hooks\n\nn=${b.hooks.length} p50=${median(hookMs).toFixed(1)} ms max=${hookMs.length === 0 ? 0 : Math.max(...hookMs)} ms. Non-zero or timeout: ${failedHooks.length}.\n${hookFail}\n\n### Series (phase B hold)\n\n${mdTable(['Series', 'n', 'min', 'median', 'max'], series)}\n\nPer-stage max VmHWM and -wal:\n\n${mdTable(['stage', 'n', 'max VmHWM KiB', 'max -wal bytes'], stageMax)}\n\n### Checks\n\n${mdTable(['Check', 'Status', 'Measured'], checkRows(report))}\n\nPhase B pid VmRSS (first/last) and sample count; growth is not gated:\n\n${pids}\n\n${closing}\nAn interrupted run can leave a detached resident in the temp home.\n`;
}
async function phaseA(cli, paths, env) {
  const result = await spawnWait(process.execPath, [BUNDLE, 'fixture', 'replay', cli.fixture, '--json', '--home', paths.oboeteHome, '--keep'], { env, cwd: ROOT, timeoutMs: REPLAY_TIMEOUT_MS, inheritStderr: true });
  if (result.timedOut) throw new HarnessError('phase A replay timed out');
  if (result.status === 2 || result.status === 3) throw new HarnessError(`phase A replay exited ${result.status}`);
  if (!existsSync(paths.db)) throw new HarnessError('phase A left no memory.db');
  const json = parseJsonStdout(result.stdout);
  const found = findReplayRepo(json, result.stderr, paths.tmp);
  paths.repo = found.repo;
  return { exit: result.status, ms: result.ms, json, dbBytes: fileBytes(paths.db), walBytes: fileBytes(`${paths.db}-wal`), repo: found.repo, repoSource: found.source, ...replayGates(json) };
}
async function phaseB(cli, paths, env, runId, samples) {
  let stage = 'hold', reader, walWitness, holdFrom = 0, holdTo = 0;
  const sampler = startSampler(paths, samples, () => stage);
  try {
    reader = holdReader(paths.db);
    holdFrom = Date.now();
    const sessionRun = await runSessions(env, paths.repo, cli, runId);
    const remaining = cli.holdMs - (Date.now() - holdFrom);
    if (remaining > 0) await sleep(remaining);
    const ends = [];
    for (const sessionId of sessionRun.sessionIds) {
      ends.push({ event: 'SessionEnd', session: sessionId, ...(await runHook(env, paths.repo, 'SessionEnd', claudePayload(paths.repo, sessionId, 'SessionEnd', { reason: 'prompt_input_exit' }))) });
    }
    await waitHeldBatch(join(paths.oboeteHome, 'logs', 'observe.log'), holdFrom);
    holdTo = Date.now();
    reader.release();
    const hooks = [...sessionRun.hooks, ...ends], hookErrors = hooks.filter(hookFailed);
    stage = 'drain';
    await waitPending(env, paths.repo, PENDING_TIMEOUT_MS);
    stage = 'stop';
    // Without a second connection the resident's own close is the last one, and SQLite then deletes
    // the -wal itself: a zero-byte file would say nothing about the product's TRUNCATE checkpoint.
    walWitness = openRo(paths.db, 2_000);
    walWitness.prepare('SELECT 1 AS ok FROM raw_events LIMIT 1').get();
    const stopAt = Date.now();
    const stop = await spawnWait(process.execPath, [BUNDLE, 'observe', '--stop'], { env, cwd: paths.repo, timeoutMs: HOOK_TIMEOUT_MS });
    if (stop.timedOut || stop.status !== 0) throw new HarnessError(`observe --stop failed status=${stop.status} signal=${stop.signal}`);
    await waitGone(paths.oboeteHome, paths.db, STOPPED_TIMEOUT_MS);
    stage = 'final';
    let logText = '';
    try { logText = readFileSync(join(paths.oboeteHome, 'logs', 'observe.log'), 'utf8'); } catch { /* missing */ }
    const batchesHeld = countBatchLines(logText, holdFrom, holdTo);
    if (batchesHeld === 0) throw new HarnessError('held reader never overlapped a worker batch');
    const held = samples.filter((s) => s.stage === 'hold');
    if (!held.some((s) => s.processes.some((p) => p.observe))) throw new HarnessError('no worker process of this home was sampled while the reader was held');
    const walStart = held[0]?.walBytes ?? 0;
    const walFinal = fileBytes(`${paths.db}-wal`);
    return {
      samples, hooks, hookErrors, sessionIds: sessionRun.sessionIds, markers: sessionRun.markers,
      walStart, walPeak: held.reduce((m, s) => Math.max(m, s.walBytes), walStart), walFinal,
      exitReason: endReasonFrom(logText, stopAt), stopMarker: existsSync(join(paths.oboeteHome, 'worker-stop')), batchesHeld,
      workerErrors: countErrorLines(logText, holdFrom), badEnds: badEndReasons(logText, holdFrom),
    };
  } finally {
    try { reader?.release(); } catch { /* released */ }
    try { walWitness?.close(); } catch { /* closed */ }
    sampler.stop();
  }
}
async function runLive(cli) {
  const startedAt = new Date().toISOString(), loadAtStart = loadAverage(), runId = randomUUID().slice(0, 8), samples = [];
  let isolation, paths, workerReason, error;
  let a = { gated: { hooks: false, duplicates: false, lifecycle: false, worker: false }, failed: [], notGated: [], bounds: [], rssKb: 0, dbBytes: 0, walBytes: 0, repo: '', repoSource: '' };
  let b = { samples, hooks: [], hookErrors: [], sessionIds: [], markers: [], walStart: 0, walPeak: 0, walFinal: 0, exitReason: null, stopMarker: false, batchesHeld: 0 };
  let checks = null;
  try {
    if (!existsSync(cli.fixture)) throw new HarnessError(`fixture file not found: ${cli.fixture}`);
    if (!existsSync(BUNDLE)) throw new HarnessError(`engine bundle not found: ${BUNDLE}`);
    isolation = mkdtempSync(join(tmpdir(), 'oboete-t042-'));
    paths = {
      root: isolation, userHome: join(isolation, 'home'), oboeteHome: join(isolation, 'oboete'),
      tmp: join(isolation, 'tmp'), repo: '', db: join(isolation, 'oboete', 'memory.db'), spool: join(isolation, 'oboete', 'spool'),
      bin: join(isolation, 'bin'),
    };
    for (const dir of [paths.userHome, paths.tmp, paths.bin]) mkdirSync(dir, { recursive: true, mode: 0o700 });
    symlinkSync(process.execPath, join(paths.bin, 'node'));
    writeConfig(paths.oboeteHome);
    const env = childEnv(paths);
    a = await phaseA(cli, paths, env);
    b = await phaseB(cli, paths, env, runId, samples);
    const hits = loadHits(paths.db, paths.spool, b.markers, b.sessionIds);
    const doctor = await readDoctor(env, paths.repo);
    workerReason = doctor.worker?.reason;
    checks = {
      retained: checkRetained(hits),
      notStuck: checkNotStuck({ ...doctor.stuck, spoolFiles: spoolCount(paths.spool), liveBatches: liveBatches(paths.db),
        endReason: b.exitReason, workerErrors: b.workerErrors, badEnds: b.badEnds }),
      wal: checkWal({ start: b.walStart, peak: b.walPeak, final: b.walFinal, batchesHeld: b.batchesHeld }),
      rss: checkRss({ phaseAHwmKb: a.rssKb, phaseB: pidStats(samples) }),
    };
  } catch (err) {
    if (!(err instanceof HarnessError)) throw err;
    error = err.message;
    process.stderr.write(`${error}\n`);
  } finally {
    if (isolation !== undefined) {
      await stopHomeProcesses(paths.oboeteHome);
      copyObserveLog(paths.oboeteHome, cli.jsonOut);
      if (cli.keep) process.stderr.write(`kept ${isolation}\n`); else rmSync(isolation, { recursive: true, force: true });
    }
  }
  const failed = error !== undefined || a.failed.length > 0 || b.hookErrors.length > 0 || (checks !== null && Object.values(checks).some((row) => !row.pass));
  return {
    startedAt, node: `${process.execPath} (${process.version})`, commit: gitHead(), fixture: cli.fixture,
    fixtureLines: existsSync(cli.fixture) ? readFileSync(cli.fixture, 'utf8').split('\n').filter((line) => line !== '').length : 0,
    loadAtStart, sessions: cli.sessions, prompts: cli.prompts, holdMs: cli.holdMs, runId, phaseA: a, phaseB: b, checks, failed, error, workerReason,
  };
}
async function main(argv) {
  const cli = parseCli(argv);
  if (cli.selfCheck) { selfCheck(); return 0; }
  const report = await runLive(cli);
  process.stdout.write(renderMarkdown(report));
  if (cli.jsonOut !== undefined) {
    mkdirSync(dirname(cli.jsonOut), { recursive: true });
    writeFileSync(cli.jsonOut, `${JSON.stringify(report, null, 2)}\n`);
  }
  return report.error !== undefined ? 2 : report.failed ? 1 : 0;
}
main(process.argv.slice(2)).then((code) => { process.exit(code); }).catch((error) => {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
  process.exit(2);
});
