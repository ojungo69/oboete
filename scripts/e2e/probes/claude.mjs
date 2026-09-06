import fs from "node:fs";
import path from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import {
  CLAUDE_COMPACT_PROMPT,
  CLAUDE_EVENTS,
  binVersion,
  compactionIdentity,
  finalText,
  gitInit,
  named,
  oversizedOutcome,
  oversizedPrompt,
  pairFor,
  parseEvents,
  redactValue,
  shapeProbe,
  topKeys,
  toolUsePrompt,
  waitUntil,
  writeClaudeSettings,
  writeCompactFixture,
  writeFixture,
} from "../probe-lib/agents.mjs";
import { readyTui, tmuxSession, tuiQuit, tuiSubmit } from "../probe-lib/tmux.mjs";

const ROW_SHAPES = "Native tool payload shapes for read/write/edit/bash on all four agents";
const ROW_OVER = "Hook runner behaviour when the hook exits with unread stdin above 1 MB";
const ROW_COMPACT = "Compaction identity and order per agent";
const ROW_STOP = "Codex and Grok `PostCompact` payload (summary text field); Grok `Stop` `lastAssistantMessage` field";

const ECHO_PROMPT =
  "Reply with exactly the word DONE followed by every marker token you have seen. Do not use tools.";
const TUI_MANUAL = [
  "manual TUI: as oboete-dogfood, in a throwaway git repo run: claude --settings <settings.json> --dangerously-skip-permissions",
  "first TUI launch on this user hits theme picker then login-method; complete that interactively (headless -p already has credentials; do not start a second OAuth from the probe)",
  "type a short prompt, wait for the reply, type /compact, wait for PostCompact in events.jsonl, type /compact again",
  "compare the two PostCompact stdin payloads for a distinguisher besides compact_summary; record order vs SessionStart source=compact",
];

const EXPECTED = {
  Read: {
    file: "read.json",
    normalized: "read",
    input: ["file_path"],
    output: ["type", "file"],
    path: "tool_input.file_path (absolute); echoed back at tool_response.file.filePath (camelCase)",
  },
  Write: {
    file: "write.json",
    normalized: "write",
    input: ["file_path", "content"],
    output: ["type", "filePath", "content", "structuredPatch", "originalFile", "userModified"],
    path: "tool_input.file_path ; tool_response.filePath",
  },
  Edit: {
    file: "edit.json",
    normalized: "edit",
    input: ["file_path", "old_string", "new_string", "replace_all"],
    output: ["filePath", "oldString", "newString", "originalFile", "structuredPatch", "userModified", "replaceAll"],
    path: "tool_input.file_path ; tool_response.filePath",
  },
  Bash: {
    file: "bash.json",
    normalized: "bash",
    input: ["command", "description"],
    output: ["stdout", "stderr", "interrupted", "isImage", "noOutputExpected"],
    path: "tool_input.command ; tool_response has NO command echo — Pre/Post must be joined by tool_use_id",
  },
};

function ssSource(events) {
  const ev = events.find((e) => e.event === "SessionStart");
  return ev?.stdin?.source ?? null;
}

function markerFlags(token) {
  return { hookFlags: { SessionStart: ["--plain", `'Note: ${token}'`] } };
}

function compactRelated(events) {
  return events
    .filter((e) =>
      ["PreCompact", "PostCompact", "SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "PostToolUseFailure"].includes(
        e.event,
      ),
    )
    .map((e) => {
      const src = e.event === "SessionStart" ? e.stdin?.source : e.stdin?.trigger;
      const tool = e.stdin?.tool_name || e.stdin?.toolName;
      const err = e.event === "PostToolUseFailure" && e.stdin?.error ? String(e.stdin.error).split("\n")[0] : "";
      return `${e.event}${src ? ":" + src : ""}${tool ? ":" + tool : ""}${err ? ":" + err : ""}@${e.at || "?"}`;
    });
}

function evalB(events) {
  const indexed = (events || []).map((e, i) => ({ i, e }));
  const posts = indexed.filter((x) => x.e.event === "PostCompact");
  const pres = indexed.filter((x) => x.e.event === "PreCompact");
  if (!posts.length) return { ok: null, details: ["no PostCompact"] };
  const isInj = (x) =>
    (x.e.event === "SessionStart" && x.e.stdin?.source === "compact") || x.e.event === "UserPromptSubmit";
  const details = [];
  let ok = true;
  for (const p of posts) {
    const prevPre = [...pres].reverse().find((q) => q.i < p.i);
    let lo;
    let noPre = false;
    if (prevPre) {
      lo = prevPre.i;
    } else {
      noPre = true;
      const prevPost = [...posts].reverse().find((q) => q.i < p.i);
      lo = prevPost ? prevPost.i : -1;
    }
    const note = noPre ? " (no PreCompact recorded)" : "";
    const before = indexed.find((x) => x.i > lo && x.i < p.i && isInj(x));
    const after = indexed.find((x) => x.i > p.i && isInj(x));
    if (before) {
      ok = false;
      details.push(
        `injection ${before.e.event}:${before.e.stdin?.source || ""} idx ${before.i} BEFORE PostCompact idx ${p.i} (at ${before.e.at} vs ${p.e.at})${note}`,
      );
    } else if (after) {
      details.push(
        `PostCompact idx ${p.i} @${p.e.at} before ${after.e.event}:${after.e.stdin?.source || ""} idx ${after.i} @${after.e.at}${note}`,
      );
    } else {
      details.push(`PostCompact idx ${p.i} @${p.e.at} with no later injection hook${note}`);
    }
  }
  return { ok, details };
}

function usageOf(r) {
  try {
    const j = JSON.parse((r.stdout || "").trim());
    return j.usage || null;
  } catch {
    return null;
  }
}

async function waitPostCompact(eventsPath, n, ms) {
  return (
    (await waitUntil(() => {
      const ev = parseEvents(eventsPath);
      return named(ev, "PostCompact").length >= n ? ev : null;
    }, ms, 400)) || parseEvents(eventsPath)
  );
}

async function tuiTwoCompacts(dir, repo) {
  const { settingsPath, eventsPath } = writeClaudeSettings(dir);
  const launch = path.join(dir, "tui.sh");
  fs.writeFileSync(
    launch,
    `#!/bin/bash\nexport PATH="$HOME/.local/bin:$HOME/.npm-global/bin:$PATH"\nexec claude --settings ${JSON.stringify(settingsPath)} --dangerously-skip-permissions\n`,
    { mode: 0o755 },
  );
  const name = `pbc${process.pid}${Date.now().toString(36)}`.replace(/[^a-zA-Z0-9]/g, "").slice(0, 24);
  // tmuxSession turns each variable into a `-e NAME=VALUE` argument of the tmux client, and a
  // command line is world-readable in /proc, so a pane is handed the variables it needs rather
  // than a copy of the developer's environment. The pane inherits the rest from the tmux server,
  // which probe-lib/tmux.mjs already strips of credentials.
  const tmux = tmuxSession({
    name,
    command: launch,
    cwd: repo,
    // The launch script above exports the PATH the agent needs.
    env: { TERM: "xterm-256color" },
  });
  const paneLog = path.join(dir, "tmux-pane.txt");
  const dump = (label) => {
    try {
      fs.appendFileSync(paneLog, `\n----- ${label} -----\n` + tmux.capture() + "\n");
    } catch {
      /* ignore */
    }
  };
  try {
    await sleep(2000);
    await readyTui("claude", tmux);
    dump("after-onboard");
    await tuiSubmit(name, tmux, "Reply with exactly the word DONE. Do not use tools.", { timeoutMs: 120_000 });
    await tmux.waitFor(/\bDONE\b/, 120_000);
    dump("after-done");
    await sleep(2000);
    await tuiSubmit(name, tmux, "/compact", { timeoutMs: 120_000 });
    await sleep(1500);
    if (/compact this conversation|Are you sure|Yes/i.test(tmux.capture())) tmux.send("");
    let events = await waitPostCompact(eventsPath, 1, 120_000);
    dump("after-compact-1");
    if (named(events, "PostCompact").length < 1) {
      tmux.send("");
      events = await waitPostCompact(eventsPath, 1, 60_000);
    }
    await tuiSubmit(name, tmux, "/compact", { timeoutMs: 120_000 });
    await sleep(1500);
    if (/compact this conversation|Are you sure|Yes/i.test(tmux.capture())) tmux.send("");
    events = await waitPostCompact(eventsPath, 2, 120_000);
    dump("after-compact-2");
    const pane = tmux.capture();
    await tuiQuit(tmux, name, { timeoutMs: 120_000 });
    return { events: parseEvents(eventsPath), pane, eventsPath };
  } catch (e) {
    let pane = "";
    try {
      pane = tmux.capture();
    } catch {
      pane = "";
    }
    dump("error");
    fs.appendFileSync(paneLog, "\n" + String(e && e.message ? e.message : e) + "\n");
    return {
      events: parseEvents(eventsPath),
      pane,
      error: String(e && e.message ? e.message : e),
      eventsPath,
    };
  } finally {
    try {
      tmux.kill();
    } catch {
      /* ignore */
    }
  }
}

export const probes = [
  {
    id: "claude-payload-shapes",
    agent: "claude",
    row: ROW_SHAPES,
    run: shapeProbe({
      agent: "claude",
      expected: EXPECTED,
      launch: (ctx) => ctx.claude(ctx.dir, { prompt: toolUsePrompt("claude") }),
      fixtureDir: "claude",
    }),
  },
  {
    id: "claude-oversized-stdin",
    agent: "claude",
    row: ROW_OVER,
    async run(ctx) {
      const hooks = [
        ...CLAUDE_EVENTS.filter((e) => e !== "PostToolUse"),
        { event: "PostToolUse", flags: ["--no-read"], label: "PostToolUse-noread" },
        { event: "PostToolUse", flags: [], label: "PostToolUse" },
      ];
      return oversizedOutcome(await ctx.claude(ctx.dir, { prompt: oversizedPrompt("Bash"), hooks }), ctx.dir);
    },
  },
  {
    id: "claude-session-start-sources",
    agent: "claude",
    row: ROW_COMPACT,
    async run(ctx) {
      const prompt = ECHO_PROMPT;
      const a = await ctx.claude(path.join(ctx.dir, "a"), {
        prompt,
        ...markerFlags("PROBE-SS-startup"),
      });
      const evidence = [
        `A exit=${a.exitCode} source=${ssSource(a.events)} session=${a.sessionId || "none"} elapsed_s=${(a.elapsedMs / 1000).toFixed(1)}`,
      ];
      if (!a.sessionId) {
        evidence.push(`A stdout_head=${(a.stdout || "").slice(0, 300).replace(/\s+/g, " ")}`);
        evidence.push(`A events=${a.events.map((e) => e.event).join(",") || "none"}`);
        return { status: "fail", evidence, data: { a: { source: ssSource(a.events), sessionId: a.sessionId } } };
      }
      const b = await ctx.claude(path.join(ctx.dir, "b"), {
        prompt,
        repo: a.repo,
        extraArgs: ["--resume", a.sessionId],
        ...markerFlags("PROBE-SS-resume"),
      });
      const c = await ctx.claude(path.join(ctx.dir, "c"), {
        prompt,
        repo: a.repo,
        extraArgs: ["--resume", a.sessionId, "--fork-session"],
        ...markerFlags("PROBE-SS-fork"),
      });
      const runs = [
        { name: "A", token: "PROBE-SS-startup", expect: "startup", r: a },
        { name: "B", token: "PROBE-SS-resume", expect: "resume", r: b },
        { name: "C", token: "PROBE-SS-fork", expect: "fork", r: c },
      ];
      const data = {};
      for (const run of runs) {
        const source = ssSource(run.r.events);
        const text = finalText("claude", run.r, run.r.events) || "";
        const delivered = text.includes(run.token);
        data[run.name] = {
          source,
          sessionId: run.r.sessionId || null,
          delivered,
          text: text.slice(0, 400),
        };
        evidence.push(
          `${run.name} source=${source} expected=${run.expect} session=${run.r.sessionId || "none"} marker_delivered=${delivered} answer=${text.slice(0, 200).replace(/\s+/g, " ")}`,
        );
      }
      const sourcesOk = data.A.source === "startup" && data.B.source === "resume" && data.C.source === "fork";
      const idOk = data.A.sessionId && data.A.sessionId === data.B.sessionId && data.C.sessionId && data.C.sessionId !== data.A.sessionId;
      evidence.push(`id_continuity A==B=${data.A.sessionId === data.B.sessionId} C!=A=${data.C.sessionId !== data.A.sessionId}`);
      evidence.push(`sources_ok=${sourcesOk} id_ok=${idOk}`);
      return { status: sourcesOk && idOk ? "pass" : "fail", evidence, data };
    },
  },
  {
    id: "claude-tool-failure",
    agent: "claude",
    row: ROW_SHAPES,
    async run(ctx) {
      const prompt = [
        "Do these three steps in order, without asking for confirmation. Each step names the exact tool to use; use that tool and no other. Do not substitute shell commands for the file tools. Do both tool calls even if they fail.",
        "1. Use the Bash tool to run exactly this command: echo fail-stderr >&2; exit 3",
        "2. Use the Read tool on missing-probe-file-does-not-exist.txt in the current directory.",
        "3. Reply with exactly the word DONE.",
      ].join(" ");
      const r = await ctx.claude(ctx.dir, { prompt });
      const version = binVersion("claude");
      const captured_at = new Date().toISOString();
      const evidence = [`exit=${r.exitCode} elapsed_s=${(r.elapsedMs / 1000).toFixed(1)} session=${r.sessionId || "none"}`];
      const missing = [];
      for (const [native, file, normalized] of [
        ["Bash", "bash-failure.json", "bash"],
        ["Read", "read-failure.json", "read"],
      ]) {
        const failPair = pairFor(r.events, native, "PreToolUse", "PostToolUseFailure");
        const postPair = pairFor(r.events, native, "PreToolUse", "PostToolUse");
        const fail = failPair.post;
        const alsoPost = Boolean(postPair.post);
        if (!failPair.pre || !fail) {
          missing.push(native);
          evidence.push(
            `${native}: missing ${!failPair.pre ? "PreToolUse" : ""}${!fail ? "PostToolUseFailure" : ""}; PostToolUse=${alsoPost}; events=${r.events
              .filter((e) => ["PreToolUse", "PostToolUse", "PostToolUseFailure"].includes(e.event))
              .map((e) => `${e.event}:${e.stdin?.tool_name || e.stdin?.toolName || "?"}`)
              .join(",")}`,
          );
          continue;
        }
        const failKeys = topKeys(fail.stdin);
        const err = fail.stdin?.error;
        const errKeys = err && typeof err === "object" ? topKeys(err) : typeof err;
        evidence.push(
          `${native} PostToolUseFailure keys=[${failKeys.join(",")}] error_field=${errKeys} PostToolUse_also=${alsoPost}`,
        );
        writeFixture(ctx.repoRoot, `test/contracts/claude/${file}`, {
          agent: "claude",
          agent_version: version,
          captured_at,
          native_tool: native,
          normalized_tool: normalized,
          events: {
            PreToolUse: redactValue(failPair.pre.stdin, r.repo),
            PostToolUseFailure: redactValue(fail.stdin, r.repo),
          },
          notes: `PostToolUse also fires for this failed call: ${alsoPost}; error is ${typeof err}${typeof err === "string" ? " (string)" : ""}`,
        });
      }
      return { status: missing.length ? "fail" : "pass", evidence, data: { missing, sessionId: r.sessionId } };
    },
  },
  {
    id: "claude-stop-message",
    agent: "claude",
    row: ROW_STOP,
    async run(ctx) {
      const r = await ctx.claude(ctx.dir, {
        prompt: "Reply with exactly the word DONE. Do not use tools.",
      });
      const text = finalText("claude", r, r.events) || "";
      const stop = r.events.find((e) => e.event === "Stop");
      const stdin = stop?.stdin && typeof stop.stdin === "object" ? stop.stdin : {};
      const last = stdin.last_assistant_message;
      const keys = topKeys(stdin);
      const equal = String(last ?? "") === String(text);
      const equalTrim = String(last ?? "").trim() === String(text).trim();
      return {
        status: stop && equalTrim ? "pass" : "fail",
        evidence: [
          `exit=${r.exitCode}`,
          `Stop=${Boolean(stop)}`,
          `equal=${equal} equal_trim=${equalTrim}`,
          `stop_hook_active=${Object.prototype.hasOwnProperty.call(stdin, "stop_hook_active") ? JSON.stringify(stdin.stop_hook_active) : "absent"}`,
          `background_tasks_key=${Object.prototype.hasOwnProperty.call(stdin, "background_tasks")}`,
          `session_crons_key=${Object.prototype.hasOwnProperty.call(stdin, "session_crons")}`,
          `Stop.keys=[${keys.join(",")}]`,
          `result=${JSON.stringify(text).slice(0, 200)}`,
          `last_assistant_message=${JSON.stringify(last).slice(0, 200)}`,
        ],
        data: { equal, equalTrim, keys, stop_hook_active: stdin.stop_hook_active ?? null },
      };
    },
  },
  {
    id: "claude-postcompact-payload",
    agent: "claude",
    row: ROW_COMPACT,
    async run(ctx) {
      const evidence = [];
      const autoDir = path.join(ctx.dir, "auto");
      const repo = gitInit(path.join(autoDir, "repo"));
      const big = path.join(repo, "big.txt");
      const bytes = writeCompactFixture(big);
      evidence.push(`big.txt_bytes=${bytes}`);
      const autoEnv = { CLAUDE_CODE_AUTO_COMPACT_WINDOW: "100000" };
      let auto = await ctx.claude(autoDir, { repo, prompt: CLAUDE_COMPACT_PROMPT, env: autoEnv });
      if (!named(auto.events, "PostCompact").length && auto.sessionId) {
        const auto2 = await ctx.claude(path.join(ctx.dir, "auto2"), {
          repo,
          prompt: "Reply with exactly the word DONE. Do not use tools.",
          extraArgs: ["--resume", auto.sessionId],
          env: autoEnv,
        });
        auto = {
          ...auto2,
          events: [...auto.events, ...auto2.events],
          elapsedMs: auto.elapsedMs + auto2.elapsedMs,
        };
        evidence.push(`auto2 resume exit=${auto2.exitCode} PostCompact=${named(auto2.events, "PostCompact").length} elapsed_s=${(auto2.elapsedMs / 1000).toFixed(1)}`);
      }
      const usage = usageOf(auto);
      evidence.push(
        `auto exit=${auto.exitCode} elapsed_s=${(auto.elapsedMs / 1000).toFixed(1)} PostCompact=${named(auto.events, "PostCompact").length} PreCompact=${named(auto.events, "PreCompact").length} usage=${usage ? JSON.stringify(usage) : "none"}`,
      );
      evidence.push(`auto seq=${compactRelated(auto.events).join(" | ") || "none"}`);
      for (const ev of auto.events.filter((e) => e.event === "PreCompact" || e.event === "PostCompact")) {
        const s = ev.stdin && typeof ev.stdin === "object" ? ev.stdin : {};
        evidence.push(
          `auto ${ev.event} keys=[${topKeys(s).join(",")}] trigger=${s.trigger ?? "absent"} compact_summary=${typeof s.compact_summary === "string" ? "len=" + s.compact_summary.length : s.compact_summary == null ? "absent" : typeof s.compact_summary}`,
        );
      }

      let tui = { events: [], error: "not-run", pane: "" };
      try {
        tui = await tuiTwoCompacts(path.join(ctx.dir, "tui"), gitInit(path.join(ctx.dir, "tui-repo")));
      } catch (e) {
        tui = { events: [], error: String(e && e.message ? e.message : e), pane: "" };
      }
      evidence.push(
        `tui PostCompact=${named(tui.events, "PostCompact").length} PreCompact=${named(tui.events, "PreCompact").length} error=${tui.error || "none"} pane_chars=${(tui.pane || "").length}`,
      );
      evidence.push(`tui seq=${compactRelated(tui.events).join(" | ") || "none"}`);
      if (tui.error) evidence.push(`tui_error=${tui.error.replace(/https:\S+/g, "<url>").slice(0, 400)}`);
      if (tui.pane) evidence.push(`tui_pane=${tui.pane.replace(/https:\S+/g, "<url>").slice(-500).replace(/\s+/g, " ")}`);
      for (const ev of tui.events.filter((e) => e.event === "PreCompact" || e.event === "PostCompact")) {
        const s = ev.stdin && typeof ev.stdin === "object" ? ev.stdin : {};
        evidence.push(
          `tui ${ev.event} keys=[${topKeys(s).join(",")}] trigger=${s.trigger ?? "absent"} compact_summary=${typeof s.compact_summary === "string" ? "len=" + s.compact_summary.length : s.compact_summary == null ? "absent" : typeof s.compact_summary}`,
        );
      }

      const autoPosts = named(auto.events, "PostCompact");
      const tuiPosts = named(tui.events, "PostCompact");
      const posts = tuiPosts.length >= 2 ? tuiPosts : autoPosts.length >= 2 ? autoPosts : [...autoPosts, ...tuiPosts];
      const a = compactionIdentity(posts);
      const bAuto = evalB(auto.events);
      const bTui = evalB(tui.events);
      const bOk = (autoPosts.length ? bAuto.ok : true) && (tuiPosts.length ? bTui.ok : true);
      evidence.push(
        `(a) n=${a.n} candidates=[${a.candidates.join(",")}] ok=${a.ok}${a.note ? " note=" + a.note : ""} values=${JSON.stringify(a.values)}`,
      );
      evidence.push(`(b) auto=${bAuto.ok} ${bAuto.details.join(" | ")}`);
      evidence.push(`(b) tui=${bTui.ok} ${bTui.details.join(" | ")}`);

      if (!autoPosts.length && !tuiPosts.length) {
        evidence.push(...TUI_MANUAL);
        return {
          status: "blocked",
          evidence,
          data: { a, bAuto, bTui, usage, autoTokens: usage, tuiError: tui.error || null },
        };
      }
      const status = a.ok && bOk ? "pass" : "fail";
      return { status, evidence, data: { a, bAuto, bTui, usage } };
    },
  },
];
