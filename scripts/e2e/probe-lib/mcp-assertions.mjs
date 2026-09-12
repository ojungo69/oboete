import { parseJsonl, parseMaybeJson } from "./agent-events.mjs";

export const SUPPORTED_PROTOCOL_VERSIONS = ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];
export const LATEST_PROTOCOL = SUPPORTED_PROTOCOL_VERSIONS.at(-1);
export const EXPECTED_TOOLS = ["search", "timeline", "get"];

export function messageOf(entry) {
  return entry?.frame && typeof entry.frame === "object" ? entry.frame : entry;
}

function listed(frames) {
  return (frames || []).map((entry) => ({ dir: entry.dir, at: entry.at, msg: messageOf(entry) }));
}

function pairByMethod(frames, method) {
  const rows = listed(frames);
  const inn = rows.find((row) => row.dir === "in" && row.msg?.method === method);
  if (!inn) return { inn: null, out: null };
  const out = rows.find((row) => row.dir === "out" && row.msg?.id === inn.msg.id);
  return { inn, out };
}

export function assertInitialize(frames) {
  const { inn, out } = pairByMethod(frames, "initialize");
  if (!inn) return { ok: false, reason: "no initialize in-frame" };
  if (!out) return { ok: false, reason: "no initialize out-frame" };
  const requested = inn.msg?.params?.protocolVersion;
  const echoed = out.msg?.result?.protocolVersion;
  if (typeof echoed !== "string" || echoed === "") {
    return { ok: false, reason: "initialize out-frame has no protocolVersion" };
  }
  if (SUPPORTED_PROTOCOL_VERSIONS.includes(requested)) {
    if (echoed !== requested) {
      return { ok: false, reason: `protocolVersion ${echoed} != requested ${requested}` };
    }
  } else if (echoed !== LATEST_PROTOCOL) {
    return { ok: false, reason: `unsupported ${requested} expected latest ${LATEST_PROTOCOL}, got ${echoed}` };
  }
  return { ok: true, reason: `protocolVersion=${echoed}`, protocolVersion: echoed };
}

export function assertInitializedNotification(frames) {
  const found = listed(frames).some((row) => row.dir === "in" && row.msg?.method === "notifications/initialized");
  return found
    ? { ok: true, reason: "notifications/initialized" }
    : { ok: false, reason: "no notifications/initialized in-frame" };
}

export function assertToolsList(frames) {
  const { inn, out } = pairByMethod(frames, "tools/list");
  if (!inn) return { ok: false, reason: "no tools/list in-frame" };
  if (!out) return { ok: false, reason: "no tools/list out-frame" };
  const tools = out.msg?.result?.tools;
  if (!Array.isArray(tools)) return { ok: false, reason: "tools/list result.tools is not an array" };
  const names = tools.map((tool) => tool?.name);
  if (names.length !== EXPECTED_TOOLS.length || EXPECTED_TOOLS.some((name, i) => names[i] !== name)) {
    return { ok: false, reason: `tools/list names=[${names.join(",")}] expected [${EXPECTED_TOOLS.join(",")}]` };
  }
  const missingSchema = tools.filter((tool) => !tool?.inputSchema || typeof tool.inputSchema !== "object");
  if (missingSchema.length) {
    return { ok: false, reason: `tools/list missing inputSchema on ${missingSchema.map((t) => t.name).join(",")}` };
  }
  return { ok: true, reason: "tools/list search,timeline,get" };
}

export function assertToolsCallSearch(frames) {
  const rows = listed(frames);
  const inn = rows.find(
    (row) => row.dir === "in" && row.msg?.method === "tools/call" && row.msg?.params?.name === "search",
  );
  if (!inn) return { ok: false, reason: "no tools/call search in-frame" };
  const out = rows.find((row) => row.dir === "out" && row.msg?.id === inn.msg.id);
  if (!out) return { ok: false, reason: "no tools/call search out-frame" };
  const result = out.msg?.result;
  const content0 = Array.isArray(result?.content) ? result.content[0] : null;
  if (content0?.type !== "text") {
    return { ok: false, reason: `tools/call content[0].type=${content0?.type ?? "missing"}` };
  }
  if (!Array.isArray(result?.structuredContent?.memories)) {
    return { ok: false, reason: "tools/call structuredContent.memories is not an array" };
  }
  return { ok: true, reason: `search memories=${result.structuredContent.memories.length}` };
}

export function assertRepoRejected(frame) {
  const msg = messageOf(frame);
  const code = msg?.error?.code;
  if (code !== -32602) return { ok: false, reason: `repo rejection code=${code ?? "missing"}` };
  return { ok: true, reason: "-32602" };
}

export function assertGetMissing(frame) {
  const msg = messageOf(frame);
  if (msg?.result?.isError !== true) {
    return { ok: false, reason: `get missing isError=${msg?.result?.isError ?? "missing"}` };
  }
  return { ok: true, reason: "isError: true" };
}

export function assertAgentFrames(frames) {
  const checks = [
    assertInitialize(frames),
    assertInitializedNotification(frames),
    assertToolsList(frames),
    assertToolsCallSearch(frames),
  ];
  const failed = checks.filter((check) => !check.ok);
  const protocolVersion = checks[0].protocolVersion ?? readMcpFramesFrom(frames);
  return {
    ok: failed.length === 0,
    reason: failed.length ? failed.map((check) => check.reason).join("; ") : checks.map((check) => check.reason).join("; "),
    protocolVersion,
    checks,
  };
}

function readMcpFramesFrom(frames) {
  const inn = listed(frames).find((row) => row.dir === "in" && row.msg?.method === "initialize");
  return inn?.msg?.params?.protocolVersion ?? null;
}

const TOOL_NAME_TOKEN = /mcp__oboete_probe__\w+|oboete_probe__\w+|oboete_search/g;
const TOOL_NAME_EXACT = /^(?:mcp__)?oboete_probe__\w+$|^oboete_search$/;

function collectObjectToolNames(value, acc) {
  for (const [key, item] of Object.entries(value)) {
    if (/^(tool_?name|name)$/i.test(key) && typeof item === "string" && TOOL_NAME_EXACT.test(item)) acc.add(item);
    else collectToolNames(item, acc);
  }
}

function collectToolNames(value, acc) {
  if (typeof value === "string") {
    if (TOOL_NAME_EXACT.test(value)) acc.add(value);
    else for (const match of value.matchAll(TOOL_NAME_TOKEN)) acc.add(match[0]);
    return;
  }
  if (Array.isArray(value)) {
    for (const item of value) collectToolNames(item, acc);
    return;
  }
  if (!value || typeof value !== "object") return;
  collectObjectToolNames(value, acc);
}

export function extractToolName({ stdout = "", stderr = "", hookLog = "", dbPayloads = [] } = {}) {
  const names = new Set();
  for (const blob of [stdout, stderr, hookLog, ...dbPayloads]) {
    if (!blob) continue;
    const parsed = parseJsonl(blob);
    const envelope = parseMaybeJson(blob);
    if (envelope) parsed.push(envelope);
    for (const obj of parsed) collectToolNames(obj, names);
    for (const match of String(blob).matchAll(TOOL_NAME_TOKEN)) names.add(match[0]);
  }
  const list = [...names].filter((name) => TOOL_NAME_EXACT.test(name) && name.length < 80);
  const preferred =
    list.find((name) => name === "mcp__oboete_probe__search") ||
    list.find((name) => name === "oboete_probe__search") ||
    list.find((name) => name === "oboete_search") ||
    list[0];
  if (preferred) return { toolName: preferred, reason: null };
  return {
    toolName: "unknown",
    reason: "hook.log records capture outcome without tool_name; agent JSON had no PreToolUse tool_name",
  };
}

function piResultTexts(results, calls, lines) {
  const texts = [];
  for (const line of results.concat(calls, lines)) {
    const content = line.content || line.message?.content || line.result;
    if (typeof content === "string") texts.push(content);
    if (Array.isArray(content)) {
      for (const block of content) if (typeof block?.text === "string") texts.push(block.text);
    }
  }
  return texts;
}

function parsePiMemories(texts, stdout) {
  let parsed = null;
  for (const text of texts) {
    const obj = parseMaybeJson(text);
    if (obj && Array.isArray(obj.memories)) {
      parsed = obj;
      break;
    }
  }
  if (!parsed) {
    const obj = parseMaybeJson(stdout);
    if (obj && Array.isArray(obj.memories)) parsed = obj;
  }
  return parsed;
}

export function assertPiJson(stdout) {
  const lines = parseJsonl(stdout);
  const types = [...new Set(lines.map((line) => line.type).filter(Boolean))];
  const named = (line) => line?.toolName || line?.tool_name || line?.name || "";
  const calls = lines.filter((line) => named(line) === "oboete_search" || (line.type === "tool_call" && /oboete_search/.test(JSON.stringify(line))));
  const results = lines.filter((line) => line.type === "tool_result" && /oboete_search/.test(JSON.stringify(line)));
  const texts = piResultTexts(results, calls, lines);
  const parsed = parsePiMemories(texts, stdout);
  if (calls.length === 0 && !parsed) {
    return {
      ok: false,
      toolName: "unknown",
      reason: `Pi JSON output does not expose tool calls or CLI --json; types=[${types.join(",") || "none"}]`,
      types,
    };
  }
  if (!parsed) {
    return {
      ok: false,
      toolName: calls.length ? "oboete_search" : "unknown",
      reason: `Pi called the tool but the result text is not CLI --json {memories:[]}; types=[${types.join(",")}]`,
      types,
    };
  }
  if (calls.length === 0) {
    return {
      ok: true,
      toolName: "oboete_search",
      reason: `Pi JSONL has no tool_call events (types=[${types.join(",") || "none"}]); CLI --json memories=${parsed.memories.length}`,
      types,
    };
  }
  return { ok: true, toolName: "oboete_search", reason: `oboete_search memories=${parsed.memories.length}`, types };
}
