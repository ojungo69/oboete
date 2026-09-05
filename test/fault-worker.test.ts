// Worker half of the User Story 2 failure matrix (T059). Sources: quickstart.md "Failure
// injection", spec.md User Story 2 scenario 2, contracts/agents.md (A11, A16, FR-024, session-start
// pack), FR-002, FR-003, R6. Engine defects stay failing for T063.
import assert from 'node:assert/strict';
import { spawn, type ChildProcess } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { existsSync, statSync, writeFileSync } from 'node:fs';
import { createServer, type IncomingMessage } from 'node:http';
import type { AddressInfo } from 'node:net';
import { join } from 'node:path';
import type { TestContext } from 'node:test';

import {
  configSchema,
  consentHash,
  consentTuple,
} from '../src/config.js';
import { utcDay } from '../src/observer/reservation.js';
import { RECLAIM_AFTER_MS } from '../src/worker/batches.js';
import { claimLease, FUTURE_SKEW_MS, STALE_AFTER_MS } from '../src/worker/lease.js';
import {
  BUNDLE,
  childEnv,
  claudePayload,
  fixture,
  observeLog,
  rows,
  scenario,
  seedWorkersAiCatalog,
  SELECTOR,
  spawnEngine,
  spoolFiles,
  withDb,
  type Place,
  type SpawnEngineOptions,
  type SpawnResult,
} from './helpers/fault.js';

const CF: NodeJS.ProcessEnv = {
  OBOETE_CF_API_TOKEN: 'fault-worker-token',
  OBOETE_CF_ACCOUNT_ID: 'fault-worker-account',
};
// A session-end batch makes observe exit on its own. Still bound: a run that idles until maxRunMs
// is a harness kill, not a pass (same rule as T058).
const OBSERVE_MS = 8_000;
const PROVIDER_MS = 15_000;
const POLL_MS = 5_000;

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve!: () => void;
  const promise = new Promise<void>((inner) => {
    resolve = inner;
  });
  return { promise, resolve };
}

async function withDeadline<T>(promise: Promise<T>, ms: number, label: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<T>((_, reject) => {
        timer = setTimeout(() => reject(new Error(`${label} exceeded ${ms} ms`)), ms);
      }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

type ObserveChild = {
  pid: number;
  finished: Promise<{ status: number | null; signal: NodeJS.Signals | null }>;
  kill(signal?: NodeJS.Signals): void;
};

function spawnObserve(place: Place, opts: Omit<SpawnEngineOptions, 'cwd' | 'input'>): ObserveChild {
  const child: ChildProcess = spawn(process.execPath, [BUNDLE, 'observe'], {
    cwd: place.repo,
    env: childEnv({ ...opts, cwd: place.repo }),
    stdio: 'ignore',
  });
  if (child.pid === undefined) assert.fail('observe spawn produced no pid');
  const finished = new Promise<{ status: number | null; signal: NodeJS.Signals | null }>(
    (resolve, reject) => {
      child.once('error', reject);
      child.once('close', (status, signal) => resolve({ status, signal }));
    },
  );
  return {
    pid: child.pid,
    finished,
    kill(signal: NodeJS.Signals = 'SIGKILL') {
      child.kill(signal);
    },
  };
}

/** Async observe. spawnSync would freeze this process's fake provider server. */
async function runObserveAsync(
  place: Place,
  opts: Omit<SpawnEngineOptions, 'cwd' | 'input'>,
  deadlineMs: number,
): Promise<{ status: number | null; signal: NodeJS.Signals | null; elapsedMs: number }> {
  const started = performance.now();
  const child = spawnObserve(place, opts);
  try {
    const exit = await withDeadline(child.finished, deadlineMs, 'observe');
    return { ...exit, elapsedMs: performance.now() - started };
  } catch (error) {
    child.kill('SIGKILL');
    await child.finished.catch(() => undefined);
    throw error;
  }
}

function observe(place: Place, opts: Omit<SpawnEngineOptions, 'home' | 'cwd' | 'input'> = {}): SpawnResult {
  return spawnEngine(['observe'], {
    ...opts,
    home: place.home,
    cwd: place.repo,
    timeoutMs: opts.timeoutMs ?? OBSERVE_MS,
  });
}

function writeObserverConfig(place: Place, preset: 'none' | 'workers-ai'): void {
  const parsed = configSchema.parse({ observer: { preset } });
  if (preset === 'none') {
    writeFileSync(join(place.home, 'config.toml'), '[observer]\npreset = "none"\n');
    return;
  }
  const hash = consentHash(consentTuple(parsed, { ...process.env, ...CF }));
  writeFileSync(
    join(place.home, 'config.toml'),
    `[observer]\npreset = "workers-ai"\n\n[consent]\nhash = "${hash}"\naccepted_at = ${Date.now()}\n`,
  );
}

function holdLease(place: Place): string {
  return withDb(place, (db) => {
    const token = claimLease(db, { pid: process.pid, now: Date.now() });
    if (token === null) assert.fail('expected a held lease so the hook does not spawn observe (R6)');
    return token;
  });
}

function dropLease(place: Place, token?: string): void {
  withDb(place, (db) => {
    if (token !== undefined) {
      db.prepare(
        'UPDATE worker_lease SET owner_token = NULL, pid = NULL WHERE id = 1 AND owner_token = ?',
      ).run(token);
    } else {
      db.prepare('UPDATE worker_lease SET owner_token = NULL, pid = NULL WHERE id = 1').run();
    }
  });
}

function captureHook(place: Place, event: string, payload: unknown): SpawnResult {
  // Seeding is not the SLA under test: runHook's 2×CAPTURE_DEADLINE_MS bound flakes when the
  // serial fault glob is already warm (T058). The real hook binary still has to exit 0.
  const result = spawnEngine(['hook', '--agent', SELECTOR, '--event', event], {
    home: place.home,
    cwd: place.repo,
    input: JSON.stringify(payload),
    timeoutMs: 5_000,
  });
  assert.equal(
    result.status,
    0,
    `hook ${event} exited ${result.status} signal=${result.signal}: ${result.stderr}`,
  );
  assert.equal(result.stdout, '', `capture hook ${event} prints nothing to stdout`);
  return result;
}

const SUMMARIZABLE_PREDICATE = `kind IN ('prompt', 'tool_result', 'last_assistant_message', 'tool_call')
        AND classification_state IS NOT 'failed' AND sensitivity <> 'secret'
        AND TRIM(COALESCE(content, '')) <> ''`;

function seedEndedSession(place: Place): void {
  const held = holdLease(place);
  try {
    captureHook(place, 'PostToolUse', claudePayload(place.repo));
    const stop = claudePayload(place.repo, 'Stop');
    stop.last_assistant_message = 'The upload path now retries once after a failure.';
    captureHook(place, 'Stop', stop);
    captureHook(place, 'SessionEnd', claudePayload(place.repo, 'SessionEnd'));
  } finally {
    dropLease(place, held);
  }
  const n = Number(rows(place, `SELECT COUNT(*) AS n FROM raw_events WHERE ${SUMMARIZABLE_PREDICATE}`)[0]?.n);
  const dump = rows(
    place,
    `SELECT kind, classification_state, LENGTH(COALESCE(content, '')) AS len FROM raw_events`,
  );
  assert.ok(
    n > 0,
    `capture hit its 300 ms deadline under load and blanked the rows — src/capture.ts failedEventRow; events=${JSON.stringify(dump)}`,
  );
  assert.deepEqual(
    spoolFiles(place),
    [],
    `capture hit its 300 ms deadline under load and blanked the rows — src/capture.ts failedEventRow; spool=${JSON.stringify(spoolFiles(place))} events=${JSON.stringify(dump)}`,
  );
}

function seedMemory(place: Place): void {
  withDb(place, (db) => {
    const repo = db.prepare('SELECT id FROM repos').get();
    if (typeof repo?.id !== 'string') assert.fail('expected a captured repository');
    db.prepare(
      `INSERT INTO memories (id, repo_id, type, title, body, cjk_bigrams, material_hash,
         content_hash, sensitivity, review_state, pinned_at, pin_order, created_at)
       VALUES ('m-fault-worker', ?, 'discovery', 'Retry handling',
         'The upload path retries once after a failure.', '', 'mh-fault-worker', 'ch-fault-worker',
         'eligible', 'unreviewed', ?, 0, ?)`,
    ).run(repo.id, Date.now(), Date.now());
  });
}

function setForeignConversationRoot(place: Place): string {
  return withDb(place, (db) => {
    const session = db.prepare('SELECT id FROM sessions').get();
    if (typeof session?.id !== 'string') assert.fail('expected a captured session');
    db.prepare("UPDATE sessions SET conversation_id = 'root-not-me' WHERE id = ?").run(session.id);
    return session.id;
  });
}

function sessionStartPayload(repo: string, source: string): Record<string, unknown> {
  const payload = claudePayload(repo, 'SessionStart');
  payload.source = source;
  payload.model = 'claude-opus-5[1m]';
  return payload;
}

function hookSessionStart(place: Place, source: string): SpawnResult {
  return spawnEngine(['hook', '--agent', SELECTOR, '--event', 'SessionStart'], {
    home: place.home,
    cwd: place.repo,
    input: JSON.stringify(sessionStartPayload(place.repo, source)),
    timeoutMs: 5_000,
  });
}

function leaseOwner(place: Place): unknown {
  return rows(place, 'SELECT owner_token FROM worker_lease WHERE id = 1')[0]?.owner_token;
}

function memorySnapshot(place: Place): string {
  return JSON.stringify(rows(place, 'SELECT id, content_hash FROM memories ORDER BY id'));
}

function eventCount(place: Place): number {
  return rows(place, 'SELECT id FROM raw_events').length;
}

/**
 * Induce STALE_AFTER_MS on the lease and RECLAIM_AFTER_MS on a running batch without sleeping (R6,
 * A11). The unit tests advance `now`; a spawned worker reads the wall clock.
 */
function prepareTakeover(place: Place): void {
  const now = Date.now();
  withDb(place, (db) => {
    db.prepare('UPDATE worker_lease SET heartbeat_at = ? WHERE id = 1').run(now - (STALE_AFTER_MS + 1_000));
    db.prepare(`UPDATE observation_batches SET claimed_at = ? WHERE state = 'running'`).run(
      now - (RECLAIM_AFTER_MS + 1_000),
    );
  });
}

function writeLiveLease(place: Place, heartbeatAt: number): string {
  const token = randomUUID();
  withDb(place, (db) => {
    db.prepare(
      'UPDATE worker_lease SET owner_token = ?, pid = ?, started_at = ?, heartbeat_at = ? WHERE id = 1',
    ).run(token, 4242, Date.now(), heartbeatAt);
  });
  return token;
}

type RecordedRequest = {
  method: string;
  path: string;
  headers: Record<string, string>;
  body: string;
};

type ProviderHandle = {
  url: string;
  requests: RecordedRequest[];
  firstRequest: Promise<void>;
  close(): Promise<void>;
};

function recordedHeaders(req: IncomingMessage): Record<string, string> {
  const headers: Record<string, string> = {};
  for (const [name, value] of Object.entries(req.headers)) {
    if (value === undefined) continue;
    headers[name] = Array.isArray(value) ? value.join(',') : value;
  }
  return headers;
}

function firstSummarizableId(place: Place): string {
  const found = rows(
    place,
    `SELECT id FROM raw_events WHERE ${SUMMARIZABLE_PREDICATE} ORDER BY captured_at, id LIMIT 1`,
  );
  const id = found[0]?.id;
  if (typeof id !== 'string') assert.fail('expected a summarizable raw event');
  return id;
}

function observerOutput(eventId: string): unknown {
  return {
    observations: [
      {
        type: 'discovery',
        title: 'Retry behavior',
        body: 'The upload path retries after a failure.',
        concepts: ['how-it-works'],
        citations: { files_read: [], files_modified: [], commits: [] },
        source_event_ids: [eventId],
        classification: { decision: 'add', target: null, reason: 'new behavior' },
      },
    ],
  };
}

function workersAiBody(output: unknown): string {
  return JSON.stringify({
    success: true,
    result: { response: output, usage: { prompt_tokens: 8, completion_tokens: 2 } },
    errors: [],
    messages: [],
  });
}

function exhaustedBody(): string {
  return JSON.stringify({ errors: [{ code: 3036, message: 'failure' }] });
}

function startProvider(
  options: { gate?: Promise<void>; exhausted?: boolean; output?: unknown } = {},
): Promise<ProviderHandle> {
  const requests: RecordedRequest[] = [];
  let firstResolve: (() => void) | undefined;
  const firstRequest = new Promise<void>((resolve) => {
    firstResolve = resolve;
  });
  const server = createServer((req, res) => {
    const chunks: Buffer[] = [];
    req.on('data', (chunk: Buffer) => chunks.push(chunk));
    req.on('end', () => {
      const recorded: RecordedRequest = {
        method: req.method ?? '',
        path: req.url ?? '',
        headers: recordedHeaders(req),
        body: Buffer.concat(chunks).toString('utf8'),
      };
      requests.push(recorded);
      if (requests.length === 1) firstResolve?.();
      void (async () => {
        if (options.gate !== undefined) await options.gate;
        const status = options.exhausted === true ? 429 : 200;
        const body =
          options.exhausted === true ? exhaustedBody() : workersAiBody(options.output ?? observerOutput('missing'));
        res.writeHead(status, { 'content-type': 'application/json' });
        res.end(body);
      })().catch(() => {
        res.statusCode = 500;
        res.end();
      });
    });
  });
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address() as AddressInfo;
      resolve({
        url: `http://127.0.0.1:${address.port}`,
        requests,
        firstRequest,
        close: () =>
          new Promise<void>((done, fail) => {
            server.closeAllConnections();
            server.close((error) => (error === undefined ? done() : fail(error)));
          }),
      });
    });
  });
}

async function waitForLeasePid(place: Place, child: ObserveChild, deadlineMs: number): Promise<void> {
  let stop = false;
  const claimed = (async () => {
    while (!stop) {
      const pid = rows(place, 'SELECT pid FROM worker_lease WHERE id = 1')[0]?.pid;
      if (Number(pid) === child.pid) return;
      await delay(5);
    }
  })();
  try {
    const outcome = await Promise.race([
      claimed.then(() => 'claimed' as const),
      child.finished.then((exit) => exit),
      delay(deadlineMs).then(() => 'timeout' as const),
    ]);
    if (outcome === 'claimed') return;
    if (outcome === 'timeout') {
      assert.fail(`lease pid never became ${child.pid} within ${deadlineMs} ms`);
    }
    assert.fail(
      `observe exited status=${outcome.status} signal=${outcome.signal} before claiming the lease`,
    );
  } finally {
    stop = true;
  }
}

async function waitForFirstRequest(
  server: ProviderHandle,
  child: ObserveChild,
  deadlineMs: number,
): Promise<void> {
  const outcome = await Promise.race([
    server.firstRequest.then(() => 'received' as const),
    child.finished.then((exit) => exit),
    delay(deadlineMs).then(() => 'timeout' as const),
  ]);
  if (outcome === 'received') return;
  if (outcome === 'timeout') assert.fail(`provider saw no request within ${deadlineMs} ms`);
  assert.fail(
    `observe exited status=${outcome.status} signal=${outcome.signal} before the provider request`,
  );
}

function stealLease(place: Place): string {
  return withDb(place, (db) => {
    const token = claimLease(db, { pid: process.pid, now: Date.now() + STALE_AFTER_MS + 1_000 });
    if (token === null) assert.fail('expected the in-flight lease to be stolen');
    return token;
  });
}

function assertUniqueBatches(place: Place): void {
  const grouped = rows(
    place,
    `SELECT session_id, through_event_id, destination, COUNT(*) AS n
     FROM observation_batches
     GROUP BY session_id, through_event_id, destination`,
  );
  assert.ok(
    grouped.length > 0,
    `expected at least one observation_batches row; sessions=${JSON.stringify(rows(place, 'SELECT id, status, summary_state, turn_count FROM sessions'))} events=${JSON.stringify(rows(place, 'SELECT kind, batch_id, classification_state FROM raw_events'))} log=${observeLog(place).slice(-1500)}`,
  );
  for (const row of grouped) {
    assert.equal(Number(row.n), 1, `duplicate batch ${JSON.stringify(row)}`);
  }
}

function assertOneMemorySet(place: Place): void {
  const sets = rows(
    place,
    `SELECT source_batch_id, COUNT(*) AS n FROM memories
     WHERE source_batch_id IS NOT NULL GROUP BY source_batch_id`,
  );
  assert.equal(sets.length, 1, `memory sets: ${JSON.stringify(sets)}`);
}

scenario('worker-kill', async (t: TestContext) => {
  let place: Place | undefined;
  let child: ObserveChild | undefined;
  const reap = async (): Promise<void> => {
    child?.kill('SIGKILL');
    await child?.finished.catch(() => undefined);
    child = undefined;
  };
  try {
    for (let attempt = 1; attempt <= 3; attempt += 1) {
      await reap();
      place?.cleanup();
      place = fixture();
      writeObserverConfig(place, 'none');
      seedEndedSession(place);
      const eventsBefore = eventCount(place);

      child = spawnObserve(place, { home: place.home });
      await waitForLeasePid(place, child, POLL_MS);
      child.kill('SIGKILL');
      await withDeadline(child.finished, POLL_MS, 'worker-kill SIGKILL reap');

      const ownerAfterKill = leaseOwner(place);
      if (ownerAfterKill === null) {
        t.diagnostic('worker-kill: kill landed after release, retrying');
        continue;
      }

      prepareTakeover(place);
      const recovered = observe(place, { timeoutMs: OBSERVE_MS });
      t.diagnostic(
        `worker-kill recovery status=${recovered.status} signal=${recovered.signal} elapsed=${recovered.elapsedMs.toFixed(1)} ms`,
      );
      assertUniqueBatches(place);
      assert.equal(recovered.signal, null, 'recovery must exit on its own');
      assert.equal(recovered.status, 1, `recovery exited ${recovered.status}: ${recovered.stderr}`);
      assert.notEqual(leaseOwner(place), ownerAfterKill, 'owner_token must change after takeover');
      assert.equal(eventCount(place), eventsBefore, 'no raw event is lost across the kill (FR-003)');
      assertOneMemorySet(place);
      return;
    }
    assert.fail('worker-kill: all three attempts missed the lease-held window');
  } finally {
    await reap();
    place?.cleanup();
  }
});

scenario('worker-kill-after-response', async (t: TestContext) => {
  const place = fixture();
  let server: ProviderHandle | undefined;
  try {
    writeObserverConfig(place, 'workers-ai');
    seedWorkersAiCatalog(place, CF.OBOETE_CF_ACCOUNT_ID ?? 'fault-worker-account');
    seedEndedSession(place);
    server = await startProvider({ output: observerOutput(firstSummarizableId(place)) });

    const killed = await runObserveAsync(
      place,
      { home: place.home, fault: 'worker-kill-after-response', faultUrl: server.url, extraEnv: CF },
      PROVIDER_MS,
    );
    t.diagnostic(
      `worker-kill-after-response run1 status=${killed.status} signal=${killed.signal} elapsed=${killed.elapsedMs.toFixed(1)} ms`,
    );
    assert.equal(killed.signal, 'SIGKILL', `run 1 signal=${killed.signal} status=${killed.status}`);

    prepareTakeover(place);
    const recovered = await runObserveAsync(
      place,
      { home: place.home, faultUrl: server.url, extraEnv: CF },
      PROVIDER_MS,
    );
    t.diagnostic(
      `worker-kill-after-response run2 status=${recovered.status} signal=${recovered.signal} requests=${server.requests.length}`,
    );
    assert.equal(recovered.signal, null);
    assert.equal(recovered.status, 0, `run 2 exited ${recovered.status}`);
    assert.equal(server.requests.length, 2, 'A11: one extra provider call after the crash');
    const batch = rows(
      place,
      `SELECT state, provider_attempts FROM observation_batches WHERE destination = 'remote_observer'`,
    )[0];
    assert.deepEqual(
      { state: batch?.state, provider_attempts: Number(batch?.provider_attempts) },
      { state: 'applied', provider_attempts: 2 },
    );
    const applied = rows(
      place,
      `SELECT COUNT(*) AS n FROM memories WHERE source_batch_id = (
         SELECT id FROM observation_batches WHERE destination = 'remote_observer')`,
    )[0];
    assert.equal(Number(applied?.n), 1, 'A11: one apply');
  } finally {
    await server?.close().catch(() => undefined);
    place.cleanup();
  }
});

scenario('lease-steal', async (t: TestContext) => {
  const place = fixture();
  const gate = deferred();
  let server: ProviderHandle | undefined;
  let child: ObserveChild | undefined;
  try {
    writeObserverConfig(place, 'workers-ai');
    seedWorkersAiCatalog(place, CF.OBOETE_CF_ACCOUNT_ID ?? 'fault-worker-account');
    seedEndedSession(place);
    const memoriesBefore = memorySnapshot(place);
    server = await startProvider({ gate: gate.promise, output: observerOutput(firstSummarizableId(place)) });

    child = spawnObserve(place, { home: place.home, faultUrl: server.url, extraEnv: CF });
    await waitForFirstRequest(server, child, POLL_MS);
    const token = stealLease(place);
    gate.resolve();
    const exit = await withDeadline(child.finished, PROVIDER_MS, 'lease-steal victim');
    t.diagnostic(`lease-steal victim status=${exit.status} signal=${exit.signal}`);
    assert.equal(exit.signal, null);
    assert.equal(exit.status, 0, `victim exited ${exit.status}`);
    assert.equal(leaseOwner(place), token);
    assert.match(observeLog(place), /run end .*reason=lease_lost/);
    assert.equal(memorySnapshot(place), memoriesBefore, 'the victim must not apply after the steal');
    const batch = rows(place, 'SELECT state, owner_token FROM observation_batches')[0];
    assert.ok(batch !== undefined, 'expected a batch');
    assert.notEqual(batch.state, 'applied');
  } finally {
    gate.resolve();
    child?.kill('SIGKILL');
    await child?.finished.catch(() => undefined);
    await server?.close().catch(() => undefined);
    place.cleanup();
  }
});

scenario('lease-lost-after-3036', async (t: TestContext) => {
  const place = fixture();
  const gate = deferred();
  let server: ProviderHandle | undefined;
  let child: ObserveChild | undefined;
  try {
    writeObserverConfig(place, 'workers-ai');
    seedWorkersAiCatalog(place, CF.OBOETE_CF_ACCOUNT_ID ?? 'fault-worker-account');
    seedEndedSession(place);
    const memoriesBefore = memorySnapshot(place);
    server = await startProvider({
      gate: gate.promise,
      exhausted: true,
      output: observerOutput(firstSummarizableId(place)),
    });

    child = spawnObserve(place, { home: place.home, faultUrl: server.url, extraEnv: CF });
    await waitForFirstRequest(server, child, POLL_MS);
    stealLease(place);
    gate.resolve();
    const exit = await withDeadline(child.finished, PROVIDER_MS, '3036 victim');
    t.diagnostic(`lease-lost-after-3036 victim status=${exit.status} signal=${exit.signal}`);
    assert.equal(exit.signal, null);
    assert.equal(exit.status, 0, `old worker exited ${exit.status}`);
    assert.equal(memorySnapshot(place), memoriesBefore, 'no memory was written by the lost worker');
    t.diagnostic(`lease-lost-after-3036 log=${observeLog(place)}`);
    t.diagnostic(`lease-lost-after-3036 usage=${JSON.stringify(rows(place, 'SELECT * FROM provider_usage'))}`);
    const usage = rows(
      place,
      `SELECT exhausted_at, exhausted_reservation_id FROM provider_usage
       WHERE utc_day = '${utcDay(Date.now())}' AND preset = 'workers-ai'`,
    )[0];
    assert.notEqual(usage?.exhausted_at ?? null, null, 'unfenced 3036 write must survive the lost lease');
    assert.equal(typeof usage?.exhausted_reservation_id, 'string');

    dropLease(place);
    prepareTakeover(place);
    const recovered = await runObserveAsync(
      place,
      { home: place.home, faultUrl: server.url, extraEnv: CF },
      PROVIDER_MS,
    );
    t.diagnostic(
      `lease-lost-after-3036 recovery status=${recovered.status} requests=${server.requests.length}`,
    );
    assert.equal(recovered.signal, null);
    // contracts/cli.md:15 — exit 1 = fallback used; the exhausted reservation forces the rule-based fallback.
    assert.equal(recovered.status, 1, `recovery exited ${recovered.status}`);
    assert.equal(server.requests.length, 1, 'exhausted reservation must not call the provider again');
    const fallback = rows(place, 'SELECT state, destination FROM observation_batches');
    assert.equal(fallback.length, 1);
    assert.equal(fallback[0]?.state, 'fallback');
    assertOneMemorySet(place);
  } finally {
    gate.resolve();
    child?.kill('SIGKILL');
    await child?.finished.catch(() => undefined);
    await server?.close().catch(() => undefined);
    place.cleanup();
  }
});

scenario('clock-jump', async (t: TestContext) => {
  async function stolen(offsetMs: number): Promise<void> {
    const place = fixture();
    try {
      writeObserverConfig(place, 'none');
      seedEndedSession(place);
      const planted = writeLiveLease(place, Date.now() + offsetMs);
      const result = observe(place, { timeoutMs: OBSERVE_MS });
      t.diagnostic(
        `clock-jump offset=${offsetMs} status=${result.status} signal=${result.signal} owner=${String(leaseOwner(place))}`,
      );
      assert.equal(result.signal, null);
      assert.equal(result.status, 1, `clock-jump steal exited ${result.status}: ${result.stderr}`);
      assert.notEqual(leaseOwner(place), planted, `lease was not stolen at offset ${offsetMs}`);
    } finally {
      place.cleanup();
    }
  }

  await stolen(FUTURE_SKEW_MS * 2);
  await stolen(-(FUTURE_SKEW_MS * 2));

  const place = fixture();
  try {
    writeObserverConfig(place, 'none');
    seedEndedSession(place);
    const planted = writeLiveLease(place, Date.now() + (FUTURE_SKEW_MS - 1_000));
    const result = observe(place, { timeoutMs: OBSERVE_MS });
    t.diagnostic(`clock-jump control status=${result.status} reason log=${observeLog(place)}`);
    assert.equal(result.signal, null);
    assert.equal(result.status, 0);
    assert.equal(leaseOwner(place), planted, 'heartbeat 59 s in the future must not be stolen');
    assert.match(observeLog(place), /run end .*reason=another_worker/);
  } finally {
    place.cleanup();
  }
});

scenario('resume', () => {
  const place = fixture();
  try {
    captureHook(place, 'PostToolUse', claudePayload(place.repo));
    const before = rows(place, 'SELECT id, conversation_id FROM sessions')[0];
    assert.ok(before !== undefined);
    setForeignConversationRoot(place);
    const result = hookSessionStart(place, 'resume');
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, '', 'resume does not emit a session-start pack');
    const after = rows(place, 'SELECT id, conversation_id FROM sessions')[0];
    assert.equal(after?.conversation_id, 'root-not-me');
  } finally {
    place.cleanup();
  }
});

scenario('fork', () => {
  const place = fixture();
  try {
    captureHook(place, 'PostToolUse', claudePayload(place.repo));
    const before = rows(place, 'SELECT id FROM sessions')[0];
    assert.ok(before !== undefined);
    setForeignConversationRoot(place);
    const result = hookSessionStart(place, 'fork');
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, '', 'fork does not emit a session-start pack');
    const after = rows(place, 'SELECT id, conversation_id FROM sessions')[0];
    assert.equal(after?.id, before?.id);
    assert.equal(after?.conversation_id, after?.id, 'fork detaches onto its own id');
  } finally {
    place.cleanup();
  }
});

scenario('clear', () => {
  const place = fixture();
  try {
    captureHook(place, 'PostToolUse', claudePayload(place.repo));
    seedMemory(place);
    setForeignConversationRoot(place);
    const result = hookSessionStart(place, 'clear');
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /oboete memory context/, 'clear emits a plain-stdout session-start pack');
    const after = rows(place, 'SELECT id, conversation_id FROM sessions')[0];
    assert.equal(after?.conversation_id, 'root-not-me');
  } finally {
    place.cleanup();
  }
});

scenario('compact', () => {
  const place = fixture();
  try {
    captureHook(place, 'PostToolUse', claudePayload(place.repo));
    seedMemory(place);
    const epochBefore = Number(rows(place, 'SELECT context_epoch FROM sessions')[0]?.context_epoch);
    assert.equal(epochBefore, 0);

    const pair = (): void => {
      const start = hookSessionStart(place, 'compact');
      assert.equal(start.status, 0, start.stderr);
      captureHook(place, 'PostCompact', claudePayload(place.repo, 'PostCompact'));
    };
    pair();
    const epochAfter = Number(rows(place, 'SELECT context_epoch FROM sessions')[0]?.context_epoch);
    assert.equal(epochAfter, epochBefore + 1, 'A16: the distinct compaction pair opens one epoch');
    pair();
    const epochRedelivered = Number(rows(place, 'SELECT context_epoch FROM sessions')[0]?.context_epoch);
    assert.equal(epochRedelivered, epochAfter, 'A16: byte-identical redelivery must not move the epoch');

    assert.deepEqual(
      rows(place, "SELECT kind, context_epoch FROM injections WHERE kind = 'session_start'").map((row) => ({ ...row })),
      [{ kind: 'session_start', context_epoch: 1 }],
    );
    assert.deepEqual(
      rows(place, 'SELECT memory_id, context_epoch, decision, reason FROM injection_items').map((row) => ({ ...row })),
      [{ memory_id: 'm-fault-worker', context_epoch: 1, decision: 'included', reason: 'pinned' }],
    );
  } finally {
    place.cleanup();
  }
});

scenario('pause', (t: TestContext) => {
  const place = fixture();
  try {
    writeFileSync(join(place.home, 'paused'), '');
    captureHook(place, 'PostToolUse', claudePayload(place.repo));
    assert.equal(eventCount(place), 0);
    assert.deepEqual(spoolFiles(place), []);
    assert.equal(existsSync(join(place.home, 'logs', 'hook.log')), false);

    const planted = writeLiveLease(place, Date.now());
    const leaseBefore = rows(
      place,
      'SELECT owner_token, pid, started_at, heartbeat_at FROM worker_lease WHERE id = 1',
    )[0];
    const mtimeBefore = statSync(place.db).mtimeMs;
    const result = observe(place, { timeoutMs: OBSERVE_MS });
    t.diagnostic(`pause observe status=${result.status} signal=${result.signal}`);
    assert.equal(result.signal, null);
    assert.equal(result.status, 0);
    const leaseAfter = rows(
      place,
      'SELECT owner_token, pid, started_at, heartbeat_at FROM worker_lease WHERE id = 1',
    )[0];
    assert.deepEqual(leaseAfter, leaseBefore);
    assert.equal(leaseAfter?.owner_token, planted);
    assert.equal(statSync(place.db).mtimeMs, mtimeBefore);
  } finally {
    place.cleanup();
  }
});
