import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { createReport, enumerateLifecycleAgents, enumeratePairs, parseArguments, runHarness } from "./isolated-user.mjs";
import { isolatedAccount, recordingDependencies } from "./isolated-user.test-support.mjs";

const SINGLE_QUOTED = /'([^']+)'/g;

test("parseArguments accepts the T054 flags", () => {
  const options = parseArguments([
    "--pairs",
    "claude:codex,grok:pi",
    "--no-credentials",
    "--daily",
    "--timeout",
    "45",
    "--run-dir",
    "/tmp/oboete-run",
  ]);

  assert.deepEqual(options.pairs, [
    { from: "claude", to: "codex" },
    { from: "grok", to: "pi" },
  ]);
  assert.equal(options.noCredentials, true);
  assert.equal(options.daily, true);
  assert.equal(options.timeoutMs, 45_000);
  assert.equal(options.runDir, "/tmp/oboete-run");
});

test("parseArguments defaults to all pairs and rejects invalid usage", () => {
  assert.equal(parseArguments([]).pairs.length, 12);
  assert.throws(() => parseArguments(["--timeout", "0"]), /positive integer/);
  assert.throws(() => parseArguments(["--wat"]), /Unknown option/);
});

test("parseArguments selects the Claude and Codex lifecycle checks", () => {
  const all = parseArguments(["--lifecycle"]);
  assert.equal(all.lifecycle, true);
  assert.deepEqual(all.agents, ["claude", "codex"]);

  const codex = parseArguments(["--lifecycle", "--agents", "codex"]);
  assert.deepEqual(codex.agents, ["codex"]);
  assert.deepEqual(enumerateLifecycleAgents("codex,claude"), ["codex", "claude"]);
  assert.throws(() => parseArguments(["--agents", "codex"]), /requires --lifecycle/);
  assert.throws(() => parseArguments(["--lifecycle", "--pairs", "claude:codex"]), /cannot be combined/);
  assert.throws(() => parseArguments(["--lifecycle", "--agents", "grok"]), /Unknown lifecycle agent/);
  assert.throws(() => parseArguments(["--lifecycle", "--agents", "codex,codex"]), /Duplicate lifecycle agent/);
});


test("enumeratePairs returns all 12 ordered cross-agent pairs", () => {
  assert.deepEqual(enumeratePairs("all"), [
    { from: "claude", to: "codex" },
    { from: "claude", to: "grok" },
    { from: "claude", to: "pi" },
    { from: "codex", to: "claude" },
    { from: "codex", to: "grok" },
    { from: "codex", to: "pi" },
    { from: "grok", to: "claude" },
    { from: "grok", to: "codex" },
    { from: "grok", to: "pi" },
    { from: "pi", to: "claude" },
    { from: "pi", to: "codex" },
    { from: "pi", to: "grok" },
  ]);
  assert.throws(() => enumeratePairs("claude:claude"), /distinct agents/);
  assert.throws(() => enumeratePairs("claude:codex,claude:codex"), /duplicate pair/);
});


test("createReport has the required pair shape and redacts the run directory", () => {
  const report = createReport({
    runId: "2026-09-04T12-00-00-000Z",
    runDir: "/tmp/private-run",
    startedAt: "2026-09-04T12:00:00.000Z",
    finishedAt: "2026-09-04T12:00:03.000Z",
    noCredentials: false,
    timeoutMs: 120_000,
    requestedPairs: 2,
    results: [
      {
        from: "claude",
        to: "codex",
        elapsedMs: 3_000,
        status: "pass",
        missingFacts: [],
        degradedMarker: true,
        stdout: {
          seed: "/tmp/private-run/claude-to-codex/seed/stdout.txt",
          receive: "/tmp/private-run/claude-to-codex/receive/stdout.txt",
        },
        stderr: {
          seed: "/tmp/private-run/claude-to-codex/seed/stderr.txt",
          receive: "/tmp/private-run/claude-to-codex/receive/stderr.txt",
        },
      },
    ],
  });

  assert.equal(report.runDir, "<run>");
  assert.equal(report.summary, "1 of 2 requested pairs pass (partial run; SC-001 needs all 12)");
  assert.deepEqual(report.pairs[0].agents, { seed: "claude", receive: "codex" });
  assert.equal(report.pairs[0].elapsed_ms, 3_000);
  assert.equal(report.pairs[0].status, "pass");
  assert.deepEqual(report.pairs[0].missing_facts, []);
  assert.equal(report.pairs[0].degraded_marker, true);
  assert.equal(report.pairs[0].stdout.seed, "<run>/claude-to-codex/seed/stdout.txt");
  assert.ok(!JSON.stringify(report).includes("/tmp/private-run"));
});

test("createReport counts the pairs the run asked for and keeps 12 as the SC-001 target", () => {
  const result = (index) => ({
    from: "claude",
    to: "codex",
    elapsedMs: index,
    status: "pass",
    missingFacts: [],
    stdout: {},
    stderr: {},
  });
  const base = {
    runId: "run",
    runDir: "/tmp/run",
    startedAt: "2026-09-04T12:00:00.000Z",
    finishedAt: "2026-09-04T12:00:01.000Z",
    noCredentials: false,
    timeoutMs: 120_000,
  };

  const full = createReport({
    ...base,
    requestedPairs: 12,
    results: Array.from({ length: 12 }, (unused, index) => result(index)),
  });
  assert.equal(full.summary, "12 of 12 pairs pass");
  assert.equal(full.requested_pairs, 12);

  // Mid-run report of that same full run: honest about the denominator it is still working towards.
  const partial = createReport({
    ...base,
    requestedPairs: 12,
    results: [result(0), result(1)],
  });
  assert.equal(partial.summary, "2 of 12 pairs pass");

  const single = createReport({ ...base, requestedPairs: 1, results: [result(0)] });
  assert.equal(single.summary, "1 of 1 requested pairs pass (partial run; SC-001 needs all 12)");
  assert.equal(single.requested_pairs, 1);
  assert.equal(single.total_pairs, 12);
});


test("runHarness keeps oboete credentials off every agent and trusts the copied Codex hooks", async (t) => {
  const account = isolatedAccount(t);
  const runDir = fs.mkdtempSync(path.join(os.tmpdir(), "oboete-run-"));
  t.after(() => fs.rmSync(runDir, { recursive: true, force: true }));
  const dependencies = recordingDependencies(account.home);

  const report = await runHarness(
    {
      pairs: [{ from: "codex", to: "claude" }],
      noCredentials: false,
      daily: false,
      timeoutMs: 120_000,
      runDir,
    },
    dependencies,
  );

  assert.equal(report.pairs[0].status, "pass", JSON.stringify(report.pairs[0].reason));
  assert.equal(report.summary, "1 of 1 requested pairs pass (partial run; SC-001 needs all 12)");

  // FR-016: an agent CLI never sees oboete's provider credentials, credentials or not in this run.
  for (const agent of ["codex", "claude"]) {
    const call = dependencies.calls.find((entry) => entry.argv[0] === agent);
    assert.ok(call, agent);
    assert.equal(call.env.OBOETE_NIM_API_KEY, undefined, agent);
    assert.equal(call.env.OBOETE_CF_API_TOKEN, undefined, agent);
    assert.equal(call.env.OBOETE_CF_ACCOUNT_ID, undefined, agent);
    assert.ok(call.env.OBOETE_HOME, agent);
  }
  // The oboete legs are the only ones that need them, so this run still exercises the provider.
  const observe = dependencies.calls.find((entry) => entry.argv[0] === "oboete" && entry.argv[1] === "observe");
  assert.equal(observe.env.OBOETE_NIM_API_KEY, "nim-secret");
  assert.equal(observe.env.OBOETE_CF_ACCOUNT_ID, "cf-account");

  // The copied Codex home carries its own trust, so the run gates the trust rule instead of it.
  const codex = dependencies.calls.find((entry) => entry.argv[0] === "codex");
  assert.ok(!codex.argv.includes("--dangerously-bypass-hook-trust"));
  const agentHome = path.join(runDir, "codex-to-claude", "seed", "agent-home");
  const copied = fs.readFileSync(path.join(agentHome, "config.toml"), "utf8");
  assert.ok(copied.includes(`[hooks.state."${path.join(agentHome, "hooks.json")}:session_start:0:0"]`));
  assert.ok(!copied.includes(account.hooksPath));
  assert.equal(codex.env.CODEX_HOME, agentHome);
});

test("every harness test file is run by npm test", () => {
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
  const files = fs
    .readdirSync(path.join(root, "scripts", "e2e"), { recursive: true })
    .filter((entry) => entry.endsWith(".test.mjs"))
    .map((entry) => `scripts/e2e/${entry.split(path.sep).join("/")}`);
  assert.ok(
    files.some((file) => file.split("/").length > 3),
    "expected a harness test below scripts/e2e/, otherwise the ** in the globs is untested",
  );

  const { scripts } = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8"));
  const globs = [...scripts.test.matchAll(SINGLE_QUOTED)].map((match) => match[1]);
  for (const file of files) {
    assert.ok(
      globs.some((glob) => path.matchesGlob(file, glob)),
      `npm test does not run ${file} (globs: ${globs.join(" ")})`,
    );
  }
});
