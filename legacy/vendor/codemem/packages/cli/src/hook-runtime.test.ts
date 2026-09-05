import {
	existsSync,
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { resolveSpoolLayout } from "@codemem/core";
import { describe, expect, it } from "vitest";
import { HOOK_RUNTIME_INPUT_MAX_BYTES, runHookRuntime } from "./hook-runtime.js";

describe("bundled hook runtime", () => {
	it("fails open without persisting invalid or oversized input", async () => {
		const root = mkdtempSync(join(tmpdir(), "codemem-hook-runtime-"));
		const env = {
			CODEMEM_CLAUDE_HOOK_CONTEXT_DIR: process.env.CODEMEM_CLAUDE_HOOK_CONTEXT_DIR,
			CODEMEM_DATA_DIR: process.env.CODEMEM_DATA_DIR,
			CODEMEM_PLUGIN_IGNORE: process.env.CODEMEM_PLUGIN_IGNORE,
			CODEMEM_PLUGIN_LOG_PATH: process.env.CODEMEM_PLUGIN_LOG_PATH,
		};
		try {
			const dataDir = join(root, "data");
			const readyDir = resolveSpoolLayout(dataDir).readyDir;
			const ready = () => (existsSync(readyDir) ? readdirSync(readyDir) : []);
			process.env.CODEMEM_DATA_DIR = dataDir;
			process.env.CODEMEM_CLAUDE_HOOK_CONTEXT_DIR = join(root, "claude-context");
			process.env.CODEMEM_PLUGIN_LOG_PATH = join(root, "plugin.log");
			delete process.env.CODEMEM_PLUGIN_IGNORE;

			await expect(runHookRuntime("claude-hook-ingest", "not-json")).resolves.toBe("");
			await expect(runHookRuntime("codex-hook-ingest", "not-json")).resolves.toBe(
				'{"continue":true}',
			);
			await expect(runHookRuntime("claude-hook-file-context", "null")).resolves.toBe(
				'{"continue":true}',
			);
			await expect(runHookRuntime("codex-hook-inject", "[]")).resolves.toBe('{"continue":true}');
			await expect(
				runHookRuntime("claude-hook-inject", "x".repeat(HOOK_RUNTIME_INPUT_MAX_BYTES + 1)),
			).resolves.toBe('{"continue":true}');

			process.env.CODEMEM_PLUGIN_IGNORE = "true";
			await expect(runHookRuntime("claude-hook-ingest", "{}")).resolves.toBe("");
			await expect(runHookRuntime("codex-hook-ingest", "{}")).resolves.toBe('{"continue":true}');
			delete process.env.CODEMEM_PLUGIN_IGNORE;
			expect(ready()).toEqual([]);
			await expect(
				runHookRuntime("codex-hook-ingest", "{}", performance.now() + 1_000),
			).resolves.toBe('{"continue":true}');

			const claudePayload = (sessionId: string) =>
				JSON.stringify({
					hook_event_name: "UserPromptSubmit",
					session_id: sessionId,
					prompt: "resume work",
					cwd: root,
				});
			await expect(
				runHookRuntime("claude-hook-ingest", claudePayload("claude-ingest")),
			).resolves.toBe("");
			expect(ready()).toHaveLength(1);
			await expect(
				runHookRuntime("codex-hook-ingest", claudePayload("codex-ingest")),
			).resolves.toBe('{"continue":true}');
			expect(ready()).toHaveLength(2);
			const claudeInject = await runHookRuntime(
				"claude-hook-inject",
				claudePayload("claude-inject"),
			);
			expect(JSON.parse(claudeInject)).toMatchObject({ continue: true });
			expect(ready()).toHaveLength(3);
			const codexInject = await runHookRuntime("codex-hook-inject", claudePayload("codex-inject"));
			expect(JSON.parse(codexInject)).toMatchObject({ continue: true });
			expect(ready()).toHaveLength(4);

			const repo = join(root, "repo");
			const file = join(repo, "app.json");
			mkdirSync(repo);
			writeFileSync(file, "{}\n");
			await expect(
				runHookRuntime(
					"claude-hook-file-context",
					JSON.stringify({
						hook_event_name: "PreToolUse",
						session_id: "claude-file-context",
						tool_use_id: "tool-1",
						tool_name: "Read",
						tool_input: { file_path: file },
						cwd: repo,
						project: "repo",
					}),
				),
			).resolves.toSatisfy((value) => JSON.parse(value).continue === true);
			const events = ready().map((name) =>
				JSON.parse(readFileSync(join(readyDir, name), "utf8")),
			) as Array<{ body: { event: { kind: string; nativeSessionId: string } } }>;
			expect(events.map((entry) => entry.body.event.nativeSessionId).sort()).toEqual([
				"claude-file-context",
				"claude-ingest",
				"claude-inject",
				"codex-ingest",
				"codex-inject",
			]);
			expect(events.map((entry) => entry.body.event.kind).sort()).toEqual([
				"tool_started",
				"user_prompted",
				"user_prompted",
				"user_prompted",
				"user_prompted",
			]);
			const claudeBundle = readFileSync(
				join(process.cwd(), "plugins", "claude", "scripts", "hook-runtime.mjs"),
				"utf8",
			);
			const codexBundle = readFileSync(
				join(process.cwd(), "plugins", "codex", "scripts", "hook-runtime.mjs"),
				"utf8",
			);
			expect(claudeBundle).toBe(codexBundle);
			expect(claudeBundle).not.toMatch(/^\/\/#region (?:\/|[A-Za-z]:[\\/]).*node_modules/m);
		} finally {
			for (const [key, value] of Object.entries(env)) {
				if (value === undefined) delete process.env[key];
				else process.env[key] = value;
			}
			rmSync(root, { recursive: true, force: true });
		}
	});

	it("rejects commands outside the hook-only allowlist", async () => {
		await expect(runHookRuntime("memory-forget", "{}")).rejects.toThrow("unsupported hook command");
	});
});
