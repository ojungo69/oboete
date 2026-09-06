import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

/**
 * Runs `fn` with `OBOETE_HOME` pointing at a fresh temporary directory, so a test never touches
 * the real `~/.oboete` (docs/dev/conventions.md "Tests").
 */
export async function withTempHome<T>(fn: (home: string) => T | Promise<T>): Promise<T> {
  const home = mkdtempSync(join(tmpdir(), 'oboete-home-'));
  const previous = process.env.OBOETE_HOME;
  process.env.OBOETE_HOME = home;
  try {
    return await fn(home);
  } finally {
    if (previous === undefined) delete process.env.OBOETE_HOME;
    else process.env.OBOETE_HOME = previous;
    rmSync(home, { recursive: true, force: true });
  }
}
