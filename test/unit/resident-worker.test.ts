import assert from 'node:assert/strict';
import { existsSync, readFileSync, unlinkSync, utimesSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

import { shouldSpawnResident } from '../../src/capture-command.js';
import { writeWorkerStop } from '../../src/pause.js';
import { queueIsEmpty, runObserve, type ObserveDeps } from '../../src/worker/observe.js';
import { claimLease } from '../../src/worker/lease.js';
import { openDatabase } from '../../src/db/open.js';
import {
  NOW,
  captureEndedSession,
  cleanEnv,
  eventId,
  openAiResponse,
  providerOutput,
  runObserveForFixture,
  withFixture,
  writeConfig,
  type Fixture,
} from '../helpers/observe.js';

const FIRST_RETRY_MS = 5 * 60_000;

test('a resident retries a due source in a later epoch of the same process', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'resident-retry-key' });
    writeConfig(fixture, 'openrouter', fixture.env);
    const prompt = 'Record the upload retry behavior.';
    await captureEndedSession(fixture, {
      sessionId: 'resident-retry',
      prompts: [prompt],
      assistant: 'The upload path retries after a failure.',
    });
    const sourceId = eventId(fixture, prompt);

    let clock = NOW;
    let calls = 0;
    const tokens: string[] = [];
    const fetchImpl: typeof fetch = async () => {
      calls += 1;
      fixture.withDb((db) => {
        const token = db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token;
        if (typeof token === 'string') tokens.push(token);
      });
      if (calls === 1) {
        return new Response(JSON.stringify({ error: { message: 'temporary' } }), {
          status: 500,
          headers: { 'content-type': 'application/json' },
        });
      }
      return openAiResponse(providerOutput(sourceId));
    };

    const now = () => {
      if (clock > NOW) return clock;
      try {
        if (readFileSync(fixture.paths.observeLog, 'utf8').includes('state=fallback')) {
          clock = NOW + FIRST_RETRY_MS + 1;
        }
      } catch {
        // The first now() can precede run start.
      }
      return clock;
    };

    let elapsed = 0;
    const exit = await runObserveForFixture(
      fixture,
      {
        now,
        fetch: fetchImpl,
        maxRunMs: 60 * 60 * 1000,
        elapsedMs: () => elapsed,
        sleep: async (ms) => {
          elapsed += ms;
        },
      },
      ['--resident'],
    );

    const diag = fixture.withDb((db) => {
      const batches = db
        .prepare('SELECT state, owner_token FROM observation_batches ORDER BY rowid')
        .all()
        .map((row) => ({ state: String(row.state), owner_token: String(row.owner_token) }));
      const source = db.prepare(
        "SELECT processing_state, retry_after FROM raw_events WHERE kind = 'prompt'",
      ).get();
      return {
        batches,
        processing_state: source?.processing_state ?? null,
        retry_after: source?.retry_after ?? null,
        clock,
        tokens,
        exit,
      };
    });

    assert.equal(
      calls,
      2,
      `the later epoch must retry the due source in this process; idle probe vs held token: ${JSON.stringify(diag)}`,
    );
    assert.equal(diag.processing_state, 'processed');
    assert.equal(new Set(tokens).size, 2, 'the retry epoch rotates the lease token');
    assert.equal(diag.batches.length, 2);
    assert.equal(diag.batches[0]?.state, 'fallback');
    assert.equal(diag.batches[1]?.state, 'applied');
    assert.notEqual(diag.batches[0]?.owner_token, diag.batches[1]?.owner_token);
    fixture.withDb((db) => {
      assert.equal(
        Number(
          db
            .prepare(
              "SELECT COUNT(*) AS n FROM memories WHERE source_batch_id = (SELECT id FROM observation_batches WHERE state = 'applied')",
            )
            .get()?.n,
        ) > 0,
        true,
      );
      assert.equal(
        Number(db.prepare("SELECT COUNT(*) AS n FROM observation_batches WHERE state = 'applied'").get()?.n),
        1,
      );
    });
    assert.equal(exit, 0);
  });
});

function residentClock(): {
  elapsed: number;
  elapsedMs: () => number;
  sleep: (ms: number) => Promise<void>;
} {
  const state = { elapsed: 0 };
  return {
    get elapsed() {
      return state.elapsed;
    },
    set elapsed(value: number) {
      state.elapsed = value;
    },
    elapsedMs: () => state.elapsed,
    sleep: async (ms: number) => {
      state.elapsed += ms;
    },
  };
}

async function runResident(
  fixture: Fixture,
  extra: Partial<ObserveDeps> = {},
  argv: string[] = ['--resident'],
): Promise<number> {
  const clock = residentClock();
  return runObserveForFixture(
    fixture,
    {
      maxRunMs: 60 * 60 * 1000,
      elapsedMs: clock.elapsedMs,
      sleep: clock.sleep,
      ...extra,
    },
    argv,
  );
}

test('one-shot observe still exits after a failed source and does not retry in-process', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'one-shot-no-retry' });
    writeConfig(fixture, 'openrouter', fixture.env);
    await captureEndedSession(fixture, {
      sessionId: 'one-shot-no-retry',
      prompts: ['Leave this for the next invocation.'],
    });
    let clock = NOW;
    let calls = 0;
    const exit = await runObserveForFixture(fixture, {
      now: () => {
        try {
          if (readFileSync(fixture.paths.observeLog, 'utf8').includes('state=fallback')) {
            clock = NOW + FIRST_RETRY_MS + 1;
          }
        } catch {
          // run start
        }
        return clock;
      },
      fetch: async () => {
        calls += 1;
        return new Response('down', { status: 500, headers: { 'content-type': 'application/json' } });
      },
      maxRunMs: 60 * 60 * 1000,
    });
    assert.equal(calls, 1);
    assert.equal(exit, 1);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, null);
    });
  });
});

test('the idle probe sees a due retry, a spool file, a pending batch and a pending summary', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await captureEndedSession(fixture, {
      sessionId: 'probe-kinds',
      prompts: ['A session that can wait for a summary.'],
    });
    fixture.withDb((db) => {
      assert.equal(queueIsEmpty(db, fixture.paths, '', NOW), false, 'due session_end source');
    });

    await runObserveForFixture(fixture);
    fixture.withDb((db) => {
      db.exec("UPDATE sessions SET summary_state = 'pending', summary_updated_at = NULL");
      assert.equal(queueIsEmpty(db, fixture.paths, '', NOW), false, 'session awaiting a summary');
      db.exec("UPDATE sessions SET summary_state = 'done'");
      db.exec("UPDATE observation_batches SET state = 'pending' WHERE state = 'fallback'");
      assert.equal(queueIsEmpty(db, fixture.paths, '', NOW), false, 'adoptable pending batch');
      db.exec("UPDATE observation_batches SET state = 'fallback'");
      assert.equal(queueIsEmpty(db, fixture.paths, '', NOW), true);
    });
    writeFileSync(`${fixture.paths.spool}/waiting.json`, '{}');
    fixture.withDb((db) => {
      assert.equal(queueIsEmpty(db, fixture.paths, '', NOW), false, 'spool file');
    });
  });
});

test('a running batch inside its reclaim window does not start an epoch per poll', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await captureEndedSession(fixture, {
      sessionId: 'reclaim-wait',
      prompts: ['Do not spin while a batch is running.'],
    });
    fixture.withDb((db) => {
      const event = db.prepare("SELECT id, session_id, repo_id FROM raw_events WHERE kind = 'prompt'").get();
      if (event === undefined) assert.fail('expected a captured prompt');
      db.prepare(
        `INSERT INTO observation_batches
           (id, repo_id, session_id, through_event_id, destination, trigger, state, owner_token, provider_attempts, claimed_at)
         VALUES ('running-wait', ?, ?, ?, 'fallback', 'session_end', 'running', 'dead-worker', 1, ?)`,
      ).run(event.repo_id, event.session_id, event.id, NOW);
      db.exec("UPDATE raw_events SET processing_state = 'processed', batch_id = 'running-wait'");
      db.exec("UPDATE sessions SET summary_state = 'done'");
    });

    let polls = 0;
    const exit = await runResident(fixture, {
      now: () => NOW,
      sleep: async (ms) => {
        polls += 1;
        if (polls >= 8) writeWorkerStop(fixture.paths);
        await new Promise((resolve) => setTimeout(resolve, 0));
        void ms;
      },
      elapsedMs: () => polls * 2_000,
    });
    assert.equal(exit, 0);
    const log = readFileSync(fixture.paths.observeLog, 'utf8');
    assert.equal(log.includes('\n') && / epoch /.test(log), false, 'no epoch line while the batch is unreclaimable');
    assert.match(log, /reason=stopped/);
    assert.ok(polls >= 8);
    assert.ok(polls < 8 + 5, 'stop is noticed on the next poll, not one epoch per poll');
  });
});

test('each cooperative control exits 0 with its own reason', async () => {
  const cases: Array<{ name: string; reason: string; poke: (fixture: Fixture, polls: number) => void }> = [
    {
      name: 'paused',
      reason: 'paused',
      poke: (fixture, polls) => {
        if (polls === 1) writeFileSync(fixture.paths.paused, '');
      },
    },
    {
      name: 'stopped',
      reason: 'stopped',
      poke: (fixture, polls) => {
        if (polls === 1) writeWorkerStop(fixture.paths);
      },
    },
    {
      name: 'rewritten config',
      reason: 'config_changed',
      poke: (fixture, polls) => {
        if (polls === 1) writeFileSync(fixture.paths.config, `${readFileSync(fixture.paths.config, 'utf8')}\n`);
      },
    },
    {
      name: 'unreadable config',
      reason: 'config_changed',
      poke: (fixture, polls) => {
        if (polls === 1) writeFileSync(fixture.paths.config, '[observer\npreset = ');
      },
    },
    {
      name: 'changed engine artifact',
      reason: 'upgraded',
      poke: (fixture, polls) => {
        if (polls === 1) utimesSync(join(fixture.home, 'engine-artifact'), 1_700_000_000, 1_700_000_000);
      },
    },
    {
      name: 'removed engine artifact',
      reason: 'upgraded',
      poke: (fixture, polls) => {
        if (polls === 1) unlinkSync(join(fixture.home, 'engine-artifact'));
      },
    },
    {
      name: 'idle timeout',
      reason: 'idle_exit',
      poke: () => undefined,
    },
    {
      name: 'lost lease',
      reason: 'lease_lost',
      poke: (fixture, polls) => {
        if (polls === 1) {
          fixture.withDb((db) => {
            db.prepare("UPDATE worker_lease SET owner_token = 'thief', heartbeat_at = ? WHERE id = 1").run(NOW);
          });
        }
      },
    },
  ];

  for (const item of cases) {
    await withFixture(async (fixture) => {
      writeConfig(fixture, 'none');
      const artifact = join(fixture.home, 'engine-artifact');
      writeFileSync(artifact, 'engine');
      let polls = 0;
      const extra: Partial<ObserveDeps> = {
        engineArtifact: artifact,
        ...(item.reason === 'idle_exit'
          ? {}
          : {
              sleep: async () => {
                polls += 1;
                item.poke(fixture, polls);
              },
              elapsedMs: () => polls * 2_000,
            }),
      };
      const exit = await runResident(fixture, extra);
      assert.equal(exit, 0, item.name);
      assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), new RegExp(`reason=${item.reason}`), item.name);
    });
  }
});

test('a fallback epoch still exits 0 on a cooperative stop', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await captureEndedSession(fixture, {
      sessionId: 'fallback-then-stop',
      prompts: ['Fallback must not force exit 1 on stop.'],
    });
    let polls = 0;
    const exit = await runResident(fixture, {
      sleep: async () => {
        polls += 1;
        if (polls === 1) writeWorkerStop(fixture.paths);
      },
      elapsedMs: () => polls * 2_000,
    });
    assert.equal(exit, 0);
    assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), /reason=stopped/);
    fixture.withDb((db) => {
      assert.ok(Number(db.prepare("SELECT COUNT(*) AS n FROM observation_batches WHERE state = 'fallback'").get()?.n) >= 1);
    });
  });
});

test('worker-stop is removed before the lease is released and pause is not consumed', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await captureEndedSession(fixture, {
      sessionId: 'stop-consumed',
      prompts: ['Stop the resident once it is idle.'],
    });
    let polls = 0;
    await runResident(fixture, {
      sleep: async () => {
        polls += 1;
        if (polls === 1) writeWorkerStop(fixture.paths);
      },
      elapsedMs: () => polls * 2_000,
    });
    assert.equal(readFileSync(fixture.paths.observeLog, 'utf8').includes('reason=stopped'), true);
    assert.equal(existsSync(fixture.paths.workerStop), false);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, null);
    });

    writeFileSync(fixture.paths.paused, '');
    const pausedExit = await runResident(fixture);
    assert.equal(pausedExit, 0);
    assert.equal(existsSync(fixture.paths.paused), true);
    assert.equal(existsSync(fixture.paths.observeLog) && readFileSync(fixture.paths.observeLog, 'utf8').includes('run start'), true);
  });
});

test('shutdown with queued work releases the lease so a later spawn can reach it', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await captureEndedSession(fixture, {
      sessionId: 'shutdown-queued',
      prompts: ['Keep this queued across shutdown.'],
    });
    fixture.withDb((db) => {
      db.exec("UPDATE raw_events SET processing_state = 'waiting', retry_after = 0");
    });
    writeWorkerStop(fixture.paths);
    const exit = await runResident(fixture);
    assert.equal(exit, 0);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, null);
      assert.ok(Number(db.prepare('SELECT COUNT(*) AS n FROM raw_events').get()?.n) > 0);
    });
  });
});

test('observe --stop writes the sentinel and exits 0 without claiming the lease', async () => {
  await withFixture(async (fixture) => {
    assert.equal(await runObserve(['--stop'], { env: fixture.env }), 0);
    assert.equal(existsSync(fixture.paths.workerStop), true);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, null);
    });
  });
});

test('a second resident exits 0 as another_worker without writing', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await captureEndedSession(fixture, {
      sessionId: 'race-claim',
      prompts: ['Only one owner.'],
    });
    const held = openDatabase({ path: fixture.paths.db, timeoutMs: 1_000 }).db;
    try {
      const token = claimLease(held, { pid: 99, now: NOW });
      if (token === null) assert.fail('expected a foreign lease');
      assert.equal(await runResident(fixture), 0);
      assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), /reason=another_worker/);
      assert.equal(held.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, token);
      assert.equal(held.prepare('SELECT COUNT(*) AS n FROM observation_batches').get()?.n, 0);
    } finally {
      held.close();
    }
  });
});

test('a wall-clock jump does not end an epoch budget measured on elapsed time', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await captureEndedSession(fixture, {
      sessionId: 'clock-jump',
      prompts: ['Epoch budgets stay on the monotonic clock.'],
    });
    let wall = NOW;
    let polls = 0;
    const exit = await runResident(fixture, {
      now: () => wall,
      maxRunMs: 5_000,
      sleep: async () => {
        polls += 1;
        wall += 60 * 60_000;
        if (polls === 2) writeWorkerStop(fixture.paths);
      },
      elapsedMs: () => polls * 2_000,
    });
    assert.equal(exit, 0);
    assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), /reason=stopped/);
    assert.equal(readFileSync(fixture.paths.observeLog, 'utf8').includes('reason=max_run'), false);
  });
});

test('shouldSpawnResident follows [worker] resident and defaults true', async () => {
  await withFixture(async (fixture) => {
    assert.equal(shouldSpawnResident(fixture.paths), true);
    writeFileSync(fixture.paths.config, '[worker]\nresident = false\n');
    assert.equal(shouldSpawnResident(fixture.paths), false);
    writeFileSync(fixture.paths.config, '[observer\n');
    assert.equal(shouldSpawnResident(fixture.paths), true);
  });
});

