import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { binVersion } from "./process.mjs";

const USERNAME = os.userInfo().username;
function reEscape(s) {
  return String(s).replace(/[.*+?^${}()|[\]\\]/g, String.raw`\$&`);
}
const HOME_RE = new RegExp(`(/home/|%2[Ff]home%2[Ff]|-home-)${reEscape(USERNAME)}-?`, "g");
const RUN_ID_RE = String.raw`\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}Z`;
const RUN_RE = new RegExp(
  String.raw`~(?:/\.cache/oboete-probes/|%2[Ff]\.cache%2[Ff]oboete-probes%2[Ff]|-cache-oboete-probes-)${RUN_ID_RE}`,
  "g",
);
const COMPACTION_KEY_EXACT =
  /^(compaction_?id|compact_?id|id|counter|seq(uence)?|ordinal|epoch|generation|timestamp|count)$/i;
const COMPACTION_KEY_SUFFIX = /(_id|Id)$/;

export const SUMMARY_KEYS = [
  "compact_summary",
  "compactSummary",
  "compaction_summary",
  "summary",
  "summary_text",
  "summaryText",
  "compactedSummary",
  "text",
];

const COMPACTION_EXCLUDE = new Set([
  "session_id",
  "sessionId",
  "transcript_path",
  "transcriptPath",
  "cwd",
  "hook_event_name",
  "hookEventName",
  "permission_mode",
  "permissionMode",
  "workspaceRoot",
  "prompt_id",
  "turn_id",
  "model",
  "trigger",
  "matcher",
  "source",
  "compact_summary",
  "compaction_summary",
  "summary",
]);

export function parseEvents(file) {
  if (!fs.existsSync(file)) return [];
  return fs
    .readFileSync(file, "utf8")
    .split("\n")
    .filter(Boolean)
    .map((l) => {
      try {
        return JSON.parse(l);
      } catch {
        return { event: "parse_error", raw: l.slice(0, 200) };
      }
    });
}

export function redactValue(value, repoPath, label = "<repo>") {
  const walk = (v) => {
    if (typeof v === "string") {
      let s = v;
      if (repoPath) s = s.split(repoPath).join(label);
      return s.replace(HOME_RE, "~").replace(RUN_RE, "<run>");
    }
    if (Array.isArray(v)) return v.map(walk);
    if (v && typeof v === "object") {
      const o = {};
      for (const [k, val] of Object.entries(v)) o[k] = walk(val);
      return o;
    }
    return v;
  };
  return walk(value);
}

export function toolUseIdOf(ev) {
  const s = ev?.stdin || {};
  return s.tool_use_id || s.toolUseId || s.toolCallId || null;
}

export function pairFor(events, name, preEvent = "PreToolUse", postEvent = "PostToolUse") {
  const pres = events.filter((e) => e.event === preEvent && toolNameOf(e) === name);
  const posts = events.filter((e) => e.event === postEvent && toolNameOf(e) === name);
  for (const pre of pres) {
    const id = toolUseIdOf(pre);
    const post = id ? posts.find((p) => toolUseIdOf(p) === id) : posts[0];
    if (post) return { pre, post };
  }
  return { pre: pres[0] || null, post: posts[0] || null };
}

export function keyDiff(obs, exp) {
  const missing = (exp || []).filter((k) => !obs.includes(k));
  const extra = obs.filter((k) => !(exp || []).includes(k));
  const bits = [];
  if (missing.length) bits.push("missing " + missing.join(","));
  if (extra.length) bits.push("extra " + extra.join(","));
  return bits.length ? bits.join("; ") : "match recon";
}

export function toolNameOf(ev) {
  const s = ev?.stdin || {};
  return s.toolName || s.tool_name || s.tool || null;
}

export function toolInputOf(ev) {
  const s = ev?.stdin || {};
  return s.toolInput || s.tool_input || s.input || null;
}

export function toolOutputOf(ev) {
  const s = ev?.stdin || {};
  if ("toolResult" in s) return s.toolResult;
  if ("tool_response" in s) return s.tool_response;
  if ("content" in s) return s.content;
  return undefined;
}

export function parseMaybeJson(text) {
  const t = (text || "").trim();
  if (!t) return null;
  try {
    return JSON.parse(t);
  } catch {
    /* fall through */
  }
  const start = t.indexOf("{");
  const end = t.lastIndexOf("}");
  if (start >= 0 && end > start) {
    try {
      return JSON.parse(t.slice(start, end + 1));
    } catch {
      return null;
    }
  }
  return null;
}

export function parseJsonl(text) {
  return (text || "")
    .split("\n")
    .filter((l) => l.trim().startsWith("{"))
    .map((l) => {
      try {
        return JSON.parse(l);
      } catch {
        return null;
      }
    })
    .filter(Boolean);
}

function claudeEnvelopeMeta(envelope, meta) {
  const env = envelope;
  meta.sessionId = env?.session_id || null;
  const mu = env?.modelUsage || {};
  let best = -1;
  for (const [id, u] of Object.entries(mu)) {
    const n = (u && (u.output_tokens ?? u.outputTokens)) || 0;
    if (n > best) {
      best = n;
      meta.model = id;
    }
  }
}

function codexEnvelopeMeta(proc, events, meta) {
  for (const line of parseJsonl(proc.stdout)) {
    if (line.type === "thread.started") meta.sessionId = line.thread_id || line.threadId || meta.sessionId;
  }
  for (const ev of events) {
    const s = ev.stdin || {};
    if (s.model) meta.model = s.model;
    if (s.session_id) meta.sessionId = meta.sessionId || s.session_id;
  }
}

function grokEnvelopeMeta(envelope, events, meta) {
  const env = envelope;
  meta.sessionId = env?.sessionId || env?.session_id || null;
  const mu = env?.modelUsage || {};
  meta.model = Object.keys(mu)[0] || null;
  for (const ev of events) {
    const s = ev.stdin || {};
    meta.sessionId = meta.sessionId || s.sessionId || s.session_id;
  }
}

function piEnvelopeMeta(proc, events, meta) {
  const lines = parseJsonl(proc.stdout);
  for (let i = lines.length - 1; i >= 0; i--) {
    if (lines[i].type === "turn_end" && lines[i].message?.model) {
      meta.model = lines[i].message.model;
      break;
    }
  }
  for (const ev of events) {
    if (ev.sessionId) meta.sessionId = ev.sessionId;
  }
}

function eventSessionId(events, sessionId) {
  for (const ev of events) {
    const s = ev.stdin || {};
    sessionId = s.session_id || s.sessionId || sessionId;
  }
  return sessionId;
}

function envelopeMeta(agent, proc, events, envelope) {
  const meta = { sessionId: null, model: null };
  if (agent === "claude") claudeEnvelopeMeta(envelope, meta);
  else if (agent === "codex") codexEnvelopeMeta(proc, events, meta);
  else if (agent === "grok") grokEnvelopeMeta(envelope, events, meta);
  else if (agent === "pi") piEnvelopeMeta(proc, events, meta);
  if (!meta.sessionId) meta.sessionId = eventSessionId(events, meta.sessionId);
  return meta;
}

export function piContentText(content) {
  if (!Array.isArray(content)) return typeof content === "string" ? content : "";
  return content
    .filter((b) => b?.type === "text")
    .map((b) => b.text || "")
    .join("");
}

function claudeFinalText(proc) {
  const env = parseMaybeJson(proc.stdout);
  return typeof env?.result === "string" ? env.result : undefined;
}

function codexFinalText(proc) {
  for (const line of parseJsonl(proc.stdout).reverse()) {
    if (line.type === "item.completed" && line.item?.text) return line.item.text;
  }
  return undefined;
}

function grokFinalText(proc, events) {
  for (const ev of events) {
    if (ev.event === "Stop") {
      const s = ev.stdin || {};
      if (s.reason === "end_turn" && s.lastAssistantMessage) return s.lastAssistantMessage;
    }
  }
  const env = parseMaybeJson(proc.stdout);
  return typeof env?.text === "string" ? env.text : undefined;
}

// Pi is the one agent with no fall-through: an unrecognised transcript is its trimmed stdout.
function piFinalText(proc) {
  const lines = parseJsonl(proc.stdout);
  for (let i = lines.length - 1; i >= 0; i--) {
    const l = lines[i];
    if (l.type === "turn_end" || l.type === "message_end") {
      const t = piContentText(l.message?.content);
      if (t) return t;
    }
  }
  return proc.stdout.trim();
}

// The first Stop event answers for every agent that has one, even when its message is empty.
function stopEventText(events) {
  for (const ev of events) {
    if (ev.event === "Stop") {
      const s = ev.stdin || {};
      return s.last_assistant_message || s.lastAssistantMessage || "";
    }
  }
  return undefined;
}

function agentFinalText(agent, proc, events) {
  if (agent === "claude") return claudeFinalText(proc);
  if (agent === "codex") return codexFinalText(proc);
  if (agent === "grok") return grokFinalText(proc, events);
  if (agent === "pi") return piFinalText(proc);
  return undefined;
}

export function finalText(agent, proc, events) {
  const own = agentFinalText(agent, proc, events);
  if (own !== undefined) return own;
  const stop = stopEventText(events);
  return stop === undefined ? proc.stdout.slice(-2000) : stop;
}

export function packResult(agent, dir, repo, tree, proc, eventsPath) {
  const events = parseEvents(eventsPath);
  const envelope = agent === "claude" || agent === "grok" ? parseMaybeJson(proc.stdout) : null;
  const { sessionId, model } = envelopeMeta(agent, proc, events, envelope);
  return {
    agent,
    exitCode: proc.exitCode,
    stdout: proc.stdout,
    stderr: proc.stderr,
    events,
    elapsedMs: proc.elapsedMs,
    sessionId,
    model,
    envelope,
    tree,
    repo,
    dir,
  };
}

export function topKeys(v) {
  if (v == null) return [];
  if (typeof v === "string") return ["(string)"];
  if (Array.isArray(v)) return ["(array)"];
  if (typeof v === "object") return Object.keys(v);
  return [typeof v];
}

export function named(events, name) {
  return (events || []).filter((e) => e.event === name);
}

export function summaryOf(stdin) {
  if (!stdin || typeof stdin !== "object") return { field: null, length: 0 };
  for (const k of SUMMARY_KEYS) {
    if (!(k in stdin) || stdin[k] == null || stdin[k] === "") continue;
    const v = stdin[k];
    return { field: k, length: typeof v === "string" ? v.length : JSON.stringify(v).length };
  }
  return { field: null, length: 0 };
}

function flattenOneLevel(stdin) {
  const s = stdin && typeof stdin === "object" && !Array.isArray(stdin) ? stdin : {};
  const out = { ...s };
  for (const v of Object.values(s)) {
    if (!v || typeof v !== "object" || Array.isArray(v)) continue;
    for (const [ik, iv] of Object.entries(v)) {
      if (!(ik in out)) out[ik] = iv;
    }
  }
  return out;
}

function compactionCandidates(payloads) {
  const keySet = new Set();
  for (const p of payloads) {
    for (const k of Object.keys(p)) {
      if (COMPACTION_EXCLUDE.has(k)) continue;
      if (COMPACTION_KEY_EXACT.test(k) || COMPACTION_KEY_SUFFIX.test(k)) keySet.add(k);
    }
  }
  return [...keySet];
}

export function compactionIdentity(posts) {
  const payloads = (posts || []).map((e) => flattenOneLevel(e?.stdin));
  const n = payloads.length;
  const candidates = compactionCandidates(payloads);
  const values = payloads.map((p) => Object.fromEntries(candidates.map((k) => [k, p[k]])));
  if (n === 1) {
    return { ok: false, candidates, values, n, note: `single observation; candidate keys = [${candidates.join(", ")}]` };
  }
  if (n < 2) return { ok: false, candidates, values, n, note: "no observations" };
  const sigs = values.map((v) => JSON.stringify(v));
  const unique = new Set(sigs).size === n;
  let note;
  if (unique) note = undefined;
  else if (candidates.length) note = "no distinguishing candidate";
  else note = "no candidate";
  return {
    ok: unique,
    candidates,
    values,
    n,
    note,
  };
}

export function grepLines(text, patterns) {
  const re = new RegExp(patterns.map((p) => p.replace(/[.*+?^${}()|[\]\\]/g, String.raw`\$&`)).join("|"), "i");
  return (text || "")
    .split("\n")
    .filter((l) => re.test(l))
    .slice(0, 30);
}

export function oversizedOutcome(r, dir) {
  const text = finalText(r.agent, r, r.events);
  const done = /\bDONE\b/.test(text);
  const stop = r.events.some((e) => e.event === "Stop");
  const end = r.events.some((e) => e.event === "SessionEnd");
  const unread = r.events.filter((e) => e.event === "PostToolUse-noread");
  const read = r.events.filter((e) => e.event === "PostToolUse");
  const sizes = read.map((e) => e.stdinBytes ?? JSON.stringify(e.stdin || {}).length);
  const hookHits = grepLines(r.stderr, [dir, "hook.mjs", "hook", "EPIPE", "SIGPIPE", "failed"]);
  const failedHook = /hook.*fail|EPIPE|SIGPIPE/i.test(r.stderr || "");
  return {
    status: done && stop && end && !failedHook ? "pass" : "fail",
    evidence: [
      `exit=${r.exitCode}`,
      `DONE=${done}`,
      `Stop=${stop}`,
      `SessionEnd=${end}`,
      `unread_handlers=${unread.length}`,
      `read_PostToolUse=${read.length} sizes=${sizes.join(",") || "none"}`,
      `hook_lines=${hookHits.length ? hookHits.slice(0, 5).join(" | ") : "none"}`,
      `elapsed_s=${(r.elapsedMs / 1000).toFixed(1)}`,
    ],
    data: { sizes, hookHits },
  };
}

export function stripFences(text) {
  const t = String(text || "").trim();
  if (t.length < 6 || !t.startsWith("```") || !t.endsWith("```")) return t;
  return t.slice(t.startsWith("```json") ? 7 : 3, -3).trim();
}

export function parseObservationsJson(text) {
  const stripped = stripFences(text);
  let obj;
  try {
    obj = JSON.parse(stripped);
  } catch {
    const start = stripped.indexOf("{");
    const end = stripped.lastIndexOf("}");
    if (start < 0 || end <= start) return null;
    try {
      obj = JSON.parse(stripped.slice(start, end + 1));
    } catch {
      return null;
    }
  }
  if (!obj || typeof obj !== "object") return null;
  if (!Array.isArray(obj.observations) || typeof obj.summary !== "string") return null;
  return obj;
}

export function writeFixture(repoRoot, rel, obj) {
  const dest = path.join(repoRoot, rel);
  const body = JSON.stringify(redactValue(obj, null), null, 2) + "\n";
  if (USERNAME && body.includes(USERNAME)) throw new Error("unredacted path in fixture");
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.writeFileSync(dest, body);
  return dest;
}

function recordShapeSuccess(options) {
  const { native, exp, r, preEvent, postEvent, evidence, ctx, fixtureDir, agent, version, captured_at, pre, post } = options;
  const inKeys = topKeys(toolInputOf(pre) || toolInputOf(post));
  let outKeys = topKeys(toolOutputOf(post));
  if (post.stdin && "details" in post.stdin && !outKeys.includes("details")) outKeys = [...outKeys, "details"];
  const diff = exp.output
    ? `${keyDiff(inKeys, exp.input)}; out ${keyDiff(outKeys, exp.output)}`
    : keyDiff(inKeys, exp.input);
  evidence.push(`${native} input=[${inKeys.join(",")}] output=[${outKeys.join(",")}] path=${exp.path} (${diff})`);
  const events = {
    [preEvent]: redactValue(pre.stdin, r.repo),
    [postEvent]: redactValue(post.stdin, r.repo),
  };
  writeFixture(ctx.repoRoot, `test/contracts/${fixtureDir}/${exp.file}`, {
    agent,
    agent_version: version,
    captured_at,
    native_tool: native,
    normalized_tool: exp.normalized,
    events,
    notes: exp.path,
  });
}

function recordMissingShape(native, pre, post, preEvent, postEvent, evidence, missing) {
  missing.push(native);
  evidence.push(`${native}: missing ${!pre ? preEvent : ""}${!post ? postEvent : ""}`);
}

export function shapeProbe({ agent, expected, launch, preEvent = "PreToolUse", postEvent = "PostToolUse", fixtureDir }) {
  return async (ctx) => {
    const r = await launch(ctx);
    const evidence = [];
    const missing = [];
    const version = binVersion(agent);
    const captured_at = new Date().toISOString();
    for (const [native, exp] of Object.entries(expected)) {
      const { pre, post } = pairFor(r.events, native, preEvent, postEvent);
      if (!pre || !post) {
        recordMissingShape(native, pre, post, preEvent, postEvent, evidence, missing);
        continue;
      }
      recordShapeSuccess({
        native,
        exp,
        r,
        preEvent,
        postEvent,
        evidence,
        ctx,
        fixtureDir,
        agent,
        version,
        captured_at,
        pre,
        post,
      });
    }
    evidence.push(
      `exit=${r.exitCode} elapsed_s=${(r.elapsedMs / 1000).toFixed(1)} session=${r.sessionId || "none"} model=${r.model || "none"}`,
    );
    return { status: missing.length ? "fail" : "pass", evidence, data: { missing, sessionId: r.sessionId, model: r.model } };
  };
}

export function eventsFile(home) {
  return path.join(home, "events.jsonl");
}

export function truncateEvents(home) {
  fs.writeFileSync(eventsFile(home), "");
}

export function saveFix(ctx, file, obj) {
  try {
    writeFixture(ctx.repoRoot, `test/contracts/grok/${file}`, obj);
    return file;
  } catch (e) {
    return "skip:" + (e?.message ? e.message : e);
  }
}
