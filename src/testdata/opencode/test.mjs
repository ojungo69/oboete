// Exercise the generated plugin at OpenCode's callback and subprocess boundaries.
// No OpenCode service, oboete child or real config is accessed by this harness.
import assert from "node:assert/strict";
import childProcess from "node:child_process";
import { EventEmitter } from "node:events";
import { readFileSync } from "node:fs";
import { syncBuiltinESMExports } from "node:module";

const [file, expectedExe, expectedHome] = process.argv.slice(2);
const homeArgs = expectedHome === undefined ? [] : ["--home", expectedHome];
const captures = [];
const injections = [];
let failSpawn = false;
let active = 0;
let maxActive = 0;
childProcess.spawn = (exe, args, options) => {
  assert.equal(exe, expectedExe);
  assert.deepEqual(args.slice(0, -1), [...homeArgs, "hook", "opencode"]);
  assert.deepEqual(options.stdio, ["pipe", "ignore", "ignore"]);
  assert.equal(options.shell, undefined);
  if (failSpawn === "throw") throw new Error("spawn failed");
  const child = new EventEmitter();
  child.stdin = new EventEmitter();
  child.unref = () => { child.unrefed = true; };
  child.stdin.end = (text) => {
    active += 1;
    maxActive = Math.max(maxActive, active);
    captures.push({ event: args.at(-1), payload: JSON.parse(text), cwd: options.cwd });
    setImmediate(() => {
      assert.equal(child.unrefed, true);
      active -= 1;
      if (failSpawn) {
        child.stdin.emit("error", new Error("EPIPE"));
        child.emit("error", new Error("ENOENT"));
      }
      child.emit("close", 0);
    });
  };
  return child;
};
let injectResult = "remembered context";
childProcess.execFile = (exe, args, options, callback) => {
  assert.equal(exe, expectedExe);
  assert.deepEqual(args, [...homeArgs, "inject"]);
  assert.equal(options.timeout, 3000);
  assert.equal(options.killSignal, "SIGKILL");
  injections.push(options.cwd);
  setImmediate(() => callback(injectResult instanceof Error ? injectResult : null,
    injectResult instanceof Error ? "partial output must be discarded" : injectResult));
};
syncBuiltinESMExports();
const { default: plugin } = await import(`data:text/javascript;base64,${readFileSync(file).toString("base64")}`);
const tick = () => new Promise((resolve) => setImmediate(resolve));
async function drain() {
  // Each capture closes on the next tick; this bounded wait also exposes a stalled queue.
  for (let i = 0; i < 40; i += 1) await tick();
  assert.equal(active, 0);
}
function context() {
  const hooks = {};
  const queued = [];
  let waiting;
  let signal;
  const ctx = {
    location: { directory: "/project with spaces" },
    session: { hook: (name, callback) => { hooks[name] = callback; } },
    tool: { hook: (name, callback) => { hooks[name] = callback; } },
    event: {
      subscribe(options) {
        signal = options.signal;
        signal.addEventListener("abort", () => waiting?.({ done: true }));
        return {
          [Symbol.asyncIterator]() { return this; },
          next() {
            if (signal.aborted) return Promise.resolve({ done: true });
            if (queued.length) return Promise.resolve({ value: queued.shift(), done: false });
            return new Promise((resolve) => { waiting = resolve; });
          },
        };
      },
    },
  };
  return {
    ctx, hooks,
    get signal() { return signal; },
    async emit(type, data, location) {
      const event = { type, data, location };
      if (waiting) { const resolve = waiting; waiting = undefined; resolve({ value: event, done: false }); }
      else queued.push(event);
      await tick();
    },
  };
}
for (const skip of ["1", ""]) {
  process.env.OBOETE_SKIP = skip;
  const skipped = context();
  const cleanup = await plugin.setup(skipped.ctx);
  cleanup();
  assert.deepEqual(skipped.hooks, {});
  assert.equal(skipped.signal, undefined);
}
delete process.env.OBOETE_SKIP;
const local = context();
const cleanup = await plugin.setup(local.ctx);
const location = local.ctx.location;
const user = (sessionID, text) => ({ sessionID, item: { type: "user", payload: { text } } });
await local.emit("session.inbox.enqueued", user("foreign", "wrong repo"), { directory: "/other" });
await local.emit("session.execution.succeeded", { sessionID: "unknown" });
assert.equal(captures.length, 0);

await local.emit("session.inbox.enqueued", user("one", "first prompt"), location);
await local.emit("session.inbox.enqueued", { sessionID: "one", item: { type: "synthetic" } });
await local.emit("session.inbox.enqueued", user("two", "second session"), location);
assert.equal(local.hooks["execute.after"]({ sessionID: "one", tool: "read", input: { path: "a" },
  status: "completed", result: { content: [{ type: "text", text: "a" }, { type: "file" }, { type: "text", text: "b" }], output: "ignored" } }), undefined);
local.hooks["execute.after"]({ sessionID: "one", tool: "object", input: {}, status: "completed", result: { output: { ok: true } } });
local.hooks["execute.after"]({ sessionID: "one", tool: "shell", input: {}, status: "error", error: { message: "denied" } });
await local.emit("session.text.ended", { sessionID: "one", assistantMessageID: "old", text: "earlier step" }, location);
await local.emit("session.text.ended", { sessionID: "one", assistantMessageID: "new", text: "final answer" }, location);
await local.emit("session.text.ended", { sessionID: "two", text: "other answer" }, location);
await local.emit("session.text.ended", { sessionID: "one", text: "wrong repo" }, { directory: "/other" });
await local.emit("session.execution.succeeded", { sessionID: "one" });
await local.emit("session.execution.failed", { sessionID: "two" });
await local.emit("session.execution.interrupted", { sessionID: "one" });
await local.emit("session.compaction.ended", { sessionID: "one", text: "compacted" });
await drain();
assert.equal(maxActive, 1);
assert.deepEqual(captures.map((c) => c.event), ["SessionStart", "UserPromptSubmit", "SessionStart", "UserPromptSubmit",
  "PostToolUse", "PostToolUse", "PostToolUseFailure", "Stop", "Stop", "Stop", "PostCompact"]);
assert.deepEqual(captures[0].payload, { session_id: "one", cwd: location.directory, source: "startup" });
assert.equal(captures[1].payload.prompt, "first prompt");
assert.equal(captures[4].payload.tool_response, "a\nb");
assert.equal(captures[5].payload.tool_response, '{"ok":true}');
assert.equal(captures[6].payload.tool_response, "denied");
assert.deepEqual(captures.filter((c) => c.event === "Stop").map((c) => c.payload.last_assistant_message), ["final answer", "other answer", ""]);
assert.equal(captures.at(-1).payload.compact_summary, "compacted");
assert(captures.every((c) => c.cwd === location.directory && c.payload.cwd === location.directory));

// A hook can be the first sign of a session (its bus event not read yet): it is still captured,
// in this instance's directory, after its SessionStart.
const beforeHookFirst = captures.length;
local.hooks["execute.after"]({ sessionID: "hook-first", tool: "read", input: {}, status: "completed", result: { output: "x" } });
await drain();
assert.deepEqual(captures.slice(beforeHookFirst).map((c) => c.event), ["SessionStart", "PostToolUse"]);
assert(captures.slice(beforeHookFirst).every((c) => c.cwd === location.directory));

const calls = Array.from({ length: 3 }, () => ({ sessionID: "one", system: [] }));
await Promise.all(calls.map((call) => local.hooks.context(call)));
await local.hooks.context(calls[0]);
assert.equal(injections.length, 1);
assert.deepEqual(calls[1].system, [{ type: "text", text: "remembered context" }]);
assert.equal(calls[0].system.length, 2);
injectResult = new Error("timeout");
const failed = { sessionID: "two", system: [] };
await local.hooks.context(failed);
await local.hooks.context(failed);
assert.equal(injections.length, 2);
assert.deepEqual(failed.system, []);
injectResult = "";
await local.emit("session.inbox.enqueued", user("empty", "no memory"), location);
const empty = { sessionID: "empty", system: [] };
await local.hooks.context(empty);
await local.hooks.context(empty);
assert.equal(injections.length, 3);
assert.deepEqual(empty.system, []);
await drain();

for (const failure of [true, "throw"]) {
  failSpawn = failure;
  await local.emit("session.inbox.enqueued", user("one", "failed spawn"));
  await drain();
}
failSpawn = false;
await local.emit("session.inbox.enqueued", user("one", "queue recovered"));
await drain();
assert.equal(captures.at(-1).payload.prompt, "queue recovered");
cleanup();
assert.equal(local.signal.aborted, true);

// A new plugin instance must not reuse another instance's session or injection cache.
const next = context();
const nextCleanup = await plugin.setup(next.ctx);
await next.emit("session.execution.succeeded", { sessionID: "one" });
const before = captures.length;
await drain();
assert.equal(captures.length, before);
await next.emit("session.inbox.enqueued", user("one", "resume"), next.ctx.location);
injectResult = "fresh context";
const resumed = { sessionID: "one", system: [] };
await next.hooks.context(resumed);
assert.deepEqual(resumed.system, [{ type: "text", text: "fresh context" }]);
await drain();
nextCleanup();
