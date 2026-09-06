#!/usr/bin/env node
// Thin launcher for T068. Builds nothing; the engine bundle must already exist.
import { spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '../..');
const args = process.argv.slice(2);
const keep = args.includes('--keep');
const hasHome = args.includes('--home');
const home = hasHome ? null : mkdtempSync(join(tmpdir(), 'oboete-replay-home-'));
const forwarded = home === null ? args : ['--home', home, ...args];
const result = spawnSync(
  process.execPath,
  [join(root, 'dist', 'oboete.mjs'), 'fixture', 'replay', ...forwarded],
  { stdio: 'inherit', env: { ...process.env, ...(home === null ? {} : { OBOETE_HOME: home }) } },
);
if (home !== null && !keep) rmSync(home, { recursive: true, force: true });
process.exit(result.status ?? 1);
