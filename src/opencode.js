import { execFile, spawn } from "node:child_process";

// setup.rs prefixes this template with JSON-escaped exe and home constants.
/* global exe, home */

// oboete's own failures are dropped: the agent must never break because of them.
const ignore = () => undefined;

export default {
  id: "oboete",
  async setup(ctx) {
    if (process.env.OBOETE_SKIP !== undefined) return ignore;

    const args = home === null ? [] : ["--home", home];
    const sessions = new Map();
    const abort = new AbortController();
    let pending = Promise.resolve();

    // Finish each capture before starting the next, without awaiting it in an agent hook.
    function send(event, payload) {
      pending = pending.then(() => new Promise((resolve) => {
        const child = spawn(exe, [...args, "hook", "opencode", event], {
          cwd: payload.cwd,
          stdio: ["pipe", "ignore", "ignore"],
        });
        child.once("error", resolve);
        child.once("close", resolve);
        child.stdin.on("error", ignore);
        child.stdin.end(JSON.stringify(payload));
        child.unref();
      })).catch(ignore);
    }

    function toolText(e) {
      const content = e.result?.content;
      if (typeof content === "string") return content;
      const parts = Array.isArray(content)
        ? content.filter((p) => p.type === "text" && typeof p.text === "string")
        : [];
      if (parts.length) return parts.map((p) => p.text).join("\n");
      if (e.result?.output !== undefined) return JSON.stringify(e.result.output);
      return e.error?.message ?? "";
    }

    function session(id, location) {
      if (typeof id !== "string" || !id) return null;
      if (location && location.directory !== ctx.location.directory) return null;
      let state = sessions.get(id);
      if (!state) {
        // Unlocated bus events can belong to another plugin instance's sessions.
        if (!location) return null;
        state = { dir: location.directory, started: false, message: null, parts: [] };
        sessions.set(id, state);
      }
      if (!state.started) {
        state.started = true;
        send("SessionStart", { session_id: id, cwd: state.dir, source: "startup" });
      }
      return state;
    }

    // Hook calls carry no location but only reach this instance's sessions, so the first one
    // may also be where a session is first seen (before its bus event is read).
    ctx.tool.hook("execute.after", (e) => {
      const state = session(e.sessionID, ctx.location);
      if (!state || !["completed", "error"].includes(e.status)) return;
      send(e.status === "error" ? "PostToolUseFailure" : "PostToolUse", {
        session_id: e.sessionID,
        cwd: state.dir,
        tool_name: e.tool,
        tool_input: e.input,
        tool_response: toolText(e),
      });
    });

    ctx.session.hook("context", async (e) => {
      const state = session(e.sessionID, ctx.location);
      if (!state) return;
      // Cache the promise too: overlapping calls still start only one injection process.
      state.context ??= new Promise((resolve) => {
        execFile(exe, [...args, "inject"], {
          cwd: state.dir,
          timeout: 3000,
          killSignal: "SIGKILL",
        }, (error, stdout) => resolve(error ? "" : stdout));
      }).catch(() => "");
      const text = await state.context;
      if (text) e.system.push({ type: "text", text });
    });

    (async () => {
      for await (const ev of ctx.event.subscribe({ signal: abort.signal })) {
        const data = ev.data;
        const state = session(data?.sessionID, ev.location);
        if (!state) continue;
        const payload = { session_id: data.sessionID, cwd: state.dir };
        switch (ev.type) {
          case "session.inbox.enqueued":
            if (data.item?.type === "user") {
              send("UserPromptSubmit", { ...payload, prompt: data.item.payload?.text });
            }
            break;
          case "session.text.ended":
            // One event per text part: keep every part of the newest assistant message.
            if (data.assistantMessageID !== state.message) {
              state.message = data.assistantMessageID;
              state.parts = [];
            }
            state.parts[data.ordinal ?? state.parts.length] = data.text ?? "";
            break;
          case "session.execution.succeeded":
          case "session.execution.failed":
          case "session.execution.interrupted":
            send("Stop", { ...payload, last_assistant_message: state.parts.filter(Boolean).join("\n") });
            state.message = null;
            state.parts = [];
            break;
          case "session.compaction.ended":
            send("PostCompact", { ...payload, compact_summary: data.text });
            break;
        }
      }
    })().catch(ignore);

    return () => abort.abort();
  },
};
