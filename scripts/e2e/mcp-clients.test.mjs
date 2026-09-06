import assert from "node:assert/strict";
import test from "node:test";

import {
  LATEST_PROTOCOL,
  assertGetMissing,
  assertInitialize,
  assertInitializedNotification,
  assertPiJson,
  assertRepoRejected,
  assertToolsCallSearch,
  assertToolsList,
  buildReportRow,
  exitCodeFor,
  extractToolName,
} from "./mcp-clients.mjs";

function entry(dir, frame) {
  return { dir, at: "2026-09-06T00:00:00.000Z", frame };
}

const initIn = entry("in", {
  jsonrpc: "2.0",
  id: 1,
  method: "initialize",
  params: { protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "claude-code", version: "0" } },
});
const initOut = entry("out", {
  jsonrpc: "2.0",
  id: 1,
  result: {
    protocolVersion: "2025-11-25",
    capabilities: { tools: { listChanged: false } },
    serverInfo: { name: "oboete", version: "0.1.0-alpha.0" },
  },
});
const initialized = entry("in", { jsonrpc: "2.0", method: "notifications/initialized" });
const listIn = entry("in", { jsonrpc: "2.0", id: 2, method: "tools/list" });
const listOut = entry("out", {
  jsonrpc: "2.0",
  id: 2,
  result: {
    tools: [
      { name: "search", description: "Search memories of the current repository", inputSchema: { type: "object", properties: { query: { type: "string" } }, required: ["query"] } },
      { name: "timeline", description: "Sessions and turns of the current repository", inputSchema: { type: "object", properties: {} } },
      { name: "get", description: "One memory by id within the current repository", inputSchema: { type: "object", properties: { id: { type: "string" } }, required: ["id"] } },
    ],
  },
});
const callIn = entry("in", {
  jsonrpc: "2.0",
  id: 3,
  method: "tools/call",
  params: { name: "search", arguments: { query: "wiring probe" } },
});
const callOut = entry("out", {
  jsonrpc: "2.0",
  id: 3,
  result: {
    content: [{ type: "text", text: "0 memories" }],
    structuredContent: { memories: [], degraded: null },
  },
});

const happy = [initIn, initOut, initialized, listIn, listOut, callIn, callOut];

test("assertInitialize echoes a supported protocolVersion", () => {
  const result = assertInitialize(happy);
  assert.equal(result.ok, true);
  assert.equal(result.protocolVersion, "2025-11-25");
});

test("assertInitialize uses the latest legacy version for an unknown client", () => {
  const frames = [
    entry("in", { jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2031-01-01" } }),
    entry("out", { jsonrpc: "2.0", id: 1, result: { protocolVersion: LATEST_PROTOCOL } }),
  ];
  assert.equal(assertInitialize(frames).ok, true);
  const mismatch = [
    entry("in", { jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2031-01-01" } }),
    entry("out", { jsonrpc: "2.0", id: 1, result: { protocolVersion: "2024-11-05" } }),
  ];
  assert.equal(assertInitialize(mismatch).ok, false);
});

test("assertInitialize fails when the echo does not match a supported request", () => {
  const frames = [
    entry("in", { jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-06-18" } }),
    entry("out", { jsonrpc: "2.0", id: 1, result: { protocolVersion: "2025-11-25" } }),
  ];
  assert.equal(assertInitialize(frames).ok, false);
  assert.match(assertInitialize(frames).reason, /protocolVersion/);
});

test("assertInitializedNotification requires the in-frame", () => {
  assert.equal(assertInitializedNotification(happy).ok, true);
  assert.equal(assertInitializedNotification([initIn, initOut]).ok, false);
});

test("assertToolsList requires exactly the three tools with inputSchema", () => {
  assert.equal(assertToolsList(happy).ok, true);
  const missing = [
    listIn,
    entry("out", {
      jsonrpc: "2.0",
      id: 2,
      result: { tools: [{ name: "search", inputSchema: { type: "object" } }] },
    }),
  ];
  assert.equal(assertToolsList(missing).ok, false);
  const noSchema = [
    listIn,
    entry("out", {
      jsonrpc: "2.0",
      id: 2,
      result: {
        tools: [
          { name: "search" },
          { name: "timeline", inputSchema: { type: "object" } },
          { name: "get", inputSchema: { type: "object" } },
        ],
      },
    }),
  ];
  assert.equal(assertToolsList(noSchema).ok, false);
});

test("assertToolsCallSearch requires text content and memories array", () => {
  assert.equal(assertToolsCallSearch(happy).ok, true);
  const noMemories = [
    callIn,
    entry("out", { jsonrpc: "2.0", id: 3, result: { content: [{ type: "text", text: "x" }] } }),
  ];
  assert.equal(assertToolsCallSearch(noMemories).ok, false);
  const wrongType = [
    callIn,
    entry("out", {
      jsonrpc: "2.0",
      id: 3,
      result: { content: [{ type: "json", text: "{}" }], structuredContent: { memories: [] } },
    }),
  ];
  assert.equal(assertToolsCallSearch(wrongType).ok, false);
});

test("assertRepoRejected detects JSON-RPC -32602", () => {
  const frame = entry("out", {
    jsonrpc: "2.0",
    id: 1,
    error: { code: -32602, message: "Invalid params", data: "the repository is derived from the working directory" },
  });
  assert.equal(assertRepoRejected(frame).ok, true);
  assert.equal(assertRepoRejected(entry("out", { jsonrpc: "2.0", id: 1, error: { code: -32601 } })).ok, false);
  assert.equal(assertRepoRejected(entry("out", { jsonrpc: "2.0", id: 1, result: {} })).ok, false);
});

test("assertGetMissing detects a tool-level isError", () => {
  const frame = entry("out", {
    jsonrpc: "2.0",
    id: 2,
    result: { content: [{ type: "text", text: "not found" }], isError: true },
  });
  assert.equal(assertGetMissing(frame).ok, true);
  assert.equal(assertGetMissing(entry("out", { jsonrpc: "2.0", id: 2, result: { content: [{ type: "text", text: "not found" }] } })).ok, false);
});

test("buildReportRow writes the report contract", () => {
  assert.deepEqual(
    buildReportRow({
      agent: "claude",
      status: "pass",
      protocolVersion: "2025-11-25",
      toolName: "mcp__oboete_probe__search",
      frames: 7,
      reason: "ok",
    }),
    {
      agent: "claude",
      status: "pass",
      protocolVersion: "2025-11-25",
      toolName: "mcp__oboete_probe__search",
      frames: 7,
      reason: "ok",
    },
  );
  assert.throws(() => buildReportRow({ agent: "claude", status: "skipped" }), /invalid status/);
});

test("exit-code rule: fail is non-zero, blocked is zero", () => {
  const pass = [buildReportRow({ agent: "claude", status: "pass" })];
  const fail = [buildReportRow({ agent: "claude", status: "fail", reason: "no frames" })];
  const blocked = [buildReportRow({ agent: "grok", status: "blocked", reason: "HTTP 402" })];
  assert.equal(exitCodeFor(pass), 0);
  assert.notEqual(exitCodeFor(fail), 0);
  assert.equal(exitCodeFor(blocked), 0);
  assert.equal(exitCodeFor([...pass, ...blocked]), 0);
  assert.notEqual(exitCodeFor([...pass, ...fail]), 0);
  assert.notEqual(exitCodeFor(pass, "fail"), 0);
});

test("extractToolName reads Claude/Grok PreToolUse names and falls back to unknown", () => {
  const claude = extractToolName({
    stdout: JSON.stringify({ tool_name: "mcp__oboete_probe__search", result: "0" }),
  });
  assert.equal(claude.toolName, "mcp__oboete_probe__search");
  const empty = extractToolName({ hookLog: "2026-09-06T00:00:00.000Z info capture agent=claude event=PreToolUse outcome=spooled rows=1\n" });
  assert.equal(empty.toolName, "unknown");
  assert.match(empty.reason, /hook\.log/);
});

test("extractToolName does not swallow a thinking paragraph that mentions the tool", () => {
  const grok = extractToolName({
    stdout: JSON.stringify({
      text: 'The user wants me to use the oboete_probe search tool. I will call oboete_probe__search with query "wiring probe".',
    }),
  });
  assert.equal(grok.toolName, "oboete_probe__search");
});

test("assertPiJson accepts CLI --json in a tool_result", () => {
  const stdout = [
    JSON.stringify({ type: "tool_call", toolName: "oboete_search", input: { query: "wiring probe" } }),
    JSON.stringify({
      type: "tool_result",
      toolName: "oboete_search",
      content: [{ type: "text", text: JSON.stringify({ memories: [], reason: "No memories matched this query in the current repository." }) }],
    }),
    JSON.stringify({ type: "turn_end", message: { content: [{ type: "text", text: "0" }] } }),
  ].join("\n");
  const result = assertPiJson(stdout);
  assert.equal(result.ok, true);
  assert.equal(result.toolName, "oboete_search");
});

test("assertPiJson records what headless output exposes when there is no tool call", () => {
  const result = assertPiJson(`${JSON.stringify({ type: "turn_end", message: { content: [{ type: "text", text: "0" }] } })}\n`);
  assert.equal(result.ok, false);
  assert.match(result.reason, /does not expose tool calls/);
  assert.deepEqual(result.types, ["turn_end"]);
});
