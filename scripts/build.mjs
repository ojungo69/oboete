#!/usr/bin/env node
// Builds the engine bundle dist/oboete.mjs and compiles the tests to build/test/**/*.test.mjs.
// Bundle composition is security-owned (plan.md "Structure Decision", research R2): only the
// hook-path packages ride inside the engine file; the heavy runtime packages stay in node_modules
// and are imported lazily off the hook path (constitution principle II, amendment A1).
import { build } from 'esbuild';
import { chmodSync, existsSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
const pkg = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));

// Kept out of the bundle and loaded with dynamic import() by observe, view, mcp and setup.
const EXTERNAL = ['ai', '@ai-sdk/*', 'workers-ai-provider', 'hono', '@hono/*', 'preact', 'preact/*'];

const common = {
  bundle: true,
  platform: 'node',
  format: 'esm',
  target: 'node22',
  loader: { '.sql': 'text', '.md': 'text' },
  define: { OBOETE_VERSION: JSON.stringify(pkg.version) },
  legalComments: 'none',
  logLevel: 'warning',
};

// Bundled CommonJS dependencies call require() for Node built-ins; ESM output has no require.
const requireShim =
  "import { createRequire as __oboeteCreateRequire } from 'node:module';\n" +
  'const require = __oboeteCreateRequire(import.meta.url);\n';

for (const dir of ['dist', 'build']) rmSync(join(root, dir), { recursive: true, force: true });

await build({
  ...common,
  entryPoints: [join(root, 'src/cli.ts')],
  outfile: join(root, 'dist/oboete.mjs'),
  external: EXTERNAL,
  banner: { js: '#!/usr/bin/env node\n' + requireShim },
});
chmodSync(join(root, 'dist/oboete.mjs'), 0o755);

// The Pi extension is a second entry point rather than an export of the engine: it is imported into
// Pi's own process (src/pi-extension.ts, FR-007), so it must carry nothing of the hook path with it
// and the hook path must not carry it. Its only import is `node:child_process`; the loader written
// by src/setup/write-pi.ts imports `piExtension` from this file next to the engine bundle.
await build({
  ...common,
  entryPoints: [join(root, 'src/pi-extension.ts')],
  outfile: join(root, 'dist/pi-extension.mjs'),
  external: EXTERNAL,
});

// Viewer assets (src/viewer/app, Vite) are embedded here from T079 on.

const tests = [];
const walk = (dir) => {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) walk(path);
    else if (entry.name.endsWith('.test.ts')) tests.push(path);
  }
};
if (existsSync(join(root, 'test'))) walk(join(root, 'test'));

if (tests.length > 0) {
  await build({
    ...common,
    entryPoints: tests,
    outdir: join(root, 'build/test'),
    outbase: join(root, 'test'),
    outExtension: { '.js': '.mjs' },
    packages: 'external',
    banner: { js: requireShim },
    sourcemap: 'inline',
  });
}

console.log(`built dist/oboete.mjs and ${tests.length} test file(s)`);
