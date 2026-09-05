import assert from 'node:assert/strict';
import { existsSync, renameSync, writeFileSync } from 'node:fs';
import type { DatabaseSync } from 'node:sqlite';

import { captureEvent, type CaptureDeps, type CaptureOutcome } from '../../src/capture.js';
import {
  PRESET_CATALOG,
  configSchema,
  consentHash,
  consentTuple,
  type PresetName,
} from '../../src/config.js';
import { openDatabase } from '../../src/db/open.js';
import type { ObserverOutput } from '../../src/observer/contract.js';
import { ensureDirectories, oboetePaths, type OboetePaths } from '../../src/paths.js';
import { childEnvironment, credentialValues } from '../../src/log.js';
import { detectSync } from '../../src/privacy/detect.js';
import { runObserve, type ObserveDeps } from '../../src/worker/observe.js';
import { withTempHome } from './home.js';

/**
 * The worker fixture shared by the observe and privacy suites: a temporary home with an open
 * database, capture through the real hook path, a config writer, and provider response shapes.
 */
export const NOW = Date.UTC(2026, 8, 4, 0, 0, 0);
export const DAY = 24 * 60 * 60 * 1000;

export type Json = Record<string, unknown>;
export type Fixture = {
  home: string;
  paths: OboetePaths;
  env: NodeJS.ProcessEnv;
  /** Captures through the real hook path and returns the outcome, whose `stdout` is the pack. */
  capture(eventName: string, payload: Json, expected?: CaptureOutcome['outcome']): Promise<CaptureOutcome>;
  withDb<T>(fn: (db: DatabaseSync) => T): T;
};

/** The developer's shell without oboete credentials (log.ts childEnvironment), pointed at the fixture home. */
export function cleanEnv(home: string, extra: NodeJS.ProcessEnv = {}): NodeJS.ProcessEnv {
  return { ...childEnvironment(process.env), OBOETE_HOME: home, NODE_ENV: 'test', ...extra };
}

/** Moves the database files aside (or back) so one hook runs without storage and spools. */
export function toggleDatabase(fixture: Fixture, away: boolean): void {
  for (const suffix of ['', '-wal', '-shm']) {
    const live = `${fixture.paths.db}${suffix}`;
    const parked = `${live}.away`;
    if (away && existsSync(live)) renameSync(live, parked);
    if (!away && existsSync(parked)) renameSync(parked, live);
  }
}

export async function withFixture(fn: (fixture: Fixture) => Promise<void>): Promise<void> {
  await withTempHome(async (home) => {
    const paths = oboetePaths(home);
    ensureDirectories(paths);
    openDatabase({ path: paths.db, timeoutMs: 1_000 }).db.close();
    let capturedAt = NOW - DAY;
    const captureDeps: CaptureDeps = {
      detect: (input) => detectSync({ ...input, credentialValues: credentialValues(fixture.env) }),
      now: () => {
        capturedAt += 1;
        return capturedAt;
      },
      elapsedMs: () => 0,
      spawnWorker: () => undefined,
    };
    const fixture: Fixture = {
      home,
      paths,
      env: cleanEnv(home),
      capture: async (eventName, payload, expected = 'stored') => {
        const outcome = await captureEvent(captureDeps, {
          agent: 'claude',
          eventName,
          paths,
          readStdin: () => ({ text: JSON.stringify(payload), truncated: false }),
        });
        assert.equal(outcome.outcome, expected);
        return outcome;
      },
      withDb: (work) => {
        const opened = openDatabase({ path: paths.db, timeoutMs: 1_000 });
        try {
          return work(opened.db);
        } finally {
          opened.db.close();
        }
      },
    };
    await fn(fixture);
  });
}

export function writeConfig(
  fixture: Fixture,
  preset: PresetName | 'none',
  env: NodeJS.ProcessEnv = fixture.env,
  consent: 'valid' | 'invalid' = 'valid',
): void {
  const parsed = configSchema.parse({ observer: { preset } });
  const hash = consent === 'valid' ? consentHash(consentTuple(parsed, env)) : 'not-the-live-consent-hash';
  writeFileSync(
    fixture.paths.config,
    `[observer]\npreset = "${preset}"\n${preset === 'none' ? '' : `\n[consent]\nhash = "${hash}"\naccepted_at = ${NOW}\n`}`,
  );
}

export function eventBase(sessionId: string): Json {
  return { session_id: sessionId, cwd: process.cwd() };
}

export async function captureEndedSession(
  fixture: Fixture,
  options: {
    sessionId: string;
    prompts: string[];
    assistant?: string;
    tools?: { id: string; path: string; text?: string }[];
  },
): Promise<void> {
  const common = eventBase(options.sessionId);
  await fixture.capture('SessionStart', { ...common, source: 'startup' });
  for (const [index, prompt] of options.prompts.entries()) {
    await fixture.capture('UserPromptSubmit', {
      ...common,
      prompt_id: `prompt-${index + 1}`,
      prompt,
    });
  }
  for (const tool of options.tools ?? []) {
    await fixture.capture('PreToolUse', {
      ...common,
      prompt_id: `prompt-${options.prompts.length}`,
      tool_name: 'Edit',
      tool_use_id: tool.id,
      tool_input: {
        file_path: tool.path,
        old_string: tool.text ?? 'before',
        new_string: 'after',
      },
    });
  }
  if (options.assistant !== undefined) {
    await fixture.capture('Stop', {
      ...common,
      prompt_id: `prompt-${options.prompts.length}`,
      last_assistant_message: options.assistant,
    });
  }
  await fixture.capture('SessionEnd', { ...common, reason: 'prompt_input_exit' });
}

export function eventId(fixture: Fixture, content: string): string {
  return fixture.withDb((db) => {
    const id = db.prepare('SELECT id FROM raw_events WHERE content = ?').get(content)?.id;
    if (typeof id !== 'string') assert.fail(`event not found for ${content}`);
    return id;
  });
}

export function providerOutput(
  sourceEventId: string,
  language: 'en' | 'ja' = 'en',
): ObserverOutput {
  return {
    observations: [
      {
        type: 'discovery',
        title: language === 'ja' ? '再試行の仕組み' : 'Retry behavior',
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

export function openAiResponse(output: ObserverOutput, model = PRESET_CATALOG.openrouter.defaultModel): Response {
  return new Response(
    JSON.stringify({
      id: 'response-1',
      model,
      choices: [{ index: 0, message: { role: 'assistant', content: JSON.stringify(output) }, finish_reason: 'stop' }],
      usage: { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 },
    }),
    { status: 200, headers: { 'content-type': 'application/json' } },
  );
}

export function workersResponse(output: ObserverOutput): Response {
  return new Response(
    JSON.stringify({
      success: true,
      result: { response: output, usage: { prompt_tokens: 8, completion_tokens: 2 } },
      errors: [],
      messages: [],
    }),
    { status: 200, headers: { 'content-type': 'application/json' } },
  );
}

export function catalogResponse(page = 1): Response {
  return new Response(
    JSON.stringify({
      success: true,
      result: page === 1 ? [{ name: PRESET_CATALOG['workers-ai'].defaultModel, properties: [] }] : [],
      errors: [],
      messages: [],
    }),
    { status: 200, headers: { 'content-type': 'application/json' } },
  );
}

export function runObserveForFixture(
  fixture: Fixture,
  overrides: Partial<ObserveDeps> = {},
): Promise<number> {
  return runObserve([], {
    env: fixture.env,
    now: () => NOW,
    detect: detectSync,
    heartbeatMs: 60_000,
    fetch: async () => assert.fail('an observer request was not expected'),
    ...overrides,
  });
}

