import fs from "node:fs";
import path from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

import { GROK_EVENTS, GROK_ISOLATION_ENV, prepareGrokHome, shellQuote } from "./agents.mjs";
import { compactionIdentity, named, parseEvents, redactValue, saveFix, summaryOf, topKeys } from "./agent-events.mjs";
import { agentPath } from "./process.mjs";
import { tmuxSession } from "./tmux.mjs";

const ROW_COMPACT =
  "Codex and Grok `PostCompact` payload (summary text field); Compaction identity and order per agent";

const ROW_RESUME = "Grok Build resume: `SessionStart` `source` value and session id continuity";

const ROW_STOP = "Grok Build `Stop` `lastAssistantMessage` field";

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

function previousPostTime(posts, tPost) {
  const prevPost = [...posts].reverse().find((p) => (Date.parse(p.at) || 0) < tPost);
  return prevPost ? Date.parse(prevPost.at) || 0 : Number.NEGATIVE_INFINITY;
}

function injectionFailure(viol, post, matchingPre) {
  const extra = matchingPre ? "" : " (no PreCompact recorded)";
  return { ok: false, note: `violator ${viol.event}@${viol.at} before PostCompact@${post.at}${extra}` };
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
      tPre = previousPostTime(posts, tPost);
    }
    const viol = events.find((e) => {
      if (!isInj(e)) return false;
      const t = Date.parse(e.at) || 0;
      return t >= tPre && t < tPost;
    });
    if (viol) {
      return injectionFailure(viol, post, matchingPre);
    }
  }
  const last = posts[posts.length - 1];
  const extra = missingPre ? "; no PreCompact recorded" : "";
  return { ok: true, note: `all injections after matching PreCompact have at >= PostCompact.at (last@${last.at})${extra}` };
}

function captureGrokPane(tmux) {
  let pane = "";
  try {
    pane = tmux ? tmux.capture() : "";
  } catch {
    /* ignore */
  }
  return pane;
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
    const pane = captureGrokPane(tmux);
    try {
      fs.writeFileSync(paneFile, pane + "\nERR " + String(e?.message ? e.message : e));
    } catch {
      /* ignore */
    }
    return { ok: false, pane, error: String(e?.message ? e.message : e) };
  } finally {
    try {
      if (tmux) tmux.kill();
    } catch {
      /* ignore */
    }
  }
}

function describeGrokCompact(ev) {
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
}

function grokPostcompactStatus(options) {
  const { posts, tuiPosts, allPostEvs, ident, bOk, bNote, tui, evidence } = options;
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
  return status;
}

function grokHeadlessCompact(r) {
  const preC = named(r.events, "PreCompact");
  const postC = named(r.events, "PostCompact");
  const posts = postC.map(describeGrokCompact);
  const pres = preC.map(describeGrokCompact);
  const bHead = injectionOrder(r.events);
  return { preC, postC, posts, pres, bHead };
}

function grokTuiCompactAnalysis(head, tuiEvents) {
  const tuiPostEvs = named(tuiEvents, "PostCompact");
  const tuiPosts = tuiPostEvs.map(describeGrokCompact);
  const allPostEvs = head.postC.concat(tuiPostEvs);
  const ident = compactionIdentity(allPostEvs);
  const bTui = tuiPostEvs.length ? injectionOrder(tuiEvents) : { ok: true, note: "no tui PostCompact" };
  const bOk = (head.postC.length ? head.bHead.ok : true) && (tuiPostEvs.length ? bTui.ok : true);
  const bNote = [head.bHead.note, tuiPostEvs.length ? bTui.note : null].filter(Boolean).join(" | ");
  return { tuiPostEvs, tuiPosts, allPostEvs, ident, bTui, bOk, bNote };
}

function grokCompactEvidence(head, analysis, tui, nbytes, r) {
  return [
    `(a) identity: headless_PostCompact_n=${head.posts.length} tui_PostCompact_n=${analysis.tuiPosts.length} ok=${analysis.ident.ok} candidates=[${analysis.ident.candidates.join(",")}] note=${analysis.ident.note || ""} values=${JSON.stringify(analysis.ident.values)}`,
    `(b) order: ${analysis.bNote} b_ok=${analysis.bOk}`,
    `payload PreCompact_n=${head.pres.length} keys=${head.pres[0] ? head.pres[0].keys.join(",") : "none"} matcher=${JSON.stringify(head.pres[0]?.matcher ?? null)}`,
    `payload PostCompact keys=${head.posts[0] ? head.posts[0].keys.join(",") : "none"} summary=${JSON.stringify(head.posts[0]?.summary || null)} matcher=${JSON.stringify(head.posts[0]?.matcher ?? null)}`,
    `tui ok=${tui.ok} error=${tui.error || "none"} pane=${JSON.stringify(String(tui.pane || "").replace(/\s+/g, " ").slice(0, 240))}`,
    `big.txt_bytes=${nbytes} exit=${r.exitCode} elapsed_s=${(r.elapsedMs / 1000).toFixed(1)} model=${r.model || "none"}`,
  ];
}

function grokPostcompactResult(options) {
  const { ctx, r, nbytes, tuiHome, tui, head, tuiEvents } = options;
  const analysis = grokTuiCompactAnalysis(head, tuiEvents);
  saveFix(ctx, "postcompact.json", {
    agent: "grok",
    PreCompact: redactValue(head.preC[0]?.stdin ?? null, r.repo),
    PostCompact: redactValue(head.postC[0]?.stdin ?? null, r.repo),
    tuiPostCompact: redactValue(analysis.tuiPostEvs[0]?.stdin ?? null, tuiHome.repo),
  });
  const evidence = grokCompactEvidence(head, analysis, tui, nbytes, r);
  const status = grokPostcompactStatus({
    posts: head.posts,
    tuiPosts: analysis.tuiPosts,
    allPostEvs: analysis.allPostEvs,
    ident: analysis.ident,
    bOk: analysis.bOk,
    bNote: analysis.bNote,
    tui,
    evidence,
  });
  return { status, evidence, data: { posts: head.posts, pres: head.pres, tuiPosts: analysis.tuiPosts, ident: analysis.ident, tui } };
}

export const MARKER_HOOKS = (marker) =>
  GROK_EVENTS.map((e) => (e === "PreToolUse" ? { event: e, flags: ["--marker", marker] } : e));

export const grokPostcompactProbe = {
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
      const head = grokHeadlessCompact(r);

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
      return grokPostcompactResult({ ctx, r, nbytes, tuiHome, tui, head, tuiEvents });
    },
  };

export const grokResumeProbe = {
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
  };

export const grokStopProbe = {
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
  };
