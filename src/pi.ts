import { spawn } from "node:child_process";
import { Type } from "typebox";

// Plain JavaScript is valid TypeScript; Pi loads this file through jiti without a build.
// setup.rs prefixes this template with JSON-escaped OBOETE_BIN and OBOETE_ARGS constants.
/* global OBOETE_BIN, OBOETE_ARGS */

// oboete's own failures are dropped: the agent must never break because of them.
const ignore = () => undefined;

export default function (pi) {
  if (process.env.OBOETE_SKIP) return;

  let chain = Promise.resolve("");
  let pending = "";
  const textParts = (parts) => (Array.isArray(parts) ? parts : [])
    .filter((p) => p?.type === "text").map((p) => p.text).join("\n");

  const call = (event, cwd, payload) => new Promise((resolve) => {
    const child = spawn(OBOETE_BIN, [...OBOETE_ARGS, "hook", "pi", event], {
      cwd, stdio: ["pipe", "pipe", "ignore"],
    });
    let out = "";
    const finish = (text) => { clearTimeout(timer); resolve(text); };
    const timer = setTimeout(() => { child.kill("SIGKILL"); finish(""); }, 2000);
    child.on("error", () => finish(""));
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (data) => { out += data; });
    child.on("close", (code) => finish(code === 0 ? out : ""));
    child.stdin.on("error", ignore);
    child.stdin.end(payload);
  });

  const send = (event, ctx, extra) => {
    try {
      // Capture now: Pi may switch sessions before this queued process starts.
      const cwd = ctx.cwd;
      const payload = JSON.stringify({
        session_id: ctx.sessionManager.getSessionId(), cwd,
        transcript_path: ctx.sessionManager.getSessionFile() ?? null,
        hook_event_name: event, ...extra,
      });
      chain = chain.then(() => call(event, cwd, payload)).catch(() => "");
    } catch {
      // A missing executable, closed pipe, or bad payload must never break the agent.
      return Promise.resolve("");
    }
    return chain;
  };

  // Cap the whole wait, including any queued capture calls, not just the final process.
  const wait = async (work) => {
    let timer;
    try {
      return await Promise.race([work, new Promise((resolve) => {
        timer = setTimeout(() => resolve(""), 2000);
      })]);
    } finally {
      clearTimeout(timer);
    }
  };
  const contextOf = (out) => {
    try {
      const context = JSON.parse(out).hookSpecificOutput?.additionalContext;
      return typeof context === "string" ? context : "";
    } catch { return ""; }
  };

  pi.on("session_start", async (event, ctx) => {
    if (event.reason === "reload") return;
    pending = "";
    const resumed = ctx.sessionManager.getEntries().some((entry) => entry.type === "message");
    pending = contextOf(await wait(send("SessionStart", ctx, { source: resumed ? "resume" : "startup" })));
  });
  pi.on("before_agent_start", () => {
    if (!pending) return undefined;
    const content = pending;
    pending = "";
    return { message: { customType: "oboete", content, display: false } };
  });
  pi.on("input", (event, ctx) => {
    if (event.source !== "extension") send("UserPromptSubmit", ctx, { prompt: event.text });
  });
  pi.on("tool_result", (event, ctx) => {
    send(event.isError ? "PostToolUseFailure" : "PostToolUse", ctx, {
      tool_name: event.toolName, tool_input: event.input, tool_response: textParts(event.content),
    });
  });
  pi.on("agent_end", (event, ctx) => {
    const last = [...(event.messages ?? [])].reverse().find((message) => message?.role === "assistant");
    send("Stop", ctx, { last_assistant_message: last ? textParts(last.content) : "" });
  });
  pi.on("session_compact", async (event, ctx) => {
    pending = "";
    send("PostCompact", ctx, { compact_summary: event.compactionEntry?.summary, trigger: event.reason });
    pending = contextOf(await wait(send("SessionStart", ctx, { source: "compact" })));
  });
  pi.on("session_shutdown", async (event, ctx) => {
    if (event.reason === "reload") return;
    send("SessionEnd", ctx, { reason: event.reason });
    await wait(chain);
  });

  const runTool = async (args, ctx, signal) => {
    const result = await pi.exec(OBOETE_BIN, [...OBOETE_ARGS, ...args], {
      cwd: ctx.cwd, timeout: 10000, signal,
    });
    if (result.code !== 0 || result.killed) {
      throw new Error(result.stderr || `oboete ${args[0]} failed (exit ${result.code}${result.killed ? ", killed" : ""})`);
    }
    return { content: [{ type: "text", text: result.stdout }], details: undefined };
  };
  const listArgs = (command, params) => [command,
    ...(params.all ? ["--all"] : []),
    // Match the MCP tools' result cap; the CLI also serves non-model callers.
    ...(params.limit === undefined ? [] : ["--limit", String(Math.min(params.limit, 100))]),
  ];
  const listing = {
    all: Type.Optional(Type.Boolean()),
    limit: Type.Optional(Type.Integer({ minimum: 1, description: "Maximum results, capped at 100." })),
  };
  pi.registerTool({
    name: "oboete_search", label: "Search memory",
    description: "Search oboete observations, summaries and prompts in this repository, or all repositories with all=true.",
    parameters: Type.Object({ query: Type.String(), ...listing }),
    async execute(_id, params, signal, _update, ctx) {
      return runTool([...listArgs("search", params), "--", params.query], ctx, signal);
    },
  });
  pi.registerTool({
    name: "oboete_get", label: "Read memory",
    description: "Read an oboete document by its search id (o12, s5, p7).",
    parameters: Type.Object({ id: Type.String() }),
    async execute(_id, params, signal, _update, ctx) {
      return runTool(["get", "--", params.id], ctx, signal);
    },
  });
  pi.registerTool({
    name: "oboete_timeline", label: "Memory timeline",
    description: "List recent oboete sessions and summaries in this repository, or all repositories with all=true.",
    parameters: Type.Object(listing),
    async execute(_id, params, signal, _update, ctx) {
      return runTool(listArgs("timeline", params), ctx, signal);
    },
  });
}
