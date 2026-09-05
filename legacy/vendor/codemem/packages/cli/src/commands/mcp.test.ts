import { afterEach, describe, expect, it, vi } from "vitest";
import { mcpCommand } from "./mcp.js";

const stdioImportMock = vi.hoisted(() => vi.fn());

vi.mock("@codemem/mcp/stdio", () => {
	stdioImportMock(process.env.CODEMEM_DB);
	return {};
});

const originalDb = process.env.CODEMEM_DB;

describe("mcp command", () => {
	afterEach(() => {
		stdioImportMock.mockClear();
		process.exitCode = undefined;
		if (originalDb === undefined) delete process.env.CODEMEM_DB;
		else process.env.CODEMEM_DB = originalDb;
	});

	it("keeps stdio mode as the default command", () => {
		expect(mcpCommand.name()).toBe("mcp");
		expect(mcpCommand.summary()).toBe("Start the MCP stdio server");
	});

	it("does not expose HTTP mode after carve-out", () => {
		const httpCommand = mcpCommand.commands.find((command) => command.name() === "http");
		expect(httpCommand).toBeUndefined();
	});

	it("routes the database option to the daemon-backed stdio server", () => {
		expect(mcpCommand.options.some((option) => option.long === "--db-path")).toBe(true);
	});

	it("runs stdio mode by default", async () => {
		await mcpCommand.parseAsync(["--db-path", "/tmp/codemem-mcp.sqlite"], { from: "user" });
		expect(stdioImportMock).toHaveBeenCalledWith("/tmp/codemem-mcp.sqlite");
	});
});
