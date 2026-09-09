import fs from "node:fs";
import path from "node:path";

import { CODEX_EVENTS, oversizedPrompt, toolUsePrompt } from "../probe-lib/agents.mjs";
import { oversizedOutcome, redactValue, toolInputOf, toolNameOf, toolOutputOf, toolUseIdOf, topKeys, writeFixture } from "../probe-lib/agent-events.mjs";
import { binVersion } from "../probe-lib/process.mjs";
import { codexMcpProbe } from "../probe-lib/codex-mcp.mjs";
import { codexPostcompactProbe, codexSessionStartProbe, codexTuiTrustProbe } from "../probe-lib/codex-lifecycle.mjs";
const ROW_SHAPES = "Native tool payload shapes for read/write/edit/bash on all four agents";
const ROW_OVER = "Hook runner behaviour when the hook exits with unread stdin above 1 MB";
const ROW_TRUST = "Codex rollout flush at PostToolUse; TUI trust path";
const ROW_FLUSH = "Codex rollout flush at `PostToolUse`";

function cmdOf(ev) {
  const input = toolInputOf(ev) || {};
  return typeof input.command === "string" ? input.command : "";
}

function classifyCodex(ev) {
  const name = toolNameOf(ev);
  const cmd = cmdOf(ev);
  if (name === "apply_patch" && /\*\*\* Add File:/.test(cmd)) return "apply_patch-add";
  if (name === "apply_patch" && /\*\*\* Update File:/.test(cmd)) return "apply_patch-update";
  if (name === "Bash" && /README/.test(cmd)) return "bash-read";
  if (name === "Bash") return "bash";
  return null;
}

const EXPECTED = {
  "bash-read": {
    file: "bash-read.json",
    native: "Bash",
    normalized: "read",
    input: ["command"],
    path: "tool_input.command — path is inside the shell command text",
  },
  "apply_patch-add": {
    file: "apply_patch-add.json",
    native: "apply_patch",
    normalized: "write",
    input: ["command"],
    path: "tool_input.command *** Add File: <path>",
  },
  "apply_patch-update": {
    file: "apply_patch-update.json",
    native: "apply_patch",
    normalized: "edit",
    input: ["command"],
    path: "tool_input.command *** Update File: <path>",
  },
  bash: {
    file: "bash.json",
    native: "Bash",
    normalized: "bash",
    input: ["command"],
    path: "tool_input.command",
  },
};

function indexCodexEvents(events) {
  const byKind = {};
  for (const ev of events) {
    if (ev.event !== "PreToolUse" && ev.event !== "PostToolUse") continue;
    const kind = classifyCodex(ev);
    if (!kind) continue;
    const id = toolUseIdOf(ev) || "_";
    byKind[kind] ||= {};
    byKind[kind][id] ||= {};
    byKind[kind][id][ev.event] = ev;
  }
  return byKind;
}

function recordCodexShape(options) {
  const { kind, exp, evidence, ctx, r, version, captured_at, pre, post } = options;
  const inKeys = topKeys(toolInputOf(pre));
  const outKeys = topKeys(toolOutputOf(post));
  evidence.push(`${kind} input=[${inKeys.join(",")}] output=[${outKeys.join(",")}] path=${exp.path}`);
  writeFixture(ctx.repoRoot, `test/contracts/codex/${exp.file}`, {
    agent: "codex",
    agent_version: version,
    captured_at,
    native_tool: exp.native,
    normalized_tool: exp.normalized,
    events: {
      PreToolUse: redactValue(pre.stdin, r.repo),
      PostToolUse: redactValue(post.stdin, r.repo),
    },
    notes: exp.path + "; tool_response is a bare string",
  });
}

export const probes = [
  {
    id: "codex-payload-shapes",
    agent: "codex",
    row: ROW_SHAPES,
    async run(ctx) {
      const r = await ctx.codex(ctx.dir, { prompt: toolUsePrompt("codex") });
      const evidence = [];
      const missing = [];
      const version = binVersion("codex");
      const captured_at = new Date().toISOString();
      const byKind = indexCodexEvents(r.events);
      for (const [kind, exp] of Object.entries(EXPECTED)) {
        const matched = Object.values(byKind[kind] || {}).find((p) => p.PreToolUse && p.PostToolUse);
        const pre = matched?.PreToolUse;
        const post = matched?.PostToolUse;
        if (!pre || !post) {
          missing.push(kind);
          evidence.push(`${kind}: missing ${!pre ? "Pre" : ""}${!post ? "Post" : ""}`);
          continue;
        }
        recordCodexShape({ kind, exp, evidence, ctx, r, version, captured_at, pre, post });
      }
      evidence.push(`exit=${r.exitCode} elapsed_s=${(r.elapsedMs / 1000).toFixed(1)} session=${r.sessionId || "none"} model=${r.model || "none"}`);
      return { status: missing.length ? "fail" : "pass", evidence, data: { missing, sessionId: r.sessionId, model: r.model } };
    },
  },
  {
    id: "codex-oversized-stdin",
    agent: "codex",
    row: ROW_OVER,
    async run(ctx) {
      const hooks = [
        ...CODEX_EVENTS.filter((e) => e !== "PostToolUse"),
        { event: "PostToolUse", flags: ["--no-read"], label: "PostToolUse-noread" },
        { event: "PostToolUse", flags: [], label: "PostToolUse" },
      ];
      return oversizedOutcome(await ctx.codex(ctx.dir, { prompt: oversizedPrompt("Bash"), hooks }), ctx.dir);
    },
  },
  {
    id: "codex-trust-hash",
    agent: "codex",
    row: ROW_TRUST,
    async run(ctx) {
      const r = await ctx.codex(ctx.dir, { prompt: toolUsePrompt("codex"), trust: true });
      const names = r.events.map((e) => e.event);
      const need = ["SessionStart", "PreToolUse", "PostToolUse", "Stop"];
      const missing = need.filter((n) => !names.includes(n));
      const cfg = path.join(r.tree, "config.toml");
      const toml = fs.existsSync(cfg) ? fs.readFileSync(cfg, "utf8") : "";
      const rows = (toml.match(/trusted_hash/g) || []).length;
      return {
        status: missing.length ? "fail" : "pass",
        evidence: [`events=${[...new Set(names)].join(",")}`, `trust_rows=${rows}`, `exit=${r.exitCode}`],
        data: { missing, rows },
      };
    },
  },
  codexSessionStartProbe,
  codexPostcompactProbe,
  {
    id: "codex-rollout-flush",
    agent: "codex",
    row: ROW_FLUSH,
    async run(ctx) {
      const r = await ctx.codex(ctx.dir, {
        prompt: toolUsePrompt("codex"),
        hookFlags: { PostToolUse: ["--grep-transcript"] },
      });
      const posts = r.events.filter((e) => e.event === "PostToolUse");
      const rows = posts.map((e) => ({
        tool: toolNameOf(e),
        tool_use_id: toolUseIdOf(e),
        transcript_has_tool_use_id: e.transcript_has_tool_use_id === true,
        transcript_bytes: e.transcript_bytes ?? null,
      }));
      const miss = rows.filter((x) => !x.transcript_has_tool_use_id);
      const evidence = rows.map(
        (x) => `id=${x.tool_use_id || "none"} tool=${x.tool} in_transcript=${x.transcript_has_tool_use_id} bytes=${x.transcript_bytes}`,
      );
      evidence.push(`calls=${rows.length} misses=${miss.length} exit=${r.exitCode}`);
      writeFixture(ctx.repoRoot, "test/contracts/codex/rollout-flush.json", {
        agent: "codex",
        agent_version: binVersion("codex"),
        captured_at: new Date().toISOString(),
        rows,
      });
      return {
        status: rows.length && miss.length === 0 ? "pass" : "fail",
        evidence,
        data: { calls: rows.length, misses: miss.length },
      };
    },
  },
  codexTuiTrustProbe,
  codexMcpProbe,
];
