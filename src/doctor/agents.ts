import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';
import type { DatabaseSync } from 'node:sqlite';
import { stringify as stringifyToml } from 'smol-toml';

import {
  asNumber,
  countOf,
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
import type { OboetePaths } from '../paths.js';
import {
  detectAgents,
  type AgentDetection,
  type SetupAgent,
} from '../setup/detect.js';
import { hasUnmarkedTomlBlock, isOboeteOwned, isPlainObject, readOboeteMcp } from '../setup/managed-block.js';
import { probeEventStored, runProbes, type ProbeResult } from '../setup/probe.js';
import { SETUP_RESULT_KEY } from '../setup/setup.js';
import { PI_HANG_AFTER_MS, runtimeStateGet } from '../worker/purge.js';

const AGENT_LABEL: Record<SetupAgent, string> = {
  claude: 'Claude',
  codex: 'Codex',
  grok: 'Grok',
  pi: 'Pi',
};

const AGENTS: readonly SetupAgent[] = ['claude', 'codex', 'grok', 'pi'];

export async function agentItems(
  db: DatabaseSync | null,
  integrityFailed: boolean,
  deps: DoctorDeps,
  options: DoctorOptions,
): Promise<DoctorItem[]> {
  try {
    const detected = detectAgents(deps.env, deps.versionSpawn);
    const missingMarkers = new Set(detected.filter(configMarkersMissing).map(({ agent }) => agent));
    const targets = detected.filter(
      (agent) =>
        agent.installed &&
        !missingMarkers.has(agent.agent) &&
        !hookMissing(agent) &&
        !options.noProbeAgents &&
        db !== null &&
        agent.cliPath !== null,
    );
    const results = new Map<SetupAgent, ProbeResult>();
    if (targets.length > 0 && db !== null) {
      const probed = await runProbes(
        targets.map(({ agent, cliPath, configPath }) => ({ agent, cliPath, configPath })),
        {
          spawn: deps.spawn,
          lookupProbe: (agent, marker) => probeEventStored(db, agent, marker),
          env: deps.env,
        },
      );
      for (const result of probed) results.set(result.agent, result);
    }

    const items: DoctorItem[] = [];
    for (const agent of detected) {
      items.push(oneAgentItem(agent, db, integrityFailed, options, results, missingMarkers.has(agent.agent)));
      if (agent.nativeMemory !== null) {
        items.push(
          warning(
            `native-memory:${agent.agent}`,
            `${agent.agent}: its own memory feature (${agent.nativeMemory}) is enabled. oboete neither reads it nor changes it; the two run side by side.`,
            'Both systems record memory independently.',
            'No action is required; doctor does not change that setting.',
          ),
        );
      }
    }
    return items;
  } catch (error) {
    return AGENTS.map((agent) => failedItem(`agent:${agent}`, error));
  }
}

function oneAgentItem(
  agent: AgentDetection,
  db: DatabaseSync | null,
  integrityFailed: boolean,
  options: DoctorOptions,
  results: ReadonlyMap<SetupAgent, ProbeResult>,
  markersMissing: boolean,
): DoctorItem {
  const name = `agent:${agent.agent}`;
  const label = AGENT_LABEL[agent.agent];
  const captureConsequence = `${label} sessions capture nothing and receive no memories.`;
  const setupRecovery = `\`oboete setup --agents ${agent.agent}\``;
  const context = { name, label, captureConsequence, setupRecovery };

  const preProbe = preProbeAgentItem(agent, db, integrityFailed, options, markersMissing, context);
  if (preProbe !== null) return preProbe;
  const result = results.get(agent.agent);
  return probedAgentItem(agent, result, context);
}

type AgentItemContext = {
  name: string;
  label: string;
  captureConsequence: string;
  setupRecovery: string;
};

function preProbeAgentItem(
  agent: AgentDetection,
  db: DatabaseSync | null,
  integrityFailed: boolean,
  options: DoctorOptions,
  markersMissing: boolean,
  context: AgentItemContext,
): DoctorItem | null {
  const { name, label, captureConsequence, setupRecovery } = context;
  if (!agent.installed) return healthy(name, 'Not installed.');
  if (markersMissing) {
    return degraded(
      name,
      `${label} rewrote its config.toml and dropped the oboete markers; the MCP table is still there.`,
      `${label} capture and injection have not been verified for this configuration.`,
      `Run ${setupRecovery}.`,
    );
  }
  const missingHook = missingHookItem(agent, context);
  if (missingHook !== null) return missingHook;
  if (db === null) {
    return dbUnread(
      name,
      integrityFailed,
      'The database is unavailable, so the hook probe could not be verified.',
      captureConsequence,
      '`oboete doctor` after storage is repaired.',
    );
  }
  if (options.noProbeAgents) {
    return unverified(
      name,
      `Not probed this run; last setup result: ${lastSetupResult(db, agent.agent)}.`,
      `${label} capture and injection were not verified this run.`,
      '`oboete doctor` (without --no-probe-agents)',
    );
  }
  if (agent.cliPath === null) {
    return unverified(
      name,
      `The ${agent.agent} executable is not on the PATH, so the hook could not be probed.`,
      `${label} capture and injection were not verified this run.`,
      `Install the ${label} CLI on the PATH, then run \`oboete doctor\`.`,
    );
  }
  return null;
}

function missingHookItem(agent: AgentDetection, context: AgentItemContext): DoctorItem | null {
  if (hookMissing(agent)) {
    const reason =
      agent.agent === 'codex' && agent.trust === 'untrusted'
        ? `Codex has not trusted the hook definition in ${agent.configPath}.`
        : `No oboete hook in ${agent.configPath}.`;
    return degraded(context.name, reason, context.captureConsequence, context.setupRecovery);
  }
  return null;
}

function probedAgentItem(
  agent: AgentDetection,
  result: ProbeResult | undefined,
  context: AgentItemContext,
): DoctorItem {
  const { name, label, captureConsequence, setupRecovery } = context;
  if (result === undefined) {
    return unverified(
      name,
      'The hook probe did not run this time.',
      `${label} capture and injection were not verified this run.`,
      '`oboete doctor` (without --no-probe-agents)',
    );
  }
  if (result.status === 'pass') {
    return healthy(
      name,
      `The hook fired and the event was stored (${result.elapsedMs} milliseconds); trust: ${agent.trust}.`,
    );
  }
  if (result.status === 'not_installed') {
    return unverified(
      name,
      `The ${agent.agent} executable is not on the PATH, so the hook could not be probed.`,
      `${label} capture and injection were not verified this run.`,
      `Install the ${label} CLI on the PATH, then run \`oboete doctor\`.`,
    );
  }

  return degraded(
    name,
    probeReason(label, result.reason),
    captureConsequence,
    `${setupRecovery}; if it still fails, run the agent once by hand and read its own error output.`,
  );
}

function configMarkersMissing(agent: AgentDetection): boolean {
  if (agent.agent !== 'grok' && agent.agent !== 'codex') return false;
  const home = agent.agent === 'grok' ? dirname(dirname(agent.configPath)) : dirname(agent.configPath);
  const config = join(home, 'config.toml');
  const mcp = readOboeteMcp(config, agent.configPath, agent.agent === 'grok' ? 'claude-or-grok' : 'codex');
  return mcp !== null && hasUnmarkedTomlBlock(config, stringifyToml({ mcp_servers: { oboete: mcp } }));
}

export function probeReason(label: string, code: string): string {
  switch (code) {
    case 'agent_not_installed':
      return `${label} is not installed, so the probe could not run.`;
    case 'spawn_failed':
      return `${label} could not be started for the probe.`;
    case 'probe_event_stored':
      return `${label} ran and its capture event reached oboete.`;
    case 'probe_lookup_failed':
      return `The capture event from ${label} could not be checked in the oboete database.`;
    case 'probe_event_missing':
      return `${label} ran but no capture event reached oboete.`;
    case 'agent_exit_signal':
      return `${label} was stopped by a signal before the probe finished.`;
    case 'deadline_exceeded':
      return `${label} did not finish the probe within 90 seconds.`;
    default:
      return agentExitReason(label, code);
  }
}

function hookMissing(agent: AgentDetection): boolean {
  if (agent.agent === 'codex') return agent.trust === 'absent' || agent.trust === 'untrusted';
  if (agent.agent === 'claude') return !claudeHookPresent(agent.configPath);
  return agent.trust === 'absent';
}

function claudeHookPresent(configPath: string): boolean {
  try {
    const parsed: unknown = JSON.parse(readFileSync(configPath, 'utf8'));
    if (!isPlainObject(parsed)) return false;
    const hooks = parsed.hooks;
    if (!isPlainObject(hooks)) return false;
    return Object.values(hooks).some((entries) => Array.isArray(entries) && entries.some(isOboeteOwned));
  } catch {
    return false;
  }
}

function lastSetupResult(db: DatabaseSync | null, agent: SetupAgent): string {
  if (db === null) return 'none';
  try {
    const raw = runtimeStateGet(db, SETUP_RESULT_KEY);
    if (raw === undefined) return 'none';
    const parsed: unknown = JSON.parse(raw);
    if (!isPlainObject(parsed) || !Array.isArray(parsed.agents)) return 'none';
    const row = parsed.agents.find(
      (entry) => isPlainObject(entry) && entry.agent === agent,
    ) as { probe?: unknown; trust?: unknown } | undefined;
    if (row === undefined) return 'none';
    return `${String(row.probe ?? 'none')}/${String(row.trust ?? 'none')}`;
  } catch {
    return 'none';
  }
}

export function unrecognizedItem(db: DatabaseSync | null, integrityFailed: boolean): DoctorItem {
  if (db === null) {
    return dbUnread(
      'unrecognized-agents',
      integrityFailed,
      'The database is unavailable, so unrecognized-agent diagnostics could not be read.',
      'Invocations from an unsupported agent cannot be checked until storage is open.',
      '`oboete doctor` after storage is repaired.',
    );
  }
  try {
    const rows = db
      .prepare(
        `SELECT message_code, count, last_seen_at
         FROM diagnostics
         WHERE kind = 'unknown_agent' AND cleared_at IS NULL`,
      )
      .all();
    if (rows.length === 0) return healthy('unrecognized-agents', 'No invocation from an unrecognized agent.');
    let total = 0;
    let lastCode = 'unknown';
    let lastSeen = 0;
    for (const row of rows) {
      total += countOf(row, 'count');
      const seen = asNumber(row.last_seen_at) ?? 0;
      if (seen >= lastSeen) {
        lastSeen = seen;
        lastCode = typeof row.message_code === 'string' ? row.message_code : 'unknown';
      }
    }
    const eventName = sanitizeDisplayName(lastCode) || 'unknown';
    return warning(
      'unrecognized-agents',
      `${total} invocations from an unrecognized agent (last event name: ${eventName}, last seen ${iso(lastSeen)}).`,
      'Those events were not captured.',
      '`oboete setup` rewrites the handlers; if the agent is not one of the four, it is not supported in M1.',
    );
  } catch (error) {
    return failedItem('unrecognized-agents', error);
  }
}

export function piItem(
  paths: OboetePaths,
  db: DatabaseSync | null,
  integrityFailed: boolean,
  now: number,
): DoctorItem {
  if (integrityFailed) {
    return dbUnread(
      'pi',
      true,
      'The database is unavailable, so Pi diagnostics could not be read.',
      'Those Pi turns were not captured.',
      `Delete the \`.started\` files under ${paths.piAck} and run \`oboete observe\`; if it recurs, run \`oboete setup --agents pi\`.`,
    );
  }

  const hangs = piHangAges(paths, now);
  const diag = db === null ? [] : piDiagnostics(db, now);
  if (hangs.length > 0) {
    const oldest = Math.max(...hangs);
    const seconds = Math.max(0, Math.round(oldest / 1000));
    const listing = diag.length > 0 ? ` ${diag.join('. ')}.` : '';
    return degraded(
      'pi',
      `${hangs.length} Pi capture children never finished (oldest ${seconds} seconds ago), which is pi_child_hang.${listing}`,
      'Those Pi turns were not captured.',
      `Delete the \`.started\` files under ${paths.piAck} and run \`oboete observe\`; if it recurs, run \`oboete setup --agents pi\`.`,
    );
  }
  if (diag.length > 0) {
    return warning(
      'pi',
      `${diag.join('. ')}; the files are gone, this is the history.`,
      'Those Pi turns were not captured.',
      `Delete the \`.started\` files under ${paths.piAck} and run \`oboete observe\`; if it recurs, run \`oboete setup --agents pi\`.`,
    );
  }
  return healthy('pi', 'No Pi diagnostics.');
}

function piHangAges(paths: OboetePaths, now: number): number[] {
  if (!existsSync(paths.piAck)) return [];
  const hangs: number[] = [];
  for (const name of readdirSync(paths.piAck)) {
    if (!name.endsWith('.started')) continue;
    try {
      const age = now - statSync(join(paths.piAck, name)).mtimeMs;
      if (age > PI_HANG_AFTER_MS) hangs.push(age);
    } catch {
      // The ack file was removed while we listed the directory.
    }
  }
  return hangs;
}

/** Diagnostics of the last 24 hours (data-model: the `.started` files themselves are kept that long). */
const DIAGNOSTICS_WINDOW_MS = 24 * 60 * 60 * 1000;

function piDiagnostics(db: DatabaseSync, now: number): string[] {
  try {
    const rows = db
      .prepare(
        `SELECT message_code, SUM(count) AS count, MAX(last_seen_at) AS last_seen_at
         FROM diagnostics
         WHERE agent = 'pi' AND cleared_at IS NULL AND last_seen_at > ?
         GROUP BY message_code`,
      )
      .all(now - DIAGNOSTICS_WINDOW_MS);
    return rows.flatMap((row) => {
      const code = typeof row.message_code === 'string' ? row.message_code : '';
      if (code === '') return [];
      const last = asNumber(row.last_seen_at);
      const count = countOf(row, 'count');
      const when = last === null ? 'at an unknown time' : `last at ${iso(last)}`;
      const frequency = count === 1 ? 'once' : `${count} times`;
      return [`The worker recorded ${code} ${frequency}, ${when}`];
    });
  } catch {
    return [];
  }
}

function sanitizeDisplayName(value: string): string {
  let out = '';
  for (let i = 0; i < value.length && out.length < 64; i += 1) {
    const code = value.codePointAt(i)!;
    if (code === 27) {
      i = escapeSequenceEnd(value, i);
      continue;
    }
    if (code <= 31 || (code >= 127 && code <= 159)) continue;
    out += value[i];
  }
  return out;
}

function escapeSequenceEnd(value: string, i: number): number {
  const next = value[i + 1];
  if (next === '[') {
    i += 2;
    i = csiSequenceEnd(value, i);
  } else {
    i += 1;
  }
  return i;
}

function csiSequenceEnd(value: string, i: number): number {
  while (i < value.length) {
    const end = value.codePointAt(i)!;
    if (end >= 64 && end <= 126) break;
    i += 1;
  }
  return i;
}

function agentExitReason(label: string, code: string): string {
  const exit = /^agent_exit_(-?\d+)$/.exec(code);
  return exit
    ? `${label} exited with code ${exit[1]} before the probe finished.`
    : `The ${label} probe could not be verified.`;
}
