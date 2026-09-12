// dist/oboete.mjs is a launcher (src/launcher.mjs) that turns the V8 compile cache on before the
// engine bundle is compiled, issue #210. Timing is not pinned here -- that belongs to whatever load
// the machine is under. What is pinned is everything the speed-up depends on: the entry file stays
// small, the engine is a separate file, the cache is owner-only and is read back on the next run,
// a directory this user does not exclusively own is refused, a home that cannot hold one costs the
// cache and not the command, and the installer writes the launcher rather than the engine.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  chmodSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  statSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { withTempHome } from '../helpers/home.js';

const root = fileURLToPath(new URL('../../..', import.meta.url));
const bin = join(root, 'dist/oboete.mjs');

function run(home: string, args: string[] = ['--version']) {
  const env: NodeJS.ProcessEnv = { ...process.env, HOME: home, OBOETE_HOME: join(home, '.oboete') };
  // Either variable would send the cache somewhere other than the directory under test, or turn it
  // off entirely, and the child would then fail these assertions for a reason outside the code.
  delete env.NODE_COMPILE_CACHE;
  delete env.NODE_DISABLE_COMPILE_CACHE;
  delete env.XDG_CACHE_HOME;
  return spawnSync(process.execPath, [bin, ...args], { encoding: 'utf8', env });
}

function cacheDir(home: string): string {
  return join(home, '.cache', 'oboete', 'compile');
}

/** Every cache file with its mtime. A cache that is written but never read back rewrites the same
 *  file names (the key is path plus source hash), so paths alone cannot tell a hit from a miss. */
function cacheEntries(home: string): { path: string; mtimeMs: number }[] {
  const entries = readdirSync(cacheDir(home), { recursive: true, withFileTypes: true });
  return entries
    .filter((entry) => entry.isFile())
    .map((entry) => join(entry.parentPath, entry.name))
    .sort()
    .map((path) => ({ path, mtimeMs: statSync(path).mtimeMs }));
}

test('the entry file is a launcher and the engine is the file next to it', () => {
  const launcher = readFileSync(bin, 'utf8');
  assert.equal(launcher, readFileSync(join(root, 'src/launcher.mjs'), 'utf8'));
  assert.ok(launcher.startsWith('#!/usr/bin/env node\n'), 'dist/oboete.mjs keeps its shebang');
  assert.equal(statSync(bin).mode & 0o777, 0o755);
  // A launcher that grew back into the program would be compiled before it could enable anything.
  // Most of the file is the comment explaining why it exists, and a comment costs nothing to
  // compile, so the bound is on the code: adding a paragraph must not fail the build.
  const code = launcher
    .split('\n')
    .filter((line) => line.trim() !== '' && !/^\s*(\/\/|\*|\/\*)/u.test(line))
    .join('\n');
  assert.ok(code.length < 1536, `the launcher's code is ${code.length} bytes`);
  assert.ok(statSync(join(root, 'dist/engine.mjs')).isFile(), 'the engine is a file of its own');
});

test('a run fills an owner-only compile cache and the next run reads it back', async () => {
  await withTempHome((home) => {
    assert.equal(run(home).status, 0);
    assert.equal(statSync(cacheDir(home)).mode & 0o777, 0o700, 'the compile cache is owner-only');
    const first = cacheEntries(home);
    assert.equal(first.length, 1, 'one cache entry after one run');
    assert.equal(run(home).status, 0);
    assert.deepEqual(cacheEntries(home), first, 'the second run rewrote the entry');
    // Unchanged is what a cache that was read back looks like -- and also what one that was never
    // enabled looks like. Corrupting the entry separates them: an enabled cache rejects seven bytes
    // and writes the real thing again, a disabled one leaves them there. Nothing else in the suite
    // can see a launcher that quietly stops enabling the cache (issue #210 round three: the check
    // on the versioned directory inside did exactly that wherever the umask is 002).
    const entry = first[0]?.path as string;
    writeFileSync(entry, '1234567');
    assert.equal(run(home).status, 0);
    assert.notEqual(statSync(entry).size, 7, 'the launcher stopped enabling the cache');
  });
});

for (const mode of [0o777, 0o750]) {
  test(`a cache directory open to anyone else (${mode.toString(8)}) is refused`, async () => {
    await withTempHome((home) => {
      mkdirSync(cacheDir(home), { recursive: true });
      // mkdirSync leaves an existing directory's mode alone, so the launcher has to check it. Being
      // unwritable is not enough here: V8 reads from a directory inside this one that Node creates
      // group-writable under a 002 umask, so nobody else may even traverse in.
      chmodSync(cacheDir(home), mode);
      const result = run(home);
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(cacheEntries(home), [], 'the launcher used a cache directory others can enter');
    });
  });
}

for (const mode of [0o755, 0o775, 0o777]) {
  test(`cache directories above it at ${mode.toString(8)} are accepted`, async () => {
    await withTempHome((home) => {
      // The rule stops at the cache directory itself. Holding the ones above it to the same
      // standard refuses the 0755 ~/.cache almost every machine has, and the 0775 one a 002 umask
      // leaves -- while buying nothing: anything an attacker swaps in below is owned by them, and
      // the check on the directory itself refuses that. The launcher creates what is missing at
      // 0700, so what is asserted here is that a loose ancestor does not disable the cache.
      mkdirSync(join(home, '.cache'), { mode });
      chmodSync(join(home, '.cache'), mode);
      assert.equal(run(home).status, 0);
      assert.equal(cacheEntries(home).length, 1, 'a loose ancestor disabled the cache');
      // Accepted, and left as it was found: the launcher does not tighten a directory it shares.
      assert.equal(statSync(join(home, '.cache')).mode & 0o777, mode);
    });
  });
}

test('a cache directory whose parent is a symlink is refused', async () => {
  await withTempHome((home) => {
    // Recursive mkdir follows a link, so a link here would have the launcher create `compile` in
    // somebody else's tree and write half a megabyte of bytecode into it, with every check on
    // `compile` itself still passing: it would be ours, 0700 and a real directory.
    const elsewhere = join(home, 'elsewhere');
    mkdirSync(elsewhere);
    mkdirSync(join(home, '.cache'));
    symlinkSync(elsewhere, join(home, '.cache', 'oboete'));
    assert.equal(run(home).status, 0);
    assert.deepEqual(readdirSync(elsewhere, { recursive: true }), [], 'the launcher followed a symlink');
  });
});

test('a cache directory this run creates is 0700 and ~/.cache is left to the umask', async () => {
  await withTempHome((home) => {
    // ~/.cache belongs to every tool on the machine and this may be the first thing to create it,
    // so it gets whatever an ordinary mkdir would give it -- which under a 077 umask is 0700 too,
    // hence the control directory rather than a fixed mode to compare against.
    const control = join(home, 'control');
    mkdirSync(control);
    assert.equal(run(home).status, 0);
    assert.equal(statSync(join(home, '.cache')).mode & 0o777, statSync(control).mode & 0o777);
    assert.equal(statSync(join(home, '.cache', 'oboete')).mode & 0o777, 0o700);
    assert.equal(statSync(cacheDir(home)).mode & 0o777, 0o700);
  });
});

test('a cache directory that is a symlink is refused', async () => {
  await withTempHome((home) => {
    mkdirSync(join(home, '.cache', 'oboete'), { recursive: true });
    const target = join(home, 'elsewhere');
    mkdirSync(target);
    // Following it would give whoever planted the link a write primitive into its target: Node
    // creates the versioned directory and writes a half-megabyte blob into it.
    symlinkSync(target, cacheDir(home));
    assert.equal(run(home).status, 0);
    assert.deepEqual(readdirSync(target, { recursive: true }), [], 'the launcher followed a symlink');
  });
});

test('the engine is found through a bin symlink even with --preserve-symlinks-main', async () => {
  await withTempHome((home) => {
    // A global install runs the symlink npm puts in its bin directory. Under that flag
    // `import.meta.url` is the link, so a relative import would look for the engine beside it.
    const link = join(home, 'oboete');
    symlinkSync(bin, link);
    const result = spawnSync(process.execPath, ['--preserve-symlinks-main', link, '--version'], {
      encoding: 'utf8',
      env: { ...process.env, HOME: home, OBOETE_HOME: join(home, '.oboete') },
    });
    assert.equal(result.status, 0, result.stderr);
  });
});

test('a home that cannot hold a cache costs the cache, not the command', async () => {
  await withTempHome((home) => {
    // A regular file where a directory would have to be: mkdir fails with ENOTDIR for every uid,
    // where a mode-based denial would not apply to root and would skip the test in CI containers.
    const blocker = join(home, 'blocked');
    writeFileSync(blocker, '');
    const result = run(join(blocker, 'home'));
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stderr, '');
  });
});

test('setup writes the launcher into the hook commands, not the engine', async () => {
  await withTempHome((home) => {
    // An agent counts as installed when its home exists, so this needs no CLI on PATH -- and an
    // empty PATH keeps the run from spawning the real one where a developer has it (detect.ts).
    mkdirSync(join(home, '.claude'), { recursive: true });
    const emptyBin = join(home, 'empty-bin');
    mkdirSync(emptyBin, { recursive: true });
    const result = spawnSync(
      process.execPath,
      [bin, 'setup', '--agents', 'claude', '--provider', 'none', '--yes'],
      {
        encoding: 'utf8',
        env: { HOME: home, OBOETE_HOME: join(home, '.oboete'), PATH: emptyBin, NODE_ENV: 'test' },
      },
    );
    assert.equal(result.status, 0, result.stderr);
    const settings = readFileSync(join(home, '.claude', 'settings.json'), 'utf8');
    // The engine never enables the cache, so an installed hook that names it gets none of this.
    assert.ok(settings.includes('dist/oboete.mjs'), settings.slice(0, 400));
    assert.equal(settings.includes('engine.mjs'), false, 'a hook command names the engine');
  });
});
