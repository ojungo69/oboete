import assert from "node:assert/strict";
import test from "node:test";

import { buildReportRow, exitCodeFor } from "./mcp-report.mjs";

test("buildReportRow writes the report contract", () => {
  assert.deepEqual(
    buildReportRow({
      agent: "claude",
      status: "pass",
      protocolVersion: "2025-11-25",
      toolName: "mcp__oboete_probe__search",
      frames: 7,
      reason: "ok",
    }),
    {
      agent: "claude",
      status: "pass",
      protocolVersion: "2025-11-25",
      toolName: "mcp__oboete_probe__search",
      frames: 7,
      reason: "ok",
    },
  );
  assert.throws(() => buildReportRow({ agent: "claude", status: "skipped" }), /invalid status/);
});

test("exit-code rule: fail is non-zero, blocked is zero", () => {
  const pass = [buildReportRow({ agent: "claude", status: "pass" })];
  const fail = [buildReportRow({ agent: "claude", status: "fail", reason: "no frames" })];
  const blocked = [buildReportRow({ agent: "grok", status: "blocked", reason: "HTTP 402" })];
  assert.equal(exitCodeFor(pass), 0);
  assert.notEqual(exitCodeFor(fail), 0);
  assert.equal(exitCodeFor(blocked), 0);
  assert.equal(exitCodeFor([...pass, ...blocked]), 0);
  assert.notEqual(exitCodeFor([...pass, ...fail]), 0);
  assert.notEqual(exitCodeFor(pass, "fail"), 0);
});
