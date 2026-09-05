// The consent screen of `oboete setup` (FR-022, research R8): what a remote destination would
// receive, and whether the developer has agreed to it. Consent is bound to the hash of the tuple
// shown here -- preset, host, credential source, cost class, egress classes -- so a stored record
// stops accepting `--yes` the moment any of those five change. The same hash is recomputed by
// src/config.ts before every provider call (contracts/observer.md call policy 6); nothing in this
// module is on the hook path.
import { chmodSync, existsSync, readFileSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { parse as parseToml, stringify as stringifyToml } from 'smol-toml';

import {
  consentHash,
  consentMatches,
  consentTuple,
  PRESET_CATALOG,
  readCredentials,
  type ConsentTuple,
  type OboeteConfig,
} from '../config.js';
import type { OboetePaths } from '../paths.js';

/** Where `oboete doctor` reads the consent record back from (data-model.md runtime_state). */
export const CONSENT_STATE_KEY = 'consent.record';

export type ConsentDecision = {
  /** `not_required` for a preset that sends nothing off this machine (`ollama`, `none`). */
  state: 'not_required' | 'accepted' | 'missing';
  tuple: ConsentTuple;
  hash: string;
};

function isRemote(config: OboeteConfig): boolean {
  const preset = config.observer.preset;
  return preset !== 'none' && PRESET_CATALOG[preset].egress === 'remote';
}

/** The five values consent is bound to, as the developer reads them before accepting (FR-022). */
export function consentDisplay(config: OboeteConfig, env: NodeJS.ProcessEnv): string[] {
  const tuple = consentTuple(config, env);
  if (tuple.preset === 'none') return ['The observer has no provider: memories are written by rule alone.'];

  const lines = [
    isRemote(config)
      ? 'The observer would send memory material to a destination outside this machine:'
      : 'The observer would summarize on this machine:',
    `  Preset: ${tuple.preset}`,
    `  Destination host: ${tuple.host}`,
    `  Credential source: ${tuple.credentialSource}`,
    `  Cost class: ${tuple.costClass}`,
    `  Sensitivity classes sent: ${tuple.egressClasses.join(', ') || 'none'}`,
  ];
  if (tuple.preset === 'agent-cli') {
    const cli = config.observer.agent_cli;
    lines.push(
      `  This preset spends your own ${cli} subscription: the observer runs the ${cli} command line tool`,
      '  as a child process, so every summary is billed to that subscription rather than to a provider',
      "  allowance of oboete's own.",
    );
  }
  return lines;
}

/**
 * `--accept-egress` accepts the tuple shown now; `--yes` accepts only when the stored record is
 * the hash of that same tuple, which is what makes a changed host, credential source or egress
 * class refuse the flag (R8).
 */
export function decideConsent(options: {
  config: OboeteConfig;
  env: NodeJS.ProcessEnv;
  acceptEgress?: boolean;
  yes?: boolean;
}): ConsentDecision {
  const tuple = consentTuple(options.config, options.env);
  const hash = consentHash(tuple);
  if (!isRemote(options.config)) return { state: 'not_required', tuple, hash };
  if (options.acceptEgress === true) return { state: 'accepted', tuple, hash };
  if (options.yes === true && consentMatches(options.config, options.env)) {
    return { state: 'accepted', tuple, hash };
  }
  return { state: 'missing', tuple, hash };
}

export function saveConsent(paths: OboetePaths, hash: string, acceptedAt: number): void {
  updateConfigFile(paths, (root) => {
    const consent = table(root, 'consent');
    consent.hash = hash;
    consent.accepted_at = acceptedAt;
  });
}

export function saveProviderPreset(paths: OboetePaths, preset: string): void {
  updateConfigFile(paths, (root) => {
    table(root, 'observer').preset = preset;
  });
}

/**
 * Rewrites `config.toml` with the settings the developer did not name left as they were. This is
 * oboete's own file, not a foreign one, so it is re-serialized rather than spliced: keys and values
 * survive, comments and ordering do not.
 */
function updateConfigFile(paths: OboetePaths, apply: (root: Record<string, unknown>) => void): void {
  const present = existsSync(paths.config);
  const parsed: unknown = present ? parseToml(readFileSync(paths.config, 'utf8')) : {};
  const root = (typeof parsed === 'object' && parsed !== null ? parsed : {}) as Record<string, unknown>;
  apply(root);

  // The file records what the developer consented to, so it stays owner-only unless it already
  // carries a mode of their own.
  const mode = present ? statSync(paths.config).mode & 0o7777 : 0o600;
  const temporary = `${paths.config}.oboete-tmp-${process.pid}`;
  // `wx`: an exclusive create, so a link pre-placed at this predictable name is refused rather than
  // followed (the rule src/setup/managed-block.ts states for the foreign files).
  writeFileSync(temporary, `${stringifyToml(root)}\n`, { mode, flag: 'wx' });
  try {
    chmodSync(temporary, mode); // The creation mode is masked by the umask; this is not.
    // R8's temporary file -> re-parse -> rename, so a result oboete could not load again never
    // replaces a configuration it can (every later command reads this file through loadConfig).
    parseToml(readFileSync(temporary, 'utf8'));
  } catch (error) {
    rmSync(temporary, { force: true });
    throw error;
  }
  renameSync(temporary, paths.config);
}

function table(root: Record<string, unknown>, name: string): Record<string, unknown> {
  const current = root[name];
  if (typeof current === 'object' && current !== null && !Array.isArray(current)) {
    return current as Record<string, unknown>;
  }
  const created: Record<string, unknown> = {};
  root[name] = created;
  return created;
}

/**
 * What to do when the configured preset has no credentials. Setup prints this and carries on with
 * the rule-based fallback instead of failing (contracts/cli.md `oboete setup`, amendment list 4).
 */
export function credentialGuidance(config: OboeteConfig, env: NodeJS.ProcessEnv): string[] {
  const preset = config.observer.preset;
  if (preset === 'none') return [];
  const credentials = readCredentials(preset, env, config.observer.agent_cli);
  if (credentials.present) return [];

  const lines = [`No credentials are set for the ${preset} preset (${credentials.source}).`];
  if (credentials.kind === 'cloudflare') {
    lines.push(
      '  1. Create a free Cloudflare account: https://dash.cloudflare.com/sign-up',
      '  2. Copy the account identifier from Workers & Pages: https://dash.cloudflare.com/',
      '  3. Create an API token from the Workers AI template: https://dash.cloudflare.com/profile/api-tokens',
      '  4. Export OBOETE_CF_ACCOUNT_ID and OBOETE_CF_API_TOKEN in the shell that runs the agents.',
    );
  } else {
    lines.push(`  Export that variable in the shell that runs the agents.`);
  }
  lines.push(
    'Setup continues without a provider. You can also run `oboete setup --provider ollama` to summarize',
    'with a model on this machine, `oboete setup --provider agent-cli` to spend your agent command line',
    'subscription, or leave it as it is: until a provider is configured, memories are written by rule alone.',
  );
  return lines;
}
