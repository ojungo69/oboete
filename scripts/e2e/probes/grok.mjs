import fs from "node:fs";
import path from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import {
  GROK_EVENTS,
  GROK_ISOLATION_ENV,
  agentPath,
  childEnv,
  compactionIdentity,
  finalText,
  named,
  oversizedOutcome,
  oversizedPrompt,
  parseEvents,
  prepareGrokHome,
  redactValue,
  runTimed,
  shapeProbe,
  shellQuote,
  summaryOf,
  toolNameOf,
  toolUseIdOf,
  toolUsePrompt,
  topKeys,
  writeFixture,
} from "../probe-lib/agents.mjs";
import { readMcpFrames } from "../probe-lib/mcp-frames.mjs";
import { tmuxSession } from "../probe-lib/tmux.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const MCP_SRC = path.join(HERE, "../probe-lib/mcp-dummy.mjs");

const ROW_SHAPES = "Native tool payload shapes for read/write/edit/bash on all four agents";
const ROW_OVER = "Hook runner behaviour when the hook exits with unread stdin above 1 MB";
const ROW_PARALLEL =
  "Grok parallel batches: whether `additionalContext` attached to several calls of one batch reaches the model once or once per call";
const ROW_FAIL =
  "Grok Build `PreToolUse` context on an executed-but-failed call (`PostToolUseFailure`)";
const ROW_PERM = "`PermissionDenied` payload";
const ROW_COMPACT =
  "Codex and Grok `PostCompact` payload (summary text field); Compaction identity and order per agent";
const ROW_RESUME = "Grok Build resume: `SessionStart` `source` value and session id continuity";
const ROW_MCP = "Grok Build user-scoped MCP registration; Legacy-era MCP server against Grok client";
const ROW_STOP = "Grok Build `Stop` `lastAssistantMessage` field";

const EXPECTED = {
  read_file: {
    file: "read_file.json",
    normalized: "read",
    input: ["target_file"],
    output: ["type", "FileContent"],
    path: "toolInput.target_file (relative); absolute at toolResult.FileContent.absolute_path",
  },
  write: {
    file: "write.json",
    normalized: "write",
    input: ["file_path", "content"],
    output: ["type", "EditsApplied"],
    path: "toolInput.file_path (relative); absolute at toolResult.EditsApplied.absolute_path; toolResult.type is SearchReplace",
  },
  search_replace: {
    file: "search_replace.json",
    normalized: "edit",
    input: ["file_path", "old_string", "new_string"],
    output: ["type", "EditsApplied"],
    path: "toolInput.file_path (relative); absolute at toolResult.EditsApplied.absolute_path",
  },
  run_terminal_command: {
    file: "run_terminal_command.json",
    normalized: "bash",
    input: ["command", "description"],
    output: [
      "type",
      "output",
      "output_for_prompt",
      "exit_code",
      "command",
      "truncated",
      "signal",
      "timed_out",
      "description",
      "current_dir",
      "output_file",
      "total_bytes",
      "was_bare_echo",
    ],
    path: "toolInput.command; output is a byte array, output_for_prompt is the string",
  },
};

const COMPACT_TOML = `
[session]
auto_compact_threshold_percent = 10

[features]
compaction_mode = "summary"

[model."grok-4.6"]
compaction_at_tokens = 1500
context_window = 6000
auto_compact_threshold_percent = 10

[model."grok-4.6-build"]
compaction_at_tokens = 1500
context_window = 6000
auto_compact_threshold_percent = 10

[ui]
permission_mode = "always-approve"
yolo = true
`;

function tomlStr(s) {
  return JSON.stringify(String(s));
}

function countOcc(text, needle) {
  return !text || !needle ? 0 : String(text).split(needle).length - 1;
}

function sessionTexts(home) {
  const root = path.join(home, "sessions");
  if (!fs.existsSync(root)) return [];
  return fs
    .readdirSync(root, { recursive: true, withFileTypes: true })
    .filter((e) => e.isFile() && /updates\.jsonl$|chat_history\.jsonl$/i.test(e.name))
    .map((e) => {
      const p = path.join(e.parentPath, e.name);
      return { path: p, text: fs.readFileSync(p, "utf8") };
    });
}

function markerHits(home, events, needle) {
  const answer = named(events, "Stop")
    .map((e) => e.stdin?.lastAssistantMessage || "")
    .join("\n");
  const files = sessionTexts(home);
  const inFiles = files.reduce((n, f) => n + countOcc(f.text, needle), 0);
  const fileHits = files.filter((f) => f.text.includes(needle)).map((f) => path.basename(path.dirname(f.path)) + "/" + path.basename(f.path));
  return { answerCount: countOcc(answer, needle), transcriptCount: inFiles, fileHits, answer };
}

function sameSecondBatch(pres) {
  const ats = pres.map((e) => e.at || "");
  const secs = new Set(ats.map((a) => String(a).slice(0, 19)));
  const times = pres.map((e) => Date.parse(e.at) || 0);
  const spread = times.length ? Math.max(...times) - Math.min(...times) : 0;
  return { n: pres.length, ats, secCount: secs.size, spreadMs: spread, parallel: pres.length >= 2 && spread <= 500 };
}

function saveFix(ctx, file, obj) {
  try {
    writeFixture(ctx.repoRoot, `test/contracts/grok/${file}`, obj);
    return file;
  } catch (e) {
    return "skip:" + (e && e.message ? e.message : e);
  }
}

function startSource(ev) {
  const s = ev?.stdin || {};
  return {
    source: s.source ?? s.Source ?? null,
    sessionId: s.sessionId || s.session_id || null,
    transcriptPath: s.transcriptPath || s.transcript_path || null,
    keys: topKeys(s),
  };
}

function writeBig(repo, bytes = 200 * 1024) {
  fs.mkdirSync(repo, { recursive: true });
  const lines = [];
  let n = 0;
  let i = 0;
  const pad = "abcdefghijklmnopqrstuvwxyz0123456789".repeat(4);
  while (n < bytes) {
    const line = `L${i} ${pad} token-${i}-${(i * 7919) % 99991}\n`;
    lines.push(line);
    n += line.length;
    i += 1;
  }
  const body = lines.join("").slice(0, bytes);
  fs.writeFileSync(path.join(repo, "big.txt"), body);
  return body.length;
}

function labeledMcpMethods(frames) {
  return frames.filter((f) => f.frame?.method).map((f) => `${f.dir}:${f.frame.method}`);
}

function injectionOrder(events) {
  const posts = named(events, "PostCompact");
  const pres = named(events, "PreCompact");
  if (!posts.length) return { ok: false, note: "no PostCompact" };
  const isInj = (e) =>
    e.event === "UserPromptSubmit" ||
    e.event === "PreToolUse" ||
    (e.event === "SessionStart" && e.stdin?.source === "compact");
  let missingPre = false;
  for (const post of posts) {
    const tPost = Date.parse(post.at) || 0;
    const matchingPre = [...pres].reverse().find((p) => (Date.parse(p.at) || 0) <= tPost);
    let tPre;
    if (matchingPre) {
      tPre = Date.parse(matchingPre.at) || 0;
    } else {
      missingPre = true;
      const prevPost = [...posts].reverse().find((p) => (Date.parse(p.at) || 0) < tPost);
      tPre = prevPost ? Date.parse(prevPost.at) || 0 : Number.NEGATIVE_INFINITY;
    }
    const viol = events.find((e) => {
      if (!isInj(e)) return false;
      const t = Date.parse(e.at) || 0;
      return t >= tPre && t < tPost;
    });
    if (viol) {
      const extra = matchingPre ? "" : " (no PreCompact recorded)";
      return { ok: false, note: `violator ${viol.event}@${viol.at} before PostCompact@${post.at}${extra}` };
    }
  }
  const last = posts[posts.length - 1];
  const extra = missingPre ? "; no PreCompact recorded" : "";
  return { ok: true, note: `all injections after matching PreCompact have at >= PostCompact.at (last@${last.at})${extra}` };
}

async function tuiTwoCompact(dir, { home, repo }) {
  const name = "obg-pc-" + Date.now().toString(36);
  const paneFile = path.join(dir, "tui-pane.txt");
  let tmux;
  try {
    tmux = tmuxSession({
      name,
      command: `grok --cwd ${shellQuote(repo)} --yolo`,
      cwd: repo,
      // The command is a bare `grok`, so the pane needs the probe PATH as well; see the note in
      // probes/claude.mjs for why this is a named list rather than the whole environment.
      env: { GROK_HOME: home, ...GROK_ISOLATION_ENV, TERM: "xterm-256color", PATH: agentPath() },
    });
    await tmux.waitFor(/Grok|Ask|❯|›|session|\//i, 90_000);
    tmux.send("say hi then wait");
    await sleep(20_000);
    tmux.send("/compact");
    await sleep(25_000);
    tmux.send("/compact");
    await sleep(70_000);
    const pane = tmux.capture();
    fs.writeFileSync(paneFile, pane);
    return { ok: true, pane };
  } catch (e) {
    let pane = "";
    try {
      pane = tmux ? tmux.capture() : "";
    } catch {
      /* ignore */
    }
    try {
      fs.writeFileSync(paneFile, pane + "\nERR " + String(e && e.message ? e.message : e));
    } catch {
      /* ignore */
    }
    return { ok: false, pane, error: String(e && e.message ? e.message : e) };
  } finally {
    try {
      if (tmux) tmux.kill();
    } catch {
      /* ignore */
    }
  }
}

const MARKER_HOOKS = (marker) =>
  GROK_EVENTS.map((e) => (e === "PreToolUse" ? { event: e, flags: ["--marker", marker] } : e));

const DENY_HOOKS = GROK_EVENTS.map((e) =>
  e === "PreToolUse" ? { event: e, flags: ["--deny"], matcher: "run_terminal_command|Bash" } : e,
);

export const probes = [
  {
    id: "grok-payload-shapes",
    agent: "grok",
    row: ROW_SHAPES,
    run: shapeProbe({
      agent: "grok",
      expected: EXPECTED,
      launch: (ctx) => ctx.grok(ctx.dir, { prompt: toolUsePrompt("grok"), grokSeed: ctx.grokSeed }),
      fixtureDir: "grok",
    }),
  },
  {
    id: "grok-oversized-stdin",
    agent: "grok",
    row: ROW_OVER,
    async run(ctx) {
      const hooks = [
        ...GROK_EVENTS.filter((e) => e !== "PostToolUse"),
        { event: "PostToolUse", flags: ["--no-read"], label: "PostToolUse-noread" },
        { event: "PostToolUse", flags: [], label: "PostToolUse" },
      ];
      return oversizedOutcome(
        await ctx.grok(ctx.dir, { prompt: oversizedPrompt("run_terminal_command"), hooks, grokSeed: ctx.grokSeed }),
        ctx.dir,
      );
    },
  },
  {
    id: "grok-parallel-batch",
    agent: "grok",
    row: ROW_PARALLEL,
    async run(ctx) {
      const prompt1 =
        "Run these two shell commands in parallel in one batch: `echo first` and `echo second`; then reply with exactly the word DONE followed by every marker token you have seen.";
      const prompt2 =
        "CRITICAL: fire TWO separate run_terminal_command tool calls in ONE assistant step as a parallel batch (not sequential, not combined with &&). Command A: echo first. Command B: echo second. After both results, reply with exactly the word DONE followed by every marker token you have seen.";
      const runOnce = (dir, prompt) =>
        ctx.grok(dir, {
          prompt,
          grokSeed: ctx.grokSeed,
          hooks: MARKER_HOOKS("PROBE-PB"),
        });
      let r = await runOnce(path.join(ctx.dir, "try1"), prompt1);
      let pres = named(r.events, "PreToolUse");
      let batch = sameSecondBatch(pres);
      if (!batch.parallel) {
        r = await runOnce(path.join(ctx.dir, "try2"), prompt2);
        pres = named(r.events, "PreToolUse");
        batch = sameSecondBatch(pres);
      }
      const hits = markerHits(r.tree, r.events, "PROBE-PB");
      const hookDeliveries = sessionTexts(r.tree).reduce(
        (n, f) => n + countOcc(f.text, "Context from PreToolUse hook"),
        0,
      );
      const text = finalText("grok", r, r.events);
      const tools = pres.map((e) => `${toolNameOf(e)}:${toolUseIdOf(e)}@${e.at}`);
      const evidence = [
        `pre_n=${batch.n} same_at_second=${batch.parallel} secCount=${batch.secCount} spread_ms=${batch.spreadMs} ats=${batch.ats.join(",") || "none"}`,
        `pre_calls=${tools.join(" | ") || "none"}`,
        `hook_context_deliveries=${hookDeliveries} marker_in_answer=${hits.answerCount} marker_in_transcript=${hits.transcriptCount} files=${hits.fileHits.join(",") || "none"}`,
        `DONE=${/\bDONE\b/.test(text)} answer=${JSON.stringify(String(hits.answer || text).slice(0, 240))}`,
        `exit=${r.exitCode} elapsed_s=${(r.elapsedMs / 1000).toFixed(1)} session=${r.sessionId || "none"}`,
      ];
      if (!batch.parallel) {
        return {
          status: "blocked",
          evidence: evidence.concat(["model did not parallelize after retry; observation only"]),
          data: { batch, hits, hookDeliveries },
        };
      }
      const oncePerCall = hookDeliveries >= 2;
      const oncePerBatch = hookDeliveries === 1;
      const status = oncePerCall ? "fail" : oncePerBatch ? "pass" : "blocked";
      if (status === "blocked") evidence.push("PreToolUse additionalContext not found in transcript");
      if (status === "fail") evidence.push("once per call (A15 default)");
      if (status === "pass") evidence.push("once per batch");
      return { status, evidence, data: { batch, hits, hookDeliveries } };
    },
  },
  {
    id: "grok-pretooluse-failed-call",
    agent: "grok",
    row: ROW_FAIL,
    async run(ctx) {
      const r = await ctx.grok(ctx.dir, {
        prompt:
          "Use the run_terminal_command tool to run exactly: bash -c 'echo boom >&2; exit 3' ; then reply with exactly the word DONE followed by every marker token you have seen.",
        grokSeed: ctx.grokSeed,
        hooks: MARKER_HOOKS("PROBE-FAIL"),
      });
      const failEvs = named(r.events, "PostToolUseFailure");
      const postEvs = named(r.events, "PostToolUse");
      const pres = named(r.events, "PreToolUse");
      const text = finalText("grok", r, r.events);
      const delivered = /\bPROBE-FAIL\b/.test(text) || markerHits(r.tree, r.events, "PROBE-FAIL").transcriptCount > 0;
      const fail0 = failEvs[0];
      const post0 = postEvs[0];
      saveFix(ctx, "posttooluse-failure.json", {
        agent: "grok",
        PostToolUseFailure: redactValue(fail0?.stdin ?? null, r.repo),
        PostToolUse: redactValue(post0?.stdin ?? null, r.repo),
      });
      return {
        status: pres.length && (failEvs.length || postEvs.length) ? "pass" : "blocked",
        evidence: [
          `PostToolUseFailure_n=${failEvs.length} keys=${fail0 ? topKeys(fail0.stdin).join(",") : "none"} error=${JSON.stringify(fail0?.stdin?.error || fail0?.stdin?.errorDetails || fail0?.stdin?.message || null)}`,
          `PostToolUse_n=${postEvs.length} keys=${post0 ? topKeys(post0.stdin).join(",") : "none"} exit_code=${post0?.stdin?.toolResult?.exit_code ?? post0?.stdin?.tool_response?.exit_code ?? "n/a"}`,
          `PROBE-FAIL_reached_model=${delivered} delivery=${delivered ? "delivered" : "dropped"}`,
          `DONE=${/\bDONE\b/.test(text)} answer=${JSON.stringify(String(text).slice(0, 200))}`,
          `exit=${r.exitCode} elapsed_s=${(r.elapsedMs / 1000).toFixed(1)}`,
        ],
        data: { failKeys: fail0 ? topKeys(fail0.stdin) : [], postKeys: post0 ? topKeys(post0.stdin) : [], delivered },
      };
    },
  },
  {
    id: "grok-permission-denied",
    agent: "grok",
    row: ROW_PERM,
    async run(ctx) {
      const prompt =
        "Use the run_terminal_command tool once to run: echo perm-probe ; then reply with exactly the word DONE followed by every marker token you have seen.";
      const a = await ctx.grok(path.join(ctx.dir, "noapprove"), {
        prompt,
        grokSeed: ctx.grokSeed,
        noApprove: true,
        hooks: DENY_HOOKS,
        extraArgs: ["--max-turns", "3"],
        timeoutMs: 90_000,
      });
      const b = await ctx.grok(path.join(ctx.dir, "approve-deny"), {
        prompt,
        grokSeed: ctx.grokSeed,
        noApprove: false,
        hooks: DENY_HOOKS,
      });
      const c = await ctx.grok(path.join(ctx.dir, "rule-deny"), {
        prompt,
        grokSeed: ctx.grokSeed,
        noApprove: false,
        extraArgs: ["--deny", "Bash(*)"],
        configToml: `
[permission]
deny = ["Bash(*)", "Bash(echo perm-probe)"]
`,
      });
      const summarize = (label, r) => {
        const dens = named(r.events, "PermissionDenied");
        const pres = named(r.events, "PreToolUse");
        const d0 = dens[0];
        const d1 = dens[1];
        const text = finalText("grok", r, r.events);
        return {
          label,
          n: dens.length,
          keys: d0 ? topKeys(d0.stdin) : [],
          toolUseId: d0?.stdin?.toolUseId || d0?.stdin?.tool_use_id || null,
          reason: d0?.stdin?.reason || d0?.stdin?.permissionDecisionReason || d0?.stdin?.message || d0?.stdin?.denialReason || null,
          secondKeys: d1 ? topKeys(d1.stdin) : [],
          pre_n: pres.length,
          answer: String(text).slice(0, 180),
          exit: r.exitCode,
          stdin: d0?.stdin ?? null,
          repo: r.repo,
        };
      };
      const A = summarize("noApprove+hook-deny", a);
      const B = summarize("always-approve+hook-deny", b);
      const C = summarize("always-approve+permission-deny-rule", c);
      const captured = A.n + B.n + C.n > 0;
      saveFix(ctx, "permission-denied.json", {
        agent: "grok",
        noApproveHookDeny: redactValue(A.stdin, A.repo),
        alwaysApproveHookDeny: redactValue(B.stdin, B.repo),
        permissionRule: redactValue(C.stdin, C.repo),
      });
      const fmt = (s) =>
        `${s.label}: PermissionDenied_n=${s.n} keys=[${s.keys.join(",")}] toolUseId=${s.toolUseId} reason=${JSON.stringify(s.reason)} second_keys=[${s.secondKeys.join(",")}] PreToolUse_n=${s.pre_n} answer=${JSON.stringify(s.answer)} exit=${s.exit}`;
      return {
        status: captured ? "pass" : "fail",
        evidence: [fmt(A), fmt(B), fmt(C), `payload_captured=${captured}`],
        data: { A, B, C },
      };
    },
  },
  {
    id: "grok-postcompact",
    agent: "grok",
    row: ROW_COMPACT,
    async run(ctx) {
      const autoDir = path.join(ctx.dir, "auto");
      const repo = path.join(autoDir, "repo");
      const nbytes = writeBig(repo);
      const r = await ctx.grok(autoDir, {
        prompt:
          "Use the read_file tool to read the entire file big.txt, then use read_file to read big.txt a second time. Then reply with exactly the word DONE.",
        grokSeed: ctx.grokSeed,
        repo,
        configToml: COMPACT_TOML,
      });
      const preC = named(r.events, "PreCompact");
      const postC = named(r.events, "PostCompact");
      const describe = (ev) => {
        const s = ev?.stdin || {};
        const sum = summaryOf(s);
        return {
          at: ev.at,
          keys: topKeys(s),
          matcher: s.matcher ?? s.trigger ?? s.compactTrigger ?? s.source ?? null,
          summary: sum.field
            ? { name: sum.field, length: sum.length, preview: typeof s[sum.field] === "string" ? s[sum.field].slice(0, 120) : null }
            : null,
        };
      };
      const posts = postC.map(describe);
      const pres = preC.map(describe);
      const bHead = injectionOrder(r.events);

      const tuiDir = path.join(ctx.dir, "tui");
      const tuiRepo = path.join(tuiDir, "repo");
      writeBig(tuiRepo);
      const tuiHome = prepareGrokHome(tuiDir, {
        grokSeed: ctx.grokSeed,
        repo: tuiRepo,
        configToml: COMPACT_TOML,
      });
      const tui = await tuiTwoCompact(tuiDir, { home: tuiHome.home, repo: tuiHome.repo });
      const tuiEvents = parseEvents(tuiHome.eventsPath);
      const tuiPostEvs = named(tuiEvents, "PostCompact");
      const tuiPosts = tuiPostEvs.map(describe);
      const allPostEvs = postC.concat(tuiPostEvs);
      const ident = compactionIdentity(allPostEvs);
      const bTui = tuiPostEvs.length ? injectionOrder(tuiEvents) : { ok: true, note: "no tui PostCompact" };
      const bOk = (postC.length ? bHead.ok : true) && (tuiPostEvs.length ? bTui.ok : true);
      const bNote = [bHead.note, tuiPostEvs.length ? bTui.note : null].filter(Boolean).join(" | ");
      saveFix(ctx, "postcompact.json", {
        agent: "grok",
        PreCompact: redactValue(preC[0]?.stdin ?? null, r.repo),
        PostCompact: redactValue(postC[0]?.stdin ?? null, r.repo),
        tuiPostCompact: redactValue(tuiPostEvs[0]?.stdin ?? null, tuiHome.repo),
      });
      const evidence = [
        `(a) identity: headless_PostCompact_n=${posts.length} tui_PostCompact_n=${tuiPosts.length} ok=${ident.ok} candidates=[${ident.candidates.join(",")}] note=${ident.note || ""} values=${JSON.stringify(ident.values)}`,
        `(b) order: ${bNote} b_ok=${bOk}`,
        `payload PreCompact_n=${pres.length} keys=${pres[0] ? pres[0].keys.join(",") : "none"} matcher=${JSON.stringify(pres[0]?.matcher ?? null)}`,
        `payload PostCompact keys=${posts[0] ? posts[0].keys.join(",") : "none"} summary=${JSON.stringify(posts[0]?.summary || null)} matcher=${JSON.stringify(posts[0]?.matcher ?? null)}`,
        `tui ok=${tui.ok} error=${tui.error || "none"} pane=${JSON.stringify(String(tui.pane || "").replace(/\s+/g, " ").slice(0, 240))}`,
        `big.txt_bytes=${nbytes} exit=${r.exitCode} elapsed_s=${(r.elapsedMs / 1000).toFixed(1)} model=${r.model || "none"}`,
      ];
      let status;
      if (!posts.length && !tuiPosts.length) {
        status = "blocked";
        evidence.push("no PostCompact fired; could not force compaction");
        if (!tui.ok) {
          evidence.push(
            "TUI manual steps: GROK_HOME=<tui grok-home> grok --cwd <repo> --yolo ; wait ready ; /compact Enter ; wait ; /compact Enter ; compare the two PostCompact stdin payloads for a native id/counter/timestamp",
          );
        }
      } else if (allPostEvs.length < 2) {
        status = "blocked";
        evidence.push("only one PostCompact; identity (a) untested. TUI two /compact did not yield a second event");
        if (!tui.ok) {
          evidence.push(
            "TUI manual steps: GROK_HOME=<tui grok-home> grok --cwd <repo> --yolo ; /compact twice in one session ; capture PostCompact stdin",
          );
        }
      } else if (!ident.ok) {
        status = "fail";
        evidence.push("(a) failed: two PostCompact not distinguishable (A16 default)");
      } else if (!bOk) {
        status = "fail";
        evidence.push("(b) failed: " + bNote);
      } else {
        status = "pass";
      }
      return { status, evidence, data: { posts, pres, tuiPosts, ident, tui } };
    },
  },
  {
    id: "grok-resume",
    agent: "grok",
    row: ROW_RESUME,
    async run(ctx) {
      const a = await ctx.grok(path.join(ctx.dir, "A"), {
        prompt: "Remember marker ALPHA-A. Reply with exactly the word DONE.",
        grokSeed: ctx.grokSeed,
        hooks: MARKER_HOOKS("RESUME-A"),
      });
      const b = await ctx.grok(path.join(ctx.dir, "B"), {
        prompt: "New marker RESUME-B. Reply with exactly the word DONE followed by every marker token you have seen.",
        grokSeed: ctx.grokSeed,
        repo: a.repo,
        homeFrom: a.tree,
        extraArgs: ["--resume", a.sessionId || "missing"],
        hooks: MARKER_HOOKS("RESUME-B"),
      });
      const c = await ctx.grok(path.join(ctx.dir, "C"), {
        prompt: "New marker RESUME-C. Reply with exactly the word DONE followed by every marker token you have seen.",
        grokSeed: ctx.grokSeed,
        repo: a.repo,
        homeFrom: b.tree,
        extraArgs: ["--resume", a.sessionId || "missing", "--fork-session"],
        hooks: MARKER_HOOKS("RESUME-C"),
      });
      const pick = (r, label) => {
        const ss = named(r.events, "SessionStart").map(startSource);
        return {
          label,
          sessionId: r.sessionId,
          envelopeId: r.envelope?.sessionId || r.envelope?.session_id || null,
          starts: ss,
          source: ss[0]?.source ?? null,
          transcriptPath: ss[0]?.transcriptPath ?? null,
          keys: ss[0]?.keys || [],
          exit: r.exitCode,
        };
      };
      const A = pick(a, "A-new");
      const B = pick(b, "B-resume");
      const C = pick(c, "C-fork");
      const idContinuous = Boolean(A.sessionId && B.sessionId && A.sessionId === B.sessionId);
      const forkNew = Boolean(C.sessionId && A.sessionId && C.sessionId !== A.sessionId);
      const sourcePresent = B.source != null && String(B.source).length > 0;
      saveFix(ctx, "session-start-resume.json", {
        agent: "grok",
        A: redactValue(named(a.events, "SessionStart")[0]?.stdin ?? null, a.repo),
        B: redactValue(named(b.events, "SessionStart")[0]?.stdin ?? null, b.repo),
        C: redactValue(named(c.events, "SessionStart")[0]?.stdin ?? null, c.repo),
      });
      const fmt = (s) =>
        `${s.label}: source=${JSON.stringify(s.source)} sessionId=${s.sessionId} envelope=${s.envelopeId} transcriptPath=${s.transcriptPath ? "present" : "absent"} keys=[${s.keys.join(",")}] exit=${s.exit}`;
      return {
        status: sourcePresent && idContinuous ? "pass" : "fail",
        evidence: [
          fmt(A),
          fmt(B),
          fmt(C),
          `id_A_eq_B=${idContinuous} C_new_id=${forkNew} B_source_present=${sourcePresent} A=${A.sessionId} B=${B.sessionId} C=${C.sessionId}`,
        ],
        data: { A, B, C },
      };
    },
  },
  {
    id: "grok-mcp-registration",
    agent: "grok",
    row: ROW_MCP,
    async run(ctx) {
      const dummy = path.join(ctx.dir, "mcp-dummy.mjs");
      fs.copyFileSync(MCP_SRC, dummy);
      const prompt =
        "Call the MCP tool oboete_probe search with query hello and reply DONE followed by the tool result";
      const logToml = path.join(ctx.dir, "mcp-toml.jsonl");
      const logCli = path.join(ctx.dir, "mcp-cli.jsonl");
      const mcpToml = `
[mcp_servers.oboete_probe]
command = ${tomlStr(process.execPath)}
args = [${tomlStr(dummy)}]
env = { PROBE_MCP_LOG = ${tomlStr(logToml)} }
enabled = true
`;
      const tomlRun = await ctx.grok(path.join(ctx.dir, "toml"), {
        prompt,
        grokSeed: ctx.grokSeed,
        configToml: mcpToml,
        env: { PROBE_MCP_LOG: logToml },
      });
      const addHome = path.join(ctx.dir, "cli-add-home");
      fs.cpSync(ctx.grokSeed, addHome, { recursive: true });
      const cfg = path.join(addHome, "config.toml");
      const before = fs.existsSync(cfg) ? fs.readFileSync(cfg, "utf8") : "";
      const addArgs = [
        "grok",
        "mcp",
        "add",
        "--scope",
        "user",
        "oboete_probe",
        "-e",
        `PROBE_MCP_LOG=${logCli}`,
        "--",
        process.execPath,
        dummy,
      ];
      const addProc = await runTimed(addArgs, {
        cwd: ctx.dir,
        env: childEnv({ GROK_HOME: addHome, ...GROK_ISOLATION_ENV }),
        stdoutPath: path.join(ctx.dir, "mcp-add.out"),
        stderrPath: path.join(ctx.dir, "mcp-add.err"),
        timeoutMs: 60_000,
      });
      const after = fs.existsSync(cfg) ? fs.readFileSync(cfg, "utf8") : "";
      const mcpAdd = {
        exit: addProc.exitCode,
        stdout: (addProc.stdout || "").slice(0, 2000),
        stderr: (addProc.stderr || "").slice(0, 2000),
        wrote: after,
        changed: after !== before,
      };
      const cliRun = await ctx.grok(path.join(ctx.dir, "cli"), {
        prompt,
        grokSeed: ctx.grokSeed,
        homeFrom: addHome,
        env: { PROBE_MCP_LOG: logCli },
      });
      const framesToml = readMcpFrames(logToml).frames;
      const framesCli = readMcpFrames(logCli).frames;
      const methToml = labeledMcpMethods(framesToml);
      const methCli = labeledMcpMethods(framesCli);
      const has = (meth, name) => meth.some((x) => x.includes(name));
      const tomlPres = named(tomlRun.events, "PreToolUse");
      const cliPres = named(cliRun.events, "PreToolUse");
      const pres = tomlPres.concat(cliPres);
      const toolNames = [...new Set(pres.map((e) => toolNameOf(e)).filter(Boolean))];
      const text = [finalText("grok", tomlRun, tomlRun.events), finalText("grok", cliRun, cliRun.events)].join("\n");
      const echoed = /dummy result for hello/i.test(text);
      const framesOk =
        (has(methToml, "initialize") && has(methToml, "tools/list") && has(methToml, "tools/call")) ||
        (has(methCli, "initialize") && has(methCli, "tools/list") && has(methCli, "tools/call"));
      const firstRepo = tomlPres.length ? tomlRun.repo : cliRun.repo;
      saveFix(ctx, "mcp-search.json", {
        agent: "grok",
        toolNames,
        PreToolUse: redactValue(pres[0]?.stdin ?? null, firstRepo),
        mcpAddWrote: redactValue(mcpAdd.wrote ?? null, ctx.dir),
      });
      const wrote = mcpAdd.wrote || "";
      const wroteSnippet = wrote.includes("oboete_probe")
        ? wrote.slice(Math.max(0, wrote.indexOf("oboete_probe") - 40), wrote.indexOf("oboete_probe") + 400)
        : wrote.slice(0, 400);
      return {
        status: framesOk && echoed ? "pass" : "fail",
        evidence: [
          `toml frames=${methToml.join(",") || "none"}`,
          `cli frames=${methCli.join(",") || "none"}`,
          `PreToolUse toolName=[${toolNames.join(",")}]`,
          `echoed_dummy=${echoed} text=${JSON.stringify(text.slice(0, 240))}`,
          `mcp add exit=${mcpAdd.exit} changed=${mcpAdd.changed} wrote=${JSON.stringify(wroteSnippet)}`,
          `mcp add stdout=${JSON.stringify((mcpAdd.stdout || "").slice(0, 200))} stderr=${JSON.stringify((mcpAdd.stderr || "").slice(0, 200))}`,
        ],
        data: { toolNames, methToml, methCli, mcpAdd },
      };
    },
  },
  {
    id: "grok-stop-messages",
    agent: "grok",
    row: ROW_STOP,
    async run(ctx) {
      const r = await ctx.grok(ctx.dir, {
        prompt: "Do not use tools. Reply with exactly the word DONE.",
        grokSeed: ctx.grokSeed,
      });
      const stops = named(r.events, "Stop");
      const ends = named(r.events, "SessionEnd");
      const endTurn = stops.find((e) => (e.stdin?.reason || "") === "end_turn") || stops[0];
      const shutdown = stops.find((e) => {
        const reason = e.stdin?.reason || "";
        return reason === "shutdown" || reason === "channel_closed";
      });
      const msg = endTurn?.stdin?.lastAssistantMessage ?? null;
      const shutMsg = shutdown?.stdin?.lastAssistantMessage;
      const envText = typeof r.envelope?.text === "string" ? r.envelope.text : "";
      const match = typeof msg === "string" && msg.trim().length > 0 && (msg.trim() === envText.trim() || envText.includes(msg.trim()) || /\bDONE\b/.test(msg));
      const endAt = ends[0]?.at || null;
      const shutAt = shutdown?.at || null;
      const endBeforeShut = !endAt || !shutAt ? null : Date.parse(endAt) <= Date.parse(shutAt);
      saveFix(ctx, "stop-end-turn.json", {
        agent: "grok",
        end_turn: redactValue(endTurn?.stdin ?? null, r.repo),
        shutdown: redactValue(shutdown?.stdin ?? null, r.repo),
        SessionEnd: redactValue(ends[0]?.stdin ?? null, r.repo),
      });
      return {
        status: match ? "pass" : "fail",
        evidence: [
          `Stop_n=${stops.length} reasons=${stops.map((e) => e.stdin?.reason).join(",")}`,
          `end_turn lastAssistantMessage=${JSON.stringify(msg)} keys=[${endTurn ? topKeys(endTurn.stdin).join(",") : ""}]`,
          `shutdown lastAssistantMessage=${shutdown ? JSON.stringify(shutMsg ?? null) : "no-shutdown-Stop"} keys=[${shutdown ? topKeys(shutdown.stdin).join(",") : ""}]`,
          `envelope.text=${JSON.stringify(envText.slice(0, 120))} match=${match}`,
          `SessionEnd_n=${ends.length} SessionEnd.at=${endAt} shutdown.at=${shutAt} SessionEnd_before_shutdown=${endBeforeShut}`,
          `exit=${r.exitCode} elapsed_s=${(r.elapsedMs / 1000).toFixed(1)}`,
        ],
        data: { reasons: stops.map((e) => e.stdin?.reason), match, endBeforeShut },
      };
    },
  },
];
