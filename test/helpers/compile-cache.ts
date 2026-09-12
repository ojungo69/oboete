import { existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

function repositoryRoot(): string {
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
 * Used by the suites that spawn the CLI under a time bound: `test/helpers/fault.ts` (every
 * `fault-*` suite), `test/e2e-hook.test.ts` and `test/e2e-inject.test.ts`. The unit batch spawns
 * cold, which costs it the same 35 ms per spawn and is bounded by nothing it asserts; give it this
 * directory too if that ever stops being true.
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
