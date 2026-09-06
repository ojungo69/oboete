import assert from 'node:assert/strict';
import type { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { EventEmitter } from 'node:events';
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  unlinkSync,
  utimesSync,
  writeFileSync,
} from 'node:fs';
import { basename, join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { PRESET_CATALOG, configSchema, consentHash, consentTuple } from '../../src/config.js';
import { openDatabase } from '../../src/db/open.js';
import { runDoctor, type DoctorDeps, type DoctorItem } from '../../src/doctor.js';
import { probeReason } from '../../src/doctor/agents.js';
import { allowanceItem, catalogItems, providerItem } from '../../src/doctor/provider.js';
import { ftsItem, migrationItem, openStorage, spoolItem, workerItem } from '../../src/doctor/storage.js';
import { ensureDirectories, oboetePaths, type OboetePaths } from '../../src/paths.js';
import type { VersionSpawn } from '../../src/setup/detect.js';
import { removeJsonHandlers } from '../../src/setup/managed-block.js';
import { runSetup, type SetupDeps } from '../../src/setup/setup.js';
import { utcDay } from '../../src/observer/reservation.js';
import { runtimeStateSet } from '../../src/worker/purge.js';
import { withTempHome } from '../helpers/home.js';

const NODE = '/usr/bin/node';
const BUNDLE = '/opt/oboete/dist/oboete.mjs';

const TOKEN = 'test-token-value';
const ACCOUNT = 'test-account-id';

const noVersion: VersionSpawn = (command) => {
  throw new Error(`no version probe expected: ${command}`);
};

const versionOk = (() => ({ status: 0, stdout: '1.0.0\n', stderr: '' })) as unknown as VersionSpawn;

type Report = { items: DoctorItem[]; notes: unknown; view: unknown };

type Harness = {
  home: string;
  userHome: string;
  paths: ReturnType<typeof oboetePaths>;
  env: NodeJS.ProcessEnv;
  output: string;
  now: number;
  spawn: typeof spawn;
  fetch: typeof globalThis.fetch;
  doctor(argv?: string[], overrides?: Partial<DoctorDeps>): Promise<number>;
  setup(argv: string[], overrides?: Partial<SetupDeps>): Promise<number>;
  report(): Report;
  item(name: string): DoctorItem;
};

function agentHomes(userHome: string): void {
  for (const directory of ['.claude', '.codex', '.grok', join('.pi', 'agent')]) {
    mkdirSync(join(userHome, directory), { recursive: true });
  }
}

function stubBinaries(bin: string): void {
  mkdirSync(bin, { recursive: true });
  for (const name of ['claude', 'codex', 'grok', 'pi']) {
    writeFileSync(join(bin, name), '#!/bin/sh\nexit 0\n');
    chmodSync(join(bin, name), 0o755);
  }
}

function storeMarker(dbPath: string, agent: string, marker: string): void {
  const { db } = openDatabase({ path: dbPath, timeoutMs: 5_000 });
  try {
    db.prepare(
      `INSERT OR IGNORE INTO repos (id, identity_kind, normalized_identity) VALUES ('doctor-probe', 'common_dir', 'doctor-probe')`,
    ).run();
    db.prepare(
      `INSERT OR IGNORE INTO sessions (id, repo_id, agent, native_session_id, conversation_id, status)
       VALUES (?, 'doctor-probe', ?, ?, ?, 'active')`,
    ).run(`probe-${agent}`, agent, `native-${agent}`, `conv-${agent}`);
    db.prepare(
      `INSERT INTO raw_events (id, repo_id, session_id, agent, kind, content, sensitivity, classification_state, captured_at, expires_at)
       VALUES (?, 'doctor-probe', ?, ?, 'prompt', ?, 'local_only', 'done', ?, ?)`,
    ).run(randomUUID(), `probe-${agent}`, agent, marker, Date.now(), Date.now() + 86_400_000);
  } finally {
    db.close();
  }
}

function markerFromArgs(args: readonly string[]): string | undefined {
  for (const arg of args) {
    const match = /oboete-probe:[0-9a-f-]+/i.exec(arg);
    if (match) return match[0];
  }
  return undefined;
}

function closingChild(code: number): EventEmitter {
  const child = new EventEmitter();
  queueMicrotask(() => child.emit('close', code, null));
  return child;
}

function storingSpawn(dbPath: string): typeof spawn {
  return ((command: string, args: readonly string[]) => {
    const agent = basename(command);
    const marker = markerFromArgs(args);
    if (marker !== undefined && ['claude', 'codex', 'grok', 'pi'].includes(agent)) {
      try {
        storeMarker(dbPath, agent, marker);
      } catch {
        // The probe lookup reports a miss; throwing here would look like spawn_failed.
      }
    }
    return closingChild(0);
  }) as unknown as typeof spawn;
}

function observerOutput(): unknown {
  return {
    observations: [
      {
        type: 'bugfix',
        title: 'Doctor probe',
        body: 'The provider answered the doctor probe.',
        concepts: ['problem-solution'],
        citations: { files_read: [], files_modified: [], commits: [] },
        source_event_ids: ['e1'],
        classification: { decision: 'add', target: null, reason: 'probe' },
      },
    ],
  };
}

function answeringFetch(): typeof globalThis.fetch {
  return async () => {
    const headers = new Headers();
    headers.set('content-type', 'application/json');
    return new Response(
      JSON.stringify({
        success: true,
        result: {
          response: observerOutput(),
          usage: { prompt_tokens: 8, completion_tokens: 2 },
        },
        errors: [],
        messages: [],
      }),
      { status: 200, headers },
    );
  };
}

function refusingFetch(): typeof globalThis.fetch {
  return async () => {
    const error = new Error('connect ECONNREFUSED 127.0.0.1:1') as NodeJS.ErrnoException;
    error.code = 'ECONNREFUSED';
    throw error;
  };
}

function ollamaAnsweringFetch(): typeof globalThis.fetch {
  return async () => {
    const headers = new Headers();
    headers.set('content-type', 'application/json');
    return new Response(
      JSON.stringify({
        id: 'response-1',
        model: 'qwen3:8b',
        choices: [
          {
            index: 0,
            message: { role: 'assistant', content: JSON.stringify(observerOutput()) },
            finish_reason: 'stop',
          },
        ],
        usage: { prompt_tokens: 8, completion_tokens: 2, total_tokens: 10 },
      }),
      { status: 200, headers },
    );
  };
}

function corruptQuickCheck(dbPath: string): void {
  for (const suffix of ['-wal', '-shm']) {
    try {
      unlinkSync(`${dbPath}${suffix}`);
    } catch {
      // Sidecar may be absent.
    }
  }
  const original = readFileSync(dbPath);
  let pageSize = original.readUInt16BE(16);
  if (pageSize === 1) pageSize = 65_536;
  if (pageSize < 512) pageSize = 4_096;
  const start = pageSize;
  const padded =
    original.length < start + pageSize
      ? Buffer.concat([original, Buffer.alloc(start + pageSize - original.length)])
      : Buffer.from(original);
  padded.fill(0, start, start + pageSize);
  writeFileSync(dbPath, padded);
}

async function harness(fn: (context: Harness) => Promise<void>): Promise<void> {
  await withTempHome(async (home) => {
    const userHome = join(home, 'user');
    const bin = join(home, 'bin');
    agentHomes(userHome);
    stubBinaries(bin);
    const paths = oboetePaths(home);
    const env: NodeJS.ProcessEnv = {
      HOME: userHome,
      PATH: bin,
      OBOETE_HOME: home,
      OBOETE_CF_API_TOKEN: TOKEN,
      OBOETE_CF_ACCOUNT_ID: ACCOUNT,
    };
    const context: Harness = {
      home,
      userHome,
      paths,
      env,
      output: '',
      now: Date.now(),
      spawn: storingSpawn(paths.db),
      fetch: answeringFetch(),
      async setup(argv, overrides = {}) {
        return await runSetup(argv, {
          env,
          versionSpawn: versionOk,
          spawn: context.spawn,
          runCli: () => ({ ok: true, reason: '' }),
          write: () => undefined,
          node: NODE,
          bundle: BUNDLE,
          ...overrides,
        });
      },
      async doctor(argv = ['--json'], overrides = {}) {
        context.output = '';
        return await runDoctor(argv, {
          env,
          versionSpawn: versionOk,
          spawn: context.spawn,
          fetch: context.fetch,
          now: () => context.now,
          write: (text) => {
            context.output += text;
          },
          ...overrides,
        });
      },
      report() {
        return JSON.parse(context.output) as Report;
      },
      item(name) {
        const found = context.report().items.find((entry) => entry.item === name);
        assert.ok(found, `missing ${name} in ${context.output}`);
        return found;
      },
    };
    assert.equal(await context.setup(['--accept-egress']), 0);
    await fn(context);
  });
}

function assertBroken(
  entry: DoctorItem,
  status: 'degraded' | 'warning' | 'unverified',
  ...words: string[]
): void {
  assert.equal(entry.status, status, `${entry.item}: ${entry.reason}`);
  assert.notEqual(entry.reason.trim(), '', `${entry.item} reason`);
  assert.notEqual(entry.consequence.trim(), '', `${entry.item} consequence`);
  assert.notEqual(entry.recovery.trim(), '', `${entry.item} recovery`);
  const blob = `${entry.reason}\n${entry.consequence}\n${entry.recovery}`;
  for (const word of words) assert.match(blob, new RegExp(word, 'i'), blob);
}

test('hook entry removed degrades agent:claude and setup restores it', async () => {
  await harness(async (context) => {
    const settings = join(context.userHome, '.claude', 'settings.json');
    removeJsonHandlers(settings);

    const broken = await context.doctor();
    assert.equal(broken, 1, context.output);
    assertBroken(context.item('agent:claude'), 'degraded', 'hook', 'settings.json', 'setup');

    assert.equal(await context.setup(['--agents', 'claude', '--yes']), 0);
    const restored = await context.doctor();
    assert.equal(restored, 0, context.output);
    assert.equal(context.item('agent:claude').status, 'healthy');
  });
});

for (const [agent, label] of [['grok', 'Grok'], ['codex', 'Codex']]) {
  test(`doctor reports the marker-less ${agent} table and setup repairs it`, async () => {
    await harness(async (context) => {
      const configPath = join(context.userHome, `.${agent}`, 'config.toml');
      const unmarked = readFileSync(configPath, 'utf8').replace(/^# oboete:(?:begin|end)\n/gm, '');
      writeFileSync(configPath, unmarked);

      for (const argv of [['--json'], ['--json', '--no-probe-agents']]) {
        assert.equal(await context.doctor(argv), 1, context.output);
        const item = context.item(`agent:${agent}`);
        assert.equal(item.status, 'degraded');
        assert.equal(item.reason, `${label} rewrote its config.toml and dropped the oboete markers; the MCP table is still there.`);
        assert.equal(item.recovery, `Run \`oboete setup --agents ${agent}\`.`);
        assert.equal(readFileSync(configPath, 'utf8'), unmarked, 'doctor only reads the file');
      }

      assert.equal(await context.setup(['--agents', agent, '--yes']), 0);
      assert.equal(await context.doctor(), 0, context.output);
      assert.equal(context.item(`agent:${agent}`).status, 'healthy');
    });
  });

  test(`doctor does not claim a foreign ${agent} table lost oboete markers`, async () => {
    await harness(async (context) => {
      const configPath = join(context.userHome, `.${agent}`, 'config.toml');
      const unmarked = readFileSync(configPath, 'utf8')
        .replace(/^# oboete:(?:begin|end)\n/gm, '')
        .replace(`command = "${NODE}"`, 'command = "foreign-server"');
      writeFileSync(configPath, unmarked);
      await context.doctor(['--json', '--no-probe-agents']);
      assert.doesNotMatch(context.item(`agent:${agent}`).reason, /dropped the oboete markers/);
    });
  });
}

test('every probe outcome has a sentence for every agent', () => {
  const outcomes = {
    agent_not_installed: 'Grok is not installed, so the probe could not run.',
    spawn_failed: 'Grok could not be started for the probe.',
    probe_event_stored: 'Grok ran and its capture event reached oboete.',
    probe_lookup_failed: 'The capture event from Grok could not be checked in the oboete database.',
    probe_event_missing: 'Grok ran but no capture event reached oboete.',
    agent_exit_7: 'Grok exited with code 7 before the probe finished.',
    agent_exit_signal: 'Grok was stopped by a signal before the probe finished.',
    deadline_exceeded: 'Grok did not finish the probe within 90 seconds.',
  };
  for (const [code, sentence] of Object.entries(outcomes)) {
    for (const label of ['Grok', 'Codex', 'Claude', 'Pi']) {
      assert.equal(probeReason(label, code), sentence.replace('Grok', label));
    }
  }
  assert.equal(probeReason('Grok', 'unknown_outcome'), 'The Grok probe could not be verified.');
});

test('doctor renders an agent exit as a sentence', async () => {
  await harness(async (context) => {
    context.spawn = ((command: string, args: readonly string[]) => {
      if (basename(command) === 'grok') return closingChild(7);
      return storingSpawn(context.paths.db)(command, [...args]);
    }) as unknown as typeof spawn;
    assert.equal(await context.doctor(), 1, context.output);
    assert.equal(context.item('agent:grok').reason, 'Grok exited with code 7 before the probe finished.');
  });
});

test('database chmod 0o444 degrades storage with exit 1 and chmod 0o600 restores it', async () => {
  await harness(async (context) => {
    chmodSync(context.paths.db, 0o444);
    const broken = await context.doctor();
    assert.equal(broken, 1, context.output);
    assertBroken(context.item('storage'), 'degraded', 'writable|chmod|not writable', 'summarized', 'chmod');

    chmodSync(context.paths.db, 0o600);
    for (const suffix of ['-wal', '-shm']) {
      try {
        chmodSync(`${context.paths.db}${suffix}`, 0o600);
      } catch {
        // The sidecar may be absent.
      }
    }
    const restored = await context.doctor();
    assert.equal(restored, 0, context.output);
    assert.equal(context.item('storage').status, 'healthy');
  });
});

test('corrupted header degrades storage with exit 3 and restore turns it green', async () => {
  await harness(async (context) => {
    const original = readFileSync(context.paths.db);
    const wal = existsSync(`${context.paths.db}-wal`) ? readFileSync(`${context.paths.db}-wal`) : null;
    const shm = existsSync(`${context.paths.db}-shm`) ? readFileSync(`${context.paths.db}-shm`) : null;
    const buf = Buffer.from(original);
    buf.fill(0x58, 0, Math.min(100, buf.length));
    writeFileSync(context.paths.db, buf);

    const broken = await context.doctor();
    assert.equal(broken, 3, context.output);
    assertBroken(
      context.item('storage'),
      'degraded',
      'database',
      'oboete export',
      'oboete setup',
      'oboete import',
    );

    writeFileSync(context.paths.db, original);
    if (wal !== null) writeFileSync(`${context.paths.db}-wal`, wal);
    if (shm !== null) writeFileSync(`${context.paths.db}-shm`, shm);
    const restored = await context.doctor();
    assert.equal(restored, 0, context.output);
    assert.equal(context.item('storage').status, 'healthy');
  });
});

test('a stale worker lease degrades worker and releasing it restores health', async () => {
  await harness(async (context) => {
    const { db } = openDatabase({ path: context.paths.db, timeoutMs: 2_000 });
    try {
      db.prepare(
        `UPDATE worker_lease SET owner_token = 't', pid = 4242, heartbeat_at = ? WHERE id = 1`,
      ).run(context.now - 60_000);
    } finally {
      db.close();
    }

    const broken = await context.doctor();
    assert.equal(broken, 1, context.output);
    assertBroken(context.item('worker'), 'degraded', '4242', 'heartbeat', 'observe');

    const again = openDatabase({ path: context.paths.db, timeoutMs: 2_000 });
    try {
      again.db.prepare('UPDATE worker_lease SET owner_token = NULL, pid = NULL WHERE id = 1').run();
    } finally {
      again.db.close();
    }
    const restored = await context.doctor();
    assert.equal(restored, 0, context.output);
    assert.equal(context.item('worker').status, 'healthy');
  });
});

test('an unreachable provider degrades provider and an answering fetch restores it', async () => {
  await harness(async (context) => {
    context.fetch = refusingFetch();
    const broken = await context.doctor(['--json', '--probe-provider']);
    assert.equal(broken, 1, context.output);
    const failed = context.item('provider');
    assertBroken(failed, 'degraded', 'Provider request failed', 'rule-based', 'network|host');
    assert.match(failed.reason, /^Provider request failed.*\.$/);

    context.fetch = answeringFetch();
    const restored = await context.doctor(['--json', '--probe-provider']);
    assert.equal(restored, 0, context.output);
    assert.equal(context.item('provider').status, 'healthy');

    const { db } = openDatabase({ path: context.paths.db, timeoutMs: 2_000 });
    try {
      const row = db
        .prepare('SELECT calls FROM provider_usage WHERE utc_day = ? AND preset = ?')
        .get(utcDay(context.now), 'workers-ai');
      assert.equal(Number(row?.calls), 2, 'one increment per probe');
    } finally {
      db.close();
    }
  });
});

test('an exhausted allowance degrades and advancing now past reset_at restores it', async () => {
  await harness(async (context) => {
    const now = Date.UTC(2026, 8, 6, 12, 0, 0);
    const resetAt = Date.UTC(2026, 8, 7, 0, 0, 0);
    context.now = now;
    const { db } = openDatabase({ path: context.paths.db, timeoutMs: 2_000 });
    try {
      db.prepare(
        `INSERT INTO provider_usage (utc_day, preset, calls, neurons_estimate, reset_at, exhausted_at)
         VALUES (?, 'workers-ai', 10, 0, ?, ?)`,
      ).run(utcDay(now), resetAt, now);
    } finally {
      db.close();
    }

    const broken = await context.doctor();
    assert.equal(broken, 1, context.output);
    assertBroken(context.item('allowance'), 'degraded', 'exhaust', 'fallback', 'reset');

    context.now = resetAt + 1;
    const restored = await context.doctor();
    assert.equal(restored, 0, context.output);
    assert.equal(context.item('allowance').status, 'healthy');
    assert.match(context.item('allowance').reason, /Estimated/);
  });
});

test('a stale Pi .started file degrades pi and deleting it restores health', async () => {
  await harness(async (context) => {
    const file = join(context.paths.piAck, 'abc.started');
    writeFileSync(file, '');
    const past = new Date(context.now - 60_000);
    utimesSync(file, past, past);

    const broken = await context.doctor();
    assert.equal(broken, 1, context.output);
    assertBroken(context.item('pi'), 'degraded', 'pi_child_hang', 'captured', 'observe');

    unlinkSync(file);
    const restored = await context.doctor();
    assert.equal(restored, 0, context.output);
    assert.equal(context.item('pi').status, 'healthy');
  });
});

test('a Pi spawn failure degrades agent:pi with a sentence', async () => {
  await harness(async (context) => {
    context.spawn = ((command: string, args: readonly string[]) => {
      if (basename(command) === 'pi') {
        const child = new EventEmitter();
        const error = new Error('spawn pi ENOENT') as NodeJS.ErrnoException;
        error.code = 'ENOENT';
        queueMicrotask(() => child.emit('error', error));
        return child;
      }
      const agent = basename(command);
      const marker = markerFromArgs(args);
      if (marker !== undefined) storeMarker(context.paths.db, agent, marker);
      return closingChild(0);
    }) as unknown as typeof spawn;

    const broken = await context.doctor();
    assert.equal(broken, 1, context.output);
    const entry = context.item('agent:pi');
    assertBroken(entry, 'degraded', 'could not be started', 'Pi', 'setup');
    assert.equal(entry.reason, 'Pi could not be started for the probe.');

    context.spawn = storingSpawn(context.paths.db);
    const restored = await context.doctor();
    assert.equal(restored, 0, context.output);
    assert.equal(context.item('agent:pi').status, 'healthy');
  });
});

test('--no-probe-agents leaves wired agents unverified, never healthy', async () => {
  await harness(async (context) => {
    const code = await context.doctor(['--json', '--no-probe-agents']);
    assert.equal(code, 0, context.output);
    const agents = context.report().items.filter((entry) => entry.item.startsWith('agent:'));
    assert.equal(agents.length, 4);
    for (const entry of agents) {
      assert.equal(entry.status, 'unverified', entry.item);
      assert.notEqual(entry.status, 'healthy');
      assert.match(entry.reason, /Not probed this run/);
    }
  });
});

test('without --probe-provider the provider item is unverified', async () => {
  await harness(async (context) => {
    const code = await context.doctor(['--json']);
    assert.equal(code, 0, context.output);
    const entry = context.item('provider');
    assert.equal(entry.status, 'unverified');
    assert.match(entry.reason, /Not probed this run/);
    assert.match(entry.recovery, /--probe-provider/);
  });
});

test('--json output parses and carries items, lexical notes, and the view line', async () => {
  await harness(async (context) => {
    const code = await context.doctor(['--json']);
    assert.equal(code, 0, context.output);
    const report = context.report();
    assert.ok(Array.isArray(report.items));
    assert.ok(report.items.length > 0);
    const notes = Array.isArray(report.notes) ? report.notes.join('\n') : String(report.notes);
    assert.match(notes, /lexical/);
    assert.match(String(report.view), /oboete view --open/);
  });
});

test('the paused marker is a warning and does not change the exit code', async () => {
  await harness(async (context) => {
    writeFileSync(context.paths.paused, '');
    const code = await context.doctor();
    assert.equal(code, 0, context.output);
    assertBroken(context.item('paused'), 'warning', 'paused', 'resume');
  });
});

test('config mode 0o644 degrades config', async () => {
  await harness(async (context) => {
    chmodSync(context.paths.config, 0o644);
    const code = await context.doctor();
    assert.equal(code, 1, context.output);
    assertBroken(context.item('config'), 'degraded', '644|0o644', 'Other users', 'chmod 600');
  });
});

test('an untrusted Codex hook degrades agent:codex and setup restores it', async () => {
  await harness(async (context) => {
    const configPath = join(context.userHome, '.codex', 'config.toml');
    const original = readFileSync(configPath, 'utf8');
    writeFileSync(
      configPath,
      original.replace(/trusted_hash = "sha256:[0-9a-f]+"/g, `trusted_hash = "sha256:${'0'.repeat(64)}"`),
    );

    const broken = await context.doctor();
    assert.equal(broken, 1, context.output);
    assertBroken(context.item('agent:codex'), 'degraded', 'Codex has not trusted');
    assert.match(context.item('agent:codex').reason, /Codex has not trusted/);

    assert.equal(await context.setup(['--agents', 'codex', '--yes']), 0);
    const restored = await context.doctor();
    assert.equal(restored, 0, context.output);
    assert.equal(context.item('agent:codex').status, 'healthy');
  });
});

test('quick_check failure degrades storage with exit 3 and does not spawn agent probes', async () => {
  await harness(async (context) => {
    corruptQuickCheck(context.paths.db);
    let spawned = 0;
    const code = await context.doctor(['--json'], {
      spawn: ((command: string) => {
        spawned += 1;
        throw new Error(`spawn must not run against a corrupt database: ${command}`);
      }) as unknown as typeof spawn,
    });
    assert.equal(code, 3, context.output);
    assertBroken(context.item('storage'), 'degraded', 'database|quick_check|malformed', 'oboete export', 'oboete setup');
    for (const name of ['agent:claude', 'agent:codex', 'agent:grok', 'agent:pi']) {
      assert.equal(context.item(name).status, 'unverified', `${name}: ${context.item(name).reason}`);
      assert.match(context.item(name).reason, /integrity check/i);
    }
    assert.equal(spawned, 0, 'probe must not spawn against a corrupt database');
  });
});

test('a database behind the schema is reported, not migrated, and the items that read it are unverified', async () => {
  await harness(async (context) => {
    const before = new DatabaseSync(context.paths.db);
    before.exec('PRAGMA user_version = 1');
    before.close();
    const code = await context.doctor(['--json', '--no-probe-agents']);
    assert.equal(code, 1, context.output);
    assertBroken(context.item('migration'), 'degraded', 'behind', 'missing', 'oboete setup');
    assert.equal(context.item('storage').status, 'healthy', context.item('storage').reason);
    for (const name of ['fts', 'worker', 'allowance']) {
      assert.equal(context.item(name).status, 'unverified', `${name}: ${context.item(name).reason}`);
    }
    const after = new DatabaseSync(context.paths.db, { readOnly: true });
    assert.equal(after.prepare('PRAGMA user_version').get()?.user_version, 1, 'doctor must not migrate the file it diagnoses');
    after.close();
  });
});

test('a throwing item is degraded and the rest of the report still prints', async () => {
  await harness(async (context) => {
    // Owner write+execute, no read: `ensureDirectories` can still see existing children, `listSpool` cannot.
    chmodSync(context.paths.spool, 0o300);
    try {
      const code = await context.doctor();
      assert.equal(code, 1, context.output);
      assertBroken(context.item('spool'), 'degraded', 'could not be checked');
      assert.equal(context.item('config').status, 'healthy');
      assert.ok(context.report().items.length > 4);
    } finally {
      chmodSync(context.paths.spool, 0o700);
    }
  });
});

test('invalid TOML degrades config and leaves provider, allowance, and catalog unverified', async () => {
  await harness(async (context) => {
    writeFileSync(context.paths.config, 'this is not [ valid toml\n');
    chmodSync(context.paths.config, 0o600);
    const code = await context.doctor();
    assert.equal(code, 1, context.output);
    assertBroken(context.item('config'), 'degraded', 'TOML|valid');
    assertBroken(context.item('provider'), 'unverified', 'configuration could not be read');
    assertBroken(context.item('allowance'), 'unverified', 'configuration could not be read');
    assertBroken(context.item('catalog'), 'unverified', 'configuration could not be read');
  });
});

test('an ollama probe is healthy and writes no provider_usage row', async () => {
  await harness(async (context) => {
    writeFileSync(context.paths.config, '[observer]\npreset = "ollama"\nmodel = "qwen3:8b"\n');
    chmodSync(context.paths.config, 0o600);
    context.fetch = ollamaAnsweringFetch();
    const code = await context.doctor(['--json', '--probe-provider']);
    assert.equal(code, 0, context.output);
    assert.equal(context.item('provider').status, 'healthy', context.item('provider').reason);
    const { db } = openDatabase({ path: context.paths.db, timeoutMs: 2_000 });
    try {
      const row = db.prepare('SELECT calls FROM provider_usage WHERE preset = ?').get('ollama');
      assert.equal(row, undefined);
    } finally {
      db.close();
    }
  });
});

test('a catalog cache from another account is unverified', async () => {
  await harness(async (context) => {
    const { db } = openDatabase({ path: context.paths.db, timeoutMs: 2_000 });
    try {
      db.prepare(
        'INSERT INTO runtime_state (key, value_json, updated_at) VALUES (?, ?, ?)',
      ).run(
        'workers_ai_catalog',
        JSON.stringify({
          accountId: 'other-account',
          models: [PRESET_CATALOG['workers-ai'].defaultModel],
          defaultModelPresent: true,
          hasPaidOnlyModels: false,
          fetchedAt: context.now,
        }),
        context.now,
      );
    } finally {
      db.close();
    }
    const code = await context.doctor();
    assert.equal(code, 0, context.output);
    assertBroken(context.item('catalog'), 'unverified', 'another account');
  });
});

test('an unknown option exits 2', async () => {
  await withTempHome(async (home) => {
    let output = '';
    const code = await runDoctor(['--nope'], {
      env: { HOME: join(home, 'user'), PATH: join(home, 'empty-bin'), OBOETE_HOME: home },
      versionSpawn: noVersion,
      spawn: (() => {
        throw new Error('no spawn');
      }) as unknown as typeof spawn,
      fetch: async () => {
        throw new Error('no fetch');
      },
      write: (text) => {
        output += text;
      },
    });
    assert.equal(code, 2);
    assert.match(output, /unknown|nope/i);
  });
});

const ITEM_NOW = Date.UTC(2026, 8, 6, 12);
const ITEM_RESET = Date.UTC(2026, 8, 7);
const itemDeps: DoctorDeps = {
  env: {},
  versionSpawn: () => assert.fail('no version probe expected'),
  spawn: () => assert.fail('no agent process expected'),
  fetch: async () => assert.fail('no network request expected'),
  write: () => assert.fail('an item returns its sentence without printing'),
  now: () => ITEM_NOW,
};
const itemOptions = { probeProvider: true, noProbeAgents: true, json: true };

async function withItemDatabase(run: (db: DatabaseSync, paths: OboetePaths) => void | Promise<void>): Promise<void> {
  await withTempHome(async (home) => {
    const paths = oboetePaths(home);
    ensureDirectories(paths);
    const { db } = openDatabase({ path: paths.db, timeoutMs: 100 });
    try { await run(db, paths); } finally { db.close(); }
  });
}

test('missing storage explains spooling without creating a database', async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(home);
    assert.deepEqual(openStorage(paths), {
      item: {
        item: 'storage', status: 'degraded', reason: `No database at ${paths.db}.`,
        consequence: 'Hooks spool every event and nothing is summarized or injected.',
        recovery: '`oboete setup`',
      },
      db: null, schemaVersion: null, schemaAhead: false, integrityFailed: false,
    });
    assert.equal(existsSync(paths.db), false);
  });
});

test('a newer database schema is diagnosed without migrating it', async () => {
  await withItemDatabase((db, paths) => {
    db.exec('PRAGMA user_version = 4');
    assert.deepEqual(openStorage(paths), {
      item: {
        item: 'storage', status: 'healthy',
        reason: `\`${paths.db}\` opened; the schema is newer than this bundle knows.`,
        consequence: '', recovery: '',
      },
      db: null, schemaVersion: 4, schemaAhead: true, integrityFailed: false,
    });
    assert.deepEqual(migrationItem(4, true, false), {
      item: 'migration', status: 'degraded',
      reason: 'The database schema is version 4, newer than this bundle knows; upgrade oboete.',
      consequence: 'This version of oboete cannot migrate or write this database.',
      recovery: 'Upgrade oboete to a version that knows schema version 4.',
    });
    assert.equal(db.prepare('PRAGMA user_version').get()?.user_version, 4);
  });
});

test('a missing full-text table explains why search and injection are unavailable', async () => {
  await withItemDatabase((db) => {
    db.exec('DROP TABLE memories_fts');
    assert.deepEqual(ftsItem(db, false), {
      item: 'fts', status: 'degraded', reason: 'no such table: memories_fts',
      consequence: 'Search and injection return nothing until full-text search is back (packs say `index_unavailable`).',
      recovery: 'Use a Node.js build whose bundled SQLite has FTS5 (22.16 and 24.x do), then run `oboete doctor` again.',
    });
  });
});

test('an unreadable lease table produces a worker recovery sentence', async () => {
  await withItemDatabase((db) => {
    db.exec('DROP TABLE worker_lease');
    assert.deepEqual(workerItem(db, ITEM_NOW, false), {
      item: 'worker', status: 'degraded', reason: 'no such table: worker_lease',
      consequence: 'Queued events are not summarized until the lease is reclaimed.',
      recovery: '`oboete observe` (it reclaims a stale lease and releases it when the queue is empty)',
    });
  });
});

test('a fresh worker heartbeat reports the process and elapsed seconds', async () => {
  await withItemDatabase((db) => {
    db.prepare("UPDATE worker_lease SET owner_token = 'live-owner', pid = 1234, heartbeat_at = ? WHERE id = 1").run(ITEM_NOW - 2000);
    assert.deepEqual(workerItem(db, ITEM_NOW, false), {
      item: 'worker', status: 'healthy', reason: 'The worker process 1234 is alive (heartbeat 2 seconds ago).',
      consequence: '', recovery: '',
    });
  });
});

test('a spool backlog reports waiting events and removes its writable probe', async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(home);
    ensureDirectories(paths);
    writeFileSync(join(paths.spool, 'waiting.json'), '{}');
    assert.deepEqual(spoolItem(paths), {
      item: 'spool', status: 'degraded', reason: '1 events are waiting in the spool.',
      consequence: 'They are not summarized or searchable yet.', recovery: '`oboete observe`',
    });
    assert.deepEqual(readdirSync(paths.spool).sort(), ['failed', 'pi-ack', 'waiting.json']);
  });
});

test('quarantined spool files are a warning and directories are not counted', async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(home);
    ensureDirectories(paths);
    writeFileSync(join(paths.spoolFailed, 'rejected.json'), '{}');
    mkdirSync(join(paths.spoolFailed, 'directory'));
    assert.deepEqual(spoolItem(paths), {
      item: 'spool', status: 'warning', reason: `1 quarantined files are under ${paths.spoolFailed}.`,
      consequence: 'Those events were not recovered into storage.',
      recovery: `Inspect and delete the files under ${paths.spoolFailed}.`,
    });
  });
});

test('a missing spool directory reports potential event loss', async () => {
  await withTempHome((home) => {
    const paths = oboetePaths(home);
    assert.deepEqual(spoolItem(paths), {
      item: 'spool', status: 'degraded', reason: `The spool directory ${paths.spool} is not writable.`,
      consequence: 'When the database is also unavailable, events are lost (the hook reports the count on stderr).',
      recovery: `\`chmod u+rwx ${paths.spool}\``,
    });
  });
});

test('an unconfigured provider explains fallback without probing', async () => {
  await withItemDatabase(async (db, paths) => {
    assert.deepEqual(await providerItem({
      config: configSchema.parse({ observer: { preset: 'none' } }), paths, db,
      integrityFailed: false, deps: itemDeps, options: itemOptions, now: ITEM_NOW,
    }), {
      item: 'provider', status: 'degraded', reason: 'No observer provider is configured.',
      consequence: 'Summaries come from the rule-based fallback only (packs say `Degraded:`).',
      recovery: '`oboete setup --provider <preset>` (workers-ai is the free remote default; ollama stays local)',
    });
    assert.equal(db.prepare('SELECT count(*) AS n FROM provider_usage').get()?.n, 0);
  });
});

test('missing provider credentials name the variable to export', async () => {
  await withItemDatabase(async (db, paths) => {
    assert.deepEqual(await providerItem({
      config: configSchema.parse({ observer: { preset: 'openrouter' } }), paths, db,
      integrityFailed: false, deps: itemDeps, options: itemOptions, now: ITEM_NOW,
    }), {
      item: 'provider', status: 'degraded',
      reason: 'No credentials are set for the openrouter preset (env:OBOETE_OPENROUTER_API_KEY).',
      consequence: 'Summaries come from the rule-based fallback only (packs say `Degraded:`).',
      recovery: 'Export that variable in the shell that runs the agents.',
    });
  });
});

test('a provider probe without storage remains unverified', async () => {
  await withTempHome(async (home) => {
    assert.deepEqual(await providerItem({
      config: configSchema.parse({ observer: { preset: 'ollama', model: 'qwen3:8b' } }),
      paths: oboetePaths(home), db: null, integrityFailed: false,
      deps: itemDeps, options: itemOptions, now: ITEM_NOW,
    }), {
      item: 'provider', status: 'unverified', reason: 'The database is unavailable, so the provider could not be probed.',
      consequence: 'Summaries cannot be checked until storage is open.',
      recovery: '`oboete doctor --probe-provider` after storage is repaired.',
    });
  });
});

test('changed provider consent stops a doctor probe before reserving allowance', async () => {
  await withItemDatabase(async (db, paths) => {
    assert.deepEqual(await providerItem({
      config: configSchema.parse({}), paths, db, integrityFailed: false,
      deps: { ...itemDeps, env: { OBOETE_CF_ACCOUNT_ID: 'account', OBOETE_CF_API_TOKEN: 'test-token' } },
      options: itemOptions, now: ITEM_NOW,
    }), {
      item: 'provider', status: 'degraded', reason: 'Observer consent changed before reservation.',
      consequence: 'Summaries fall back to rule-based until the provider answers.',
      recovery: '`oboete setup --accept-egress`',
    });
    assert.equal(db.prepare('SELECT count(*) AS n FROM provider_usage').get()?.n, 0);
  });
});

for (const [name, calls, exhaustedAt, reason] of [
  ['provider exhaustion', 1, ITEM_NOW, 'provider_exhausted: The provider reported exhaustion today.'],
  ['the daily cap', 150, null, 'daily_cap: The daily cap of 150 calls is used up.'],
] as const) {
  test(`${name} stops a doctor probe without consuming another call`, async () => {
    await withItemDatabase(async (db, paths) => {
      db.prepare('INSERT INTO provider_usage (utc_day, preset, calls, exhausted_at, reset_at) VALUES (?, ?, ?, ?, ?)')
        .run('2026-09-06', 'workers-ai', calls, exhaustedAt, ITEM_RESET);
      const config = configSchema.parse({});
      assert.deepEqual(await providerItem({
        config, paths, db, integrityFailed: false,
        deps: { ...itemDeps, env: { OBOETE_CF_ACCOUNT_ID: 'account', OBOETE_CF_API_TOKEN: 'test-token' } },
        options: itemOptions, now: ITEM_NOW,
      }), {
        item: 'provider', status: 'degraded', reason,
        consequence: 'Summaries fall back to rule-based until the provider answers.',
        recovery: 'Wait for the reset at 2026-09-07T00:00:00.000Z or choose another preset with `oboete setup --provider`.',
      });
      assert.deepEqual(allowanceItem(config, db, false, ITEM_NOW), {
        item: 'allowance', status: 'degraded',
        reason: exhaustedAt === null ? 'The daily cap of 150 calls is used up.' : 'The provider reported exhaustion today.',
        consequence: 'Summaries come from the fallback until the allowance resets; no call is retried.',
        recovery: 'Wait for the reset at 2026-09-07T00:00:00.000Z or switch preset with `oboete setup --provider`.',
      });
      assert.deepEqual(
        { ...db.prepare('SELECT calls, exhausted_at FROM provider_usage').get() },
        { calls, exhausted_at: exhaustedAt },
      );
    });
  });
}

test('a rejected provider credential consumes one probe and recommends checking credentials', async () => {
  await withItemDatabase(async (db, paths) => {
    const env = { OBOETE_OPENROUTER_API_KEY: 'test-key' };
    const draft = configSchema.parse({ observer: { preset: 'openrouter' } });
    const config = configSchema.parse({ ...draft, consent: { hash: consentHash(consentTuple(draft, env)), accepted_at: ITEM_NOW } });
    let requests = 0;
    const item = await providerItem({
      config, paths, db, integrityFailed: false, options: itemOptions, now: ITEM_NOW,
      deps: { ...itemDeps, env, fetch: async () => {
        requests += 1;
        return new Response(JSON.stringify({ error: { message: 'invalid credential' } }), {
          status: 401, headers: { 'content-type': 'application/json' },
        });
      } },
    });
    assert.deepEqual(item, {
      item: 'provider', status: 'degraded', reason: 'Provider request failed with HTTP 401.',
      consequence: 'Summaries fall back to rule-based until the provider answers.',
      recovery: 'Check the credentials for this preset and run `oboete doctor --probe-provider` again.',
    });
    assert.equal(requests, 1);
    assert.deepEqual(
      { ...db.prepare('SELECT utc_day, preset, calls, exhausted_at FROM provider_usage').get() },
      { utc_day: '2026-09-06', preset: 'openrouter', calls: 1, exhausted_at: null },
    );
  });
});

for (const [name, fetchedAt] of [['an expired', ITEM_NOW - 86_400_000], ['a future-dated', ITEM_NOW + 1]] as const) {
  test(`${name} catalog cannot verify the configured model`, async () => {
    await withItemDatabase((db) => {
      runtimeStateSet(db, 'workers_ai_catalog', JSON.stringify({
        accountId: 'account', models: ['chosen-model'], defaultModelPresent: false,
        hasPaidOnlyModels: false, fetchedAt,
      }), ITEM_NOW);
      assert.deepEqual(catalogItems(configSchema.parse({ observer: { model: 'chosen-model' } }), db, false,
        { OBOETE_CF_ACCOUNT_ID: 'account', OBOETE_CF_API_TOKEN: 'test-token' }, ITEM_NOW), [{
        item: 'catalog', status: 'unverified', reason: 'The cached catalog is stale; the worker refreshes it on the next batch.',
        consequence: 'The configured model has not been checked against the provider list this run.',
        recovery: '`oboete observe` fetches the catalog on the first batch.',
      }]);
    });
  });
}

for (const [name, models, paid, expected] of [
  ['a missing model', ['another-model'], false, {
    status: 'degraded', reason: 'The configured model is not in the catalog of 1 models fetched 2026-09-06T12:00:00.000Z.',
    consequence: 'Summaries fall back to rule-based until `[observer] model` names a listed model.', recovery: 'Set `[observer] model` to a listed model.',
  }],
  ['paid models', ['chosen-model'], true, {
    status: 'warning', reason: 'The catalog lists models that need a paid Workers plan; the configured model chosen-model is only used if it is free.',
    consequence: 'A paid-only model will fail with provider_paid and fall back to rule-based summaries.', recovery: 'Keep `[observer] model` on a free model.',
  }],
  ['a listed model', ['chosen-model'], false, {
    status: 'healthy', reason: 'The catalog of 1 models fetched 2026-09-06T12:00:00.000Z includes the configured model.', consequence: '', recovery: '',
  }],
] as const) {
  test(`a fresh catalog reports ${name}`, async () => {
    await withItemDatabase((db) => {
      runtimeStateSet(db, 'workers_ai_catalog', JSON.stringify({
        accountId: 'account', models, defaultModelPresent: false, hasPaidOnlyModels: paid, fetchedAt: ITEM_NOW,
      }), ITEM_NOW);
      assert.deepEqual(catalogItems(configSchema.parse({ observer: { model: 'chosen-model' } }), db, false,
        { OBOETE_CF_ACCOUNT_ID: 'account', OBOETE_CF_API_TOKEN: 'test-token' }, ITEM_NOW), [{ item: 'catalog', ...expected }]);
    });
  });
}
