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
import { lstatSync, mkdirSync } from 'node:fs';
import { enableCompileCache } from 'node:module';
import { homedir } from 'node:os';
import { isAbsolute, join } from 'node:path';

/** True when `path` is a directory of this user's that nobody else can even enter. `mkdirSync`
 *  leaves an existing directory's mode and owner alone and follows a symlink, so what came back is
 *  checked rather than assumed. Being unwritable by others is not enough: V8 reads its entries from
 *  a versioned directory that Node creates inside this one at 0777 minus the umask, group-writable
 *  wherever the umask is 002. Denying the traverse bit here puts that out of everyone else's reach
 *  whatever its own mode, which inspecting the children could not do without disabling the cache
 *  outright on those machines. Refusing beats correcting: a chmod would land on the target of a
 *  planted symlink.
 *
 *  Only this directory is checked, not the ones above it. Anything an attacker puts here in its
 *  place -- a directory of their own, a symlink -- belongs to them, and this check refuses it; what
 *  write access above buys is the race between this check and V8's read, which is accepted anyway
 *  (`homedir()` is not checked either, so demanding unwritable ancestors would only narrow it, at
 *  the price of refusing the 0775 `~/.cache` that a 002 umask leaves behind). */
function ownedAndClosed(path, uid) {
  const found = lstatSync(path);
  if (found.isSymbolicLink() || !found.isDirectory()) return false;
  return uid === undefined || (found.uid === uid && (found.mode & 0o077) === 0);
}

try {
  const base = process.env.XDG_CACHE_HOME;
  const root = base !== undefined && isAbsolute(base) ? base : join(homedir(), '.cache');
  const cache = join(root, 'oboete', 'compile');
  mkdirSync(cache, { recursive: true, mode: 0o700 });
  if (ownedAndClosed(cache, process.getuid?.())) enableCompileCache(cache);
} catch {
  // No cache, same behaviour. A read-only home costs the cache and never the command.
}

await import('./engine.mjs');
