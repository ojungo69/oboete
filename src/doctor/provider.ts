import { randomUUID } from 'node:crypto';
import type { DatabaseSync } from 'node:sqlite';

import {
  PRESET_CATALOG,
  admittedChain,
  consentMatches,
  readCredentials,
  type ChainTarget,
  type OboeteConfig,
  type PresetName,
} from '../config.js';
import { isBusyError } from '../db/open.js';
import {
  asNumber,
  configUnread,
  dbUnread,
  degraded,
  failedItem,
  healthy,
  iso,
  unverified,
  warning,
  type DoctorDeps,
  type DoctorItem,
  type DoctorOptions,
} from '../doctor.js';
import { CACHE_MS, cachedCatalog } from '../observer/catalog.js';
import type { ObserverInput } from '../observer/contract.js';
import { summarizeWithProvider, type CallOutcome } from '../observer/llm.js';
import {
  DAILY_CAP,
  recordExhausted,
  recordProviderAttempt,
  usageEstimate,
  utcDay,
} from '../observer/reservation.js';
import type { OboetePaths } from '../paths.js';
import { credentialGuidance } from '../setup/consent.js';
import { describe } from '../setup/report.js';
import { transactionImmediate } from '../worker/lease.js';

/**
 * The worker allows 60 s per call (observer/llm.ts REQUEST_TIMEOUT_MS); the default model answers
 * the probe in several seconds, so 10 s reported a healthy provider as timed out (dogfood 2026-09-06).
 */
const PROVIDER_PROBE_TIMEOUT_MS = 30_000;

const PROVIDER_PROBE_INPUT: ObserverInput = {
  repo_ref: 'doctor',
  checkpoint_context: { state: 'none' },
  session: {
    started_at: 1_757_000_000_000,
    turns: [{ ordinal: 1, started_at: 1_757_000_000_000, ended_at: null }],
  },
  events: [{ id: 'e1', kind: 'prompt', text: 'Say OK.' }],
  free_summaries: {},
  nearby: [],
  language_hint: 'en',
};

const FALLBACK_CONSEQUENCE =
  'Temporary guidance is available while source processing waits for the provider.';
const ALLOWANCE_CONSEQUENCE =
  'Source processing waits for the allowance to reset; later worker runs retry due sources.';

export async function providerItem(input: {
  config: OboeteConfig | null;
  paths: OboetePaths;
  db: DatabaseSync | null;
  integrityFailed: boolean;
  deps: DoctorDeps;
  options: DoctorOptions;
  now: number;
}): Promise<DoctorItem> {
  const { config, paths, db, integrityFailed, deps, options, now } = input;
  const configured = configuredProvider(config, integrityFailed, deps.env);
  if (!('kind' in configured)) return configured;
  const { config: readyConfig, preset, credentials } = configured;
  const probe = providerProbeReadiness(readyConfig, preset, db, options, now);
  if (!('kind' in probe)) return probe;
  const { db: openDb, model, estimate } = probe;

  try {
    const outcome = await summarizeWithProvider(PROVIDER_PROBE_INPUT, {
      preset,
      model,
      agentCli: readyConfig.observer.agent_cli,
      credentials,
      consentOk: () => consentMatches(readyConfig, deps.env),
      reserve: () => doctorReserve(openDb, preset, now),
      onExhausted: (reservationId) => recordExhausted(openDb, { preset, reservationId, now }),
      fetch: deps.fetch,
      spawn: deps.spawn,
      now: deps.now,
      timeoutMs: PROVIDER_PROBE_TIMEOUT_MS,
    });

    if (outcome.ok) {
      return healthy(
        'provider',
        `Provider ${preset} answered with model ${outcome.resolvedModel ?? model}.`,
      );
    }
    return degraded(
      'provider',
      outcomeSentence(outcome),
      FALLBACK_CONSEQUENCE,
      providerRecovery(outcome.reason, readyConfig, paths, deps.env, estimate.resetAt),
    );
  } catch (error) {
    if (isBusyError(error)) return failedItem('provider', error);
    throw error;
  }
}

/**
 * Both halves of the provider check answer either with the doctor item they already decided on, or
 * with what the next step needs. The `kind` tag is what tells them apart: a structural test on
 * `status` would break the day either bag grew a field of that name.
 */
type ConfiguredProvider = {
  kind: 'configured';
  config: OboeteConfig;
  preset: Exclude<PresetName, 'none'>;
  credentials: ReturnType<typeof readCredentials>;
};

type ProviderProbeReadiness =
  | DoctorItem
  | { kind: 'ready'; db: DatabaseSync; model: string; estimate: ReturnType<typeof usageEstimate> };

function configuredProvider(
  config: OboeteConfig | null,
  integrityFailed: boolean,
  env: NodeJS.ProcessEnv,
): DoctorItem | ConfiguredProvider {
  if (config === null) return configUnread('provider');
  if (integrityFailed) {
    return dbUnread(
      'provider',
      true,
      'The database is unavailable, so the provider could not be probed.',
      'Summaries cannot be checked until storage is open.',
      '`oboete doctor --probe-provider` after storage is repaired.',
    );
  }

  const preset = config.observer.preset;
  if (preset === 'none') {
    return degraded(
      'provider',
      'No observer provider is configured.',
      'Summaries come from the rule-based fallback only (packs say `Degraded:`).',
      '`oboete setup --provider <preset>` (workers-ai is the free remote default; ollama stays local)',
    );
  }

  const credentials = readCredentials(preset, env, config.observer.agent_cli);
  if (!credentials.present) {
    return degraded(
      'provider',
      `No credentials are set for the ${preset} preset (${credentials.source}).`,
      'Summaries come from the rule-based fallback only (packs say `Degraded:`).',
      credentialSteps(config, env) ||
        '`oboete setup --provider <preset>` (workers-ai is the free remote default; ollama stays local)',
    );
  }
  return { kind: 'configured', config, preset, credentials };
}

function providerProbeReadiness(
  config: OboeteConfig,
  preset: Exclude<PresetName, 'none'>,
  db: DatabaseSync | null,
  options: DoctorOptions,
  now: number,
): ProviderProbeReadiness {
  if (!options.probeProvider) {
    const last = db === null ? 'none yet' : lastProviderOutcome(db);
    return unverified(
      'provider',
      `Not probed this run; last worker outcome: ${last}.`,
      'Provider reachability was not verified this run.',
      '`oboete doctor --probe-provider` (one call against the daily allowance)',
    );
  }

  if (db === null) {
    return dbUnread(
      'provider',
      false,
      'The database is unavailable, so the provider could not be probed.',
      'Summaries cannot be checked until storage is open.',
      '`oboete doctor --probe-provider` after storage is repaired.',
    );
  }

  const model = (config.observer.model ?? PRESET_CATALOG[preset].defaultModel).trim();
  const estimate = usageEstimate(db, now);
  const capItem = providerCapItem(preset, estimate);
  if (capItem !== null) return capItem;
  return { kind: 'ready', db, model, estimate };
}

function providerCapItem(
  preset: Exclude<PresetName, 'none'>,
  estimate: ReturnType<typeof usageEstimate>,
): DoctorItem | null {
  if (PRESET_CATALOG[preset].capped && estimate.exhausted) {
    return degraded(
      'provider',
      'provider_exhausted: The provider reported exhaustion today.',
      FALLBACK_CONSEQUENCE,
      `Wait for the reset at ${iso(estimate.resetAt)} or choose another preset with \`oboete setup --provider\`.`,
    );
  }
  if (PRESET_CATALOG[preset].capped && estimate.remaining <= 0) {
    return degraded(
      'provider',
      `daily_cap: The daily cap of ${DAILY_CAP} calls is used up.`,
      FALLBACK_CONSEQUENCE,
      `Wait for the reset at ${iso(estimate.resetAt)} or choose another preset with \`oboete setup --provider\`.`,
    );
  }
  return null;
}

function doctorReserve(
  db: DatabaseSync,
  preset: PresetName,
  now: number,
): { ok: true; reservationId: string } | { ok: false; reason: 'daily_cap' | 'provider_exhausted' } {
  if (!PRESET_CATALOG[preset].capped) {
    return { ok: true, reservationId: randomUUID() };
  }
  return transactionImmediate(db, () => {
    const estimate = usageEstimate(db, now);
    if (estimate.exhausted) return { ok: false, reason: 'provider_exhausted' };
    if (estimate.remaining <= 0) return { ok: false, reason: 'daily_cap' };
    const reservationId = randomUUID();
    recordProviderAttempt(db, { preset, now });
    return { ok: true, reservationId };
  });
}

function lastProviderOutcome(db: DatabaseSync): string {
  try {
    const row = db
      .prepare(
        `SELECT state, degraded_reason, completed_at
         FROM observation_batches
         WHERE destination <> 'fallback'
         ORDER BY COALESCE(completed_at, 0) DESC
         LIMIT 1`,
      )
      .get();
    if (row === undefined) return 'none yet';
    const state = typeof row.state === 'string' ? row.state : 'unknown';
    const reason = typeof row.degraded_reason === 'string' ? row.degraded_reason : 'none';
    const completed = asNumber(row.completed_at);
    return `${state}/${reason}/${completed === null ? 'none' : iso(completed)}`;
  } catch {
    return 'none yet';
  }
}

/** The worker's outcome as one sentence: the detail already names the failure, the code is dropped. */
function outcomeSentence(outcome: Extract<CallOutcome, { ok: false }>): string {
  const detail = outcome.detail.trim().replace(/\.$/u, '');
  const text = detail === '' ? `The provider call failed (${outcome.reason})` : `${detail[0].toUpperCase()}${detail.slice(1)}`;
  return outcome.reason === 'timeout'
    ? `${text} after ${PROVIDER_PROBE_TIMEOUT_MS / 1000} seconds.`
    : `${text}.`;
}

function providerRecovery(
  reason: Extract<CallOutcome, { ok: false }>['reason'],
  config: OboeteConfig,
  paths: OboetePaths,
  env: NodeJS.ProcessEnv,
  resetAt: number,
): string {
  switch (reason) {
    case 'unreachable':
    case 'timeout':
      return `Check the network and the host in ${paths.config}.`;
    case 'auth_failed':
      return (
        credentialSteps(config, env) ||
        'Check the credentials for this preset and run `oboete doctor --probe-provider` again.'
      );
    case 'provider_exhausted':
    case 'daily_cap':
      return `Wait for the reset at ${iso(resetAt)} or choose another preset with \`oboete setup --provider\`.`;
    case 'provider_paid':
      return 'Choose a free model in `[observer] model` or another preset.';
    case 'consent_changed':
      return '`oboete setup --accept-egress`';
    case 'model_alias':
    case 'unusable_output':
      return 'Set `[observer] model` to a model the provider lists.';
    case 'no_provider':
      return '`oboete setup --provider <preset>` (workers-ai is the free remote default; ollama stays local)';
  }
}

function credentialSteps(config: OboeteConfig, env: NodeJS.ProcessEnv): string {
  return credentialGuidance(config, env)
    .filter((line) => /^\s+\d+\./.test(line) || /^\s+Export /.test(line))
    .map((line) => line.trim())
    .join(' ');
}

/**
 * The chain's targets are reported, never probed: `providerItem` spends a real reservation, so one
 * probe per target would spend the daily allowance on diagnostics
 * (contracts/provider-fallback.md "Diagnostics"). No configured chain means no items at all.
 */
export function fallbackItems(
  config: OboeteConfig | null,
  db: DatabaseSync | null,
  integrityFailed: boolean,
  env: NodeJS.ProcessEnv,
  now: number,
): DoctorItem[] {
  if (config === null) return [];
  const entries = config.observer.fallback;
  if (entries.length === 0) return [];
  const chain = admittedChain(config);
  if (chain.error !== null) {
    return [degraded(
      'fallback',
      `Fallback target ${chain.error.position} cannot be used: ${chain.error.code.replace(/_/g, ' ')}.`,
      'The observer runs with no provider at all while the chain is unusable.',
      'Correct the `[[observer.fallback]]` entry in the configuration file, then run `oboete doctor` again.',
    )];
  }
  return entries.map((entry, index) =>
    fallbackTargetItem({ entry, position: index + 1, chain: chain.targets, config, db, integrityFailed, env, now }));
}

function fallbackTargetItem(input: {
  entry: OboeteConfig['observer']['fallback'][number];
  position: number;
  chain: ChainTarget[];
  config: OboeteConfig;
  db: DatabaseSync | null;
  integrityFailed: boolean;
  env: NodeJS.ProcessEnv;
  now: number;
}): DoctorItem {
  const { entry, position, chain, config, db, integrityFailed, env, now } = input;
  const name = `fallback:${position}`;
  const catalog = PRESET_CATALOG[entry.preset];
  const model = (entry.model ?? catalog.defaultModel).trim();
  const where = `Target ${position} is ${entry.preset} with model ${model}`;
  if (!chain.some((target) => target.preset === entry.preset && target.model === model)) {
    return warning(
      name,
      `${where}, which the cost policy does not admit or a nearer target already covers.`,
      'This target is never attempted, so a failure ahead of it falls through to rule-based records.',
      `Add "${catalog.costClass}" to \`[observer] cost_policy\` to admit it, or remove the entry.`,
    );
  }
  const credentials = readCredentials(entry.preset, env, config.observer.agent_cli);
  if (!credentials.present) {
    return warning(
      name,
      `${where}, and its credentials are not set (${credentials.source}).`,
      'The target is attempted and answers without a request, so the chain moves straight past it.',
      'Set that credential in the shell that runs the agents, or remove the entry from the chain.',
    );
  }
  if (db === null) {
    return dbUnread(
      name,
      integrityFailed,
      `${where}, and today's allowance record could not be read.`,
      'Whether this target still has allowance is unknown until storage is open.',
      '`oboete doctor` after storage is repaired.',
    );
  }
  const exhaustedAt = presetExhaustedAt(entry.preset, db, now);
  if (exhaustedAt !== null) {
    return warning(
      name,
      `${where}, and it reported its allowance exhausted at ${iso(exhaustedAt)}.`,
      'The target is skipped at its own reservation until the allowance resets.',
      'Wait for the reset, or reorder the chain so a target with allowance comes first.',
    );
  }
  return healthy(name, `${where}, admitted as ${catalog.costClass} and ready.`);
}

/** Today's per-preset exhaustion stamp, or null when the preset may still be reserved. */
function presetExhaustedAt(preset: PresetName, db: DatabaseSync, now: number): number | null {
  const row = db.prepare('SELECT exhausted_at, reset_at FROM provider_usage WHERE utc_day = ? AND preset = ?')
    .get(utcDay(now), preset);
  const exhaustedAt = asNumber(row?.exhausted_at);
  return exhaustedAt !== null && (asNumber(row?.reset_at) ?? 0) > now ? exhaustedAt : null;
}

export function allowanceItem(
  config: OboeteConfig | null,
  db: DatabaseSync | null,
  integrityFailed: boolean,
  now: number,
): DoctorItem {
  if (config === null) return configUnread('allowance');
  const preset = config.observer.preset;
  if (preset === 'none' || !PRESET_CATALOG[preset].capped) {
    return healthy('allowance', `The ${preset} preset has no daily cap.`);
  }
  if (db === null) {
    return dbUnread(
      'allowance',
      integrityFailed,
      'The database is unavailable, so the daily allowance could not be estimated.',
      'The remaining call count cannot be checked until storage is open.',
      '`oboete doctor` after storage is repaired.',
    );
  }
  return allowanceEstimateItem(preset, db, now);
}

function allowanceEstimateItem(
  preset: Exclude<PresetName, 'none'>,
  db: DatabaseSync,
  now: number,
): DoctorItem {
  try {
    const estimate = usageEstimate(db, now);
    if (estimate.exhausted) {
      return degraded(
        'allowance',
        'The provider reported exhaustion today.',
        ALLOWANCE_CONSEQUENCE,
        `Wait for the reset at ${iso(estimate.resetAt)} or switch preset with \`oboete setup --provider\`.`,
      );
    }
    if (estimate.remaining === 0) {
      return degraded(
        'allowance',
        `The daily cap of ${DAILY_CAP} calls is used up.`,
        ALLOWANCE_CONSEQUENCE,
        `Wait for the reset at ${iso(estimate.resetAt)} or switch preset with \`oboete setup --provider\`.`,
      );
    }
    return healthy(
      'allowance',
      `Estimated ${estimate.remaining} of ${DAILY_CAP} calls remaining today (${estimate.day}); resets at ${iso(estimate.resetAt)}.`,
    );
  } catch (error) {
    return degraded(
      'allowance',
      describe(error),
      ALLOWANCE_CONSEQUENCE,
      '`oboete setup --provider` to switch preset, or wait for the next UTC day.',
    );
  }
}

export function catalogItems(
  config: OboeteConfig | null,
  db: DatabaseSync | null,
  integrityFailed: boolean,
  env: NodeJS.ProcessEnv,
  now: number,
): DoctorItem[] {
  if (config === null) return [configUnread('catalog')];
  if (config.observer.preset !== 'workers-ai') return [];
  if (db === null) {
    return [
      dbUnread(
        'catalog',
        integrityFailed,
        'The database is unavailable, so the cached catalog could not be read.',
        'The configured model cannot be checked against the provider list until storage is open.',
        '`oboete doctor` after storage is repaired.',
      ),
    ];
  }
  const cache = cachedCatalog(db);
  if (cache === null) {
    return [
      unverified(
        'catalog',
        'No catalog cached yet; the worker fetches it on the first batch.',
        'The configured model has not been checked against the provider list this run.',
        '`oboete observe` fetches the catalog on the first batch.',
      ),
    ];
  }
  return catalogCacheItems(config, cache, env, now);
}

function catalogCacheItems(
  config: OboeteConfig,
  cache: NonNullable<ReturnType<typeof cachedCatalog>>,
  env: NodeJS.ProcessEnv,
  now: number,
): DoctorItem[] {
  const accountId = readCredentials('workers-ai', env).values.accountId ?? '';
  if (cache.accountId !== accountId) {
    return [
      unverified(
        'catalog',
        'The cached catalog belongs to another account; the worker refreshes it on the next batch.',
        'The configured model has not been checked against the provider list this run.',
        '`oboete observe` fetches the catalog on the first batch.',
      ),
    ];
  }
  if (now < cache.fetchedAt || now - cache.fetchedAt >= CACHE_MS) {
    return [
      unverified(
        'catalog',
        'The cached catalog is stale; the worker refreshes it on the next batch.',
        'The configured model has not been checked against the provider list this run.',
        '`oboete observe` fetches the catalog on the first batch.',
      ),
    ];
  }
  return catalogModelItems(config, cache);
}

function catalogModelItems(
  config: OboeteConfig,
  cache: NonNullable<ReturnType<typeof cachedCatalog>>,
): DoctorItem[] {
  const configured = (config.observer.model ?? PRESET_CATALOG['workers-ai'].defaultModel).trim();
  if (!cache.models.includes(configured)) {
    return [
      degraded(
        'catalog',
        `The configured model is not in the catalog of ${cache.models.length} models fetched ${iso(cache.fetchedAt)}.`,
        'Summaries fall back to rule-based until `[observer] model` names a listed model.',
        'Set `[observer] model` to a listed model.',
      ),
    ];
  }
  if (cache.hasPaidOnlyModels) {
    return [
      warning(
        'catalog',
        `The catalog lists models that need a paid Workers plan; the configured model ${configured} is only used if it is free.`,
        'A paid-only model will fail with provider_paid and fall back to rule-based summaries.',
        'Keep `[observer] model` on a free model.',
      ),
    ];
  }
  return [
    healthy(
      'catalog',
      `The catalog of ${cache.models.length} models fetched ${iso(cache.fetchedAt)} includes the configured model.`,
    ),
  ];
}
