#!/usr/bin/env node
// Stdio tee for `oboete mcp`: byte-for-byte forward, JSONL frames to PROBE_MCP_LOG.
import { spawn } from "node:child_process";
import fs from "node:fs";

const bundle = process.argv[2];
if (!bundle) {
  process.stderr.write("mcp-tee: pass the oboete bundle path as the first argument\n");
  process.exit(2);
}

function logFrame(dir, frame) {
  const dest = process.env.PROBE_MCP_LOG;
  if (!dest) return;
  try {
    fs.appendFileSync(dest, `${JSON.stringify({ dir, at: new Date().toISOString(), frame })}\n`);
  } catch {
    /* ignore */
  }
}

function attach(source, dir, sink) {
  let buf = "";
  source.on("data", (chunk) => {
    sink.write(chunk);
    buf += chunk.toString("utf8");
    for (;;) {
      const nl = buf.indexOf("\n");
      if (nl < 0) break;
      const line = buf.slice(0, nl);
      buf = buf.slice(nl + 1);
      if (!line.trim()) continue;
      try {
        logFrame(dir, JSON.parse(line));
      } catch {
        logFrame(dir, { parse_error: line.slice(0, 120) });
      }
    }
  });
  source.on("end", () => {
    if (!buf.trim()) return;
    try {
      logFrame(dir, JSON.parse(buf));
    } catch {
      /* trailing fragment */
    }
  });
}

const child = spawn(process.execPath, [bundle, "mcp"], {
  cwd: process.cwd(),
  env: process.env,
  stdio: ["pipe", "pipe", "inherit"],
});

attach(process.stdin, "in", child.stdin);
attach(child.stdout, "out", process.stdout);
process.stdin.on("end", () => child.stdin.end());
process.stdin.on("error", () => child.stdin.end());

const stop = () => {
  if (!child.killed) child.kill("SIGTERM");
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);

child.on("error", () => process.exit(1));
child.on("close", (code, signal) => {
  if (code == null) {
    process.exit(signal ? 1 : 0);
  } else {
    process.exit(code);
  }
});
