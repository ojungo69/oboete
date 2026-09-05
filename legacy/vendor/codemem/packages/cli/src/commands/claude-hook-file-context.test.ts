import type { FileContextRetrievalAttempt, RefQueryResult } from "@codemem/core";
import { describe, expect, it } from "vitest";
import {
	buildClaudeFileContext,
	claudeHookFileContextCommand,
} from "./claude-hook-file-context.js";

const delivered = async () => ({ via: "rpc" as const });

function row(
	id: number,
	sessionId: number,
	createdAt: string,
	filesModified: string[] = [],
): RefQueryResult {
	return {
		id,
		session_id: sessionId,
		kind: "decision",
		title: `Memory ${id}`,
		subtitle: null,
		body_text: "body",
		narrative: null,
		confidence: 0.9,
		tags_text: "[]",
		created_at: createdAt,
		updated_at: createdAt,
		files_read: null,
		files_modified: JSON.stringify(filesModified),
		concepts: null,
		metadata_json: null,
	};
}

function payload(filePath = "/repo/src/large.ts"): Record<string, unknown> {
	return {
		hook_event_name: "PreToolUse",
		session_id: "claude-session",
		tool_use_id: "tool-1",
		tool_name: "Read",
		tool_input: { file_path: filePath },
		cwd: "/repo",
		project: "repo",
		timestamp: "2026-08-14T01:00:00.000Z",
	};
}

describe("claude-hook-file-context", () => {
	it("keeps legacy DB flags while describing PreToolUse output", () => {
		const flags = claudeHookFileContextCommand.options.map((option) => option.long);
		expect(flags).toEqual(expect.arrayContaining(["--db", "--db-path"]));
		expect(claudeHookFileContextCommand.helpInformation()).toContain("PreToolUse:Read");
	});

	it("delivers the hook event even when no file path can be searched", async () => {
		let count = 0;
		const dbPath = "/tmp/codemem-file-context.sqlite";
		const result = await buildClaudeFileContext(
			{ hook_event_name: "PreToolUse", session_id: "s", tool_name: "Read" },
			{ dbPath },
			{
				deliver: async (_agent, _payload, options) => {
					expect(options).toMatchObject({ dbPath });
					count += 1;
					return { via: "rpc" };
				},
			},
		);
		expect(result).toEqual({ continue: true });
		expect(count).toBe(1);
	});

	it("rejects paths outside cwd before daemon search", async () => {
		let queried = false;
		const result = await buildClaudeFileContext(
			payload("/repo/../secret.ts"),
			{},
			{
				deliver: delivered,
				queryByFile: async () => {
					queried = true;
					return [];
				},
			},
		);
		expect(result).toEqual({ continue: true });
		expect(queried).toBe(false);
	});

	it("applies the size gate but bypasses it for config files", async () => {
		let queries = 0;
		const deps = {
			deliver: delivered,
			statFile: () => ({ sizeBytes: 20, mtimeMs: 0 }),
			queryByFile: async () => {
				queries += 1;
				return [];
			},
		};
		await buildClaudeFileContext(payload("/repo/src/tiny.ts"), {}, deps);
		await buildClaudeFileContext(payload("/repo/config/app.yaml"), {}, deps);
		expect(queries).toBe(1);
	});

	it("scores, deduplicates, and formats daemon search results", async () => {
		let attempt: FileContextRetrievalAttempt | undefined;
		let delivery: string | undefined;
		const result = await buildClaudeFileContext(
			payload(),
			{},
			{
				deliver: delivered,
				statFile: () => ({ sizeBytes: 2_000, mtimeMs: Date.parse("2026-08-14T02:00:00Z") }),
				queryByFile: async (repositoryPath, project, limit) => {
					expect(repositoryPath).toBe("src/large.ts");
					expect(project).toBe("repo");
					expect(limit).toBe(40);
					return [
						row(1, 10, "2026-08-14T01:00:00Z", ["src/other.ts"]),
						row(2, 10, "2026-08-13T23:00:00Z", ["src/large.ts"]),
						row(3, 11, "2026-08-14T00:30:00Z", ["src/large.ts"]),
					];
				},
				recordAttempt: async (input) => {
					attempt = input;
				},
				updateDelivery: async (_attemptId, status) => {
					delivery = status;
				},
			},
		);
		const context = result.hookSpecificOutput?.additionalContext ?? "";
		expect(result.hookSpecificOutput?.permissionDecision).toBe("allow");
		expect(context).toContain("Past entries may be partially stale");
		expect(context).toContain("Memory 2");
		expect(context).toContain("Memory 3");
		expect(context).not.toContain("Memory 1");
		expect(attempt).toMatchObject({
			retrievalStatus: "succeeded",
			candidateIds: [1, 2, 3],
			selectedIds: [2, 3],
			repositoryPath: "src/large.ts",
		});
		expect(delivery).toBe("handed_off");
	});

	it("records a no-results attempt", async () => {
		let attempt: FileContextRetrievalAttempt | undefined;
		await buildClaudeFileContext(
			payload(),
			{},
			{
				deliver: delivered,
				statFile: () => ({ sizeBytes: 2_000, mtimeMs: 0 }),
				queryByFile: async () => [],
				recordAttempt: async (input) => {
					attempt = input;
				},
			},
		);
		expect(attempt).toMatchObject({
			retrievalStatus: "no_results",
			repositoryPath: "src/large.ts",
		});
	});

	it("fails open when daemon search fails", async () => {
		let attempt: FileContextRetrievalAttempt | undefined;
		const result = await buildClaudeFileContext(
			payload(),
			{},
			{
				deliver: delivered,
				statFile: () => ({ sizeBytes: 2_000, mtimeMs: 0 }),
				queryByFile: async () => {
					throw new Error("daemon unavailable");
				},
				recordAttempt: async (input) => {
					attempt = input;
				},
			},
		);
		expect(result).toEqual({ continue: true });
		expect(attempt).toMatchObject({
			retrievalStatus: "failed",
			failureCode: "query_failed",
			failureStage: "retrieval",
		});
	});

	it("marks delivery failed when formatting cannot hand off selected rows", async () => {
		const broken = row(7, 12, "2026-08-14T00:30:00Z", ["src/large.ts"]);
		Object.defineProperty(broken, "title", {
			get: () => {
				throw new Error("broken title");
			},
		});
		const deliveries: string[] = [];
		const result = await buildClaudeFileContext(
			payload(),
			{},
			{
				deliver: delivered,
				statFile: () => ({ sizeBytes: 2_000, mtimeMs: 0 }),
				queryByFile: async () => [broken],
				recordAttempt: async () => {},
				updateDelivery: async (_attemptId, status) => {
					deliveries.push(status);
				},
			},
		);
		expect(result).toEqual({ continue: true });
		expect(deliveries).toEqual(["failed"]);
	});

	it("honors the global kill switch before event delivery", async () => {
		const original = process.env.CODEMEM_PLUGIN_IGNORE;
		process.env.CODEMEM_PLUGIN_IGNORE = "1";
		try {
			let called = false;
			const result = await buildClaudeFileContext(
				payload(),
				{},
				{
					deliver: async () => {
						called = true;
						return { via: "rpc" };
					},
				},
			);
			expect(result).toEqual({ continue: true });
			expect(called).toBe(false);
		} finally {
			if (original === undefined) delete process.env.CODEMEM_PLUGIN_IGNORE;
			else process.env.CODEMEM_PLUGIN_IGNORE = original;
		}
	});
});
