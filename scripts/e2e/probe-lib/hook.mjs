#!/usr/bin/env node
// Recorder for Claude Code / Codex / Grok hook processes. Always exit 0.
import fs from "node:fs";

process.on("uncaughtException", () => process.exit(0));
process.on("unhandledRejection", () => process.exit(0));
process.on("SIGPIPE", () => process.exit(0));

const args = process.argv.slice(2);
const event = args[0] || "unknown";
const flags = { noRead: false, deny: false, marker: null, plain: null, grepTranscript: false };
for (let i = 1; i < args.length; i++) {
  const a = args[i];
  if (a === "--no-read") flags.noRead = true;
  else if (a === "--deny") flags.deny = true;
  else if (a === "--grep-transcript") flags.grepTranscript = true;
  else if (a === "--marker") flags.marker = args[i + 1] && !args[i + 1].startsWith("--") ? args[++i] : "probe";
  else if (a === "--plain") flags.plain = args[i + 1] && !args[i + 1].startsWith("--") ? args[++i] : "probe";
}

function envSnapshot() {
  const env = {};
  for (const [k, v] of Object.entries(process.env)) {
    if (/^(CLAUDE_|CODEX_|GROK_|PI_|HOOK)/.test(k)) env[k] = v;
  }
  return env;
}

function append(rec) {
  const out = process.env.PROBE_EVENTS;
  if (!out) return;
  try {
    fs.appendFileSync(out, JSON.stringify(rec) + "\n");
  } catch {
    /* ignore */
  }
}

let finished = false;
function finish(raw) {
  if (finished) return;
  finished = true;
  let stdin;
  try {
    stdin = raw ? JSON.parse(raw) : raw;
  } catch {
    stdin = raw;
  }
  const rec = {
    event,
    env: envSnapshot(),
    stdin,
    at: new Date().toISOString(),
    stdinBytes: Buffer.byteLength(raw || "", "utf8"),
  };
  if (flags.grepTranscript && stdin && typeof stdin === "object") {
    const tp = stdin.transcript_path || stdin.transcriptPath;
    const id = stdin.tool_use_id || stdin.toolUseId;
    if (tp && id) {
      try {
        rec.transcript_bytes = fs.statSync(tp).size;
        rec.transcript_has_tool_use_id = fs.readFileSync(tp, "utf8").includes(String(id));
      } catch {
        rec.transcript_bytes = 0;
        rec.transcript_has_tool_use_id = false;
      }
    }
  }
  append(rec);
  if (flags.deny) {
    process.stdout.write(
      JSON.stringify({
        hookSpecificOutput: {
          hookEventName: "PreToolUse",
          permissionDecision: "deny",
          permissionDecisionReason: "probe",
        },
      }),
    );
  } else if (flags.marker != null) {
    process.stdout.write(
      JSON.stringify({
        hookSpecificOutput: {
          hookEventName: event,
          additionalContext: flags.marker,
        },
      }),
    );
  } else if (flags.plain != null) {
    const text = flags.plain.startsWith("{") ? ` ${flags.plain}` : flags.plain;
    process.stdout.write(text);
  }
  process.exit(0);
}

if (flags.noRead) {
  append({ event, env: envSnapshot(), at: new Date().toISOString(), unread: true });
  process.exit(0);
}

let raw = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (c) => {
  raw += c;
});
process.stdin.on("end", () => finish(raw));
process.stdin.on("error", () => finish(raw));
process.on("SIGTERM", () => finish(raw));
process.on("SIGINT", () => finish(raw));
