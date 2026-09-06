import assert from 'node:assert/strict';
import type { spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { EventEmitter } from 'node:events';
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  unlinkSync,
  utimesSync,
  writeFileSync,
} from 'node:fs';
import { basename, join } from 'node:path';
import { test } from 'node:test';

import { PRESET_CATALOG } from '../../src/config.js';
import { openDatabase } from '../../src/db/open.js';
import { runDoctor, type DoctorDeps, type DoctorItem } from '../../src/doctor.js';
import { oboetePaths } from '../../src/paths.js';
import type { VersionSpawn } from '../../src/setup/detect.js';
import { removeJsonHandlers } from '../../src/setup/managed-block.js';
import { runSetup, type SetupDeps } from '../../src/setup/setup.js';
import { utcDay } from '../../src/observer/reservation.js';
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
    assertBroken(failed, 'degraded', 'unreachable', 'rule-based', 'network|host');
    assert.match(failed.reason, /^unreachable/);

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

test('a Pi spawn ENOENT degrades agent:pi with pi_spawn_failed', async () => {
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
    assertBroken(entry, 'degraded', 'pi_spawn_failed', 'Pi', 'setup');
    assert.match(entry.reason, /pi_spawn_failed/);

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
