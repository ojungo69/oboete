import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { warmRedactionWorker } from "@codemem/core";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { buildClaudeHookInjection } from "./claude-hook-inject.js";

type Fixture = {
	name: string;
	payload: Record<string, unknown>;
	pack?: string;
	env?: Record<string, string>;
	expected: Record<string, unknown>;
};

const fixtures: Fixture[] = [
	{
		name: "prompt emits exact UserPromptSubmit output",
		payload: { hook_event_name: "UserPromptSubmit", session_id: "c1", prompt: "resume" },
		pack: "GOLDEN_PACK_BODY",
		expected: {
			continue: true,
			hookSpecificOutput: {
				hookEventName: "UserPromptSubmit",
				additionalContext: "GOLDEN_PACK_BODY",
			},
		},
	},
	{
		name: "non-prompt events cannot inject prompt-shaped data",
		payload: { hook_event_name: "SessionStart", session_id: "c2", prompt: "resume" },
		pack: "INVARIANT_PACK",
		expected: { continue: true },
	},
	{
		name: "empty prompt continues without output",
		payload: { hook_event_name: "UserPromptSubmit", session_id: "c3", prompt: "" },
		pack: "unused",
		expected: { continue: true },
	},
	{
		name: "disabled injection continues without output",
		payload: { hook_event_name: "UserPromptSubmit", session_id: "c4", prompt: "resume" },
		env: { CODEMEM_INJECT_CONTEXT: "0" },
		pack: "unused",
		expected: { continue: true },
	},
	{
		name: "long pack is truncated deterministically",
		payload: { hook_event_name: "UserPromptSubmit", session_id: "c5", prompt: "resume" },
		env: { CODEMEM_INJECT_MAX_CHARS: "12" },
		pack: "12345678901234567890",
		expected: {
			continue: true,
			hookSpecificOutput: {
				hookEventName: "UserPromptSubmit",
				additionalContext: "123456789012\n\n[pack truncated]",
			},
		},
	},
];

beforeAll(() => expect(warmRedactionWorker()).toBe(true));

describe("claude-hook-inject output contract", () => {
	let root: string;
	const saved: Record<string, string | undefined> = {};
	const keys = [
		"CODEMEM_CLAUDE_HOOK_CONTEXT_DIR",
		"CODEMEM_PLUGIN_LOG_PATH",
		"CODEMEM_INJECT_CONTEXT",
		"CODEMEM_INJECT_MAX_CHARS",
	];

	beforeEach(() => {
		root = mkdtempSync(join(tmpdir(), "codemem-inject-contract-"));
		for (const key of keys) {
			saved[key] = process.env[key];
			delete process.env[key];
		}
		process.env.CODEMEM_CLAUDE_HOOK_CONTEXT_DIR = root;
		process.env.CODEMEM_PLUGIN_LOG_PATH = join(root, "plugin.log");
	});

	afterEach(() => {
		for (const key of keys) {
			const value = saved[key];
			if (value === undefined) delete process.env[key];
			else process.env[key] = value;
		}
		rmSync(root, { recursive: true, force: true });
	});

	it.each(fixtures)("$name", async ({ payload, pack = "", env, expected }) => {
		for (const [key, value] of Object.entries(env ?? {})) process.env[key] = value;
		const result = await buildClaudeHookInjection(
			payload,
			{},
			{
				deliver: async () => ({ via: "rpc" }),
				requestPack: async () => ({
					packText: pack,
					items: pack ? 1 : 0,
					packTokens: pack.length,
				}),
			},
		);
		expect(result).toEqual(expected);
	});
});
