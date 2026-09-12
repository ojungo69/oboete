import { existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

/** The repository root, found by walking up to the `package.json`: the three suites that spawn
 *  `dist/oboete.mjs` all need it, and this is the file all three already import. */
export function repositoryRoot(): string {
  let directory = fileURLToPath(new URL('.', import.meta.url));
  for (;;) {
    if (existsSync(join(directory, 'package.json'))) return directory;
    const parent = dirname(directory);
    if (parent === directory) throw new Error('the repository root must contain package.json');
    directory = parent;
  }
}

/**
 * One V8 compile cache for every CLI this suite spawns.
 *
 * The launcher keeps its compile cache inside `OBOETE_HOME` (issue #210) and every test gets a
 * temporary one, so without this each spawn compiles the two-megabyte engine from source again.
 * Measured on CI as a median of 246 ms against 215 ms over 48 hook invocations -- 35 ms, which is
 * most of the headroom under the 300 ms capture budget, and it turned two suites red. The cold
 * cache is an artefact of the harness and not of the product: a real installation's cache persists
 * between invocations, so sharing one directory is what measures the hook the way it actually runs.
 *
 * Setting the variable rather than passing it per spawn is what reaches every one of them: the unit
 * batch, the ad-hoc spawn in `test/fault-pi.test.ts`, and anything added later, without each having
 * to remember. A test file that imports this module for any reason gets it too, which is what makes
 * a single file run straight from `node --test` measure the same hook as the suite does.
 *
 * `NODE_COMPILE_CACHE` wins over the launcher's own `enableCompileCache` call, which is the one
 * thing contracts/injection-performance.md records the launcher cannot defend against. That is why
 * this works, and why `test/unit/launcher.test.ts` deletes the variable: the directory the launcher
 * chooses for itself is exactly what that suite is about.
 *
 * `scripts/build.mjs` removes `build/`, so a rebuild starts the suite on an empty cache -- the
 * first spawn pays the compile once and the rest read it back, which is what CI did before the
 * cache moved under the data directory.
 */
export const SHARED_COMPILE_CACHE = join(repositoryRoot(), 'build', 'compile-cache');

// `package.json` loads this file into the test runner itself with `--import`, so the variable is in
// the environment every test file and every CLI it spawns inherits -- including the unit batch,
// which spawns the bundle a few dozen times and is what used to leave the runner's cache warm for
// the timed suites that run after it. Without that the first spawn of the serial batch pays the
// whole compile and lands on the 300 ms budget: `e2e-hook.test.ts:110` failed that way on CI, with
// a partial row and a null `content`, on both duplicate runs. `??=` so an operator who points the
// variable somewhere else keeps it, and so the suites below that set it explicitly -- for a single
// file run straight from `node --test` -- agree rather than fight.
process.env.NODE_COMPILE_CACHE ??= SHARED_COMPILE_CACHE;
