import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  assertAgentOutput,
  buildFactSeedingPrompt,
  launchAgent,
  prepareAgent,
  requireAgentSuccess,
  resolveSourceHomes,
  retargetCodexTrust,
} from "./probe-lib/isolated-agent.mjs";
import { credentialEntries, stageCredential } from "./probe-lib/agents.mjs";
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

/** The account's own credential files, which the CLI rotates; the fixture stages only the rest. */
function writeAccountCredentials(home) {
  for (const [directory, files] of [
    [path.join(home, ".codex"), ["auth.json"]],
    [path.join(home, ".grok"), ["auth.json", "config.toml"]],
    [path.join(home, ".pi", "agent"), ["auth.json", "settings.json", "models-store.json"]],
  ]) {
    fs.mkdirSync(directory, { recursive: true });
    for (const file of files) fs.writeFileSync(path.join(directory, file), `{"account":"${file}"}\n`);
  }
  fs.mkdirSync(path.join(home, ".grok", "hooks"), { recursive: true });
  fs.writeFileSync(path.join(home, ".grok", "hooks", "oboete.json"), "{}\n");
  fs.mkdirSync(path.join(home, ".pi", "agent", "extensions"), { recursive: true });
  fs.writeFileSync(path.join(home, ".pi", "agent", "extensions", "oboete.js"), "// extension\n");
}

function credentialFixture(t, name) {
  const account = isolatedAccount(t);
  writeAccountCredentials(account.home);
  const root = fs.mkdtempSync(path.join(os.tmpdir(), `oboete-${name}-`));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  return { account, homes: resolveSourceHomes({}, account.home), root };
}

test("every leg links the credential its CLI rotates and copies what the harness rewrites", (t) => {
  const { homes, root } = credentialFixture(t, "credentials");

  for (const [agent, copied] of [
    ["codex", ["config.toml", "hooks.json"]],
    ["grok", ["config.toml"]],
    ["pi", ["settings.json", "models-store.json"]],
  ]) {
    const directory = path.join(root, agent);
    const prepared = prepareAgent(agent, directory, homes, "prompt", path.join(root, "repo"));
    const staged = path.join(directory, "agent-home", "auth.json");
    assert.deepEqual(
      prepared.credentials,
      [{ staged, source: path.join(homes[agent], "auth.json") }],
      `${agent} reports the staged path and the account file behind it`,
    );
    assert.ok(fs.lstatSync(staged).isSymbolicLink(), `${agent} links auth.json`);
    assert.equal(fs.readlinkSync(staged), path.join(homes[agent], "auth.json"));
    for (const file of copied) {
      assert.ok(!fs.lstatSync(path.join(directory, "agent-home", file)).isSymbolicLink(), `${agent} copies ${file}`);
    }
  }
  // Claude reads its credentials from the configured home, so its leg stages none to settle.
  const claude = prepareAgent("claude", path.join(root, "claude"), homes, "prompt", path.join(root, "repo"));
  assert.deepEqual(claude.credentials, []);
});

test("codex still refuses a leg whose account is missing the files the harness rewrites", (t) => {
  const { homes, root } = credentialFixture(t, "codex-required");
  fs.rmSync(path.join(homes.codex, "hooks.json"));
  assert.throws(
    () => prepareAgent("codex", path.join(root, "codex"), homes, "prompt", path.join(root, "repo")),
    (error) => error.name === "PreconditionError" && /missing setup file/.test(error.message),
  );
});

test("a refresh reaches the account whether the CLI writes in place or renames over the link", async (t) => {
  const { homes, root } = credentialFixture(t, "settle");
  const accountFile = path.join(homes.grok, "auth.json");
  const runtimeDir = path.join(root, "runtime");

  const leg = (name, runTimed) => launchAgent({
    agent: "grok",
    directory: path.join(root, name),
    repo: path.join(root, "repo"),
    prompt: "prompt",
    options: { timeoutMs: 1000 },
    homes,
    oboeteHome: path.join(root, "oboete-home"),
    dependencies: { childEnv: probeChildEnv, runTimed },
    // Every real caller shares one runtime directory across the legs of a suite.
    launch: { runtimeDir },
  });

  // Written in place: the link carries it, and the staged path is still a link afterwards.
  await leg("in-place", async (argv, options) => {
    fs.writeFileSync(path.join(options.env.GROK_HOME, "auth.json"), '{"account":"refreshed in place"}\n');
    return { exitCode: 0, stdout: "", stderr: "" };
  });
  assert.equal(JSON.parse(fs.readFileSync(accountFile, "utf8")).account, "refreshed in place");
  assert.ok(fs.lstatSync(path.join(runtimeDir, "agent-home", "auth.json")).isSymbolicLink());

  // Renamed over the link: settling carries the file back and restores the link.
  await leg("renamed", async (argv, options) => {
    const staged = path.join(options.env.GROK_HOME, "auth.json");
    fs.rmSync(staged);
    fs.writeFileSync(staged, '{"account":"refreshed by rename"}\n');
    return { exitCode: 0, stdout: "", stderr: "" };
  });
  assert.equal(JSON.parse(fs.readFileSync(accountFile, "utf8")).account, "refreshed by rename");
  assert.ok(fs.lstatSync(path.join(runtimeDir, "agent-home", "auth.json")).isSymbolicLink());

  // Signed itself out: the link is gone, the account file stands, and the leg's own error stands.
  const removed = await leg("removed", async (argv, options) => {
    fs.rmSync(path.join(options.env.GROK_HOME, "auth.json"));
    return { exitCode: 1, stdout: "", stderr: "Not signed in." };
  });
  assert.equal(removed.exitCode, 1);
  assert.equal(JSON.parse(fs.readFileSync(accountFile, "utf8")).account, "refreshed by rename");
});

test("a half-written credential is reported and never overwrites the account's own", async (t) => {
  const { homes, root } = credentialFixture(t, "settle-partial");
  const accountFile = path.join(homes.grok, "auth.json");
  await assert.rejects(
    launchAgent({
      agent: "grok",
      directory: path.join(root, "partial"),
      repo: path.join(root, "repo"),
      prompt: "prompt",
      options: { timeoutMs: 1000 },
      homes,
      oboeteHome: path.join(root, "oboete-home"),
      dependencies: {
        childEnv: probeChildEnv,
        runTimed: async (argv, options) => {
          const staged = path.join(options.env.GROK_HOME, "auth.json");
          fs.rmSync(staged);
          fs.writeFileSync(staged, '{"account":"half');
          return { exitCode: 0, stdout: "", stderr: "" };
        },
      },
    }),
    (error) => error.name === "Error" && /grok left an unreadable auth.json/.test(error.message),
  );
  assert.equal(JSON.parse(fs.readFileSync(accountFile, "utf8")).account, "auth.json");
});

test("a staged credential the harness requires is a precondition, not a silent skip", (t) => {
  const { homes, root } = credentialFixture(t, "required");
  const missing = path.join(root, "no-account", "auth.json");
  assert.deepEqual(stageCredential(missing, path.join(root, "optional", "auth.json")), []);
  assert.throws(
    () => stageCredential(missing, path.join(root, "required", "auth.json"), true),
    (error) => /missing credential file/.test(error.message),
  );
  // The probe harness derives the account path from the home it staged.
  assert.deepEqual(credentialEntries("grok", "/run/grok-home"), [
    { staged: "/run/grok-home/auth.json", source: path.join(os.homedir(), ".grok/auth.json") },
  ]);
  assert.deepEqual(credentialEntries("claude", "/run/claude-home"), []);
  assert.ok(fs.existsSync(path.join(homes.grok, "auth.json")));
});

test("staging a leg carries back a refresh an unsettled leg left in the directory", (t) => {
  const { homes, root } = credentialFixture(t, "restage");
  const accountFile = path.join(homes.grok, "auth.json");
  const directory = path.join(root, "shared");

  prepareAgent("grok", directory, homes, "prompt", path.join(root, "repo"));
  const staged = path.join(directory, "agent-home", "auth.json");
  // A leg that renamed over the link and was never settled: the only live token is this file.
  fs.rmSync(staged);
  fs.writeFileSync(staged, '{"account":"refreshed but stranded"}\n');

  prepareAgent("grok", directory, homes, "prompt", path.join(root, "repo"));
  assert.equal(JSON.parse(fs.readFileSync(accountFile, "utf8")).account, "refreshed but stranded");
  assert.ok(fs.lstatSync(staged).isSymbolicLink());
});
