import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

import { parse as parseToml } from 'smol-toml';

import { trustedHash, trustIdentity, trustKey } from '../../src/setup/codex-trust.js';
import { ManagedFileError, BACKUP_SUFFIX } from '../../src/setup/managed-block.js';
import { removeCodex, writeCodex } from '../../src/setup/write-codex.js';
import { withTempHome } from '../helpers/home.js';

const NODE = '/usr/bin/node';
const BUNDLE = '/opt/oboete/dist/oboete.mjs';

function command(event: string): string {
  return `'${NODE}' '${BUNDLE}' hook --agent codex --event ${event}`;
}

/**
 * The hashes of `scripts/e2e/probe-lib/trusthash.mjs`, the Phase 1 implementation that was checked
 * against five of the developer's real `trusted_hash` rows and made twelve hooks fire without
 * `--dangerously-bypass-hook-trust` (docs/research/oboete-contracts-probes.md). They are frozen
 * here, not recomputed, so a change of the rule fails this test instead of moving with it.
 */
const PROBE_HASHES = {
  sessionStart: 'sha256:cadb93d1d0a7e0a7211f98f2c0245ee8a3e2775640003e50c53150b2b7b765e2',
  userPromptSubmit: 'sha256:a379aaa1b1890276eef01e24b0b96a47d0a2c85b27e553275275df8377fcab3e',
  sessionEnd: 'sha256:314f08228c71e550b437af54990dc336d289b0151481df03e2976a122e3a23a4',
};

/** The identity of the handlers oboete really writes: `timeout` and `additionalContextLimit` set. */
const OBOETE_IDENTITY = {
  sessionStart: `{"event_name":"session_start","hooks":[{"additionalContextLimit":0,"async":false,"command":"${command('SessionStart')}","timeout":12,"type":"command"}],"matcher":"startup|clear|compact"}`,
  sessionEnd: `{"event_name":"session_end","hooks":[{"async":false,"command":"${command('SessionEnd')}","timeout":3,"type":"command"}]}`,
};
const OBOETE_HASHES = {
  sessionStart: 'sha256:a7f79019961eca8f04eaeb8c98561963c0fd00e64c2ffa5110e4bbb1126b93d7',
  sessionEnd: 'sha256:55b10cb55f653418545badd77649c0b1d9e020a212bea9b35782fa5c7e4f2252',
};

const EVENTS = [
  'SessionStart',
  'UserPromptSubmit',
  'PreToolUse',
  'PostToolUse',
  'Stop',
  'PostCompact',
  'SessionEnd',
];

type Group = { matcher?: string; oboete?: true; hooks: Record<string, unknown>[] };

function hooksOf(file: string): Record<string, Group[]> {
  return (JSON.parse(readFileSync(file, 'utf8')) as { hooks: Record<string, Group[]> }).hooks;
}

test('the trust rule reproduces the Phase 1 probe hashes for handlers without a timeout', () => {
  assert.equal(
    trustedHash('SessionStart', 'startup|clear|compact', {
      type: 'command',
      command: command('SessionStart'),
    }),
    PROBE_HASHES.sessionStart,
  );
  assert.equal(
    trustedHash('UserPromptSubmit', undefined, {
      type: 'command',
      command: command('UserPromptSubmit'),
    }),
    PROBE_HASHES.userPromptSubmit,
  );
  // SessionEnd normalizes to one second where every other event normalizes to six hundred.
  assert.equal(
    trustedHash('SessionEnd', undefined, { type: 'command', command: command('SessionEnd') }),
    PROBE_HASHES.sessionEnd,
  );
  assert.equal(
    trustKey('/home/dev/.codex/hooks.json', 'UserPromptSubmit', 2, 1),
    '/home/dev/.codex/hooks.json:user_prompt_submit:2:1',
  );
});

test('the trust identity carries the explicit timeout and the disabled context limit', () => {
  const start = { type: 'command' as const, command: command('SessionStart'), timeout: 12, additionalContextLimit: 0 };
  assert.equal(trustIdentity('SessionStart', 'startup|clear|compact', start), OBOETE_IDENTITY.sessionStart);
  assert.equal(trustedHash('SessionStart', 'startup|clear|compact', start), OBOETE_HASHES.sessionStart);

  const end = { type: 'command' as const, command: command('SessionEnd'), timeout: 3 };
  assert.equal(trustIdentity('SessionEnd', undefined, end), OBOETE_IDENTITY.sessionEnd);
  assert.equal(trustedHash('SessionEnd', undefined, end), OBOETE_HASHES.sessionEnd);

  // Codex clamps a SessionEnd timeout to three seconds and hashes the clamped value, so a larger
  // one in the file would leave the row pointing at an identity Codex never computes.
  assert.equal(
    trustIdentity('SessionEnd', undefined, { type: 'command', command: command('SessionEnd'), timeout: 30 }),
    OBOETE_IDENTITY.sessionEnd,
  );
  // The default 2,500 is the one value the identity leaves out.
  assert.equal(
    trustIdentity('PreToolUse', undefined, {
      type: 'command',
      command: 'x',
      timeout: 3,
      additionalContextLimit: 2500,
    }),
    '{"event_name":"pre_tool_use","hooks":[{"async":false,"command":"x","timeout":3,"type":"command"}]}',
  );
});

test('writeCodex wires the seven events and repeats byte-identically', async () => {
  await withTempHome(async (home) => {
    const result = writeCodex(home, { node: NODE, bundle: BUNDLE });
    const hooksPath = join(home, 'hooks.json');
    const configPath = join(home, 'config.toml');
    assert.deepEqual(result.files, [hooksPath, configPath]);

    const hooks = hooksOf(hooksPath);
    assert.deepEqual(Object.keys(hooks), EVENTS);
    assert.deepEqual(hooks.SessionStart, [
      {
        matcher: 'startup|clear|compact',
        hooks: [
          {
            type: 'command',
            command: command('SessionStart'),
            timeout: 12,
            additionalContextLimit: 0,
          },
        ],
        oboete: true,
      },
    ]);
    assert.deepEqual(hooks.UserPromptSubmit[0].hooks, [
      {
        type: 'command',
        command: command('UserPromptSubmit'),
        timeout: 12,
        additionalContextLimit: 0,
      },
    ]);
    assert.equal('matcher' in hooks.UserPromptSubmit[0], false, 'only SessionStart takes a matcher');
    for (const event of ['PreToolUse', 'PostToolUse', 'Stop', 'PostCompact', 'SessionEnd']) {
      assert.deepEqual(
        hooks[event][0].hooks,
        [{ type: 'command', command: command(event), timeout: 3 }],
        `${event} is a capture hook: three seconds, no additional context`,
      );
    }

    const config = parseToml(readFileSync(configPath, 'utf8')) as {
      mcp_servers: { oboete: { command: string; args: string[] } };
      hooks: { state: Record<string, { trusted_hash: string }> };
    };
    assert.deepEqual(config.mcp_servers.oboete, { command: NODE, args: [BUNDLE, 'mcp'] });
    assert.deepEqual(
      Object.keys(config.hooks.state),
      EVENTS.map((event) => trustKey(hooksPath, event, 0, 0)),
    );
    assert.equal(
      config.hooks.state[trustKey(hooksPath, 'SessionStart', 0, 0)].trusted_hash,
      OBOETE_HASHES.sessionStart,
    );
    assert.equal(
      config.hooks.state[trustKey(hooksPath, 'SessionEnd', 0, 0)].trusted_hash,
      OBOETE_HASHES.sessionEnd,
    );

    const written = [readFileSync(hooksPath, 'utf8'), readFileSync(configPath, 'utf8')];
    writeCodex(home, { node: NODE, bundle: BUNDLE });
    assert.deepEqual(
      [readFileSync(hooksPath, 'utf8'), readFileSync(configPath, 'utf8')],
      written,
      'a repeated setup writes the same bytes',
    );
  });
});

test('the trust rows follow the merged handler positions and cover only oboete handlers', async () => {
  await withTempHome(async (home) => {
    const userGroup = { matcher: 'startup', hooks: [{ type: 'command', command: 'notify-send hi' }] };
    const hooksPath = join(home, 'hooks.json');
    mkdirSync(home, { recursive: true });
    writeFileSync(
      hooksPath,
      `${JSON.stringify({ hooks: { SessionStart: [userGroup], PreCompact: [userGroup] } }, null, 2)}\n`,
    );

    writeCodex(home, { node: NODE, bundle: BUNDLE });

    const hooks = hooksOf(hooksPath);
    assert.deepEqual(hooks.SessionStart[0], userGroup, 'the developer keeps the first slot');
    assert.equal(hooks.SessionStart[1].oboete, true);
    assert.deepEqual(hooks.PreCompact, [userGroup], 'an event oboete does not wire is untouched');

    const config = readFileSync(join(home, 'config.toml'), 'utf8');
    assert.ok(config.includes(trustKey(hooksPath, 'SessionStart', 1, 0)), 'the merged position');
    assert.equal(
      config.includes(trustKey(hooksPath, 'SessionStart', 0, 0)),
      false,
      'oboete never trusts a handler the developer wrote',
    );
  });
});

test('removeCodex gives the developer both files back', async () => {
  await withTempHome(async (home) => {
    const userGroup = { matcher: 'startup', hooks: [{ type: 'command', command: 'notify-send hi' }] };
    const settings = { hooks: { SessionStart: [userGroup] } };
    mkdirSync(home, { recursive: true });
    const hooksPath = join(home, 'hooks.json');
    const configPath = join(home, 'config.toml');
    writeFileSync(hooksPath, `${JSON.stringify(settings, null, 2)}\n`);
    const config = '# the developer wrote this\nmodel = "gpt-5"\n\n[features]\nhooks = true\n';
    writeFileSync(configPath, config);

    writeCodex(home, { node: NODE, bundle: BUNDLE });
    removeCodex(home);

    assert.deepEqual(JSON.parse(readFileSync(hooksPath, 'utf8')), settings);
    assert.equal(readFileSync(configPath, 'utf8'), config, 'the TOML comes back byte for byte');
    assert.equal(existsSync(configPath + BACKUP_SUFFIX), false);
    assert.equal(existsSync(hooksPath + BACKUP_SUFFIX), false);
    removeCodex(home);
    assert.equal(readFileSync(configPath, 'utf8'), config, 'a second removal changes nothing');
  });
});

test('removeCodex takes back the files oboete created', async () => {
  await withTempHome(async (home) => {
    writeCodex(home, { node: NODE, bundle: BUNDLE });
    removeCodex(home);
    assert.deepEqual(readdirSync(home), [], 'a home oboete wired is a home it can empty again');
  });
});

/** The developer registered oboete by hand; appending the block would define the table twice. */
const HAND_WRITTEN_MCP = '[mcp_servers.oboete]\ncommand = "oboete"\nargs = ["mcp"]\n';

test('a setup that cannot write config.toml puts hooks.json back as it found it', async () => {
  await withTempHome(async (home) => {
    mkdirSync(home, { recursive: true });
    const configPath = join(home, 'config.toml');
    const hooksPath = join(home, 'hooks.json');
    writeFileSync(configPath, HAND_WRITTEN_MCP);
    const userGroup = { matcher: 'startup', hooks: [{ type: 'command', command: 'notify-send hi' }] };
    const hooks = `${JSON.stringify({ hooks: { SessionStart: [userGroup] } }, null, 2)}\n`;
    writeFileSync(hooksPath, hooks);

    assert.throws(
      () => writeCodex(home, { node: NODE, bundle: BUNDLE }),
      (error: unknown) => error instanceof ManagedFileError && error.code === 'reparse_failed',
    );
    assert.equal(readFileSync(configPath, 'utf8'), HAND_WRITTEN_MCP);
    // Handlers whose trust rows were never written are wired and silently inert: Codex skips a
    // hook it does not trust and exits 0, so a failed setup takes them back out (FR-031).
    assert.equal(readFileSync(hooksPath, 'utf8'), hooks, 'no oboete handler survives the failure');
    assert.deepEqual(
      readdirSync(home).sort(),
      ['config.toml', 'hooks.json'],
      'not even a backup of the run that failed is left in the developer\'s home',
    );
  });
});

test('a setup that fails leaves behind no hooks.json of its own', async () => {
  await withTempHome(async (home) => {
    mkdirSync(home, { recursive: true });
    writeFileSync(join(home, 'config.toml'), HAND_WRITTEN_MCP);

    assert.throws(() => writeCodex(home, { node: NODE, bundle: BUNDLE }), ManagedFileError);
    assert.deepEqual(readdirSync(home), ['config.toml'], 'only the developer\'s file is left');
  });
});

test('a failing repeat keeps the copy of config.toml from before oboete', async () => {
  await withTempHome(async (home) => {
    mkdirSync(home, { recursive: true });
    const configPath = join(home, 'config.toml');
    const original = '# the developer wrote this\nmodel = "gpt-5"\n';
    writeFileSync(configPath, original);

    writeCodex(home, { node: NODE, bundle: BUNDLE });
    // The developer registers oboete by hand next to the block oboete already wrote.
    writeFileSync(configPath, readFileSync(configPath, 'utf8') + HAND_WRITTEN_MCP);

    assert.throws(
      () => writeCodex(home, { node: NODE, bundle: BUNDLE }),
      (error: unknown) => error instanceof ManagedFileError && error.code === 'reparse_failed',
    );
    // The backup is the only copy of the file from before oboete: rolling back a repeat must not
    // take it, or `oboete setup --remove` would have nothing left to restore.
    assert.equal(readFileSync(configPath + BACKUP_SUFFIX, 'utf8'), original);
    // The handlers of the failed run go, so nothing is wired to a trust row that was never written.
    assert.equal('hooks' in JSON.parse(readFileSync(join(home, 'hooks.json'), 'utf8')), false);
  });
});

test('a failing repeat keeps the copy of hooks.json from before oboete', async () => {
  await withTempHome(async (home) => {
    mkdirSync(home, { recursive: true });
    const configPath = join(home, 'config.toml');
    const hooksPath = join(home, 'hooks.json');
    const userGroup = { matcher: 'startup', hooks: [{ type: 'command', command: 'notify-send hi' }] };
    const original = `${JSON.stringify({ hooks: { SessionStart: [userGroup] } }, null, 2)}\n`;
    writeFileSync(hooksPath, original);

    writeCodex(home, { node: NODE, bundle: BUNDLE });
    assert.equal(readFileSync(hooksPath + BACKUP_SUFFIX, 'utf8'), original);
    // The developer registers oboete by hand next to the block oboete already wrote.
    writeFileSync(configPath, readFileSync(configPath, 'utf8') + HAND_WRITTEN_MCP);

    assert.throws(
      () => writeCodex(home, { node: NODE, bundle: BUNDLE }),
      (error: unknown) => error instanceof ManagedFileError && error.code === 'reparse_failed',
    );
    // The same rule the config.toml backup has: the copy from before oboete is what
    // `oboete setup --remove` restores, so a rollback of a later run must not take it.
    assert.equal(readFileSync(hooksPath + BACKUP_SUFFIX, 'utf8'), original);
  });
});
