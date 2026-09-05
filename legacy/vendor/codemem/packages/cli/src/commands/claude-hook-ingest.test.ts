import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { claudeHookIngestCommand, ingestClaudeHookPayload } from "./claude-hook-ingest.js";

describe("claude-hook-ingest", () => {
	let stateDir: string;
	let originalStateDir: string | undefined;

	beforeEach(() => {
		originalStateDir = process.env.CODEMEM_CLAUDE_HOOK_CONTEXT_DIR;
		stateDir = mkdtempSync(join(tmpdir(), "codemem-claude-ingest-"));
		process.env.CODEMEM_CLAUDE_HOOK_CONTEXT_DIR = stateDir;
	});

	afterEach(() => {
		if (originalStateDir === undefined) delete process.env.CODEMEM_CLAUDE_HOOK_CONTEXT_DIR;
		else process.env.CODEMEM_CLAUDE_HOOK_CONTEXT_DIR = originalStateDir;
		rmSync(stateDir, { recursive: true, force: true });
	});

	it("keeps legacy flags while describing daemon delivery", () => {
		const flags = claudeHookIngestCommand.options.map((option) => option.long);
		expect(flags).toEqual(expect.arrayContaining(["--db", "--db-path", "--host", "--port"]));
		expect(claudeHookIngestCommand.helpInformation()).toContain("local codemem daemon");
	});

	it.each([
		["rpc", { inserted: 1, skipped: 0, via: "rpc" }],
		["spool", { inserted: 0, skipped: 0, via: "spool" }],
		["skipped", { inserted: 0, skipped: 1, via: "skipped" }],
		["dropped", { inserted: 0, skipped: 0, via: "dropped" }],
	] as const)("reports %s delivery without a DB fallback", async (via, expected) => {
		const dbPath = join(stateDir, "custom.sqlite");
		const payload = {
			hook_event_name: "UserPromptSubmit",
			session_id: "claude-session",
			prompt: "resume work",
		};
		let delivered: Record<string, unknown> | undefined;
		const result = await ingestClaudeHookPayload(
			payload,
			{ host: "127.0.0.1", port: 38888, dbPath },
			{
				deliver: async (agent, value, options) => {
					expect(agent).toBe("claude");
					expect(options).toMatchObject({ dbPath });
					delivered = value;
					return { via };
				},
			},
		);
		expect(delivered).toBe(payload);
		expect(result).toEqual(expected);
	});
});
