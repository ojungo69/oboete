// Grok Build's wiring: `<grok home>/hooks/oboete.json` for the hooks and an oboete-managed block
// in `<grok home>/config.toml` for the MCP server (contracts/agents.md "Capture and injection per
// agent", FR-031). Grok kills a hook at its own timeout -- 5 s by default, 1.5 s on `SessionEnd` --
// so every handler states one (FR-031). Grok also reads `~/.claude/settings.json` as a compat layer
// unless `GROK_CLAUDE_HOOKS_ENABLED` is 0, which would run oboete's Claude handlers a second time;
// that is reported to the caller and never repaired here: the developer's Claude file is not this
// writer's to edit and an environment variable is not oboete's to set (FR-043).
import { existsSync, readFileSync, rmSync } from 'node:fs';
import { join } from 'node:path';
import { stringify as stringifyToml } from 'smol-toml';

import {
  applyJsonHandlers,
  applyTomlBlock,
  BACKUP_SUFFIX,
  isOboeteOwned,
  isPlainObject,
  removeJsonHandlers,
  removeTomlBlock,
} from './managed-block.js';
import { shellQuote } from './shell-quote.js';

/**
 * Every event oboete takes from Grok, with the seconds the agent gives the hook. 12 s covers the
 * hooks that build or deliver a pack -- the 1 s session-start wait plus margin -- where FR-045 puts
 * delivery on `PreToolUse` and `PostToolUse`; 3 s covers the capture-only ones. The keys are also
 * the order in which the file is written.
 */
const HOOK_TIMEOUTS: Readonly<Record<string, number>> = {
  SessionStart: 12,
  UserPromptSubmit: 12,
  PreToolUse: 12,
  PostToolUse: 12,
  PostToolUseFailure: 3,
  PermissionDenied: 3,
  Stop: 3,
  PostCompact: 3,
  SessionEnd: 3,
};

export type GrokWriteOptions = {
  /** Absolute path of the node binary the handler runs (`process.execPath`). */
  nodePath: string;
  /** Absolute path of `dist/oboete.mjs`. */
  bundlePath: string;
  /** `~/.claude/settings.json`, read only, to name the handlers the compat layer would repeat. */
  claudeSettingsPath?: string;
};

export type GrokWriteResult = {
  /** The files this writer created or edited, in the order it touched them. */
  files: string[];
  /**
   * Events whose oboete handler in the developer's Claude Code settings the Grok compat layer
   * would fire a second time. The setup command decides what to tell the developer.
   */
  duplicateClaudeEvents: string[];
};

export function writeGrok(grokHome: string, options: GrokWriteOptions): GrokWriteResult {
  const hooksFile = join(grokHome, 'hooks', 'oboete.json');
  const configFile = join(grokHome, 'config.toml');
  const hooksExisted = existsSync(hooksFile);
  const hooksBackupExisted = existsSync(hooksFile + BACKUP_SUFFIX);
  const configBackupExisted = existsSync(configFile + BACKUP_SUFFIX);

  const handlers: Record<string, unknown[]> = {};
  for (const [event, timeout] of Object.entries(HOOK_TIMEOUTS)) {
    handlers[event] = [
      {
        hooks: [{ type: 'command', command: hookCommand(options, event), timeout }],
        oboete: true,
      },
    ];
  }
  // One call: applyJsonHandlers takes `handlers` for the whole of what oboete owns in the file.
  applyJsonHandlers(hooksFile, handlers);

  // Verified 2026-09-03 (docs/research/oboete-contracts-probes.md "Grok user-scoped MCP
  // registration"): the same table `grok mcp add --scope user` writes; hooks then see the tools as
  // `oboete__<tool>`. config.toml can carry an `env` table with a token, so its backup is 0600.
  try {
    applyTomlBlock(
      configFile,
      stringifyToml({
        mcp_servers: {
          oboete: { command: options.nodePath, args: [options.bundlePath, 'mcp'], enabled: true },
        },
      }),
      { credentialBearing: true },
    );
  } catch (error) {
    // The handlers go in before the block, so a failure here would otherwise leave Grok running
    // oboete's hooks while setup reports `wired: failed` and no probe runs. A setup that fails
    // hands the files back the way it found them (FR-031, the rule src/setup/write-codex.ts keeps).
    // A backup this run took is its own leftover; an older one is the pre-oboete copy and stays.
    removeJsonHandlers(hooksFile, { keepBackup: hooksBackupExisted });
    if (!hooksExisted) rmSync(hooksFile, { force: true });
    if (!configBackupExisted) rmSync(configFile + BACKUP_SUFFIX, { force: true });
    throw error;
  }

  return {
    files: [hooksFile, configFile],
    duplicateClaudeEvents: claudeCompatDuplicates(options.claudeSettingsPath),
  };
}

/** Undoes {@link writeGrok}: the handlers, the file when it held nothing else, and the MCP block. */
export function removeGrok(grokHome: string): void {
  const hooksFile = join(grokHome, 'hooks', 'oboete.json');
  removeJsonHandlers(hooksFile);
  const remaining = readJson(hooksFile);
  // oboete created this file, so an empty one is oboete's leftover rather than the developer's.
  if (remaining !== null && Object.keys(remaining).length === 0) rmSync(hooksFile);
  removeTomlBlock(join(grokHome, 'config.toml'));
}

/**
 * Grok runs the command through a shell, so both absolute paths are quoted (agent identity per
 * contracts/agents.md: the selector is fixed on the command line, never guessed from the payload).
 */
function hookCommand(options: GrokWriteOptions, event: string): string {
  return `${shellQuote(options.nodePath)} ${shellQuote(options.bundlePath)} hook --agent claude-or-grok --event ${event}`;
}

function claudeCompatDuplicates(settingsPath: string | undefined): string[] {
  if (settingsPath === undefined) return [];
  const root = readJson(settingsPath);
  const container = root === null ? null : root.hooks;
  if (!isPlainObject(container)) return [];
  return Object.keys(HOOK_TIMEOUTS).filter((event) => {
    const entries = container[event];
    return Array.isArray(entries) && entries.some(isOboeteOwned);
  });
}

/** A file oboete only reads: unreadable or unparsable is reported as "nothing to say about it". */
function readJson(file: string): Record<string, unknown> | null {
  try {
    const parsed: unknown = JSON.parse(readFileSync(file, 'utf8'));
    return isPlainObject(parsed) ? parsed : null;
  } catch {
    return null;
  }
}
