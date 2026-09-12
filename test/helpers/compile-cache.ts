import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

/** The repository root, found by walking up to the `package.json`: the suites that spawn
 *  `dist/oboete.mjs` all need it, and this is the file they all import anyway. */
export function repositoryRoot(): string {
  let directory = fileURLToPath(new URL('.', import.meta.url));
  for (;;) {
    if (existsSync(join(directory, 'package.json'))) return directory;
    const parent = dirname(directory);
    if (parent === directory) throw new Error('the repository root must contain package.json');
    directory = parent;
  }
}

// One V8 compile cache for every CLI this suite spawns.
//
// The launcher keeps its compile cache inside `OBOETE_HOME` (issue #210) and every test gets a
// temporary one, so without this each spawn compiles the two-megabyte engine from source again.
// Measured on CI as a median of 246 ms against 215 ms over 48 hook invocations -- 35 ms, which is
// most of the headroom under the 300 ms capture budget, and it turned two suites red. The cold
// cache is an artefact of the harness and not of the product: a real installation's cache outlives
// its invocations, so sharing one directory measures the hook the way it actually runs.
//
// It is an environment variable rather than an argument to each spawn because that is what reaches
// all of them -- the unit batch, the ad-hoc spawn in `test/fault-pi.test.ts`, and anything added
// later, without each having to remember. `package.json` loads this file into the test runner with
// `--import`, so it is set before any test file starts; a file that imports this module for any
// other reason gets it too, which keeps a single file run straight from `node --test` measuring the
// same hook the suite does. Wiring three spawn sites instead was the first attempt and it left the
// unit batch cold, which is what used to warm the cache for the timed suites that follow it.
//
// `NODE_COMPILE_CACHE` wins over the launcher's own `enableCompileCache` call, the one thing
// contracts/injection-performance.md records the launcher cannot defend against. That is why this
// works, and why `test/unit/launcher.test.ts` deletes the variable: the directory the launcher
// chooses for itself is exactly what that suite is about. An operator's own choice of directory is
// left alone, but only a real one: `NODE_COMPILE_CACHE=` set to nothing, or to spaces, is how Node
// is told to write a cache into a directory named by the empty string or by whitespace, and the
// repository resolves such a value the same way `src/paths.ts` does -- trim it, and treat what is
// left of nothing as unset. `NODE_DISABLE_COMPILE_CACHE` is deleted rather than respected, because
// Node reads it during child bootstrap and it wins over the directory, and a suite that measures a
// hook with no compile cache is measuring a hook nobody runs.
export const SHARED_COMPILE_CACHE =
  process.env.NODE_COMPILE_CACHE?.trim() || join(repositoryRoot(), 'build', 'compile-cache');
delete process.env.NODE_DISABLE_COMPILE_CACHE;
process.env.NODE_COMPILE_CACHE = SHARED_COMPILE_CACHE;

/**
 * One throwaway run of `bundle`, so the run that compiles the engine is never a timed one.
 *
 * `npm test` warms the cache in the unit batch, but `.github/workflows/ci.yml` runs the e2e bundle
 * tests as a step of their own -- alone and uninstrumented, which is what makes their numbers worth
 * reading -- and there the first spawn finds an empty cache: 231.0 ms against 161.5 for the next
 * one, 260.4 against 195.7 on the run after that. The same spawn on a busier runner took 299.1 and
 * 304.9 ms and stored a partial row, which is what a capture that runs out of its 300 ms does. An
 * installed oboete pays that compile once, at install, and never inside a hook an agent is waiting
 * on.
 *
 * The assertion is the pin on the mechanism above it. Two of the three suites that share the cache
 * hold this module open only because they want something out of it, and a cache that is not set is
 * indistinguishable from one that is until a loaded runner turns it into a missed deadline in a
 * suite nobody changed. Failing here names the cause instead.
 */
export function warmCompileCache(bundle: string): void {
  assert.equal(
    process.env.NODE_COMPILE_CACHE,
    SHARED_COMPILE_CACHE,
    'the shared compile cache is not set: this suite would time a cold engine compile',
  );
  // A home of its own, thrown away again: the launcher makes `cache/compile` inside whatever
  // `OBOETE_HOME` names, and every other spawn in these suites is given a temporary one. What this
  // run fills is `NODE_COMPILE_CACHE`, which is set above and inherited, so the home it is handed
  // changes nothing about the warm-up and keeps the suite out of the developer's own `~/.oboete`.
  const home = mkdtempSync(join(tmpdir(), 'oboete-warm-'));
  try {
    spawnSync(process.execPath, [bundle, '--version'], {
      encoding: 'utf8',
      env: { ...process.env, OBOETE_HOME: home },
    });
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
}
