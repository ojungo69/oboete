import assert from 'node:assert/strict';
import { chmodSync, existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { parse as parseToml } from 'smol-toml';

import { BACKUP_SUFFIX, ManagedFileError } from '../../src/setup/managed-block.js';
import { removeGrok, writeGrok } from '../../src/setup/write-grok.js';
import { withTempHome } from '../helpers/home.js';

const NODE = '/usr/local/bin/node';
const BUNDLE = '/opt/oboete/dist/oboete.mjs';

/**
 * The timeout the agent must see per event, written out rather than derived: 12 s on the hooks
 * that build or deliver a pack (FR-045 puts the pack on `PreToolUse`/`PostToolUse`), 3 s on the
 * capture-only ones (contracts/agents.md "Capture and injection per agent").
 */
const EXPECTED_TIMEOUTS: Record<string, number> = {
  SessionStart: 12,
  UserPromptSubmit: 12,
  PreToolUse: 12,
  PostToolUse: 12,
  PostToolUseFailure: 3,
  PermissionDenied: 3,
  Stop: 3,
  PostCompact: 3,
  SessionEnd: 3,
};

type HookEntry = { hooks: { type: string; command: string; timeout: number }[]; oboete?: boolean };

function grokHome(home: string): string {
  return join(home, '.grok');
}

function hooksFile(home: string): string {
  return join(grokHome(home), 'hooks', 'oboete.json');
}

function configFile(home: string): string {
  return join(grokHome(home), 'config.toml');
}

function readHooks(home: string): Record<string, HookEntry[]> {
  const parsed = JSON.parse(readFileSync(hooksFile(home), 'utf8')) as {
    hooks: Record<string, HookEntry[]>;
  };
  return parsed.hooks;
}

function write(file: string, content: string): string {
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, content);
  return file;
}

test('writeGrok wires every Grok event with its own timeout and the fixed selector', async () => {
  await withTempHome(async (home) => {
    writeGrok(grokHome(home), { nodePath: NODE, bundlePath: BUNDLE });

    const hooks = readHooks(home);
    assert.deepEqual(Object.keys(hooks).sort(), Object.keys(EXPECTED_TIMEOUTS).sort());
    for (const [event, timeout] of Object.entries(EXPECTED_TIMEOUTS)) {
      const entries = hooks[event];
      assert.equal(entries.length, 1, `${event} has one oboete entry`);
      assert.equal(entries[0].oboete, true, `${event} is marked oboete-owned so removal finds it`);
      assert.equal(entries[0].hooks.length, 1);
      const handler = entries[0].hooks[0];
      assert.equal(handler.type, 'command');
      assert.equal(
        typeof handler.timeout,
        'number',
        `${event} carries an explicit numeric timeout (FR-031)`,
      );
      assert.equal(handler.timeout, timeout, `${event} timeout`);
      assert.equal(
        handler.command,
        `'${NODE}' '${BUNDLE}' hook --agent claude-or-grok --event ${event}`,
      );
    }
  });
});

test('writeGrok registers the MCP server and repeats byte-identically over the developer edits', async () => {
  await withTempHome(async (home) => {
    const head = 'model = "grok-4.6-build"\n\n[compat.claude]\nhooks = true\n';
    write(configFile(home), head);

    writeGrok(grokHome(home), { nodePath: NODE, bundlePath: BUNDLE });
    const config = readFileSync(configFile(home), 'utf8');
    assert.ok(config.startsWith(head), 'the developer bytes stay ahead of the block');
    const server = (parseToml(config) as { mcp_servers: Record<string, unknown> }).mcp_servers
      .oboete;
    assert.deepEqual(server, { command: NODE, args: [BUNDLE, 'mcp'], enabled: true });

    const firstHooks = readFileSync(hooksFile(home), 'utf8');
    writeGrok(grokHome(home), { nodePath: NODE, bundlePath: BUNDLE });
    assert.equal(readFileSync(hooksFile(home), 'utf8'), firstHooks, 'the hooks file is unchanged');
    assert.equal(readFileSync(configFile(home), 'utf8'), config, 'the block is unchanged');

    // An edit outside the block survives the next setup.
    const edited = `${config}\n[history]\npersistence = "save-all"\n`;
    writeFileSync(configFile(home), edited);
    writeGrok(grokHome(home), { nodePath: NODE, bundlePath: BUNDLE });
    assert.equal(readFileSync(configFile(home), 'utf8'), edited);
  });
});

test('the copy writeGrok takes of config.toml is never world-readable', async () => {
  await withTempHome(async (home) => {
    // `config.toml` is where the developer's MCP servers live, and an `env` table there holds the
    // tokens they run those servers with, so the pre-oboete copy is 0600 whatever mode the
    // original carried (research.md R8, contracts/cli.md "0600 for credential-bearing files").
    const original = '[mcp_servers.paid.env]\nAPI_TOKEN = "a token oboete never spreads"\n';
    write(configFile(home), original);
    chmodSync(configFile(home), 0o644);

    writeGrok(grokHome(home), { nodePath: NODE, bundlePath: BUNDLE });

    const backup = configFile(home) + BACKUP_SUFFIX;
    assert.equal(readFileSync(backup, 'utf8'), original, 'the backup is the file as it stood');
    assert.equal(statSync(backup).mode & 0o777, 0o600, 'the backup may hold a token');
  });
});

test('removeGrok takes the hooks file and the MCP block away again', async () => {
  await withTempHome(async (home) => {
    const original = 'model = "grok-4.6-build"\n';
    write(configFile(home), original);

    writeGrok(grokHome(home), { nodePath: NODE, bundlePath: BUNDLE });
    removeGrok(grokHome(home));

    assert.equal(existsSync(hooksFile(home)), false, 'a file only oboete filled is deleted');
    assert.equal(readFileSync(configFile(home), 'utf8'), original);
  });
});

test('removeGrok keeps a hooks file that still holds a handler the developer added', async () => {
  await withTempHome(async (home) => {
    writeGrok(grokHome(home), { nodePath: NODE, bundlePath: BUNDLE });
    const foreign = { hooks: [{ type: 'command', command: 'notify-send hi' }] };
    const parsed = JSON.parse(readFileSync(hooksFile(home), 'utf8')) as {
      hooks: Record<string, unknown[]>;
    };
    parsed.hooks.PreCompact = [foreign];
    writeFileSync(hooksFile(home), JSON.stringify(parsed, null, 2));

    removeGrok(grokHome(home));

    assert.deepEqual(readHooks(home), { PreCompact: [foreign] });
  });
});

test('writeGrok reports the Claude handlers the compat layer would run a second time', async () => {
  await withTempHome(async (home) => {
    const settings = write(
      join(home, '.claude', 'settings.json'),
      JSON.stringify({
        hooks: {
          SessionStart: [
            { hooks: [{ type: 'command', command: 'oboete', timeout: 12 }], oboete: true },
          ],
          Stop: [{ hooks: [{ type: 'command', command: 'oboete', timeout: 3 }], oboete: true }],
          // The developer's own handler duplicates nothing of oboete's, and a marker that is not
          // `true` is not a marker: managed-block would not remove that entry either.
          PostToolUse: [{ hooks: [{ type: 'command', command: 'notify-send hi' }], oboete: false }],
          // oboete never wires PreCompact on Grok, so this one cannot collide either.
          PreCompact: [{ hooks: [{ type: 'command', command: 'oboete', timeout: 3 }], oboete: true }],
        },
      }),
    );

    const result = writeGrok(grokHome(home), {
      nodePath: NODE,
      bundlePath: BUNDLE,
      claudeSettingsPath: settings,
    });
    assert.deepEqual(result.duplicateClaudeEvents, ['SessionStart', 'Stop']);
    assert.deepEqual(result.files, [hooksFile(home), configFile(home)]);

    const alone = writeGrok(join(home, 'other-grok'), {
      nodePath: NODE,
      bundlePath: BUNDLE,
      claudeSettingsPath: join(home, '.claude', 'absent.json'),
    });
    assert.deepEqual(alone.duplicateClaudeEvents, [], 'no Claude file, nothing to warn about');
  });
});

/** The developer registered oboete by hand; appending the block would define the table twice. */
const HAND_WRITTEN_MCP = '[mcp_servers.oboete]\ncommand = "oboete"\nargs = ["mcp"]\n';

test('a setup that cannot write config.toml leaves no live handler behind', async () => {
  await withTempHome(async (home) => {
    write(configFile(home), HAND_WRITTEN_MCP);

    assert.throws(
      () => writeGrok(grokHome(home), { nodePath: NODE, bundlePath: BUNDLE }),
      (error: unknown) => error instanceof ManagedFileError && error.code === 'reparse_failed',
    );
    // The handlers are written before the MCP block, so a failure at the block would otherwise
    // leave Grok running oboete's hooks while setup reports `wired: failed` (FR-031).
    assert.equal(existsSync(hooksFile(home)), false, 'the hooks file of the failed run goes');
    assert.equal(readFileSync(configFile(home), 'utf8'), HAND_WRITTEN_MCP);
    // `oboete setup --remove` leaves the directory too; what must be gone is everything in it.
    assert.deepEqual(readdirSync(dirname(hooksFile(home))), [], 'no backup and no temporary file');
  });
});

test('a failing repeat keeps the copy of the hooks file from before oboete', async () => {
  await withTempHome(async (home) => {
    const foreign = { hooks: [{ type: 'command', command: 'notify-send hi' }] };
    const original = `${JSON.stringify({ hooks: { PreCompact: [foreign] } }, null, 2)}\n`;
    write(hooksFile(home), original);

    writeGrok(grokHome(home), { nodePath: NODE, bundlePath: BUNDLE });
    assert.equal(readFileSync(hooksFile(home) + BACKUP_SUFFIX, 'utf8'), original);
    // The developer registers oboete by hand next to the block oboete already wrote.
    write(configFile(home), readFileSync(configFile(home), 'utf8') + HAND_WRITTEN_MCP);

    assert.throws(
      () => writeGrok(grokHome(home), { nodePath: NODE, bundlePath: BUNDLE }),
      (error: unknown) => error instanceof ManagedFileError && error.code === 'reparse_failed',
    );
    assert.deepEqual(readHooks(home), { PreCompact: [foreign] }, 'no oboete handler stays live');
    // The copy from before oboete is what `oboete setup --remove` restores.
    assert.equal(readFileSync(hooksFile(home) + BACKUP_SUFFIX, 'utf8'), original);
    assert.equal(
      existsSync(configFile(home) + BACKUP_SUFFIX),
      false,
      'the copy this run took of a file it then left alone is its own leftover',
    );
  });
});
