import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

import {
  LIMIT_BYTES,
  REQUIRED_PACK_FILES,
  checkPackFiles,
  exceedsSizeLimit,
  installedSizeLine,
  sumInstalledBytes,
} from './pack-check.mjs';

function tempTree() {
  return mkdtempSync(join(tmpdir(), 'oboete-pack-check-test-'));
}

test('sumInstalledBytes adds regular files and does not follow symlinks', () => {
  const dir = tempTree();
  try {
    writeFileSync(join(dir, 'a.txt'), Buffer.alloc(10));
    mkdirSync(join(dir, 'sub'));
    writeFileSync(join(dir, 'sub', 'b.txt'), Buffer.alloc(20));
    symlinkSync(join(dir, 'a.txt'), join(dir, 'link.txt'));
    symlinkSync(join(dir, 'sub'), join(dir, 'sublink'));
    assert.equal(sumInstalledBytes(dir), 30);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test('size at the 30 MB limit passes; one byte over fails', () => {
  assert.equal(exceedsSizeLimit(LIMIT_BYTES), false);
  assert.equal(exceedsSizeLimit(LIMIT_BYTES + 1), true);
  assert.equal(exceedsSizeLimit(0), false);
  assert.equal(installedSizeLine(LIMIT_BYTES), 'installed size: 30.000 MB (limit 30 MB)');
  assert.equal(installedSizeLine(30_568_448), 'installed size: 29.152 MB (limit 30 MB)');
});

test('checkPackFiles requires every dist file and rejects src/test/build/legacy', () => {
  const required = REQUIRED_PACK_FILES.map((path) => ({ path }));
  assert.deepEqual(checkPackFiles(required), { missing: [], forbidden: [], ok: true });

  const missing = checkPackFiles([{ path: 'package.json' }]);
  assert.equal(missing.ok, false);
  assert.deepEqual(missing.missing, REQUIRED_PACK_FILES);
  assert.deepEqual(missing.forbidden, []);

  const forbidden = checkPackFiles([
    ...required,
    { path: 'src/cli.ts' },
    { path: 'test/unit/x.test.ts' },
    { path: 'build/test/x.mjs' },
    { path: 'legacy/README.md' },
  ]);
  assert.equal(forbidden.ok, false);
  assert.deepEqual(forbidden.missing, []);
  assert.deepEqual(forbidden.forbidden, [
    'src/cli.ts',
    'test/unit/x.test.ts',
    'build/test/x.mjs',
    'legacy/README.md',
  ]);
});

for (const { name, files, message } of [
  ...REQUIRED_PACK_FILES.map((path) => ({
    name: `a missing ${path}`,
    files: REQUIRED_PACK_FILES.filter((other) => other !== path),
    message: `FAIL: tarball missing ${path}`,
  })),
  {
    name: 'a forbidden top-level path',
    files: [...REQUIRED_PACK_FILES, 'src/cli.ts'],
    message: 'FAIL: tarball contains src/cli.ts',
  },
]) {
  test(`pack-check exits nonzero for ${name}`, (t) => {
    const dir = tempTree();
    t.after(() => rmSync(dir, { recursive: true, force: true }));
    const stub = `
      import childProcess from 'node:child_process';
      import { syncBuiltinESMExports } from 'node:module';
      childProcess.spawnSync = (command, args) => {
        if (command === 'cp') return { status: 1 };
        if (command === 'npm' && args[0] === 'run' && args[1] === 'build') return { status: 0 };
        if (command === 'npm' && args[0] === 'pack') {
          return { status: 0, stdout: ${JSON.stringify(JSON.stringify([{ files }]))} };
        }
        throw new Error('unexpected subprocess: ' + command + ' ' + args.join(' '));
      };
      syncBuiltinESMExports();
    `;
    const result = spawnSync(process.execPath, [
      '--import', `data:text/javascript,${encodeURIComponent(stub)}`,
      fileURLToPath(new URL('./pack-check.mjs', import.meta.url)),
    ], { encoding: 'utf8', env: { ...process.env, TMPDIR: dir }, timeout: 30_000 });

    assert.equal(result.error, undefined);
    assert.equal(result.stderr.trim(), message);
    assert.equal(result.status, 1);
    assert.deepEqual(readdirSync(dir), [], 'the temporary pack directory is removed');
  });
}
