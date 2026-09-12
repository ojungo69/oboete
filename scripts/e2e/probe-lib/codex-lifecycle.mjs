import { randomBytes } from "node:crypto";
import fs from "node:fs";
import path from "node:path";

import { DONE_PROMPT, shellQuote } from "./agents.mjs";
import { compactionIdentity, eventsFile, parseEvents, redactValue, summaryOf, topKeys, truncateEvents, writeFixture } from "./agent-events.mjs";
import { PreconditionError, agentPath, binVersion, waitUntil } from "./process.mjs";
import { TRUST_PANE_RE, readyTui, tmux, tmuxSession, tuiCmd, tuiQuit, tuiSubmit } from "./tmux.mjs";
const ROW_SESSION = "Codex `SessionStart` fires with `source = compact` and `clear`";

const ROW_POSTCOMPACT =
  "Codex and Grok `PostCompact` payload (summary text field); Compaction identity and order per agent";

const ROW_TUI = "TUI trust path";

const COMPACT_ARGS = [
  "-c",
  "model_auto_compact_token_limit=2000",
  "-c",
  'model_auto_compact_token_limit_scope="body_after_prefix"',
];

const COMPACT_PROMPT =
  "Use a shell command to read the file big.txt in the current directory twice (run: cat big.txt; cat big.txt). Then reply with exactly the word DONE.";

const RESUME_PROMPT = "Reply with exactly the word DONE followed by every marker token you have seen.";

const TUI_PROMPT = "Use the shell to run: echo tui-ok ; then reply with exactly the word DONE";

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
  const name = "obc-" + Date.now().toString(36) + "-" + randomBytes(2).toString("hex");
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

function saveCodexResumeArtifacts(dirA, a) {
  fs.copyFileSync(path.join(dirA, "stdout.txt"), path.join(dirA, "a-stdout.txt"));
  fs.copyFileSync(path.join(dirA, "stderr.txt"), path.join(dirA, "a-stderr.txt"));
  fs.copyFileSync(path.join(a.tree, "events.jsonl"), path.join(dirA, "a-events.jsonl"));
}

function recordCodexStartup(a, observed, evidence) {
  const srcA = sessionSources(a.events);
  observed.push(...srcA);
  evidence.push(`A_startup sources=[${srcA.join(",")}] session=${a.sessionId || "none"} exit=${a.exitCode}`);
  const threadId = a.sessionId;
  return threadId;
}

function prepareCodexCompactDir(ctx) {
  const dirC = path.join(ctx.dir, "c");
  fs.mkdirSync(path.join(dirC, "repo"), { recursive: true });
  writeBigFile(path.join(dirC, "repo"));
  return dirC;
}

function recordCodexCompactSession(ctx, dirC, c, observed, evidence) {
  const srcC = sessionSources(c.events);
  observed.push(...srcC);
  const tlC = compactTimeline(c.events);
  evidence.push(
    `C_compact sources=[${srcC.join(",")}] events=${c.events.map((e) => e.event).join(">")}`,
    `C_timeline=${JSON.stringify(tlC)}`,
  );
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
}

function recordCodexTuiSession(seed, dEvents, observed, evidence, tuiBlocked) {
  const srcD = sessionSources(dEvents);
  observed.push(...srcD);
  const clearEv = dEvents.filter((e) => e.event === "SessionStart" && e.stdin?.source === "clear");
  const newIds = [...new Set(clearEv.map((e) => e.stdin?.session_id).filter(Boolean))];
  evidence.push(
    `D_tui sources=[${srcD.join(",")}] clear_session_ids=[${newIds.join(",")}] seed_session=${seed.sessionId || "none"}`,
    `D_new_session_id=${newIds.length ? newIds.some((id) => id !== seed.sessionId) : "no-clear-event"}`,
  );
  if (tuiBlocked) evidence.push(`D_tui_blocked=${tuiBlocked}`);
  const uniq = [...new Set(observed)];
  evidence.push(`observed_sources=[${uniq.join(",")}]`);
  const hasCompact = uniq.includes("compact");
  const hasClear = uniq.includes("clear");
  const cannotRun = Boolean(tuiBlocked && /composer never appeared|tmux new-session/i.test(tuiBlocked));
  if (tuiBlocked) evidence.push(TUI_MANUAL);
  return { uniq, hasCompact, hasClear, cannotRun };
}

function recordCodexHeadlessCompact(c, evidence) {
  const preC = c.events.filter((e) => e.event === "PreCompact");
  const postC = c.events.filter((e) => e.event === "PostCompact");
  const sumC = postC.map((e) => summaryOf(e.stdin));
  evidence.push(
    `C PreCompact n=${preC.length} keys=[${preC.map((e) => topKeys(e.stdin).join("|")).join(" ; ")}]`,
    `C PostCompact n=${postC.length} keys=[${postC.map((e) => topKeys(e.stdin).join("|")).join(" ; ")}] summary=${JSON.stringify(sumC)} identity=${JSON.stringify(compactionIdentity(postC))}`,
    `C timeline=${JSON.stringify(compactTimeline(c.events))}`,
  );
  const ordC = orderOk(c.events);
  evidence.push(`C order_b=${ordC.ok} ${ordC.detail}`);
  return { preC, postC };
}

function recordCodexTuiCompact(dEvents, evidence, tuiBlocked) {
  const postD = dEvents.filter((e) => e.event === "PostCompact");
  const preD = dEvents.filter((e) => e.event === "PreCompact");
  evidence.push(
    `D PreCompact n=${preD.length} keys=[${preD.map((e) => topKeys(e.stdin).join("|")).join(" ; ")}]`,
    `D PostCompact n=${postD.length} keys=[${postD.map((e) => topKeys(e.stdin).join("|")).join(" ; ")}] summary=${JSON.stringify(postD.map((e) => summaryOf(e.stdin)))} identity=${JSON.stringify(compactionIdentity(postD))}`,
    `D timeline=${JSON.stringify(compactTimeline(dEvents))}`,
  );
  if (tuiBlocked) evidence.push(`D_tui_blocked=${tuiBlocked}`);
  return { postD, preD };
}

function writeCodexPostcompactFixture(options) {
  const { ctx, c, seed, summary, aOk, bOk, preC, postC, preD, postD, dEvents } = options;
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
}

function recordCodexPostcompact(options) {
  const { ctx, c, seed, preC, postC, preD, postD, dEvents, tuiBlocked, evidence } = options;
  const posts = [...postC, ...postD];
  const summary = posts.length ? summaryOf(posts.at(-1).stdin) : { field: null, length: 0 };
  evidence.push(`summary_field=${summary.field} summary_length=${summary.length}`);
  let aOk = false;
  let aDetail;
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
  evidence.push(`pass_a=${aOk} ${aDetail}`, `pass_b=${bOk} ${ord.detail}`);
  writeCodexPostcompactFixture({ ctx, c, seed, summary, aOk, bOk, preC, postC, preD, postD, dEvents });
  return { posts, summary, aOk, bOk };
}

export const codexSessionStartProbe = {
    id: "codex-session-start-sources",
    agent: "codex",
    row: ROW_SESSION,
    async run(ctx) {
      const evidence = [];
      const observed = [];
      const dirA = path.join(ctx.dir, "a");
      const a = await ctx.codex(dirA, { prompt: DONE_PROMPT });
      const threadId = recordCodexStartup(a, observed, evidence);
      if (!threadId) {
        evidence.push("A produced no session_id; resume skipped");
      } else {
        saveCodexResumeArtifacts(dirA, a);
        const b = await ctx.codex(dirA, { extraArgs: ["resume", threadId], prompt: RESUME_PROMPT });
        const srcB = sessionSources(b.events);
        observed.push(...srcB);
        const same = (b.sessionId || "") === threadId || b.events.some((e) => e.stdin?.session_id === threadId);
        evidence.push(
          `B_resume sources=[${srcB.join(",")}] session=${b.sessionId || "none"} same_session_id=${same} exit=${b.exitCode}`,
        );
      }

      const dirC = prepareCodexCompactDir(ctx);
      const c = await ctx.codex(dirC, { extraArgs: COMPACT_ARGS, prompt: COMPACT_PROMPT });
      recordCodexCompactSession(ctx, dirC, c, observed, evidence);

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
      const { uniq, hasCompact, hasClear, cannotRun } = recordCodexTuiSession(seed, dEvents, observed, evidence, tuiBlocked);
      if (!hasClear && cannotRun) {
        return { status: "blocked", evidence, data: { observed: uniq, tuiBlocked } };
      }
      return {
        status: hasCompact && hasClear ? "pass" : "fail",
        evidence,
        data: { observed: uniq, hasCompact, hasClear, beforeIds: [...beforeIds] },
      };
    },
  };

export const codexPostcompactProbe = {
    id: "codex-postcompact-payload",
    agent: "codex",
    row: ROW_POSTCOMPACT,
    async run(ctx) {
      const evidence = [];
      const dirC = path.join(ctx.dir, "c");
      fs.mkdirSync(path.join(dirC, "repo"), { recursive: true });
      writeBigFile(path.join(dirC, "repo"));
      const c = await ctx.codex(dirC, { extraArgs: COMPACT_ARGS, prompt: COMPACT_PROMPT });
      const { preC, postC } = recordCodexHeadlessCompact(c, evidence);

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
      const { postD, preD } = recordCodexTuiCompact(dEvents, evidence, tuiBlocked);
      const { posts, summary, aOk, bOk } = recordCodexPostcompact({
        ctx,
        c,
        seed,
        preC,
        postC,
        preD,
        postD,
        dEvents,
        tuiBlocked,
        evidence,
      });
      if (!posts.length) return { status: "fail", evidence, data: { aOk, bOk } };
      if (postD.length < 2 && tuiBlocked) return { status: "blocked", evidence, data: { aOk, bOk, tuiBlocked } };
      return { status: aOk && bOk ? "pass" : "fail", evidence, data: { aOk, bOk, summary } };
    },
  };

export const codexTuiTrustProbe = {
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
  };
