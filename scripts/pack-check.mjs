#!/usr/bin/env node
// npm pack the current tree into an empty prefix and gate installed size at 30 MB.
// Build first: `npm pack` ships whatever is already in dist/ (no prepack script).
import { spawnSync } from 'node:child_process';
import {
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  realpathSync,
  rmSync,
} from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('..', import.meta.url));
const NPM_TIMEOUT_MS = 10 * 60 * 1000;

export const LIMIT_BYTES = 30 * 1024 * 1024;
export const REQUIRED_PACK_FILES = [
  'dist/oboete.mjs',
  'dist/engine.mjs',
  'dist/pi-extension.mjs',
  'dist/viewer/app.js',
  'dist/viewer/app.css',
];
const FORBIDDEN_TOP = ['src', 'test', 'build', 'legacy'];

export function sumInstalledBytes(root) {
  let total = 0;
  const walk = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isSymbolicLink()) continue;
      if (entry.isDirectory()) walk(path);
      else if (entry.isFile()) total += lstatSync(path).size;
    }
  };
  walk(root);
  return total;
}

export function exceedsSizeLimit(bytes, limit = LIMIT_BYTES) {
  return bytes > limit;
}

export function installedSizeLine(bytes) {
  return `installed size: ${(bytes / (1024 * 1024)).toFixed(3)} MB (limit 30 MB)`;
}

export function checkPackFiles(files) {
  const paths = files.map((file) => {
    const raw = typeof file === 'string' ? file : file.path;
    return String(raw).replaceAll('\\', '/');
  });
  const missing = REQUIRED_PACK_FILES.filter((required) => !paths.includes(required));
  const forbidden = paths.filter((path) =>
    FORBIDDEN_TOP.some((top) => path === top || path.startsWith(`${top}/`)),
  );
  return { missing, forbidden, ok: missing.length === 0 && forbidden.length === 0 };
}

function packManifest(json) {
  if (Array.isArray(json)) {
    if (json.length < 1) throw new Error('npm pack --json returned an empty array');
    return json[0];
  }
  if (json && typeof json === 'object' && Array.isArray(json.files)) return json;
  if (json && typeof json === 'object') {
    const values = Object.values(json);
    if (values.length === 1 && values[0] && Array.isArray(values[0].files)) return values[0];
  }
  throw new Error('npm pack --json did not include a files array');
}

function parsePackJson(stdout) {
  const trimmed = stdout.trim();
  try {
    return JSON.parse(trimmed);
  } catch {
    const start = Math.min(
      ...['{', '['].map((token) => {
        const at = trimmed.indexOf(token);
        return at < 0 ? Number.POSITIVE_INFINITY : at;
      }),
    );
    if (!Number.isFinite(start)) throw new Error('npm pack --json produced no JSON');
    return JSON.parse(trimmed.slice(start));
  }
}

function npmEnv(cacheDir, extra = {}) {
  const env = { ...process.env, npm_config_cache: cacheDir, ...extra };
  delete env.npm_config_prefix;
  delete env.npm_config_global;
  delete env.PREFIX;
  return env;
}

function runNpm(args, { cwd = ROOT, env, timeout = NPM_TIMEOUT_MS, stdio } = {}) {
  const result = spawnSync('npm', args, {
    cwd,
    env,
    encoding: 'utf8',
    timeout,
    maxBuffer: 16 * 1024 * 1024,
    stdio,
  });
  if (result.error) throw new Error(`npm ${args.join(' ')}: ${result.error.message}`);
  if (result.status !== 0) {
    const detail = (result.stderr || result.stdout || `exit ${String(result.status)}`).trim();
    throw new Error(`npm ${args.join(' ')} failed: ${detail}`);
  }
  return result;
}

// Isolated cache so a previously packed oboete in ~/.npm cannot satisfy the install.
// Seed content-addressed blobs (T015) when present: this host has blocked registry access before.
function seedCache(cacheDir) {
  const src = join(homedir(), '.npm', '_cacache');
  const content = join(src, 'content-v2');
  const index = join(src, 'index-v5');
  if (!existsSync(content) || !existsSync(index)) return false;
  const dest = join(cacheDir, '_cacache');
  mkdirSync(join(dest, 'tmp'), { recursive: true });
  const blobs = spawnSync('cp', ['-as', content, join(dest, 'content-v2')], { encoding: 'utf8' });
  if (blobs.status !== 0) return false;
  const idx = spawnSync('cp', ['-a', index, join(dest, 'index-v5')], { encoding: 'utf8' });
  return idx.status === 0;
}

function isDirectInvocation(argv1, moduleUrl) {
  if (!argv1) return false;
  try {
    return realpathSync(argv1) === realpathSync(fileURLToPath(moduleUrl));
  } catch {
    return true;
  }
}

function fail(message) {
  console.error(message);
  return true;
}

function packAndCheck(packDir, env) {
  runNpm(['run', 'build'], { env, stdio: 'inherit' });

  const packed = runNpm(['pack', '--json', `--pack-destination=${packDir}`], { env });
  const manifest = packManifest(parsePackJson(packed.stdout));
  const filesCheck = checkPackFiles(manifest.files ?? []);
  let failed = false;
  if (filesCheck.missing.length > 0) {
    failed = fail(`FAIL: tarball missing ${filesCheck.missing.join(', ')}`);
  }
  if (filesCheck.forbidden.length > 0) {
    failed = fail(`FAIL: tarball contains ${filesCheck.forbidden.join(', ')}`);
  }
  if (failed) return null;

  const filename = manifest.filename;
  if (!filename) throw new Error('npm pack --json omitted filename');
  const tarball = join(packDir, filename);
  if (!existsSync(tarball)) throw new Error(`tarball not written: ${tarball}`);
  return { filename, tarball };
}

function installAndVerify({ filename, tarball }, prefix, env) {
  runNpm(['install', '-g', `--prefix=${prefix}`, tarball], { env });

  const installed = join(prefix, 'lib', 'node_modules', 'oboete');
  if (!existsSync(installed)) throw new Error(`install missing ${installed}`);

  const bytes = sumInstalledBytes(installed);
  console.log(installedSizeLine(bytes));
  let failed = false;
  if (exceedsSizeLimit(bytes)) {
    failed = fail(`FAIL: installed size ${bytes} bytes exceeds ${LIMIT_BYTES} bytes`);
  }
  console.log(`tarball: ${filename}`);

  const bin = join(prefix, 'bin', 'oboete');
  if (!existsSync(bin)) throw new Error(`installed bin missing: ${bin}`);
  const version = spawnSync(bin, ['--version'], {
    encoding: 'utf8',
    timeout: 30_000,
    env: process.env,
  });
  if (version.error) throw new Error(`oboete --version: ${version.error.message}`);
  if (version.status !== 0) {
    const detail = (version.stderr || version.stdout || `exit ${String(version.status)}`).trim();
    throw new Error(`oboete --version failed: ${detail}`);
  }
  const printed = version.stdout.trim();
  if (!printed) throw new Error('oboete --version printed nothing');
  console.log(`oboete --version: ${printed}`);
  return failed;
}

function main() {
  let tmp;
  let failed = false;
  try {
    tmp = mkdtempSync(join(tmpdir(), 'oboete-pack-check-'));
    const cacheDir = join(tmp, 'npm-cache');
    const packDir = join(tmp, 'pack');
    const prefix = join(tmp, 'prefix');
    mkdirSync(cacheDir);
    mkdirSync(packDir);
    mkdirSync(prefix);
    const seeded = seedCache(cacheDir);
    const env = npmEnv(cacheDir, seeded ? { npm_config_prefer_offline: 'true' } : {});

    const packed = packAndCheck(packDir, env);
    failed = packed === null || installAndVerify(packed, prefix, env);
  } catch (error) {
    failed = fail(`FAIL: ${error instanceof Error ? error.message : String(error)}`);
  } finally {
    if (tmp !== undefined) rmSync(tmp, { recursive: true, force: true });
  }
  if (failed) process.exitCode = 1;
}

if (isDirectInvocation(process.argv[1], import.meta.url)) main();
