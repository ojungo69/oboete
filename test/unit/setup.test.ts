import assert from 'node:assert/strict';
import type { spawn } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { chmodSync, existsSync, mkdirSync, readFileSync, symlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

import { consentMatches, loadConfig } from '../../src/config.js';
import { openDatabase } from '../../src/db/open.js';
import { oboetePaths } from '../../src/paths.js';
import { CONSENT_STATE_KEY } from '../../src/setup/consent.js';
import type { VersionSpawn } from '../../src/setup/detect.js';
import { runSetup, SETUP_RESULT_KEY, type SetupDeps } from '../../src/setup/setup.js';
import { withTempHome } from '../helpers/home.js';

const NODE = '/usr/bin/node';
const BUNDLE = '/opt/oboete/dist/oboete.mjs';

const noVersion: VersionSpawn = (command) => {
  throw new Error(`no version probe expected: ${command}`);
};

const noSpawn = (() => {
  throw new Error('no agent process expected');
}) as unknown as typeof spawn;

/** The four agent homes, so detection reports every agent installed without a CLI on PATH. */
function agentHomes(userHome: string): void {
  for (const directory of ['.claude', '.codex', '.grok', join('.pi', 'agent')]) {
    mkdirSync(join(userHome, directory), { recursive: true });
  }
}

type Harness = {
  home: string;
  userHome: string;
  paths: ReturnType<typeof oboetePaths>;
  calls: string[][];
  output: string;
  run(argv: string[], overrides?: Partial<SetupDeps>): Promise<number>;
};

async function harness(fn: (harness: Harness) => Promise<void>): Promise<void> {
  await withTempHome(async (home) => {
    const userHome = join(home, 'user');
    agentHomes(userHome);
    const context: Harness = {
      home,
      userHome,
      paths: oboetePaths(home),
      calls: [],
      output: '',
      async run(argv, overrides = {}) {
        return await runSetup(argv, {
          env: { HOME: userHome, PATH: join(home, 'empty-bin'), OBOETE_HOME: home },
          versionSpawn: noVersion,
          spawn: noSpawn,
          runCli: (command) => {
            context.calls.push([...command]);
            return { ok: true, reason: '' };
          },
          write: (text) => {
            context.output += text;
          },
          node: NODE,
          bundle: BUNDLE,
          ...overrides,
        });
      },
    };
    await fn(context);
  });
}

function runtimeState(dbPath: string, key: string): string | undefined {
  const { db } = openDatabase({ path: dbPath, timeoutMs: 2_000 });
  try {
    const row = db.prepare('SELECT value_json FROM runtime_state WHERE key = ?').get(key);
    return typeof row?.value_json === 'string' ? row.value_json : undefined;
  } finally {
    db.close();
  }
}

test('setup wires the four agents, reports them and prints the viewer line', async () => {
  await harness(async (context) => {
    const code = await context.run(['--provider', 'ollama']);

    assert.equal(code, 0, context.output);
    const settings = readFileSync(join(context.userHome, '.claude', 'settings.json'), 'utf8');
    assert.match(settings, /"oboete": true/);
    assert.match(settings, /hook --agent claude-or-grok --event SessionStart/);
    const codexConfig = readFileSync(join(context.userHome, '.codex', 'config.toml'), 'utf8');
    assert.match(codexConfig, /trusted_hash = "sha256:/);
    assert.match(
      readFileSync(join(context.userHome, '.codex', 'hooks.json'), 'utf8'),
      /hook --agent codex --event SessionStart/,
    );
    assert.ok(existsSync(join(context.userHome, '.grok', 'hooks', 'oboete.json')));
    assert.ok(existsSync(join(context.userHome, '.pi', 'agent', 'extensions', 'oboete.js')));

    for (const agent of ['claude', 'codex', 'grok', 'pi']) assert.match(context.output, new RegExp(agent));
    assert.match(context.output, /oboete view --open/);
    // No `claude` on PATH: setup never hands a bare name to the spawn for PATH to resolve.
    assert.deepEqual(context.calls, []);
    assert.match(context.output, /claude: the memory tools could not be registered because the claude/);
    assert.ok(runtimeState(context.paths.db, SETUP_RESULT_KEY) !== undefined, 'doctor can read the result');
  });
});

test('a remote preset without consent writes nothing and exits 2', async () => {
  await harness(async (context) => {
    const code = await context.run([]);

    assert.equal(code, 2);
    assert.match(context.output, /api\.cloudflare\.com/);
    assert.match(context.output, /--accept-egress/);
    assert.match(context.output, /--yes/);
    assert.equal(existsSync(join(context.userHome, '.claude', 'settings.json')), false);
    assert.deepEqual(context.calls, []);
  });
});

test('--accept-egress stores the record, prints the credential steps and continues', async () => {
  await harness(async (context) => {
    const code = await context.run(['--accept-egress']);

    assert.equal(code, 0, context.output);
    assert.match(context.output, /https:\/\/dash\.cloudflare\.com\/sign-up/);
    assert.match(context.output, /--provider ollama/);
    assert.match(context.output, /--provider agent-cli/);
    const stored = loadConfig(context.paths);
    assert.equal(typeof stored.consent.hash, 'string');
    const record = runtimeState(context.paths.db, CONSENT_STATE_KEY);
    assert.equal(JSON.parse(record ?? '{}').hash, stored.consent.hash);
    assert.ok(existsSync(join(context.userHome, '.claude', 'settings.json')));
  });
});

test('--yes accepts the stored tuple and is refused once the destination changes', async () => {
  await harness(async (context) => {
    assert.equal(await context.run(['--accept-egress']), 0);
    assert.equal(await context.run(['--yes']), 0, context.output);
    assert.equal(await context.run(['--provider', 'gemini', '--yes']), 2);
    assert.match(context.output, /generativelanguage\.googleapis\.com/);
  });
});

test('switching to a local preset re-records consent instead of leaving the remote hash behind', async () => {
  await harness(async (context) => {
    assert.equal(await context.run(['--accept-egress']), 0, context.output);
    assert.equal(await context.run(['--provider', 'ollama']), 0, context.output);

    const stored = loadConfig(context.paths);
    assert.equal(stored.observer.preset, 'ollama');
    // The observer recomputes this before every reservation and every send (R8): a record left
    // over from the remote preset would degrade every batch with `consent_changed` for good.
    assert.equal(consentMatches(stored, { HOME: context.userHome }), true);
    assert.equal(JSON.parse(runtimeState(context.paths.db, CONSENT_STATE_KEY) ?? '{}').hash, stored.consent.hash);
  });
});

test('a destination the run refuses is not written to the configuration', async () => {
  await harness(async (context) => {
    assert.equal(await context.run(['--accept-egress']), 0, context.output);
    const before = loadConfig(context.paths);

    assert.equal(await context.run(['--provider', 'gemini', '--yes']), 2);

    const after = loadConfig(context.paths);
    assert.equal(after.observer.preset, before.observer.preset, 'the refused destination is not enabled');
    assert.equal(after.consent.hash, before.consent.hash);
    assert.match(context.output, /generativelanguage\.googleapis\.com/, 'it still shows what it refused');
  });
});

test('a machine with no supported agent says that nothing was wired', async () => {
  await withTempHome(async (home) => {
    const userHome = join(home, 'user');
    mkdirSync(userHome, { recursive: true });
    let output = '';
    const code = await runSetup(['--provider', 'none'], {
      env: { HOME: userHome, PATH: join(home, 'empty-bin'), OBOETE_HOME: home },
      versionSpawn: noVersion,
      spawn: noSpawn,
      runCli: () => ({ ok: true, reason: '' }),
      write: (text) => {
        output += text;
      },
      node: NODE,
      bundle: BUNDLE,
    });

    assert.equal(code, 0, output);
    assert.match(output, /no supported agent/i);
    assert.match(output, /oboete view --open/);
  });
});

test('a probe that fails exits 1 and names the agent', async () => {
  await harness(async (context) => {
    const bin = join(context.home, 'bin');
    mkdirSync(bin, { recursive: true });
    writeFileSync(join(bin, 'claude'), '#!/bin/sh\nexit 0\n');
    chmodSync(join(bin, 'claude'), 0o755);

    const code = await context.run(['--provider', 'ollama', '--agents', 'claude', '--json'], {
      env: { HOME: context.userHome, PATH: bin, OBOETE_HOME: context.home },
      versionSpawn: (() => ({ status: 0, stdout: '2.1.258\n', stderr: '' })) as unknown as VersionSpawn,
      spawn: ((): EventEmitter => {
        const child = new EventEmitter();
        queueMicrotask(() => child.emit('close', 1, null));
        return child;
      }) as unknown as typeof spawn,
    });

    assert.equal(code, 1);
    const report = JSON.parse(context.output) as { agents: { agent: string; probe: string }[] };
    assert.deepEqual(report.agents.map((agent) => [agent.agent, agent.probe]), [['claude', 'fail']]);
  });
});

test('--remove restores the foreign files and keeps the consent record', async () => {
  await harness(async (context) => {
    const settingsPath = join(context.userHome, '.claude', 'settings.json');
    const original = `${JSON.stringify({ hooks: { Stop: [{ hooks: [{ type: 'command', command: 'mine' }] }] } }, null, 2)}\n`;
    writeFileSync(settingsPath, original);

    assert.equal(await context.run(['--accept-egress']), 0);
    context.calls.length = 0;
    const code = await context.run(['--remove']);

    assert.equal(code, 0, context.output);
    assert.equal(readFileSync(settingsPath, 'utf8'), original);
    assert.equal(existsSync(join(context.userHome, '.pi', 'agent', 'extensions', 'oboete.js')), false);
    assert.equal(existsSync(join(context.userHome, '.grok', 'hooks', 'oboete.json')), false);
    assert.equal(existsSync(join(context.userHome, '.codex', 'hooks.json')), false);
    assert.equal(typeof loadConfig(context.paths).consent.hash, 'string', 'consent is kept');
    assert.deepEqual(context.calls, []);
    assert.match(context.output, /claude: the memory tools could not be unregistered because the claude/);
  });
});

test('an agent named but not installed is reported without failing the run', async () => {
  await withTempHome(async (home) => {
    const userHome = join(home, 'user');
    mkdirSync(userHome, { recursive: true });
    let output = '';
    const code = await runSetup(['--provider', 'none', '--agents', 'pi', '--json'], {
      env: { HOME: userHome, PATH: join(home, 'empty-bin'), OBOETE_HOME: home },
      versionSpawn: noVersion,
      spawn: noSpawn,
      runCli: () => ({ ok: true, reason: '' }),
      write: (text) => {
        output += text;
      },
      node: NODE,
      bundle: BUNDLE,
    });

    assert.equal(code, 0);
    assert.deepEqual(JSON.parse(output).agents, [
      { agent: 'pi', wired: 'not installed', probe: 'not_installed', trust: 'absent', native_memory: null },
    ]);
  });
});

test('the JSON report carries wired, probe, trust and native memory per agent', async () => {
  await harness(async (context) => {
    mkdirSync(join(context.userHome, '.codex', 'memories'), { recursive: true });
    const code = await context.run(['--provider', 'ollama', '--json']);

    assert.equal(code, 0, context.output);
    const report = JSON.parse(context.output) as {
      agents: { agent: string; wired: string; probe: string; trust: string; native_memory: string | null }[];
      view: string;
    };
    assert.deepEqual(
      report.agents.map((agent) => agent.agent),
      ['claude', 'codex', 'grok', 'pi'],
    );
    assert.equal(report.agents[1]?.native_memory, 'codex_memories');
    assert.equal(report.agents[1]?.trust, 'trusted');
    assert.equal(report.agents[0]?.wired, 'yes');
    assert.equal(report.agents[0]?.probe, 'not_installed');
    assert.match(report.view, /oboete view --open/);
  });
});

test('an unknown flag or agent name exits 2 without writing', async () => {
  await harness(async (context) => {
    assert.equal(await context.run(['--nope']), 2);
    assert.equal(await context.run(['--agents', 'emacs']), 2);
    assert.equal(existsSync(join(context.userHome, '.claude', 'settings.json')), false);
  });
});

test('the agent CLI is spawned by the absolute path detection resolved, never by the bare name', async () => {
  await harness(async (context) => {
    const bin = join(context.home, 'bin');
    mkdirSync(bin, { recursive: true });
    writeFileSync(join(bin, 'claude'), '#!/bin/sh\nexit 0\n');
    chmodSync(join(bin, 'claude'), 0o755);
    const overrides = {
      env: { HOME: context.userHome, PATH: bin, OBOETE_HOME: context.home },
      versionSpawn: (() => ({ status: 0, stdout: '2.1.258\n', stderr: '' })) as unknown as VersionSpawn,
      spawn: ((): EventEmitter => {
        const child = new EventEmitter();
        queueMicrotask(() => child.emit('close', 1, null));
        return child;
      }) as unknown as typeof spawn,
    };

    await context.run(['--provider', 'ollama', '--agents', 'claude'], overrides);
    await context.run(['--remove', '--agents', 'claude'], overrides);

    // `spawn` resolves a bare name through PATH itself, and a PATH holding '.' or an empty entry
    // would run a file called `claude` out of the developer's repository -- the reason
    // src/setup/detect.ts refuses a relative PATH entry when it resolves the CLI in the first place.
    // Registration removes before it adds, because `claude mcp add` refuses a name it already
    // holds; both removals name the scope the registration writes, or claude refuses that too once
    // an older local-scope entry is there as well.
    assert.deepEqual(context.calls, [
      [join(bin, 'claude'), 'mcp', 'remove', 'oboete', '--scope', 'user'],
      [join(bin, 'claude'), 'mcp', 'add', 'oboete', '--scope', 'user', '--', NODE, BUNDLE, 'mcp'],
      [join(bin, 'claude'), 'mcp', 'remove', 'oboete', '--scope', 'user'],
    ]);
  });
});

test('a symbolic link left at the configuration’s temporary path is refused instead of written through', async () => {
  await harness(async (context) => {
    const victim = join(context.home, 'victim.toml');
    writeFileSync(victim, 'stays = true\n');
    symlinkSync(victim, `${context.paths.config}.oboete-tmp-${process.pid}`);

    await assert.rejects(context.run(['--provider', 'ollama']));
    assert.equal(readFileSync(victim, 'utf8'), 'stays = true\n', 'the link is never followed');
  });
});
