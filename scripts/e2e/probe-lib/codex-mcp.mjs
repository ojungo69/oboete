import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { eventsFile, parseEvents, redactValue, toolNameOf, truncateEvents, writeFixture } from "./agent-events.mjs";
import { readMcpFrames } from "./mcp-frames.mjs";
import { binVersion, childEnv, runTimed } from "./process.mjs";
import { DONE_PROMPT } from "./agents.mjs";
const HERE = path.dirname(fileURLToPath(import.meta.url));

const MCP_DUMMY = path.join(HERE, "../probe-lib/mcp-dummy.mjs");

const ROW_MCP = "Legacy-era MCP server against Claude Code, Codex, Grok clients (raw frames compared)";

const MCP_PROMPT =
  "Call the MCP tool oboete_probe/search with query hello and reply DONE followed by the tool result";

function scrub(v) {
  const u = os.userInfo().username;
  return JSON.parse(JSON.stringify(redactValue(v, null)).split(u).join("<user>"));
}

function tryFixture(repoRoot, rel, obj) {
  try {
    writeFixture(repoRoot, rel, scrub(obj));
    return null;
  } catch (e) {
    return String(e?.message ? e.message : e);
  }
}

function configureCodexMcp(seed, log) {
  const mcpToml = [
    "",
    "[mcp_servers.oboete_probe]",
    'command = "node"',
    `args = [${JSON.stringify(MCP_DUMMY)}]`,
    "startup_timeout_sec = 8",
    "",
    "[mcp_servers.oboete_probe.env]",
    `PROBE_MCP_LOG = ${JSON.stringify(log)}`,
    "",
  ].join("\n");
  const cfg = path.join(seed.tree, "config.toml");
  const prev = fs.existsSync(cfg) ? fs.readFileSync(cfg, "utf8") : "";
  if (!prev.includes("[mcp_servers.oboete_probe]")) fs.writeFileSync(cfg, prev + mcpToml);
  truncateEvents(seed.tree);
}

function codexMcpResult(ctx, seed, proc, log) {
  const events = parseEvents(eventsFile(seed.tree));
  const parsed = readMcpFrames(log);
  const frames = parsed.frames;
  const pre = events.filter((e) => e.event === "PreToolUse");
  const toolNames = pre.map((e) => toolNameOf(e));
  const echoed = /dummy result for hello/i.test(proc.stdout || "");
  const evidence = [
    `protocolVersion=${parsed.protocolVersion || "none"}`,
    `methods_in=[${parsed.methods.join(",")}]`,
    `tools/list=${parsed.hasList}`,
    `tools/call=${parsed.hasCall}`,
    `PreToolUse_tool_name=[${toolNames.join(",")}]`,
    `echoed_dummy=${echoed}`,
    `frames=${frames.length} exit=${proc.exitCode} elapsed_s=${(proc.elapsedMs / 1000).toFixed(1)}`,
  ];
  const fixtureErr = tryFixture(ctx.repoRoot, "test/contracts/codex/mcp-frames.json", {
    agent: "codex",
    agent_version: binVersion("codex"),
    captured_at: new Date().toISOString(),
    protocolVersion: parsed.protocolVersion,
    methods: parsed.methods,
    tool_names: toolNames,
    echoed_dummy: echoed,
    frames: frames.map((f) => ({
      dir: f.dir,
      at: f.at,
      method: (f.frame || f).method || (f.frame || f).result?.serverInfo?.name || null,
      protocolVersion: (f.frame || f).params?.protocolVersion || (f.frame || f).result?.protocolVersion || null,
    })),
  });
  if (fixtureErr) evidence.push("fixture_skip=" + fixtureErr);
  return {
    status: parsed.hasList && parsed.hasCall && echoed ? "pass" : "fail",
    evidence,
    data: parsed,
  };
}

export const codexMcpProbe = {
    id: "codex-mcp-legacy-client",
    agent: "codex",
    row: ROW_MCP,
    async run(ctx) {
      const log = path.join(ctx.dir, "mcp-frames.jsonl");
      const seed = await ctx.codex(ctx.dir, { prompt: DONE_PROMPT });
      configureCodexMcp(seed, log);
      const proc = await runTimed(
        [
          "codex",
          "exec",
          "--dangerously-bypass-hook-trust",
          "--dangerously-bypass-approvals-and-sandbox",
          "--skip-git-repo-check",
          "--json",
          "-C",
          seed.repo,
          "-c",
          `mcp_servers.oboete_probe={command="node", args=[${JSON.stringify(MCP_DUMMY)}], env={PROBE_MCP_LOG=${JSON.stringify(log)}}, startup_timeout_sec=8}`,
          MCP_PROMPT,
        ],
        {
          cwd: seed.repo,
          env: childEnv({ CODEX_HOME: seed.tree }),
          stdoutPath: path.join(ctx.dir, "stdout-mcp.txt"),
          stderrPath: path.join(ctx.dir, "stderr-mcp.txt"),
          timeoutMs: 90_000,
        },
      );
      return codexMcpResult(ctx, seed, proc, log);
    },
  };
