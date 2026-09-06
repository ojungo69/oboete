import assert from 'node:assert/strict';
import type { spawn } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { test } from 'node:test';

import { openDatabase } from '../../src/db/open.js';
import { oboetePaths } from '../../src/paths.js';
import { probeEventStored, runProbes, type ProbeTarget } from '../../src/setup/probe.js';
import { withTempHome } from '../helpers/home.js';

type SpawnCall = {
  command: string;
  args: readonly string[];
  options: {
    cwd?: string;
    env?: NodeJS.ProcessEnv;
    signal?: AbortSignal;
    stdio?: unknown;
    detached?: boolean;
  };
};

const BYPASS_FLAG = /dangerously|bypass|always-approve|skip-permissions/;

function probeTargets(userHome: string): ProbeTarget[] {
  const bin = join(userHome, 'bin');
  return [
    {
      agent: 'claude' as const,
      cliPath: join(bin, 'claude'),
      configPath: join(userHome, '.claude', 'settings.json'),
    },
    {
      agent: 'codex' as const,
      cliPath: join(bin, 'codex'),
      configPath: join(userHome, '.codex', 'hooks.json'),
    },
    {
      agent: 'grok' as const,
      cliPath: join(bin, 'grok'),
      configPath: join(userHome, '.grok', 'hooks', 'oboete.json'),
    },
    {
      agent: 'pi' as const,
      cliPath: join(bin, 'pi'),
      configPath: join(userHome, '.pi', 'agent', 'extensions', 'oboete.js'),
    },
  ];
}

function closingChild(code: number | null, error?: Error): EventEmitter {
  const child = new EventEmitter();
  queueMicrotask(() => {
    if (error !== undefined) child.emit('error', error);
    else child.emit('close', code, null);
  });
  return child;
}

test('runProbes launches all installed agents in parallel with isolated native homes', async () => {
  await withTempHome(async (home) => {
    const userHome = join(home, 'user');
    const bin = join(userHome, 'bin');
    const calls: SpawnCall[] = [];
    let active = 0;
    let maxActive = 0;
    const spawnFake = ((command: string, args: readonly string[], options: SpawnCall['options']) => {
      calls.push({ command, args, options });
      active += 1;
      maxActive = Math.max(maxActive, active);
      const child = new EventEmitter();
      setImmediate(() => {
        active -= 1;
        child.emit('close', 0, null);
      });
      return child;
    }) as unknown as typeof spawn;
    const markers = new Map<string, string>();

    const results = await runProbes(probeTargets(userHome), {
      spawn: spawnFake,
      lookupProbe: (agent, marker) => {
        markers.set(agent, marker);
        return true;
      },
      env: { HOME: userHome, PATH: bin },
      now: () => 100,
    });

    assert.equal(maxActive, 4);
    assert.equal(new Set(markers.values()).size, 4, 'every agent receives a unique marker');
    assert.deepEqual(
      results,
      ['claude', 'codex', 'grok', 'pi'].map((agent) => ({
        agent,
        status: 'pass',
        elapsedMs: 0,
        reason: 'probe_event_stored',
      })),
    );

    assert.deepEqual(
      calls.map((call) => call.command),
      ['claude', 'codex', 'grok', 'pi'].map((agent) => join(bin, agent)),
      'the CLI is spawned by the absolute path detection resolved',
    );
    const workingDirectories = calls.map((call) => call.options.cwd ?? '');
    for (const [index, agent] of ['claude', 'codex', 'grok', 'pi'].entries()) {
      const args = calls[index]?.args ?? [];
      const prompt = agent === 'codex' ? args.at(-1) : args[1];
      assert.equal(typeof prompt, 'string');
      assert.ok(prompt?.includes(markers.get(agent) ?? ''), agent);
      assert.equal(calls[index]?.options.stdio, 'ignore');
      for (const argument of args) assert.ok(!BYPASS_FLAG.test(argument), `${agent}: ${argument}`);
      const workingDirectory = workingDirectories[index] ?? '';
      assert.ok(workingDirectory.startsWith(tmpdir()), `${agent} runs in a temporary directory`);
      assert.equal(existsSync(workingDirectory), false, `${agent}: the probe directory is removed`);
    }
    assert.equal(new Set(workingDirectories).size, 4, 'every probe gets its own directory');

    assert.deepEqual(calls[0]?.args.slice(0, 1), ['-p']);
    assert.deepEqual(calls[0]?.args.slice(2), [
      '--settings',
      join(userHome, '.claude', 'settings.json'),
      '--output-format',
      'json',
    ]);
    assert.deepEqual(calls[1]?.args.slice(0, 6), [
      'exec',
      '--sandbox',
      'read-only',
      '--skip-git-repo-check',
      '--json',
      '-C',
    ]);
    assert.equal(calls[1]?.args[6], workingDirectories[1]);
    assert.deepEqual(calls[2]?.args.slice(0, 1), ['-p']);
    assert.deepEqual(calls[2]?.args.slice(2), [
      '--output-format',
      'json',
      '--cwd',
      workingDirectories[2] ?? '',
    ]);
    assert.deepEqual(calls[3]?.args.slice(2), [
      '--mode',
      'json',
      '--session-dir',
      workingDirectories[3] ?? '',
    ]);

    assert.equal(new Set(calls.map((call) => call.options.signal)).size, 1);
    assert.equal(calls[1]?.options.env?.CODEX_HOME, join(userHome, '.codex'));
    assert.equal(calls[2]?.options.env?.GROK_HOME, join(userHome, '.grok'));
    assert.equal(calls[2]?.options.env?.GROK_CLAUDE_HOOKS_ENABLED, '0');
    assert.equal(calls[2]?.options.env?.GROK_CLAUDE_MCPS_ENABLED, '0');
    assert.equal(calls[2]?.options.env?.GROK_CURSOR_HOOKS_ENABLED, '0');
    assert.equal(calls[2]?.options.env?.GROK_CURSOR_MCPS_ENABLED, '0');
    assert.equal(calls[3]?.options.env?.PI_CODING_AGENT_DIR, join(userHome, '.pi', 'agent'));
  });
});

test('no oboete credential reaches the agent CLI the probe spawns', async () => {
  await withTempHome(async (home) => {
    const userHome = join(home, 'user');
    const environments: NodeJS.ProcessEnv[] = [];
    const spawnFake = ((_command: string, _args: readonly string[], options: SpawnCall['options']) => {
      environments.push(options.env ?? {});
      return closingChild(0);
    }) as unknown as typeof spawn;

    const credentials = {
      OBOETE_NIM_API_KEY: 'nim-secret-value',
      OBOETE_OPENROUTER_API_KEY: 'openrouter-secret-value',
      OBOETE_CF_API_TOKEN: 'cloudflare-secret-value',
      OBOETE_CF_ACCOUNT_ID: 'cloudflare-account-value',
    };
    await runProbes(probeTargets(userHome), {
      spawn: spawnFake,
      lookupProbe: () => true,
      env: {
        ...credentials,
        HOME: userHome,
        PATH: join(userHome, 'bin'),
        OBOETE_HOME: home,
        ANTHROPIC_API_KEY: 'the agent CLI keeps its own credentials',
      },
      now: () => 0,
    });

    assert.equal(environments.length, 4);
    for (const env of environments) {
      for (const [name, value] of Object.entries(credentials)) {
        assert.equal(env[name], undefined, name);
        assert.ok(!Object.values(env).includes(value), `${name} value`);
      }
      assert.equal(env.OBOETE_HOME, home, 'the probe event must land in the same storage');
      assert.equal(env.HOME, userHome);
      assert.equal(env.ANTHROPIC_API_KEY, 'the agent CLI keeps its own credentials');
    }
  });
});

test('runProbes reports a missing CLI, a failed spawn and a non-zero exit', async () => {
  await withTempHome(async (home) => {
    const userHome = join(home, 'user');
    const targets = probeTargets(userHome);
    // A leftover agent home with no CLI on PATH: there is nothing to verify (contracts/cli.md).
    targets[0] = { ...targets[0]!, cliPath: null };
    const spawned: string[] = [];
    const spawnFake = ((command: string) => {
      spawned.push(command);
      if (command.endsWith('pi')) return closingChild(null, new Error('spawn pi ENOENT'));
      return closingChild(command.endsWith('grok') ? 2 : 0);
    }) as unknown as typeof spawn;

    const results = await runProbes(targets, {
      spawn: spawnFake,
      lookupProbe: (agent) => agent === 'codex',
      env: { HOME: userHome, PATH: join(userHome, 'bin') },
      now: () => 10,
    });

    assert.deepEqual(
      spawned,
      ['codex', 'grok', 'pi'].map((agent) => join(userHome, 'bin', agent)),
    );
    assert.deepEqual(results, [
      { agent: 'claude', status: 'not_installed', elapsedMs: 0, reason: 'agent_not_installed' },
      { agent: 'codex', status: 'pass', elapsedMs: 0, reason: 'probe_event_stored' },
      { agent: 'grok', status: 'fail', elapsedMs: 0, reason: 'agent_exit_2' },
      { agent: 'pi', status: 'fail', elapsedMs: 0, reason: 'spawn_failed' },
    ]);
  });
});

test('a marker that reaches storage after the agent exits still passes', async () => {
  await withTempHome(async (home) => {
    const userHome = join(home, 'user');
    let lookups = 0;
    const results = await runProbes([probeTargets(userHome)[1]!], {
      spawn: (() => closingChild(0)) as unknown as typeof spawn,
      lookupProbe: () => {
        lookups += 1;
        // Codex runs its async handler after the CLI exits; Pi captures in a detached child.
        return lookups >= 3;
      },
      env: { HOME: userHome, PATH: join(userHome, 'bin') },
      now: () => 0,
    });

    assert.equal(lookups, 3, 'the marker is looked up again until it appears');
    assert.deepEqual(results, [
      { agent: 'codex', status: 'pass', elapsedMs: 0, reason: 'probe_event_stored' },
    ]);
  });
});

test('a clean run whose marker never arrives stops looking at the shared deadline', async (context) => {
  await withTempHome(async (home) => {
    const userHome = join(home, 'user');
    const deadline = new AbortController();
    const timeoutCalls: number[] = [];
    context.mock.method(AbortSignal, 'timeout', (milliseconds: number) => {
      timeoutCalls.push(milliseconds);
      setTimeout(() => deadline.abort(new Error('deadline')), 250);
      return deadline.signal;
    });
    let lookups = 0;

    const results = await runProbes([probeTargets(userHome)[1]!], {
      spawn: (() => closingChild(0)) as unknown as typeof spawn,
      lookupProbe: () => {
        lookups += 1;
        return false;
      },
      env: { HOME: userHome, PATH: join(userHome, 'bin') },
      now: () => 0,
    });

    assert.deepEqual(timeoutCalls, [90_000]);
    assert.ok(lookups >= 2, `polled ${lookups} times`);
    assert.deepEqual(results, [
      { agent: 'codex', status: 'fail', elapsedMs: 0, reason: 'probe_event_missing' },
    ]);
  });
});

test('the shared deadline kills the whole agent process group', async (context) => {
  await withTempHome(async (home) => {
    const userHome = join(home, 'user');
    const deadline = new AbortController();
    const timeoutCalls: number[] = [];
    context.mock.method(AbortSignal, 'timeout', (milliseconds: number) => {
      timeoutCalls.push(milliseconds);
      queueMicrotask(() => deadline.abort(new Error('deadline')));
      return deadline.signal;
    });
    const killed: [number, unknown][] = [];
    context.mock.method(process, 'kill', (pid: number, signalName?: string | number) => {
      killed.push([pid, signalName]);
      // An agent CLI that already exited leaves no group: the single process is the fallback.
      throw Object.assign(new Error('kill ESRCH'), { code: 'ESRCH' });
    });
    let childKills = 0;
    const calls: SpawnCall[] = [];
    const spawnFake = ((command: string, args: readonly string[], options: SpawnCall['options']) => {
      calls.push({ command, args, options });
      return Object.assign(new EventEmitter(), {
        pid: 4242,
        kill: () => {
          childKills += 1;
          return true;
        },
      });
    }) as unknown as typeof spawn;

    const results = await runProbes([probeTargets(userHome)[1]!], {
      spawn: spawnFake,
      lookupProbe: () => false,
      env: { HOME: userHome, PATH: join(userHome, 'bin') },
      now: () => 10,
    });

    assert.deepEqual(timeoutCalls, [90_000]);
    assert.equal(calls[0]?.options.detached, true, 'the agent CLI leads its own process group');
    assert.deepEqual(killed, [[-4242, 'SIGTERM']]);
    assert.equal(childKills, 1, 'the single process is the fallback');
    assert.deepEqual(results, [
      { agent: 'codex', status: 'timeout', elapsedMs: 0, reason: 'deadline_exceeded' },
    ]);
  });
});

async function withDatabase(fn: (db: DatabaseSync) => void | Promise<void>): Promise<void> {
  await withTempHome(async (home) => {
    const { db } = openDatabase({ path: oboetePaths(home).db, timeoutMs: 1_000 });
    try {
      db.prepare(
        `INSERT INTO repos (id, identity_kind, normalized_identity, created_at, last_seen_at)
         VALUES ('repo', 'common_dir', '/tmp/repo', 1, 1)`,
      ).run();
      for (const agent of ['claude', 'codex']) {
        db.prepare(
          `INSERT INTO sessions (id, repo_id, agent, native_session_id, conversation_id, status)
           VALUES (?, 'repo', ?, ?, ?, 'active')`,
        ).run(`session-${agent}`, agent, `native-${agent}`, `session-${agent}`);
      }
      await fn(db);
    } finally {
      db.close();
    }
  });
}

test('probeEventStored accepts the marker only in a captured prompt of that agent', async () => {
  await withDatabase((db) => {
    db.prepare(
      `INSERT INTO raw_events
         (id, repo_id, session_id, agent, kind, content, payload_json, sensitivity, classification_state)
       VALUES
         ('prompt', 'repo', 'session-claude', 'claude', 'prompt', 'check marker-prompt now', '{}', 'local_only', 'done'),
         ('other', 'repo', 'session-codex', 'codex', 'tool_result', 'marker-wrong-kind', '{}', 'local_only', 'done')`,
    ).run();

    assert.equal(probeEventStored(db, 'claude', 'marker-prompt'), true);
    assert.equal(probeEventStored(db, 'codex', 'marker-wrong-kind'), false);
    assert.equal(probeEventStored(db, 'grok', 'marker-prompt'), false);
  });
});
