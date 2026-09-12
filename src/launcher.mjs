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
// A namespace import, because a named one is resolved when the module is linked, before any
// statement here runs and outside the reach of the try below: on a Node older than 22.1, where
// `enableCompileCache` does not exist, that is a SyntaxError and a non-zero exit for every command,
// including the hook that is contracted to exit 0 whatever happens (FR-002). `engines` only warns.
import * as nodeModule from 'node:module';
import { homedir } from 'node:os';
import { dirname, isAbsolute, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

/** True when `path` is a real directory belonging to this user, with none of `denied`'s bits set
 *  for anyone else. `mkdirSync` leaves an existing directory's mode and owner alone and follows a
 *  symlink, so what came back is checked rather than assumed, and one observation decides all of
 *  it. Refusing beats correcting: a chmod would land on the target of a planted symlink. Windows
 *  has neither uid nor mode bits, so there the first two checks are the whole test. */
function ours(path, denied) {
  const found = lstatSync(path);
  if (found.isSymbolicLink() || !found.isDirectory()) return false;
  const uid = process.getuid?.();
  return uid === undefined || (found.uid === uid && (found.mode & denied) === 0);
}

/** The directory holding the cache is asked for no particular mode: one left loose by an older
 *  version or a restored backup is still the user's own, and refusing it would turn the cache off
 *  without a word. The cache itself must be closed to everyone else rather than merely unwritable,
 *  because V8 reads its entries from a versioned directory Node creates inside at 0777 minus the
 *  umask -- group-writable wherever the umask is 002 -- and denying the traverse bit on the parent
 *  is what puts that out of reach whatever its own mode. */
const ANY_MODE = 0;
const CLOSED = 0o077;

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
  if (ours(parent, ANY_MODE)) {
    mkdirSync(cache, { recursive: true, mode: 0o700 });
    // `compile` must be closed to everyone else, not merely unwritable: V8 reads its entries from
    // a versioned directory Node creates inside at 0777 minus the umask, group-writable wherever
    // the umask is 002, and denying the traverse bit puts that out of reach whatever its own mode.
    if (ours(cache, CLOSED)) nodeModule.enableCompileCache?.(cache);
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
