import fs from "node:fs";

const OUT = process.env.PROBE_EVENTS || "";
const MARKER = process.env.PROBE_MARKER || "";
const THROW_AT = process.env.PROBE_THROW || "";
const REGISTER_TOOL = process.env.PROBE_TOOL === "1";

const EVENTS = [
  "project_trust",
  "resources_discover",
  "session_start",
  "session_info_changed",
  "session_before_switch",
  "session_before_fork",
  "session_before_compact",
  "session_compact",
  "session_compact_failed",
  "session_shutdown",
  "session_before_tree",
  "session_tree",
  "context",
  "before_provider_request",
  "before_provider_headers",
  "after_provider_response",
  "before_agent_start",
  "agent_start",
  "agent_end",
  "agent_settled",
  "ui_prompt_start",
  "ui_prompt_end",
  "turn_start",
  "turn_end",
  "message_start",
  "message_update",
  "message_end",
  "tool_execution_start",
  "tool_execution_update",
  "tool_execution_end",
  "model_select",
  "thinking_level_select",
  "tool_call",
  "tool_result",
  "user_bash",
  "input",
];

function piEnv(): Record<string, string> {
  const e: Record<string, string> = {};
  for (const [k, v] of Object.entries(process.env)) {
    if (/^(PI_|CLAUDE_|CODEX_|GROK_|HOOK|PROBE_)/.test(k) && typeof v === "string") e[k] = v;
  }
  return e;
}

function safe(value: unknown): unknown {
  const seen = new WeakSet<object>();
  const walk = (v: unknown, depth: number): unknown => {
    if (typeof v === "string") return v.length > 4000 ? v.slice(0, 4000) + "…[TRUNC]" : v;
    if (v === null || typeof v === "number" || typeof v === "boolean") return v;
    if (typeof v === "bigint") return String(v);
    if (typeof v === "function") return "[function " + (v as Function).name + "]";
    if (typeof v === "undefined") return undefined;
    if (typeof v === "symbol") return String(v);
    if (depth > 12) return "[depth]";
    if (typeof v === "object") {
      const o = v as object;
      if (seen.has(o)) return "[circular]";
      seen.add(o);
      if (Array.isArray(o)) return o.slice(0, 200).map((x) => walk(x, depth + 1));
      const out: Record<string, unknown> = {};
      for (const k of Object.keys(o)) {
        try {
          out[k] = walk((o as Record<string, unknown>)[k], depth + 1);
        } catch {
          out[k] = "[throw]";
        }
      }
      return out;
    }
    return String(v);
  };
  return walk(value, 0);
}

function rec(obj: unknown) {
  if (!OUT) return;
  try {
    fs.appendFileSync(OUT, JSON.stringify(obj) + "\n");
  } catch {
    /* ignore */
  }
}

export default (pi: any) => {
  let messageUpdateCount = 0;
  let sessionId: unknown = null;
  let sessionFile: unknown = null;

  const captureSession = (ctx: any) => {
    try {
      if (typeof ctx?.sessionManager?.getSessionId === "function") {
        sessionId = ctx.sessionManager.getSessionId();
      }
    } catch {
      sessionId = "[throw]";
    }
    try {
      if (typeof ctx?.sessionManager?.getSessionFile === "function") {
        sessionFile = ctx.sessionManager.getSessionFile();
      }
    } catch {
      sessionFile = "[throw]";
    }
  };

  for (const name of EVENTS) {
    pi.on(name, (event: unknown, ctx: any) => {
      if (THROW_AT && name === THROW_AT) throw new Error("probe throw at " + name);
      if (name === "message_update") {
        messageUpdateCount += 1;
        return undefined;
      }
      if (name === "session_start") captureSession(ctx);
      rec({
        event: name,
        env: piEnv(),
        stdin: safe(event),
        at: new Date().toISOString(),
        sessionId,
        sessionFile,
      });
      return undefined;
    });
  }

  pi.on("session_shutdown", () => {
    rec({
      event: "message_update_count",
      env: piEnv(),
      stdin: { count: messageUpdateCount },
      at: new Date().toISOString(),
      sessionId,
      sessionFile,
    });
  });

  pi.on("before_agent_start", (_e: unknown, ctx: any) => {
    if (!sessionId) captureSession(ctx);
    if (!MARKER) return undefined;
    return { message: { customType: "probe", content: MARKER, display: true } };
  });

  if (REGISTER_TOOL && typeof pi.registerTool === "function") {
    try {
      pi.registerTool({
        name: "oboete_probe",
        label: "oboete probe",
        description: "oboete job-B probe tool",
        parameters: { type: "object", properties: {} },
        execute: async () => ({ content: [{ type: "text", text: "oboete_probe ok" }] }),
      });
    } catch {
      /* ignore */
    }
  }
};
