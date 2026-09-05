// Pi's wiring is one file oboete owns outright: the loader in Pi's global extension directory
// (spec FR-031 -- global, because a project-local `.pi/extensions` entry does not load until the
// project is trusted, and headless runs never ask). Pi needs no settings entry to enable it: every
// `*.js` in that directory is loaded (docs/research/oboete-contracts-2026-09-02.md, Pi section), so
// nothing of the developer's own configuration is edited here (FR-043) and the managed blocks of
// src/setup/managed-block.ts have nothing to hold. The write is still temporary file -> rename, so
// Pi never reads half a loader, and a file that does not carry the marker is never touched.
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';

import { ManagedFileError } from './managed-block.js';

/** The line that makes the loader identifiable as oboete's on the next setup or removal. */
export const PI_LOADER_MARKER = '// oboete:managed';

export type WritePiOptions = {
  /** Absolute paths: the node binary and the engine bundle the extension's children run. */
  node: string;
  bundle: string;
};

export type WriteResult = { files: string[] };

export function piLoaderPath(home: string): string {
  return join(home, '.pi', 'agent', 'extensions', 'oboete.js');
}

export function writePi(home: string, options: WritePiOptions): WriteResult {
  const loader = piLoaderPath(home);
  requireOwned(loader);
  // The extension bundle is built next to the engine bundle (scripts/build.mjs), and Pi imports it
  // as a URL so that a Windows path is a valid specifier.
  const extension = pathToFileURL(join(dirname(options.bundle), 'pi-extension.mjs')).href;
  const content =
    `${PI_LOADER_MARKER} written by \`oboete setup\`; \`oboete setup --remove\` deletes it.\n` +
    `import { piExtension } from ${JSON.stringify(extension)};\n` +
    `export default (pi) => piExtension(pi, { node: ${JSON.stringify(options.node)}, bundle: ${JSON.stringify(options.bundle)} });\n`;

  mkdirSync(dirname(loader), { recursive: true });
  const temporary = `${loader}.oboete-tmp-${process.pid}`;
  // `wx`: an exclusive create, so a link pre-placed at this predictable name is refused rather than
  // followed (the rule src/setup/managed-block.ts states for the foreign files).
  writeFileSync(temporary, content, { mode: 0o600, flag: 'wx' });
  renameSync(temporary, loader);
  return { files: [loader] };
}

export function removePi(home: string): void {
  const loader = piLoaderPath(home);
  requireOwned(loader);
  rmSync(loader, { force: true });
}

function requireOwned(loader: string): void {
  if (!existsSync(loader)) return;
  if (readFileSync(loader, 'utf8').startsWith(PI_LOADER_MARKER)) return;
  throw new ManagedFileError(
    `${loader} was not written by oboete (it does not start with "${PI_LOADER_MARKER}"); move it aside to let setup write the loader`,
    'unmarked_handler',
    loader,
  );
}
