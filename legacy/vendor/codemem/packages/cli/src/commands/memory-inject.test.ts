import { Command } from "commander";
import { afterEach, describe, expect, it, vi } from "vitest";

const request = vi.fn();
vi.mock("@codemem/mcp", () => ({
	createMcpRpcClient: () => ({ request }),
}));

import { memoryCommand } from "./memory.js";

afterEach(() => {
	request.mockReset();
	process.exitCode = 0;
	vi.restoreAllMocks();
});

async function parseInjectCommand(args: string[]): Promise<void> {
	const root = new Command("codemem").enablePositionalOptions().addCommand(memoryCommand);
	await root.parseAsync(["memory", "inject", ...args], { from: "user" });
}

describe("memory inject command", () => {
	it("prints raw pack text and forwards project and working-set flags", async () => {
		request.mockResolvedValue({
			ok: true,
			result: { pack: { pack_text: "RAW PACK BODY" } },
		});
		const log = vi.spyOn(console, "log").mockImplementation(() => {});
		vi.spyOn(console, "error").mockImplementation(() => {});

		await parseInjectCommand([
			"continue viewer health work",
			"--project",
			"codemem",
			"--working-set-file",
			"packages/ui/src/app.ts",
			"--token-budget",
			"90",
		]);

		expect(request).toHaveBeenCalledWith("POST /v1/context/pack", {
			requestId: expect.any(String),
			context: "continue viewer health work",
			limit: 10,
			tokenBudget: 90,
			filters: {
				project: "codemem",
				working_set_paths: ["packages/ui/src/app.ts"],
			},
			trace: false,
		});
		expect(log).toHaveBeenLastCalledWith("RAW PACK BODY");
	});

	it("omits project filters for all-projects inject requests", async () => {
		request.mockResolvedValue({ ok: true, result: { pack: { pack_text: "" } } });
		vi.spyOn(console, "log").mockImplementation(() => {});
		vi.spyOn(console, "error").mockImplementation(() => {});

		await parseInjectCommand(["query", "--all-projects"]);
		expect(request.mock.calls[0]?.[1]).toMatchObject({ filters: {} });
	});
});
