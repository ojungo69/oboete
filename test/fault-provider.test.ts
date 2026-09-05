// Provider half of the User Story 2 failure matrix. Sources: quickstart.md "Failure injection",
// contracts/observer.md (presets, call policy, 150/day cap, session_end reserve), FR-012, R3, A11.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { TestContext } from 'node:test';

import {
  PRESET_CATALOG,
  configSchema,
  consentHash,
  consentTuple,
  type PresetName,
} from '../src/config.js';
import type { ObserverOutput } from '../src/observer/contract.js';
import { DAILY_CAP, nextUtcMidnight, utcDay } from '../src/observer/reservation.js';
import {
  fixture,
  observeLog,
  rows,
  run,
  scenario,
  seedWorkersAiCatalog,
  SELECTOR,
  spawnEngine,
  type Place,
  type SpawnEngineOptions,
  type SpawnResult,
} from './helpers/fault.js';

const OBSERVE_MS = 15_000;
const HANG_BUDGET_MS = 5_000;
const HOLD_TOKEN = 't060-lease-hold';
const CF_TOKEN = 't060-dummy-cf-token';
const CF_ACCOUNT = 't060-dummy-cf-account';
const OPENROUTER_KEY = 't060-dummy-openrouter-key';
const CF_ENV: NodeJS.ProcessEnv = {
  OBOETE_CF_API_TOKEN: CF_TOKEN,
  OBOETE_CF_ACCOUNT_ID: CF_ACCOUNT,
};
const OPENROUTER_ENV: NodeJS.ProcessEnv = { OBOETE_OPENROUTER_API_KEY: OPENROUTER_KEY };
const ENGLISH_TITLE = 'Retry behavior';
const JA_PROMPT = 'アップロード処理の再試行を記録してください。';
const JA_ASSISTANT = 'アップロード処理は一回再試行します。';
// firstSentence(JA_ASSISTANT): the trailing 。 is kept (fallback.ts).
const JA_FALLBACK_TITLE = 'アップロード処理は一回再試行します。';
const EN_PROMPT = 'Record the retry behavior.';
const EN_ASSISTANT = 'The upload path retries once after a failure.';

type Scripted = { hang: true } | { status: number; body: string };

type RecordedRequest = {
  method: string;
  path: string;
  headers: Record<string, string>;
  body: unknown;
};

type Fake = {
  url: string;
  requests: RecordedRequest[];
  close(): void;
};

function writeConfig(
  place: Place,
  preset: PresetName,
  env: NodeJS.ProcessEnv,
  hash = consentHash(consentTuple(configSchema.parse({ observer: { preset } }), env)),
): void {
  writeFileSync(
    join(place.home, 'config.toml'),
    `[observer]\npreset = "${preset}"\n\n[consent]\nhash = "${hash}"\naccepted_at = ${Date.now()}\n`,
  );
}

// A held lease makes capture.ts skip spawnWorker (isLeaseFree is false). Callers releaseLease after the last trigger hook.
function holdLease(place: Place): void {
  const now = Date.now();
  run(
    place,
    'UPDATE worker_lease SET owner_token = ?, pid = 1, started_at = ?, heartbeat_at = ? WHERE id = 1',
    [HOLD_TOKEN, now, now],
  );
}

function releaseLease(place: Place): void {
  run(
    place,
    'UPDATE worker_lease SET owner_token = NULL, pid = NULL, started_at = NULL, heartbeat_at = NULL WHERE id = 1',
  );
}

function hook(
  place: Place,
  event: string,
  payload: unknown,
  extraEnv?: NodeJS.ProcessEnv,
): void {
  const result = spawnEngine(['hook', '--agent', SELECTOR, '--event', event], {
    home: place.home,
    cwd: place.repo,
    input: JSON.stringify(payload),
    extraEnv,
    timeoutMs: 5_000,
  });
  assert.equal(result.status, 0, `hook ${event} exited ${result.status}: ${result.stderr}`);
}

function seedEndedSession(
  place: Place,
  options: {
    sessionId: string;
    prompt: string;
    assistant: string;
    extraEnv?: NodeJS.ProcessEnv;
  },
): void {
  const { sessionId, prompt, assistant, extraEnv } = options;
  const common = { session_id: sessionId, cwd: place.repo };
  holdLease(place);
  hook(place, 'SessionStart', { ...common, source: 'startup' }, extraEnv);
  let classified = false;
  for (let attempt = 1; attempt <= 5; attempt += 1) {
    const promptId = `${sessionId}-p${attempt}`;
    hook(
      place,
      'UserPromptSubmit',
      { ...common, prompt_id: promptId, prompt },
      extraEnv,
    );
    holdLease(place);
    hook(
      place,
      'Stop',
      { ...common, prompt_id: promptId, last_assistant_message: assistant },
      extraEnv,
    );
    const row = rows(
      place,
      `SELECT classification_state, content FROM raw_events
       WHERE kind = 'last_assistant_message'
         AND session_id = (SELECT id FROM sessions WHERE native_session_id = ?)
       ORDER BY captured_at DESC LIMIT 1`,
      [sessionId],
    )[0];
    if (row?.classification_state === 'done' && row.content !== null && row.content !== undefined) {
      classified = true;
      break;
    }
  }
  if (!classified) {
    assert.fail(
      'capture never classified the seeded last_assistant_message (detector missed the 300 ms budget)',
    );
  }
  holdLease(place);
  hook(place, 'SessionEnd', { ...common, reason: 'prompt_input_exit' }, extraEnv);
}

function observe(place: Place, opts: Omit<SpawnEngineOptions, 'home' | 'cwd'>): SpawnResult {
  return spawnEngine(['observe'], {
    home: place.home,
    cwd: place.repo,
    timeoutMs: opts.timeoutMs ?? OBSERVE_MS,
    extraEnv: opts.extraEnv,
    fault: opts.fault,
    faultUrl: opts.faultUrl,
  });
}

// spawnEngine uses spawnSync, which blocks this process's event loop. A same-process HTTP
// server cannot answer the child, so the stand-in lives in its own process.
const FAKE_SERVER_SOURCE = `
const fs = require('node:fs');
const http = require('node:http');
const script = JSON.parse(process.env.OBOETE_FAKE_SCRIPT);
const reqFile = process.env.OBOETE_FAKE_REQS;
const portFile = process.env.OBOETE_FAKE_PORT;
let index = 0;
const CRED = new Set(['authorization', 'x-api-key', 'api-key', 'x-goog-api-key']);
const server = http.createServer((req, res) => {
  const chunks = [];
  let recorded = false;
  const headers = {};
  for (const [key, value] of Object.entries(req.headers)) {
    if (value === undefined) continue;
    const name = String(key).toLowerCase();
    if (CRED.has(name)) continue;
    headers[name] = Array.isArray(value) ? value.join(',') : String(value);
  }
  const record = (body) => {
    if (recorded) return;
    recorded = true;
    fs.appendFileSync(reqFile, JSON.stringify({
      method: req.method,
      path: req.url,
      headers,
      body,
    }) + '\\n');
  };
  req.on('data', (chunk) => chunks.push(chunk));
  req.on('end', () => {
    const raw = Buffer.concat(chunks).toString('utf8');
    let parsed = raw;
    try { parsed = raw === '' ? null : JSON.parse(raw); } catch { /* keep text */ }
    record(parsed);
    const step = script[index++];
    if (step && step.hang) return;
    if (!step) {
      res.writeHead(500, { 'content-type': 'text/plain', connection: 'close' });
      res.end('unexpected request');
      return;
    }
    res.writeHead(step.status, { 'content-type': 'application/json', connection: 'close' });
    res.end(step.body);
  });
  req.on('close', () => record(null));
});
server.listen(0, '127.0.0.1', () => {
  fs.writeFileSync(portFile, String(server.address().port));
});
`;

function startFake(script: Scripted[]): Fake {
  const dir = mkdtempSync(join(tmpdir(), 'oboete-fake-'));
  const reqFile = join(dir, 'reqs');
  const portFile = join(dir, 'port');
  writeFileSync(reqFile, '');
  const child = spawn(process.execPath, ['-e', FAKE_SERVER_SOURCE], {
    env: {
      ...process.env,
      OBOETE_FAKE_SCRIPT: JSON.stringify(script),
      OBOETE_FAKE_REQS: reqFile,
      OBOETE_FAKE_PORT: portFile,
    },
    stdio: 'ignore',
  });
  child.unref();
  const deadline = performance.now() + 2_000;
  let portText = '';
  while (performance.now() < deadline) {
    if (existsSync(portFile)) {
      portText = readFileSync(portFile, 'utf8').trim();
      if (/^[1-9]\d*$/.test(portText)) break;
    }
    Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 10);
  }
  if (!/^[1-9]\d*$/.test(portText)) {
    child.kill('SIGKILL');
    rmSync(dir, { recursive: true, force: true });
    throw new Error('fake server did not bind');
  }
  const port = Number(portText);
  return {
    url: `http://127.0.0.1:${port}`,
    get requests() {
      const text = readFileSync(reqFile, 'utf8').trim();
      if (text === '') return [];
      return text.split('\n').map((line) => JSON.parse(line) as RecordedRequest);
    },
    close() {
      child.kill('SIGTERM');
      rmSync(dir, { recursive: true, force: true });
    },
  };
}

function workersBody(response: unknown, extra: Record<string, unknown> = {}): string {
  return JSON.stringify({
    success: true,
    result: { response, usage: { prompt_tokens: 8, completion_tokens: 2 }, ...extra },
    errors: [],
    messages: [],
  });
}

function openAiBody(
  content: string,
  extra: { finishReason?: string; model?: string } = {},
): string {
  return JSON.stringify({
    id: 'response-1',
    model: extra.model ?? PRESET_CATALOG.openrouter.defaultModel,
    choices: [
      {
        index: 0,
        message: { role: 'assistant', content },
        finish_reason: extra.finishReason ?? 'stop',
      },
    ],
    usage: { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 },
  });
}

function observation(sourceEventId: string, language: 'en' | 'ja' = 'en'): ObserverOutput {
  return {
    observations: [
      {
        type: 'discovery',
        title: language === 'ja' ? '再試行の仕組み' : ENGLISH_TITLE,
        body:
          language === 'ja'
            ? 'アップロード処理は失敗時に再試行します。'
            : 'The upload path retries after a failure.',
        concepts: ['how-it-works'],
        citations: { files_read: [], files_modified: [], commits: [] },
        source_event_ids: [sourceEventId],
        classification: { decision: 'add', target: null, reason: 'new behavior' },
      },
    ],
  };
}

function fallbackDecision(sourceEventId: string, text: string): ObserverOutput {
  return {
    observations: [
      {
        type: 'decision',
        title: text,
        body: text,
        concepts: ['why-it-exists'],
        citations: { files_read: [], files_modified: [], commits: [] },
        source_event_ids: [sourceEventId],
        classification: { decision: 'add', target: null, reason: 'rule:decision' },
      },
    ],
  };
}

function eventId(place: Place, kind: string): string {
  const row = rows(
    place,
    `SELECT id FROM raw_events
     WHERE kind = ? AND classification_state = 'done' AND content IS NOT NULL
     ORDER BY captured_at DESC LIMIT 1`,
    [kind],
  )[0];
  assert.equal(typeof row?.id, 'string', `missing admissible ${kind} event`);
  return String(row.id);
}

function remoteBatch(place: Place): Record<string, unknown> {
  const row = rows(
    place,
    "SELECT state, degraded_reason, provider_attempts FROM observation_batches WHERE destination = 'remote_observer'",
  )[0];
  assert.ok(row !== undefined, `no remote_observer batch; batches=${JSON.stringify(rows(place, 'SELECT destination, state, degraded_reason, provider_attempts FROM observation_batches'))}`);
  return row;
}

function usageRow(place: Place, preset: PresetName): Record<string, unknown> | undefined {
  return rows(
    place,
    'SELECT calls, exhausted_at, exhausted_reservation_id FROM provider_usage WHERE preset = ?',
    [preset],
  )[0];
}

function cappedCallSum(place: Place): number {
  const names = (Object.keys(PRESET_CATALOG) as PresetName[]).filter((name) => PRESET_CATALOG[name].capped);
  const placeholders = names.map(() => '?').join(', ');
  const row = rows(
    place,
    `SELECT COALESCE(SUM(COALESCE(calls, 0)), 0) AS calls
     FROM provider_usage WHERE utc_day = ? AND preset IN (${placeholders})`,
    [utcDay(Date.now()), ...names],
  )[0];
  return numberOf(row?.calls);
}

function numberOf(value: unknown): number {
  if (typeof value === 'number') return value;
  if (typeof value === 'bigint') return Number(value);
  return Number(value ?? 0);
}

function prepareWorkersAi(): { place: Place; env: NodeJS.ProcessEnv } {
  const env = CF_ENV;
  const place = fixture();
  writeConfig(place, 'workers-ai', env);
  seedWorkersAiCatalog(place, CF_ACCOUNT);
  holdLease(place);
  return { place, env };
}

scenario('provider-unreachable', async (t: TestContext) => {
  const { place, env } = prepareWorkersAi();
  try {
    seedEndedSession(place, { sessionId: 's-unreach', prompt: EN_PROMPT, assistant: EN_ASSISTANT, extraEnv: env });
    releaseLease(place);

    const result = observe(place, { extraEnv: env, faultUrl: 'http://127.0.0.1:1' });
    t.diagnostic(`unreachable status=${result.status} signal=${result.signal} elapsed=${result.elapsedMs.toFixed(1)}`);
    assert.equal(result.signal, null, `observe killed: ${result.stderr}`);
    // contracts/cli.md / logEnd: a non-rule_based fallback ends the run with exit 1.
    assert.equal(result.status, 1, `observe exited ${result.status}: ${result.stderr}\n${observeLog(place)}`);

    const batch = remoteBatch(place);
    assert.equal(batch.state, 'fallback');
    assert.equal(batch.degraded_reason, 'unreachable');
    assert.equal(numberOf(batch.provider_attempts), 1);
    const memories = rows(place, 'SELECT COUNT(*) AS n FROM memories WHERE degraded_reason = ?', [
      'unreachable',
    ]);
    assert.ok(numberOf(memories[0]?.n) > 0, 'fallback memory must carry degraded_reason');
  } finally {
    place.cleanup();
  }
});

scenario('provider-hang', async (t: TestContext) => {
  const { place, env } = prepareWorkersAi();
  let fake: Fake | undefined;
  try {
    seedEndedSession(place, { sessionId: 's-hang', prompt: EN_PROMPT, assistant: EN_ASSISTANT, extraEnv: env });
    releaseLease(place);
    fake = startFake([{ hang: true }]);

    const result = observe(place, {
      extraEnv: env,
      fault: 'provider-hang',
      faultUrl: fake.url,
      timeoutMs: 10_000,
    });
    t.diagnostic(`hang status=${result.status} requests=${fake.requests.length} elapsed=${result.elapsedMs.toFixed(1)}`);
    assert.equal(result.signal, null, `observe killed: ${result.stderr}`);
    assert.equal(result.status, 1, `observe exited ${result.status}: ${result.stderr}\n${observeLog(place)}`);
    assert.ok(
      result.elapsedMs < HANG_BUDGET_MS,
      `hang run took ${result.elapsedMs.toFixed(1)} ms (budget ${HANG_BUDGET_MS})`,
    );
    assert.equal(fake.requests.length, 1);
    const batch = remoteBatch(place);
    assert.equal(batch.degraded_reason, 'timeout');
    assert.equal(numberOf(batch.provider_attempts), 1);
  } finally {
    fake?.close();
    place.cleanup();
  }
});

scenario('provider-429-3036', async (t: TestContext) => {
  const { place, env } = prepareWorkersAi();
  let fake: Fake | undefined;
  try {
    seedEndedSession(place, { sessionId: 's-3036', prompt: EN_PROMPT, assistant: EN_ASSISTANT, extraEnv: env });
    releaseLease(place);
    fake = startFake([{ status: 429, body: '{"errors":[{"code":3036}]}' }]);

    const first = observe(place, { extraEnv: env, faultUrl: fake.url });
    t.diagnostic(`3036 first status=${first.status} requests=${fake.requests.length}`);
    assert.equal(first.signal, null);
    assert.equal(first.status, 1, `observe exited ${first.status}: ${first.stderr}\n${observeLog(place)}`);
    const batch = remoteBatch(place);
    assert.equal(batch.state, 'fallback');
    assert.equal(batch.degraded_reason, 'provider_exhausted');
    assert.equal(numberOf(batch.provider_attempts), 1);
    const usage = usageRow(place, 'workers-ai');
    assert.ok(usage?.exhausted_at !== null && usage?.exhausted_at !== undefined);
    assert.equal(typeof usage?.exhausted_reservation_id, 'string');
    assert.equal(fake.requests.length, 1);

    // FR-012: exhaustion is never retried; a later session today must not call the provider.
    holdLease(place);
    seedEndedSession(place, {
      sessionId: 's-3036-b',
      prompt: EN_PROMPT,
      assistant: EN_ASSISTANT,
      extraEnv: env,
    });
    releaseLease(place);
    const before = fake.requests.length;
    const source = eventId(place, 'last_assistant_message');
    fake.close();
    fake = startFake([{ status: 200, body: workersBody(observation(source)) }]);
    const second = observe(place, { extraEnv: env, faultUrl: fake.url });
    t.diagnostic(`3036 second status=${second.status} extraRequests=${fake.requests.length}`);
    assert.equal(second.signal, null);
    assert.equal(second.status, 1);
    assert.equal(fake.requests.length, 0, `exhaustion retried; first-run requests=${before}`);
    const later = rows(
      place,
      "SELECT state, degraded_reason, provider_attempts FROM observation_batches WHERE session_id = (SELECT id FROM sessions WHERE native_session_id = ?)",
      ['s-3036-b'],
    );
    assert.ok(later.some((row) => row.degraded_reason === 'provider_exhausted' && row.state === 'fallback'));
  } finally {
    fake?.close();
    place.cleanup();
  }
});

scenario('provider-403-5035', async (t: TestContext) => {
  const { place, env } = prepareWorkersAi();
  let fake: Fake | undefined;
  try {
    seedEndedSession(place, { sessionId: 's-5035', prompt: EN_PROMPT, assistant: EN_ASSISTANT, extraEnv: env });
    releaseLease(place);
    fake = startFake([{ status: 403, body: '{"errors":[{"code":5035}]}' }]);

    const result = observe(place, { extraEnv: env, faultUrl: fake.url });
    t.diagnostic(`5035 status=${result.status} requests=${fake.requests.length}`);
    assert.equal(result.signal, null);
    assert.equal(result.status, 1, `observe exited ${result.status}: ${result.stderr}\n${observeLog(place)}`);
    const batch = remoteBatch(place);
    assert.equal(batch.degraded_reason, 'provider_paid');
    assert.equal(numberOf(batch.provider_attempts), 1);
    assert.equal(fake.requests.length, 1);
    const usage = usageRow(place, 'workers-ai');
    assert.equal(usage?.exhausted_at ?? null, null);
  } finally {
    fake?.close();
    place.cleanup();
  }
});

scenario('provider-401', async (t: TestContext) => {
  const { place, env } = prepareWorkersAi();
  let fake: Fake | undefined;
  try {
    seedEndedSession(place, { sessionId: 's-401', prompt: EN_PROMPT, assistant: EN_ASSISTANT, extraEnv: env });
    releaseLease(place);
    fake = startFake([{ status: 401, body: '{}' }]);

    const result = observe(place, { extraEnv: env, faultUrl: fake.url });
    t.diagnostic(`401 status=${result.status} requests=${fake.requests.length}`);
    assert.equal(result.signal, null);
    assert.equal(result.status, 1, `observe exited ${result.status}: ${result.stderr}\n${observeLog(place)}`);
    const batch = remoteBatch(place);
    assert.equal(batch.degraded_reason, 'auth_failed', '401 is auth_failed, never provider_paid');
    assert.equal(numberOf(batch.provider_attempts), 1);
    assert.equal(fake.requests.length, 1);
    assert.match(
      String(fake.requests[0]?.path),
      /^\/client\/v4\/accounts\/t060-dummy-cf-account\/ai\/run\//,
    );
  } finally {
    fake?.close();
    place.cleanup();
  }
});

scenario('provider-length', async (t: TestContext) => {
  const { place, env } = prepareWorkersAi();
  let fake: Fake | undefined;
  try {
    seedEndedSession(place, { sessionId: 's-length', prompt: EN_PROMPT, assistant: EN_ASSISTANT, extraEnv: env });
    releaseLease(place);
    const source = eventId(place, 'last_assistant_message');
    const body = workersBody(observation(source), { finish_reason: 'length' });
    fake = startFake([
      { status: 200, body },
      { status: 200, body },
    ]);

    const result = observe(place, { extraEnv: env, faultUrl: fake.url });
    t.diagnostic(`length status=${result.status} requests=${fake.requests.length}`);
    assert.equal(result.signal, null);
    assert.equal(result.status, 1, `observe exited ${result.status}: ${result.stderr}\n${observeLog(place)}`);
    const batch = remoteBatch(place);
    assert.equal(batch.degraded_reason, 'unusable_output');
    assert.equal(numberOf(batch.provider_attempts), 2);
    assert.equal(fake.requests.length, 2);
    assert.equal(numberOf(usageRow(place, 'workers-ai')?.calls), 2);
  } finally {
    fake?.close();
    place.cleanup();
  }
});

scenario('provider-malformed', async (t: TestContext) => {
  const { place, env } = prepareWorkersAi();
  let fake: Fake | undefined;
  try {
    seedEndedSession(place, { sessionId: 's-malformed', prompt: EN_PROMPT, assistant: EN_ASSISTANT, extraEnv: env });
    releaseLease(place);
    const body = workersBody('not json');
    fake = startFake([
      { status: 200, body },
      { status: 200, body },
    ]);

    const result = observe(place, { extraEnv: env, faultUrl: fake.url });
    t.diagnostic(`malformed status=${result.status} requests=${fake.requests.length}`);
    assert.equal(result.signal, null);
    assert.equal(result.status, 1, `observe exited ${result.status}: ${result.stderr}\n${observeLog(place)}`);
    const batch = remoteBatch(place);
    assert.equal(batch.degraded_reason, 'unusable_output');
    assert.equal(numberOf(batch.provider_attempts), 2);
    assert.equal(fake.requests.length, 2);
    assert.match(
      `${observeLog(place)}\n${result.stderr}`,
      /not valid JSON/,
      'logged detail must name the JSON parse failure',
    );
  } finally {
    fake?.close();
    place.cleanup();
  }
});

scenario(
  'provider-malformed',
  'provider-malformed: schema-invalid JSON',
  async (t: TestContext) => {
    const { place, env } = prepareWorkersAi();
    let fake: Fake | undefined;
    try {
      seedEndedSession(place, {
        sessionId: 's-schema',
        prompt: EN_PROMPT,
        assistant: EN_ASSISTANT,
        extraEnv: env,
      });
      releaseLease(place);
      const body = workersBody({ observations: [{ type: 'nope' }] });
      fake = startFake([
        { status: 200, body },
        { status: 200, body },
      ]);

      const result = observe(place, { extraEnv: env, faultUrl: fake.url });
      t.diagnostic(`schema-invalid status=${result.status} requests=${fake.requests.length}`);
      assert.equal(result.signal, null);
      assert.equal(result.status, 1, `observe exited ${result.status}: ${result.stderr}\n${observeLog(place)}`);
      const batch = remoteBatch(place);
      assert.equal(batch.degraded_reason, 'unusable_output');
      assert.equal(numberOf(batch.provider_attempts), 2);
      assert.equal(fake.requests.length, 2);
    } finally {
      fake?.close();
      place.cleanup();
    }
  },
);

scenario('provider-wrong-language', async (t: TestContext) => {
  const { place, env } = prepareWorkersAi();
  let fake: Fake | undefined;
  try {
    seedEndedSession(place, { sessionId: 's-lang', prompt: JA_PROMPT, assistant: JA_ASSISTANT, extraEnv: env });
    releaseLease(place);
    const source = eventId(place, 'last_assistant_message');
    const body = workersBody(observation(source, 'en'));
    fake = startFake([
      { status: 200, body },
      { status: 200, body },
    ]);

    const result = observe(place, { extraEnv: env, faultUrl: fake.url });
    t.diagnostic(`language status=${result.status} requests=${fake.requests.length}`);
    assert.equal(result.signal, null);
    assert.equal(result.status, 1, `observe exited ${result.status}: ${result.stderr}\n${observeLog(place)}`);
    const batch = remoteBatch(place);
    assert.equal(batch.degraded_reason, 'language_mismatch');
    assert.equal(numberOf(batch.provider_attempts), 2);
    assert.equal(fake.requests.length, 2);
    const titles = rows(place, 'SELECT title, body FROM memories').map((row) => `${row.title}\n${row.body}`);
    assert.equal(
      titles.some((text) => text.includes(ENGLISH_TITLE)),
      false,
      `English provider title leaked into memories: ${titles.join(' | ')}`,
    );
    assert.ok(
      rows(place, 'SELECT title FROM memories WHERE title = ?', [JA_FALLBACK_TITLE]).length > 0,
      `fallback title missing: ${titles.join(' | ')}`,
    );
  } finally {
    fake?.close();
    place.cleanup();
  }
});

async function capBoundaryForPreset(
  t: TestContext,
  preset: PresetName,
  env: NodeJS.ProcessEnv,
  scriptFor: (sourceEventId: string) => Scripted,
  seedPreset: PresetName,
): Promise<void> {
  const place = fixture();
  let fake: Fake | undefined;
  try {
    writeConfig(place, preset, env);
    if (preset === 'workers-ai') seedWorkersAiCatalog(place, env.OBOETE_CF_ACCOUNT_ID ?? CF_ACCOUNT);
    const now = Date.now();
    run(
      place,
      `INSERT INTO provider_usage (utc_day, preset, calls, neurons_estimate, reset_at)
       VALUES (?, ?, ?, 0, ?)`,
      [utcDay(now), seedPreset, DAILY_CAP - 1, nextUtcMidnight(now)],
    );
    holdLease(place);
    seedEndedSession(place, { sessionId: 's-cap-a', prompt: EN_PROMPT, assistant: EN_ASSISTANT, extraEnv: env });
    releaseLease(place);
    fake = startFake([scriptFor(eventId(place, 'last_assistant_message'))]);

    const beforeOwn = numberOf(usageRow(place, preset)?.calls);
    const first = observe(place, { extraEnv: env, faultUrl: fake.url });
    t.diagnostic(`${preset} 150 status=${first.status} requests=${fake.requests.length} calls=${usageRow(place, preset)?.calls}`);
    assert.equal(first.signal, null, `observe killed: ${first.stderr}`);
    assert.equal(fake.requests.length, 1, `${preset} 150th attempt must reach the provider`);
    assert.equal(cappedCallSum(place), DAILY_CAP);
    assert.equal(numberOf(usageRow(place, preset)?.calls), beforeOwn + 1);

    holdLease(place);
    seedEndedSession(place, { sessionId: 's-cap-b', prompt: EN_PROMPT, assistant: EN_ASSISTANT, extraEnv: env });
    releaseLease(place);
    fake.close();
    fake = startFake([scriptFor('unused')]);
    const afterOwn = numberOf(usageRow(place, preset)?.calls);
    const second = observe(place, { extraEnv: env, faultUrl: fake.url });
    t.diagnostic(`${preset} 151 status=${second.status} requests=${fake.requests.length}`);
    assert.equal(second.signal, null);
    assert.equal(second.status, 1, `151st must fall back: ${second.stderr}\n${observeLog(place)}`);
    assert.equal(fake.requests.length, 0, `${preset} 151st attempt must not call the provider`);
    assert.equal(cappedCallSum(place), DAILY_CAP);
    assert.equal(numberOf(usageRow(place, preset)?.calls), afterOwn);
    const refused = rows(
      place,
      "SELECT degraded_reason FROM observation_batches WHERE session_id = (SELECT id FROM sessions WHERE native_session_id = ?)",
      ['s-cap-b'],
    );
    assert.ok(
      refused.some((row) => row.degraded_reason === 'daily_cap'),
      `151st reason missing: ${JSON.stringify(refused)}`,
    );
  } finally {
    fake?.close();
    place.cleanup();
  }
}

scenario('cap-boundary', async (t: TestContext) => {
  // FR-012 / observer.md call policy: attempt 150 allowed, 151 refused, across capped presets.
  // SESSION_END_RESERVE: ten_turns would refuse at 140, so these sessions end (trigger session_end).
  await capBoundaryForPreset(
    t,
    'workers-ai',
    CF_ENV,
    (id) => ({ status: 200, body: workersBody(observation(id)) }),
    'workers-ai',
  );
  await capBoundaryForPreset(
    t,
    'openrouter',
    OPENROUTER_ENV,
    (id) => ({ status: 200, body: openAiBody(JSON.stringify(observation(id))) }),
    'workers-ai',
  );
});

scenario('consent-changed', async (t: TestContext) => {
  const { place, env } = prepareWorkersAi();
  let fake: Fake | undefined;
  try {
    seedEndedSession(place, { sessionId: 's-consent', prompt: EN_PROMPT, assistant: EN_ASSISTANT, extraEnv: env });
    writeConfig(place, 'workers-ai', env, 'not-the-live-consent-hash');
    releaseLease(place);
    fake = startFake([{ status: 200, body: workersBody(observation('unused')) }]);

    const result = observe(place, { extraEnv: env, faultUrl: fake.url });
    t.diagnostic(`consent status=${result.status} requests=${fake.requests.length}`);
    assert.equal(result.signal, null);
    assert.equal(result.status, 1, `observe exited ${result.status}: ${result.stderr}\n${observeLog(place)}`);
    const batch = remoteBatch(place);
    assert.equal(batch.degraded_reason, 'consent_changed');
    assert.equal(numberOf(batch.provider_attempts), 0);
    assert.equal(usageRow(place, 'workers-ai'), undefined);
    assert.equal(fake.requests.length, 0);
  } finally {
    fake?.close();
    place.cleanup();
  }
});

scenario('remote-no-duplicate', async (t: TestContext) => {
  const { place, env } = prepareWorkersAi();
  let fake: Fake | undefined;
  try {
    seedEndedSession(place, { sessionId: 's-dedup', prompt: EN_PROMPT, assistant: EN_ASSISTANT, extraEnv: env });
    releaseLease(place);
    const source = eventId(place, 'last_assistant_message');
    const remoteBody = workersBody(fallbackDecision(source, EN_ASSISTANT));
    fake = startFake([
      { status: 200, body: remoteBody },
      { status: 200, body: remoteBody },
    ]);

    const first = observe(place, { extraEnv: env, faultUrl: fake.url });
    t.diagnostic(`dedup first status=${first.status} requests=${fake.requests.length}`);
    assert.equal(first.signal, null);
    assert.equal(first.status, 0, `observe exited ${first.status}: ${first.stderr}\n${observeLog(place)}`);
    assert.equal(fake.requests.length, 1, 'remote path must be exercised on run 1');
    const stored = rows(
      place,
      'SELECT content_hash, title, degraded_reason FROM memories WHERE title = ? AND type != ?',
      [EN_ASSISTANT, 'session_summary'],
    );
    assert.equal(stored.length, 1, `remote memory missing: ${JSON.stringify(rows(place, 'SELECT type, title, degraded_reason FROM memories'))}`);
    assert.equal(stored[0]?.degraded_reason, null);
    const hash = String(stored[0]?.content_hash);

    holdLease(place);
    const common = { session_id: 's-dedup', cwd: place.repo };
    hook(place, 'Stop', { ...common, prompt_id: 's-dedup-p2', last_assistant_message: EN_ASSISTANT }, env);
    // A second SessionEnd with the same reason is collapsed as a re-delivery (R7) and would leave
    // the reopened session active, so the worker idles with no batch trigger (T058/T063).
    run(place, "UPDATE sessions SET status = 'ended', ended_at = ? WHERE native_session_id = ?", [
      Date.now(),
      's-dedup',
    ]);
    releaseLease(place);
    fake.close();
    fake = undefined;

    const second = observe(place, { extraEnv: env, faultUrl: 'http://127.0.0.1:1' });
    t.diagnostic(`dedup second status=${second.status}`);
    assert.equal(second.signal, null);
    assert.equal(second.status, 1, `fallback run exited ${second.status}: ${second.stderr}\n${observeLog(place)}`);
    const copies = rows(place, 'SELECT id, source_batch_id FROM memories WHERE content_hash = ?', [hash]);
    assert.equal(copies.length, 1, `duplicate memories for ${hash}: ${JSON.stringify(copies)}`);
    const secondBatches = rows(
      place,
      `SELECT b.id, b.state, b.degraded_reason
       FROM observation_batches b
       JOIN sessions s ON s.id = b.session_id
       WHERE s.native_session_id = ? AND b.state = 'fallback' AND b.degraded_reason = 'unreachable'`,
      ['s-dedup'],
    );
    assert.ok(secondBatches.length >= 1, `second fallback batch missing: ${JSON.stringify(rows(place, 'SELECT destination, state, degraded_reason FROM observation_batches'))}`);
    const addedOnFallback = rows(
      place,
      `SELECT m.id FROM memories m
       WHERE m.content_hash = ? AND m.source_batch_id IN (${secondBatches.map(() => '?').join(', ')})`,
      [hash, ...secondBatches.map((row) => String(row.id))],
    );
    // applyObservations records a duplicate as noop and does not INSERT (classify.ts, A11).
    assert.equal(addedOnFallback.length, 0, 'second apply must not add the same content hash');
  } finally {
    fake?.close();
    place.cleanup();
  }
});
