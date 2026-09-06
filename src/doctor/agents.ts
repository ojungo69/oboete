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

  if (!agent.installed) return healthy(name, 'Not installed.');

  if (markersMissing) {
    return degraded(
      name,
      `${label} rewrote its config.toml and dropped the oboete markers; the MCP table is still there.`,
      `${label} capture and injection have not been verified for this configuration.`,
      `Run ${setupRecovery}.`,
    );
  }

  if (hookMissing(agent)) {
    const reason =
      agent.agent === 'codex' && agent.trust === 'untrusted'
        ? `Codex has not trusted the hook definition in ${agent.configPath}.`
        : `No oboete hook in ${agent.configPath}.`;
    return degraded(name, reason, captureConsequence, setupRecovery);
  }

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

  const result = results.get(agent.agent);
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
    default: {
      const exit = /^agent_exit_(-?\d+)$/.exec(code);
      return exit
        ? `${label} exited with code ${exit[1]} before the probe finished.`
        : `The ${label} probe could not be verified.`;
    }
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

  const hangs: number[] = [];
  if (existsSync(paths.piAck)) {
    for (const name of readdirSync(paths.piAck)) {
      if (!name.endsWith('.started')) continue;
      try {
        const age = now - statSync(join(paths.piAck, name)).mtimeMs;
        if (age > PI_HANG_AFTER_MS) hangs.push(age);
      } catch {
        // The ack file was removed while we listed the directory.
      }
    }
  }

  const diag = db === null ? [] : piDiagnostics(db);
  if (hangs.length > 0) {
    const oldest = Math.max(...hangs);
    const seconds = Math.max(0, Math.round(oldest / 1000));
    const listing = diag.length > 0 ? ` ${diag.join(', ')}.` : '';
    return degraded(
      'pi',
      `${hangs.length} Pi capture children never finished (oldest ${seconds} seconds ago). pi_child_hang.${listing}`,
      'Those Pi turns were not captured.',
      `Delete the \`.started\` files under ${paths.piAck} and run \`oboete observe\`; if it recurs, run \`oboete setup --agents pi\`.`,
    );
  }
  if (diag.length > 0) {
    return warning(
      'pi',
      diag.join(', '),
      'Those Pi turns were not captured.',
      `Delete the \`.started\` files under ${paths.piAck} and run \`oboete observe\`; if it recurs, run \`oboete setup --agents pi\`.`,
    );
  }
  return healthy('pi', 'No Pi diagnostics.');
}

function piDiagnostics(db: DatabaseSync): string[] {
  try {
    const rows = db
      .prepare(
        `SELECT message_code, SUM(count) AS count, MAX(last_seen_at) AS last_seen_at
         FROM diagnostics
         WHERE agent = 'pi' AND cleared_at IS NULL
         GROUP BY message_code`,
      )
      .all();
    return rows.flatMap((row) => {
      const code = typeof row.message_code === 'string' ? row.message_code : '';
      if (code === '') return [];
      const last = asNumber(row.last_seen_at);
      const count = countOf(row, 'count');
      return [
        `${code} ${count} times (last seen ${last === null ? 'unknown' : iso(last)})`,
      ];
    });
  } catch {
    return [];
  }
}

function sanitizeDisplayName(value: string): string {
  let out = '';
  for (let i = 0; i < value.length && out.length < 64; i += 1) {
    const code = value.charCodeAt(i);
    if (code === 27) {
      const next = value[i + 1];
      if (next === '[') {
        i += 2;
        while (i < value.length) {
          const end = value.charCodeAt(i);
          if (end >= 64 && end <= 126) break;
          i += 1;
        }
      } else {
        i += 1;
      }
      continue;
    }
    if (code <= 31 || (code >= 127 && code <= 159)) continue;
    out += value[i];
  }
  return out;
}
