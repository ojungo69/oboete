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
// Sessions whose SessionStart capture has closed.
const started = new Set();
// "ok", "error", "throw", or "stall" (only the native spawn timeout closes it).
let spawnMode = "ok";
let active = 0;
let maxActive = 0;
// What a prompt's hook prints.
let promptOutput = "";
childProcess.spawn = (exe, args, options) => {
  const mode = spawnMode;
  assert.equal(exe, expectedExe);
  assert.deepEqual(args.slice(0, -1), [...homeArgs, "hook", "opencode"]);
  // Only a prompt's output is read.
  const read = args.at(-1) === "UserPromptSubmit";
  assert.deepEqual(options.stdio, ["pipe", read ? "pipe" : "ignore", "ignore"]);
  assert.equal(options.shell, undefined);
  if (mode === "throw") throw new Error("spawn failed");
  const child = new EventEmitter();
  child.stdin = new EventEmitter();
  if (read) {
    child.stdout = new EventEmitter();
    child.stdout.setEncoding = () => child.stdout;
  }
  child.unref = () => { child.unrefed = true; };
  child.stdin.end = (text) => {
    const payload = JSON.parse(text);
    const response = read && typeof promptOutput === "function" ? promptOutput(payload) : promptOutput;
    active += 1;
    maxActive = Math.max(maxActive, active);
    captures.push({ event: args.at(-1), payload, cwd: options.cwd });
    setImmediate(() => {
      assert.equal(child.unrefed, true);
      if (mode === "stall" && options.timeout === undefined) return;
      active -= 1;
      if (mode === "stall") {
        assert.equal(options.timeout, 3000);
        assert.equal(options.killSignal, "SIGKILL");
        if (read) child.stdout.emit("data", JSON.stringify({
          hookSpecificOutput: { additionalContext: "expired partial context" },
        }));
        child.emit("close", null, "SIGKILL");
        return;
      }
      if (mode === "error") {
        child.stdin.emit("error", new Error("EPIPE"));
        child.emit("error", new Error("ENOENT"));
      }
      if (args.at(-1) === "SessionStart") started.add(JSON.parse(text).session_id);
      if (read && mode === "ok") child.stdout.emit("data", response);
      child.emit("close", 0);
    });
  };
  return child;
};
let injectResult = "remembered context";
childProcess.execFile = (exe, args, options, callback) => {
  assert.equal(exe, expectedExe);
  assert.deepEqual(args.slice(0, -1), [...homeArgs, "inject"]);
  assert.match(args.at(-1), /^--session=./);
  // The manifest is read after the session's SessionStart capture has run.
  assert(started.has(args.at(-1).slice("--session=".length)));
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
    async emit(type, data, location, deliver = true) {
      const event = { type, data, location };
      if (waiting) { const resolve = waiting; waiting = undefined; resolve({ value: event, done: false }); }
      else queued.push(event);
      await tick();
      // Existing scenarios deliver steering input at the next boundary. Queued input is delivered
      // explicitly by its regression, using the public inboxID shared by both events.
      if (deliver && type === "session.inbox.enqueued" && data.item?.type === "user" && data.item.delivery !== "queue") {
        await this.emit("session.inbox.delivered", { sessionID: data.sessionID, inboxID: data.inboxID }, location);
      }
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
let inboxID = 0;
const user = (sessionID, text, delivery = "steer") => {
  const id = `msg_${++inboxID}`;
  return { sessionID, inboxID: id, item: { id, type: "user", payload: { text }, delivery } };
};
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
await local.emit("session.text.ended", { sessionID: "one", assistantMessageID: "new", ordinal: 0, text: "final" }, location);
await local.emit("session.text.ended", { sessionID: "one", assistantMessageID: "new", ordinal: 1, text: "answer" }, location);
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
assert.deepEqual(captures.filter((c) => c.event === "Stop").map((c) => c.payload.last_assistant_message), ["final\nanswer", "other answer", ""]);
assert.equal(captures.at(-1).payload.compact_summary, "compacted");
assert(captures.every((c) => c.cwd === location.directory && c.payload.cwd === location.directory));

// A hook can be the first sign of a session (its bus event not read yet): it is still captured,
// in this instance's directory, after its SessionStart.
const beforeHookFirst = captures.length;
local.hooks["execute.after"]({ sessionID: "hook-first", tool: "read", input: {}, status: "completed", result: { content: "string form" } });
await drain();
assert.deepEqual(captures.slice(beforeHookFirst).map((c) => c.event), ["SessionStart", "PostToolUse"]);
assert.equal(captures.at(-1).payload.tool_response, "string form");
assert(captures.slice(beforeHookFirst).every((c) => c.cwd === location.directory));

// A context hook can be the first sign of a session too: its SessionStart still comes first.
const contextFirst = { sessionID: "context-first", system: [] };
await local.hooks.context(contextFirst);
assert.deepEqual(contextFirst.system, [{ type: "text", text: "remembered context" }]);
injections.length = 0;
await drain();

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

// A prompt's output is pushed at each call of its turn, after the manifest, which is read again
// when the prompt got something (it may name a change to the manifest). The turn's end drops it;
// a compaction reads the manifest again too.
injectResult = "remembered context";
await local.emit("session.inbox.enqueued", user("turns", "nothing picked"), location);
await drain();
injections.length = 0;
await local.hooks.context({ sessionID: "turns", system: [] });
await local.emit("session.execution.succeeded", { sessionID: "turns" });
promptOutput = JSON.stringify({ hookSpecificOutput: { additionalContext: "picked" } });
await local.emit("session.inbox.enqueued", user("turns", "a prompt"), location);
await drain();
promptOutput = "";
const turnCalls = Array.from({ length: 2 }, () => ({ sessionID: "turns", system: [] }));
for (const call of turnCalls) await local.hooks.context(call);
for (const call of turnCalls) {
  assert.deepEqual(call.system, [{ type: "text", text: "remembered context" }, { type: "text", text: "picked" }]);
}
assert.equal(injections.length, 2);
await local.emit("session.execution.succeeded", { sessionID: "turns" });
const afterTurn = { sessionID: "turns", system: [] };
await local.hooks.context(afterTurn);
assert.deepEqual(afterTurn.system, [{ type: "text", text: "remembered context" }]);
await local.emit("session.inbox.enqueued", user("turns", "nothing picked"), location);
await drain();
await local.hooks.context({ sessionID: "turns", system: [] });
assert.equal(injections.length, 2);
await local.emit("session.compaction.ended", { sessionID: "turns", text: "summary" });
await drain();
const afterCompaction = { sessionID: "turns", system: [] };
await local.hooks.context(afterCompaction);
assert.deepEqual(afterCompaction.system, [{ type: "text", text: "remembered context" }]);
assert.equal(injections.length, 3);
await drain();

// Enqueue B while A is executing: B belongs only to its later delivery, and A's completion must
// not drop it. context has no executionID; these are the official inbox lifecycle fields.
promptOutput = ({ prompt }) => JSON.stringify({ hookSpecificOutput: {
  additionalContext: prompt === "prompt A" ? "turn A" : "turn B",
} });
await local.emit("session.inbox.enqueued", user("queued-turns", "prompt A"), location);
await drain();
const firstA = { sessionID: "queued-turns", system: [] };
await local.hooks.context(firstA);
const promptsFor = (session) => captures.filter((c) => c.event === "UserPromptSubmit" && c.payload.session_id === session);
assert.equal(promptsFor("queued-turns").length, 1);
const queuedB = user("queued-turns", "prompt B", "queue");
await local.emit("session.inbox.enqueued", queuedB, location);
await drain();
assert.equal(promptsFor("queued-turns").length, 1, "queued input must not consume hookstate before delivery");
const continuedA = { sessionID: "queued-turns", system: [] };
await local.hooks.context(continuedA);
await local.emit("session.execution.succeeded", { sessionID: "queued-turns" });
await local.emit("session.inbox.delivered", { sessionID: "queued-turns", inboxID: queuedB.inboxID });
const firstB = { sessionID: "queued-turns", system: [] };
await local.hooks.context(firstB);
assert.equal(promptsFor("queued-turns").length, 2);
assert.deepEqual([continuedA.system, firstB.system], [
  [{ type: "text", text: "remembered context" }, { type: "text", text: "turn A" }],
  [{ type: "text", text: "remembered context" }, { type: "text", text: "turn B" }],
]);
await local.emit("session.execution.succeeded", { sessionID: "queued-turns" });
promptOutput = "";

// One step boundary may promote several steers ahead of queued input. Cancelled and already
// consumed inbox items must contribute no context; the queued prompt still starts its own turn.
const steering = [];
promptOutput = ({ prompt }) => JSON.stringify({ hookSpecificOutput: { additionalContext: prompt } });
for (const text of ["cancelled context", "queued context", "first steer", "second steer"]) {
  const item = user("steering", text, text.endsWith("steer") ? "steer" : "queue");
  steering.push(item);
  await local.emit("session.inbox.enqueued", item, location, false);
  await drain();
}
assert.equal(promptsFor("steering").length, 0);
const deliver = (item) => local.emit("session.inbox.delivered", { sessionID: "steering", inboxID: item.inboxID });
await local.emit("session.inbox.cancelled", { sessionID: "steering", inboxID: steering[0].inboxID });
await drain();
assert.equal(promptsFor("steering").length, 0, "cancelled input must not consume hookstate");
await deliver(steering[2]);
await deliver(steering[3]);
await deliver(steering[2]); // replaying an already-consumed event cannot append it twice
const steered = { sessionID: "steering", system: [] };
await local.hooks.context(steered);
assert.deepEqual(steered.system, [
  { type: "text", text: "remembered context" },
  { type: "text", text: "first steer\nsecond steer" },
]);
await local.emit("session.execution.succeeded", { sessionID: "steering" });
await deliver(steering[1]);
const queued = { sessionID: "steering", system: [] };
await local.hooks.context(queued);
assert.deepEqual(promptsFor("steering").map((c) => c.payload.prompt), ["first steer", "second steer", "queued context"]);
assert.deepEqual(queued.system, [
  { type: "text", text: "remembered context" }, { type: "text", text: "queued context" },
]);
promptOutput = "";

// A stalled capture must expire and leave the serialized queue able to capture the next prompt.
spawnMode = "stall";
await local.emit("session.inbox.enqueued", user("one", "stalled capture"));
await drain();
const expired = { sessionID: "one", system: [] };
await local.hooks.context(expired);
assert(!expired.system.some((part) => part.text.includes("expired partial context")));
spawnMode = "ok";
await local.emit("session.inbox.enqueued", user("one", "after stalled capture"));
await drain();
assert.equal(captures.at(-1).payload.prompt, "after stalled capture");

for (const mode of ["error", "throw"]) {
  spawnMode = mode;
  await local.emit("session.inbox.enqueued", user("one", "failed spawn"));
  await drain();
}
spawnMode = "ok";
await local.emit("session.inbox.enqueued", user("one", "queue recovered"));
await drain();
assert.equal(captures.at(-1).payload.prompt, "queue recovered");
// The session table is bounded: after 256 newer sessions, "one" is started again.
const starts = () => captures.filter((c) => c.event === "SessionStart" && c.payload.session_id === "one").length;
const startsBefore = starts();
for (let i = 0; i < 256; i += 1) {
  await local.emit("session.inbox.enqueued", user(`bulk${i}`, "x"), location);
  await drain();
}
await local.emit("session.inbox.enqueued", user("one", "back again"), location);
await drain();
assert.equal(starts(), startsBefore + 1);
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
