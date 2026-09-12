#!/usr/bin/env node
// The published entry point. It is not the program: dist/engine.mjs is, and this file exists only
// to turn Node's V8 compile cache on before the engine is compiled. A single-file bundle cannot do
// that for itself, because Node compiles an entry file before any statement in it runs -- and the
// capture hook is a cold process that paid a full parse and compile of the whole bundle on every
// invocation (issue #210). Keep it small: it is itself compiled uncached, every time.
//
// The cache directory is deliberately not OBOETE_HOME (the cache belongs to the build, not to a
// data directory, and the fault harness gives every scenario a fresh home) and deliberately not
// Node's own default of /tmp/node-compile-cache, which is shared by every user on the machine.
// V8 does not authenticate cache entries, so a directory somebody else can write to is a place to
// plant bytecode that this process will execute.
import { lstatSync, mkdirSync, realpathSync } from 'node:fs';
import { enableCompileCache } from 'node:module';
import { homedir } from 'node:os';
import { dirname, isAbsolute, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

/** True when `path` is a real directory of this user's and nobody else has any of `denied`'s bits.
 *  `mkdirSync` leaves an existing directory's mode and owner alone and follows a symlink, so what
 *  came back is checked rather than assumed. Refusing beats correcting: a chmod would land on the
 *  target of a planted symlink. */
function ours(path, denied) {
  const found = lstatSync(path);
  if (found.isSymbolicLink() || !found.isDirectory()) return false;
  const uid = process.getuid?.();
  // Windows has neither, so the symlink and directory checks above are the whole test there.
  if (uid === undefined) return true;
  return found.uid === uid && (found.mode & denied) === 0;
}

try {
  const base = process.env.XDG_CACHE_HOME;
  const root = base !== undefined && isAbsolute(base) ? base : join(homedir(), '.cache');
  const parent = join(root, 'oboete');
  const cache = join(parent, 'compile');
  // `~/.cache` is left at whatever mode the umask gives it, because this may be the first thing on
  // the machine to create it and that directory belongs to every tool. The two below it are ours.
  mkdirSync(root, { recursive: true });
  mkdirSync(parent, { recursive: true, mode: 0o700 });
  // The parent is checked before `compile` is created, not after: recursive mkdir follows a link,
  // so a link planted here would otherwise have us create `compile` in somebody else's tree and
  // write bytecode into it while every check on `compile` itself still passed -- it would be ours,
  // 0700 and a real directory. It only has to be ours and not a link; nothing above it is checked,
  // because a symlink there is the user's own arrangement and replacing it needs write on $HOME.
  if (ours(parent, 0)) {
    mkdirSync(cache, { recursive: true, mode: 0o700 });
    // `compile` must be closed to everyone else, not merely unwritable: V8 reads its entries from
    // a versioned directory Node creates inside at 0777 minus the umask, group-writable wherever
    // the umask is 002, and denying the traverse bit puts that out of reach whatever its own mode.
    if (ours(cache, 0o077)) enableCompileCache(cache);
  }
} catch {
  // No cache, same behaviour. A read-only home costs the cache and never the command.
}

// Through this file's real path, not a bare './engine.mjs': a global install runs the bin symlink
// npm creates, and under --preserve-symlinks-main `import.meta.url` is that symlink, so a relative
// specifier would look for the engine beside the link and every invocation would fail with
// ERR_MODULE_NOT_FOUND.
const here = realpathSync(fileURLToPath(import.meta.url));
await import(pathToFileURL(join(dirname(here), 'engine.mjs')).href);
