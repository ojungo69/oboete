import assert from 'node:assert/strict';
import { execFile, spawn } from 'node:child_process';
import { existsSync, readFileSync, unlinkSync, utimesSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { setTimeout as delay } from 'node:timers/promises';
import { promisify } from 'node:util';

import { shouldSpawnResident } from '../../src/capture-command.js';
import { writeWorkerStop } from '../../src/pause.js';
import { queueIsEmpty, runObserve, type ObserveDeps } from '../../src/worker/observe.js';
import { claimLease } from '../../src/worker/lease.js';
import { openDatabase } from '../../src/db/open.js';
import { detectSync } from '../../src/privacy/detect.js';
import { repositoryRoot } from '../helpers/compile-cache.js';
import { WALL_CLOCK_IS_MEASURED } from '../helpers/home.js';
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

// The heartbeat is a real `setInterval`, so wait for the tick that carries the advanced wall clock
// rather than a fixed sleep the instrumented coverage leg can outrun (test/helpers/home.ts). The
// last row is returned on timeout so the caller's own assertion reports the failure.
async function heartbeatAtLeast(fixture: Fixture, expected: number): Promise<Record<string, unknown> | undefined> {
  const deadline = performance.now() + 8_000;
  let row: Record<string, unknown> | undefined;
  do {
    row = fixture.withDb((db) =>
      db.prepare('SELECT owner_token, heartbeat_at, pid FROM worker_lease WHERE id = 1').get());
    if (Number(row?.heartbeat_at) >= expected) return row;
    await delay(5);
  } while (performance.now() < deadline);
  return row;
}

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

    const exit = await runObserveForFixture(
      fixture,
      {
        now,
        fetch: fetchImpl,
        maxRunMs: 60 * 60 * 1000,
        ...residentClock(),
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

function residentClock(
  onSleep?: (polls: number) => void | Promise<void>,
  stepMs?: number,
): Pick<ObserveDeps, 'elapsedMs' | 'sleep'> {
  let elapsed = 0;
  let polls = 0;
  return {
    elapsedMs: () => elapsed,
    sleep: async (ms: number) => {
      elapsed += stepMs ?? ms;
      polls += 1;
      await onSleep?.(polls);
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

async function captureRunningBatch(fixture: Fixture): Promise<void> {
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
    await captureRunningBatch(fixture);

    const waits: number[] = [];
    const tokens = new Set<string>();
    let elapsed = 0;
    const exit = await runResident(fixture, {
      now: () => NOW,
      maxRunMs: 400,
      sleep: async (ms) => {
        waits.push(ms);
        elapsed += ms;
        fixture.withDb((db) => {
          tokens.add(String(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token));
        });
        if (waits.length === 8) writeWorkerStop(fixture.paths);
      },
      elapsedMs: () => elapsed,
    });
    assert.equal(exit, 0);
    const log = readFileSync(fixture.paths.observeLog, 'utf8');
    assert.equal(log.includes('\n') && / epoch /.test(log), false, 'no epoch line while the batch is unreclaimable');
    assert.match(log, /reason=stopped/);
    assert.deepEqual(waits, [200, 200, 2_000, 200, 200, 2_000, 200, 200]);
    assert.equal(tokens.size, 3, 'max_run ends only the epoch; each new epoch gets its full budget');
    assert.equal(log.includes('reason=max_run'), false);
  });
});

test('the heartbeat keeps ownership under the token rotated for the second epoch', async () => {
  await withFixture(async (fixture) => {
    await captureRunningBatch(fixture);
    let wall = NOW;
    const epochTokens: unknown[] = [];
    let heartbeatRow: Record<string, unknown> | undefined;
    const clock = residentClock(async (polls) => {
      if (polls === 1 || polls === 4) {
        epochTokens.push(fixture.withDb((db) =>
          db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token));
      }
      if (polls === 4) {
        // Two 5 ms waits exhaust epoch one; the idle wait precedes epoch two's first wait.
        wall += 1_000;
        heartbeatRow = await heartbeatAtLeast(fixture, wall);
        writeWorkerStop(fixture.paths);
      }
    });
    const exit = await runResident(fixture, {
      ...clock, now: () => wall, maxRunMs: 10, heartbeatMs: 5,
    });
    assert.equal(exit, 0);
    const log = readFileSync(fixture.paths.observeLog, 'utf8');
    assert.doesNotMatch(log, /run end .*reason=lease_lost/,
      'the second epoch heartbeat must use the rotated lease token');
    assert.match(log, /run end .*reason=stopped/);
    assert.equal(epochTokens.length, 2);
    assert.equal(typeof epochTokens[0], 'string');
    assert.equal(typeof epochTokens[1], 'string');
    assert.notEqual(epochTokens[0], epochTokens[1]);
    assert.equal(heartbeatRow?.owner_token, epochTokens[1], 'the lease stays owned during the heartbeat wait');
    assert.equal(heartbeatRow?.pid, process.pid);
    assert.equal(heartbeatRow?.heartbeat_at, NOW + 1_000, 'the real interval fires after the wall clock advances');
  });
});

test('a batch_error ends a run after one attempt, including at the deadline in either mode', async () => {
  for (const mode of ['resident', 'one-shot', 'resident-at-deadline', 'one-shot-at-deadline']) {
    const resident = mode.startsWith('resident');
    await withFixture(async (fixture) => {
      fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'batch-error-test-key' });
      writeConfig(fixture, 'openrouter', fixture.env);
      const prompt = 'End the process when applying a batch fails.';
      await captureEndedSession(fixture, { sessionId: 'batch-error', prompts: [prompt] });
      let elapsed = 0;
      let calls = 0;
      let waits = 0;
      const exit = await runResident(fixture, {
        maxRunMs: 400,
        now: () => NOW + elapsed,
        elapsedMs: () => elapsed,
        fetch: async () => {
          calls += 1;
          return openAiResponse(providerOutput(eventId(fixture, prompt)));
        },
        applyHook: () => {
          if (mode.endsWith('at-deadline')) elapsed = 400;
          throw new Error('fixture apply failure');
        },
        sleep: async () => {
          waits += 1;
          writeWorkerStop(fixture.paths);
        },
      }, resident ? ['--resident'] : []);
      assert.equal(exit, 0);
      assert.equal(calls, 1);
      assert.equal(waits, 0, 'a batch error must not begin another idle wait or epoch');
      assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), /run end .*reason=batch_error/);
      fixture.withDb((db) => {
        const owner = db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token;
        if (resident) assert.equal(owner, null);
        else assert.equal(typeof owner, 'string', 'one-shot batch_error keeps its existing lease receipt');
      });
    });
  }
});

test('a stop before the provider request leaves the batch pending for immediate adoption', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'request-stop-test-key' });
    writeConfig(fixture, 'openrouter', fixture.env);
    const prompt = 'Keep a stopped request pending.';
    await captureEndedSession(fixture, { sessionId: 'request-stop', prompts: [prompt] });
    const sourceId = eventId(fixture, prompt);
    let calls = 0;
    const fetch: typeof globalThis.fetch = async () => {
      calls += 1;
      return openAiResponse(providerOutput(sourceId));
    };
    assert.equal(await runResident(fixture, {
      fetch,
      detect: async (input) => {
        const result = await detectSync(input);
        if (input.text.includes('"checkpoint_context"')) writeWorkerStop(fixture.paths);
        return result;
      },
    }), 0);
    assert.equal(calls, 0);
    fixture.withDb((db) => {
      assert.deepEqual({ ...db.prepare('SELECT state, provider_attempts FROM observation_batches').get() },
        { state: 'pending', provider_attempts: 0 });
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM provider_usage').get()?.n, 0);
      assert.equal(db.prepare('SELECT processing_offset FROM raw_events WHERE id = ?').get(sourceId)?.processing_offset, 0);
      assert.equal(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, null);
    });
    assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), /run end .*reason=stopped/);
    assert.equal(await runObserveForFixture(fixture, { fetch }), 0);
    assert.equal(calls, 1, 'the next spawn adopts the pending batch without a reclaim delay');
  });
});

test('a control after a usable response preserves the applied batch citations and log', async () => {
  const root = repositoryRoot();
  const { stdout } = await promisify(execFile)('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' });
  const head = stdout.trim();
  for (const control of ['sentinel', 'signal']) {
    await withFixture(async (fixture) => {
      fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'applied-stop-key' });
      writeConfig(fixture, 'openrouter', fixture.env);
      const prompt = 'Keep citations when stopping after a usable response.';
      await captureEndedSession(fixture, { sessionId: `applied-${control}`, cwd: root, prompts: [prompt] });
      const sourceId = eventId(fixture, prompt);
      const output = providerOutput(sourceId);
      output.observations[0]!.citations.files_read = ['package.json'];
      const listeners = process.listeners('SIGTERM');
      let applied = 0;
      const exit = await runResident(fixture, {
        fetch: async () => openAiResponse(output),
        applyHook: () => {
          applied += 1;
          if (control === 'sentinel') writeWorkerStop(fixture.paths);
          else assert.equal(signalWorker('SIGTERM', listeners), true);
        },
      });
      assert.equal(exit, 0);
      assert.equal(applied, 1, 'the control becomes true after a usable provider response');
      const log = readFileSync(fixture.paths.observeLog, 'utf8');
      assert.match(log, new RegExp(`run end .*reason=${control === 'sentinel' ? 'stopped' : 'signal'}`));
      const batchId = fixture.withDb((db) => {
        const batch = db.prepare('SELECT id, state FROM observation_batches').get();
        assert.equal(batch?.state, 'applied');
        const memories = db.prepare('SELECT id, citations_head, citations_ok FROM memories WHERE source_batch_id = ?')
          .all(String(batch?.id));
        assert.ok(memories.length > 0, 'the accepted response must produce memories');
        for (const memory of memories) {
          assert.equal(memory.citations_head, head, `${control}: applied memories must have citations stamped after a control`);
          assert.equal(memory.citations_ok, 1);
          assert.ok(db.prepare('SELECT 1 FROM memory_sources WHERE memory_id = ? AND raw_event_id = ? AND context_only = 0')
            .get(String(memory.id), sourceId));
        }
        assert.ok(db.prepare(`SELECT 1 FROM memory_sources ms JOIN memories m ON m.id = ms.memory_id
          WHERE m.source_batch_id = ? AND ms.citation_kind = 'file_read' AND ms.citation_value = 'package.json'
            AND ms.context_only = 0`).get(String(batch?.id)));
        return String(batch?.id);
      });
      assert.ok(log.includes(` batch id=${batchId} state=applied `),
        `${control}: the applied batch must be logged after a control`);
    });
  }
});

test('the heartbeat fires during a delayed apply and the lease survives it', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'delayed-apply-key' });
    writeConfig(fixture, 'openrouter', fixture.env);
    const prompt = 'A delayed apply must not let the lease go stale.';
    await captureEndedSession(fixture, { sessionId: 'delayed-apply', prompts: [prompt] });
    const sourceId = eventId(fixture, prompt);
    let wall = NOW;
    let duringApply: Record<string, unknown> | undefined;
    const exit = await runResident(fixture, {
      ...residentClock(() => { writeWorkerStop(fixture.paths); }),
      now: () => wall,
      heartbeatMs: 5,
      fetch: async () => openAiResponse(providerOutput(sourceId)),
      applyHook: async () => {
        wall += 1_000;
        duringApply = await heartbeatAtLeast(fixture, wall);
      },
    });
    assert.equal(exit, 0);
    assert.equal(duringApply?.heartbeat_at, NOW + 1_000, 'the schedule heartbeats while the apply is in flight');
    assert.equal(typeof duringApply?.owner_token, 'string', 'the lease is still owned when the apply commits');
    const log = readFileSync(fixture.paths.observeLog, 'utf8');
    assert.doesNotMatch(log, /reason=lease_lost/);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT state FROM observation_batches').get()?.state, 'applied');
    });
  });
});

test('a stop after a response prevents both output and language retries', async () => {
  for (const retry of ['output', 'language']) {
    await withFixture(async (fixture) => {
      fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'request-retry-stop-key' });
      writeConfig(fixture, 'openrouter', fixture.env);
      const prompt = 'アップロードの再試行について記録してください。';
      await captureEndedSession(fixture, { sessionId: `stop-${retry}-retry`, prompts: [prompt] });
      let calls = 0;
      const exit = await runResident(fixture, {
        fetch: async () => {
          calls += 1;
          writeWorkerStop(fixture.paths);
          const output = providerOutput(eventId(fixture, prompt));
          if (retry === 'output') output.observations[0]!.title = '';
          return openAiResponse(output);
        },
      });
      assert.equal(exit, 0);
      assert.equal(calls, 1, retry);
      assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), /run end .*reason=stopped/);
      fixture.withDb((db) => {
        assert.equal(db.prepare('SELECT state FROM observation_batches').get()?.state, 'running');
        assert.equal(db.prepare('SELECT COUNT(*) AS n FROM memories').get()?.n, 0);
      });
    });
  }
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
      const exit = await runResident(fixture, {
        engineArtifact: artifact,
        ...residentClock((polls) => { item.poke(fixture, polls); }),
      });
      assert.equal(exit, 0, item.name);
      assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), new RegExp(`reason=${item.reason}`), item.name);
    });
  }
});

test('a config malformed at startup exits as config_changed before loading the worker config', async () => {
  await withFixture(async (fixture) => {
    writeFileSync(fixture.paths.config, '[observer\npreset = ');
    assert.equal(await runResident(fixture), 0);
    assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), /run end .*reason=config_changed/);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, null);
    });
  });
});

test('capture activity resets the idle budget while an unchanged session expires', async () => {
  for (const advances of [true, false]) {
    await withFixture(async (fixture) => {
      writeConfig(fixture, 'none');
      await fixture.capture('SessionStart', {
        session_id: 'idle-capture', cwd: process.cwd(), source: 'startup',
      });
      fixture.withDb((db) => {
        assert.equal(db.prepare('SELECT COUNT(*) AS n FROM sessions').get()?.n, 1);
        assert.equal(queueIsEmpty(db, fixture.paths, '', NOW), true);
      });
      const clock = residentClock((polls) => {
        if (advances && polls === 1) {
          fixture.withDb((db) => {
            db.prepare('UPDATE sessions SET last_captured_at = ?').run(NOW);
          });
        }
        if (polls === 3) writeWorkerStop(fixture.paths);
      }, 450_000);
      assert.equal(await runResident(fixture, clock), 0);
      const log = readFileSync(fixture.paths.observeLog, 'utf8');
      if (advances) {
        assert.match(log, /run end .*reason=stopped/,
          'capture activity at 450000 ms must keep the resident alive past 900000 ms');
        assert.equal(clock.elapsedMs(), 1_350_000);
      } else {
        assert.match(log, /run end .*reason=idle_exit/, 'an unchanged session must exhaust the idle budget');
        assert.equal(clock.elapsedMs(), 900_000);
      }
    });
  }
});

test('a backward system clock does not read continuing captures as idleness', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await fixture.capture('SessionStart', {
      session_id: 'idle-backward', cwd: process.cwd(), source: 'startup',
    });
    // The session starts with a capture stamp the startup poll reads, and each later poll writes an
    // EARLIER one, which is what a backward system-clock correction produces: activity is still
    // activity, so the idle budget must keep resetting.
    let stamp = NOW;
    fixture.withDb((db) => { db.prepare('UPDATE sessions SET last_captured_at = ?').run(stamp); });
    const clock = residentClock((polls) => {
      stamp -= 60_000;
      fixture.withDb((db) => { db.prepare('UPDATE sessions SET last_captured_at = ?').run(stamp); });
      if (polls === 3) writeWorkerStop(fixture.paths);
    }, 450_000);
    assert.equal(await runResident(fixture, clock), 0);
    assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), /run end .*reason=stopped/,
      'a capture whose stamp moved backward must still reset the idle budget');
    assert.equal(clock.elapsedMs(), 1_350_000);
  });
});

// Invoke the newly installed worker handler without triggering node:test's own signal handler.
function signalWorker(signal: NodeJS.Signals, existing: NodeJS.SignalsListener[]): boolean {
  const handler = process.listeners(signal).find((listener) => !existing.includes(listener));
  if (handler === undefined) return false;
  handler(signal);
  return true;
}

test('signals interrupt an injected wait during an epoch and release the lease', async () => {
  for (const signal of ['SIGTERM', 'SIGINT'] as const) {
    await withFixture(async (fixture) => {
      await captureRunningBatch(fixture);
      let elapsed = 0;
      let sleptToEnd = false;
      let timer: ReturnType<typeof setTimeout> | undefined;
      const listeners = process.listeners(signal);
      try {
        const exit = await runResident(fixture, {
          elapsedMs: () => elapsed,
          sleep: (ms) => new Promise((resolve) => {
            assert.equal(ms, 200, 'the signal lands inside the active epoch');
            queueMicrotask(() => signalWorker(signal, listeners));
            timer = setTimeout(() => {
              sleptToEnd = true;
              elapsed += ms;
              resolve();
            }, 100);
          }),
        });
        assert.equal(exit, 0);
        assert.equal(sleptToEnd, false, 'the signal wakes an injected sleep before it resolves');
        assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), /run end .*reason=signal/);
        fixture.withDb((db) => {
          assert.equal(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, null);
          assert.equal(db.prepare('SELECT state FROM observation_batches').get()?.state, 'running');
        });
        assert.deepEqual(process.listeners(signal), listeners);
      } finally {
        clearTimeout(timer);
      }
    });
  }
});

test('SIGTERM cancels the native idle timer so the resident process exits promptly', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const child = spawn(process.execPath, [join(repositoryRoot(), 'dist', 'oboete.mjs'), 'observe', '--resident'], {
      env: fixture.env,
      stdio: 'ignore',
    });
    const finished = new Promise<{ status: number | null; signal: NodeJS.Signals | null }>((resolve, reject) => {
      child.once('error', reject);
      child.once('close', (status, signal) => resolve({ status, signal }));
    });
    void finished.catch(() => undefined);
    const watchdog = setTimeout(() => child.kill('SIGKILL'), 10_000);
    try {
      const deadline = performance.now() + 8_000;
      let idle = false;
      while (performance.now() < deadline) {
        assert.equal(child.exitCode, null, 'the resident must stay alive until signalled');
        assert.equal(child.signalCode, null);
        idle = fixture.withDb((db) => {
          const lease = db.prepare('SELECT pid, started_at, heartbeat_at FROM worker_lease WHERE id = 1').get();
          return lease?.pid === child.pid && Number(lease?.heartbeat_at) > Number(lease?.started_at);
        });
        if (idle) break;
        await delay(20);
      }
      assert.equal(idle, true, 'the empty resident must reach its first periodic heartbeat');
      // The heartbeat precedes the next idle wait; signal near the beginning of that wait.
      await delay(100);
      const signalledAt = performance.now();
      assert.equal(child.kill('SIGTERM'), true);
      assert.deepEqual(await finished, { status: 0, signal: null });
      const elapsed = performance.now() - signalledAt;
      if (WALL_CLOCK_IS_MEASURED) {
        assert.ok(elapsed < 1_000, `the native idle timer kept the process alive for ${elapsed.toFixed(0)} ms`);
      }
      assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), /run end .*reason=signal/);
      fixture.withDb((db) => {
        assert.equal(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, null);
      });
    } finally {
      clearTimeout(watchdog);
      if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
      await finished.catch(() => undefined);
    }
  });
});

test('signal handlers survive shutdown and a signalled worker preserves the stop sentinel', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    let signalled = false;
    let handledDuringShutdown = false;
    const listeners = process.listeners('SIGINT');
    const termListeners = process.listeners('SIGTERM');
    const exit = await runResident(fixture, {
      now: () => {
        if (signalled && !handledDuringShutdown) {
          fixture.withDb((db) => {
            assert.equal(typeof db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, 'string');
          });
          handledDuringShutdown = signalWorker('SIGINT', listeners);
        }
        return NOW;
      },
      sleep: async () => {
        writeWorkerStop(fixture.paths);
        signalled = signalWorker('SIGTERM', termListeners);
      },
    });
    assert.equal(exit, 0);
    assert.equal(handledDuringShutdown, true, 'a second signal still has a handler before release');
    assert.equal(existsSync(fixture.paths.workerStop), true);
    assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), /run end .*reason=signal/);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, null);
    });
    assert.deepEqual(process.listeners('SIGINT'), listeners);
  });
});

test('a fallback epoch still exits 0 on a cooperative stop', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await captureEndedSession(fixture, {
      sessionId: 'fallback-then-stop',
      prompts: ['Fallback must not force exit 1 on stop.'],
    });
    const exit = await runResident(fixture, residentClock(() => { writeWorkerStop(fixture.paths); }));
    assert.equal(exit, 0);
    assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), /reason=stopped/);
    fixture.withDb((db) => {
      assert.ok(Number(db.prepare("SELECT COUNT(*) AS n FROM observation_batches WHERE state = 'fallback'").get()?.n) >= 1);
    });
  });
});

test('fallback exits keep worker and storage error codes in resident mode', async () => {
  for (const storage of [false, true]) {
    await withFixture(async (fixture) => {
      writeConfig(fixture, 'none');
      await captureEndedSession(fixture, {
        sessionId: 'fallback-error', prompts: ['Report a failure after a fallback.'],
      });
      const exit = await runResident(fixture, {
        sleep: async () => {
          throw storage ? Object.assign(new Error('fixture storage failure'), { code: 'EIO' })
            : new Error('fixture worker failure');
        },
      });
      assert.equal(exit, storage ? 3 : 1);
      assert.match(readFileSync(fixture.paths.observeLog, 'utf8'),
        new RegExp(`run end .*reason=${storage ? 'storage_error' : 'worker_error'}`));
    });
  }
});

test('an idle exit preserves a stop sentinel written during that exit', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    let elapsed = 0;
    assert.equal(await runResident(fixture, {
      elapsedMs: () => {
        if (elapsed === 900_000) writeWorkerStop(fixture.paths);
        return elapsed;
      },
      sleep: async () => { elapsed = 900_000; },
    }), 0);
    assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), /run end .*reason=idle_exit/);
    assert.equal(existsSync(fixture.paths.workerStop), true);
  });
});

test('a maintenance epoch purges an expired secret with no batchable work', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    const prompt = 'Delete expired private material.';
    await captureEndedSession(fixture, { sessionId: 'secret-retention', prompts: [prompt] });
    const sourceId = eventId(fixture, prompt);
    fixture.withDb((db) => {
      db.exec("UPDATE sessions SET summary_state = 'done'");
      db.exec('UPDATE raw_events SET work_binding_id = NULL');
      db.prepare("UPDATE raw_events SET sensitivity = 'secret', classification_state = 'done', expires_at = ? WHERE id = ?")
        .run(NOW - 1, sourceId);
    });
    // The queue is empty, so only the maintenance interval can open the epoch that purges.
    let elapsed = 0;
    assert.equal(await runResident(fixture, {
      elapsedMs: () => elapsed,
      sleep: async () => {
        if (elapsed === 0) elapsed = 60_000;
        else writeWorkerStop(fixture.paths);
      },
    }), 0);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT id FROM raw_events WHERE id = ?').get(sourceId), undefined);
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM observation_batches').get()?.n, 0);
      assert.equal(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, null);
    });
    assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), /epoch .*purged=1/);
  });
});

test('worker-stop is removed before the lease is released and pause is not consumed', async () => {
  await withFixture(async (fixture) => {
    writeConfig(fixture, 'none');
    await captureEndedSession(fixture, {
      sessionId: 'stop-consumed',
      prompts: ['Stop the resident once it is idle.'],
    });
    let stopWritten = false;
    let beforeRelease: { stopExists: boolean; leaseOwned: boolean } | undefined;
    const clock = residentClock(() => {
      writeWorkerStop(fixture.paths);
      stopWritten = true;
    });
    assert.equal(await runResident(fixture, {
      ...clock,
      now: () => {
        // After the idle wait writes stop, the next now() evaluates releaseForExit's arguments.
        if (stopWritten && beforeRelease === undefined) {
          beforeRelease = fixture.withDb((db) => ({
            stopExists: existsSync(fixture.paths.workerStop),
            leaseOwned: typeof db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token === 'string',
          }));
        }
        return NOW;
      },
    }), 0);
    assert.deepEqual(beforeRelease, { stopExists: false, leaseOwned: true },
      'worker-stop must be absent while the lease is still owned immediately before release');
    assert.equal(readFileSync(fixture.paths.observeLog, 'utf8').includes('reason=stopped'), true);
    assert.equal(existsSync(fixture.paths.workerStop), false);
    fixture.withDb((db) => {
      assert.equal(db.prepare('SELECT owner_token FROM worker_lease WHERE id = 1').get()?.owner_token, null);
    });

    writeFileSync(fixture.paths.paused, '');
    const beforePause = readFileSync(fixture.paths.observeLog, 'utf8');
    const pausedExit = await runResident(fixture);
    assert.equal(pausedExit, 0);
    assert.equal(existsSync(fixture.paths.paused), true);
    assert.equal(readFileSync(fixture.paths.observeLog, 'utf8'), beforePause, 'the paused run never starts a worker');
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
    const exit = await runResident(fixture, {
      now: () => wall,
      maxRunMs: 5_000,
      ...residentClock((polls) => {
        wall += 60 * 60_000;
        if (polls === 2) writeWorkerStop(fixture.paths);
      }),
    });
    assert.equal(exit, 0);
    assert.match(readFileSync(fixture.paths.observeLog, 'utf8'), /reason=stopped/);
    assert.equal(readFileSync(fixture.paths.observeLog, 'utf8').includes('reason=max_run'), false);
  });
});

test('a wall-clock jump during apply does not cut the active epoch short', async () => {
  await withFixture(async (fixture) => {
    fixture.env = cleanEnv(fixture.home, { OBOETE_OPENROUTER_API_KEY: 'in-flight-clock-key' });
    writeConfig(fixture, 'openrouter', fixture.env);
    const prompt = 'Finish the active epoch despite a wall-clock jump.';
    await captureEndedSession(fixture, { sessionId: 'in-flight-clock', prompts: [prompt] });
    const sourceId = eventId(fixture, prompt);
    let wall = NOW;
    let applied = 0;
    const clock = residentClock(() => { writeWorkerStop(fixture.paths); });
    const exit = await runResident(fixture, {
      ...clock,
      now: () => wall,
      maxRunMs: 5_000,
      fetch: async () => openAiResponse(providerOutput(sourceId)),
      applyHook: () => {
        applied += 1;
        wall += 60 * 60_000;
      },
    });
    assert.equal(exit, 0);
    assert.equal(applied, 1, 'the wall clock jumps while the batch is in flight');
    assert.equal(clock.elapsedMs(), 2_000);
    fixture.withDb((db) => {
      const batch = db.prepare('SELECT state, completed_at FROM observation_batches').get();
      assert.equal(batch?.state, 'applied');
      assert.equal(batch?.completed_at, NOW + 60 * 60_000);
      assert.equal(db.prepare('SELECT summary_state FROM sessions').get()?.summary_state, 'done',
        'a wall-clock jump must not cut the active epoch short before its session summary');
    });
    const log = readFileSync(fixture.paths.observeLog, 'utf8');
    assert.match(log, /run end .*reason=stopped/);
    assert.doesNotMatch(log, /reason=max_run/);
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
