import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

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

test('checkPackFiles requires the four dist files and rejects src/test/build/legacy', () => {
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
