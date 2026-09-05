import { describe, expect, it } from "vitest";
import { codexHookIngestCommand, ingestCodexHookPayload } from "./codex-hook-ingest.js";

describe("codex-hook-ingest", () => {
	it("keeps legacy flags while describing daemon delivery", () => {
		const flags = codexHookIngestCommand.options.map((option) => option.long);
		expect(flags).toEqual(expect.arrayContaining(["--db", "--db-path", "--host", "--port"]));
		expect(codexHookIngestCommand.helpInformation()).toContain("local codemem daemon");
	});

	it("reports daemon delivery without a DB fallback", async () => {
		const dbPath = "/tmp/codemem-codex-ingest.sqlite";
		const result = await ingestCodexHookPayload(
			{
				hook_event_name: "UserPromptSubmit",
				session_id: "codex-session",
				prompt: "resume work",
				timestamp: "2026-08-14T01:00:00.000Z",
			},
			{ host: "127.0.0.1", port: 38888, dbPath },
			{
				deliver: async (agent, _payload, options) => {
					expect(agent).toBe("codex");
					expect(options).toMatchObject({ dbPath });
					return { via: "spool" };
				},
			},
		);
		expect(result).toEqual({ inserted: 0, skipped: 0, via: "spool" });
	});

	it("gives repeated timestamp-less payloads distinct delivery identities", async () => {
		const delivered: Record<string, unknown>[] = [];
		const payload = {
			hook_event_name: "PreToolUse",
			session_id: "codex-session",
			tool_name: "Read",
			tool_input: { file_path: "src/a.ts" },
		};
		const deps = {
			deliver: async (_agent: "claude" | "codex", value: Record<string, unknown>) => {
				delivered.push(value);
				return { via: "rpc" as const };
			},
		};
		await ingestCodexHookPayload(payload, { host: "127.0.0.1", port: 38888 }, deps);
		await ingestCodexHookPayload(payload, { host: "127.0.0.1", port: 38888 }, deps);
		expect(delivered[0]?.timestamp).toEqual(expect.any(String));
		expect(delivered[1]?.timestamp).toEqual(expect.any(String));
		expect(delivered[0]?.codemem_generated_event_nonce).not.toBe(
			delivered[1]?.codemem_generated_event_nonce,
		);
	});
});
