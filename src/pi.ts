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

  // A capture call's child gets 3 s. Nothing waits for its answer (a session's end waits up to
  // 2 s for the queue and goes on), and a hook whose store is held waits 2 s for the store, then
  // keeps what it could not write and marks the failure (docs/unwritten.md U2): killed at 2 s it
  // died before either, and its event was lost with nothing to say so. A call whose answer Pi
  // waits for dies with Pi's 2 s wait: what its hook prints later must not come to count as
  // shown. Its hook is told when the wait ends (`OBOETE_ANSWER_BY_MS`), and waits for a held store
  // no longer than leaves it `KEEP_ROOM_MS` to keep its events before then (#955). A call with
  // less than that left, or none, only records: its hook is told the wait is over, so it waits
  // for no store and injects nothing, and it gets the capture's 3 s; what it prints joins
  // nothing. As Pi quits, a call gets what is left of its wait and no more, and one with none
  // left is not started, as before: Pi exits once its 2 s wait for the queue ends, and the
  // session's end must not wait behind it.
  const CAPTURE_KILL_MS = 3000;
  const KEEP_ROOM_MS = 1000;
  let quitting = false;
  const call = (event, cwd, payload, deadline) => new Promise((resolve) => {
    const left = deadline === undefined ? undefined : Math.min(2000, deadline - Date.now());
    if (quitting && left !== undefined && left <= 0) { resolve(""); return; }
    const waited = left !== undefined && (left >= KEEP_ROOM_MS || quitting);
    const env = left === undefined
      ? process.env
      : { ...process.env, OBOETE_ANSWER_BY_MS: String(waited ? deadline : Date.now()) };
    const child = spawn(OBOETE_BIN, [...OBOETE_ARGS, "hook", "pi", event], {
      cwd, env, stdio: ["pipe", "pipe", "ignore"],
    });
    let out = "";
    const finish = (text) => { clearTimeout(timer); resolve(text); };
    const timer = setTimeout(() => { child.kill("SIGKILL"); finish(""); }, waited ? left : CAPTURE_KILL_MS);
    child.on("error", () => finish(""));
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (data) => { out += data; });
    child.on("close", (code) => finish(code === 0 && (left === undefined || waited) ? out : ""));
    child.stdin.on("error", ignore);
    child.stdin.end(payload);
  });

  const send = (event, ctx, extra, deadline) => {
    try {
      // Capture now: Pi may switch sessions before this queued process starts.
      const cwd = ctx.cwd;
      const payload = JSON.stringify({
        session_id: ctx.sessionManager.getSessionId(), cwd,
        transcript_path: ctx.sessionManager.getSessionFile() ?? null,
        hook_event_name: event, ...extra,
      });
      chain = chain.then(() => call(event, cwd, payload, deadline)).catch(() => "");
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
    const deadline = Date.now() + 2000;
    pending = contextOf(await wait(send("SessionStart", ctx, { source: resumed ? "resume" : "startup" }, deadline)));
  });
  pi.on("before_agent_start", () => {
    if (!pending) return undefined;
    const content = pending;
    pending = "";
    return { message: { customType: "oboete", content, display: false } };
  });
  // Pi awaits this before the run starts: what the prompt gets joins the stash.
  pi.on("input", async (event, ctx) => {
    if (event.source === "extension") return;
    // Queue time counts too: a prompt's hook that starts after Pi moved on records and shows nothing.
    const deadline = Date.now() + 2000;
    const text = contextOf(await wait(send("UserPromptSubmit", ctx, { prompt: event.text }, deadline)));
    if (text) pending = pending ? `${pending}\n${text}` : text;
  });
  pi.on("tool_result", (event, ctx) => {
    // Capture calls stay queued; send handles rejection and later context calls await the queue.
    void send(event.isError ? "PostToolUseFailure" : "PostToolUse", ctx, {
      tool_name: event.toolName, tool_input: event.input, tool_response: textParts(event.content),
    });
  });
  pi.on("agent_end", (event, ctx) => {
    const last = [...(event.messages ?? [])].reverse().find((message) => message?.role === "assistant");
    void send("Stop", ctx, { last_assistant_message: last ? textParts(last.content) : "" });
  });
  pi.on("session_compact", async (event, ctx) => {
    pending = "";
    const deadline = Date.now() + 2000;
    void send("PostCompact", ctx, { compact_summary: event.compactionEntry?.summary, trigger: event.reason });
    pending = contextOf(await wait(send("SessionStart", ctx, { source: "compact" }, deadline)));
  });
  pi.on("session_shutdown", async (event, ctx) => {
    if (event.reason === "reload") return;
    // Pi exits after a quit; a session replaced by another leaves the process, and its queue, on.
    if (event.reason === "quit") quitting = true;
    void send("SessionEnd", ctx, { reason: event.reason });
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
