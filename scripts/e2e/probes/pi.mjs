import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import {
  binVersion,
  compactionIdentity,
  finalText,
  named,
  redactValue,
  shapeProbe,
  toolUsePrompt,
  topKeys,
  writeFixture,
} from "../probe-lib/agents.mjs";

const ROW_SHAPES = "Native tool payload shapes for read/write/edit/bash on all four agents";
const ROW_OVER = "Hook runner behaviour when the hook exits with unread stdin above 1 MB";
const ROW_COMPACT = "Pi compaction event; Compaction identity and order per agent";
const ROW_TOOLS = "Pi tool registration surface";
const ROW_RESUME = "Pi `resume` / `fork`: `session_start` firing and `PI_SESSION_ID` continuity";
const ROW_ERROR = "Pi durable error surface for extension throws";
const ROW_AFTER = "Pi after_provider_response with openai-codex (recon follow-up)";

const COMPACT_SETTINGS = { compaction: { enabled: true, reserveTokens: 268000, keepRecentTokens: 1000 } };
const COMPACT_PROMPT =
  "Do these steps in order, without asking for confirmation. Each step names the exact tool to use; use that tool and no other. Do not substitute shell commands for the file tools. 1. Use the read tool on big.txt in the current directory. 2. Use the bash tool to run: echo step2 3. Reply with exactly the word DONE.";
const RESUME_PROMPT =
  "Run with the bash tool: node -e 'console.log(process.env.PI_SESSION_ID)' and reply with exactly the word DONE followed by the printed id";

const EXPECTED = {
  read: { file: "read.json", normalized: "read", input: ["path"], output: ["content"], path: "input.path" },
  write: { file: "write.json", normalized: "write", input: ["path", "content"], output: ["content"], path: "input.path" },
  edit: {
    file: "edit.json",
    normalized: "edit",
    input: ["path", "edits"],
    output: ["content", "details"],
    path: "input.path (edits is [{oldText,newText}])",
  },
  bash: { file: "bash.json", normalized: "bash", input: ["command"], output: ["content"], path: "input.command" },
};

function writeBigTxt(repo) {
  fs.mkdirSync(repo, { recursive: true });
  let s = "";
  for (let i = 0; s.length < 40960; i++) s += `line ${i} probe-compact-payload\n`;
  fs.writeFileSync(path.join(repo, "big.txt"), s.slice(0, 40960));
}

function writeTurnEndCompactExt(file) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(
    file,
    `import fs from "node:fs";
const OUT = process.env.PROBE_EVENTS || "";
let n = 0;
function rec(obj) {
  if (!OUT) return;
  try { fs.appendFileSync(OUT, JSON.stringify(obj) + "\\n"); } catch { /* ignore */ }
}
export default (pi) => {
  pi.on("turn_end", async (_e, ctx) => {
    let usage = null;
    try { usage = typeof ctx.getContextUsage === "function" ? ctx.getContextUsage() : null; } catch (err) { usage = String(err); }
    rec({ event: "probe_context_usage", stdin: usage, at: new Date().toISOString(), n });
    if (n !== 0) return;
    n = 1;
    await new Promise((resolve) => {
      const t = setTimeout(() => { rec({ event: "probe_compact_timeout", at: new Date().toISOString() }); resolve(); }, 120000);
      const done = () => { clearTimeout(t); resolve(); };
      try {
        ctx.compact({
          onComplete: (result) => { rec({ event: "probe_compact_complete", stdin: result, at: new Date().toISOString() }); done(); },
          onError: (error) => { rec({ event: "probe_compact_error", stdin: { message: String(error && error.message ? error.message : error) }, at: new Date().toISOString() }); done(); },
        });
        rec({ event: "probe_compact_called", stdin: { n: 1 }, at: new Date().toISOString() });
      } catch (err) {
        rec({ event: "probe_compact_throw", stdin: String(err), at: new Date().toISOString() });
        done();
      }
    });
  });
};
`,
  );
}

function compactBits(ev) {
  const s = ev.stdin || {};
  const ce = s.compactionEntry && typeof s.compactionEntry === "object" ? s.compactionEntry : {};
  const prep = s.preparation && typeof s.preparation === "object" ? s.preparation : {};
  return {
    event: ev.event,
    at: ev.at,
    keys: topKeys(s),
    reason: s.reason,
    willRetry: s.willRetry,
    fromExtension: s.fromExtension,
    id: ce.id || null,
    summaryLen: typeof ce.summary === "string" ? ce.summary.length : null,
    firstKeptEntryId: ce.firstKeptEntryId || prep.firstKeptEntryId || null,
    ceKeys: topKeys(ce),
    prepKeys: topKeys(prep),
  };
}

function sessionFileOf(r) {
  for (const ev of r.events || []) {
    if (ev.sessionFile && ev.sessionFile !== "[throw]") return ev.sessionFile;
  }
  const dir = path.join(r.tree, "sessions");
  if (!fs.existsSync(dir)) return null;
  const files = fs.readdirSync(dir).filter((f) => f.endsWith(".jsonl")).map((f) => path.join(dir, f));
  return files[0] || null;
}

function sessionJsonlPaths(tree) {
  const dir = path.join(tree, "sessions");
  if (!fs.existsSync(dir)) return [];
  return fs.readdirSync(dir).filter((f) => f.endsWith(".jsonl")).map((f) => path.join(dir, f));
}

function entryPath(root, e) {
  return path.join(e.parentPath ?? e.path ?? root, e.name);
}

function hasNodeModules(p) {
  return String(p).split(path.sep).includes("node_modules");
}

function parentDepth(root, parent) {
  const rel = path.relative(root, parent);
  if (!rel || rel === ".") return 0;
  return rel.split(path.sep).length;
}

function listTree(root, maxDepth = 2) {
  if (!fs.existsSync(root)) return { exists: false, entries: [] };
  let ents;
  try {
    ents = fs.readdirSync(root, { recursive: true, withFileTypes: true });
  } catch {
    return { exists: true, entries: [] };
  }
  const entries = [];
  for (const e of ents) {
    const parent = e.parentPath ?? e.path ?? root;
    const fp = path.join(parent, e.name);
    if (hasNodeModules(fp)) continue;
    if (parentDepth(root, parent) > maxDepth) continue;
    let st;
    try {
      st = fs.statSync(fp);
    } catch {
      continue;
    }
    const rel = path.relative(root, fp).split(path.sep).join("/");
    entries.push({ path: rel, size: st.size, mtimeMs: st.mtimeMs, dir: st.isDirectory() });
  }
  entries.sort((a, b) => a.path.localeCompare(b.path));
  return { exists: true, entries };
}

function listingDiff(before, after) {
  const b = new Map((before.entries || []).map((e) => [e.path, e]));
  const added = [];
  const changed = [];
  for (const e of after.entries || []) {
    const prev = b.get(e.path);
    if (!prev) added.push(e.path + " size=" + e.size);
    else if (prev.size !== e.size || prev.mtimeMs !== e.mtimeMs) changed.push(e.path + " size " + prev.size + "->" + e.size);
  }
  return { added, changed };
}

function toolText(content) {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .filter((b) => b && b.type === "text")
    .map((b) => b.text || "")
    .join("");
}

function bashChildId(r) {
  for (const ev of named(r.events, "tool_result")) {
    const s = ev.stdin || {};
    if (s.toolName !== "bash") continue;
    const t = toolText(s.content).trim();
    if (t) return t.split(/\s+/)[0];
  }
  return null;
}

function nextAfter(events, compactAt, names) {
  const t = Date.parse(compactAt);
  if (!Number.isFinite(t)) return null;
  let best = null;
  for (const ev of events) {
    if (!names.includes(ev.event) || !ev.at) continue;
    const n = Date.parse(ev.at);
    if (!Number.isFinite(n) || n <= t) continue;
    if (!best || n < Date.parse(best.at)) best = ev;
  }
  return best;
}

function searchLines(text, re) {
  return (text || "")
    .split("\n")
    .filter((l) => re.test(l))
    .slice(0, 20);
}

async function launchCompact(ctx, dir, extPath) {
  fs.mkdirSync(dir, { recursive: true });
  const repo = path.join(dir, "repo");
  writeBigTxt(repo);
  return ctx.pi(dir, {
    repo,
    prompt: COMPACT_PROMPT,
    settings: COMPACT_SETTINGS,
    extraArgs: ["-e", extPath],
  });
}

export const probes = [
  {
    id: "pi-payload-shapes",
    agent: "pi",
    row: ROW_SHAPES,
    run: shapeProbe({
      agent: "pi",
      expected: EXPECTED,
      launch: (ctx) => ctx.pi(ctx.dir, { prompt: toolUsePrompt("pi") }),
      preEvent: "tool_call",
      postEvent: "tool_result",
      fixtureDir: "pi",
    }),
  },
  {
    id: "pi-oversized-stdin",
    agent: "pi",
    row: ROW_OVER,
    async run() {
      return {
        status: "skipped",
        evidence: [
          "Pi has no hook process; the equivalent is oboete's own capture child, probed after the child exists",
        ],
      };
    },
  },
  {
    id: "pi-compaction",
    agent: "pi",
    row: ROW_COMPACT,
    async run(ctx) {
      const extPath = path.join(ctx.dir, "compact-at-turn-end.ts");
      writeTurnEndCompactExt(extPath);
      const r1 = await launchCompact(ctx, path.join(ctx.dir, "run1"), extPath);
      const file1 = sessionFileOf(r1);
      let r2 = null;
      if (file1) {
        const dir2 = path.join(ctx.dir, "run2");
        fs.mkdirSync(dir2, { recursive: true });
        r2 = await ctx.pi(dir2, {
          repo: r1.repo,
          prompt: COMPACT_PROMPT,
          settings: COMPACT_SETTINGS,
          extraArgs: ["--session", file1, "-e", extPath],
        });
      }
      const events = [...(r1.events || []), ...(r2?.events || [])];
      const before = named(events, "session_before_compact").map(compactBits);
      const compactEvs = named(events, "session_compact");
      const compact = compactEvs.map(compactBits);
      const failed = named(events, "session_compact_failed").map(compactBits);
      const ident = compactionIdentity(compactEvs);
      const ids = compact.map((c) => c.id).filter(Boolean);
      const uniqueIds = [...new Set(ids)];
      const a = ident.ok;
      const firstCompact = compact[0];
      const injectNames = ["before_agent_start", "turn_start", "context"];
      const nextInj = firstCompact ? nextAfter(events, firstCompact.at, injectNames) : null;
      const b = Boolean(firstCompact && nextInj && Date.parse(nextInj.at) > Date.parse(firstCompact.at));
      const usages = named(events, "probe_context_usage").map((e) => e.stdin);
      const lastUsage = usages.length ? usages[usages.length - 1] : null;
      if (compact.length) {
        writeFixture(ctx.repoRoot, "test/contracts/pi/compaction.json", {
          agent: "pi",
          agent_version: binVersion("pi"),
          captured_at: new Date().toISOString(),
          events: {
            session_before_compact: named(events, "session_before_compact").map((e) => redactValue(e.stdin, r1.repo)),
            session_compact: named(events, "session_compact").map((e) => redactValue(e.stdin, r1.repo)),
          },
          notes: "run1 extra ext ctx.compact() on first turn_end; run2 --session same file + same ext",
        });
      }
      const evidence = [
        `run1 exit=${r1.exitCode} elapsed_s=${(r1.elapsedMs / 1000).toFixed(1)} session=${r1.sessionId || "none"}`,
        `run2 exit=${r2 ? r2.exitCode : "skipped"} elapsed_s=${r2 ? (r2.elapsedMs / 1000).toFixed(1) : "n/a"} session=${r2?.sessionId || "none"}`,
        `session_before_compact=${before.length} ${before.map((c) => `keys=[${c.keys}] reason=${c.reason} firstKept=${c.firstKeptEntryId}`).join(" | ") || "none"}`,
        `session_compact=${compact.length} ${compact.map((c) => `keys=[${c.keys}] id=${c.id} summaryLen=${c.summaryLen} firstKept=${c.firstKeptEntryId} reason=${c.reason} at=${c.at} ceKeys=[${c.ceKeys}]`).join(" | ") || "none"}`,
        `session_compact_failed=${failed.length} ${failed.map((c) => `reason=${c.reason}`).join(" | ") || "none"}`,
        `compactionEntry.ids=${ids.join(",") || "none"} unique=${uniqueIds.length} (a) distinguishing=${a} candidates=[${ident.candidates.join(",")}] note=${ident.note || ""}`,
        `first compact at=${firstCompact?.at || "none"} next inject ${nextInj ? nextInj.event + " at=" + nextInj.at : "none"} (b) committed_before_next_inject=${b}`,
        `probe_compact_called=${named(events, "probe_compact_called").length} complete=${named(events, "probe_compact_complete").length} error=${JSON.stringify(named(events, "probe_compact_error").map((e) => e.stdin))}`,
        `getContextUsage=${JSON.stringify(lastUsage)}`,
        `DONE r1=${/\bDONE\b/.test(finalText("pi", r1, r1.events))} r2=${r2 ? /\bDONE\b/.test(finalText("pi", r2, r2.events)) : "n/a"}`,
      ];
      let status = "pass";
      if (!compact.length && !before.length) {
        status = "blocked";
        evidence.push("no compaction triggered; token count from ctx.getContextUsage() above");
      } else if (!a || !b) {
        status = "fail";
      }
      return { status, evidence, data: { a, b, ids: uniqueIds, before, compact, failed } };
    },
  },
  {
    id: "pi-tools",
    agent: "pi",
    row: ROW_TOOLS,
    async run(ctx) {
      const r = await ctx.pi(ctx.dir, {
        env: { PROBE_TOOL: "1" },
        prompt:
          "Call the tool oboete_probe with no arguments and reply with exactly the word DONE followed by the tool result",
      });
      const calls = named(r.events, "tool_call").filter((e) => (e.stdin || {}).toolName === "oboete_probe");
      const results = named(r.events, "tool_result").filter((e) => (e.stdin || {}).toolName === "oboete_probe");
      const start = named(r.events, "before_agent_start")[0];
      const selected = start?.stdin?.systemPromptOptions?.selectedTools;
      const inSelected = Array.isArray(selected) && selected.includes("oboete_probe");
      const resultText = results.map((e) => toolText((e.stdin || {}).content)).join(" ");
      const text = finalText("pi", r, r.events);
      const echoed = /oboete_probe ok/i.test(text) || /oboete_probe ok/i.test(resultText);
      const called = calls.length > 0 && results.length > 0;
      if (calls[0] || results[0]) {
        writeFixture(ctx.repoRoot, "test/contracts/pi/oboete_probe.json", {
          agent: "pi",
          agent_version: binVersion("pi"),
          captured_at: new Date().toISOString(),
          native_tool: "oboete_probe",
          events: {
            tool_call: redactValue(calls[0]?.stdin || null, r.repo),
            tool_result: redactValue(results[0]?.stdin || null, r.repo),
          },
          selectedTools: selected || null,
        });
      }
      const evidence = [
        `exit=${r.exitCode} elapsed_s=${(r.elapsedMs / 1000).toFixed(1)}`,
        `tool_call n=${calls.length} toolName=${calls[0]?.stdin?.toolName || "none"} input=${JSON.stringify(calls[0]?.stdin?.input ?? null)} keys=[${topKeys(calls[0]?.stdin)}]`,
        `tool_result n=${results.length} toolName=${results[0]?.stdin?.toolName || "none"} content=${JSON.stringify(results[0]?.stdin?.content ?? null)} keys=[${topKeys(results[0]?.stdin)}]`,
        `before_agent_start.systemPromptOptions.selectedTools=${JSON.stringify(selected || null)} includes_oboete_probe=${inSelected}`,
        `model_text=${JSON.stringify(text).slice(0, 300)} echoed=${echoed}`,
      ];
      return { status: called && echoed ? "pass" : "fail", evidence, data: { called, echoed, inSelected } };
    },
  },
  {
    id: "pi-resume-fork",
    agent: "pi",
    row: ROW_RESUME,
    async run(ctx) {
      const rA = await ctx.pi(path.join(ctx.dir, "A"), { prompt: "Reply with exactly the word DONE" });
      const fileA = sessionFileOf(rA);
      const idA = rA.sessionId;
      if (!fileA) {
        return { status: "fail", evidence: [`run A produced no sessionFile exit=${rA.exitCode} session=${idA || "none"}`] };
      }
      const rB = await ctx.pi(path.join(ctx.dir, "B"), {
        repo: rA.repo,
        prompt: RESUME_PROMPT,
        extraArgs: ["--session", fileA],
      });
      const rC = await ctx.pi(path.join(ctx.dir, "C"), {
        repo: rA.repo,
        prompt: RESUME_PROMPT,
        extraArgs: ["--fork", fileA],
      });
      const reason = (r) => named(r.events, "session_start")[0]?.stdin?.reason ?? null;
      const reasons = { A: reason(rA), B: reason(rB), C: reason(rC) };
      const ids = { A: rA.sessionId, B: rB.sessionId, C: rC.sessionId };
      const bash = { B: bashChildId(rB), C: bashChildId(rC) };
      const textB = finalText("pi", rB, rB.events);
      const textC = finalText("pi", rC, rC.events);
      const resumeKeeps = ids.A && ids.A === ids.B;
      const forkChanges = ids.C && ids.C !== ids.A;
      const bashEq = bash.B === ids.B && bash.C === ids.C && bash.B && bash.C;
      const evidence = [
        `session_start.reason A=${reasons.A} B=${reasons.B} C=${reasons.C}`,
        `sessionId A=${ids.A} B=${ids.B} C=${ids.C} A==B=${resumeKeeps} C!=A=${forkChanges}`,
        `sessionFile A=${fileA} B=${sessionFileOf(rB)} C=${sessionFileOf(rC)}`,
        `bash PI_SESSION_ID B=${bash.B} C=${bash.C} equals_extension B=${bash.B === ids.B} C=${bash.C === ids.C}`,
        `text B=${JSON.stringify(textB).slice(0, 200)} C=${JSON.stringify(textC).slice(0, 200)}`,
        `exit A=${rA.exitCode} B=${rB.exitCode} C=${rC.exitCode}`,
      ];
      const pass = resumeKeeps && forkChanges && bashEq;
      return { status: pass ? "pass" : "fail", evidence, data: { reasons, ids, bash } };
    },
  },
  {
    id: "pi-error-surface",
    agent: "pi",
    row: ROW_ERROR,
    async run(ctx) {
      const real = path.join(os.homedir(), ".pi/agent");
      const beforeReal = listTree(real, 2);
      const r = await ctx.pi(ctx.dir, {
        env: { PROBE_THROW: "before_agent_start" },
        prompt: "Reply with exactly the word DONE",
      });
      const afterReal = listTree(real, 2);
      const tmpDiff = listTree(r.tree, 3);
      const realDiff = listingDiff(beforeReal, afterReal);
      const THROW = "probe throw at before_agent_start";
      const throwRe = /probe throw at before_agent_start/;
      const stderrHits = searchLines(r.stderr, /Extension error|probe throw/);
      const stdoutHits = searchLines(r.stdout, throwRe);
      const stdoutTypes = [];
      for (const line of (r.stdout || "").split("\n")) {
        try {
          const o = JSON.parse(line);
          if (o && /error/i.test(String(o.type || ""))) stdoutTypes.push(o.type);
        } catch {
          /* skip */
        }
      }
      const sessionTypes = [];
      const durable = [];
      for (const f of sessionJsonlPaths(r.tree)) {
        const body = fs.readFileSync(f, "utf8");
        for (const line of body.split("\n").filter(Boolean)) {
          try {
            sessionTypes.push(JSON.parse(line).type);
          } catch {
            /* skip */
          }
        }
        const hits = searchLines(body, throwRe);
        if (hits.length) durable.push({ path: f, hits });
      }
      const skipLog = new Set(["events.jsonl", "stdout.txt", "stderr.txt"]);
      const walkLogs = (root) => {
        if (!fs.existsSync(root)) return [];
        let ents;
        try {
          ents = fs.readdirSync(root, { recursive: true, withFileTypes: true });
        } catch {
          return [];
        }
        const out = [];
        for (const e of ents) {
          if (!e.isFile()) continue;
          const fp = entryPath(root, e);
          if (hasNodeModules(fp)) continue;
          if (!/\.(log|txt|jsonl)$/i.test(e.name) || skipLog.has(e.name)) continue;
          const body = fs.readFileSync(fp, "utf8");
          if (throwRe.test(body)) out.push({ path: fp, hits: searchLines(body, throwRe) });
        }
        return out;
      };
      const tmpLogs = walkLogs(r.tree);
      const realLogs = [];
      for (const e of afterReal.entries || []) {
        if (e.dir || !/\.(log|txt)$/i.test(e.path)) continue;
        const p = path.join(real, e.path);
        try {
          if (fs.existsSync(p) && throwRe.test(fs.readFileSync(p, "utf8"))) realLogs.push(p);
        } catch {
          /* unreadable */
        }
      }
      const text = finalText("pi", r, r.events);
      const continued = /\bDONE\b/.test(text) || named(r.events, "agent_settled").length > 0;
      const durableNamed = durable[0]
        ? durable[0].path + " :: " + durable[0].hits[0]
        : tmpLogs[0]
          ? tmpLogs[0].path + " :: " + tmpLogs[0].hits[0]
          : realLogs[0] || null;
      const evidence = [
        `exit=${r.exitCode} elapsed_s=${(r.elapsedMs / 1000).toFixed(1)} continued=${continued} text=${JSON.stringify(text).slice(0, 200)}`,
        `stderr hits=${stderrHits.length ? stderrHits.slice(0, 5).join(" | ") : "none"}`,
        `stdout error types=${stdoutTypes.join(",") || "none"} stdout hits=${stdoutHits.length ? stdoutHits.slice(0, 3).join(" | ") : "none"}`,
        `session jsonl types=[${sessionTypes.join(",")}] throw records=${durable.length ? JSON.stringify(redactValue(durable, r.repo)).slice(0, 500) : "none"}`,
        `piagent logs with throw=${tmpLogs.length ? JSON.stringify(redactValue(tmpLogs, r.repo)).slice(0, 400) : "none"}`,
        `~/.pi/agent added=${realDiff.added.join(",") || "none"} changed=${realDiff.changed.join(",") || "none"} throw logs=${realLogs.join(",") || "none"}`,
        `durable=${durableNamed || "no durable record"} (stderr/in-memory only unless a path is named)`,
        `tmp tree files=${(tmpDiff.entries || []).map((e) => e.path).slice(0, 40).join(",")}`,
      ];
      const status = durableNamed ? "pass" : "fail";
      return { status, evidence, data: { continued, durableNamed, stderrHits, throw: THROW } };
    },
  },
  {
    id: "pi-after-provider-response",
    agent: "pi",
    row: ROW_AFTER,
    async run(ctx) {
      const r = await ctx.pi(ctx.dir, { prompt: "Reply with exactly the word DONE" });
      const after = named(r.events, "after_provider_response");
      const before = named(r.events, "before_provider_request");
      const evidence = [
        `exit=${r.exitCode} elapsed_s=${(r.elapsedMs / 1000).toFixed(1)} model=${r.model || "none"}`,
        `after_provider_response count=${after.length} keys=${after[0] ? "[" + topKeys(after[0].stdin) + "]" : "n/a"}`,
        `before_provider_request count=${before.length}`,
        `event names=${[...new Set((r.events || []).map((e) => e.event))].join(",")}`,
      ];
      return { status: "pass", evidence, data: { after: after.length, before: before.length } };
    },
  },
];
