import type { PackTrace } from "@codemem/core";
import { Command } from "commander";
import { afterEach, describe, expect, it, vi } from "vitest";

const request = vi.fn();
vi.mock("@codemem/mcp", () => ({
	createMcpRpcClient: () => ({ request }),
}));

import { packCommand, renderPackTrace } from "./pack.js";

const pack = {
	items: [{ id: 7, kind: "decision", title: "Use daemon RPC" }],
	pack_text: "RPC PACK",
	metrics: {
		total_items: 1,
		pack_tokens: 2,
		fallback_used: false,
		sources: { fts: 0, semantic: 0, fuzzy: 0 },
	},
};

const candidate: PackTrace["retrieval"]["candidates"][number] = {
	id: 7,
	rank: 1,
	kind: "decision",
	title: "Use daemon RPC",
	preview: "CLI commands use the daemon.",
	scores: {
		base_score: 0.75,
		combined_score: 0.9,
		recency: 0.1,
		kind_bonus: 0.1,
		quality_boost: 0,
		role_adjustment: 0,
		working_set_overlap: 0.5,
		query_path_overlap: 0,
		personal_bias: 0,
		shared_trust_penalty: 0,
		recap_penalty: 0,
		tasklike_penalty: 0,
		text_overlap: 2,
		tag_overlap: 1,
	},
	reasons: ["query match", "working-set overlap"],
	disposition: "selected",
	section: "summary",
	artifact_class: "derived_fact",
	inferred_role: "durable",
	role_reason: "decision",
};

const trace = {
	version: 1,
	inputs: {
		query: "continue work",
		project: "demo",
		working_set_files: ["src/index.ts"],
		token_budget: 90,
		limit: 10,
	},
	mode: { selected: "task", reasons: ["task-like query"] },
	retrieval: {
		candidate_count: 4,
		candidates: [
			candidate,
			{
				...candidate,
				id: 8,
				rank: 2,
				preview: "",
				scores: {
					...candidate.scores,
					base_score: null,
					combined_score: null,
					working_set_overlap: 0,
					text_overlap: 0,
					tag_overlap: 0,
				},
				reasons: [],
				disposition: "dropped",
				section: null,
			},
			{ ...candidate, id: 9, rank: 3, disposition: "deduped", section: null },
			{ ...candidate, id: 10, rank: 4, disposition: "trimmed", section: null },
		],
		omissions: [],
		degradations: [],
	},
	assembly: {
		deduped_ids: [9],
		collapsed_groups: [],
		compressed_clusters: [],
		trimmed_ids: [10],
		trim_reasons: ["token budget"],
		sections: { summary: [7], timeline: [], observations: [] },
	},
	output: {
		estimated_tokens: 2,
		truncated: true,
		section_counts: { summary: 1, timeline: 0, observations: 0 },
		pack_text: "RPC PACK",
	},
} as PackTrace;

afterEach(() => {
	request.mockReset();
	process.exitCode = 0;
	vi.restoreAllMocks();
});

async function parsePackCommand(args: string[]): Promise<void> {
	const root = new Command("codemem").enablePositionalOptions().addCommand(packCommand);
	await root.parseAsync(["pack", ...args], { from: "user" });
}

describe("pack command", () => {
	it("T031 keeps destination authority daemon-owned while project remains a filter", async () => {
		request.mockResolvedValue({ ok: true, result: { pack } });
		vi.spyOn(console, "log").mockImplementation(() => {});

		await parsePackCommand(["continue work", "--json", "--project", "demo"]);
		const body = request.mock.calls[0]?.[1] as Record<string, unknown>;
		expect(body.filters).toEqual({ project: "demo" });
		for (const key of [
			"executionLocation",
			"localTrust",
			"modelLocal",
			"providerPeerTrust",
			"repository",
			"repositoryIdentity",
		]) {
			expect(body, key).not.toHaveProperty(key);
		}
		expect(packCommand.options.map((option) => option.long)).not.toContain("--local-trust");
	});

	it("registers trace as a pack subcommand with shared options", () => {
		const nested = packCommand.commands.find((command) => command.name() === "trace");
		expect(nested?.options.map((option) => option.long)).toEqual(
			expect.arrayContaining([
				"--db-path",
				"--json",
				"--working-set-file",
				"--project",
				"--all-projects",
			]),
		);
	});

	it("supports the main pack commander path with json output", async () => {
		request.mockResolvedValue({ ok: true, result: { pack } });
		const log = vi.spyOn(console, "log").mockImplementation(() => {});

		await parsePackCommand([
			"continue work",
			"--json",
			"--project",
			"demo",
			"--token-budget",
			"90",
		]);

		expect(request).toHaveBeenCalledWith("POST /v1/context/pack", {
			requestId: expect.any(String),
			context: "continue work",
			limit: 10,
			tokenBudget: 90,
			filters: { project: "demo" },
			trace: false,
		});
		expect(JSON.parse(String(log.mock.calls.at(-1)?.[0]))).toMatchObject({
			pack_text: "RPC PACK",
		});

		const output = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
		await parsePackCommand(["continue work", "--all-projects"]);
		expect(output.mock.calls.flat().join("")).toContain("Use daemon RPC");
		expect(output.mock.calls.flat().join("")).toContain("RPC PACK");

		output.mockClear();
		request.mockResolvedValueOnce({
			ok: true,
			result: { pack: { ...pack, metrics: { ...pack.metrics, fallback_used: true } } },
		});
		await parsePackCommand(["continue work", "--all-projects"]);
		expect(output.mock.calls.flat().join("")).toContain("(fallback)");
		output.mockClear();
		request.mockResolvedValueOnce({ ok: true, result: { pack: { ...pack, items: [] } } });
		await parsePackCommand(["continue work", "--all-projects"]);
		expect(output.mock.calls.flat().join("")).toContain("No relevant memories found.");
	});

	it("routes trace flags to the trace subcommand after the positional context", async () => {
		request.mockResolvedValue({ ok: true, result: { pack, trace } });
		const log = vi.spyOn(console, "log").mockImplementation(() => {});

		await parsePackCommand(["trace", "continue work", "--json", "--all-projects"]);
		expect(request.mock.calls[0]?.[1]).toMatchObject({
			context: "continue work",
			filters: {},
			trace: true,
		});
		expect(JSON.parse(String(log.mock.calls.at(-1)?.[0]))).toMatchObject({
			version: 1,
			mode: { selected: "task" },
		});

		await parsePackCommand(["trace", "continue work", "--all-projects"]);
		expect(String(log.mock.calls.at(-1)?.[0])).toContain("1. [7] (decision) Use daemon RPC");
		expect(String(log.mock.calls.at(-1)?.[0])).toContain("Trimmed\n4. [10]");
	});

	it("emits structured json errors for pack failures", async () => {
		request.mockResolvedValue({
			ok: false,
			error: { code: "daemon_unavailable", message: "daemon down", retryable: true },
		});
		const log = vi.spyOn(console, "log").mockImplementation(() => {});

		await parsePackCommand(["continue work", "--json"]);
		expect(JSON.parse(String(log.mock.calls.at(-1)?.[0]))).toEqual({
			error: "daemon_unavailable",
			message: "daemon down",
		});
		expect(process.exitCode).toBe(1);

		process.exitCode = 0;
		const output = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
		await parsePackCommand(["continue work"]);
		expect(process.exitCode).toBe(1);
		expect(output.mock.calls.flat().join("")).toContain("daemon down");

		process.exitCode = 0;
		await parsePackCommand(["trace", "continue work", "--json"]);
		expect(JSON.parse(String(log.mock.calls.at(-1)?.[0]))).toEqual({
			error: "daemon_unavailable",
			message: "daemon down",
		});
		expect(process.exitCode).toBe(1);
		process.exitCode = 0;
		output.mockClear();
		await parsePackCommand(["trace", "continue work"]);
		expect(process.exitCode).toBe(1);
		expect(output.mock.calls.flat().join("")).toContain("daemon down");

		request.mockRejectedValueOnce("pack primitive rejection");
		process.exitCode = 0;
		await parsePackCommand(["continue work", "--json"]);
		expect(JSON.parse(String(log.mock.calls.at(-1)?.[0]))).toEqual({
			error: "pack_failed",
			message: "pack primitive rejection",
		});
		expect(process.exitCode).toBe(1);
		request.mockRejectedValueOnce(17);
		process.exitCode = 0;
		output.mockClear();
		await parsePackCommand(["continue work"]);
		expect(process.exitCode).toBe(1);
		expect(output.mock.calls.flat().join("")).toContain("17");

		request.mockRejectedValueOnce(true);
		process.exitCode = 0;
		await parsePackCommand(["trace", "continue work", "--json"]);
		expect(JSON.parse(String(log.mock.calls.at(-1)?.[0]))).toEqual({
			error: "pack_trace_failed",
			message: "true",
		});
		expect(process.exitCode).toBe(1);
		request.mockRejectedValueOnce(null);
		process.exitCode = 0;
		output.mockClear();
		await parsePackCommand(["trace", "continue work"]);
		expect(process.exitCode).toBe(1);
		expect(output.mock.calls.flat().join("")).toContain("null");

		const requestCount = request.mock.calls.length;
		process.exitCode = 0;
		await parsePackCommand(["continue work", "--compact", "--json"]);
		expect(JSON.parse(String(log.mock.calls.at(-1)?.[0]))).toMatchObject({
			error: "usage_error",
			message: expect.stringContaining("rendering overrides"),
		});
		expect(process.exitCode).toBe(2);
		process.exitCode = 0;
		output.mockClear();
		await parsePackCommand(["continue work", "--compact"]);
		expect(process.exitCode).toBe(2);
		expect(output.mock.calls.flat().join("")).toContain("rendering overrides");
		process.exitCode = 0;
		await parsePackCommand(["trace", "continue work", "--compact", "--json"]);
		expect(JSON.parse(String(log.mock.calls.at(-1)?.[0]))).toMatchObject({
			error: "usage_error",
		});
		expect(process.exitCode).toBe(2);
		process.exitCode = 0;
		output.mockClear();
		await parsePackCommand(["trace", "continue work", "--compact"]);
		expect(process.exitCode).toBe(2);
		expect(output.mock.calls.flat().join("")).toContain("rendering overrides");
		expect(request).toHaveBeenCalledTimes(requestCount);
	});

	it("renders grouped human-readable trace text", async () => {
		const rendered = renderPackTrace(trace);
		expect(rendered).toContain("Final pack\nRPC PACK");
		expect(rendered).toContain("combined=0.90 base=0.75 text=2 tag=1 working_set=0.50");
		expect(rendered).toContain("- deduped ids: 9");
		expect(rendered).toContain("- truncated: yes");
		expect(
			renderPackTrace({
				...trace,
				inputs: {
					...trace.inputs,
					sanitized_query: "continue safe work",
					working_set_files: [],
				},
			}),
		).toContain("- Sanitized query: continue safe work\n- Project: demo\n- Working set: (none)");
		expect(
			renderPackTrace({
				...trace,
				assembly: {
					...trace.assembly,
					deduped_ids: [],
					trimmed_ids: [],
					trim_reasons: [],
				},
				output: { ...trace.output, truncated: false },
			}),
		).toContain(
			"- deduped ids: (none)\n- trimmed ids: (none)\n- trim reasons: (none)\n- section counts: summary=1 timeline=0 observations=0\n- estimated tokens: 2\n- truncated: no",
		);

		const output = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
		await parsePackCommand(["continue work", "--limit", "0"]);
		expect(request).not.toHaveBeenCalled();
		expect(process.exitCode).toBe(2);
		expect(output.mock.calls.flat().join("")).toContain("limit must be a positive integer");
	});
});
