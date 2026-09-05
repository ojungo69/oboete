import type { ChildProcess, spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { once } from 'node:events';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { performance } from 'node:perf_hooks';
import type { DatabaseSync } from 'node:sqlite';
import { setTimeout as delay } from 'node:timers/promises';

import { childEnvironment } from '../log.js';
import type { AgentDetection, SetupAgent } from './detect.js';

const PROBE_DEADLINE_MS = 90_000;
const PROBE_POLL_INTERVAL_MS = 200;

/** Only the CLI path decides whether a probe can run: a leftover home has nothing to verify. */
export type ProbeTarget = Pick<AgentDetection, 'agent' | 'cliPath' | 'configPath'>;
export type ProbeStatus = 'pass' | 'fail' | 'timeout' | 'not_installed';
export type ProbeResult = {
  agent: SetupAgent;
  status: ProbeStatus;
  elapsedMs: number;
  reason: string;
};

export type ProbeDeps = {
  spawn: typeof spawn;
  lookupProbe(agent: SetupAgent, marker: string): boolean | Promise<boolean>;
  env: NodeJS.ProcessEnv;
  now?: () => number;
};

type Invocation = {
  command: string;
  args: string[];
  env: NodeJS.ProcessEnv;
  cwd: string;
};

type ProcessOutcome =
  | { kind: 'closed'; code: number | null }
  | { kind: 'error'; error: unknown };

/**
 * The probe only needs one turn with no tool, so it runs with the agent's own guardrails on and in
 * a throwaway working directory instead of the developer's tree (FR-031). Codex has no approval
 * flag on `exec` (codex-cli 0.153.2), so the sandbox is the guardrail pinned here.
 */
function invocation(
  target: ProbeTarget,
  cliPath: string,
  marker: string,
  deps: ProbeDeps,
): Invocation {
  const env = childEnvironment(deps.env);
  const cwd = mkdtempSync(join(tmpdir(), `oboete-probe-${target.agent}-`));
  const message = `oboete wiring probe ${marker}. Reply with exactly OK and do not use tools.`;
  switch (target.agent) {
    case 'claude':
      return {
        command: cliPath,
        args: ['-p', message, '--settings', target.configPath, '--output-format', 'json'],
        env,
        cwd,
      };
    case 'codex':
      env.CODEX_HOME = dirname(target.configPath);
      return {
        command: cliPath,
        args: [
          'exec',
          '--sandbox',
          'read-only',
          '--skip-git-repo-check',
          '--json',
          '-C',
          cwd,
          message,
        ],
        env,
        cwd,
      };
    case 'grok':
      env.GROK_HOME = dirname(dirname(target.configPath));
      env.GROK_CLAUDE_HOOKS_ENABLED = '0';
      env.GROK_CLAUDE_MCPS_ENABLED = '0';
      env.GROK_CURSOR_HOOKS_ENABLED = '0';
      env.GROK_CURSOR_MCPS_ENABLED = '0';
      return {
        command: cliPath,
        args: ['-p', message, '--output-format', 'json', '--cwd', cwd],
        env,
        cwd,
      };
    case 'pi':
      env.PI_CODING_AGENT_DIR = dirname(dirname(target.configPath));
      return {
        command: cliPath,
        args: ['-p', message, '--mode', 'json', '--session-dir', cwd],
        env,
        cwd,
      };
  }
}

/**
 * An agent CLI starts MCP servers and hook processes, so the deadline has to reach its whole
 * group; where there are no process groups the single process is the fallback.
 */
function killProcessGroup(child: ChildProcess): void {
  try {
    if (child.pid === undefined) child.kill();
    else process.kill(-child.pid, 'SIGTERM');
  } catch {
    try {
      child.kill();
    } catch {
      // The agent CLI already exited: there is nothing left to stop.
    }
  }
}

async function runProcess(
  spawnFn: typeof spawn,
  invoked: Invocation,
  signal: AbortSignal,
): Promise<ProcessOutcome> {
  let child: ChildProcess | undefined;
  const onAbort = (): void => {
    if (child !== undefined) killProcessGroup(child);
  };
  try {
    child = spawnFn(invoked.command, invoked.args, {
      cwd: invoked.cwd,
      detached: true,
      env: invoked.env,
      signal,
      stdio: 'ignore',
    });
    signal.addEventListener('abort', onAbort, { once: true });
    if (signal.aborted) onAbort();
    const [code] = await once(child, 'close', { signal });
    return { kind: 'closed', code: typeof code === 'number' ? code : null };
  } catch (error) {
    return { kind: 'error', error };
  } finally {
    signal.removeEventListener('abort', onAbort);
  }
}

function elapsed(now: () => number, started: number): number {
  return Math.max(0, Math.round(now() - started));
}

/**
 * The marker can reach storage after the CLI exits (Codex runs its async handler then, Pi captures
 * in a detached child), so a clean run keeps looking until the shared deadline (R12).
 */
async function lookupBeforeDeadline(
  deps: ProbeDeps,
  agent: SetupAgent,
  marker: string,
  signal: AbortSignal,
  retry: boolean,
): Promise<'found' | 'missing' | 'timeout' | 'error'> {
  for (;;) {
    if (signal.aborted) return 'timeout';
    let found: boolean;
    try {
      found = await deps.lookupProbe(agent, marker);
    } catch {
      return 'error';
    }
    if (found) return 'found';
    if (!retry) return 'missing';
    // The shared deadline aborts the wait, and timers/promises reports that as a rejection.
    try {
      await delay(PROBE_POLL_INTERVAL_MS, undefined, { signal });
    } catch {
      return 'timeout';
    }
  }
}

async function runProbe(
  target: ProbeTarget,
  deps: ProbeDeps,
  signal: AbortSignal,
): Promise<ProbeResult> {
  const now = deps.now ?? (() => performance.now());
  const started = now();
  const cliPath = target.cliPath;
  if (cliPath === null) {
    return {
      agent: target.agent,
      status: 'not_installed',
      elapsedMs: elapsed(now, started),
      reason: 'agent_not_installed',
    };
  }

  const marker = `oboete-probe:${randomUUID()}`;
  let child: Invocation;
  try {
    child = invocation(target, cliPath, marker, deps);
  } catch {
    return {
      agent: target.agent,
      status: 'fail',
      elapsedMs: elapsed(now, started),
      reason: 'spawn_failed',
    };
  }

  try {
    const outcome = await runProcess(deps.spawn, child, signal);
    // A run that failed has nothing left to land, so only a clean exit is waited out.
    const cleanExit = outcome.kind === 'closed' && outcome.code === 0;
    const lookup = await lookupBeforeDeadline(deps, target.agent, marker, signal, cleanExit);
    if (lookup === 'found') {
      return {
        agent: target.agent,
        status: 'pass',
        elapsedMs: elapsed(now, started),
        reason: 'probe_event_stored',
      };
    }
    if (lookup === 'error') {
      return {
        agent: target.agent,
        status: 'fail',
        elapsedMs: elapsed(now, started),
        reason: 'probe_lookup_failed',
      };
    }
    if (outcome.kind === 'closed') {
      return {
        agent: target.agent,
        status: 'fail',
        elapsedMs: elapsed(now, started),
        reason:
          outcome.code === 0 ? 'probe_event_missing' : `agent_exit_${outcome.code ?? 'signal'}`,
      };
    }
    if (lookup === 'timeout' || signal.aborted) {
      return {
        agent: target.agent,
        status: 'timeout',
        elapsedMs: elapsed(now, started),
        reason: 'deadline_exceeded',
      };
    }
    return {
      agent: target.agent,
      status: 'fail',
      elapsedMs: elapsed(now, started),
      reason: 'spawn_failed',
    };
  } finally {
    try {
      rmSync(child.cwd, { recursive: true, force: true });
    } catch {
      // Probe status is about hook wiring; a leftover temporary directory cannot change it.
    }
  }
}

/** One shared 90-second deadline covers every selected headless wiring probe (FR-031, R12). */
export async function runProbes(
  agents: readonly ProbeTarget[],
  deps: ProbeDeps,
): Promise<ProbeResult[]> {
  const signal = AbortSignal.timeout(PROBE_DEADLINE_MS);
  return await Promise.all(agents.map((agent) => runProbe(agent, deps, signal)));
}

/** The lookup seam T053/T069 can bind to their already-open database. */
export function probeEventStored(db: DatabaseSync, agent: SetupAgent, marker: string): boolean {
  return (
    db
      .prepare(
        `SELECT 1 AS present FROM raw_events
         WHERE agent = ? AND kind = 'prompt' AND instr(content, ?) > 0
         LIMIT 1`,
      )
      .get(agent, marker) !== undefined
  );
}
