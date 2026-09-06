// Claude Code's hook wiring: oboete-owned handler groups merged into the developer's
// `settings.json` (contracts/agents.md "Capture and injection per agent", FR-031). Nothing else in
// that file is read or rewritten, and Claude Code's own memory feature is never touched (FR-032,
// FR-043). Grok Build reads this same file as its Claude compatibility layer, which is why the
// handlers carry the `claude-or-grok` selector the hook resolves at run time.
import { readFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';

import {
  applyJsonHandlers,
  BACKUP_SUFFIX,
  isOboeteOwned,
  isPlainObject,
  removeJsonHandlers,
} from './managed-block.js';
import { shellQuote } from './shell-quote.js';

export type WriteClaudeOptions = {
  /** Absolute path of the node binary that runs the bundle (`process.execPath` at setup). */
  nodePath: string;
  /** Absolute path of `dist/oboete.mjs`. */
  bundlePath: string;
};

export type WriteClaudeResult = {
  file: string;
  /**
   * The arguments of `claude mcp add oboete -- <node> <bundle> mcp`, without the executable: setup
   * puts the absolute path detection resolved on PATH in front of them and runs the result through
   * its own spawner, so a missing `claude` is reported rather than thrown from here (contracts/cli.md
   * `oboete setup`). No shell is involved, so the arguments carry no quoting.
   */
  mcpArgs: readonly string[];
};

/**
 * Every event oboete wires, with the timeout Claude Code gives that handler in seconds. Injection
 * hooks get 12 s and capture hooks 3 s (contracts/agents.md); the hook's own deadline is far
 * shorter (300 ms, 1 s at session start), so the timeout is only the ceiling that keeps a stalled
 * handler from becoming the developer's problem. `SessionEnd` is capture only -- Claude Code shares
 * 1.5 s between all `SessionEnd` handlers and a timeout here cannot raise that budget -- so its
 * command is the same capture entry point and no pack is ever built on it.
 */
const TIMEOUTS: Record<string, number> = {
  SessionStart: 12,
  UserPromptSubmit: 12,
  PreToolUse: 3,
  PostToolUse: 3,
  PostToolUseFailure: 3,
  Stop: 3,
  PostCompact: 3,
  SessionEnd: 3,
};

/**
 * Writes the handlers into `<claudeConfigDir>/settings.json`, creating the file and its directory
 * when a fresh machine has neither. Repeatable: the whole set goes in one call, so a rerun replaces
 * exactly what oboete owns and leaves the developer's handlers where they stand (FR-031).
 */
export function writeClaude(claudeConfigDir: string, options: WriteClaudeOptions): WriteClaudeResult {
  const file = join(claudeConfigDir, 'settings.json');
  const handlers: Record<string, unknown[]> = {};
  for (const [event, timeout] of Object.entries(TIMEOUTS)) {
    handlers[event] = [
      {
        hooks: [
          {
            type: 'command',
            command: `${shellQuote(options.nodePath)} ${shellQuote(options.bundlePath)} hook --agent claude-or-grok --event ${event}`,
            timeout,
          },
        ],
        oboete: true,
      },
    ];
  }
  // `settings.json` can carry credentials (`env`, `apiKeyHelper`), so the copy taken before the
  // first oboete write is owner-only whatever mode the developer's own file has (research.md R8).
  applyJsonHandlers(file, handlers, { credentialBearing: true });
  return {
    file,
    mcpArgs: ['mcp', 'add', 'oboete', ...MCP_SCOPE, '--', options.nodePath, options.bundlePath, 'mcp'],
  };
}

/**
 * `claude mcp add` and `claude mcp remove` both default to local scope, which registers the tools
 * for the directory setup happened to run in and nowhere else; contracts/mcp.md registers oboete
 * for the user, the way the Codex and Grok writers do. Verified against claude 2.1.260: without the
 * flag `claude mcp get oboete` answers "No MCP server named" from any other directory, and a
 * scope-less `remove` refuses with "exists in multiple scopes" once both scopes hold an entry.
 */
const MCP_SCOPE = ['--scope', 'user'] as const;

/** The removal that matches the registration above; `oboete setup --remove` runs it. */
export const MCP_REMOVE_ARGS = ['mcp', 'remove', 'oboete', ...MCP_SCOPE] as const;

/**
 * Takes the oboete handlers back out of `settings.json` and leaves everything else as it was. What
 * comes back is the developer's settings, not their byte layout: JSON has no textual managed block,
 * so the file is re-serialized in the two-space form and only the backup holds the original bytes.
 * That is why a file holding nothing of oboete's is never handed to `removeJsonHandlers` -- there
 * would be nothing to take out, and rewriting a file oboete never owned is exactly the "disturbing
 * unrelated configuration" FR-031 forbids. Its backup, if setup left one, still goes.
 */
export function removeClaude(claudeConfigDir: string): void {
  const file = join(claudeConfigDir, 'settings.json');
  if (holdsOboeteHandler(file)) removeJsonHandlers(file);
  else rmSync(file + BACKUP_SUFFIX, { force: true });
}

/**
 * Whether `file` holds an entry removal would strip. A file that is absent, unreadable, or not a
 * JSON object answers yes, so it still reaches `removeJsonHandlers` and is reported there rather
 * than passed over in silence.
 */
function holdsOboeteHandler(file: string): boolean {
  let root: unknown;
  try {
    root = JSON.parse(readFileSync(file, 'utf8'));
  } catch {
    return true;
  }
  if (!isPlainObject(root)) return true;
  const hooks = root.hooks;
  if (!isPlainObject(hooks)) return false;
  return Object.values(hooks).some((entries) => Array.isArray(entries) && entries.some(isOboeteOwned));
}
