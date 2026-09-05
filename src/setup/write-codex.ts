// Codex CLI wiring (FR-031): oboete-owned matcher groups in `<CODEX_HOME>/hooks.json`, and one
// managed block in `<CODEX_HOME>/config.toml` holding the trust rows those handlers need plus the
// MCP registration. The two files are written in that order because a trust key names the position
// a handler ended up in after the merge with the developer's own groups. Nothing else in either
// file is touched, and Codex's own `memories` setting is read by nobody here (FR-032, FR-043).
import { existsSync, readFileSync, rmSync } from 'node:fs';
import { resolve } from 'node:path';
import { stringify as stringifyToml } from 'smol-toml';

import { trustedHash, trustKey } from './codex-trust.js';
import type { CodexHandler } from './codex-trust.js';
import {
  applyJsonHandlers,
  applyTomlBlock,
  BACKUP_SUFFIX,
  removeJsonHandlers,
  removeTomlBlock,
} from './managed-block.js';
import { shellQuote } from './shell-quote.js';

export type CodexSetupOptions = {
  /** Absolute path of the node binary that runs the bundle (`process.execPath`). */
  node: string;
  /** Absolute path of the oboete bundle. */
  bundle: string;
};

export type WriteResult = {
  /** The files setup wrote, in the order they were written. */
  files: string[];
};

/** A matcher group as it is written: `oboete: true` is what makes it findable again for removal. */
type CodexGroup = { matcher?: string; hooks: CodexHandler[]; oboete: true };

type Wiring = {
  event: string;
  matcher?: string;
  /** Seconds. The hook's own deadline is 300 ms; this is the runner's outer bound (FR-031). */
  timeout: number;
  /** Set on the events that may answer with `additionalContext` and that oboete injects into. */
  injects?: true;
};

/**
 * The events of the Codex row of contracts/agents.md. The SessionStart matcher is the verified
 * enum without `resume`, where the transcript already carries what was injected. SessionEnd is
 * capture only and Codex clamps it to three seconds anyway.
 */
const WIRING: readonly Wiring[] = [
  { event: 'SessionStart', matcher: 'startup|clear|compact', timeout: 12, injects: true },
  { event: 'UserPromptSubmit', timeout: 12, injects: true },
  { event: 'PreToolUse', timeout: 3 },
  { event: 'PostToolUse', timeout: 3 },
  { event: 'Stop', timeout: 3 },
  { event: 'PostCompact', timeout: 3 },
  { event: 'SessionEnd', timeout: 3 },
];

export function writeCodex(home: string, options: CodexSetupOptions): WriteResult {
  const hooksPath = resolve(home, 'hooks.json');
  const configPath = resolve(home, 'config.toml');
  const hooksExisted = existsSync(hooksPath);
  const hooksBackupExisted = existsSync(hooksPath + BACKUP_SUFFIX);
  const backupExisted = existsSync(configPath + BACKUP_SUFFIX);

  const groups: Record<string, CodexGroup[]> = {};
  // One call: `handlers` is the whole of what oboete owns in the file, so wiring the events in
  // separate calls would leave each call's handlers stripped by the next.
  for (const wiring of WIRING) groups[wiring.event] = [group(wiring, options)];
  applyJsonHandlers(hooksPath, groups);

  try {
    // `config.toml` can hold an API key, so its backup is owner-only.
    applyTomlBlock(configPath, blockText(hooksPath, options), { credentialBearing: true });
  } catch (error) {
    // Codex skips a handler that has no matching trust row and says nothing, so handlers left
    // behind by a setup that could not write the rows would be wired and silently inert. A setup
    // that fails hands the files back the way it found them instead (FR-031).
    // Same rule as the config.toml backup below: a copy this run took is its own leftover, an
    // older one is the pre-oboete copy `oboete setup --remove` restores and stays.
    removeJsonHandlers(hooksPath, { keepBackup: hooksBackupExisted });
    if (!hooksExisted) rmSync(hooksPath, { force: true });
    // The backup is taken before the parse that rejected the write, so a copy this run made of a
    // file it then left alone is its own leftover; an older one is the pre-oboete copy and stays.
    if (!backupExisted) rmSync(configPath + BACKUP_SUFFIX, { force: true });
    throw error;
  }
  return { files: [hooksPath, configPath] };
}

export function removeCodex(home: string): void {
  const hooksPath = resolve(home, 'hooks.json');
  const configPath = resolve(home, 'config.toml');
  // A backup exists for every file that was the developer's before setup, so a file without one is
  // oboete's own: what removal leaves of it is an empty shell rather than their configuration.
  const oboetes = [hooksPath, configPath].filter((file) => !existsSync(file + BACKUP_SUFFIX));
  removeJsonHandlers(hooksPath);
  removeTomlBlock(configPath);
  for (const file of oboetes) if (isEmptyShell(file)) rmSync(file);
}

/** All that removal leaves of a file oboete created: no TOML at all, or an empty JSON object. */
function isEmptyShell(file: string): boolean {
  if (!existsSync(file)) return false;
  const text = readFileSync(file, 'utf8').trim();
  return text === '' || text === '{}';
}

function group(wiring: Wiring, options: CodexSetupOptions): CodexGroup {
  const handler: CodexHandler = {
    type: 'command',
    // Codex runs the command through a shell, and the fixed `--agent` selector is what picks the
    // adapter (contracts/agents.md "Agent identity").
    command: `${shellQuote(options.node)} ${shellQuote(options.bundle)} hook --agent codex --event ${wiring.event}`,
    timeout: wiring.timeout,
  };
  // 0 disables the 2,500-token spill, which would move the tail of a pack to a file under the
  // operating system's temporary directory that the model never reads (FR-031).
  if (wiring.injects) handler.additionalContextLimit = 0;
  const group: CodexGroup = { hooks: [handler], oboete: true };
  if (wiring.matcher !== undefined) group.matcher = wiring.matcher;
  return group;
}

/**
 * The MCP registration and one trust row per oboete handler, keyed by the position the handler
 * holds in the merged `hooks.json`: a group the developer wrote ahead of oboete's shifts the
 * index, so the positions are read back from the file rather than assumed. Only oboete's own
 * handlers get a row; trusting the developer's hooks is not oboete's decision to make.
 */
function blockText(hooksPath: string, options: CodexSetupOptions): string {
  const merged = mergedGroups(hooksPath);
  const state: Record<string, { trusted_hash: string }> = {};
  for (const wiring of WIRING) {
    (merged[wiring.event] ?? []).forEach((group, groupIndex) => {
      if (group.oboete !== true) return;
      group.hooks.forEach((handler, handlerIndex) => {
        state[trustKey(hooksPath, wiring.event, groupIndex, handlerIndex)] = {
          trusted_hash: trustedHash(wiring.event, group.matcher, handler),
        };
      });
    });
  }
  // `hooks` and `hooks.state` carry no key of their own, so smol-toml emits only the
  // `[hooks.state."<key>"]` rows and never a `[hooks]` header to collide with the developer's.
  return stringifyToml({
    mcp_servers: { oboete: { command: options.node, args: [options.bundle, 'mcp'] } },
    hooks: { state },
  });
}

function mergedGroups(hooksPath: string): Record<string, CodexGroup[]> {
  const file = JSON.parse(readFileSync(hooksPath, 'utf8')) as { hooks?: Record<string, CodexGroup[]> };
  return file.hooks ?? {};
}
