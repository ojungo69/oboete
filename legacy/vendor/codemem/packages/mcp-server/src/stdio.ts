#!/usr/bin/env node

/**
 * @codemem/mcp — MCP stdio server bootstrap.
 *
 * Runs as a separate process spawned by the host (OpenCode/Claude).
 * Communicates with the sole-writer daemon and never opens SQLite.
 */

import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { createCodememMcpServer } from "./server.js";

async function main() {
	const server = createCodememMcpServer();
	await server.connect(new StdioServerTransport());
}

main().catch((err) => {
	console.error("codemem MCP server failed to start:", err);
	process.exit(1);
});
