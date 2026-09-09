import assert from "node:assert/strict";
import test from "node:test";

import { createLifecycleReport } from "./probe-lib/isolated-lifecycle-report.mjs";

test("createLifecycleReport records every check, assertion, and blocked reason", () => {
  const report = createLifecycleReport({
    runId: "run",
    runDir: "/tmp/private-lifecycle-run",
    startedAt: "2026-09-05T12:00:00.000Z",
    finishedAt: "2026-09-05T12:00:01.000Z",
    noCredentials: true,
    timeoutMs: 120_000,
    agents: ["claude"],
    results: [
      {
        agent: "claude",
        check: "resume",
        elapsedMs: 1_000,
        status: "blocked",
        assertions: [],
        reason: "resume state unavailable",
        stdout: "/tmp/private-lifecycle-run/resume/stdout.txt",
        eventDelta: [{ kind: "prompt", native_session_id: "session" }],
      },
    ],
  });

  assert.equal(report.mode, "lifecycle");
  assert.equal(report.summary, "0 of 4 lifecycle checks pass.");
  assert.equal(report.lifecycle_checks[0].status, "blocked");
  assert.match(report.lifecycle_checks[0].asserts, /oboete conversation/);
  assert.equal(report.lifecycle_checks[0].reason, "resume state unavailable");
  assert.equal(report.lifecycle_checks[0].stdout, "<run>/resume/stdout.txt");
  assert.deepEqual(report.lifecycle_checks[0].event_delta, [{ kind: "prompt", native_session_id: "session" }]);
});
