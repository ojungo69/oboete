import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { warmRedactionWorker } from "@codemem/core";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import {
	buildClaudeHookInjection,
	claudeHookInjectCommand,
	type PackResult,
} from "./claude-hook-inject.js";
import { saveSessionState } from "./claude-hook-session-state.js";

const pack = (packText: string, items = 0, packTokens = 0): PackResult => ({
	packText,
	items,
	packTokens,
});
const delivered = async () => ({ via: "rpc" as const });

beforeAll(() => expect(warmRedactionWorker()).toBe(true));

describe("claude-hook-inject", () => {
	let root: string;
	const savedEnv: Record<string, string | undefined> = {};
	const envKeys = [
		"CODEMEM_CLAUDE_HOOK_CONTEXT_DIR",
		"CODEMEM_PLUGIN_LOG_PATH",
		"CODEMEM_INJECT_CONTEXT",
		"CODEMEM_INJECT_MAX_CHARS",
		"CODEMEM_PLUGIN_IGNORE",
	];

	beforeEach(() => {
		root = mkdtempSync(join(tmpdir(), "codemem-claude-inject-"));
		for (const key of envKeys) {
			savedEnv[key] = process.env[key];
			delete process.env[key];
		}
		process.env.CODEMEM_CLAUDE_HOOK_CONTEXT_DIR = root;
		process.env.CODEMEM_PLUGIN_LOG_PATH = join(root, "plugin.log");
	});

	afterEach(() => {
		for (const key of envKeys) {
			const value = savedEnv[key];
			if (value === undefined) delete process.env[key];
			else process.env[key] = value;
		}
		rmSync(root, { recursive: true, force: true });
	});

	it("keeps legacy DB flags without opening SQLite", () => {
		const flags = claudeHookInjectCommand.options.map((option) => option.long);
		expect(flags).toEqual(expect.arrayContaining(["--db", "--db-path"]));
		expect(claudeHookInjectCommand.helpInformation()).toContain("additionalContext");
	});

	it("requests the daemon pack and returns exact additionalContext", async () => {
		const dbPath = join(root, "custom.sqlite");
		const result = await buildClaudeHookInjection(
			{
				hook_event_name: "UserPromptSubmit",
				session_id: "sess-1",
				prompt: "fix auth callback",
				cwd: "/tmp/codemem",
				project: "codemem",
			},
			{ dbPath },
			{
				deliver: async (_agent, _payload, options) => {
					expect(options).toMatchObject({ dbPath });
					return { via: "rpc" };
				},
				requestPack: async (agent, input, options) => {
					expect(agent).toBe("claude");
					expect(options).toMatchObject({ dbPath });
					expect(input).toMatchObject({
						context: "fix auth callback codemem",
						project: "codemem",
						workingSetPaths: [],
						limit: 8,
						tokenBudget: 800,
					});
					return pack("## Summary\n[1] (decision) Auth fix", 1, 42);
				},
			},
		);
		expect(result).toEqual({
			continue: true,
			hookSpecificOutput: {
				hookEventName: "UserPromptSubmit",
				additionalContext: "## Summary\n[1] (decision) Auth fix",
			},
		});
	});

	it("enriches the query from session state and passes working-set paths", async () => {
		saveSessionState("sess-state", {
			first_prompt: "investigate flaky test",
			last_prompt: "investigate flaky test",
			files_modified: ["src/a.ts", "src/b.ts"],
			updated_at: "2026-08-14T00:00:00Z",
		});
		await buildClaudeHookInjection(
			{
				hook_event_name: "UserPromptSubmit",
				session_id: "sess-state",
				prompt: "now check the fixture",
				project: "codemem",
			},
			{},
			{
				deliver: delivered,
				requestPack: async (_agent, input) => {
					expect(input.context).toContain("investigate flaky test");
					expect(input.context).toContain("now check the fixture");
					expect(input.workingSetPaths).toEqual(["src/a.ts", "src/b.ts"]);
					return pack("");
				},
			},
		);
	});

	it("truncates the returned pack and keeps the hook schema stable", async () => {
		process.env.CODEMEM_INJECT_MAX_CHARS = "12";
		const result = await buildClaudeHookInjection(
			{ hook_event_name: "UserPromptSubmit", session_id: "sess-2", prompt: "viewer cards" },
			{},
			{ deliver: delivered, requestPack: async () => pack("12345678901234567890") },
		);
		expect(result.hookSpecificOutput).toEqual({
			hookEventName: "UserPromptSubmit",
			additionalContext: "123456789012\n\n[pack truncated]",
		});
	});

	it("fails open when the daemon read fails", async () => {
		const result = await buildClaudeHookInjection(
			{ hook_event_name: "UserPromptSubmit", session_id: "sess-3", prompt: "resume" },
			{},
			{
				deliver: delivered,
				requestPack: async () => {
					throw new Error("daemon unavailable");
				},
			},
		);
		expect(result).toEqual({ continue: true });
	});

	it("honors the global kill switch before RPC or event delivery", async () => {
		process.env.CODEMEM_PLUGIN_IGNORE = "1";
		let called = false;
		const result = await buildClaudeHookInjection(
			{ hook_event_name: "UserPromptSubmit", session_id: "sess-4", prompt: "resume" },
			{},
			{
				deliver: async () => {
					called = true;
					return { via: "rpc" };
				},
				requestPack: async () => {
					called = true;
					return pack("");
				},
			},
		);
		expect(result).toEqual({ continue: true });
		expect(called).toBe(false);
	});

	it("logs RPC pack metrics without pack content", async () => {
		await buildClaudeHookInjection(
			{
				hook_event_name: "UserPromptSubmit",
				session_id: "sess-log",
				prompt: "ship feature",
				project: "codemem",
			},
			{},
			{ deliver: delivered, requestPack: async () => pack("private body", 4, 137) },
		);
		const log = readFileSync(process.env.CODEMEM_PLUGIN_LOG_PATH as string, "utf8");
		expect(log).toContain("origin=rpc");
		expect(log).toContain("items=4");
		expect(log).toContain("pack_tokens=137");
		expect(log).not.toContain("private body");
	});
});
