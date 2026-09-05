// What `oboete setup` puts in Pi's global extension directory. Sources: spec FR-031 (a user-global
// location that needs no per-project trust), contracts/agents.md Pi row ("`~/.pi/agent/extensions/
// oboete.js` loader importing `piExtension` from the bundle"), FR-043 (nothing but hook wiring is
// touched). The loader is the one file oboete owns outright, so it is written whole rather than as
// a managed block inside a developer's file (src/setup/managed-block.ts covers those).
import assert from 'node:assert/strict';
import { existsSync, mkdirSync, readFileSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { ManagedFileError } from '../../src/setup/managed-block.js';
import { PI_LOADER_MARKER, piLoaderPath, removePi, writePi } from '../../src/setup/write-pi.js';
import { withTempHome } from '../helpers/home.js';

function repositoryRoot(): string {
  let directory = fileURLToPath(new URL('.', import.meta.url));
  while (!existsSync(join(directory, 'package.json'))) {
    const parent = dirname(directory);
    assert.notEqual(parent, directory, 'the repository root must contain package.json');
    directory = parent;
  }
  return directory;
}

const ROOT = repositoryRoot();
const BUNDLE = join(ROOT, 'dist', 'oboete.mjs');
const NODE = process.execPath;

test('writePi writes the loader into Pi’s global extension directory and repeats byte-identically', async () => {
  await withTempHome(async (home) => {
    const first = writePi(home, { node: NODE, bundle: BUNDLE });
    const loader = join(home, '.pi', 'agent', 'extensions', 'oboete.js');

    assert.deepEqual(first.files, [loader]);
    assert.equal(piLoaderPath(home), loader);
    const written = readFileSync(loader, 'utf8');
    assert.equal(statSync(loader).mode & 0o777, 0o600);
    assert.equal(written.trimEnd().split('\n').length, 3, 'the loader is three lines');
    assert.equal(PI_LOADER_MARKER, '// oboete:managed');
    assert.ok(written.startsWith(PI_LOADER_MARKER), 'removal identifies the file by this marker');
    assert.ok(
      written.includes(pathToFileURL(join(ROOT, 'dist', 'pi-extension.mjs')).href),
      'the loader imports the extension built next to the engine bundle',
    );
    assert.ok(written.includes(JSON.stringify(BUNDLE)) && written.includes(JSON.stringify(NODE)));

    writePi(home, { node: NODE, bundle: BUNDLE });
    assert.equal(readFileSync(loader, 'utf8'), written, 'setup is repeatable (FR-031)');
  });
});

test('the loader registers oboete’s handlers when Pi imports it', async () => {
  await withTempHome(async (home) => {
    writePi(home, { node: NODE, bundle: BUNDLE });
    const module = (await import(pathToFileURL(piLoaderPath(home)).href)) as {
      default: (pi: unknown) => unknown;
    };

    const events: string[] = [];
    const tools: string[] = [];
    module.default({
      on: (event: string) => events.push(event),
      registerTool: (tool: { name: string }) => tools.push(tool.name),
    });

    assert.ok(events.includes('before_agent_start'), 'the injection hook is wired');
    assert.ok(events.includes('session_start') && events.includes('input'));
    assert.deepEqual(tools.sort(), ['oboete_get', 'oboete_search', 'oboete_timeline']);
  });
});

test('removePi deletes the loader and leaves the developer’s own extensions alone', async () => {
  await withTempHome(async (home) => {
    writePi(home, { node: NODE, bundle: BUNDLE });
    const neighbour = join(home, '.pi', 'agent', 'extensions', 'mine.js');
    writeFileSync(neighbour, 'export default () => {};\n');

    removePi(home);
    assert.equal(existsSync(piLoaderPath(home)), false);
    assert.equal(readFileSync(neighbour, 'utf8'), 'export default () => {};\n');

    removePi(home); // A second removal, and a machine that never ran setup, are both no-ops.
    removePi(join(home, 'never-set-up'));
  });
});

test('a loader oboete did not write is never overwritten or deleted', async () => {
  await withTempHome(async (home) => {
    const loader = piLoaderPath(home);
    mkdirSync(dirname(loader), { recursive: true });
    const theirs = 'export default (pi) => pi.on("input", () => {});\n';
    writeFileSync(loader, theirs);

    const unmarked = (error: unknown): boolean =>
      error instanceof ManagedFileError && error.code === 'unmarked_handler';
    assert.throws(() => writePi(home, { node: NODE, bundle: BUNDLE }), unmarked);
    assert.throws(() => removePi(home), unmarked);
    assert.equal(readFileSync(loader, 'utf8'), theirs);
  });
});

test('a symbolic link left at the loader’s temporary path is refused instead of written through', async () => {
  await withTempHome(async (home) => {
    const victim = join(home, 'victim.js');
    writeFileSync(victim, 'stays = true\n');
    const loader = piLoaderPath(home);
    mkdirSync(dirname(loader), { recursive: true });
    symlinkSync(victim, `${loader}.oboete-tmp-${process.pid}`);

    assert.throws(() => writePi(home, { node: NODE, bundle: BUNDLE }));
    assert.equal(readFileSync(victim, 'utf8'), 'stays = true\n', 'the link is never followed');
    assert.equal(existsSync(loader), false);
  });
});
