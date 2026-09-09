import assert from "node:assert/strict";
import test from "node:test";

import { finalText, toolInputOf, toolNameOf, toolOutputOf, toolUseIdOf } from "./agent-events.mjs";

test("finalText preserves each agent's native response format and Stop fallback", () => {
  const cases = [
    ["claude", { stdout: JSON.stringify({ result: "CLAUDE" }) }, [], "CLAUDE"],
    ["codex", { stdout: `${JSON.stringify({ type: "item.completed", item: { text: "CODEX" } })}\n` }, [], "CODEX"],
    ["grok", { stdout: JSON.stringify({ text: "GROK" }) }, [], "GROK"],
    ["pi", { stdout: `${JSON.stringify({ type: "turn_end", message: { content: [{ type: "text", text: "PI" }] } })}\n` }, [], "PI"],
    ["unknown", { stdout: "tail" }, [{ event: "Stop", stdin: { lastAssistantMessage: "STOP" } }], "STOP"],
  ];
  for (const [agent, process, events, expected] of cases) {
    assert.equal(finalText(agent, process, events), expected, agent);
  }
});

test("tool frame accessors accept the native snake-case fields", () => {
  const event = {
    stdin: {
      tool_name: "Read",
      tool_input: { file_path: "README.md" },
      tool_response: { filePath: "README.md" },
      tool_use_id: "tool-1",
    },
  };
  assert.equal(toolNameOf(event), "Read");
  assert.deepEqual(toolInputOf(event), { file_path: "README.md" });
  assert.deepEqual(toolOutputOf(event), { filePath: "README.md" });
  assert.equal(toolUseIdOf(event), "tool-1");
});
