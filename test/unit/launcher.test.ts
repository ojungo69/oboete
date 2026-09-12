// dist/oboete.mjs is a launcher (src/launcher.mjs) that turns the V8 compile cache on before the
// engine bundle is compiled, issue #210. Timing is not pinned here -- that belongs to whatever load
// the machine is under. What is pinned is everything the speed-up depends on, in eighteen tests:
// the entry file stays small, the engine is a separate file, the cache lives in the one data
// directory and follows OBOETE_HOME wherever it points, it is owner-only and is read back on the
// next run, a directory this user does not exclusively own -- or that anyone else can enter, or
// that is reached through a symlink -- is refused while a loose one of the user's own is not, a
// home that cannot hold a cache costs the cache and not the command, an engine that cannot be
// imported costs the three agent-invoked commands nothing and every other command everything, and
// the installer writes the launcher rather than the engine. The contract's "Pinned by" paragraph
// (specs/009-memory-core/contracts/injection-performance.md) is the enumeration.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  chmodSync,
  copyFileSync,
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

/** The data directory every run below is given, and the one `run` names in `OBOETE_HOME`. */
function dataHome(home: string): string {
  return join(home, '.oboete');
}

/** `dist/oboete.mjs` in an environment of this test's own, with the two variables that would
 *  decide the compile cache before the launcher can -- one pointing it elsewhere, the other
 *  switching it off -- removed, so a developer's shell cannot fail these assertions. */
function spawn(env: NodeJS.ProcessEnv, args: string[], cwd?: string) {
  delete env.NODE_COMPILE_CACHE;
  delete env.NODE_DISABLE_COMPILE_CACHE;
  return spawnSync(process.execPath, [bin, ...args], { encoding: 'utf8', env, cwd });
}

function run(home: string, args: string[] = ['--version']) {
  return spawn({ ...process.env, HOME: home, OBOETE_HOME: dataHome(home) }, args);
}

function cacheDir(home: string): string {
  return join(dataHome(home), 'cache', 'compile');
}

/** Every cache file with its mtime. A cache that is written but never read back rewrites the same
 *  file names (the key is path plus source hash), so paths alone cannot tell a hit from a miss. */
function cacheEntries(dir: string): { path: string; mtimeMs: number }[] {
  const entries = readdirSync(dir, { recursive: true, withFileTypes: true });
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
  // compile, so the bound is on the code: adding a paragraph must not fail the build. The figure
  // is a ceiling on this file's job, not a budget that was measured: it went from 1536 to 2048
  // when the launcher took on resolving the data directory and answering for a missing engine.
  const code = launcher
    .split('\n')
    .filter((line) => line.trim() !== '' && !/^\s*(\/\/|\*|\/\*)/u.test(line))
    .join('\n');
  assert.ok(code.length < 2048, `the launcher's code is ${code.length} bytes`);
  assert.ok(statSync(join(root, 'dist/engine.mjs')).isFile(), 'the engine is a file of its own');
});

test('a run fills an owner-only compile cache and the next run reads it back', async () => {
  await withTempHome((home) => {
    assert.equal(run(home).status, 0);
    assert.equal(statSync(cacheDir(home)).mode & 0o777, 0o700, 'the compile cache is owner-only');
    const first = cacheEntries(cacheDir(home));
    // Not an exact count: how many modules one command compiles is the engine's business.
    assert.ok(first.length >= 1, 'the compile cache is empty after a run');
    assert.equal(run(home).status, 0);
    assert.deepEqual(cacheEntries(cacheDir(home)), first, 'the second run rewrote the entry');
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

test('the cache follows OBOETE_HOME and leaves nothing outside it', async () => {
  await withTempHome((home) => {
    // CONSTITUTION.md Principle VI: one data directory, relocatable through OBOETE_HOME. A cache
    // under ~/.cache would survive relocation and outlive the tree the developer meant to isolate.
    const elsewhere = join(home, 'relocated');
    const result = spawn({ ...process.env, HOME: home, OBOETE_HOME: elsewhere }, ['--version']);
    assert.equal(result.status, 0, result.stderr);
    assert.ok(cacheEntries(join(elsewhere, 'cache', 'compile')).length >= 1, 'the relocated home has no cache');
    assert.deepEqual(readdirSync(home).sort(), ['relocated'], 'the run wrote outside OBOETE_HOME');
  });
});

test('a relative OBOETE_HOME is anchored to the home directory, as src/paths.ts anchors it', async () => {
  await withTempHome((home) => {
    // The launcher cannot import resolveHome -- importing the engine is the cost it exists to
    // avoid -- so the rule is written twice and this is what keeps the second copy honest.
    // The cwd is the repository, which is not the home directory: a rule that resolved against it
    // would put the cache somewhere else entirely.
    const env = { ...process.env, HOME: home, OBOETE_HOME: 'relative/home' };
    const result = spawn(env, ['--version'], root);
    assert.equal(result.status, 0, result.stderr);
    const cache = join(home, 'relative', 'home', 'cache', 'compile');
    assert.ok(cacheEntries(cache).length >= 1, 'a relative home was resolved against the cwd');
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
      assert.deepEqual(cacheEntries(cacheDir(home)), [], 'the launcher used a cache directory others can enter');
    });
  });
}

for (const mode of [0o755, 0o775, 0o777]) {
  test(`a data directory above the cache at ${mode.toString(8)} is accepted`, async () => {
    await withTempHome((home) => {
      // The rule stops at the two directories the launcher owns. A loose data directory is a
      // problem for `oboete doctor` to report -- memory.db is in there -- and turning the cache
      // off would neither fix it nor say so. Nothing an attacker swaps in below is theirs to keep:
      // the check on `cache` and on `compile` refuses it. The launcher creates what is missing at
      // 0700, so what is asserted here is that a loose ancestor does not disable the cache.
      mkdirSync(dataHome(home), { recursive: true });
      chmodSync(dataHome(home), mode);
      assert.equal(run(home).status, 0);
      assert.ok(cacheEntries(cacheDir(home)).length >= 1, 'a loose ancestor disabled the cache');
      // Accepted, and left as it was found: the launcher does not tighten a directory it shares.
      assert.equal(statSync(dataHome(home)).mode & 0o777, mode);
    });
  });
}

test('a cache directory left loose by a restored backup is still accepted', async () => {
  await withTempHome((home) => {
    // What `cp -r` and an archive unpacked without modes leave behind: 0777 minus the umask on a
    // directory the user still owns. Asking it for a mode now refuses it and turns the cache off
    // without a word, on exactly the machines where it already worked. Nothing else here would
    // notice: the launcher makes the directory itself at 0700, which passes any mode check at any
    // umask, so every other pin stays green (issue #210 shipped that bug twice).
    const parent = join(dataHome(home), 'cache');
    mkdirSync(parent, { recursive: true });
    chmodSync(parent, 0o775);
    assert.equal(run(home).status, 0);
    assert.ok(cacheEntries(cacheDir(home)).length >= 1, 'a loose cache directory disabled the cache');
    assert.equal(statSync(parent).mode & 0o777, 0o775, 'it was tightened');
  });
});

test('a cache directory whose parent is a symlink is refused', async () => {
  await withTempHome((home) => {
    // Recursive mkdir follows a link, so a link here would have the launcher create `compile` in
    // somebody else's tree and write half a megabyte of bytecode into it, with every check on
    // `compile` itself still passing: it would be ours, 0700 and a real directory.
    const elsewhere = join(home, 'elsewhere');
    mkdirSync(elsewhere);
    mkdirSync(dataHome(home));
    symlinkSync(elsewhere, join(dataHome(home), 'cache'));
    assert.equal(run(home).status, 0);
    assert.deepEqual(readdirSync(elsewhere, { recursive: true }), [], 'the launcher followed a symlink');
  });
});

test('every directory this run creates is owner-only', async () => {
  await withTempHome((home) => {
    // Including the data directory itself: a hook can run before `oboete setup` ever has, and this
    // is then the thing that creates the directory memory.db will later live in.
    assert.equal(run(home).status, 0);
    assert.equal(statSync(dataHome(home)).mode & 0o777, 0o700);
    assert.equal(statSync(join(dataHome(home), 'cache')).mode & 0o777, 0o700);
    assert.equal(statSync(cacheDir(home)).mode & 0o777, 0o700);
  });
});

test('a cache directory that is a symlink is refused', async () => {
  await withTempHome((home) => {
    mkdirSync(join(dataHome(home), 'cache'), { recursive: true });
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
      env: { ...process.env, HOME: home, OBOETE_HOME: dataHome(home) },
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

/** A launcher with no engine beside it: what a half-finished install or a partial copy looks like. */
function withoutEngine(home: string): string {
  const install = join(home, 'install');
  mkdirSync(install);
  const copy = join(install, 'oboete.mjs');
  copyFileSync(bin, copy);
  return copy;
}

test('an engine that will not import costs the agent-invoked commands nothing', async () => {
  await withTempHome((home) => {
    // Splitting one bundle into two put a new failure ahead of everything the engine does about
    // its own: the import itself. contracts/cli.md gives `hook`, `capture` and `inject` exit 0
    // whatever happens, so the launcher has to answer for that here rather than let Node print a
    // stack over an agent's session.
    const env = { ...process.env, HOME: home, OBOETE_HOME: dataHome(home) };
    const result = spawnSync(process.execPath, [withoutEngine(home), 'hook', '--agent', 'claude'], {
      encoding: 'utf8',
      env,
      input: '{}',
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stderr, '', 'a broken install printed over the session');
    // Silent is not invisible: the one place a developer looks says which command and which error.
    const log = readFileSync(join(dataHome(home), 'logs', 'hook.log'), 'utf8');
    assert.match(log, /error engine unavailable command=hook reason=ERR_MODULE_NOT_FOUND/u);
  });
});

test('an engine that will not import fails every other command loudly', async () => {
  await withTempHome((home) => {
    // The other half of the same rule. A `--version` that quietly exits 0 with no output would
    // make a broken install indistinguishable from a working one for anyone checking by hand.
    const result = spawnSync(process.execPath, [withoutEngine(home), '--version'], {
      encoding: 'utf8',
      env: { ...process.env, HOME: home, OBOETE_HOME: dataHome(home) },
    });
    assert.notEqual(result.status, 0, 'a missing engine exited 0 for a human-facing command');
    assert.match(result.stderr, /ERR_MODULE_NOT_FOUND/u);
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
        env: { HOME: home, OBOETE_HOME: dataHome(home), PATH: emptyBin, NODE_ENV: 'test' },
      },
    );
    assert.equal(result.status, 0, result.stderr);
    const settings = readFileSync(join(home, '.claude', 'settings.json'), 'utf8');
    // The engine never enables the cache, so an installed hook that names it gets none of this.
    assert.ok(settings.includes('dist/oboete.mjs'), settings.slice(0, 400));
    assert.equal(settings.includes('engine.mjs'), false, 'a hook command names the engine');
  });
});
