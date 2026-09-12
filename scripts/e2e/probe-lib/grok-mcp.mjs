import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { GROK_ISOLATION_ENV } from "./agents.mjs";
import { finalText, named, redactValue, saveFix, toolNameOf } from "./agent-events.mjs";
import { readMcpFrames } from "./mcp-frames.mjs";
import { childEnv, runTimed } from "./process.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));

const MCP_SRC = path.join(HERE, "../probe-lib/mcp-dummy.mjs");

const ROW_MCP = "Grok Build user-scoped MCP registration; Legacy-era MCP server against Grok client";

function tomlStr(s) {
  return JSON.stringify(String(s));
}

function labeledMcpMethods(frames) {
  return frames.filter((f) => f.frame?.method).map((f) => `${f.dir}:${f.frame.method}`);
}

function prepareGrokMcpProbe(ctx) {
  const dummy = path.join(ctx.dir, "mcp-dummy.mjs");
  fs.copyFileSync(MCP_SRC, dummy);
  const prompt =
    "Call the MCP tool oboete_probe search with query hello and reply DONE followed by the tool result";
  const logToml = path.join(ctx.dir, "mcp-toml.jsonl");
  const logCli = path.join(ctx.dir, "mcp-cli.jsonl");
  const mcpToml = `
[mcp_servers.oboete_probe]
command = ${tomlStr(process.execPath)}
args = [${tomlStr(dummy)}]
env = { PROBE_MCP_LOG = ${tomlStr(logToml)} }
enabled = true
`;
  return { dummy, prompt, logToml, logCli, mcpToml };
}

function prepareGrokMcpAdd(ctx, dummy, logCli) {
  const addHome = path.join(ctx.dir, "cli-add-home");
  fs.cpSync(ctx.grokSeed, addHome, { recursive: true });
  const cfg = path.join(addHome, "config.toml");
  const before = fs.existsSync(cfg) ? fs.readFileSync(cfg, "utf8") : "";
  const addArgs = [
    "grok",
    "mcp",
    "add",
    "--scope",
    "user",
    "oboete_probe",
    "-e",
    `PROBE_MCP_LOG=${logCli}`,
    "--",
    process.execPath,
    dummy,
  ];
  return { addHome, cfg, before, addArgs };
}

function grokMcpAddResult(cfg, before, addProc) {
  const after = fs.existsSync(cfg) ? fs.readFileSync(cfg, "utf8") : "";
  return {
    exit: addProc.exitCode,
    stdout: (addProc.stdout || "").slice(0, 2000),
    stderr: (addProc.stderr || "").slice(0, 2000),
    wrote: after,
    changed: after !== before,
  };
}

function grokMcpResult({ ctx, tomlRun, cliRun, logToml, logCli, mcpAdd }) {
  const framesToml = readMcpFrames(logToml).frames;
  const framesCli = readMcpFrames(logCli).frames;
  const methToml = labeledMcpMethods(framesToml);
  const methCli = labeledMcpMethods(framesCli);
  const has = (meth, name) => meth.some((x) => x.includes(name));
  const tomlPres = named(tomlRun.events, "PreToolUse");
  const cliPres = named(cliRun.events, "PreToolUse");
  const pres = tomlPres.concat(cliPres);
  const toolNames = [...new Set(pres.map((e) => toolNameOf(e)).filter(Boolean))];
  const text = [finalText("grok", tomlRun, tomlRun.events), finalText("grok", cliRun, cliRun.events)].join("\n");
  const echoed = /dummy result for hello/i.test(text);
  const framesOk =
    (has(methToml, "initialize") && has(methToml, "tools/list") && has(methToml, "tools/call")) ||
    (has(methCli, "initialize") && has(methCli, "tools/list") && has(methCli, "tools/call"));
  const firstRepo = tomlPres.length ? tomlRun.repo : cliRun.repo;
  saveFix(ctx, "mcp-search.json", {
    agent: "grok",
    toolNames,
    PreToolUse: redactValue(pres[0]?.stdin ?? null, firstRepo),
    mcpAddWrote: redactValue(mcpAdd.wrote ?? null, ctx.dir),
  });
  const wrote = mcpAdd.wrote || "";
  const wroteSnippet = wrote.includes("oboete_probe")
    ? wrote.slice(Math.max(0, wrote.indexOf("oboete_probe") - 40), wrote.indexOf("oboete_probe") + 400)
    : wrote.slice(0, 400);
  return {
    status: framesOk && echoed ? "pass" : "fail",
    evidence: [
      `toml frames=${methToml.join(",") || "none"}`,
      `cli frames=${methCli.join(",") || "none"}`,
      `PreToolUse toolName=[${toolNames.join(",")}]`,
      `echoed_dummy=${echoed} text=${JSON.stringify(text.slice(0, 240))}`,
      `mcp add exit=${mcpAdd.exit} changed=${mcpAdd.changed} wrote=${JSON.stringify(wroteSnippet)}`,
      `mcp add stdout=${JSON.stringify((mcpAdd.stdout || "").slice(0, 200))} stderr=${JSON.stringify((mcpAdd.stderr || "").slice(0, 200))}`,
    ],
    data: { toolNames, methToml, methCli, mcpAdd },
  };
}

export const grokMcpProbe = {
    id: "grok-mcp-registration",
    agent: "grok",
    row: ROW_MCP,
    async run(ctx) {
      const { dummy, prompt, logToml, logCli, mcpToml } = prepareGrokMcpProbe(ctx);
      const tomlRun = await ctx.grok(path.join(ctx.dir, "toml"), {
        prompt,
        grokSeed: ctx.grokSeed,
        configToml: mcpToml,
        env: { PROBE_MCP_LOG: logToml },
      });
      const { addHome, cfg, before, addArgs } = prepareGrokMcpAdd(ctx, dummy, logCli);
      const addProc = await runTimed(addArgs, {
        cwd: ctx.dir,
        env: childEnv({ GROK_HOME: addHome, ...GROK_ISOLATION_ENV }),
        stdoutPath: path.join(ctx.dir, "mcp-add.out"),
        stderrPath: path.join(ctx.dir, "mcp-add.err"),
        timeoutMs: 60_000,
      });
      const mcpAdd = grokMcpAddResult(cfg, before, addProc);
      const cliRun = await ctx.grok(path.join(ctx.dir, "cli"), {
        prompt,
        grokSeed: ctx.grokSeed,
        homeFrom: addHome,
        env: { PROBE_MCP_LOG: logCli },
      });
      return grokMcpResult({ ctx, tomlRun, cliRun, logToml, logCli, mcpAdd });
    },
  };
