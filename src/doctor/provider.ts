import { randomUUID } from 'node:crypto';
import type { DatabaseSync } from 'node:sqlite';

import {
  PRESET_CATALOG,
  admittedChain,
  type ChainVerdict,
  consentMatches,
  readCredentials,
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
import { CHAIN_STOPS } from '../observer/classify.js';
import { chainErrorMessage, resolveModel } from '../observer/providers.js';
import type { ObserverInput } from '../observer/contract.js';
import { summarizeWithProvider, type CallOutcome } from '../observer/llm.js';
import {
  DAILY_CAP,
  SESSION_END_RESERVE,
  presetExhaustedAt,
  recordExhausted,
  recordProviderAttempt,
  usageEstimate,
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
// "Offered", never "summarized": admission is not runnability. A target may lack its own credential
// or its own allowance, and then the batch is rule-based after all — which is why every target's own
// verdict is reported as `fallback:N` rather than summarized into this one sentence.
const CHAINED_CONSEQUENCE =
  'This target answers without a request, so every batch is offered to the fallback chain below.';

/**
 * What a refused primary means for the queue. An admitted chain is attempted on the same batch
 * (contracts/provider-fallback.md "Advance and stop": `daily_cap` and `provider_exhausted` both
 * advance), so an item that says processing waits contradicts both the worker and the healthy
 * target reported below it. The condition is the one the uncredentialed-primary branch already
 * uses, so the two cannot disagree.
 */
function refusedPrimaryConsequence(
  config: OboeteConfig,
  whenChained: string,
  // The reserved band is the one refusal where the primary still serves something, so it passes the
  // clause's own sentence rather than the queue-waits default.
  otherwise: string = FALLBACK_CONSEQUENCE,
): string {
  return admittedChain(config).targets.length > 0 ? whenChained : otherwise;
}
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
  const { config: readyConfig, preset, credentials, model } = configured;
  const probe = providerProbeReadiness(readyConfig, preset, db, options, now);
  if (!('kind' in probe)) return probe;
  const { db: openDb, estimate } = probe;

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
      // A probe failure the chain advances past is not the queue waiting: the worker hands the same
      // batch to the admitted target this report calls healthy a few lines below. `CHAIN_STOPS` is
      // the worker's own set, not a copy (src/observer/classify.ts).
      CHAIN_STOPS.has(outcome.reason)
        ? FALLBACK_CONSEQUENCE
        : refusedPrimaryConsequence(
            readyConfig,
            'This failure advances the chain, so the batch is offered to the fallback targets below.',
          ),
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
  /** From `resolveModel`, so the probe and the worker never derive the primary's model apart. */
  model: string;
};

type ProviderProbeReadiness =
  | DoctorItem
  | { kind: 'ready'; db: DatabaseSync; estimate: ReturnType<typeof usageEstimate> };

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

  const resolved = resolvedObserver(
    config,
    'provider',
    'Summaries come from the rule-based fallback only (packs say `Degraded:`).',
    'Set `[observer] model` in the configuration file to a model that preset accepts, or correct the `[[observer.fallback]]` entry, then run `oboete doctor` again.',
  );
  if (!('kind' in resolved)) return resolved;

  const credentials = readCredentials(preset, env, config.observer.agent_cli);
  if (!credentials.present) {
    // An uncredentialed primary is one failed target, not a run without a provider: the chain is
    // still attempted (contracts/provider-fallback.md "What the chain does not do").
    return degraded(
      'provider',
      `No credentials are set for the ${preset} preset (${credentials.source}).`,
      admittedChain(config).targets.length > 0
        ? CHAINED_CONSEQUENCE
        : 'Summaries come from the rule-based fallback only (packs say `Degraded:`).',
      credentialSteps(config, env) ||
        '`oboete setup --provider <preset>` (workers-ai is the free remote default; ollama stays local)',
    );
  }
  return { kind: 'configured', config, preset, credentials, model: resolved.model };
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

  const estimate = usageEstimate(db, now);
  const capItem = providerCapItem(preset, estimate, db, now, config);
  if (capItem !== null) return capItem;
  return { kind: 'ready', db, estimate };
}

/**
 * What the shared allowance still admits. `reserveAttempt` holds the last `SESSION_END_RESERVE`
 * calls for `session_end` triggers, so a surface that stops at `remaining === 0` calls a provider
 * ready while every `ten_turns` and `retention` batch is already being refused — the same shape as
 * a target reported ready that cannot be attempted.
 */
function sharedAllowance(estimate: ReturnType<typeof usageEstimate>): 'open' | 'reserved' | 'spent' {
  if (estimate.remaining === 0) return 'spent';
  return estimate.remaining <= SESSION_END_RESERVE ? 'reserved' : 'open';
}

/**
 * The three sentences the two allowance surfaces share once it is no longer open. The reserved band
 * gets its own consequence and recovery because end-of-session summaries still run in it: saying
 * processing waits for the reset would be false for the batches that are still served.
 */
function allowanceClause(
  state: 'reserved' | 'spent',
  estimate: ReturnType<typeof usageEstimate>,
  resetAt: string,
  config: OboeteConfig,
): { reason: string; consequence: string; recovery: string } {
  return state === 'spent'
    ? {
      reason: `The daily cap of ${DAILY_CAP} calls is used up.`,
      // "Offered" rather than "summarized": the cap is shared across capped presets, so a capped
      // target refuses at its own reservation too (contracts/provider-fallback.md "Advance and
      // stop", `daily_cap`). Only an uncapped target actually answers, and `fallback:N` is where
      // each target's own allowance is reported.
      consequence: refusedPrimaryConsequence(
        config,
        'Batches are offered to the fallback chain below; a capped target there shares this allowance.',
        ALLOWANCE_CONSEQUENCE,
      ),
      recovery: `Wait for the reset at ${resetAt} or switch preset with \`oboete setup --provider\`.`,
    }
    : {
      reason: `Only ${estimate.remaining} of the daily ${DAILY_CAP} calls are left, and they are held for end-of-session batches.`,
      consequence: refusedPrimaryConsequence(
        config,
        'End-of-session summaries still run on this preset; every other batch is offered to the fallback chain below.',
        'End-of-session summaries still run; ten-turn and retention batches wait for the allowance to reset, and later worker runs retry due sources.',
      ),
      recovery: `Wait for the reset at ${resetAt} for the other batches, or switch preset with \`oboete setup --provider\`.`,
    };
}

function providerCapItem(
  preset: Exclude<PresetName, 'none'>,
  estimate: ReturnType<typeof usageEstimate>,
  db: DatabaseSync,
  now: number,
  config: OboeteConfig,
): DoctorItem | null {
  const consequence = refusedPrimaryConsequence(
    config,
    'This reservation is refused without a request, so the batch is offered to the fallback chain below.',
  );
  // Not behind `capped`: `reserveAttempt` refuses on this stamp whatever the preset's cap is, and
  // `doctorReserve` now does too — without this the probe is still stopped, but it is reported as a
  // refused reservation rather than as the exhaustion it is.
  if (presetExhaustedAt(db, preset, now) !== null) {
    return degraded(
      'provider',
      'provider_exhausted: The provider reported exhaustion today.',
      consequence,
      `Wait for the reset at ${iso(estimate.resetAt)} or choose another preset with \`oboete setup --provider\`.`,
    );
  }
  const shared = sharedAllowance(estimate);
  if (PRESET_CATALOG[preset].capped && shared !== 'open') {
    const clause = allowanceClause(shared, estimate, iso(estimate.resetAt), config);
    return degraded(
      'provider',
      `daily_cap: ${clause.reason}`,
      // In the reserved band `reserveAttempt` still grants a `session_end` batch this preset, so
      // neither "processing waits" nor "the chain takes it" is true of every batch — the clause's
      // own sentence is, and it is chain-aware. The spent band keeps this item's own sentence
      // because it is reporting a refused probe reservation rather than the shared allowance. The
      // recovery is the clause's too: the copy that stood here said the same thing in other words.
      shared === 'reserved' ? clause.consequence : consequence,
      clause.recovery,
    );
  }
  return null;
}

function doctorReserve(
  db: DatabaseSync,
  preset: PresetName,
  now: number,
): { ok: true; reservationId: string } | { ok: false; reason: 'daily_cap' | 'provider_exhausted' } {
  if (presetExhaustedAt(db, preset, now) !== null) return { ok: false, reason: 'provider_exhausted' };
  if (!PRESET_CATALOG[preset].capped) {
    return { ok: true, reservationId: randomUUID() };
  }
  return transactionImmediate(db, () => {
    if (presetExhaustedAt(db, preset, now) !== null) return { ok: false, reason: 'provider_exhausted' };
    // The probe takes a real reservation, so it must not spend the calls held for session ends.
    if (sharedAllowance(usageEstimate(db, now)) !== 'open') return { ok: false, reason: 'daily_cap' };
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
 * The worker's own resolver, asked once and answered with what it resolved: a primary whose model
 * does not resolve, or a chain entry that makes the chain unusable, leaves the observer with no
 * model and no targets at all, so neither the provider item nor any target below may be reported as
 * ready, and both say why (contracts/provider-fallback.md "What the chain does not do"). The
 * `kind` tag tells the two answers apart, as it does for `ConfiguredProvider`.
 */
function resolvedObserver(
  config: OboeteConfig,
  name: 'provider' | 'fallback',
  consequence: string,
  recovery: string,
): { kind: 'resolved'; model: string } | DoctorItem {
  try {
    return { kind: 'resolved', model: resolveModel(config).model };
  } catch (error) {
    return degraded(name, describe(error), consequence, recovery);
  }
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
      chainErrorMessage(chain.error),
      'The observer runs with no provider at all while the chain is unusable.',
      chain.error.code === 'chain_without_primary'
        ? '`oboete setup --provider <preset>`, or remove the `[[observer.fallback]]` entries.'
        : 'Correct the `[[observer.fallback]]` entry in the configuration file, then run `oboete doctor` again.',
    )];
  }
  // Only the primary's own model can reach this: a chain error returned above. `resolveModel`
  // checks the primary before the chain, so the message is always about the preset.
  const resolved = resolvedObserver(
    config,
    'fallback',
    'No target below is ever attempted: the observer has no usable primary, so every batch is rule-based.',
    'Set `[observer] model` in the configuration file to a model that preset accepts, then run `oboete doctor` again.',
  );
  if (!('kind' in resolved)) return [resolved];
  return entries.map((entry, index) => fallbackTargetItem({
    entry, position: index + 1, verdict: chain.verdicts[index] ?? 'excluded',
    config, db, integrityFailed, env, now,
  }));
}

type FallbackTarget = {
  entry: OboeteConfig['observer']['fallback'][number];
  position: number;
  verdict: ChainVerdict;
  config: OboeteConfig;
  db: DatabaseSync | null;
  integrityFailed: boolean;
  env: NodeJS.ProcessEnv;
  now: number;
};

function fallbackTargetItem(input: FallbackTarget): DoctorItem {
  const { entry, position, verdict, config, db, integrityFailed, env, now } = input;
  const name = `fallback:${position}`;
  const catalog = PRESET_CATALOG[entry.preset];
  const model = (entry.model ?? catalog.defaultModel).trim();
  const where = `Target ${position} is ${entry.preset} with model ${model}`;
  if (verdict === 'covered') {
    // Adding the cost class cannot make a duplicate runnable, so this verdict must not recommend it.
    return warning(
      name,
      `${where}, which a nearer target already covers.`,
      'This entry is never attempted on its own, because the target ahead of it is the same one.',
      'Remove the entry, or point it at another preset or model.',
    );
  }
  if (verdict === 'excluded') {
    return warning(
      name,
      `${where}, whose "${catalog.costClass}" cost class \`[observer] cost_policy\` does not admit.`,
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
  if (catalog.credential.kind === 'agent-login') {
    // `readCredentials` calls an agent login present because `setup` is what verifies it; this
    // item does not, so it must not call the target ready either.
    return unverified(
      name,
      `${where}, and whether the ${config.observer.agent_cli} login is live is not checked here.`,
      'A target whose subscription is not logged in fails its attempt and the chain moves past it.',
      '`oboete setup` reports the login state of each agent command line tool.',
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
  return fallbackAllowanceItem(name, where, entry.preset, catalog, db, now);
}

/**
 * The allowance half of a target's verdict: its own exhaustion stamp first, then the allowance all
 * capped presets share — which `allowanceItem` only reports when the *primary* is capped, so a
 * capped target under an uncapped primary has no other surface to say it.
 */
function fallbackAllowanceItem(
  name: string,
  where: string,
  preset: PresetName,
  catalog: (typeof PRESET_CATALOG)[PresetName],
  db: DatabaseSync,
  now: number,
): DoctorItem {
  const exhaustedAt = presetExhaustedAt(db, preset, now);
  if (exhaustedAt !== null) {
    return warning(
      name,
      `${where}, and it reported its allowance exhausted at ${iso(exhaustedAt)}.`,
      'The target is skipped at its own reservation until the allowance resets.',
      'Wait for the reset, or reorder the chain so a target with allowance comes first.',
    );
  }
  const estimate = usageEstimate(db, now);
  const shared = sharedAllowance(estimate);
  if (catalog.capped && shared !== 'open') {
    return warning(
      name,
      shared === 'spent'
        ? `${where}, and today's shared allowance is spent (${estimate.calls} of ${DAILY_CAP} calls).`
        : `${where}, and only ${estimate.remaining} of today's ${DAILY_CAP} shared calls are left, held for end-of-session batches.`,
      shared === 'spent'
        ? 'Every capped target refuses at its own reservation until the allowance resets.'
        : 'Every capped target refuses a ten-turn or retention batch at its own reservation; an end-of-session batch is still served.',
      `Wait for the reset at ${iso(estimate.resetAt)}, or add an uncapped target to the chain.`,
    );
  }
  return healthy(name, `${where}, admitted as ${catalog.costClass} and ready.`);
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
  return allowanceEstimateItem(preset, db, now, config);
}

function allowanceEstimateItem(
  preset: Exclude<PresetName, 'none'>,
  db: DatabaseSync,
  now: number,
  config: OboeteConfig,
): DoctorItem {
  try {
    const estimate = usageEstimate(db, now);
    if (presetExhaustedAt(db, preset, now) !== null) {
      return degraded(
        'allowance',
        'The provider reported exhaustion today.',
        // `exhausted_at` is per preset, so the chain's next target is unaffected and the worker
        // advances past `provider_exhausted` — the same reading the cap branch below takes.
        refusedPrimaryConsequence(
          config,
          'Batches are offered to the fallback chain below; a capped target there shares this allowance.',
          ALLOWANCE_CONSEQUENCE,
        ),
        `Wait for the reset at ${iso(estimate.resetAt)} or switch preset with \`oboete setup --provider\`.`,
      );
    }
    const shared = sharedAllowance(estimate);
    if (shared !== 'open') {
      const clause = allowanceClause(shared, estimate, iso(estimate.resetAt), config);
      return degraded('allowance', clause.reason, clause.consequence, clause.recovery);
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
