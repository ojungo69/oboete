import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  assertAgentOutput,
  buildFactSeedingPrompt,
  requireAgentSuccess,
  resolveSourceHomes,
  retargetCodexTrust,
} from "./probe-lib/isolated-agent.mjs";
import { startLifecycleTui } from "./probe-lib/isolated-lifecycle.mjs";
import { childEnv as probeChildEnv } from "./probe-lib/process.mjs";
import { isolatedAccount } from "./isolated-user.test-support.mjs";

test("resolveSourceHomes keeps every configured source inside the isolated account", (t) => {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "oboete-isolated-home-"));
  t.after(() => fs.rmSync(home, { recursive: true, force: true }));

  assert.equal(resolveSourceHomes({ CODEX_HOME: path.join(home, "codex") }, home).codex, path.join(home, "codex"));
  assert.throws(
    () => resolveSourceHomes({ GROK_HOME: path.resolve(home, "..", "maintainer-grok") }, home),
    /escapes the isolated account/,
  );
});

test("buildFactSeedingPrompt asks for one append tool call and preserves every fact", () => {
  const facts = [
    "fact-run-1: the build token is cedar",
    "fact-run-2: the release bird is heron",
    "fact-run-3: 配布色は琥珀",
  ];
  const prompt = buildFactSeedingPrompt(facts);

  assert.match(prompt, /exactly one tool call/i);
  assert.match(prompt, />> NOTES\.md/);
  for (const fact of facts) assert.ok(prompt.includes(fact), fact);
});

test("assertAgentOutput reports only facts absent from normalized output", () => {
  const facts = ["fact-one: cedar", "fact-two: heron", "fact-three: 琥珀"];

  assert.deepEqual(
    assertAgentOutput("fact-one: cedar\nfact-two:   heron\nfact-three: 琥珀", facts),
    { pass: true, missingFacts: [], degradedMarker: false },
  );
  assert.deepEqual(assertAgentOutput("fact-one: cedar; fact-three: 琥珀", facts), {
    pass: false,
    missingFacts: ["fact-two: heron"],
    degradedMarker: false,
  });
  assert.deepEqual(
    assertAgentOutput("fact-one: cedar; fact-two: heron; fact-three: 琥珀", facts, {
      requireDegraded: true,
    }),
    { pass: false, missingFacts: [], degradedMarker: false },
  );
  assert.deepEqual(
    assertAgentOutput(
      "fact-one: cedar; fact-two: heron; fact-three: 琥珀\n> degraded: No summarizer is configured, so these are rule-based notes.",
      facts,
      { requireDegraded: true },
    ),
    { pass: true, missingFacts: [], degradedMarker: true },
  );
  assert.equal(
    assertAgentOutput(
      "fact-one: cedar; fact-two: heron; fact-three: 琥珀\nNo > degraded: line with rule-based notes was present.",
      facts,
      { requireDegraded: true },
    ).pass,
    false,
  );
});

test("retargetCodexTrust points the copied trust rows at the copied hooks.json", () => {
  const source = "/home/oboete-dogfood/.codex/hooks.json";
  const copy = "/run/pair/seed/agent-home/hooks.json";
  const config = [
    "[mcp_servers.oboete]",
    'command = "node"',
    "",
    `[hooks.state."${source}:session_start:0:0"]`,
    'trusted_hash = "sha256:aaa"',
    "",
    `[hooks.state."${source}:pre_tool_use:1:0"]`,
    'trusted_hash = "sha256:bbb"',
    "",
    '[hooks.state."/home/oboete-dogfood/.codex/other-hooks.json:stop:0:0"]',
    'trusted_hash = "sha256:ccc"',
    "",
  ].join("\n");

  const retargeted = retargetCodexTrust(config, source, copy);

  assert.ok(retargeted.includes(`[hooks.state."${copy}:session_start:0:0"]`));
  assert.ok(retargeted.includes(`[hooks.state."${copy}:pre_tool_use:1:0"]`));
  // The hash covers the handler group alone, so the rows keep the value setup computed.
  assert.ok(retargeted.includes('trusted_hash = "sha256:aaa"'));
  assert.ok(!retargeted.includes(`"${source}:`));
  // A row naming a different hooks file is not this harness's to move.
  assert.ok(retargeted.includes('[hooks.state."/home/oboete-dogfood/.codex/other-hooks.json:stop:0:0"]'));
  assert.equal(retargeted.split("\n").length, config.split("\n").length);
});

test("retargetCodexTrust refuses a config that trusts no oboete hook", () => {
  assert.throws(
    () => retargetCodexTrust('[mcp_servers.oboete]\ncommand = "node"\n', "/h/.codex/hooks.json", "/run/hooks.json"),
    (error) => error.name === "PreconditionError" && /no Codex trust row names/.test(error.message),
  );
});

test("Codex lifecycle TUI uses childEnv and the recorded fork/resume commands", (t) => {
  const account = isolatedAccount(t);
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "oboete-tui-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const previous = process.env.OBOETE_NIM_API_KEY;
  process.env.OBOETE_NIM_API_KEY = "must-not-reach-codex";
  t.after(() => {
    if (previous === undefined) delete process.env.OBOETE_NIM_API_KEY;
    else process.env.OBOETE_NIM_API_KEY = previous;
  });
  let launch;

  const open = (agent, action, childEnv = probeChildEnv) => startLifecycleTui({
    agent,
    action,
    directory: path.join(root, action),
    runtimeDir: path.join(root, "runtime"),
    repo: path.join(root, "repo"),
    parentNativeSessionId: "parent-native",
    oboeteHome: path.join(root, "oboete-home"),
    homes: resolveSourceHomes({}, account.home),
    dependencies: {
      childEnv,
      tuiSession: (options) => {
        launch = options;
        return { send() {}, capture: () => "", waitFor: async () => true, kill() {} };
      },
    },
  });

  const opened = open("codex", "fork");
  assert.deepEqual(opened.argv.slice(-2), ["fork", "parent-native"]);
  assert.ok(!opened.argv.includes("--dangerously-bypass-hook-trust"));
  assert.equal(launch.env.OBOETE_HOME, path.join(root, "oboete-home"));
  assert.equal(launch.env.TERM, "xterm-256color");
  assert.equal(launch.env.OBOETE_NIM_API_KEY, undefined);
  assert.deepEqual(Object.keys(launch.env).sort(), ["CODEX_HOME", "OBOETE_HOME", "PATH", "TERM"]);
  assert.ok(!launch.command.includes("must-not-reach-codex"));

  const compact = open("codex", "compact");
  assert.deepEqual(compact.argv.slice(-2), ["resume", "parent-native"]);
  assert.equal(launch.env.OBOETE_NIM_API_KEY, undefined);
  open("claude", "clear", () => ({ ...probeChildEnv(), GITHUB_TOKEN: "private", ARBITRARY_VARIABLE: "private" }));
  assert.deepEqual(Object.keys(launch.env).sort(), ["OBOETE_HOME", "PATH", "TERM"]);
  assert.ok(!launch.command.includes("private"));
});

test("agent-exit classification is shared by every lifecycle action", () => {
  assert.throws(
    () => requireAgentSuccess({ exitCode: 124, stdout: "", stderr: "" }, "codex resume"),
    (error) => error.name === "PreconditionError" && /exited 124/.test(error.message),
  );
  assert.throws(
    () => requireAgentSuccess({ exitCode: 1, stdout: "", stderr: "API Error: 529 Overloaded" }, "codex resume"),
    (error) => error.name === "PreconditionError" && /Overloaded/.test(error.message),
  );
  assert.throws(
    () => requireAgentSuccess({ exitCode: 1, stdout: "", stderr: "API Error: invalid request" }, "codex resume"),
    (error) => error.name === "Error" && /invalid request/.test(error.message),
  );
});
