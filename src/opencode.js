import { execFile, spawn } from "node:child_process";

// setup.rs prefixes this template with JSON-escaped exe and home constants.
/* global exe, home */

// oboete's own failures are dropped: the agent must never break because of them.
const ignore = () => undefined;

// A long-lived service sees many sessions: keep the most recently active ones. A session that
// comes back after eviction is started again (SessionStart is an upsert).
const MAX_SESSIONS = 256;

export default {
  id: "oboete",
  async setup(ctx) {
    if (process.env.OBOETE_SKIP !== undefined) return ignore;

    const args = home === null ? [] : ["--home", home];
    const sessions = new Map();
    const abort = new AbortController();
    let pending = Promise.resolve();

    // Finish each capture before starting the next, without awaiting it in an agent hook. A
    // prompt's capture also returns what the hook printed for the turn.
    function send(event, payload) {
      const read = event === "UserPromptSubmit";
      pending = pending.then(() => new Promise((resolve) => {
        const child = spawn(exe, [...args, "hook", "opencode", event], {
          cwd: payload.cwd,
          stdio: ["pipe", read ? "pipe" : "ignore", "ignore"],
        });
        let out = "";
        child.stdout?.setEncoding("utf8");
        child.stdout?.on("data", (data) => { out += data; });
        child.once("error", () => resolve(""));
        child.once("close", () => resolve(out));
        child.stdin.on("error", ignore);
        child.stdin.end(JSON.stringify(payload));
        child.unref();
      })).catch(() => "");
      return pending;
    }

    function contextOf(out) {
      try {
        const context = JSON.parse(out).hookSpecificOutput?.additionalContext;
        return typeof context === "string" ? context : "";
      } catch { return ""; }
    }

    // A capture or an injection never holds a turn longer than this.
    const bounded = (work) => Promise.race([
      work,
      new Promise((resolve) => setTimeout(resolve, 3000, "").unref()),
    ]);

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
      if (state) {
        sessions.delete(id);
      } else {
        // Unlocated bus events can belong to another plugin instance's sessions.
        if (!location) return null;
        if (sessions.size >= MAX_SESSIONS) sessions.delete(sessions.keys().next().value);
        state = { dir: location.directory, started: false, message: null, parts: [], turn: null };
      }
      sessions.set(id, state);
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
      // Cache the promise too: overlapping calls still start only one injection process. The
      // session's queued SessionStart capture runs first: its write sets or clears the
      // recording-failure line the text reports. Bounded, so a stuck capture never holds a turn.
      state.context ??= bounded(pending).then(() => new Promise((resolve) => {
        // `=` keeps an id that starts with "-" a value. The session is left out of the other sessions.
        execFile(exe, [...args, "inject", `--session=${e.sessionID}`], {
          cwd: state.dir,
          timeout: 3000,
          killSignal: "SIGKILL",
        }, (error, stdout) => resolve(error ? "" : stdout));
      })).catch(() => "");
      const text = await state.context;
      if (text) e.system.push({ type: "text", text });
      // What the turn's prompt got, at each of the turn's calls: OpenCode keeps no system text.
      const turn = state.turn && await bounded(state.turn);
      if (turn) e.system.push({ type: "text", text: turn });
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
              // A change it names may be in the cached manifest: the turn reads it again.
              state.turn = send("UserPromptSubmit", { ...payload, prompt: data.item.payload?.text })
                .then(contextOf)
                .then((text) => {
                  if (text) state.context = undefined;
                  return text;
                });
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
            state.turn = null;
            break;
          case "session.compaction.ended":
            send("PostCompact", { ...payload, compact_summary: data.text });
            // The summary replaced what the manifest put in: the next call reads it again.
            state.context = undefined;
            break;
        }
      }
    })().catch(ignore);

    return () => abort.abort();
  },
};
