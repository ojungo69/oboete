import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { rpcContent } from "../content.js";
import { buildFilters } from "../project-scope.js";
import { mcpRequestId } from "../rpc-client.js";
import { filterSchema } from "../schemas.js";
import type { ToolRegistrationContext } from "../tool-context.js";

export function registerTimelineTools(server: McpServer, context: ToolRegistrationContext): void {
	const { client, defaultProject, requestScope } = context;

	server.registerTool(
		"memory_timeline",
		{
			description: "Get a chronological window of memories around an anchor (by ID or query).",
			inputSchema: {
				query: z.string().min(1).max(16_384).optional().describe("Search query to find anchor"),
				memory_id: z.number().int().positive().optional().describe("Anchor memory ID"),
				depth_before: z.number().int().min(0).max(100).default(3).describe("Items before anchor"),
				depth_after: z.number().int().min(0).max(100).default(3).describe("Items after anchor"),
				...filterSchema,
			},
		},
		async (args, extra) => {
			const filters = buildFilters(args, defaultProject());
			return rpcContent(
				await client.request("POST /v1/search", {
					requestId: mcpRequestId(
						"memory_timeline",
						extra?.requestId,
						extra?.sessionId ?? requestScope,
					),
					mode: "timeline",
					...(args.query ? { query: args.query } : {}),
					...(args.memory_id === undefined ? {} : { memoryId: args.memory_id }),
					depthBefore: args.depth_before,
					depthAfter: args.depth_after,
					...(filters ? { filters } : {}),
				}),
				(result) => ({ items: Array.isArray(result.items) ? result.items : [] }),
			);
		},
	);

	server.registerTool(
		"memory_expand",
		{
			description: "Fetch memories by ID with surrounding timeline context.",
			inputSchema: {
				// `[0-9]`, not `\d`: zod copies RegExp.source verbatim into the JSON
				// Schema `pattern` this tool publishes through tools/list, so the
				// spelling is part of the wire contract. Pinned in server.test.ts.
				ids: z
					.array(z.union([z.number().int().positive(), z.string().regex(/^[1-9][0-9]*$/)]))
					.min(1)
					.max(200)
					.describe("Memory IDs to expand"),
				depth_before: z.number().int().min(0).max(100).default(3).describe("Timeline items before"),
				depth_after: z.number().int().min(0).max(100).default(3).describe("Timeline items after"),
				...filterSchema,
			},
		},
		async (args, extra) => {
			const filterDefaultProject =
				args.project !== undefined && !args.project.trim() ? null : defaultProject();
			const filters = buildFilters(args, filterDefaultProject);
			return rpcContent(
				await client.request("POST /v1/search", {
					requestId: mcpRequestId(
						"memory_expand",
						extra?.requestId,
						extra?.sessionId ?? requestScope,
					),
					mode: "expand",
					ids: args.ids,
					depthBefore: args.depth_before,
					depthAfter: args.depth_after,
					...(filters ? { filters } : {}),
				}),
				(result) => ({ items: Array.isArray(result.items) ? result.items : [] }),
			);
		},
	);
}
