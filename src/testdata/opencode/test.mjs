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
const nativeTimeout = globalThis.setTimeout;
let delayedCaptures = 0;
let acknowledge = () => {};
let active = 0;
let maxActive = 0;
// What a prompt's hook prints.
let promptOutput = "";
childProcess.spawn = (exe, args, options) => {
  let mode = spawnMode;
  if (args.at(-1) === "PostToolUse" && delayedCaptures > 0) {
    delayedCaptures -= 1;
    mode = "delay";
  }
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
    if (args.at(-1) === "ContextInjected") {
      assert.deepEqual(Object.keys(payload).sort(), ["cwd", "receipt", "session_id"]);
      assert.match(payload.receipt, /^[0-9a-f]{32}$/);
    }
    const response = read && typeof promptOutput === "function" ? promptOutput(payload) : promptOutput;
    active += 1;
    maxActive = Math.max(maxActive, active);
    captures.push({ event: args.at(-1), payload, cwd: options.cwd });
    const later = mode === "delay" ? (fn) => nativeTimeout(fn, 60) : setImmediate;
    later(() => {
      assert.equal(child.unrefed, true);
      if (mode === "stall" && options.timeout === undefined) return;
      active -= 1;
      if (mode === "stall") {
        assert.equal(options.timeout, 3000);
        assert.equal(options.killSignal, "SIGKILL");
        if (read) child.stdout.emit("data", JSON.stringify({
          oboeteReceipt: "f".repeat(32),
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
      if (args.at(-1) === "ContextInjected" && mode === "ok") acknowledge(payload);
      if (read && mode === "ok") child.stdout.emit("data", response);
      child.emit("close", 0);
    });
  };
  return child;
};
let injectResult = "remembered context";
let injectReceipt;
let holdManifest = false;
let releaseManifest;
childProcess.execFile = (exe, args, options, callback) => {
  assert.equal(exe, expectedExe);
  assert.deepEqual(args.slice(0, -1), [...homeArgs, "inject", "--json"]);
  assert.match(args.at(-1), /^--session=./);
  // The manifest is read after the session's SessionStart capture has run.
  assert(started.has(args.at(-1).slice("--session=".length)));
  assert.equal(options.timeout, 3000);
  assert.equal(options.killSignal, "SIGKILL");
  injections.push(options.cwd);
  const error = injectResult instanceof Error ? injectResult : null;
  const response = JSON.stringify({ oboeteReceipt: error ? "e".repeat(32) : injectReceipt,
    hookSpecificOutput: { additionalContext: error ? "partial output must be discarded" : injectResult } });
  const finish = () => callback(error, response);
  if (holdManifest) releaseManifest = finish;
  else setImmediate(finish);
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
const steeringReceipts = { "first steer": "2".repeat(32), "second steer": "3".repeat(32),
  "queued context": "4".repeat(32), "cancelled context": "5".repeat(32) };
promptOutput = ({ prompt }) => JSON.stringify({ oboeteReceipt: steeringReceipts[prompt],
  hookSpecificOutput: { additionalContext: prompt } });
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
await drain();
assert.deepEqual(captures.filter((c) => c.event === "ContextInjected" && c.payload.session_id === "steering")
  .map((c) => c.payload.receipt), [steeringReceipts["first steer"], steeringReceipts["second steer"]]);
assert.deepEqual(steered.system, [
  { type: "text", text: "remembered context" },
  { type: "text", text: "first steer\nsecond steer" },
]);
await local.emit("session.execution.succeeded", { sessionID: "steering" });
await deliver(steering[1]);
const queued = { sessionID: "steering", system: [] };
await local.hooks.context(queued);
await drain();
assert.deepEqual(captures.filter((c) => c.event === "ContextInjected" && c.payload.session_id === "steering")
  .map((c) => c.payload.receipt), [steeringReceipts["first steer"], steeringReceipts["second steer"], steeringReceipts["queued context"]]);
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
assert(!captures.some((c) => c.event === "ContextInjected" && ["e".repeat(32), "f".repeat(32)].includes(c.payload.receipt)));
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

const packet = (text, receipt) => JSON.stringify({ oboeteReceipt: receipt,
  hookSpecificOutput: { additionalContext: text } });
const acksFor = (session) => captures.filter((c) => c.event === "ContextInjected" && c.payload.session_id === session);
const receipt = "1".repeat(32);
promptOutput = packet("acknowledged correction", receipt);
await local.emit("session.inbox.enqueued", user("ack", "a correction"), location);
await drain();
assert.equal(acksFor("ack").length, 0, "capture alone must not acknowledge delivery");
const pushed = { sessionID: "ack", system: [] };
acknowledge = () => assert(pushed.system.some((part) => part.text === "acknowledged correction"));
await local.hooks.context(pushed);
await drain();
assert.equal(acksFor("ack").length, 1);
acknowledge = () => {};
promptOutput = "";

// Receipts survive a failed ACK, for both the cached manifest and this execution's packets.
for (const kind of ["manifest", "turn"]) {
  const id = `ack-retry-${kind}`;
  const token = kind === "manifest" ? "6".repeat(32) : "7".repeat(32);
  injectResult = kind === "manifest" ? "retryable manifest" : "";
  injectReceipt = kind === "manifest" ? token : undefined;
  promptOutput = kind === "turn" ? packet("retryable turn", token) : "";
  await local.emit("session.inbox.enqueued", user(id, "retry"), location);
  await drain();
  let received = 0;
  acknowledge = () => { received += 1; };
  spawnMode = "error";
  await local.hooks.context({ sessionID: id, system: [] });
  await drain();
  assert.equal(received, 0);
  spawnMode = "ok";
  await local.hooks.context({ sessionID: id, system: [] });
  await drain();
  assert.deepEqual(acksFor(id).map((c) => c.payload.receipt), [token, token]);
  assert.equal(received, 1);
}
acknowledge = () => {};

// A failed SDK push acknowledges neither group, and blank or invalid receipts never leave.
injectResult = "";
injectReceipt = undefined;
promptOutput = packet("not pushed", "8".repeat(32));
await local.emit("session.inbox.enqueued", user("not-pushed", "no push"), location);
await drain();
await assert.rejects(local.hooks.context({ sessionID: "not-pushed", system: Object.freeze([]) }), TypeError);
await drain();
assert.equal(acksFor("not-pushed").length, 0);
for (const [text, token] of [["", "9".repeat(32)], ["  ", "9".repeat(32)], ["legacy text", "not a receipt"]]) {
  const id = `empty-${text.length}`;
  promptOutput = packet(text, token);
  await local.emit("session.inbox.enqueued", user(id, "empty"), location);
  await drain();
  const call = { sessionID: id, system: [] };
  await local.hooks.context(call);
  await drain();
  assert.equal(acksFor(id).length, 0);
  assert.deepEqual(call.system, text.trim() ? [{ type: "text", text }] : []);
}

// Exact late-output timeline from proof.mjs: two 60 ms captures delay the prompt beyond its
// scaled 90 ms await. Its terminal discards the packet; only the next SDK push commits it.
globalThis.setTimeout = (fn, ms, ...args) => nativeTimeout(fn, ms === 3000 ? 90 : ms, ...args);
const lost = "receipt-loss";
injectResult = "old cached manifest";
await local.hooks.context({ sessionID: lost, system: [] });
let correctionDue = true;
let finalized = 0;
promptOutput = () => correctionDue ? packet("claim X was retracted", "a".repeat(32)) : "";
acknowledge = (payload) => {
  if (payload.receipt === "a".repeat(32) && correctionDue) { correctionDue = false; finalized += 1; }
};
injectResult = "new manifest without X";
delayedCaptures = 2;
for (let i = 0; i < 2; i += 1) local.hooks["execute.after"]({ sessionID: lost, status: "completed",
  tool: "Read", input: {}, result: { content: "ok" } });
await local.emit("session.inbox.enqueued", user(lost, "first prompt"), location);
const first = { sessionID: lost, system: [] };
await local.hooks.context(first);
assert.deepEqual(first.system, [{ type: "text", text: "old cached manifest" }]);
assert.equal(acksFor(lost).length, 0);
await local.emit("session.execution.succeeded", { sessionID: lost });
await new Promise((resolve) => nativeTimeout(resolve, 80));
await drain();
assert.equal(finalized, 0);
assert(correctionDue);
await local.emit("session.inbox.enqueued", user(lost, "next prompt"), location);
await drain();
const retried = { sessionID: lost, system: [] };
await local.hooks.context(retried);
await drain();
assert(retried.system.some((part) => part.text === "claim X was retracted"));
assert.equal(finalized, 1);
assert.equal(acksFor(lost).length, 1);
globalThis.setTimeout = nativeTimeout;
acknowledge = () => {};
injectResult = "remembered context";
promptOutput = "";

// A terminal while the manifest read is in flight also invalidates this SDK context snapshot.
injectResult = "manifest after terminal";
injectReceipt = "b".repeat(32);
holdManifest = true;
const terminalManifest = { sessionID: "manifest-terminal", system: [] };
const pendingManifest = local.hooks.context(terminalManifest);
await drain();
assert.equal(typeof releaseManifest, "function");
await local.emit("session.execution.succeeded", { sessionID: "manifest-terminal" });
holdManifest = false;
releaseManifest();
await pendingManifest;
await drain();
assert.deepEqual(terminalManifest.system, []);
assert.equal(acksFor("manifest-terminal").length, 0);
injectResult = "remembered context";
injectReceipt = undefined;
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
