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
          timeout: 3000,
          killSignal: "SIGKILL",
        });
        let out = "";
        child.stdout?.setEncoding("utf8");
        child.stdout?.on("data", (data) => { out += data; });
        child.once("error", () => resolve(""));
        child.once("close", (code) => resolve(code === 0 ? out : ""));
        child.stdin.on("error", ignore);
        child.stdin.end(JSON.stringify(payload));
        child.unref();
      })).catch(() => "");
      return pending;
    }

    function contextOf(out) {
      try {
        const packet = JSON.parse(out);
        const text = packet.hookSpecificOutput?.additionalContext;
        if (typeof text !== "string" || !text.trim()) return [];
        const receipt = typeof packet.oboeteReceipt === "string" && /^[0-9a-f]{32}$/.test(packet.oboeteReceipt)
          ? packet.oboeteReceipt : null;
        return [{ text, receipt }];
      } catch { return []; }
    }

    // A capture or an injection never holds a turn longer than this.
    const bounded = (work, empty = "") => Promise.race([
      work,
      new Promise((resolve) => setTimeout(resolve, 3000, empty).unref()),
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
        state = { dir: location.directory, started: false, message: null, parts: [], turn: null, epoch: 0, inbox: new Map() };
      }
      sessions.set(id, state);
      if (!state.started) {
        state.started = true;
        void send("SessionStart", { session_id: id, cwd: state.dir, source: "startup" });
      }
      return state;
    }

    // Hook calls carry no location but only reach this instance's sessions, so the first one
    // may also be where a session is first seen (before its bus event is read).
    ctx.tool.hook("execute.after", (e) => {
      const state = session(e.sessionID, ctx.location);
      if (!state || !["completed", "error"].includes(e.status)) return;
      void send(e.status === "error" ? "PostToolUseFailure" : "PostToolUse", {
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
      const turn = state.turn;
      const epoch = state.epoch;
      // Cache the promise too: overlapping calls still start only one injection process. The
      // session's queued SessionStart capture runs first: its write sets or clears the
      // recording-failure line the text reports. Bounded, so a stuck capture never holds a turn.
      state.context ??= bounded(pending).then(() => new Promise((resolve) => {
        // `=` keeps an id that starts with "-" a value. The session is left out of the other sessions.
        execFile(exe, [...args, "inject", "--json", `--session=${e.sessionID}`], {
          cwd: state.dir,
          timeout: 3000,
          killSignal: "SIGKILL",
        }, (error, stdout) => resolve(error ? [] : contextOf(stdout)));
      })).catch(() => []);
      const manifest = await state.context;
      // What the turn's prompt got, at each of the turn's calls: OpenCode keeps no system text.
      const packets = turn ? await bounded(turn, []) : [];
      if (state.epoch !== epoch || state.turn !== turn) return;
      const pushed = [];
      for (const group of [manifest, packets]) {
        if (!group.length) continue;
        e.system.push({ type: "text", text: group.map((packet) => packet.text).join("\n") });
        pushed.push(...group);
      }
      // Keep receipts for the next call: an ACK can fail, and the receiver is idempotent.
      for (const packet of pushed) {
        if (packet.receipt) void send("ContextInjected", {
          session_id: e.sessionID, cwd: state.dir, receipt: packet.receipt,
        });
      }
    });

    (async () => {
      for await (const ev of ctx.event.subscribe({ signal: abort.signal })) {
        const data = ev.data;
        const state = session(data?.sessionID, ev.location);
        if (!state) continue;
        const payload = { session_id: data.sessionID, cwd: state.dir };
        switch (ev.type) {
          case "session.inbox.enqueued":
            if (data.item?.type === "user" && typeof data.inboxID === "string") {
              // Queue admission is not delivery. Steers may pass queued input, so retain the
              // payload until OpenCode uses it: UserPromptSubmit also consumes corrections.
              state.inbox.set(data.inboxID, { ...payload, prompt: data.item.payload?.text });
            }
            break;
          case "session.inbox.delivered": {
            const prompt = state.inbox.get(data.inboxID);
            state.inbox.delete(data.inboxID);
            if (prompt) {
              const captured = send("UserPromptSubmit", prompt).then(contextOf);
              // A boundary may deliver several steers in this execution. Its terminal clears
              // only these delivered contexts; prompts still queued belong to later execution.
              const turn = Promise.all([state.turn ?? [], captured]).then((packets) => packets.flat());
              state.turn = turn;
              void turn.then((packets) => {
                if (packets.length && state.turn === turn) state.context = undefined;
              });
            }
            break;
          }
          case "session.inbox.cancelled":
            state.inbox.delete(data.inboxID);
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
            void send("Stop", { ...payload, last_assistant_message: state.parts.filter(Boolean).join("\n") });
            state.message = null;
            state.parts = [];
            state.turn = null;
            state.epoch += 1;
            break;
          case "session.compaction.ended":
            void send("PostCompact", { ...payload, compact_summary: data.text });
            // The summary replaced what the manifest put in: the next call reads it again.
            state.context = undefined;
            break;
        }
      }
    })().catch(ignore);

    return () => abort.abort();
  },
};
