import { redactValue } from "./agent-events.mjs";
import { isCredentialVariable } from "./process.mjs";
import { CODEX_CLEAR_RUN } from "./isolated-lifecycle-state.mjs";

export const LIFECYCLE_CHECKS = ["resume", "compact", "fork", "clear"];

export const LIFECYCLE_ASSERTS = {
  resume:
    "The resumed prompt stays in its oboete conversation and context_epoch without repeating its session-start pack.",
  compact:
    "One compaction advances context_epoch once, re-injects repository memory via SessionStart source=compact, and loses no earlier event.",
  fork:
    "The fork is a separate conversation whose ledger includes repository memory without changing the parent ledger.",
  clear:
    `Claude clear injects at SessionStart; Codex /new creates and injects a new root at the first turn's lazy SessionStart source=startup, before UserPromptSubmit, leaving the parent injections unchanged; the parent stays active because /new fires no SessionEnd (run ${CODEX_CLEAR_RUN}).`,
};

export function createLifecycleReport(options) {
  const {
    runId,
    runDir,
    startedAt,
    finishedAt,
    noCredentials,
    timeoutMs,
    daily = false,
    agents,
    results,
  } = options;
  const passed = results.filter((result) => result.status === "pass").length;
  const total = agents.length * LIFECYCLE_CHECKS.length;
  return redactValue(
    {
      mode: "lifecycle",
      runId,
      runDir,
      started_at: startedAt,
      finished_at: finishedAt,
      no_credentials: noCredentials,
      daily,
      timeout_seconds: timeoutMs / 1000,
      requested_agents: agents,
      total_checks: total,
      summary: `${passed} of ${total} lifecycle checks pass.`,
      lifecycle_checks: results.map((result) => ({
        agent: result.agent,
        check: result.check,
        asserts: LIFECYCLE_ASSERTS[result.check],
        elapsed_ms: result.elapsedMs,
        status: result.status,
        assertions: result.assertions ?? [],
        ...(result.stdout === undefined ? {} : { stdout: result.stdout }),
        ...(result.stderr === undefined ? {} : { stderr: result.stderr }),
        ...(result.pane === undefined ? {} : { pane: result.pane }),
        ...(result.argv === undefined ? {} : { argv: result.argv }),
        ...(result.eventDelta === undefined ? {} : { event_delta: result.eventDelta }),
        ...(result.evidence === undefined ? {} : { evidence: result.evidence }),
        ...(result.reason === undefined ? {} : { reason: result.reason }),
      })),
    },
    runDir,
    "<run>",
  );
}

function markdownCell(value) {
  return String(value).replaceAll(/[\\|]/g, (c) => `\\${c}`).replace(/\r?\n/g, " ");
}

function evidenceReason(reason) {
  let line = String(reason).split(/[\r\n\u2028\u2029]/u, 1)[0];
  // Keep credential-bearing diagnostics in the private report, including values after the name.
  const credential = [...line.matchAll(/\b[A-Z][A-Z0-9_]*\b/g)].find(([name]) => isCredentialVariable(name));
  if (credential) line = `${line.slice(0, credential.index)}[redacted]`;
  return line.replace(/\bBearer\s+\S+/gi, "Bearer [redacted]").slice(0, 240);
}

export function lifecycleRows(report) {
  let output = "| agent | check | status | elapsed ms | asserts | reason |\n|---|---|---:|---:|---|---|\n";
  for (const check of report.lifecycle_checks) {
    output += `| ${check.agent} | ${check.check} | ${check.status} | ${check.elapsed_ms} | ${markdownCell(check.asserts)} | ${markdownCell(evidenceReason(check.reason ?? "none"))} |\n`;
  }
  return output;
}
