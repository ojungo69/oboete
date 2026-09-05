#!/usr/bin/env node
import fs from "node:fs";
import readline from "node:readline";

function logFrame(dir, frame) {
  const p = process.env.PROBE_MCP_LOG;
  if (!p) return;
  try {
    fs.appendFileSync(p, JSON.stringify({ dir, at: new Date().toISOString(), frame }) + "\n");
  } catch {
    /* ignore */
  }
}

function send(obj) {
  logFrame("out", obj);
  process.stdout.write(JSON.stringify(obj) + "\n");
}

const rl = readline.createInterface({ input: process.stdin });
rl.on("line", (line) => {
  if (!line.trim()) return;
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    return;
  }
  logFrame("in", msg);
  const { id, method, params } = msg;
  if (method === "initialize") {
    send({
      jsonrpc: "2.0",
      id,
      result: {
        protocolVersion: params?.protocolVersion || "2024-11-05",
        capabilities: { tools: {} },
        serverInfo: { name: "oboete-dummy", version: "0" },
      },
    });
    return;
  }
  if (method === "notifications/initialized") return;
  if (method === "tools/list") {
    send({
      jsonrpc: "2.0",
      id,
      result: {
        tools: [
          {
            name: "search",
            description: "dummy search",
            inputSchema: { type: "object", properties: { query: { type: "string" } }, required: ["query"] },
          },
        ],
      },
    });
    return;
  }
  if (method === "tools/call") {
    const q = params?.arguments?.query ?? "";
    send({
      jsonrpc: "2.0",
      id,
      result: { content: [{ type: "text", text: "dummy result for " + q }] },
    });
    return;
  }
  if (id !== undefined) {
    send({ jsonrpc: "2.0", id, error: { code: -32601, message: "Method not found" } });
  }
});
rl.on("close", () => process.exit(0));
