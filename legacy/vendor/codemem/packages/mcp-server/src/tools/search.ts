import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { rpcContent } from "../content.js";
import { buildFilters } from "../project-scope.js";
import { mcpRequestId } from "../rpc-client.js";
import { filterSchema } from "../schemas.js";
import type { ToolRegistrationContext } from "../tool-context.js";

function items(result: Record<string, unknown>): { items: unknown[] } {
	return { items: Array.isArray(result.items) ? result.items : [] };
}

export function registerSearchTools(server: McpServer, context: ToolRegistrationContext): void {
	const { client, defaultProject, requestScope } = context;

	server.registerTool(
		"memory_search",
		{
			description: "Search memories by text query. Returns full body text for each match.",
			inputSchema: {
				query: z.string().min(1).max(16_384).describe("Search query"),
				limit: z.number().int().min(1).max(50).default(5).describe("Max results"),
				...filterSchema,
			},
		},
		async (args, extra) => {
			const filters = buildFilters(args, defaultProject());
			return rpcContent(
				await client.request("POST /v1/search", {
					requestId: mcpRequestId(
						"memory_search",
						extra?.requestId,
						extra?.sessionId ?? requestScope,
					),
					mode: "search",
					query: args.query,
					limit: args.limit,
					...(filters ? { filters } : {}),
				}),
				(result) => ({
					items: Array.isArray(result.items)
						? result.items.map((value) => {
								if (!value || typeof value !== "object" || Array.isArray(value)) return value;
								const item = value as Record<string, unknown>;
								return {
									id: item.id,
									title: item.title,
									kind: item.kind,
									body: item.body_text,
									confidence: item.confidence,
									score: item.score,
									session_id: item.session_id,
									metadata: item.metadata,
								};
							})
						: [],
				}),
			);
		},
	);

	server.registerTool(
		"memory_search_index",
		{
			description:
				"Search memories by text query. Returns compact index entries without body text.",
			inputSchema: {
				query: z.string().min(1).max(16_384).describe("Search query"),
				limit: z.number().int().min(1).max(50).default(8).describe("Max results"),
				...filterSchema,
			},
		},
		async (args, extra) => {
			const filters = buildFilters(args, defaultProject());
			return rpcContent(
				await client.request("POST /v1/search", {
					requestId: mcpRequestId(
						"memory_search_index",
						extra?.requestId,
						extra?.sessionId ?? requestScope,
					),
					mode: "search_index",
					query: args.query,
					limit: args.limit,
					...(filters ? { filters } : {}),
				}),
				(result) => ({
					items: Array.isArray(result.items)
						? result.items.map((value) => {
								if (!value || typeof value !== "object" || Array.isArray(value)) return value;
								const entry = { ...(value as Record<string, unknown>) };
								delete entry.body;
								delete entry.body_text;
								return entry;
							})
						: [],
				}),
			);
		},
	);

	server.registerTool(
		"memory_explain",
		{
			description: "Explain search results with detailed scoring breakdown.",
			inputSchema: {
				query: z.string().min(1).max(16_384).optional().describe("Search query"),
				ids: z.array(z.number().int().positive()).max(200).optional().describe("Memory IDs"),
				limit: z.number().int().min(1).max(50).default(10).describe("Max results"),
				include_pack_context: z.boolean().default(false).describe("Include formatted pack context"),
				...filterSchema,
			},
		},
		async (args, extra) => {
			const filters = buildFilters(args, defaultProject());
			return rpcContent(
				await client.request("POST /v1/search", {
					requestId: mcpRequestId(
						"memory_explain",
						extra?.requestId,
						extra?.sessionId ?? requestScope,
					),
					mode: "explain",
					...(args.query ? { query: args.query } : {}),
					...(args.ids ? { ids: args.ids } : {}),
					limit: args.limit,
					includePackContext: args.include_pack_context,
					...(filters ? { filters } : {}),
				}),
				(result) => result.items ?? { items: [], errors: [] },
			);
		},
	);

	server.registerTool(
		"memory_recent",
		{
			description: "Return recent memories, newest first.",
			inputSchema: {
				limit: z.number().int().min(1).max(100).default(8).describe("Max results"),
				...filterSchema,
			},
		},
		async (args, extra) => {
			const filters = buildFilters(args, defaultProject());
			return rpcContent(
				await client.request("POST /v1/search", {
					requestId: mcpRequestId(
						"memory_recent",
						extra?.requestId,
						extra?.sessionId ?? requestScope,
					),
					mode: "recent",
					limit: args.limit,
					...(filters ? { filters } : {}),
				}),
				items,
			);
		},
	);

	server.registerTool(
		"memory_pack",
		{
			description: "Build a formatted memory pack from search results.",
			inputSchema: {
				context: z.string().min(1).max(16_384).describe("Context description to search for"),
				limit: z.number().int().min(1).max(50).optional().describe("Max items to include"),
				token_budget: z
					.number()
					.int()
					.min(0)
					.max(Number.MAX_SAFE_INTEGER)
					.optional()
					.describe("Maximum pack tokens"),
				trace: z.boolean().default(false).describe("Include retrieval trace"),
				...filterSchema,
			},
		},
		async (args, extra) => {
			const filters = buildFilters(args, defaultProject());
			return rpcContent(
				await client.request("POST /v1/context/pack", {
					requestId: mcpRequestId(
						"memory_pack",
						extra?.requestId,
						extra?.sessionId ?? requestScope,
					),
					context: args.context,
					...(args.limit === undefined ? {} : { limit: args.limit }),
					...(args.token_budget === undefined ? {} : { tokenBudget: args.token_budget }),
					trace: args.trace,
					...(filters ? { filters } : {}),
				}),
				(result) =>
					result.trace === undefined ? result.pack : { pack: result.pack, trace: result.trace },
			);
		},
	);
}
