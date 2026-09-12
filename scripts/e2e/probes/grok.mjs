import fs from "node:fs";
import path from "node:path";

import { GROK_EVENTS, oversizedPrompt, toolUsePrompt } from "../probe-lib/agents.mjs";
import { saveFix, finalText, named, oversizedOutcome, redactValue, shapeProbe, toolNameOf, toolUseIdOf, topKeys } from "../probe-lib/agent-events.mjs";
import { grokMcpProbe } from "../probe-lib/grok-mcp.mjs";
import { MARKER_HOOKS, grokPostcompactProbe, grokResumeProbe, grokStopProbe } from "../probe-lib/grok-lifecycle.mjs";
const ROW_SHAPES = "Native tool payload shapes for read/write/edit/bash on all four agents";
const ROW_OVER = "Hook runner behaviour when the hook exits with unread stdin above 1 MB";
const ROW_PARALLEL =
  "Grok parallel batches: whether `additionalContext` attached to several calls of one batch reaches the model once or once per call";
const ROW_FAIL =
  "Grok Build `PreToolUse` context on an executed-but-failed call (`PostToolUseFailure`)";
const ROW_PERM = "`PermissionDenied` payload";

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
      let status;
      if (oncePerCall) status = "fail";
      else if (oncePerBatch) status = "pass";
      else status = "blocked";
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
  grokPostcompactProbe,
  grokResumeProbe,
  grokMcpProbe,
  grokStopProbe,
];
