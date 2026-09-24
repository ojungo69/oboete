import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { readFile } from "node:fs/promises";
import { SourceTextModule, SyntheticModule, createContext } from "node:vm";

const [extensionPath, expectedExe, homeArgsJSON] = process.argv.slice(2);
assert.ok(extensionPath && expectedExe && homeArgsJSON, "usage: test.mjs <extension> <exe> <home args JSON>");
const source = await readFile(extensionPath, "utf8");
const homeArgs = JSON.parse(homeArgsJSON);
assert.ok(Array.isArray(homeArgs));

function deadline(promise, label) {
  let timer;
  return Promise.race([
    Promise.resolve(promise),
    new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(`${label} exceeded 2 seconds`)), 2000); }),
  ]).finally(() => clearTimeout(timer));
}

const plain = value => JSON.parse(JSON.stringify(value));
const contextText = payload => payload.source === "resume" ? "" : `context:${payload.source}`;

async function load(env = {}, manualTimers = false) {
  const hooks = [];
  const execs = [];
  const handlers = new Map();
  const tools = new Map();
  const timeouts = [];
  const timers = new Map();
  let nextTimerId = 0;
  let failNextHook = false;
  let stallNextHook = false;
  let nextHookOutput;
  let nextExec = { stdout: "memory text", stderr: "", code: 0, killed: false };
  const spawn = (exe, args, options) => {
    const child = new EventEmitter();
    child.stdout = new EventEmitter();
    child.stdout.setEncoding = () => child.stdout;
    child.stdin = new EventEmitter();
    let input = "";
    child.stdin.write = chunk => { input += chunk.toString(); return true; };
    child.stdin.end = chunk => {
      if (chunk !== undefined) input += chunk.toString();
      const call = { exe, args: plain(args), options: plain(options), payload: JSON.parse(input) };
      hooks.push(call);
      queueMicrotask(() => {
        if (stallNextHook) { stallNextHook = false; return; }
        if (failNextHook) {
          failNextHook = false;
          child.emit("error", new Error("simulated hook failure"));
          return;
        }
        const output = nextHookOutput ?? (call.payload.hook_event_name === "SessionStart"
          ? JSON.stringify({ hookSpecificOutput: { additionalContext: contextText(call.payload) } })
          : "");
        nextHookOutput = undefined;
        child.stdout.emit("data", Buffer.from(output));
        child.emit("close", 0);
      });
    };
    child.kill = () => { child.emit("close", null); return true; };
    return child;
  };
  const Type = Object.fromEntries(["Object", "String", "Boolean", "Integer", "Number", "Optional"].map(name => [name, (...args) => ({ kind: name, args })]));
  const vmContext = createContext({
    process: { env },
    setTimeout: (fn, ms) => {
      timeouts.push(ms);
      if (!manualTimers) return setTimeout(fn, ms);
      const id = ++nextTimerId;
      timers.set(id, fn);
      return id;
    },
    clearTimeout: id => manualTimers ? timers.delete(id) : clearTimeout(id),
  });
  const module = new SourceTextModule(source, { context: vmContext, identifier: extensionPath });
  await module.link(async specifier => {
    if (specifier === "node:child_process") {
      return new SyntheticModule(["spawn"], function () { this.setExport("spawn", spawn); }, { context: vmContext });
    }
    if (specifier === "typebox") {
      return new SyntheticModule(["Type"], function () { this.setExport("Type", Type); }, { context: vmContext });
    }
    throw new Error(`unexpected runtime import: ${specifier}`);
  });
  await deadline(module.evaluate(), "extension evaluation");
  const pi = {
    on(name, handler) { assert.ok(!handlers.has(name), `duplicate handler: ${name}`); handlers.set(name, handler); },
    registerTool(tool) { assert.ok(!tools.has(tool.name), `duplicate tool: ${tool.name}`); tools.set(tool.name, tool); },
    async exec(exe, args, options) {
      execs.push({ exe, args: plain(args), options });
      return nextExec;
    },
  };
  await deadline(module.namespace.default(pi), "extension factory");
  return {
    hooks, execs, handlers, tools, timeouts,
    set failNextHook(value) { failNextHook = value; },
    set stallNextHook(value) { stallNextHook = value; },
    set nextHookOutput(value) { nextHookOutput = value; },
    set nextExec(value) { nextExec = value; },
    fireLastTimer() {
      const [id, fn] = [...timers].at(-1);
      timers.delete(id);
      fn();
    },
    fireFirstTimer() {
      const [id, fn] = timers.entries().next().value;
      timers.delete(id);
      fn();
    },
    emit(name, event, ctx) {
      assert.ok(handlers.has(name), `missing handler: ${name}`);
      return deadline(handlers.get(name)(event, ctx), name);
    },
  };
}

function session(id = "pi-session", entries = []) {
  const state = { id, entries, file: null };
  const ctx = {
    cwd: "/fixture/repo",
    sessionManager: {
      getSessionId: () => state.id,
      getSessionFile: () => state.file,
      getEntries: () => state.entries,
    },
  };
  return { state, ctx };
}

function checkHook(call, name, id = "pi-session") {
  assert.equal(call.exe, expectedExe);
  assert.deepEqual(call.args, [...homeArgs, "hook", "pi", name]);
  assert.equal(call.options.shell, undefined);
  assert.equal(call.options.cwd, "/fixture/repo");
  assert.deepEqual(call.options.stdio, ["pipe", "pipe", "ignore"]);
  assert.equal(call.payload.hook_event_name, name);
  assert.equal(call.payload.session_id, id);
  assert.equal(call.payload.cwd, "/fixture/repo");
  assert.equal(call.payload.transcript_path, null);
}

const run = await load();
assert.deepEqual([...run.handlers.keys()].sort(), [
  "agent_end", "before_agent_start", "input", "session_compact", "session_shutdown", "session_start", "tool_result",
]);
assert.deepEqual([...run.tools.keys()].sort(), ["oboete_get", "oboete_search", "oboete_timeline"]);
const searchSchema = plain(run.tools.get("oboete_search").parameters);
const getSchema = plain(run.tools.get("oboete_get").parameters);
const timelineSchema = plain(run.tools.get("oboete_timeline").parameters);
assert.equal(searchSchema.kind, "Object");
assert.equal(getSchema.kind, "Object");
assert.equal(timelineSchema.kind, "Object");
assert.equal(searchSchema.args[0].query.kind, "String");
assert.equal(searchSchema.args[0].all.kind, "Optional");
assert.equal(searchSchema.args[0].all.args[0].kind, "Boolean");
assert.equal(searchSchema.args[0].limit.kind, "Optional");
assert.equal(searchSchema.args[0].limit.args[0].kind, "Integer");
assert.equal(searchSchema.args[0].limit.args[0].args[0].minimum, 1);
assert.equal(getSchema.args[0].id.kind, "String");
assert.deepEqual(timelineSchema.args[0], { all: searchSchema.args[0].all, limit: searchSchema.args[0].limit });
const { state, ctx } = session();
await run.emit("session_start", { reason: "startup" }, ctx);
assert.equal(run.hooks.length, 1);
checkHook(run.hooks[0], "SessionStart");
assert.equal(run.hooks[0].payload.source, "startup");
assert.deepEqual(plain(await run.emit("before_agent_start", { prompt: "first" }, ctx)), {
  message: { customType: "oboete", content: "context:startup", display: false },
});
assert.equal(await run.emit("before_agent_start", { prompt: "second" }, ctx), undefined);
await run.emit("session_start", { reason: "reload" }, ctx);
assert.equal(run.hooks.length, 1);

await run.emit("input", { source: "extension", text: "internal" }, ctx);
await run.emit("input", { source: "interactive", text: "user prompt" }, ctx);
const toolEvent = {
  toolName: "bash", input: { command: "pwd" }, isError: false,
  content: [{ type: "image", data: "omitted" }, { type: "text", text: "one" }, { type: "text", text: "two" }],
};
const originalToolEvent = plain(toolEvent);
assert.equal(run.handlers.get("tool_result")(toolEvent, ctx), undefined);
assert.deepEqual(plain(toolEvent), originalToolEvent);
await run.emit("tool_result", {
  toolName: "read", input: { path: "missing" }, isError: true,
  content: [{ type: "text", text: "not found" }],
}, ctx);
await run.emit("agent_end", { messages: [
  { role: "assistant", content: [{ type: "text", text: "older" }] },
  { role: "user", content: [{ type: "text", text: "question" }] },
  { role: "assistant", content: [{ type: "thinking", thinking: "secret" }, { type: "text", text: "final" }, { type: "image", data: "omitted" }, { type: "text", text: "answer" }] },
] }, ctx);
await run.emit("session_compact", { reason: "manual", compactionEntry: { summary: "summary" } }, ctx);
assert.deepEqual(plain(await run.emit("before_agent_start", { prompt: "after compact" }, ctx)), {
  message: { customType: "oboete", content: "context:compact", display: false },
});
assert.equal(await run.emit("before_agent_start", { prompt: "again" }, ctx), undefined);
await run.emit("session_shutdown", { reason: "reload" }, ctx);
await run.emit("session_shutdown", { reason: "quit" }, ctx);
assert.deepEqual(run.hooks.map(h => h.payload.hook_event_name), [
  "SessionStart", "UserPromptSubmit", "PostToolUse", "PostToolUseFailure", "Stop", "PostCompact", "SessionStart", "SessionEnd",
]);
for (let i = 0; i < run.hooks.length; i++) checkHook(run.hooks[i], run.hooks[i].payload.hook_event_name);
assert.equal(run.hooks[1].payload.prompt, "user prompt");
assert.deepEqual(run.hooks[2].payload.tool_input, { command: "pwd" });
assert.equal(run.hooks[2].payload.tool_response, "one\ntwo");
assert.equal(run.hooks[3].payload.tool_response, "not found");
assert.equal(run.hooks[4].payload.last_assistant_message, "final\nanswer");
assert.equal(run.hooks[5].payload.compact_summary, "summary");
assert.equal(run.hooks[5].payload.trigger, "manual");
assert.equal(run.hooks[6].payload.source, "compact");
assert.equal(run.hooks[7].payload.reason, "quit");
assert.ok(run.timeouts.length > 0 && run.timeouts.every(ms => ms > 0 && ms <= 2000), "hook waits need a 2-second cap");

const { ctx: resumeCtx, state: resumeState } = session("resumed", [{ type: "custom" }]);
const resumed = await load();
resumeState.file = "/fixture/transcript.jsonl";
await resumed.emit("session_start", { reason: "startup" }, resumeCtx);
assert.equal(resumed.hooks[0].payload.source, "startup");
assert.equal(resumed.hooks[0].payload.transcript_path, resumeState.file);
resumeState.entries.push({ type: "message" });
await resumed.emit("session_start", { reason: "startup" }, resumeCtx);
assert.equal(resumed.hooks[1].payload.source, "resume");
assert.equal(await resumed.emit("before_agent_start", { prompt: "resumed" }, resumeCtx), undefined);
const malformed = await load();
malformed.nextHookOutput = "not JSON";
await malformed.emit("session_start", { reason: "startup" }, ctx);
assert.equal(await malformed.emit("before_agent_start", { prompt: "malformed" }, ctx), undefined);

const recovering = await load();
const { state: changing, ctx: changingCtx } = session("old-session");
recovering.failNextHook = true;
assert.equal(recovering.handlers.get("input")({ source: "interactive", text: "first" }, changingCtx), undefined);
assert.equal(recovering.handlers.get("input")({ source: "interactive", text: "second" }, changingCtx), undefined);
changing.id = "new-session";
await recovering.emit("session_shutdown", { reason: "quit" }, changingCtx);
assert.deepEqual(recovering.hooks.map(h => h.payload.session_id), ["old-session", "old-session", "new-session"]);
assert.deepEqual(recovering.hooks.map(h => h.payload.prompt), ["first", "second", undefined]);

for (const [eventName, event, expected] of [
  ["session_start", { reason: "startup" }, ["UserPromptSubmit", "SessionStart"]],
  ["session_compact", { reason: "manual", compactionEntry: { summary: "summary" } }, ["UserPromptSubmit", "PostCompact", "SessionStart"]],
  ["session_shutdown", { reason: "quit" }, ["UserPromptSubmit", "SessionEnd"]],
]) {
  const bounded = await load({}, true);
  bounded.stallNextHook = true;
  await bounded.emit("input", { source: "interactive", text: "stalled" }, ctx);
  assert.equal(bounded.hooks.length, 1);
  const waiting = bounded.emit(eventName, event, ctx);
  await Promise.resolve();
  bounded.fireLastTimer(); // total-queue wait, while the first spawn is still stuck
  await waiting;
  assert.deepEqual(bounded.hooks.map(h => h.payload.hook_event_name), ["UserPromptSubmit"]);
  if (eventName !== "session_shutdown") {
    assert.equal(await bounded.emit("before_agent_start", { prompt: "after timeout" }, ctx), undefined);
  }
  bounded.fireFirstTimer(); // release the stalled process and let the queue drain
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(bounded.hooks.map(h => h.payload.hook_event_name), expected);
  if (eventName !== "session_shutdown") {
    assert.equal(await bounded.emit("before_agent_start", { prompt: "after drain" }, ctx), undefined);
  }
}

const signal = new AbortController().signal;
async function tool(name, params) {
  return deadline(run.tools.get(name).execute("call-1", params, signal, undefined, ctx), name);
}
function checkExec(call, command) {
  assert.equal(call.exe, expectedExe);
  assert.deepEqual(call.args.slice(0, homeArgs.length + 1), [...homeArgs, command]);
  assert.equal(call.options.cwd, ctx.cwd);
  assert.equal(call.options.signal, signal);
  assert.ok(call.options.timeout > 0 && call.options.timeout <= 10000);
}
assert.deepEqual(plain(await tool("oboete_search", { query: "ordinary words", all: true, limit: 7 })).content, [{ type: "text", text: "memory text" }]);
checkExec(run.execs.at(-1), "search");
assert.ok(run.execs.at(-1).args.includes("--all"));
assert.deepEqual(run.execs.at(-1).args.slice(run.execs.at(-1).args.indexOf("--limit"), run.execs.at(-1).args.indexOf("--limit") + 2), ["--limit", "7"]);
const hostile = "--home /tmp/wrong";
await tool("oboete_search", { query: hostile });
const searchArgs = run.execs.at(-1).args.slice(homeArgs.length + 1);
assert.ok(searchArgs.includes("--"), "search must terminate options before query");
assert.equal(searchArgs.slice(searchArgs.indexOf("--") + 1).join(" "), hostile);
assert.ok(!searchArgs.slice(0, searchArgs.indexOf("--")).includes("--home"));
assert.deepEqual(plain(await tool("oboete_get", { id: "o17" })).content, [{ type: "text", text: "memory text" }]);
checkExec(run.execs.at(-1), "get");
assert.equal(run.execs.at(-1).args.at(-1), "o17");
assert.deepEqual(run.execs.at(-1).args.slice(homeArgs.length), ["get", "--", "o17"]);
assert.deepEqual(plain(await tool("oboete_timeline", { all: true, limit: 3 })).content, [{ type: "text", text: "memory text" }]);
checkExec(run.execs.at(-1), "timeline");
assert.ok(run.execs.at(-1).args.includes("--all"));
assert.deepEqual(run.execs.at(-1).args.slice(-2), ["--limit", "3"]);
await tool("oboete_timeline", { all: true, limit: 100000000 });
assert.deepEqual(run.execs.at(-1).args.slice(-2), ["--limit", "100"]);
run.nextExec = { stdout: "", stderr: "bad", code: 7, killed: false };
await assert.rejects(tool("oboete_get", { id: "o17" }));
run.nextExec = { stdout: "", stderr: "killed", code: 0, killed: true };
await assert.rejects(tool("oboete_get", { id: "o17" }));

const skipped = await load({ OBOETE_SKIP: "1" });
assert.equal(skipped.handlers.size, 0);
assert.equal(skipped.tools.size, 0);
assert.equal(skipped.hooks.length, 0);
console.log("Pi extension VM harness passed");
