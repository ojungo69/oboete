import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  CODEX_EVENTS,
  PreconditionError,
  agentPath,
  binVersion,
  childEnv,
  compactionIdentity,
  oversizedOutcome,
  oversizedPrompt,
  parseEvents,
  redactValue,
  runTimed,
  shellQuote,
  summaryOf,
  toolInputOf,
  toolNameOf,
  toolOutputOf,
  toolUseIdOf,
  toolUsePrompt,
  topKeys,
  waitUntil,
  writeFixture,
} from "../probe-lib/agents.mjs";
import { readMcpFrames } from "../probe-lib/mcp-frames.mjs";
import { TRUST_PANE_RE, readyTui, tmux, tmuxSession, tuiCmd, tuiQuit, tuiSubmit } from "../probe-lib/tmux.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const MCP_DUMMY = path.join(HERE, "../probe-lib/mcp-dummy.mjs");

const ROW_SHAPES = "Native tool payload shapes for read/write/edit/bash on all four agents";
const ROW_OVER = "Hook runner behaviour when the hook exits with unread stdin above 1 MB";
const ROW_TRUST = "Codex rollout flush at PostToolUse; TUI trust path";
const ROW_SESSION = "Codex `SessionStart` fires with `source = compact` and `clear`";
const ROW_POSTCOMPACT =
  "Codex and Grok `PostCompact` payload (summary text field); Compaction identity and order per agent";
const ROW_FLUSH = "Codex rollout flush at `PostToolUse`";
const ROW_TUI = "TUI trust path";
const ROW_MCP = "Legacy-era MCP server against Claude Code, Codex, Grok clients (raw frames compared)";

const COMPACT_ARGS = [
  "-c",
  "model_auto_compact_token_limit=2000",
  "-c",
  'model_auto_compact_token_limit_scope="body_after_prefix"',
];
const COMPACT_PROMPT =
  "Use a shell command to read the file big.txt in the current directory twice (run: cat big.txt; cat big.txt). Then reply with exactly the word DONE.";
const DONE_PROMPT = "Reply with exactly the word DONE.";
const RESUME_PROMPT = "Reply with exactly the word DONE followed by every marker token you have seen.";
const TUI_PROMPT = "Use the shell to run: echo tui-ok ; then reply with exactly the word DONE";
const MCP_PROMPT =
  "Call the MCP tool oboete_probe/search with query hello and reply DONE followed by the tool result";

function cmdOf(ev) {
  const input = toolInputOf(ev) || {};
  return typeof input.command === "string" ? input.command : "";
}

function classifyCodex(ev) {
  const name = toolNameOf(ev);
  const cmd = cmdOf(ev);
  if (name === "apply_patch" && /\*\*\* Add File:/.test(cmd)) return "apply_patch-add";
  if (name === "apply_patch" && /\*\*\* Update File:/.test(cmd)) return "apply_patch-update";
  if (name === "Bash" && /README/.test(cmd)) return "bash-read";
  if (name === "Bash") return "bash";
  return null;
}

const EXPECTED = {
  "bash-read": {
    file: "bash-read.json",
    native: "Bash",
    normalized: "read",
    input: ["command"],
    path: "tool_input.command — path is inside the shell command text",
  },
  "apply_patch-add": {
    file: "apply_patch-add.json",
    native: "apply_patch",
    normalized: "write",
    input: ["command"],
    path: "tool_input.command *** Add File: <path>",
  },
  "apply_patch-update": {
    file: "apply_patch-update.json",
    native: "apply_patch",
    normalized: "edit",
    input: ["command"],
    path: "tool_input.command *** Update File: <path>",
  },
  bash: {
    file: "bash.json",
    native: "Bash",
    normalized: "bash",
    input: ["command"],
    path: "tool_input.command",
  },
};

function eventsFile(home) {
  return path.join(home, "events.jsonl");
}

function truncateEvents(home) {
  fs.writeFileSync(eventsFile(home), "");
}

function sessionSources(events) {
  return events.filter((e) => e.event === "SessionStart").map((e) => e.stdin?.source ?? "missing");
}

function writeBigFile(repo) {
  fs.mkdirSync(repo, { recursive: true });
  const lines = [];
  for (let i = 0; i < 4000; i++) lines.push(`token-line-${i}-unique-payload-${"abcdefghij".repeat(6)}`);
  const dest = path.join(repo, "big.txt");
  fs.writeFileSync(dest, lines.join("\n"));
  return dest;
}

function saveText(dir, name, text) {
  fs.writeFileSync(path.join(dir, name), text == null ? "" : String(text));
}

function scrub(v) {
  const u = os.userInfo().username;
  return JSON.parse(JSON.stringify(redactValue(v, null)).split(u).join("<user>"));
}

function tryFixture(repoRoot, rel, obj) {
  try {
    writeFixture(repoRoot, rel, scrub(obj));
    return null;
  } catch (e) {
    return String(e && e.message ? e.message : e);
  }
}

function widenSessionStart(home) {
  const p = path.join(home, "hooks.json");
  const j = JSON.parse(fs.readFileSync(p, "utf8"));
  for (const g of j.hooks?.SessionStart || []) g.matcher = "startup|resume|clear|compact|new|fork";
  fs.writeFileSync(p, JSON.stringify(j, null, 2));
}

function compactTimeline(events) {
  return events
    .filter((e) => ["PreCompact", "PostCompact", "SessionStart", "UserPromptSubmit"].includes(e.event))
    .map((e) => ({
      event: e.event,
      at: e.at || null,
      source: e.stdin?.source ?? null,
      trigger: e.stdin?.trigger ?? null,
      turn_id: e.stdin?.turn_id ?? null,
      session_id: e.stdin?.session_id ?? null,
      keys: topKeys(e.stdin),
    }));
}

function orderOk(events) {
  const posts = events.filter((e) => e.event === "PostCompact");
  if (!posts.length) return { ok: false, detail: "no PostCompact" };
  const post = posts[posts.length - 1];
  const after = events.filter((e) => e.at && post.at && e.at >= post.at);
  const ss = after.find((e) => e.event === "SessionStart" && e.stdin?.source === "compact");
  const ups = after.find((e) => e.event === "UserPromptSubmit");
  const bits = [];
  if (ss) bits.push(`PostCompact.at=${post.at} <= SessionStart(compact).at=${ss.at}`);
  else bits.push("no SessionStart source=compact after last PostCompact");
  if (ups) bits.push(`PostCompact.at=${post.at} <= next UserPromptSubmit.at=${ups.at}`);
  else bits.push("no UserPromptSubmit after last PostCompact");
  return { ok: Boolean(ss) && (!ups || post.at <= ups.at) && post.at <= (ss?.at || post.at), detail: bits.join("; "), ss, ups, post };
}

async function waitEvents(home, pred, ms) {
  return (
    (await waitUntil(() => {
      const ev = parseEvents(eventsFile(home));
      return pred(ev) ? ev : null;
    }, ms, 250)) || parseEvents(eventsFile(home))
  );
}

async function withTui(dir, { home, repo, extra = [], run }) {
  const name = "obc-" + Date.now().toString(36) + "-" + Math.random().toString(36).slice(2, 6);
  let tui;
  try {
    // Only pane overrides go into tmux's world-readable argv; HOME comes from the server.
    tui = tmuxSession({
      name, command: tuiCmd(extra).map(shellQuote).join(" "), cwd: repo,
      env: { CODEX_HOME: home, PATH: agentPath(), TERM: "xterm-256color" },
    });
    return await run(tui, name);
  } finally {
    try {
      tui?.kill();
    } catch {
      /* ignore */
    }
    tmux(["kill-session", "-t", name]);
  }
}

async function waitPane(tui, re, ms) {
  let p = "";
  await waitUntil(() => {
    p = tui.capture();
    return re.test(p) ? p : null;
  }, ms, 200);
  return p || tui.capture();
}

async function tuiComposerTurn(tui, name, home, dir) {
  try {
    await readyTui("codex", tui, { timeoutMs: 25_000 });
  } finally {
    saveText(dir, "pane-start.txt", tui.capture());
  }
  await tuiSubmit(name, tui, TUI_PROMPT, { timeoutMs: 25_000 });
  await waitEvents(home, (ev) => ev.some((e) => e.event === "Stop" || e.event === "UserPromptSubmit"), 45_000);
  saveText(dir, "pane-turn.txt", tui.capture());
}

const TUI_MANUAL =
  "manual: CODEX_HOME=<tmp hooks.json> tmux `codex --sandbox danger-full-access --ask-for-approval never` in the throwaway repo; wait for composer (›); send a short turn; /compact; /new (expect SessionStart source=clear); /quit. Record events.jsonl labels and pane text.";

export const probes = [
  {
    id: "codex-payload-shapes",
    agent: "codex",
    row: ROW_SHAPES,
    async run(ctx) {
      const r = await ctx.codex(ctx.dir, { prompt: toolUsePrompt("codex") });
      const evidence = [];
      const missing = [];
      const version = binVersion("codex");
      const captured_at = new Date().toISOString();
      const byKind = {};
      for (const ev of r.events) {
        if (ev.event !== "PreToolUse" && ev.event !== "PostToolUse") continue;
        const kind = classifyCodex(ev);
        if (!kind) continue;
        const id = toolUseIdOf(ev) || "_";
        byKind[kind] ||= {};
        byKind[kind][id] ||= {};
        byKind[kind][id][ev.event] = ev;
      }
      for (const [kind, exp] of Object.entries(EXPECTED)) {
        const matched = Object.values(byKind[kind] || {}).find((p) => p.PreToolUse && p.PostToolUse);
        const pre = matched?.PreToolUse;
        const post = matched?.PostToolUse;
        if (!pre || !post) {
          missing.push(kind);
          evidence.push(`${kind}: missing ${!pre ? "Pre" : ""}${!post ? "Post" : ""}`);
          continue;
        }
        const inKeys = topKeys(toolInputOf(pre));
        const outKeys = topKeys(toolOutputOf(post));
        evidence.push(`${kind} input=[${inKeys.join(",")}] output=[${outKeys.join(",")}] path=${exp.path}`);
        writeFixture(ctx.repoRoot, `test/contracts/codex/${exp.file}`, {
          agent: "codex",
          agent_version: version,
          captured_at,
          native_tool: exp.native,
          normalized_tool: exp.normalized,
          events: {
            PreToolUse: redactValue(pre.stdin, r.repo),
            PostToolUse: redactValue(post.stdin, r.repo),
          },
          notes: exp.path + "; tool_response is a bare string",
        });
      }
      evidence.push(`exit=${r.exitCode} elapsed_s=${(r.elapsedMs / 1000).toFixed(1)} session=${r.sessionId || "none"} model=${r.model || "none"}`);
      return { status: missing.length ? "fail" : "pass", evidence, data: { missing, sessionId: r.sessionId, model: r.model } };
    },
  },
  {
    id: "codex-oversized-stdin",
    agent: "codex",
    row: ROW_OVER,
    async run(ctx) {
      const hooks = [
        ...CODEX_EVENTS.filter((e) => e !== "PostToolUse"),
        { event: "PostToolUse", flags: ["--no-read"], label: "PostToolUse-noread" },
        { event: "PostToolUse", flags: [], label: "PostToolUse" },
      ];
      return oversizedOutcome(await ctx.codex(ctx.dir, { prompt: oversizedPrompt("Bash"), hooks }), ctx.dir);
    },
  },
  {
    id: "codex-trust-hash",
    agent: "codex",
    row: ROW_TRUST,
    async run(ctx) {
      const r = await ctx.codex(ctx.dir, { prompt: toolUsePrompt("codex"), trust: true });
      const names = r.events.map((e) => e.event);
      const need = ["SessionStart", "PreToolUse", "PostToolUse", "Stop"];
      const missing = need.filter((n) => !names.includes(n));
      const cfg = path.join(r.tree, "config.toml");
      const toml = fs.existsSync(cfg) ? fs.readFileSync(cfg, "utf8") : "";
      const rows = (toml.match(/trusted_hash/g) || []).length;
      return {
        status: missing.length ? "fail" : "pass",
        evidence: [`events=${[...new Set(names)].join(",")}`, `trust_rows=${rows}`, `exit=${r.exitCode}`],
        data: { missing, rows },
      };
    },
  },
  {
    id: "codex-session-start-sources",
    agent: "codex",
    row: ROW_SESSION,
    async run(ctx) {
      const evidence = [];
      const observed = [];
      const dirA = path.join(ctx.dir, "a");
      const a = await ctx.codex(dirA, { prompt: DONE_PROMPT });
      const srcA = sessionSources(a.events);
      observed.push(...srcA);
      evidence.push(`A_startup sources=[${srcA.join(",")}] session=${a.sessionId || "none"} exit=${a.exitCode}`);
      const threadId = a.sessionId;
      if (!threadId) {
        evidence.push("A produced no session_id; resume skipped");
      } else {
        fs.copyFileSync(path.join(dirA, "stdout.txt"), path.join(dirA, "a-stdout.txt"));
        fs.copyFileSync(path.join(dirA, "stderr.txt"), path.join(dirA, "a-stderr.txt"));
        fs.copyFileSync(path.join(a.tree, "events.jsonl"), path.join(dirA, "a-events.jsonl"));
        const b = await ctx.codex(dirA, { extraArgs: ["resume", threadId], prompt: RESUME_PROMPT });
        const srcB = sessionSources(b.events);
        observed.push(...srcB);
        const same = (b.sessionId || "") === threadId || b.events.some((e) => e.stdin?.session_id === threadId);
        evidence.push(
          `B_resume sources=[${srcB.join(",")}] session=${b.sessionId || "none"} same_session_id=${same} exit=${b.exitCode}`,
        );
      }

      const dirC = path.join(ctx.dir, "c");
      fs.mkdirSync(path.join(dirC, "repo"), { recursive: true });
      writeBigFile(path.join(dirC, "repo"));
      const c = await ctx.codex(dirC, { extraArgs: COMPACT_ARGS, prompt: COMPACT_PROMPT });
      const srcC = sessionSources(c.events);
      observed.push(...srcC);
      const tlC = compactTimeline(c.events);
      evidence.push(`C_compact sources=[${srcC.join(",")}] events=${c.events.map((e) => e.event).join(">")}`);
      evidence.push(`C_timeline=${JSON.stringify(tlC)}`);
      saveText(dirC, "stdout.txt", c.stdout);
      writeFixture(ctx.repoRoot, "test/contracts/codex/session-start-compact.json", {
        agent: "codex",
        agent_version: binVersion("codex"),
        captured_at: new Date().toISOString(),
        sources: srcC,
        timeline: tlC,
        session_starts: c.events.filter((e) => e.event === "SessionStart").map((e) => redactValue(e.stdin, c.repo)),
        pre_post: c.events
          .filter((e) => e.event === "PreCompact" || e.event === "PostCompact")
          .map((e) => ({ event: e.event, at: e.at, stdin: redactValue(e.stdin, c.repo) })),
      });

      let tuiBlocked = null;
      const dirD = path.join(ctx.dir, "d");
      const seed = await ctx.codex(dirD, { prompt: DONE_PROMPT });
      widenSessionStart(seed.tree);
      truncateEvents(seed.tree);
      const beforeIds = new Set(sessionSources(seed.events));
      try {
        await withTui(dirD, {
          home: seed.tree,
          repo: seed.repo,
          extra: ["--dangerously-bypass-hook-trust"],
          async run(tui, name) {
            await tuiComposerTurn(tui, name, seed.tree, dirD);
            await tuiSubmit(name, tui, "/compact", { timeoutMs: 45_000 });
            await waitEvents(
              seed.tree,
              (ev) => ev.some((e) => e.event === "PostCompact" || (e.event === "SessionStart" && e.stdin?.source === "compact")),
              45_000,
            );
            saveText(dirD, "pane-compact.txt", tui.capture());
            await waitPane(tui, /› Ask Codex/, 20_000);
            const beforeNew = parseEvents(eventsFile(seed.tree)).filter((e) => e.event === "SessionStart").length;
            await tuiSubmit(name, tui, "/new", { timeoutMs: 45_000 });
            await waitEvents(seed.tree, (ev) => ev.filter((e) => e.event === "SessionStart").length > beforeNew, 25_000);
            saveText(dirD, "pane-new.txt", tui.capture());
            const after = parseEvents(eventsFile(seed.tree));
            const dSources = sessionSources(after);
            if (!dSources.includes("clear") && !dSources.includes("new")) {
              tuiBlocked =
                "TUI /new produced no extra SessionStart (sources=[" +
                dSources.join(",") +
                "]); pane=" +
                tui.capture().slice(-400);
            }
            await tuiQuit(tui, name, { timeoutMs: 45_000 });
          },
        });
      } catch (e) {
        if (!(e instanceof PreconditionError)) throw e;
        tuiBlocked = e.message;
      }
      const dEvents = parseEvents(eventsFile(seed.tree));
      const srcD = sessionSources(dEvents);
      observed.push(...srcD);
      const clearEv = dEvents.filter((e) => e.event === "SessionStart" && e.stdin?.source === "clear");
      const newIds = [...new Set(clearEv.map((e) => e.stdin?.session_id).filter(Boolean))];
      evidence.push(`D_tui sources=[${srcD.join(",")}] clear_session_ids=[${newIds.join(",")}] seed_session=${seed.sessionId || "none"}`);
      evidence.push(`D_new_session_id=${newIds.length ? newIds.some((id) => id !== seed.sessionId) : "no-clear-event"}`);
      if (tuiBlocked) evidence.push(`D_tui_blocked=${tuiBlocked}`);

      const uniq = [...new Set(observed)];
      evidence.push(`observed_sources=[${uniq.join(",")}]`);
      const hasCompact = uniq.includes("compact");
      const hasClear = uniq.includes("clear");
      const cannotRun = Boolean(tuiBlocked && /composer never appeared|tmux new-session/i.test(tuiBlocked));
      if (tuiBlocked) evidence.push(TUI_MANUAL);
      if (!hasClear && cannotRun) {
        return { status: "blocked", evidence, data: { observed: uniq, tuiBlocked } };
      }
      return {
        status: hasCompact && hasClear ? "pass" : "fail",
        evidence,
        data: { observed: uniq, hasCompact, hasClear, beforeIds: [...beforeIds] },
      };
    },
  },
  {
    id: "codex-postcompact-payload",
    agent: "codex",
    row: ROW_POSTCOMPACT,
    async run(ctx) {
      const evidence = [];
      const dirC = path.join(ctx.dir, "c");
      fs.mkdirSync(path.join(dirC, "repo"), { recursive: true });
      writeBigFile(path.join(dirC, "repo"));
      const c = await ctx.codex(dirC, { extraArgs: COMPACT_ARGS, prompt: COMPACT_PROMPT });
      const preC = c.events.filter((e) => e.event === "PreCompact");
      const postC = c.events.filter((e) => e.event === "PostCompact");
      const sumC = postC.map((e) => summaryOf(e.stdin));
      evidence.push(`C PreCompact n=${preC.length} keys=[${preC.map((e) => topKeys(e.stdin).join("|")).join(" ; ")}]`);
      evidence.push(
        `C PostCompact n=${postC.length} keys=[${postC.map((e) => topKeys(e.stdin).join("|")).join(" ; ")}] summary=${JSON.stringify(sumC)} identity=${JSON.stringify(compactionIdentity(postC))}`,
      );
      evidence.push(`C timeline=${JSON.stringify(compactTimeline(c.events))}`);
      const ordC = orderOk(c.events);
      evidence.push(`C order_b=${ordC.ok} ${ordC.detail}`);

      let tuiBlocked = null;
      const dirD = path.join(ctx.dir, "d");
      const seed = await ctx.codex(dirD, { prompt: DONE_PROMPT });
      truncateEvents(seed.tree);
      let dEvents = [];
      try {
        await withTui(dirD, {
          home: seed.tree,
          repo: seed.repo,
          extra: ["--dangerously-bypass-hook-trust"],
          async run(tui, name) {
            await tuiComposerTurn(tui, name, seed.tree, dirD);
            await tuiSubmit(name, tui, "/compact", { timeoutMs: 45_000 });
            await waitEvents(seed.tree, (ev) => ev.filter((e) => e.event === "PostCompact").length >= 1, 45_000);
            saveText(dirD, "pane-compact1.txt", tui.capture());
            await tuiSubmit(name, tui, "/compact", { timeoutMs: 45_000 });
            await waitEvents(seed.tree, (ev) => ev.filter((e) => e.event === "PostCompact").length >= 2, 45_000);
            saveText(dirD, "pane-compact2.txt", tui.capture());
            if (parseEvents(eventsFile(seed.tree)).filter((e) => e.event === "PostCompact").length < 2) {
              tuiBlocked = "TUI two-/compact produced <2 PostCompact; pane=" + tui.capture().slice(-400);
            }
            await tuiQuit(tui, name, { timeoutMs: 45_000 });
          },
        });
      } catch (e) {
        if (!(e instanceof PreconditionError)) throw e;
        tuiBlocked = e.message;
      }
      dEvents = parseEvents(eventsFile(seed.tree));
      const postD = dEvents.filter((e) => e.event === "PostCompact");
      const preD = dEvents.filter((e) => e.event === "PreCompact");
      evidence.push(`D PreCompact n=${preD.length} keys=[${preD.map((e) => topKeys(e.stdin).join("|")).join(" ; ")}]`);
      evidence.push(
        `D PostCompact n=${postD.length} keys=[${postD.map((e) => topKeys(e.stdin).join("|")).join(" ; ")}] summary=${JSON.stringify(postD.map((e) => summaryOf(e.stdin)))} identity=${JSON.stringify(compactionIdentity(postD))}`,
      );
      evidence.push(`D timeline=${JSON.stringify(compactTimeline(dEvents))}`);
      if (tuiBlocked) evidence.push(`D_tui_blocked=${tuiBlocked}`);

      const posts = [...postC, ...postD];
      const summary = posts.length ? summaryOf(posts[posts.length - 1].stdin) : { field: null, length: 0 };
      evidence.push(`summary_field=${summary.field} summary_length=${summary.length}`);

      let aOk = false;
      let aDetail = "need two PostCompact payloads";
      if (postD.length >= 2) {
        const ident = compactionIdentity(postD);
        aOk = ident.ok;
        aDetail = `candidates=[${ident.candidates.join(",")}] values=${JSON.stringify(ident.values)} note=${ident.note || ""}`;
      } else if (tuiBlocked) {
        aDetail = "TUI two-/compact not executed: " + tuiBlocked;
      } else {
        aDetail = `PostCompact count from TUI=${postD.length}`;
      }
      const ord = orderOk(postC.length ? c.events : dEvents);
      const bOk = ord.ok;
      evidence.push(`pass_a=${aOk} ${aDetail}`);
      evidence.push(`pass_b=${bOk} ${ord.detail}`);

      writeFixture(ctx.repoRoot, "test/contracts/codex/postcompact.json", {
        agent: "codex",
        agent_version: binVersion("codex"),
        captured_at: new Date().toISOString(),
        summary_field: summary.field,
        summary_length: summary.length,
        pass_a: aOk,
        pass_b: bOk,
        headless: {
          pre: preC.map((e) => redactValue(e.stdin, c.repo)),
          post: postC.map((e) => redactValue(e.stdin, c.repo)),
          timeline: compactTimeline(c.events),
        },
        tui: {
          pre: preD.map((e) => redactValue(e.stdin, seed.repo)),
          post: postD.map((e) => redactValue(e.stdin, seed.repo)),
          timeline: compactTimeline(dEvents),
        },
      });

      if (!posts.length) return { status: "fail", evidence, data: { aOk, bOk } };
      if (postD.length < 2 && tuiBlocked) return { status: "blocked", evidence, data: { aOk, bOk, tuiBlocked } };
      return { status: aOk && bOk ? "pass" : "fail", evidence, data: { aOk, bOk, summary } };
    },
  },
  {
    id: "codex-rollout-flush",
    agent: "codex",
    row: ROW_FLUSH,
    async run(ctx) {
      const r = await ctx.codex(ctx.dir, {
        prompt: toolUsePrompt("codex"),
        hookFlags: { PostToolUse: ["--grep-transcript"] },
      });
      const posts = r.events.filter((e) => e.event === "PostToolUse");
      const rows = posts.map((e) => ({
        tool: toolNameOf(e),
        tool_use_id: toolUseIdOf(e),
        transcript_has_tool_use_id: e.transcript_has_tool_use_id === true,
        transcript_bytes: e.transcript_bytes ?? null,
      }));
      const miss = rows.filter((x) => !x.transcript_has_tool_use_id);
      const evidence = rows.map(
        (x) => `id=${x.tool_use_id || "none"} tool=${x.tool} in_transcript=${x.transcript_has_tool_use_id} bytes=${x.transcript_bytes}`,
      );
      evidence.push(`calls=${rows.length} misses=${miss.length} exit=${r.exitCode}`);
      writeFixture(ctx.repoRoot, "test/contracts/codex/rollout-flush.json", {
        agent: "codex",
        agent_version: binVersion("codex"),
        captured_at: new Date().toISOString(),
        rows,
      });
      return {
        status: rows.length && miss.length === 0 ? "pass" : "fail",
        evidence,
        data: { calls: rows.length, misses: miss.length },
      };
    },
  },
  {
    id: "codex-tui-trust",
    agent: "codex",
    row: ROW_TUI,
    async run(ctx) {
      const seed = await ctx.codex(ctx.dir, { prompt: DONE_PROMPT, trust: true });
      const cfg = path.join(seed.tree, "config.toml");
      const toml = fs.existsSync(cfg) ? fs.readFileSync(cfg, "utf8") : "";
      const rows = (toml.match(/trusted_hash/g) || []).length;
      truncateEvents(seed.tree);
      let tuiBlocked = null;
      let pane = "";
      try {
        await withTui(ctx.dir, {
          home: seed.tree,
          repo: seed.repo,
          extra: [],
          async run(tui, name) {
            pane = await waitPane(tui, /›|review required|Trust to trust|New hook/, 25_000);
            saveText(ctx.dir, "pane-start.txt", pane);
            if (TRUST_PANE_RE.test(pane)) {
              await tuiQuit(tui, name, { timeoutMs: 25_000 });
              return;
            }
            await readyTui("codex", tui, { timeoutMs: 25_000 });
            pane = await tuiSubmit(name, tui, TUI_PROMPT, { timeoutMs: 25_000 });
            await waitEvents(seed.tree, (ev) => ev.some((e) => e.event === "Stop" || e.event === "PreToolUse" || e.event === "PostToolUse"), 45_000);
            pane = tui.capture();
            saveText(ctx.dir, "pane-turn.txt", pane);
            await tuiQuit(tui, name, { timeoutMs: 25_000 });
          },
        });
      } catch (e) {
        if (!(e instanceof PreconditionError)) throw e;
        tuiBlocked = e.message;
      }
      const events = parseEvents(eventsFile(seed.tree));
      const labels = events.map((e) => e.event);
      const uniq = [...new Set(labels)];
      const trustPrompt = TRUST_PANE_RE.test(pane);
      const hooksFired = labels.some((n) => n === "SessionStart" || n === "PreToolUse" || n === "UserPromptSubmit");
      const evidence = [
        `trust_rows=${rows}`,
        `events=${uniq.join(",") || "none"}`,
        `hooks_fired=${hooksFired}`,
        `trust_prompt=${trustPrompt}`,
        `pane_tail=${pane.slice(-400).replace(/\s+/g, " ")}`,
      ];
      if (tuiBlocked) evidence.push(`tui_blocked=${tuiBlocked}`);
      if (tuiBlocked) {
        return {
          status: "blocked",
          evidence: [
            ...evidence,
            "manual: CODEX_HOME=<tmp with hooks.json + trusted_hash rows, no --dangerously-bypass-hook-trust> tmux `codex --sandbox danger-full-access --ask-for-approval never` in the throwaway repo; send `echo tui-ok`; confirm hook labels in events.jsonl with no trust prompt; /quit",
          ],
        };
      }
      return {
        status: hooksFired && !trustPrompt ? "pass" : "fail",
        evidence,
        data: { hooksFired, trustPrompt, events: uniq, rows },
      };
    },
  },
  {
    id: "codex-mcp-legacy-client",
    agent: "codex",
    row: ROW_MCP,
    async run(ctx) {
      const log = path.join(ctx.dir, "mcp-frames.jsonl");
      const seed = await ctx.codex(ctx.dir, { prompt: DONE_PROMPT });
      const mcpToml = [
        "",
        "[mcp_servers.oboete_probe]",
        'command = "node"',
        `args = [${JSON.stringify(MCP_DUMMY)}]`,
        "startup_timeout_sec = 8",
        "",
        "[mcp_servers.oboete_probe.env]",
        `PROBE_MCP_LOG = ${JSON.stringify(log)}`,
        "",
      ].join("\n");
      const cfg = path.join(seed.tree, "config.toml");
      const prev = fs.existsSync(cfg) ? fs.readFileSync(cfg, "utf8") : "";
      if (!prev.includes("[mcp_servers.oboete_probe]")) fs.writeFileSync(cfg, prev + mcpToml);
      truncateEvents(seed.tree);
      const proc = await runTimed(
        [
          "codex",
          "exec",
          "--dangerously-bypass-hook-trust",
          "--dangerously-bypass-approvals-and-sandbox",
          "--skip-git-repo-check",
          "--json",
          "-C",
          seed.repo,
          "-c",
          `mcp_servers.oboete_probe={command="node", args=[${JSON.stringify(MCP_DUMMY)}], env={PROBE_MCP_LOG=${JSON.stringify(log)}}, startup_timeout_sec=8}`,
          MCP_PROMPT,
        ],
        {
          cwd: seed.repo,
          env: childEnv({ CODEX_HOME: seed.tree }),
          stdoutPath: path.join(ctx.dir, "stdout-mcp.txt"),
          stderrPath: path.join(ctx.dir, "stderr-mcp.txt"),
          timeoutMs: 90_000,
        },
      );
      const events = parseEvents(eventsFile(seed.tree));
      const parsed = readMcpFrames(log);
      const frames = parsed.frames;
      const pre = events.filter((e) => e.event === "PreToolUse");
      const toolNames = pre.map((e) => toolNameOf(e));
      const echoed = /dummy result for hello/i.test(proc.stdout || "");
      const evidence = [
        `protocolVersion=${parsed.protocolVersion || "none"}`,
        `methods_in=[${parsed.methods.join(",")}]`,
        `tools/list=${parsed.hasList}`,
        `tools/call=${parsed.hasCall}`,
        `PreToolUse_tool_name=[${toolNames.join(",")}]`,
        `echoed_dummy=${echoed}`,
        `frames=${frames.length} exit=${proc.exitCode} elapsed_s=${(proc.elapsedMs / 1000).toFixed(1)}`,
      ];
      const fixtureErr = tryFixture(ctx.repoRoot, "test/contracts/codex/mcp-frames.json", {
        agent: "codex",
        agent_version: binVersion("codex"),
        captured_at: new Date().toISOString(),
        protocolVersion: parsed.protocolVersion,
        methods: parsed.methods,
        tool_names: toolNames,
        echoed_dummy: echoed,
        frames: frames.map((f) => ({
          dir: f.dir,
          at: f.at,
          method: (f.frame || f).method || (f.frame || f).result?.serverInfo?.name || null,
          protocolVersion: (f.frame || f).params?.protocolVersion || (f.frame || f).result?.protocolVersion || null,
        })),
      });
      if (fixtureErr) evidence.push("fixture_skip=" + fixtureErr);
      return {
        status: parsed.hasList && parsed.hasCall && echoed ? "pass" : "fail",
        evidence,
        data: parsed,
      };
    },
  },
];
