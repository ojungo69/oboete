import assert from 'node:assert/strict';
import { chmodSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';

import { MCP_REMOVE_ARGS, removeClaude, writeClaude } from '../../src/setup/write-claude.js';
import { withTempHome } from '../helpers/home.js';

const NODE = '/opt/node/bin/node';
const BUNDLE = '/opt/oboete/dist/oboete.mjs';

/** The timeout contracts/agents.md gives each Claude Code handler, in seconds. */
const EXPECTED_TIMEOUTS: Record<string, number> = {
  SessionStart: 12,
  UserPromptSubmit: 12,
  PreToolUse: 3,
  PostToolUse: 3,
  PostToolUseFailure: 3,
  Stop: 3,
  PostCompact: 3,
  SessionEnd: 3,
};

function settingsOf(dir: string): Record<string, unknown> {
  return JSON.parse(readFileSync(join(dir, 'settings.json'), 'utf8')) as Record<string, unknown>;
}

test('writeClaude wires every event with the fixed selector, its own event name, and its timeout', async () => {
  await withTempHome(async (home) => {
    const dir = join(home, 'claude-config'); // absent, the way a fresh machine has no ~/.claude

    const result = writeClaude(dir, { nodePath: NODE, bundlePath: BUNDLE });

    assert.equal(result.file, join(dir, 'settings.json'));
    const hooks = settingsOf(dir).hooks as Record<string, unknown[]>;
    assert.deepEqual(Object.keys(hooks), Object.keys(EXPECTED_TIMEOUTS));
    for (const [event, timeout] of Object.entries(EXPECTED_TIMEOUTS)) {
      assert.deepEqual(
        hooks[event],
        [
          {
            hooks: [
              {
                type: 'command',
                command: `'${NODE}' '${BUNDLE}' hook --agent claude-or-grok --event ${event}`,
                timeout,
              },
            ],
            oboete: true,
          },
        ],
        `${event} carries one oboete-owned handler`,
      );
    }
  });
});

test('writeClaude returns the MCP registration arguments and quotes a path the shell would split', async () => {
  await withTempHome(async (home) => {
    const bundle = "/opt/my oboete's/dist/oboete.mjs";

    const result = writeClaude(home, { nodePath: NODE, bundlePath: bundle });

    // No shell is involved in the registration, so its arguments carry no quoting...
    assert.deepEqual(result.mcpArgs, ['mcp', 'add', 'oboete', '--scope', 'user', '--', NODE, bundle, 'mcp']);
    assert.deepEqual(MCP_REMOVE_ARGS, ['mcp', 'remove', 'oboete', '--scope', 'user']);
    // ...while the handler is a command line an agent hands to a shell: src/setup/shell-quote.ts
    // closes the word, escapes the quote and reopens it, for all three writers alike.
    const hooks = settingsOf(home).hooks as Record<string, { hooks: { command: string }[] }[]>;
    assert.equal(
      hooks.Stop[0].hooks[0].command,
      `'${NODE}' '/opt/my oboete'\\''s/dist/oboete.mjs' hook --agent claude-or-grok --event Stop`,
    );
  });
});

test('a repeated writeClaude is byte-identical, and removeClaude gives the developer file back', async () => {
  await withTempHome(async (home) => {
    const original = `${JSON.stringify(
      {
        model: 'opus',
        hooks: {
          Stop: [{ hooks: [{ type: 'command', command: 'notify-send done' }] }],
          Notification: [{ hooks: [{ type: 'command', command: 'say hi' }] }],
        },
        permissions: { allow: ['Bash(git status:*)'] },
      },
      null,
      2,
    )}\n`;
    mkdirSync(home, { recursive: true });
    const file = join(home, 'settings.json');
    writeFileSync(file, original);

    writeClaude(home, { nodePath: NODE, bundlePath: BUNDLE });
    const first = readFileSync(file, 'utf8');
    writeClaude(home, { nodePath: NODE, bundlePath: BUNDLE });
    assert.equal(readFileSync(file, 'utf8'), first, 'a repeated setup writes the same bytes');

    const settings = settingsOf(home);
    assert.deepEqual(settings.model, 'opus');
    assert.deepEqual(settings.permissions, { allow: ['Bash(git status:*)'] });
    const hooks = settings.hooks as Record<string, unknown[]>;
    assert.deepEqual(hooks.Notification, [{ hooks: [{ type: 'command', command: 'say hi' }] }]);
    assert.equal(hooks.Stop.length, 2, 'oboete joins the developer handler on Stop');
    assert.deepEqual(hooks.Stop[0], { hooks: [{ type: 'command', command: 'notify-send done' }] });

    removeClaude(home);
    assert.equal(
      readFileSync(file, 'utf8'),
      original,
      'removal restores the bytes of a file already laid out the serializer\'s way',
    );
    assert.ok(!existsSync(`${file}.oboete-backup`), 'and takes its backup with it');
  });
});

test('removeClaude leaves a settings.json that never held an oboete handler alone', async () => {
  await withTempHome(async (home) => {
    // Laid out the developer's way, not the serializer's. Reading such a file back out through
    // JSON.parse and JSON.stringify is a silent rewrite (formatting, duplicate keys, number
    // precision) of configuration oboete never owned -- FR-031's "without disturbing unrelated
    // configuration" holds for removal on a machine where Claude Code was never wired.
    const original = `${JSON.stringify(
      { hooks: { Stop: [{ hooks: [{ type: 'command', command: 'notify-send done' }] }] } },
      null,
      4,
    )}\n`;
    mkdirSync(home, { recursive: true });
    const file = join(home, 'settings.json');
    writeFileSync(file, original);
    // A setup whose handlers the developer has since deleted by hand leaves this behind.
    writeFileSync(`${file}.oboete-backup`, '{}\n');

    removeClaude(home);

    assert.equal(readFileSync(file, 'utf8'), original, 'nothing of oboete was in there to take out');
    assert.ok(!existsSync(`${file}.oboete-backup`), 'and the backup goes all the same');
  });
});

test('the pre-oboete settings.json is kept in an owner-only backup for the length of the setup', async () => {
  await withTempHome(async (home) => {
    // `settings.json` can carry credentials (`env`, `apiKeyHelper`), so its backup is 0600 whatever
    // mode the developer gave the file (research.md R8).
    const settings = { env: { ANTHROPIC_API_KEY: 'sk-fixture-not-a-key' }, model: 'opus' };
    const original = `${JSON.stringify(settings, null, 4)}\n`;
    mkdirSync(home, { recursive: true });
    const file = join(home, 'settings.json');
    writeFileSync(file, original);
    chmodSync(file, 0o644);

    writeClaude(home, { nodePath: NODE, bundlePath: BUNDLE });

    const backup = `${file}.oboete-backup`;
    assert.equal(readFileSync(backup, 'utf8'), original, 'the backup holds the pre-oboete bytes');
    assert.equal(statSync(backup).mode & 0o777, 0o600, 'a backup that may hold credentials is owner-only');
    assert.equal(statSync(file).mode & 0o777, 0o644, 'the rewritten file keeps the developer mode');

    // The round trip restores the settings, not the layout: JSON has no textual managed block, so
    // the file comes back in the serializer's two-space form. The backup above is the only copy of
    // the developer's own formatting, and removal takes it with the handlers.
    removeClaude(home);
    assert.deepEqual(JSON.parse(readFileSync(file, 'utf8')), settings, 'removal restores the settings');
    assert.ok(!existsSync(backup));
  });
});
