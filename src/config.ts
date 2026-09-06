import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { parse as parseToml } from 'smol-toml';
import { z } from 'zod';

import { sha256Json } from './hash.js';
import { globRuleError } from './privacy/detect.js';
import type { OboetePaths } from './paths.js';

const PRESET_NAMES = ['workers-ai', 'ollama', 'nim', 'openrouter', 'gemini', 'agent-cli'] as const;
const AGENT_CLIS = ['claude', 'codex', 'grok'] as const;

export type PresetName = (typeof PRESET_NAMES)[number];
export type AgentCli = (typeof AGENT_CLIS)[number];

export type CredentialSpec =
  | { kind: 'cloudflare' }
  | { kind: 'api-key'; envName: string }
  | { kind: 'none' }
  | { kind: 'agent-login' };

export type ProviderPreset = {
  host: string;
  baseUrl: string;
  credential: CredentialSpec;
  costClass: 'free-tier' | 'remote' | 'local' | 'own-subscription';
  egress: 'remote' | 'local' | 'none';
  defaultModel: string;
  structuredOutput: 'json_schema' | 'response_format' | 'text-json';
  capped: boolean;
};

/** Which sensitivity classes an egress kind may carry (contracts/observer.md destination table). */
export const EGRESS_CLASSES: Record<ProviderPreset['egress'], readonly string[]> = {
  // FR-018 and Principle III: a remote destination receives eligible rows and nothing else.
  remote: ['eligible'],
  // A model on this machine may also see local_only and private rows; they never leave the host.
  local: ['eligible', 'local_only', 'private'],
  none: [],
};

/**
 * The provider presets of contracts/observer.md, with the endpoints and models the R13 probe
 * verified on 2026-09-03 (docs/research/oboete-contracts-probes.md "R13 evaluation").
 * `src/observer/providers.ts` reads this record; no other module restates a host or a model id.
 */
export const PRESET_CATALOG: Record<PresetName, ProviderPreset> = {
  'workers-ai': {
    host: 'api.cloudflare.com',
    baseUrl: 'https://api.cloudflare.com/client/v4/accounts/<account>/ai',
    credential: { kind: 'cloudflare' },
    costClass: 'free-tier',
    egress: 'remote',
    defaultModel: '@cf/zai-org/glm-4.7-flash',
    structuredOutput: 'json_schema',
    capped: true,
  },
  ollama: {
    host: '127.0.0.1:11434',
    baseUrl: 'http://127.0.0.1:11434/v1',
    credential: { kind: 'none' },
    costClass: 'local',
    egress: 'local',
    // The installed models differ per machine, so this preset takes its model from [observer] model.
    defaultModel: '',
    structuredOutput: 'response_format',
    capped: false,
  },
  nim: {
    host: 'integrate.api.nvidia.com',
    baseUrl: 'https://integrate.api.nvidia.com/v1',
    credential: { kind: 'api-key', envName: 'OBOETE_NIM_API_KEY' },
    costClass: 'remote',
    egress: 'remote',
    defaultModel: 'meta/llama-3.2-11b-vision-instruct',
    structuredOutput: 'text-json',
    capped: true,
  },
  openrouter: {
    host: 'openrouter.ai',
    baseUrl: 'https://openrouter.ai/api/v1',
    credential: { kind: 'api-key', envName: 'OBOETE_OPENROUTER_API_KEY' },
    costClass: 'remote',
    egress: 'remote',
    defaultModel: 'openai/gpt-4o-mini',
    structuredOutput: 'response_format',
    capped: true,
  },
  gemini: {
    host: 'generativelanguage.googleapis.com',
    baseUrl: 'https://generativelanguage.googleapis.com/v1beta/openai',
    credential: { kind: 'api-key', envName: 'OBOETE_GEMINI_API_KEY' },
    costClass: 'remote',
    egress: 'remote',
    defaultModel: 'gemini-2.5-flash',
    structuredOutput: 'text-json',
    capped: true,
  },
  'agent-cli': {
    // A child process, not an endpoint; the CLI decides host and model from its own login.
    host: 'agent-cli child process',
    baseUrl: '',
    credential: { kind: 'agent-login' },
    costClass: 'own-subscription',
    egress: 'remote',
    defaultModel: '',
    structuredOutput: 'text-json',
    // FR-012: this preset spends the developer's subscription, not oboete's allowance.
    capped: false,
  },
};

const observerSchema = z.strictObject({
  preset: z.enum(['none', ...PRESET_NAMES]).default('workers-ai'),
  model: z.string().min(1).optional(),
  agent_cli: z.enum(AGENT_CLIS).default('claude'),
});

const injectionSchema = z.strictObject({
  context_fraction: z.number().gt(0).lte(0.5).default(0.05),
  threshold: z.number().gte(0).lte(1).default(0.3),
});

/**
 * A path rule is compiled where it is read, not where it is used: the detector compiles it inside
 * the blanket catch that answers `detector_error`, so one malformed rule would blank the content of
 * every event that carries a path instead of naming itself.
 */
function secretPathRule(rule: z.ZodString): z.ZodType<string> {
  return rule.superRefine((value, context) => {
    const error = globRuleError(value);
    if (error !== null) context.addIssue({ code: 'custom', message: `is not a usable path rule (${error})` });
  });
}

const privacySchema = z.strictObject({
  secret_paths: z.array(secretPathRule(z.string().min(1))).default([]),
});

const consentSchema = z.strictObject({
  hash: z.string().min(1).optional(),
  accepted_at: z.number().int().optional(),
});

export const configSchema = z.strictObject({
  observer: observerSchema.prefault({}),
  injection: injectionSchema.prefault({}),
  privacy: privacySchema.prefault({}),
  consent: consentSchema.prefault({}),
});

export type OboeteConfig = z.infer<typeof configSchema>;

/**
 * R4: `.oboete.toml` is repository-supplied, so its rule list is bounded before anything compiles
 * it. A repository that writes more or longer rules than this gets a RepoConfigError, which the
 * hook stores as a malformed-configuration row.
 */
export const MAX_REPO_SECRET_PATHS = 64;
export const MAX_REPO_SECRET_PATH_LENGTH = 256;

const repoRulesSchema = z.strictObject({
  privacy: z
    .strictObject({
      secret_paths: z
        .array(secretPathRule(z.string().min(1).max(MAX_REPO_SECRET_PATH_LENGTH)))
        .max(MAX_REPO_SECRET_PATHS)
        .default([]),
    })
    .prefault({}),
});

export class ConfigError extends Error {
  readonly code: 'config_malformed' | 'config_credentials';

  constructor(message: string, code: ConfigError['code']) {
    super(message);
    this.name = 'ConfigError';
    this.code = code;
  }
}

export class RepoConfigError extends Error {
  readonly code = 'repo_config_malformed';

  constructor(message: string) {
    super(message);
    this.name = 'RepoConfigError';
  }
}

const KNOWN_KEY_PATHS = new Set([
  'observer',
  'observer.preset',
  'observer.model',
  'observer.agent_cli',
  'injection',
  'injection.context_fraction',
  'injection.threshold',
  'privacy',
  'privacy.secret_paths',
  'consent',
  'consent.hash',
  'consent.accepted_at',
]);

const CREDENTIAL_LIKE_KEY = /credential|token|key|secret/i;

function reason(error: unknown): string {
  return error instanceof Error ? error.message.split('\n')[0] : String(error);
}

function issues(error: z.ZodError): string {
  return error.issues.map((issue) => `${issue.path.join('.') || 'the file'}: ${issue.message}`).join('; ');
}

/**
 * FR-016: credentials never live in the configuration file. A credential-shaped key is refused
 * with the variable to use instead, rather than the generic "unrecognized key" of the schema.
 * The key path is named, the value never is.
 */
function assertNoCredentialKeys(value: unknown, prefix = ''): void {
  if (typeof value !== 'object' || value === null) return;
  if (Array.isArray(value)) {
    // An array of tables carries keys too, so it is scanned under the same path.
    for (const item of value) assertNoCredentialKeys(item, prefix);
    return;
  }
  for (const [key, child] of Object.entries(value)) {
    const path = prefix === '' ? key : `${prefix}.${key}`;
    if (!KNOWN_KEY_PATHS.has(path) && CREDENTIAL_LIKE_KEY.test(key)) {
      throw new ConfigError(
        `The configuration key "${path}" looks like a credential. Credentials must come from an environment variable, never from the configuration file: use OBOETE_CF_API_TOKEN and OBOETE_CF_ACCOUNT_ID for Workers AI, or OBOETE_<PRESET>_API_KEY for the other presets.`,
        'config_credentials',
      );
    }
    assertNoCredentialKeys(child, path);
  }
}

/**
 * Reads `config.toml`. A missing file gives the defaults; a malformed file or a schema failure
 * throws instead of applying part of the file, and the capture path treats that as a
 * classification failure and fails closed (R4).
 */
export function loadConfig(paths: OboetePaths): OboeteConfig {
  if (!existsSync(paths.config)) return configSchema.parse({});

  let raw: unknown;
  try {
    raw = parseToml(readFileSync(paths.config, 'utf8'));
  } catch (error) {
    throw new ConfigError(
      `The configuration file ${paths.config} is not valid TOML: ${reason(error)}`,
      'config_malformed',
    );
  }

  assertNoCredentialKeys(raw);

  const parsed = configSchema.safeParse(raw);
  if (!parsed.success) {
    throw new ConfigError(
      `The configuration file ${paths.config} has invalid settings: ${issues(parsed.error)}`,
      'config_malformed',
    );
  }
  return parsed.data;
}

/**
 * Reads `<repoRoot>/.oboete.toml`. A committed file may add path rules and nothing else, so the
 * schema is strict; a malformed or wider file throws and capture fails closed (R4).
 */
export function loadRepoRules(repoRoot: string): { secretPaths: string[] } {
  const file = join(repoRoot, '.oboete.toml');
  if (!existsSync(file)) return { secretPaths: [] };

  let raw: unknown;
  try {
    raw = parseToml(readFileSync(file, 'utf8'));
  } catch (error) {
    throw new RepoConfigError(`The repository file ${file} is not valid TOML: ${reason(error)}`);
  }

  const parsed = repoRulesSchema.safeParse(raw);
  if (!parsed.success) {
    throw new RepoConfigError(
      `The repository file ${file} may only set [privacy] secret_paths: ${issues(parsed.error)}`,
    );
  }
  return { secretPaths: parsed.data.privacy.secret_paths };
}

export type Credentials = {
  kind: CredentialSpec['kind'];
  present: boolean;
  source: string;
  values: Record<string, string>;
};

function readVariable(env: NodeJS.ProcessEnv, name: string): string {
  return (env[name] ?? '').trim();
}

/**
 * FR-016 and FR-043: credential values come only from the OBOETE_ variables named for the preset,
 * never from a file and never from another agent's session or subscription store.
 */
export function readCredentials(
  preset: PresetName,
  env: NodeJS.ProcessEnv = process.env,
  agentCli: AgentCli = 'claude',
): Credentials {
  const { credential } = PRESET_CATALOG[preset];
  switch (credential.kind) {
    case 'cloudflare': {
      const token = readVariable(env, 'OBOETE_CF_API_TOKEN');
      const accountId = readVariable(env, 'OBOETE_CF_ACCOUNT_ID');
      const values: Record<string, string> = {};
      if (token !== '') values.token = token;
      if (accountId !== '') values.accountId = accountId;
      return {
        kind: 'cloudflare',
        // The account id addresses the endpoint, so one variable without the other is unusable.
        present: token !== '' && accountId !== '',
        source: 'env:OBOETE_CF_API_TOKEN+OBOETE_CF_ACCOUNT_ID',
        values,
      };
    }
    case 'api-key': {
      const apiKey = readVariable(env, credential.envName);
      return {
        kind: 'api-key',
        present: apiKey !== '',
        source: `env:${credential.envName}`,
        values: apiKey === '' ? {} : { apiKey },
      };
    }
    case 'agent-login':
      // oboete holds no credential here; the CLI's own login is checked by the setup probe.
      return { kind: 'agent-login', present: true, source: `agent login (${agentCli})`, values: {} };
    case 'none':
      return { kind: 'none', present: true, source: 'none', values: {} };
  }
}

export type ConsentTuple = {
  preset: string;
  host: string;
  credentialSource: string;
  costClass: string;
  egressClasses: readonly string[];
};

/** The tuple setup displays and consent is bound to (R8): preset, host, credential source, cost class, egress classes. */
export function consentTuple(config: OboeteConfig, env: NodeJS.ProcessEnv = process.env): ConsentTuple {
  const preset = config.observer.preset;
  // No preset means no destination at all, so there is nothing to display and nothing to consent to.
  if (preset === 'none') {
    return { preset, host: '', credentialSource: 'none', costClass: 'none', egressClasses: [] };
  }
  const entry = PRESET_CATALOG[preset];
  return {
    preset,
    host: entry.host,
    credentialSource: readCredentials(preset, env, config.observer.agent_cli).source,
    costClass: entry.costClass,
    egressClasses: EGRESS_CLASSES[entry.egress],
  };
}

export function consentHash(tuple: ConsentTuple): string {
  return sha256Json([
    tuple.preset,
    tuple.host,
    tuple.credentialSource,
    tuple.costClass,
    tuple.egressClasses,
  ]);
}

/**
 * True when the stored consent still describes the live configuration. Recomputed before every
 * reservation and again immediately before every send (contracts/observer.md call policy 6); a
 * mismatch means no network call and a batch degraded with `consent_changed`.
 */
export function consentMatches(config: OboeteConfig, env: NodeJS.ProcessEnv = process.env): boolean {
  const stored = config.consent.hash;
  if (stored !== undefined) return stored === consentHash(consentTuple(config, env));
  const preset = config.observer.preset;
  // Without a stored record only a preset that sends nothing off this machine may run (R8).
  return preset === 'none' || PRESET_CATALOG[preset].egress !== 'remote';
}

/** The pause marker. Callers check it before the database is opened (R12, "Pause"). */
export function isPaused(paths: OboetePaths): boolean {
  return existsSync(paths.paused);
}
