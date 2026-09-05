import assert from "node:assert/strict";
import test from "node:test";

import { childEnv } from "./agents.mjs";

/** The variables a developer's shell carries when they run a probe, put back afterwards. */
function withCredentialsInEnvironment(fn) {
  const credentials = {
    OBOETE_CF_API_TOKEN: "cf-token",
    // The one credential variable whose name ends in neither _API_KEY nor _API_TOKEN, so it is the
    // one a narrowed rule would drop first; it names the Cloudflare account the developer pays for.
    OBOETE_CF_ACCOUNT_ID: "cf-account",
    OBOETE_OPENROUTER_API_KEY: "openrouter-key",
  };
  const previous = {};
  for (const [name, value] of Object.entries(credentials)) {
    previous[name] = process.env[name];
    process.env[name] = value;
  }
  try {
    fn(credentials);
  } finally {
    for (const [name, value] of Object.entries(previous)) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
  }
}

// FR-016: a probe launcher hands the agent CLI the developer's shell, so the filter belongs to the
// environment builder every launcher goes through, not to one launcher.
test("childEnv keeps oboete credentials from every agent CLI and hands them over only on request", () => {
  withCredentialsInEnvironment((credentials) => {
    const isolation = {
      OBOETE_HOME: "/nonexistent/oboete-home",
      CODEX_HOME: "/nonexistent/codex-home",
      GROK_HOME: "/nonexistent/grok-home",
      PI_CODING_AGENT_DIR: "/nonexistent/pi-home",
      GROK_CLAUDE_HOOKS_ENABLED: "0",
    };

    const agent = childEnv(isolation);
    // The names are written out rather than tested with the predicate the filter itself uses: a
    // predicate that stopped recognising a variable would otherwise agree with the leak. A blanket
    // rule over the OBOETE_ prefix would be wrong here: OBOETE_TEST_FAULT and OBOETE_HOME are
    // documented variables a shell may legitimately carry into a probe.
    for (const name of Object.keys(credentials)) assert.equal(agent[name], undefined, name);
    for (const [name, value] of Object.entries(isolation)) assert.equal(agent[name], value, name);
    assert.ok(agent.PATH);

    const observer = childEnv(isolation, { credentials: true });
    for (const [name, value] of Object.entries(credentials)) assert.equal(observer[name], value, name);
    for (const [name, value] of Object.entries(isolation)) assert.equal(observer[name], value, name);
  });
});

test("childEnv drops a credential passed to it explicitly unless credentials were asked for", () => {
  const agent = childEnv({ OBOETE_NIM_API_KEY: "nim-key" });
  assert.equal(agent.OBOETE_NIM_API_KEY, undefined);
  assert.equal(childEnv({ OBOETE_NIM_API_KEY: "nim-key" }, { credentials: true }).OBOETE_NIM_API_KEY, "nim-key");
});

