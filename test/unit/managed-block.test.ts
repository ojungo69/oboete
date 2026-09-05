import assert from 'node:assert/strict';
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { dirname, join } from 'node:path';
import { test } from 'node:test';

import {
  applyJsonHandlers,
  applyTomlBlock,
  BACKUP_SUFFIX,
  ManagedFileError,
  removeJsonHandlers,
  removeTomlBlock,
} from '../../src/setup/managed-block.js';
import { withTempHome } from '../helpers/home.js';

const BLOCK = '[hooks.state."/opt/oboete.mjs:SessionStart:oboete:capture"]\ntrusted_hash = "sha256:beef"';

/**
 * A foreign agent file inside the temporary home; `mode` is what the developer's file carries.
 * Without `content` only the path is composed: the directory is left absent, the way a fresh
 * machine has no `~/.grok/hooks/`.
 */
function agentFile(home: string, name: string, content?: string, mode = 0o644): string {
  const file = join(home, name);
  if (content !== undefined) {
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, content);
    chmodSync(file, mode);
  }
  return file;
}

function isManagedFileError(code: string) {
  return (error: unknown) => error instanceof ManagedFileError && error.code === code;
}

function oboeteHandler(command: string, timeout: number) {
  return {
    matcher: 'startup|clear|compact',
    hooks: [{ type: 'command', command, timeout }],
    oboete: true,
  };
}

const userHandler = { matcher: 'startup', hooks: [{ type: 'command', command: 'notify-send hi' }] };

test('applyTomlBlock appends once, repeats byte-identically, and removeTomlBlock restores the file', async () => {
  await withTempHome(async (home) => {
    const original = '# the developer wrote this\nmodel = "gpt-5"\n\n[hooks]\nenabled = true\n';
    const file = agentFile(home, '.codex/config.toml', original);

    applyTomlBlock(file, BLOCK, { credentialBearing: true });
    const first = readFileSync(file, 'utf8');
    assert.ok(first.startsWith(original), 'the developer bytes stay untouched ahead of the block');
    assert.match(first, /\n# oboete:begin\n/);
    assert.match(first, /\n# oboete:end\n$/);

    applyTomlBlock(file, BLOCK, { credentialBearing: true });
    assert.equal(readFileSync(file, 'utf8'), first, 'a repeated setup writes the same bytes');

    removeTomlBlock(file);
    assert.equal(readFileSync(file, 'utf8'), original, 'removal restores the pre-setup bytes');
  });
});

test('applyTomlBlock replaces exactly the managed region and keeps the bytes around it', async () => {
  await withTempHome(async (home) => {
    const head = 'model = "gpt-5"\n\n';
    const tail = '\n[history]\npersistence = "save-all"\n';
    const file = agentFile(
      home,
      '.codex/config.toml',
      `${head}# oboete:begin\n[old]\nvalue = "stale"\n# oboete:end\n${tail}`,
    );

    applyTomlBlock(file, BLOCK);
    const after = readFileSync(file, 'utf8');
    assert.ok(after.startsWith(head));
    assert.ok(after.endsWith(tail));
    assert.ok(!after.includes('stale'), 'the previous block is gone');
    assert.equal(after.match(/# oboete:begin/g)?.length, 1);
    assert.equal(after.match(/# oboete:end/g)?.length, 1);
  });
});

test('applyTomlBlock refuses a file whose delimiters do not pair up', async () => {
  await withTempHome(async (home) => {
    const dangling = 'model = "gpt-5"\n# oboete:begin\n[old]\nvalue = 1\n';
    const one = agentFile(home, '.codex/one.toml', dangling);
    assert.throws(() => applyTomlBlock(one, BLOCK), isManagedFileError('malformed_block'));
    assert.equal(readFileSync(one, 'utf8'), dangling, 'the file is left alone');

    const twice =
      '# oboete:begin\na = 1\n# oboete:end\n# oboete:begin\nb = 2\n# oboete:end\n';
    const two = agentFile(home, '.codex/two.toml', twice);
    assert.throws(() => applyTomlBlock(two, BLOCK), isManagedFileError('malformed_block'));
    assert.equal(readFileSync(two, 'utf8'), twice);
  });
});

test('applyTomlBlock leaves the original in place when the result does not parse', async () => {
  await withTempHome(async (home) => {
    const original = 'model = "gpt-5"\n';
    const file = agentFile(home, '.codex/config.toml', original);

    assert.throws(() => applyTomlBlock(file, 'not = = toml'), isManagedFileError('reparse_failed'));
    assert.equal(readFileSync(file, 'utf8'), original);
    assert.deepEqual(
      readdirSync(dirname(file)).filter((name) => name.includes('oboete-tmp')),
      [],
      'the temporary file is cleaned up',
    );
  });
});

test('a credential-bearing file gets a 0600 backup, taken once, and removal deletes it', async () => {
  await withTempHome(async (home) => {
    const original = 'model = "gpt-5"\n';
    const file = agentFile(home, '.codex/config.toml', original, 0o644);
    const backup = file + BACKUP_SUFFIX;

    applyTomlBlock(file, BLOCK, { credentialBearing: true });
    assert.equal(statSync(backup).mode & 0o777, 0o600, 'the backup may hold credentials');
    assert.equal(readFileSync(backup, 'utf8'), original);
    assert.equal(statSync(file).mode & 0o777, 0o644, 'the rewritten file keeps the original mode');

    applyTomlBlock(file, '[other]\nvalue = 1', { credentialBearing: true });
    assert.equal(readFileSync(backup, 'utf8'), original, 'the pre-setup backup is not overwritten');

    removeTomlBlock(file);
    assert.equal(existsSync(backup), false);
  });
});

test('a missing file is created, with its missing directory, as two-space JSON with a trailing newline', async () => {
  await withTempHome(async (home) => {
    // `~/.grok/hooks/` does not exist before setup runs (contracts/agents.md, Setup column).
    const file = agentFile(home, '.grok/hooks/oboete.json');

    applyJsonHandlers(file, { SessionStart: [oboeteHandler('oboete hook', 12)] });
    const written = readFileSync(file, 'utf8');
    assert.equal(written.at(-1), '\n');
    assert.ok(written.includes('\n  "hooks": {'), 'two-space indentation');
    assert.equal(statSync(file).mode & 0o777, 0o600, 'a file oboete creates is owner-only');
    assert.equal(existsSync(file + BACKUP_SUFFIX), false, 'nothing existed to back up');

    const toml = agentFile(home, '.codex/nested/config.toml');
    applyTomlBlock(toml, BLOCK);
    assert.match(readFileSync(toml, 'utf8'), /# oboete:begin/, 'the TOML writer creates it too');
  });
});

test('a repeat that stops wiring an event takes its handler with it', async () => {
  await withTempHome(async (home) => {
    const file = agentFile(home, '.claude/settings.json', '{}\n', 0o644);
    applyJsonHandlers(file, {
      SessionStart: [oboeteHandler('oboete hook --event SessionStart', 12)],
      PostToolUseFailure: [oboeteHandler('oboete hook --event PostToolUseFailure', 12)],
    });

    const edited = JSON.parse(readFileSync(file, 'utf8'));
    edited.hooks.PostToolUseFailure.push(userHandler);
    writeFileSync(file, `${JSON.stringify(edited, null, 2)}\n`);

    // FR-031 "repeatable": the narrower setup must not leave the dropped hook live.
    applyJsonHandlers(file, {
      SessionStart: [oboeteHandler('oboete hook --event SessionStart', 12)],
    });
    const after = JSON.parse(readFileSync(file, 'utf8'));
    assert.deepEqual(after.hooks.PostToolUseFailure, [userHandler]);
    assert.deepEqual(after.hooks.SessionStart, [
      oboeteHandler('oboete hook --event SessionStart', 12),
    ]);

    applyJsonHandlers(file, { Stop: [oboeteHandler('oboete hook --event Stop', 3)] });
    const narrower = JSON.parse(readFileSync(file, 'utf8'));
    assert.equal('SessionStart' in narrower.hooks, false, 'the emptied array goes too');
  });
});

test('applyJsonHandlers replaces oboete entries in place and leaves the user entries in order', async () => {
  await withTempHome(async (home) => {
    const before = { matcher: 'startup', hooks: [{ type: 'command', command: 'first' }] };
    const after = { matcher: 'startup', hooks: [{ type: 'command', command: 'last' }] };
    const file = agentFile(
      home,
      '.claude/settings.json',
      `${JSON.stringify({ hooks: { SessionStart: [before, oboeteHandler('old', 3), after] }, model: 'opus' }, null, 2)}\n`,
      0o644,
    );

    applyJsonHandlers(file, { SessionStart: [oboeteHandler('oboete hook', 12)] });
    const parsed = JSON.parse(readFileSync(file, 'utf8'));
    assert.deepEqual(parsed.hooks.SessionStart, [before, oboeteHandler('oboete hook', 12), after]);
    assert.equal(parsed.model, 'opus', 'unrelated keys survive');
    assert.equal(statSync(file).mode & 0o777, 0o644);
    assert.equal(statSync(file + BACKUP_SUFFIX).mode & 0o777, 0o644, 'a plain backup keeps the mode');

    const written = readFileSync(file, 'utf8');
    applyJsonHandlers(file, { SessionStart: [oboeteHandler('oboete hook', 12)] });
    assert.equal(readFileSync(file, 'utf8'), written, 'a repeated setup writes the same bytes');
  });
});

test('a developer edit made after setup survives the next setup and the removal', async () => {
  await withTempHome(async (home) => {
    const file = agentFile(home, '.claude/settings.json', '{}\n', 0o644);
    applyJsonHandlers(file, { SessionStart: [oboeteHandler('oboete hook', 12)] });

    const edited = JSON.parse(readFileSync(file, 'utf8'));
    edited.hooks.SessionStart.push(userHandler);
    edited.model = 'sonnet';
    writeFileSync(file, `${JSON.stringify(edited, null, 2)}\n`);

    applyJsonHandlers(file, { SessionStart: [oboeteHandler('oboete hook', 12)] });
    const reapplied = JSON.parse(readFileSync(file, 'utf8'));
    assert.deepEqual(reapplied.hooks.SessionStart, [oboeteHandler('oboete hook', 12), userHandler]);
    assert.equal(reapplied.model, 'sonnet');

    removeJsonHandlers(file);
    const removed = JSON.parse(readFileSync(file, 'utf8'));
    assert.deepEqual(removed.hooks.SessionStart, [userHandler], 'only oboete entries are stripped');
    assert.equal(removed.model, 'sonnet');
  });
});

test('removeJsonHandlers drops the arrays and the container it emptied and restores the settings', async () => {
  await withTempHome(async (home) => {
    // Indented the developer's way, not the serializer's: JSON has no textual managed block, so
    // removal restores the settings, not the byte layout (plan.md row 12 binds the TOML file only).
    const settings = { model: 'opus', permissions: { allow: ['Bash(git status:*)'] } };
    const original = `${JSON.stringify(settings, null, 4)}\n`;
    const file = agentFile(home, '.claude/settings.json', original, 0o644);

    applyJsonHandlers(file, {
      SessionStart: [oboeteHandler('oboete hook', 12)],
      Stop: [oboeteHandler('oboete hook stop', 3)],
    });
    assert.ok(readFileSync(file, 'utf8').includes('SessionStart'));

    removeJsonHandlers(file);
    const restored = readFileSync(file, 'utf8');
    assert.deepEqual(JSON.parse(restored), settings, 'removal restores the pre-setup settings');
    assert.equal(restored.includes('oboete'), false, 'and leaves no trace of oboete behind');
    assert.equal(existsSync(file + BACKUP_SUFFIX), false);

    removeJsonHandlers(file);
    assert.equal(readFileSync(file, 'utf8'), restored, 'a second removal changes nothing');
  });
});

test('applyJsonHandlers refuses a root that is not an object and a handler without the marker', async () => {
  await withTempHome(async (home) => {
    const list = agentFile(home, '.claude/settings.json', '[]\n');
    assert.throws(
      () => applyJsonHandlers(list, { SessionStart: [oboeteHandler('oboete hook', 12)] }),
      isManagedFileError('not_an_object'),
    );
    assert.equal(readFileSync(list, 'utf8'), '[]\n');

    const file = agentFile(home, '.claude/other.json', '{}\n');
    assert.throws(
      () => applyJsonHandlers(file, { SessionStart: [{ type: 'command', command: 'x' }] }),
      isManagedFileError('unmarked_handler'),
    );
    assert.equal(readFileSync(file, 'utf8'), '{}\n', 'an unremovable entry is never written');
  });
});

test('a `hooks` key that is not an object is reported instead of overwritten', async () => {
  await withTempHome(async (home) => {
    // Codex skips a handler whose wiring it cannot read, so a `hooks` the developer left as a
    // string or a list has to surface as an error rather than be replaced by oboete's container.
    for (const malformed of ['"off"', '[]']) {
      const original = `{\n  "hooks": ${malformed}\n}\n`;
      const file = agentFile(home, `.claude/hooks-${malformed.length}.json`, original);
      assert.throws(
        () => applyJsonHandlers(file, { SessionStart: [oboeteHandler('oboete hook', 12)] }),
        isManagedFileError('not_an_object'),
      );
      assert.throws(() => removeJsonHandlers(file), isManagedFileError('not_an_object'));
      assert.equal(readFileSync(file, 'utf8'), original, 'the developer\'s file is left alone');
    }
  });
});

test('a symlink that leaves its directory is refused, one that stays is written through', async () => {
  await withTempHome(async (home) => {
    const outside = agentFile(home, 'dotfiles/config.toml', 'model = "gpt-5"\n');
    const escaping = join(home, '.codex', 'config.toml');
    mkdirSync(dirname(escaping), { recursive: true });
    symlinkSync(outside, escaping);
    assert.throws(() => applyTomlBlock(escaping, BLOCK), isManagedFileError('symlink_escape'));
    assert.equal(readFileSync(outside, 'utf8'), 'model = "gpt-5"\n');

    const escapingJson = join(home, '.claude', 'settings.json');
    mkdirSync(dirname(escapingJson), { recursive: true });
    symlinkSync(agentFile(home, 'dotfiles/settings.json', '{}\n'), escapingJson);
    assert.throws(
      () => applyJsonHandlers(escapingJson, { SessionStart: [oboeteHandler('h', 12)] }),
      isManagedFileError('symlink_escape'),
    );

    const real = agentFile(home, '.codex/real.toml', 'model = "gpt-5"\n');
    const inside = join(home, '.codex', 'link.toml');
    symlinkSync(real, inside);
    applyTomlBlock(inside, BLOCK);
    assert.match(readFileSync(real, 'utf8'), /# oboete:begin/);
    assert.ok(lstatSync(inside).isSymbolicLink(), 'the symlink itself is preserved');
  });
});

test('a symbolic link left at the temporary path is refused instead of written through', async () => {
  await withTempHome(async (home) => {
    const original = 'model = "gpt-5"\n';
    const file = agentFile(home, '.codex/config.toml', original);
    const victim = join(home, 'victim.toml');
    writeFileSync(victim, 'stays = true\n');
    // The temporary name is predictable, so the write must create it exclusively: an attacker who
    // pre-places a link there otherwise has oboete write the developer's configuration elsewhere.
    symlinkSync(victim, `${file}.oboete-tmp-${process.pid}`);

    assert.throws(() => applyTomlBlock(file, BLOCK));
    assert.equal(readFileSync(victim, 'utf8'), 'stays = true\n', 'the link is never followed');
    assert.equal(readFileSync(file, 'utf8'), original);
    assert.equal(lstatSync(file).isSymbolicLink(), false);
  });
});
